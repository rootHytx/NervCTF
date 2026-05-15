//! Docker Compose backend for instance provisioning.
//!
//! In split-machine mode (`runner_ssh_target` is set), all compose commands and
//! file writes happen on the remote runner host via SSH.  The monitor never
//! touches the local Docker daemon.

use anyhow::{anyhow, Context, Result};
use std::path::Path;

use super::ssh;

/// Return the runner SSH target if configured (split-machine mode).
fn runner_target() -> Option<String> {
    std::env::var("RUNNER_SSH_TARGET").ok()
        .or_else(|| {
            std::env::var("DOCKER_HOST").ok()
                .filter(|h| h.starts_with("ssh://"))
                .map(|h| h.trim_start_matches("ssh://").to_string())
        })
        .filter(|s| !s.is_empty())
}

/// Build a `docker compose` or `docker-compose` command depending on what is available.
///
/// Only used in single-machine mode (no SSH target).
pub async fn compose_cmd() -> tokio::process::Command {
    let available = tokio::process::Command::new("docker")
        .args(["compose", "version"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);

    if available {
        let mut cmd = tokio::process::Command::new("docker");
        cmd.arg("compose");
        cmd
    } else {
        tokio::process::Command::new("docker-compose")
    }
}

/// Bring up a compose project for a team instance.
///
/// `compose_file`   — path to the docker-compose.yml (on runner in split mode, local otherwise)
/// `project_name`   — unique project name (e.g. ctf-challenge-t42)
/// `internal_port`  — the container port to expose
/// `service`        — service name whose port to map (empty = "app")
/// `flag`           — optional per-instance flag value
/// `flag_delivery`  — `"env"` (default): FLAG exposed as a compose env var for
///                    `${FLAG}` substitution; `"file"`: flag written to a bind-mounted
///                    file at `flag_file_path` inside `flag_service`
/// `flag_file_path` — absolute path inside the container (required for `"file"` mode)
/// `flag_service`   — service that receives the flag file mount; defaults to `service`
/// `runner_ssh`     — override SSH target; falls back to `runner_target()` if `None`
///
/// Returns `(host_port, container_id_or_project)`.
pub async fn up(
    compose_file: &Path,
    project_name: &str,
    internal_port: u32,
    service: &str,
    used_ports: &std::collections::HashSet<u16>,
    flag: Option<&str>,
    flag_delivery: &str,
    flag_file_path: Option<&str>,
    flag_service: Option<&str>,
    runner_ssh: Option<&str>,
) -> Result<(u16, String)> {
    let host_port = crate::instance::docker::pick_free_port(used_ports)?;
    let svc_name = if service.is_empty() { "app" } else { service };

    let compose_dir = compose_file.parent().unwrap_or(Path::new("."));

    // Images were built with `-p <dir_name>`, so they are tagged `<dir_name>-<service>`.
    // We need `image:` overrides for every such service in the per-instance override file
    // so that Docker Compose uses the pre-built images instead of deriving a per-instance
    // project-based name (`<project>-<service>`) that doesn't exist.
    let challenge_dir_name = compose_dir
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_else(|| "ctf-challenge".to_string());

    // Resolve SSH target early — needed for both the image query and the compose operations.
    let effective_target = runner_ssh.map(|s| s.to_string()).or_else(runner_target);

    // Query all images on the runner/host that were pre-built for this challenge
    // (tagged `<dir_name>-<service>`).  We only add `image:` overrides for those that
    // actually exist, so public-image services (postgres, redis, …) are left untouched.
    let image_prefix = format!("{}-", challenge_dir_name);
    let pre_built_services: Vec<String> = {
        // `docker images --format '{{.Repository}}'` lists repo names (no tag).
        // We grep for lines starting with the challenge prefix to find all built services.
        let img_cmd = format!(
            "docker images --format '{{{{.Repository}}}}' | grep '^{}' | sort -u",
            image_prefix,
        );
        let raw = if let Some(ref target) = effective_target {
            ssh::output(target, &img_cmd).await.ok()
        } else {
            tokio::process::Command::new("sh")
                .args(["-c", &img_cmd])
                .output()
                .await
                .ok()
        };
        raw.filter(|o| o.status.success())
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .filter_map(|line| {
                        line.trim()
                            .strip_prefix(&image_prefix)
                            .map(|svc| svc.to_string())
                    })
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };

    // Determine flag file info up-front so we can embed volumes in the right service block.
    // Returns (host_path, bytes, target_service_name, container_path).
    let flag_info: Option<(String, Vec<u8>, String, String)> = if flag_delivery == "file" {
        match (flag, flag_file_path) {
            (Some(fval), Some(cpath)) => {
                let hp = compose_dir
                    .join(format!("{}.flag", project_name))
                    .display()
                    .to_string();
                let tsvc = flag_service.unwrap_or(svc_name).to_string();
                Some((hp, fval.as_bytes().to_vec(), tsvc, cpath.to_string()))
            }
            _ => None,
        }
    } else {
        None
    };

    // Build the per-instance override YAML.
    //
    // Structure:
    //   1. Main service (svc_name): image + ports + optional flag volume.
    //   2. All other pre-built services: image + optional flag volume.
    //   3. Flag-service stub if it differs from the main service and wasn't pre-built.
    let mut override_content = String::from("services:\n");

    // Helper closure — add image + optional ports/volumes for one service.
    // (Rust closures can't mutate `override_content` from within, so we inline the logic.)
    macro_rules! add_service {
        ($svc:expr) => {{
            let svc: &str = $svc;
            override_content.push_str(&format!("  {}:\n    image: {}-{}\n", svc, challenge_dir_name, svc));
            if svc == svc_name {
                override_content.push_str(&format!(
                    "    ports:\n      - \"{}:{}\"\n",
                    host_port, internal_port
                ));
            }
            if let Some((ref hp, _, ref tsvc, ref cp)) = flag_info {
                if tsvc.as_str() == svc {
                    override_content.push_str(&format!(
                        "    volumes:\n      - {}:{}:ro\n",
                        hp, cp
                    ));
                }
            }
        }};
    }

    // Main service first so any appended lines land in the right block.
    add_service!(svc_name);

    // All other pre-built services.
    for svc in &pre_built_services {
        if svc.as_str() == svc_name {
            continue;
        }
        add_service!(svc.as_str());
    }

    // If the flag service is neither the main service nor a pre-built service,
    // add a minimal stub that carries only the volume mount.
    if let Some((ref hp, _, ref tsvc, ref cp)) = flag_info {
        let tsvc_str = tsvc.as_str();
        if tsvc_str != svc_name && !pre_built_services.iter().any(|s| s.as_str() == tsvc_str) {
            override_content.push_str(&format!(
                "  {}:\n    volumes:\n      - {}:{}:ro\n",
                tsvc_str, hp, cp
            ));
        }
    }

    let flag_file_content: Option<(String, Vec<u8>)> =
        flag_info.map(|(hp, bytes, _, _)| (hp, bytes));

    let override_path = compose_dir.join(format!("{}.override.yml", project_name));
    let compose_file_str = compose_file.to_str().unwrap();
    let override_path_str = override_path.display().to_string();

    if let Some(ref target) = effective_target {
        // ── Split-machine mode: all writes and compose commands happen on the runner ──

        // Write flag file to runner
        if let Some((ref path, ref content)) = flag_file_content {
            ssh::write_file(target, path, content).await
                .with_context(|| format!("write flag file to runner: {}", path))?;
        }

        // Write override YAML to runner
        ssh::write_file(target, &override_path_str, override_content.as_bytes()).await
            .with_context(|| format!("write compose override to runner: {}", override_path_str))?;

        // Build the remote compose command — all path values are shell-quoted.
        let flag_env = if flag_delivery != "file" {
            flag.map(|f| format!("FLAG={} ", ssh::shell_quote(f)))
                .unwrap_or_default()
        } else {
            String::new()
        };

        let compose_dir_q = ssh::shell_quote(&compose_dir.display().to_string());
        let compose_file_q = ssh::shell_quote(compose_file_str);
        let override_path_q = ssh::shell_quote(&override_path_str);
        let project_name_q = ssh::shell_quote(project_name);

        let remote_cmd = format!(
            "cd {} && {}DOCKER_BUILDKIT=1 docker compose -f {} -f {} -p {} up -d --no-build --force-recreate",
            compose_dir_q,
            flag_env,
            compose_file_q,
            override_path_q,
            project_name_q,
        );

        let status = ssh::status(target, &remote_cmd).await
            .with_context(|| "failed to ssh to runner for docker compose up")?;

        if !status.success() {
            // Clean up override on runner (best effort)
            let _ = ssh::status(target, &format!("rm -f {}", ssh::shell_quote(&override_path_str))).await;
            return Err(anyhow!("docker compose up failed for project {} (on runner)", project_name));
        }
    } else {
        // ── Single-machine mode: local execution ──

        if !compose_file.exists() {
            return Err(anyhow!(
                "Compose file not found: {}",
                compose_file.display()
            ));
        }

        // Write flag file locally
        if let Some((ref path, ref content)) = flag_file_content {
            std::fs::write(path, content)?;
        }

        // Write override locally
        std::fs::write(&override_path, &override_content)
            .with_context(|| format!("write compose override to {}", override_path.display()))?;

        let mut cmd = compose_cmd().await;
        cmd.args([
            "-f", compose_file_str,
            "-f", override_path_str.as_str(),
            "-p", project_name,
            "up", "-d", "--no-build", "--force-recreate",
        ]);
        cmd.env("DOCKER_BUILDKIT", "1");
        if flag_delivery != "file" {
            if let Some(flag_value) = flag {
                cmd.env("FLAG", flag_value);
            }
        }
        let status = cmd.status().await
            .with_context(|| "failed to spawn docker compose (is docker installed in PATH?)")?;

        if !status.success() {
            let _ = std::fs::remove_file(&override_path);
            return Err(anyhow!("docker compose up failed for project {}", project_name));
        }
    }

    Ok((host_port, project_name.to_string()))
}

/// List all running compose project names that start with `ctf-`.
/// Used by the background expiry task to detect orphaned projects.
pub async fn list_ctf_projects() -> Vec<String> {
    let output = if let Some(target) = runner_target() {
        ssh::output(&target, "docker compose ls --all --format json").await
    } else {
        tokio::process::Command::new("docker")
            .args(["compose", "ls", "--all", "--format", "json"])
            .output()
            .await
    };

    let bytes = match output {
        Ok(o) if o.status.success() => o.stdout,
        _ => return vec![],
    };

    let parsed: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    parsed.as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|entry| entry["Name"].as_str().map(String::from))
        .filter(|name| name.starts_with("ctf-"))
        .collect()
}

/// Tear down a compose project.
///
/// `runner_ssh`    — override SSH target; falls back to `runner_target()` if `None`
/// `cleanup_dir`   — if provided, removes `<project_name>.override.yml` and
///                   `<project_name>.flag` from this directory after a successful
///                   `docker compose down` (best-effort, errors are ignored)
pub async fn down(
    project_name: &str,
    runner_ssh: Option<&str>,
    cleanup_dir: Option<&Path>,
) -> Result<()> {
    // Resolve which SSH target to use.
    let effective_target = runner_ssh.map(|s| s.to_string()).or_else(runner_target);

    let project_name_q = ssh::shell_quote(project_name);

    let status = if let Some(ref target) = effective_target {
        let remote_cmd = format!("docker compose -p {} down -v", project_name_q);
        ssh::status(target, &remote_cmd).await
            .with_context(|| "failed to ssh to runner for docker compose down")?
    } else {
        compose_cmd().await
            .args(["-p", project_name, "down", "-v"])
            .status()
            .await
            .with_context(|| "failed to spawn docker compose down")?
    };

    if !status.success() {
        return Err(anyhow!("docker compose down failed for project {}", project_name));
    }

    // Best-effort cleanup of per-instance files left on disk.
    if let Some(dir) = cleanup_dir {
        let override_file = dir.join(format!("{}.override.yml", project_name));
        let flag_file = dir.join(format!("{}.flag", project_name));

        if let Some(ref target) = effective_target {
            let override_q = ssh::shell_quote(&override_file.display().to_string());
            let flag_q = ssh::shell_quote(&flag_file.display().to_string());
            let _ = ssh::status(target, &format!("rm -f {} {}", override_q, flag_q)).await;
        } else {
            let _ = std::fs::remove_file(&override_file);
            let _ = std::fs::remove_file(&flag_file);
        }
    }

    Ok(())
}

/// Build images for a compose project on the runner (split mode) or locally.
///
/// `runner_ssh` — explicit SSH target; falls back to `runner_target()` if `None`.
pub async fn build(compose_file: &str, runner_ssh: Option<&str>) -> Result<()> {
    let effective_target = runner_ssh.map(|s| s.to_string()).or_else(runner_target);

    // Derive a stable project name from the compose file's parent directory so that
    // `docker compose build -p <name>` tags images as `<name>-<service>`, matching
    // the `image:` reference written by `up()` into every per-instance override.
    let project_name = Path::new(compose_file)
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_else(|| "ctf-challenge".to_string());

    if let Some(ref target) = effective_target {
        let compose_file_q = ssh::shell_quote(compose_file);
        let project_name_q = ssh::shell_quote(&project_name);
        let remote_cmd = format!(
            "DOCKER_BUILDKIT=1 docker compose -f {} -p {} build",
            compose_file_q, project_name_q,
        );
        let out = ssh::output(target, &remote_cmd).await
            .with_context(|| "failed to ssh to runner for docker compose build")?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(anyhow!("docker compose build failed for {}: {}", compose_file, stderr.trim()));
        }
    } else {
        let status = compose_cmd().await
            .args(["-f", compose_file, "-p", &project_name, "build"])
            .env("DOCKER_BUILDKIT", "1")
            .status()
            .await
            .with_context(|| "failed to spawn docker compose build")?;
        if !status.success() {
            return Err(anyhow!("docker compose build failed for {}", compose_file));
        }
    }
    Ok(())
}

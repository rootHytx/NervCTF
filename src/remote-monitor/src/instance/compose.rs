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

    // Build the per-instance override YAML.
    let mut override_content = format!(
        "services:\n  {svc}:\n    ports:\n      - \"{hp}:{ip}\"\n",
        svc = svc_name,
        hp = host_port,
        ip = internal_port,
    );

    let flag_file_content: Option<(String, Vec<u8>)> = if flag_delivery == "file" {
        if let (Some(flag_value), Some(container_path)) = (flag, flag_file_path) {
            let flag_host_path = compose_dir.join(format!("{}.flag", project_name));
            let flag_path_str = flag_host_path.display().to_string();

            let target_svc = flag_service.unwrap_or(svc_name);
            if target_svc == svc_name {
                override_content.push_str(&format!(
                    "    volumes:\n      - {}:{}:ro\n",
                    flag_path_str, container_path,
                ));
            } else {
                override_content.push_str(&format!(
                    "  {}:\n    volumes:\n      - {}:{}:ro\n",
                    target_svc, flag_path_str, container_path,
                ));
            }
            Some((flag_path_str, flag_value.as_bytes().to_vec()))
        } else {
            None
        }
    } else {
        None
    };

    let override_path = compose_dir.join(format!("{}.override.yml", project_name));
    let compose_file_str = compose_file.to_str().unwrap();
    let override_path_str = override_path.display().to_string();

    // Resolve which SSH target to use (explicit override wins).
    let effective_target = runner_ssh.map(|s| s.to_string()).or_else(runner_target);

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
            "cd {} && {}DOCKER_BUILDKIT=1 docker compose -f {} -f {} -p {} up -d --force-recreate",
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
            "up", "-d", "--force-recreate",
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

    if let Some(ref target) = effective_target {
        let compose_file_q = ssh::shell_quote(compose_file);
        let remote_cmd = format!("DOCKER_BUILDKIT=1 docker compose -f {} build", compose_file_q);
        let out = ssh::output(target, &remote_cmd).await
            .with_context(|| "failed to ssh to runner for docker compose build")?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(anyhow!("docker compose build failed for {}: {}", compose_file, stderr.trim()));
        }
    } else {
        let status = compose_cmd().await
            .args(["-f", compose_file, "build"])
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

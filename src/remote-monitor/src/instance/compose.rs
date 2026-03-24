//! Docker Compose backend for instance provisioning.

use anyhow::{anyhow, Result};
use std::path::Path;

/// Build a `docker compose` or `docker-compose` command depending on what is available.
///
/// Probes `docker compose version` once; falls back to the standalone `docker-compose` binary
/// if the plugin is not present in the current process's PATH.
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
/// `compose_file`   — path to the docker-compose.yml on the server
/// `project_name`   — unique project name (e.g. ctf-challenge-t42)
/// `internal_port`  — the container port to expose
/// `service`        — service name whose port to map (empty = "app")
/// `flag`           — optional per-instance flag value
/// `flag_delivery`  — `"env"` (default): FLAG exposed as a compose env var for
///                    `${FLAG}` substitution; `"file"`: flag written to a bind-mounted
///                    file at `flag_file_path` inside `flag_service`
/// `flag_file_path` — absolute path inside the container (required for `"file"` mode)
/// `flag_service`   — service that receives the flag file mount; defaults to `service`
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
) -> Result<(u16, String)> {
    if !compose_file.exists() {
        return Err(anyhow!(
            "Compose file not found: {}",
            compose_file.display()
        ));
    }

    let host_port = crate::instance::docker::pick_free_port(used_ports)?;

    let svc_name = if service.is_empty() { "app" } else { service };

    // Write a per-instance override that maps the port and optionally injects the flag
    let compose_dir = compose_file.parent().unwrap_or(Path::new("."));

    // Build the per-instance override YAML.
    // Always maps the entry-point service port.
    // For "file" delivery: also bind-mounts the flag file into the target service.
    let mut override_content = format!(
        "services:\n  {svc}:\n    ports:\n      - \"{hp}:{ip}\"\n",
        svc = svc_name,
        hp = host_port,
        ip = internal_port,
    );

    if flag_delivery == "file" {
        if let (Some(flag_value), Some(container_path)) = (flag, flag_file_path) {
            // Write flag to a project-scoped file in the compose dir (bind-mounted)
            let flag_host_path = compose_dir.join(format!("{}.flag", project_name));
            std::fs::write(&flag_host_path, flag_value)?;

            let target_svc = flag_service.unwrap_or(svc_name);
            if target_svc == svc_name {
                // Append volumes to the already-open service block
                override_content.push_str(&format!(
                    "    volumes:\n      - {}:{}:ro\n",
                    flag_host_path.display(),
                    container_path,
                ));
            } else {
                // Separate service block
                override_content.push_str(&format!(
                    "  {}:\n    volumes:\n      - {}:{}:ro\n",
                    target_svc,
                    flag_host_path.display(),
                    container_path,
                ));
            }
        }
    }

    let override_path = compose_dir.join(format!("{}.override.yml", project_name));
    std::fs::write(&override_path, override_content)?;

    let mut cmd = compose_cmd().await;
    cmd.args([
        "-f",
        compose_file.to_str().unwrap(),
        "-f",
        override_path.to_str().unwrap(),
        "-p",
        project_name,
        "up",
        "-d",
        "--force-recreate",
    ]);
    // For "env" delivery (default): expose FLAG to Docker Compose variable substitution
    // so challenge authors can use ${FLAG} in any service's environment block.
    if flag_delivery != "file" {
        if let Some(flag_value) = flag {
            cmd.env("FLAG", flag_value);
        }
    }
    let status = cmd.status().await?;

    if !status.success() {
        let _ = std::fs::remove_file(&override_path);
        return Err(anyhow!("docker compose up failed for project {}", project_name));
    }

    Ok((host_port, project_name.to_string()))
}

/// List all running compose project names that start with `ctf-`.
/// Used by the background expiry task to detect orphaned projects.
pub async fn list_ctf_projects() -> Vec<String> {
    let out = tokio::process::Command::new("docker")
        .args(["compose", "ls", "--all", "--format", "json"])
        .output()
        .await;
    let bytes = match out {
        Ok(o) if o.status.success() => o.stdout,
        _ => return vec![],
    };
    // Output is a JSON array: [{"Name":"...","Status":"...","ConfigFiles":"..."}]
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
pub async fn down(project_name: &str) -> Result<()> {
    let status = compose_cmd().await
        .args(["-p", project_name, "down", "-v"])
        .status()
        .await?;

    if !status.success() {
        return Err(anyhow!("docker compose down failed for project {}", project_name));
    }
    Ok(())
}

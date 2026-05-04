//! Docker backend for instance management.
//!
//! When `runner_ssh_target` is provided, all Docker commands execute on the
//! remote runner host via SSH instead of the local daemon.

use anyhow::{anyhow, Result};
use rand::Rng;
use tokio::io::AsyncWriteExt as _;

/// Find a free TCP port in the ephemeral range, avoiding ports already used by running instances.
///
/// Port availability is checked against the DB, not via bind(), because the monitor runs inside
/// a Docker container: bind() checks the container's network namespace, not the host where
/// challenge containers actually publish their ports.
pub fn pick_free_port(used_ports: &std::collections::HashSet<u16>) -> Result<u16> {
    let mut rng = rand::thread_rng();
    for _ in 0..200 {
        let port = rng.gen_range(40000u16..60000u16);
        if !used_ports.contains(&port) {
            return Ok(port);
        }
    }
    // Sequential fallback if the random range is crowded
    for port in 40000u16..60000u16 {
        if !used_ports.contains(&port) {
            return Ok(port);
        }
    }
    Err(anyhow!("No free ports available in range 40000-60000"))
}

/// Run `ssh -o ... <target> <cmd>` and return the output.
async fn ssh_output(target: &str, cmd: &str) -> std::io::Result<std::process::Output> {
    tokio::process::Command::new("ssh")
        .args([
            "-o", "StrictHostKeyChecking=no",
            "-o", "UserKnownHostsFile=/dev/null",
            "-o", "BatchMode=yes",
            target,
            cmd,
        ])
        .output()
        .await
}

/// Write `flag_value` to `flag_host_path` on the runner (split mode) or locally (single mode).
///
/// The parent directory is created automatically. This is used when `flag_delivery = "file"`
/// to write the per-team flag to a bind-mount source path before `docker run`.
pub async fn write_flag_file(flag_host_path: &str, flag_value: &str, runner_ssh: Option<&str>) -> Result<()> {
    if let Some(target) = runner_ssh {
        let parent = std::path::Path::new(flag_host_path)
            .parent()
            .and_then(|p| p.to_str())
            .unwrap_or("/tmp");
        let mut child = tokio::process::Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                "-o", "BatchMode=yes",
                target,
                &format!("mkdir -p '{}' && cat > '{}'", parent, flag_host_path),
            ])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| anyhow!("ssh write flag file: {}", e))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(flag_value.as_bytes()).await?;
            stdin.shutdown().await?;
        }
        let status = child.wait().await?;
        if !status.success() {
            return Err(anyhow!("ssh write flag file to {} failed", flag_host_path));
        }
    } else {
        if let Some(parent) = std::path::Path::new(flag_host_path).parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow!("create flag dir: {}", e))?;
        }
        std::fs::write(flag_host_path, flag_value.as_bytes())
            .map_err(|e| anyhow!("write flag file {}: {}", flag_host_path, e))?;
    }
    Ok(())
}

/// Start a Docker container and return its **name** (not the Docker hex ID).
///
/// `volumes` — list of `(host_path, container_path)` bind mounts; each is added as
/// `-v host_path:container_path:ro`.  Used for file-based flag delivery.
pub async fn run_container(
    image_tag: &str,
    container_name: &str,
    host_port: u16,
    internal_port: u32,
    command: Option<&str>,
    env_vars: &[(String, String)],
    volumes: &[(String, String)],
    runner_ssh: Option<&str>,
) -> Result<String> {
    let mut docker_args = vec![
        "run".to_string(),
        "-d".to_string(),
        "--name".to_string(),
        container_name.to_string(),
        "-p".to_string(),
        format!("{}:{}", host_port, internal_port),
        "--restart=unless-stopped".to_string(),
    ];
    for (k, v) in env_vars {
        docker_args.push("-e".to_string());
        docker_args.push(format!("{}={}", k, v));
    }
    for (host_path, container_path) in volumes {
        docker_args.push("-v".to_string());
        docker_args.push(format!("{}:{}:ro", host_path, container_path));
    }
    docker_args.push(image_tag.to_string());
    if let Some(cmd) = command {
        let words = shlex::split(cmd).unwrap_or_else(|| cmd.split_whitespace().map(String::from).collect());
        docker_args.extend(words);
    }

    let output = if let Some(target) = runner_ssh {
        let remote_cmd = format!("docker {}", shlex::try_join(docker_args.iter().map(|s| s.as_str())).unwrap_or_default());
        ssh_output(target, &remote_cmd).await
            .map_err(|e| anyhow!("ssh docker run failed to spawn: {}", e))?
    } else {
        tokio::process::Command::new("docker")
            .args(&docker_args)
            .output()
            .await
            .map_err(|e| anyhow!("docker run failed to spawn: {}", e))?
    };

    if !output.status.success() {
        return Err(anyhow!(
            "docker run failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // Return the container name rather than the Docker hex ID so that callers
    // and cleanup routines can derive the flag file path without a separate lookup.
    Ok(container_name.to_string())
}

/// Stop and remove a Docker container, and clean up any flag file written for it.
///
/// `container_id` may be a Docker hex ID or a container name (the latter is stored
/// by `run_container` now).  Both are accepted by `docker stop/rm`.
///
/// If a flag file exists at `/tmp/ctf-flags/{container_id}.flag` (written during
/// file-delivery provisioning) it is removed as a best-effort cleanup.
pub async fn remove_container(container_id: &str, runner_ssh: Option<&str>) -> Result<()> {
    // Best-effort flag file cleanup (no-op if file does not exist).
    let flag_path = format!("/tmp/ctf-flags/{}.flag", container_id);
    if let Some(target) = runner_ssh {
        let _ = ssh_output(target, &format!("rm -f '{}'", flag_path)).await;
    } else {
        let _ = std::fs::remove_file(&flag_path);
    }

    if let Some(target) = runner_ssh {
        // Stop + remove on runner
        let _ = ssh_output(target, &format!("docker stop --time 3 '{}'", container_id)).await;
        let output = ssh_output(target, &format!("docker rm -f '{}'", container_id)).await
            .map_err(|e| anyhow!("ssh docker rm failed: {}", e))?;
        if !output.status.success() {
            return Err(anyhow!(
                "docker rm failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    } else {
        // Stop with a short grace period (3s) before SIGKILL — best effort
        let _ = tokio::process::Command::new("docker")
            .args(["stop", "--time", "3", container_id])
            .output()
            .await;

        let output = tokio::process::Command::new("docker")
            .args(["rm", "-f", container_id])
            .output()
            .await
            .map_err(|e| anyhow!("docker rm failed: {}", e))?;

        if !output.status.success() {
            return Err(anyhow!(
                "docker rm failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }
    Ok(())
}

/// Build a Docker image from a tar.gz build context stored on disk.
pub async fn build_image(tar_path: &std::path::Path, image_tag: &str, runner_ssh: Option<&str>) -> Result<()> {
    let output = if let Some(target) = runner_ssh {
        // Pipe the tar through SSH to docker build on the runner
        let tar_file = std::fs::File::open(tar_path).map_err(|e| anyhow!("open tar: {}", e))?;
        tokio::process::Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                "-o", "BatchMode=yes",
                target,
                &format!("DOCKER_BUILDKIT=1 docker build -t '{}' -", image_tag),
            ])
            .stdin(std::process::Stdio::from(tar_file))
            .output()
            .await
            .map_err(|e| anyhow!("ssh docker build failed: {}", e))?
    } else {
        tokio::process::Command::new("docker")
            .args(["build", "-t", image_tag, "-"])
            .stdin(std::process::Stdio::from(
                std::fs::File::open(tar_path).map_err(|e| anyhow!("open tar: {}", e))?,
            ))
            .output()
            .await
            .map_err(|e| anyhow!("docker build failed: {}", e))?
    };

    if !output.status.success() {
        return Err(anyhow!(
            "docker build failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

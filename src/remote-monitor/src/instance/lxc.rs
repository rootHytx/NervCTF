use anyhow::{anyhow, Result};

pub async fn launch(
    lxc_image: &str,
    container_name: &str,
    host_port: u16,
    internal_port: u32,
    flag: Option<&str>,
) -> Result<String> {
    // Remove any stale container with the same name
    let _ = delete(container_name).await;

    // Launch
    let output = tokio::process::Command::new("lxc")
        .args(["launch", lxc_image, container_name])
        .output()
        .await
        .map_err(|e| anyhow!("lxc launch failed to spawn: {}", e))?;

    if !output.status.success() {
        return Err(anyhow!(
            "lxc launch failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // Wait for the container to be fully running before adding devices or exec-ing
    let wait = tokio::process::Command::new("lxc")
        .args(["wait", container_name, "--state=Running", "--timeout=30"])
        .output()
        .await
        .map_err(|e| anyhow!("lxc wait failed: {}", e))?;

    if !wait.status.success() {
        let _ = delete(container_name).await;
        return Err(anyhow!(
            "container {} did not reach Running state in 30s: {}",
            container_name,
            String::from_utf8_lossy(&wait.stderr)
        ));
    }

    // Add proxy device: host_port → internal_port
    let output = tokio::process::Command::new("lxc")
        .args([
            "config", "device", "add",
            container_name,
            "ctfport",
            "proxy",
            &format!("listen=tcp:0.0.0.0:{}", host_port),
            &format!("connect=tcp:127.0.0.1:{}", internal_port),
        ])
        .output()
        .await
        .map_err(|e| anyhow!("lxc config device add failed: {}", e))?;

    if !output.status.success() {
        let _ = delete(container_name).await;
        return Err(anyhow!(
            "lxc port forward setup failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // Inject flag into /challenge/flag if provided.
    // printf is used instead of echo to avoid interpretation of escape sequences.
    if let Some(flag_value) = flag {
        let cmd = format!("printf '%s\\n' '{}' > /challenge/flag && chmod 444 /challenge/flag", flag_value);
        let output = tokio::process::Command::new("lxc")
            .args(["exec", container_name, "--", "/bin/sh", "-c", &cmd])
            .output()
            .await
            .map_err(|e| anyhow!("lxc exec (flag inject) failed to spawn: {}", e))?;

        if !output.status.success() {
            eprintln!(
                "  lxc: warning: flag injection failed for {}: {}",
                container_name,
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    Ok(container_name.to_string())
}

pub async fn delete(container_name: &str) -> Result<()> {
    // Force-stop (no-op if already stopped or does not exist)
    let _ = tokio::process::Command::new("lxc")
        .args(["stop", "--force", container_name])
        .output()
        .await;

    // Delete (--force skips the "container is running" guard)
    let output = tokio::process::Command::new("lxc")
        .args(["delete", "--force", container_name])
        .output()
        .await
        .map_err(|e| anyhow!("lxc delete failed to spawn: {}", e))?;

    if !output.status.success() {
        return Err(anyhow!(
            "lxc delete failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(())
}

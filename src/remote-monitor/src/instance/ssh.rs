//! Shared SSH helpers used by the compose and docker backends.
//!
//! All functions run commands on a remote host via `ssh -o BatchMode=yes`.

use anyhow::{anyhow, Context, Result};
use tokio::io::AsyncWriteExt as _;

/// Run `cmd` on `target` via SSH and return the full output (stdout + stderr).
pub async fn output(target: &str, cmd: &str) -> std::io::Result<std::process::Output> {
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

/// Run `cmd` on `target` via SSH and return only the exit status.
pub async fn status(target: &str, cmd: &str) -> std::io::Result<std::process::ExitStatus> {
    tokio::process::Command::new("ssh")
        .args([
            "-o", "StrictHostKeyChecking=no",
            "-o", "UserKnownHostsFile=/dev/null",
            "-o", "BatchMode=yes",
            target,
            cmd,
        ])
        .status()
        .await
}

/// Write `content` to `remote_path` on `target` via SSH (piped through stdin).
///
/// The parent directory of `remote_path` is created automatically with `mkdir -p`.
pub async fn write_file(target: &str, remote_path: &str, content: &[u8]) -> Result<()> {
    let parent = std::path::Path::new(remote_path)
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or(".");

    // Single-quote escape both paths so special characters don't break the shell command.
    let parent_q = shell_quote(parent);
    let path_q = shell_quote(remote_path);

    let mut child = tokio::process::Command::new("ssh")
        .args([
            "-o", "StrictHostKeyChecking=no",
            "-o", "UserKnownHostsFile=/dev/null",
            "-o", "BatchMode=yes",
            target,
            &format!("mkdir -p {} && cat > {}", parent_q, path_q),
        ])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("ssh write to {}", remote_path))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(content).await?;
        stdin.shutdown().await?;
    }

    let st = child.wait().await?;
    if !st.success() {
        return Err(anyhow!("ssh write to {} failed", remote_path));
    }
    Ok(())
}

/// POSIX single-quote escape a string so it is safe to embed in an SSH shell command.
///
/// Wraps the value in single quotes and replaces every internal `'` with `'\''`.
pub fn shell_quote(s: &str) -> String {
    // shlex::try_quote handles this correctly (including the empty-string edge case).
    shlex::try_quote(s)
        .map(|cow| cow.into_owned())
        .unwrap_or_else(|_| format!("'{}'", s.replace('\'', "'\\''")))
}

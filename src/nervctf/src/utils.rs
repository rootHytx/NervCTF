use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Declarative CTFd event settings applied with `nervctf event apply`.
/// Field names are the friendly YAML names; `run_event` maps them to CTFd
/// `configs` keys (`name` -> `ctf_name`, and so on).
#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub struct EventConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub challenge_visibility: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registration_visibility: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score_visibility: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_visibility: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verify_emails: Option<bool>,
}

/// First CTFd admin account. Used once to complete CTFd's first-run setup; the
/// plugin ignores it after `setup` is true.
#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub struct AdminConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

/// Configuration loaded from `.nervctf.yml` (merged with env vars and CLI flags).
#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub struct Config {
    // CTFd/monitor host
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitor_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitor_port: Option<String>,
    // monitor_url is derived at runtime as http://{monitor_ip}:{monitor_port}
    // and is no longer persisted to file.

    // Authentication
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitor_token: Option<String>,

    // Deployment credentials
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitor_user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitor_ctfd_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh_key_path: Option<String>,

    // Local challenge directory
    #[serde(skip_serializing_if = "Option::is_none")]
    pub challenges_path: Option<String>,

    // Monitor tuning
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrent_provisions: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_instances_per_team: Option<u32>,
    // Override the CTFd domain used in admin dashboard links (defaults to monitor host).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ctfd_domain: Option<String>,

    // Split-machine mode: separate host for running challenge containers
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner_user: Option<String>,
    // Domain shown to players instead of the runner IP (backend still uses the IP).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner_domain: Option<String>,

    // Declarative event settings (applied with `nervctf event apply`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<EventConfig>,

    // First CTFd admin account (applied by `nervctf setup` via the plugin).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admin: Option<AdminConfig>,
}

/// Walk up from `start_dir` looking for `.nervctf.yml` or `.nervctf.yaml`.
/// Returns the parsed config and the path it was found at, or `(Config::default(), None)`
/// if no file exists.
pub fn load_config(start_dir: &Path) -> (Config, Option<PathBuf>) {
    let mut dir = start_dir.to_path_buf();
    if let Ok(abs) = dir.canonicalize() {
        dir = abs;
    }
    loop {
        for name in &[".nervctf.yml", ".nervctf.yaml"] {
            let path = dir.join(name);
            if !path.exists() {
                continue;
            }
            let content = match fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))
            {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[!] {}", e);
                    return (Config::default(), None);
                }
            };
            match serde_yaml::from_str::<Config>(&content) {
                Ok(cfg) => return (cfg, Some(path)),
                Err(e) => {
                    eprintln!();
                    eprintln!("######################################################");
                    eprintln!("[ERROR] Failed to parse config file: {}", path.display());
                    eprintln!("[ERROR] Parse error: {}", e);
                    eprintln!("######################################################");
                    eprintln!();
                    eprintln!("Fix or delete the file above, then re-run nervctf.");
                    std::process::exit(1);
                }
            }
        }
        if !dir.pop() {
            break;
        }
    }
    (Config::default(), None)
}

/// Find an existing `.nervctf.yml` walking up from `start_dir`,
/// or return `start_dir/.nervctf.yml` as the path to create.
pub fn find_config_path(start_dir: &Path) -> PathBuf {
    let mut dir = start_dir.to_path_buf();
    loop {
        for name in &[".nervctf.yml", ".nervctf.yaml"] {
            let p = dir.join(name);
            if p.exists() {
                return p;
            }
        }
        if !dir.pop() {
            break;
        }
    }
    start_dir.join(".nervctf.yml")
}

/// Serialize `config` to YAML and write it to `path`.
pub fn save_config(config: &Config, path: &Path) -> Result<()> {
    let content = serde_yaml::to_string(config).context("serializing config")?;
    fs::write(path, content)?;
    // The file may hold the first-admin password; keep it owner-only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

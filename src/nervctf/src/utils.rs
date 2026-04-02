use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Configuration loaded from `.nervctf.yml` (merged with env vars and CLI flags).
#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub struct Config {
    // Remote monitor
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitor_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitor_token: Option<String>,

    // Challenge base directory
    #[serde(skip_serializing_if = "Option::is_none")]
    pub challenges_dir: Option<String>,

    // Setup / deployment fields
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh_pubkey_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ctfd_remote_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitor_port: Option<String>,

    // Monitor tuning
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrent_provisions: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_instances_per_team: Option<u32>,

    // Split-machine mode: separate host for running challenge containers
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner_user: Option<String>,
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
                    eprintln!("[!] {}: failed to parse: {}", path.display(), e);
                    return (Config::default(), None);
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
    Ok(())
}

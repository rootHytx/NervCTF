use anyhow::{anyhow, Result};
use dialoguer::{Confirm, Input, Select};
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::Command;
use tempfile::tempdir;

use crate::{find_config_path, load_config, save_config};

const PLAYBOOK: &str = include_str!("../assets/nervctf_playbook.yml");

/// Walk up from cwd until we find a flake.nix.
fn find_flake_nix() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join("flake.nix").exists() {
            return Some(dir.join("flake.nix"));
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn list_ssh_pubkeys() -> Vec<PathBuf> {
    let ssh_dir = home_dir().join(".ssh");
    fs::read_dir(&ssh_dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().map(|e| e == "pub").unwrap_or(false))
                .collect()
        })
        .unwrap_or_default()
}

fn generate_ssh_key() -> Result<PathBuf> {
    let key_path = home_dir().join(".ssh").join("nervctf_ansible_id_rsa");
    let status = Command::new("ssh-keygen")
        .args([
            "-t",
            "rsa",
            "-b",
            "4096",
            "-f",
            key_path.to_str().unwrap(),
            "-N",
            "",
        ])
        .status()?;
    if !status.success() {
        return Err(anyhow!("ssh-keygen failed"));
    }
    Ok(key_path.with_extension("pub"))
}

fn generate_token() -> Result<String> {
    let mut buf = [0u8; 32];
    let mut f = fs::File::open("/dev/urandom")
        .map_err(|e| anyhow!("Failed to open /dev/urandom: {}", e))?;
    f.read_exact(&mut buf)
        .map_err(|e| anyhow!("Failed to read /dev/urandom: {}", e))?;
    Ok(buf.iter().map(|b| format!("{:02x}", b)).collect())
}

fn find_workspace_root() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let toml = dir.join("Cargo.toml");
        if toml.exists() {
            if let Ok(content) = fs::read_to_string(&toml) {
                if content.contains("[workspace]") {
                    return Some(dir);
                }
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn find_monitor_binary() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("remote-monitor");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    if let Some(root) = find_workspace_root() {
        // Prefer musl static builds (portable, no NixOS interpreter path issues)
        let targets = [
            "x86_64-unknown-linux-musl",
            "aarch64-unknown-linux-musl",
            "",
        ];
        for profile in &["release", "debug"] {
            for target in &targets {
                let candidate = if target.is_empty() {
                    root.join("target").join(profile).join("remote-monitor")
                } else {
                    root.join("target").join(target).join(profile).join("remote-monitor")
                };
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

fn find_plugin_src() -> Option<PathBuf> {
    // Check next to the current executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("ctfd-plugin");
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }
    // Check in the workspace source tree (dev mode)
    if let Some(root) = find_workspace_root() {
        let candidate = root
            .join("src")
            .join("nervctf")
            .join("assets")
            .join("ctfd-plugin");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

fn check_ctfd_running(ip: &str) -> bool {
    let addr = format!("{}:80", ip);
    if let Ok(parsed) = addr.parse::<std::net::SocketAddr>() {
        std::net::TcpStream::connect_timeout(&parsed, std::time::Duration::from_secs(3)).is_ok()
    } else {
        false
    }
}

fn prompt_with_default(prompt: &str, default: Option<&str>) -> Result<String> {
    let mut builder = Input::new().with_prompt(prompt);
    if let Some(d) = default {
        builder = builder.default(d.to_string());
    }
    Ok(builder.interact_text()?)
}

pub fn run_setup() -> Result<()> {
    println!("==============================================");
    println!(" NervCTF Setup: Automated CTFd Environment");
    println!("----------------------------------------------");
    println!("This will:");
    println!(" - Install Docker and CTFd on the remote machine");
    println!(" - Configure SSH access for the deployment user");
    println!(" - Deploy the Remote Monitor");
    println!("==============================================\n");

    let cwd = std::env::current_dir()?;
    let config_path = find_config_path(&cwd);
    let (mut config, existing_path) = load_config(&cwd);

    if let Some(ref p) = existing_path {
        println!("Using config: {}", p.display());
    } else {
        println!("No .nervctf.yml found -- will create {}", config_path.display());
    }
    println!();

    // ── CHALLENGES BASE_DIR ────────────────────────────────────────────────────
    let base_dir = prompt_with_default(
        "Local challenges directory",
        Some(config.base_dir.as_deref().unwrap_or(".")),
    )?;

    // ── TARGET_IP ──────────────────────────────────────────────────────────────
    let target_ip = {
        let default = config.target_ip.as_deref();
        if let Some(ip) = default {
            println!("Target IP [{}]: ", ip);
        }
        let ip = prompt_with_default("Target machine IP address", default)?;
        if ip.trim().is_empty() {
            return Err(anyhow!("IP address is required"));
        }
        ip
    };

    // ── TARGET_USER ────────────────────────────────────────────────────────────
    let target_user = prompt_with_default(
        "Remote sudo user",
        Some(config.target_user.as_deref().unwrap_or("root")),
    )?;

    // ── CTFD_REMOTE_PATH ───────────────────────────────────────────────────────
    let ctfd_path = prompt_with_default(
        "CTFd installation path on remote",
        Some(
            config
                .ctfd_remote_path
                .as_deref()
                .unwrap_or("/home/docker/CTFd"),
        ),
    )?;

    // ── MONITOR_PORT ───────────────────────────────────────────────────────────
    let monitor_port = prompt_with_default(
        "Remote Monitor port",
        Some(config.monitor_port.as_deref().unwrap_or("33133")),
    )?;

    // ── MONITOR_TOKEN ──────────────────────────────────────────────────────────
    let monitor_token = if let Some(ref token) = config.monitor_token {
        println!(
            "Using existing monitor token ({}...)",
            &token[..8.min(token.len())]
        );
        token.clone()
    } else {
        println!("Generating new monitor token...");
        generate_token()?
    };

    // ── SSH key ────────────────────────────────────────────────────────────────
    let ssh_pubkey_path = if let Some(ref key) = config.ssh_pubkey_path {
        println!("Using existing SSH public key: {}", key);
        key.clone()
    } else {
        let pubkeys = list_ssh_pubkeys();
        let key_path = if pubkeys.is_empty() {
            println!("No SSH public keys found in ~/.ssh.");
            let generate = Confirm::new()
                .with_prompt("Generate a new SSH key?")
                .default(true)
                .interact()?;
            if generate {
                generate_ssh_key()?
            } else {
                return Err(anyhow!("No SSH key selected"));
            }
        } else {
            let mut options: Vec<String> =
                pubkeys.iter().map(|p| p.display().to_string()).collect();
            options.push("Generate new key".to_string());
            let selection = Select::new()
                .with_prompt("Select SSH public key")
                .items(&options)
                .default(0)
                .interact()?;
            if selection == pubkeys.len() {
                generate_ssh_key()?
            } else {
                pubkeys[selection].clone()
            }
        };
        key_path.to_string_lossy().to_string()
    };

    // ── Save config before deployment ─────────────────────────────────────────
    config.base_dir = Some(base_dir.clone());
    config.target_ip = Some(target_ip.clone());
    config.target_user = Some(target_user.clone());
    config.ctfd_remote_path = Some(ctfd_path.clone());
    config.monitor_port = Some(monitor_port.clone());
    config.monitor_token = Some(monitor_token.clone());
    config.ssh_pubkey_path = Some(ssh_pubkey_path.clone());
    let ctfd_url = format!("http://{}", target_ip);
    let monitor_url = format!("http://{}:{}", target_ip, monitor_port);
    config.ctfd_url = Some(ctfd_url.clone());
    config.monitor_url = Some(monitor_url.clone());

    save_config(&config, &config_path)?;
    println!("  [ok] config saved to {}", config_path.display());

    // ── Check if already running ───────────────────────────────────────────────
    println!("\nChecking if CTFd is already running at {}...", target_ip);
    if check_ctfd_running(&target_ip) {
        println!("  CTFd appears to already be running at {}:80.", target_ip);
        let redeploy = Confirm::new()
            .with_prompt("Redeploy anyway? (runs the Ansible playbook again)")
            .default(false)
            .interact()?;
        if !redeploy {
            prompt_and_save_api_key(&mut config, &config_path)?;
            println!("\nCTFd URL:      {}", ctfd_url);
            println!("Monitor URL:   {}", monitor_url);
            println!("Monitor Token: {}", monitor_token);
            return Ok(());
        }
    } else {
        println!("  CTFd not detected -- proceeding with fresh deployment.");
    }

    // ── Find remote-monitor binary ─────────────────────────────────────────────
    let monitor_binary = find_monitor_binary();
    if monitor_binary.is_none() {
        println!(
            "\n[!] remote-monitor binary not found. Build it first with:\n\
             cargo build --release --target x86_64-unknown-linux-musl -p remote-monitor\n\
             The Remote Monitor will NOT be deployed in this run."
        );
    } else {
        println!(
            "  Found remote-monitor binary: {}",
            monitor_binary.as_ref().unwrap().display()
        );
    }

    // ── Find CTFd plugin source ────────────────────────────────────────────────
    let plugin_src = find_plugin_src();
    match &plugin_src {
        Some(p) => println!("  Found CTFd plugin: {}", p.display()),
        None => println!("  [!] CTFd plugin not found -- plugin will not be deployed."),
    }

    // ── Extract embedded assets to a temp dir ─────────────────────────────────
    let tmp = tempdir()?;
    let playbook_path = tmp.path().join("nervctf_playbook.yml");
    let inventory_path = tmp.path().join("inventory.ini");

    fs::write(&playbook_path, PLAYBOOK)?;
    fs::write(
        &inventory_path,
        format!("[ctfd]\n{} ansible_user={}\n", target_ip, target_user),
    )?;

    // ── Build ansible extra-vars ───────────────────────────────────────────────
    let mut evars: Vec<String> = vec![
        format!("ssh_key={}", ssh_pubkey_path),
        format!("ctfd_path={}", ctfd_path),
        format!("monitor_token={}", monitor_token),
        format!("monitor_port={}", monitor_port),
    ];
    if let Some(ref bin) = monitor_binary {
        evars.push(format!("monitor_binary={}", bin.display()));
    }
    if let Some(ref plugin) = plugin_src {
        evars.push(format!("plugin_src={}", plugin.display()));
    }
    if let Some(ref key) = config.ctfd_api_key {
        evars.push(format!("ctfd_api_key={}", key));
    }

    let mut ansible_args: Vec<String> = vec![
        "-i".to_string(),
        inventory_path.to_str().unwrap().to_string(),
        playbook_path.to_str().unwrap().to_string(),
    ];
    for ev in &evars {
        ansible_args.push("-e".to_string());
        ansible_args.push(ev.clone());
    }

    println!("\nRunning Ansible playbook...");

    let status = match Command::new("ansible-playbook").args(&ansible_args).status() {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let flake_nix = find_flake_nix().ok_or_else(|| {
                anyhow!(
                    "ansible-playbook not found in PATH and no flake.nix found. \
                     Install ansible or run inside nix develop."
                )
            })?;
            let flake_dir = flake_nix.parent().unwrap();
            println!(
                "  (ansible-playbook not in PATH -- using nix develop at {})",
                flake_dir.display()
            );
            Command::new("nix")
                .arg("develop")
                .arg(flake_dir.to_str().unwrap())
                .arg("--command")
                .arg("ansible-playbook")
                .args(&ansible_args)
                .status()?
        }
        Err(e) => return Err(e.into()),
    };

    if !status.success() {
        return Err(anyhow!(
            "ansible-playbook failed with exit code {:?}",
            status.code()
        ));
    }

    prompt_and_save_api_key(&mut config, &config_path)?;

    println!("\nNervCTF setup complete!");
    println!("  CTFd URL:      {}", ctfd_url);
    println!("  Monitor URL:   {}", monitor_url);
    println!("  Monitor Token: {}", monitor_token);
    println!("  Config:        {}", config_path.display());
    if monitor_binary.is_none() {
        println!("\n[!] remember to build and deploy the Remote Monitor:");
        println!("   cargo build --release --target x86_64-unknown-linux-musl -p remote-monitor");
        println!("   Then re-run `nervctf setup` to deploy it.");
    }
    Ok(())
}

/// Prompt for the CTFd API key and save it to config if provided.
fn prompt_and_save_api_key(config: &mut crate::Config, config_path: &std::path::Path) -> Result<()> {
    if config.ctfd_api_key.is_some() {
        println!("\nCTFd API key already set in config.");
        return Ok(());
    }
    println!("\nEnter your CTFd admin API key (leave blank to set it later):");
    let key: String = Input::new()
        .with_prompt("CTFd API key")
        .allow_empty(true)
        .interact_text()?;
    if !key.trim().is_empty() {
        config.ctfd_api_key = Some(key.trim().to_string());
        save_config(config, config_path)?;
        println!("  [ok] API key saved to {}", config_path.display());
    } else {
        println!("  Skipped. Set ctfd_api_key in {} when ready.", config_path.display());
    }
    Ok(())
}

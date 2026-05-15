use anyhow::{anyhow, Result};
use dialoguer::{Confirm, Input, Select};
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::Command;
use tempfile::tempdir;

use crate::{find_config_path, load_config, save_config};

const PLAYBOOK: &str = include_str!("../assets/nervctf_playbook.yml");
const UPGRADE_PLAYBOOK: &str = include_str!("../assets/nervctf_upgrade_playbook.yml");

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

fn workspace_root_from(start: &std::path::Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
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

fn find_workspace_root() -> Option<PathBuf> {
    // Try from the current working directory first (normal dev workflow).
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(root) = workspace_root_from(&cwd) {
            return Some(root);
        }
    }
    // Fall back to walking up from the nervctf executable's location
    // (covers the case where the user runs dist/nervctf-* from outside the repo).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            if let Some(root) = workspace_root_from(exe_dir) {
                return Some(root);
            }
        }
    }
    None
}

fn find_monitor_binary() -> Option<PathBuf> {
    // Names to search for, in preference order (musl static first — no interpreter issues)
    const NAMES: &[&str] = &[
        "remote-monitor",
        "remote-monitor-linux-x86_64-static",
        "remote-monitor-linux-x86_64",
        "remote-monitor-linux-aarch64-static",
        "remote-monitor-linux-aarch64",
    ];

    // 1. Next to the running nervctf exe (covers dist/ when user runs dist/nervctf-*)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in NAMES {
                let candidate = dir.join(name);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
    }

    // 2. Cargo build output (covers dev workflow where user runs from workspace root)
    if let Some(root) = find_workspace_root() {
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


/// Resolve a hostname to an IP address using the CLI machine's DNS stack.
/// Returns the original string unchanged if it is already an IP or resolution fails.
fn resolve_host_to_ip(host: &str) -> String {
    use std::net::{IpAddr, ToSocketAddrs};
    if host.parse::<IpAddr>().is_ok() {
        return host.to_string();
    }
    match format!("{}:22", host).to_socket_addrs() {
        Ok(mut addrs) => addrs
            .next()
            .map(|a| a.ip().to_string())
            .unwrap_or_else(|| {
                eprintln!(
                    "  [!] DNS lookup for '{}' returned no addresses — using hostname as-is",
                    host
                );
                host.to_string()
            }),
        Err(e) => {
            eprintln!(
                "  [!] Could not resolve '{}': {} — using hostname as-is (may fail inside Docker)",
                host, e
            );
            host.to_string()
        }
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

    // ── CHALLENGES_PATH ────────────────────────────────────────────────────────
    let base_dir = loop {
        let input = prompt_with_default(
            "Local challenges directory",
            Some(config.challenges_path.as_deref().unwrap_or(".")),
        )?;
        let p = std::path::Path::new(&input);
        if p == std::path::Path::new(".") || p.is_dir() {
            break input;
        }
        println!("  [!] '{}' is not an existing directory. Try again.", input);
    };

    // ── MONITOR_IP ────────────────────────────────────────────────────────────
    let monitor_ip = {
        let default = config.monitor_ip.as_deref();
        if let Some(ip) = default {
            println!("Monitor IP [{}]: ", ip);
        }
        let ip = prompt_with_default("CTFd/monitor machine IP address", default)?;
        if ip.trim().is_empty() {
            return Err(anyhow!("IP address is required"));
        }
        ip
    };

    // ── MONITOR_USER ──────────────────────────────────────────────────────────
    let monitor_user = prompt_with_default(
        "Remote sudo user",
        Some(config.monitor_user.as_deref().unwrap_or("root")),
    )?;

    // ── RUNNER (split-machine mode, optional) ─────────────────────────────────
    let runner_ip_input = prompt_with_default(
        "Challenge runner IP (blank = same machine as CTFd)",
        Some(config.runner_ip.as_deref().unwrap_or("")),
    )?;
    let (runner_ip, runner_user) = if !runner_ip_input.trim().is_empty() {
        let ru = prompt_with_default(
            "Runner sudo user",
            Some(config.runner_user.as_deref().unwrap_or("root")),
        )?;
        // Resolve hostname → IP on the CLI machine now.
        // The CTFd host's Docker container won't have Tailscale / split-DNS and
        // therefore cannot resolve Tailscale hostnames like "remote-instances".
        let resolved = resolve_host_to_ip(&runner_ip_input);
        if resolved != runner_ip_input {
            println!("  [ok] '{}' resolved to {} (IP will be stored in config)", runner_ip_input, resolved);
        }
        (Some(resolved), Some(ru))
    } else {
        (None, None)
    };

    // ── MONITOR_CTFD_PATH ─────────────────────────────────────────────────────
    let ctfd_path = prompt_with_default(
        "CTFd installation path on remote",
        Some(
            config
                .monitor_ctfd_path
                .as_deref()
                .unwrap_or("/home/docker/CTFd"),
        ),
    )?;

    // ── MONITOR_PORT ──────────────────────────────────────────────────────────
    let monitor_port = prompt_with_default(
        "Remote Monitor port",
        Some(config.monitor_port.as_deref().unwrap_or("33133")),
    )?;

    // ── MONITOR_TOKEN ─────────────────────────────────────────────────────────
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

    // ── SSH_KEY_PATH ──────────────────────────────────────────────────────────
    let ssh_key_path = if let Some(ref key) = config.ssh_key_path {
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
    config.challenges_path = Some(base_dir.clone());
    config.monitor_ip = Some(monitor_ip.clone());
    config.monitor_user = Some(monitor_user.clone());
    config.runner_ip = runner_ip.clone();
    config.runner_user = runner_user.clone();
    config.monitor_ctfd_path = Some(ctfd_path.clone());
    config.monitor_port = Some(monitor_port.clone());
    config.monitor_token = Some(monitor_token.clone());
    config.ssh_key_path = Some(ssh_key_path.clone());

    save_config(&config, &config_path)?;
    println!("  [ok] config saved to {}", config_path.display());

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

    // ── Build ansible extra-vars ───────────────────────────────────────────────
    let mut evars: Vec<String> = vec![
        format!("ssh_key={}", ssh_key_path),
        format!("ctfd_path={}", ctfd_path),
        format!("monitor_token={}", monitor_token),
        format!("monitor_port={}", monitor_port),
        format!("max_concurrent_provisions={}", config.max_concurrent_provisions.unwrap_or(4)),
        format!("max_instances_per_team={}", config.max_instances_per_team.unwrap_or(0)),
    ];
    if let Some(ref bin) = monitor_binary {
        evars.push(format!("monitor_binary={}", bin.display()));
    }
    if let Some(ref plugin) = plugin_src {
        evars.push(format!("plugin_src={}", plugin.display()));
    }
    if let Some(ref rip) = runner_ip {
        evars.push(format!("runner_ip={}", rip));
    }
    if let Some(ref ruser) = runner_user {
        evars.push(format!("runner_user={}", ruser));
    }
    if let Some(ref domain) = config.ctfd_domain {
        if !domain.is_empty() {
            evars.push(format!("ctfd_domain={}", domain));
        }
    }
    if let Some(ref domain) = config.runner_domain {
        if !domain.is_empty() {
            evars.push(format!("runner_domain={}", domain));
        }
    }
    let mut inventory = format!(
        "[ctfd]\n{} ansible_user={} ansible_ssh_common_args='-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null'\n",
        monitor_ip, monitor_user
    );
    if let (Some(ref rip), Some(ref ruser)) = (&runner_ip, &runner_user) {
        inventory.push_str(&format!(
            "\n[runner]\n{} ansible_user={} ansible_ssh_common_args='-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null'\n",
            rip, ruser
        ));
    }

    println!("\nRunning Ansible playbook...");
    run_ansible_playbook(PLAYBOOK, &inventory, &evars)?;

    let monitor_url = format!("http://{}:{}", monitor_ip, monitor_port);
    println!("\nNervCTF setup complete!");
    println!("  Admin Panel:  {}/admin", monitor_url);
    println!("  Token:        {}", monitor_token);
    println!("  Config:       {}", config_path.display());
    if monitor_binary.is_none() {
        println!("\n[!] remember to build and deploy the Remote Monitor:");
        println!("   cargo build --release --target x86_64-unknown-linux-musl -p remote-monitor");
        println!("   Then re-run `nervctf setup` to deploy it.");
    }
    Ok(())
}

// ── Shared ansible runner ─────────────────────────────────────────────────────

/// Write the playbook + inventory to a tempdir and invoke ansible-playbook.
/// Extra vars are serialised to a JSON file and passed via `-e @<path>` so
/// that values containing spaces or special characters are handled correctly.
/// Requires `ansible-playbook` to be present in PATH.
fn run_ansible_playbook(playbook: &str, inventory: &str, evars: &[String]) -> Result<()> {
    let tmp = tempdir()?;
    let playbook_path = tmp.path().join("playbook.yml");
    let inventory_path = tmp.path().join("inventory.ini");
    let cfg_path = tmp.path().join("ansible.cfg");
    fs::write(&playbook_path, playbook)?;
    fs::write(&inventory_path, inventory)?;
    // Suppress deprecation/system warnings that come from Ansible's own internal
    // modules (e.g. synchronize using deprecated _text imports) — nothing we can
    // fix in the playbook YAML itself.
    fs::write(&cfg_path,
        "[defaults]\ndeprecation_warnings = False\nsystem_warnings = False\n"
    )?;

    // Build a JSON object from "key=value" strings and write it to a temp file.
    // ansible-playbook accepts `-e @/path/to/file.json` for structured vars.
    let mut json_map = serde_json::Map::new();
    for ev in evars {
        if let Some((key, value)) = ev.split_once('=') {
            json_map.insert(key.to_string(), serde_json::Value::String(value.to_string()));
        }
    }
    let evars_json = serde_json::to_string(&serde_json::Value::Object(json_map))?;
    let evars_path = tmp.path().join("evars.json");
    fs::write(&evars_path, &evars_json)?;

    let args: Vec<String> = vec![
        "-i".to_string(),
        inventory_path.to_str().unwrap().to_string(),
        playbook_path.to_str().unwrap().to_string(),
        "-e".to_string(),
        format!("@{}", evars_path.to_str().unwrap()),
    ];

    let status = Command::new("ansible-playbook")
        .args(&args)
        .env("ANSIBLE_CONFIG", cfg_path.to_str().unwrap())
        .status()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow!("ansible-playbook not found in PATH. Install Ansible first:\n  pip install ansible\n  -- or --\n  apt install ansible")
            } else {
                e.into()
            }
        })?;

    if !status.success() {
        return Err(anyhow!(
            "ansible-playbook failed with exit code {:?}",
            status.code()
        ));
    }
    Ok(())
}

// ── Upgrade ───────────────────────────────────────────────────────────────────

/// Upgrade an existing NervCTF deployment: push the updated CTFd plugin, rebuild
/// the remote-monitor Docker image, and restart the affected containers.
/// Reads all connection details from the existing .nervctf.yml — no re-prompting.
pub fn run_upgrade() -> Result<()> {
    println!("==============================================");
    println!(" NervCTF Upgrade: Updating Deployed Services");
    println!("----------------------------------------------");
    println!("This will:");
    println!(" - Sync the CTFd plugin (rsync, delete stale files)");
    println!(" - Copy the new remote-monitor binary");
    println!(" - Rebuild the nervctf-monitor Docker image");
    println!(" - Restart the remote-monitor container");
    println!(" - Restart CTFd (to reload the plugin)");
    println!("==============================================\n");

    let cwd = std::env::current_dir()?;
    let (config, config_file) = load_config(&cwd);

    // Require an existing config — upgrade makes no sense without a prior setup.
    let config_file = config_file.ok_or_else(|| {
        anyhow!(
            "No .nervctf.yml found. Run `nervctf setup` first to create a deployment."
        )
    })?;
    println!("Using config: {}", config_file.display());

    let monitor_ip = config.monitor_ip.as_deref().ok_or_else(|| {
        anyhow!("monitor_ip not set in .nervctf.yml — run `nervctf setup` to fix.")
    })?;
    let monitor_user = config.monitor_user.as_deref().unwrap_or("root");
    let ctfd_path = config
        .monitor_ctfd_path
        .as_deref()
        .unwrap_or("/home/docker/CTFd");
    let monitor_port = config.monitor_port.as_deref().unwrap_or("33133");

    // ── Find local artifacts ───────────────────────────────────────────────────
    let monitor_binary = find_monitor_binary();
    let plugin_src = find_plugin_src();

    if monitor_binary.is_none() && plugin_src.is_none() {
        return Err(anyhow!(
            "Neither the remote-monitor binary nor the CTFd plugin could be found locally.\n\
             Build the binary first:\n\
             cargo build --release --target x86_64-unknown-linux-musl -p remote-monitor"
        ));
    }

    println!("Target:   {}@{}", monitor_user, monitor_ip);
    println!("CTFd dir: {}", ctfd_path);
    match &monitor_binary {
        Some(p) => println!("Monitor binary: {}", p.display()),
        None     => println!("[!] remote-monitor binary not found — skipping binary upgrade"),
    }
    match &plugin_src {
        Some(p) => println!("CTFd plugin:    {}", p.display()),
        None     => println!("[!] CTFd plugin not found — skipping plugin upgrade"),
    }

    println!();
    let confirmed = Confirm::new()
        .with_prompt("Proceed with upgrade?")
        .default(true)
        .interact()?;
    if !confirmed {
        println!("Aborted.");
        return Ok(());
    }

    // ── Build extra-vars ───────────────────────────────────────────────────────
    let mut evars: Vec<String> = vec![
        format!("ctfd_path={}", ctfd_path),
        format!("monitor_port={}", monitor_port),
        format!("max_concurrent_provisions={}", config.max_concurrent_provisions.unwrap_or(4)),
        format!("max_instances_per_team={}", config.max_instances_per_team.unwrap_or(0)),
    ];
    if let Some(ref bin) = monitor_binary {
        evars.push(format!("monitor_binary={}", bin.display()));
    }
    if let Some(ref plugin) = plugin_src {
        evars.push(format!("plugin_src={}", plugin.display()));
    }
    // Pass runner vars so the upgrade playbook can write the correct
    // RUNNER_SSH_TARGET / CHALLENGES_BASE_DIR into the docker-compose override.
    if let Some(ref rip) = config.runner_ip {
        if !rip.is_empty() {
            evars.push(format!("runner_ip={}", rip));
        }
    }
    if let Some(ref ruser) = config.runner_user {
        if !ruser.is_empty() {
            evars.push(format!("runner_user={}", ruser));
        }
    }
    if let Some(ref token) = config.monitor_token {
        evars.push(format!("monitor_token={}", token));
    }
    if let Some(ref domain) = config.ctfd_domain {
        if !domain.is_empty() {
            evars.push(format!("ctfd_domain={}", domain));
        }
    }
    if let Some(ref domain) = config.runner_domain {
        if !domain.is_empty() {
            evars.push(format!("runner_domain={}", domain));
        }
    }

    let mut inventory = format!(
        "[ctfd]\n{} ansible_user={} ansible_ssh_common_args='-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null'\n",
        monitor_ip, monitor_user
    );
    if let (Some(ref rip), Some(ref ruser)) = (&config.runner_ip, &config.runner_user) {
        if !rip.is_empty() {
            inventory.push_str(&format!(
                "\n[runner]\n{} ansible_user={} ansible_ssh_common_args='-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null'\n",
                rip, ruser
            ));
        }
    }

    println!("\nRunning upgrade playbook...");
    run_ansible_playbook(UPGRADE_PLAYBOOK, &inventory, &evars)?;

    let monitor_url = format!("http://{}:{}", monitor_ip, monitor_port);
    let upgrade_token = config.monitor_token.as_deref().unwrap_or("-");
    println!("\nUpgrade complete!");
    println!("  Admin Panel:  {}/admin", monitor_url);
    println!("  Token:        {}", upgrade_token);
    Ok(())
}


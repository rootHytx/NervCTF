use anyhow::{anyhow, Result};
use dialoguer::{Confirm, Input, Select};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::tempdir;

const PLAYBOOK: &str = include_str!("../assets/nervctf_playbook.yml");
const DOCKER_COMPOSE: &str = include_str!("../assets/docker-compose.yml");
const INSTALL_DOCKER_SH: &str = include_str!("../assets/install_docker_on_remote.sh");

const ENV_FILE: &str = "./.env";
const PERSIST_VARS: &[&str] = &["TARGET_IP", "TARGET_USER", "SSH_PUBKEY_PATH", "CTFD_PATH"];

fn get_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn load_env_file(path: &Path) {
    if let Ok(contents) = fs::read_to_string(path) {
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                if std::env::var(key).is_err() {
                    unsafe { std::env::set_var(key, value) };
                }
            }
        }
    }
}

fn persist_var(path: &Path, key: &str, value: &str) -> Result<()> {
    let mut contents = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };
    if !contents
        .lines()
        .any(|l| l.starts_with(&format!("{}=", key)))
    {
        if !contents.is_empty() && !contents.ends_with('\n') {
            contents.push('\n');
        }
        contents.push_str(&format!("{}={}\n", key, value));
        fs::write(path, contents)?;
    }
    Ok(())
}

/// Walk up from cwd until we find a shell.nix
fn find_shell_nix() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join("shell.nix");
        if candidate.exists() {
            return Some(candidate);
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

pub fn run_setup() -> Result<()> {
    println!("==============================================");
    println!(" NervCTF Setup: Automated CTFd Environment");
    println!("----------------------------------------------");
    println!("This will:");
    println!(" - Install Rootless Docker, CTFd, the solve webhook plugin, and the Remote Monitor");
    println!(" - Configure SSH access for the deployment user");
    println!("==============================================\n");

    let env_path = PathBuf::from(ENV_FILE);

    if env_path.exists() {
        load_env_file(&env_path);
    } else {
        println!("No .env file found. Creating a new one.");
        fs::write(&env_path, "")?;
    }

    let missing: Vec<&str> = PERSIST_VARS
        .iter()
        .copied()
        .filter(|v| get_env(v).is_none())
        .collect();

    let persist = if missing.is_empty() {
        println!("All required variables are present. Proceeding with current configuration...\n");
        false
    } else {
        Confirm::new()
            .with_prompt(format!("Persist entered values to {}?", ENV_FILE))
            .default(true)
            .interact()?
    };

    // TARGET_IP
    let target_ip = if let Some(ip) = get_env("TARGET_IP") {
        println!("Using existing TARGET_IP: {}", ip);
        ip
    } else {
        let ip: String = Input::new()
            .with_prompt("Target machine IP address")
            .interact_text()?;
        if ip.trim().is_empty() {
            return Err(anyhow!("IP address is required"));
        }
        if persist {
            persist_var(&env_path, "TARGET_IP", &ip)?;
        }
        ip
    };

    // TARGET_USER
    let target_user = if let Some(user) = get_env("TARGET_USER") {
        println!("Using existing TARGET_USER: {}", user);
        user
    } else {
        let user: String = Input::new()
            .with_prompt("Remote sudo user")
            .default("root".to_string())
            .interact_text()?;
        if persist {
            persist_var(&env_path, "TARGET_USER", &user)?;
        }
        user
    };

    // CTFD_PATH
    let ctfd_path = if let Some(path) = get_env("CTFD_PATH") {
        println!("Using existing CTFD_PATH: {}", path);
        path
    } else {
        let installed = Confirm::new()
            .with_prompt("Is CTFd already installed on the remote machine?")
            .default(false)
            .interact()?;
        let path: String = if installed {
            Input::new()
                .with_prompt("Full path to CTFd directory on remote")
                .interact_text()?
        } else {
            Input::new()
                .with_prompt("Where should CTFd be installed on remote?")
                .default("/home/docker/CTFd".to_string())
                .interact_text()?
        };
        if persist {
            persist_var(&env_path, "CTFD_PATH", &path)?;
        }
        path
    };

    // SSH key selection
    let ssh_pubkey_path = if let Some(key) = get_env("SSH_PUBKEY_PATH") {
        println!("Using existing SSH public key: {}", key);
        key
    } else {
        println!("\nAvailable SSH public keys in ~/.ssh:");
        let pubkeys = list_ssh_pubkeys();

        let key_path = if pubkeys.is_empty() {
            println!("No existing SSH public keys found in ~/.ssh.");
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

        let path_str = key_path.to_string_lossy().to_string();
        if persist {
            persist_var(&env_path, "SSH_PUBKEY_PATH", &path_str)?;
        }
        path_str
    };

    // Extract embedded assets to a temp dir
    let tmp = tempdir()?;
    let playbook_path = tmp.path().join("nervctf_playbook.yml");
    let compose_path = tmp.path().join("docker-compose.yml");
    let install_docker_path = tmp.path().join("install_docker_on_remote.sh");
    let inventory_path = tmp.path().join("inventory.ini");

    fs::write(&playbook_path, PLAYBOOK)?;
    fs::write(&compose_path, DOCKER_COMPOSE)?;
    fs::write(&install_docker_path, INSTALL_DOCKER_SH)?;
    fs::write(
        &inventory_path,
        format!("[ctfd]\n{} ansible_user={}\n", target_ip, target_user),
    )?;

    let extra_vars = format!("ssh_key={} ctfd_path={}", ssh_pubkey_path, ctfd_path);

    println!("\nRunning Ansible playbook...");

    let ansible_args = [
        "-i",
        inventory_path.to_str().unwrap(),
        playbook_path.to_str().unwrap(),
        "--extra-vars",
        &extra_vars,
    ];

    // Try ansible-playbook directly; fall back to nix-shell if not in PATH
    let status = match Command::new("ansible-playbook")
        .args(&ansible_args)
        .status()
    {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Find shell.nix by walking up from cwd
            let shell_nix = find_shell_nix().ok_or_else(|| {
                anyhow!(
                    "ansible-playbook not found in PATH and no shell.nix found. \
                     Install ansible or run inside nix-shell."
                )
            })?;
            let nix_cmd = format!("ansible-playbook {}", ansible_args.join(" "));
            println!(
                "  (ansible-playbook not in PATH — using nix-shell at {})",
                shell_nix.display()
            );
            Command::new("nix-shell")
                .args([shell_nix.to_str().unwrap(), "--run", &nix_cmd])
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

    println!("\nNervCTF setup complete!");
    Ok(())
}

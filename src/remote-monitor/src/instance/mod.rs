//! Instance lifecycle management — dispatches to backend-specific modules.

pub mod compose;
pub mod docker;
pub mod lxc;
pub mod vagrant;

use anyhow::{anyhow, Result};
use rand::distributions::Alphanumeric;
use rand::Rng;
use serde_json::Value;
use mysql_async::Pool;
use crate::db::Db;

/// Generate a random flag for `flag_mode = "random"`, or return None for static/no flag.
pub fn generate_flag(config: &Value) -> Option<String> {
    if config["flag_mode"].as_str().unwrap_or("static") != "random" {
        return None;
    }
    let prefix = config["flag_prefix"].as_str().unwrap_or("CTF{");
    let suffix = config["flag_suffix"].as_str().unwrap_or("}");
    let length = config["random_flag_length"].as_u64().unwrap_or(16) as usize;
    let random: String = rand::thread_rng()
        .sample_iter(Alphanumeric)
        .take(length)
        .map(char::from)
        .collect();
    Some(format!("{}{}{}", prefix, random, suffix))
}

/// Sanitize a challenge name to a valid Docker name component.
pub fn sanitize_name(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// Generate a unique container/project name for an instance.
/// Uses 6 random lowercase alphanumeric chars to avoid name collisions
/// when a team re-provisions before the previous container is fully torn down.
pub fn container_name(challenge_name: &str) -> String {
    let suffix: String = rand::thread_rng()
        .sample_iter(Alphanumeric)
        .take(6)
        .map(|c| (c as char).to_ascii_lowercase())
        .collect();
    format!("ctf-{}-{}", sanitize_name(challenge_name), suffix)
}

/// Stop and remove a container/project for an instance (called on expiry or explicit stop).
///
/// The `container_id` field stores either a Docker container ID (for docker backend)
/// or a compose project name (for compose backend). We try Docker removal first,
/// then compose down if it looks like a project name.
///
/// `runner_ssh` — SSH target for split-machine mode (e.g. `docker@192.168.1.50`).
pub async fn cleanup_container(container_id: &str, runner_ssh: Option<&str>) {
    // Compose project names and LXC/Docker container names all start with "ctf-"
    if container_id.starts_with("ctf-") && container_id.len() < 80 {
        let _ = compose::down(container_id).await;
        let _ = lxc::delete(container_id).await;
    }
    // Always also try docker remove (no-op if not a container ID)
    if let Err(e) = docker::remove_container(container_id, runner_ssh).await {
        eprintln!("  cleanup: failed to remove {}: {}", container_id, e);
    }
}

/// Provision a new instance for a team.
///
/// Generates a random flag (if `flag_mode = "random"`), registers it with CTFd's DB,
/// starts the container, and persists everything in the DB.
///
/// `runner_ssh` — SSH target for split-machine mode (e.g. `docker@192.168.1.50`).
///
/// Returns `(host, port, connection_type, expires_at)`.
pub async fn provision(
    db: &Db,
    challenge_name: &str,
    team_id: i64,
    user_id: Option<i64>,
    config: &Value,
    public_host: &str,
    ctfd_pool: &Pool,
    runner_ssh: Option<&str>,
) -> Result<(String, u16, String, String)> {
    let backend = config["backend"].as_str().unwrap_or("docker");
    let internal_port = config["internal_port"].as_u64().unwrap_or(4000) as u32;
    let connection = config["connection"].as_str().unwrap_or("nc").to_string();
    let timeout_minutes = config["timeout_minutes"].as_u64().unwrap_or(45);
    let command = config["command"].as_str();

    // Look up the CTFd challenge ID for flag registration.
    let ctfd_id = crate::db::get_ctfd_id(db, challenge_name)?;

    match backend {
        "docker" => {
            let image_tag = crate::db::get_image_tag(db, challenge_name)?
                .unwrap_or_else(|| format!("{}:latest", sanitize_name(challenge_name)));

            let used_ports = crate::db::get_used_ports(db)?;
            let host_port = docker::pick_free_port(&used_ports)?;
            let cname = container_name(challenge_name);

            let flag = generate_flag(config);
            let env_vars: Vec<(String, String)> = flag.as_deref()
                .map(|f| vec![("FLAG".to_string(), f.to_string())])
                .unwrap_or_default();

            let container_id = docker::run_container(
                &image_tag,
                &cname,
                host_port,
                internal_port,
                command,
                &env_vars,
                runner_ssh,
            ).await?;

            let ctfd_flag_id = match (&flag, ctfd_id) {
                (Some(f), Some(cid)) => crate::ctfd_db::create_flag(ctfd_pool, cid, f).await,
                _ => None,
            };

            let expires_at = expires_at_string(timeout_minutes);
            crate::db::insert_instance(
                db, challenge_name, team_id, user_id, &container_id,
                public_host, host_port as i64, &connection, &expires_at,
                flag.as_deref(), ctfd_flag_id,
            )?;

            Ok((public_host.to_string(), host_port, connection, expires_at))
        }
        "compose" => {
            let compose_file_str = config["compose_file"].as_str().unwrap_or("docker-compose.yml");
            // Resolve compose path: absolute paths used as-is; relative paths resolved
            // against the server-side challenge directory /data/challenges/<name>/
            let compose_path = if compose_file_str.starts_with('/') {
                std::path::PathBuf::from(compose_file_str)
            } else {
                let base = std::env::var("CHALLENGES_BASE_DIR")
                    .unwrap_or_else(|_| "/opt/nervctf/challenges".to_string());
                std::path::PathBuf::from(format!(
                    "{}/{}/{}",
                    base.trim_end_matches('/'),
                    sanitize_name(challenge_name),
                    compose_file_str
                ))
            };
            let compose_service = config["compose_service"].as_str().unwrap_or("");
            let flag_delivery = config["flag_delivery"].as_str().unwrap_or("env");
            let flag_file_path = config["flag_file_path"].as_str();
            let flag_service = config["flag_service"].as_str();
            let project_name = container_name(challenge_name);
            let used_ports = crate::db::get_used_ports(db)?;
            let flag = generate_flag(config);
            let (host_port, project) = compose::up(
                &compose_path,
                &project_name,
                internal_port,
                compose_service,
                &used_ports,
                flag.as_deref(),
                flag_delivery,
                flag_file_path,
                flag_service,
            ).await?;

            let ctfd_flag_id = match (&flag, ctfd_id) {
                (Some(f), Some(cid)) => crate::ctfd_db::create_flag(ctfd_pool, cid, f).await,
                _ => None,
            };

            let expires_at = expires_at_string(timeout_minutes);
            crate::db::insert_instance(
                db, challenge_name, team_id, user_id, &project,
                public_host, host_port as i64, &connection, &expires_at,
                flag.as_deref(), ctfd_flag_id,
            )?;
            Ok((public_host.to_string(), host_port, connection, expires_at))
        }
        "lxc" => {
            let lxc_image = config["lxc_image"].as_str().unwrap_or("");
            let cname = container_name(challenge_name);
            let used_ports = crate::db::get_used_ports(db)?;
            let host_port = docker::pick_free_port(&used_ports)?;
            let flag = generate_flag(config);
            let cid = lxc::launch(lxc_image, &cname, host_port, internal_port, flag.as_deref()).await?;

            let ctfd_flag_id = match (&flag, ctfd_id) {
                (Some(f), Some(cid_val)) => crate::ctfd_db::create_flag(ctfd_pool, cid_val, f).await,
                _ => None,
            };

            let expires_at = expires_at_string(timeout_minutes);
            crate::db::insert_instance(
                db, challenge_name, team_id, user_id, &cid,
                public_host, host_port as i64, &connection, &expires_at,
                flag.as_deref(), ctfd_flag_id,
            )?;
            Ok((public_host.to_string(), host_port, connection, expires_at))
        }
        "vagrant" => {
            let vagrantfile = config["vagrantfile"].as_str().unwrap_or("");
            let vm_name = container_name(challenge_name);
            let (host_port, vm_id) = vagrant::up(vagrantfile, &vm_name, internal_port).await?;
            let expires_at = expires_at_string(timeout_minutes);
            crate::db::insert_instance(
                db, challenge_name, team_id, user_id, &vm_id,
                public_host, host_port as i64, &connection, &expires_at, None, None,
            )?;
            Ok((public_host.to_string(), host_port, connection, expires_at))
        }
        other => Err(anyhow!("Unknown backend: {}", other)),
    }
}

pub fn expires_at_string(timeout_minutes: u64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH, Duration};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let future = now + Duration::from_secs(timeout_minutes * 60);
    let secs = future.as_secs();
    // Format as SQLite datetime: "YYYY-MM-DD HH:MM:SS"
    let dt = chrono_from_secs(secs);
    dt
}

fn chrono_from_secs(secs: u64) -> String {
    // Manual RFC3339-like formatting without chrono dependency
    let s = secs;
    // days since epoch
    let days = s / 86400;
    let time_of_day = s % 86400;
    let h = time_of_day / 3600;
    let m = (time_of_day % 3600) / 60;
    let sec = time_of_day % 60;

    // Gregorian calendar computation
    let (year, month, day) = days_to_ymd(days as u32);
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", year, month, day, h, m, sec)
}

fn days_to_ymd(days: u32) -> (u32, u32, u32) {
    // Days since Unix epoch (1970-01-01) to (year, month, day)
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

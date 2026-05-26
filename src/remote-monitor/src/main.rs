//! Remote Monitor — CTFd SQL backend + instance lifecycle manager
//!
//! Routes:
//!   GET  /health                         — liveness check (no auth)
//!   GET  /admin                          — admin dashboard (monitor token via ?token= or header)
//!   GET  /instance/:name                 — HTML player page
//!   ANY  /api/v1/diff                    — local/remote diff (monitor token)
//!   GET/POST   /api/v1/challenges        — list / create challenges (monitor token)
//!   GET/PATCH/DELETE /api/v1/challenges/{id} — get / update / delete challenge (monitor token)
//!   GET/POST   /api/v1/flags             — list (?challenge_id=N) / create flags (monitor token)
//!   DELETE     /api/v1/flags/{id}        — delete flag (monitor token)
//!   GET/POST   /api/v1/hints             — list / create hints (monitor token)
//!   DELETE     /api/v1/hints/{id}        — delete hint (monitor token)
//!   GET/POST   /api/v1/tags              — list / create tags (monitor token)
//!   DELETE     /api/v1/tags/{id}         — delete tag (monitor token)
//!   GET/POST   /api/v1/files             — list / upload files (monitor token)
//!   DELETE     /api/v1/files/{id}        — delete file (monitor token)
//!   GET/POST   /api/v1/topics             — list (?challenge_id=N) / create topics (monitor token)
//!   DELETE     /api/v1/topics/:id         — delete topic link (monitor token)
//!   POST /api/v1/instance/build          — build Docker image (monitor token)
//!   POST /api/v1/instance/build-compose  — upload+extract compose dir + pre-build images (monitor token)
//!   POST /api/v1/instance/register       — register instance config (monitor token)
//!   GET  /api/v1/instance/list           — list configs (monitor token)
//!   GET  /api/v1/admin/instances         — list all active instances (monitor token)
//!   GET  /api/v1/admin/attempts          — list flag attempts; ?alerts_only=true for sharing alerts (monitor token)
//!   GET  /api/v1/admin/solves            — list correct solves (one per team+challenge) (monitor token)
//!   GET  /api/v1/admin/probe             — CTFd compatibility probe result; ?refresh=true forces re-probe (monitor token)
//!   POST /api/v1/plugin/attempt          — record flag submission attempt (monitor token)
//!   POST /api/v1/instance/request        — provision instance (CTFd user token)
//!   GET  /api/v1/instance/info           — get own instance (CTFd user token)
//!   POST /api/v1/instance/renew          — extend timeout (CTFd user token)
//!   DELETE /api/v1/instance/stop         — destroy instance (CTFd user token)
//!
//! Environment variables:
//!   CTFD_DB_URL        — MariaDB connection string (required)
//!   CTFD_UPLOADS_DIR   — Path to CTFd uploads directory for file writes (optional)
//!   MONITOR_TOKEN      — admin token for nervctf CLI (required)
//!   PUBLIC_HOST        — hostname/IP returned in instance connection info (required)
//!   MONITOR_PORT       — bind port (default: 33133)
//!   MONITOR_BIND       — bind address (default: 0.0.0.0)
//!   DB_PATH            — SQLite database path (default: ./monitor.db)

mod ctfd_db;
mod db;
mod instance;

use anyhow::Result;
use sha2::{Digest, Sha256};
use tracing::{error, info, warn};
use tracing_subscriber::{EnvFilter, fmt};
use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{delete, get, post},
    Json, Router,
};
use bytes::Bytes;
use rand::Rng;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::env;
use std::sync::Arc;

use db::Db;

#[derive(Clone)]
struct AppState {
    public_host: String,
    db: Db,
    ctfd_pool: mysql_async::Pool,
    /// Directory where compose challenge sources are stored.
    /// In split-machine mode this path lives on the **runner** host.
    challenges_base_dir: String,
    /// Path to CTFd uploads directory for file writes (empty string if not set).
    ctfd_uploads_dir: String,
    /// CTFd base URL used in admin dashboard links (e.g. http://ctfd-host).
    /// Defaults to http://{public_host} if CTFD_DOMAIN is not set.
    ctfd_url: String,
    /// SSH target for split-machine mode, e.g. `docker@192.168.1.50`.
    /// When set, all Docker/compose commands are executed on the runner via SSH
    /// instead of the local Docker daemon.
    runner_ssh_target: Option<String>,
    /// Limits concurrent docker/compose provision operations to prevent port-pick
    /// races and avoid overwhelming the Docker daemon socket.
    provision_sem: Arc<tokio::sync::Semaphore>,
    /// Global cap on active instances (running + provisioning) per team across all
    /// challenges.  0 = unlimited.
    max_instances_per_team: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("remote_monitor=debug,info")),
        )
        .init();

    let monitor_token_env = env::var("MONITOR_TOKEN").ok();
    let public_host = env::var("PUBLIC_HOST").expect("PUBLIC_HOST is required");
    let port = env::var("MONITOR_PORT").unwrap_or_else(|_| "33133".to_string());
    let bind = env::var("MONITOR_BIND").unwrap_or_else(|_| "0.0.0.0".to_string());
    let db_path = env::var("DB_PATH").unwrap_or_else(|_| "./monitor.db".to_string());
    let challenges_base_dir = env::var("CHALLENGES_BASE_DIR")
        .unwrap_or_else(|_| "/opt/nervctf/challenges".to_string());
    let ctfd_uploads_dir = env::var("CTFD_UPLOADS_DIR").unwrap_or_default();
    let ctfd_url = env::var("CTFD_DOMAIN")
        .unwrap_or_else(|_| format!("http://{}", public_host));

    // Split-machine mode: parse RUNNER_SSH_TARGET (e.g. "docker@192.168.1.50")
    // or fall back to extracting the target from DOCKER_HOST=ssh://user@host.
    let runner_ssh_target: Option<String> = env::var("RUNNER_SSH_TARGET").ok()
        .or_else(|| {
            env::var("DOCKER_HOST").ok()
                .filter(|h| h.starts_with("ssh://"))
                .map(|h| h.trim_start_matches("ssh://").to_string())
        })
        .filter(|s| !s.is_empty());

    let ctfd_db_url = env::var("CTFD_DB_URL").expect("CTFD_DB_URL is required");

    let db = db::open(&db_path)?;

    // Bootstrap: on every startup, try to insert MONITOR_TOKEN (hashed) with INSERT OR IGNORE.
    // The UNIQUE constraint on token_hash makes this a no-op if already present, so a
    // re-deploy with the same token is safe. A re-deploy with a new token adds it alongside
    // any operator tokens that already exist.
    if let Some(ref raw) = monitor_token_env {
        let hash = hash_token(raw);
        match db::insert_operator_token_ignore(&db, "bootstrap", &hash) {
            Ok(true)  => info!("Bootstrapped operator token from MONITOR_TOKEN env var"),
            Ok(false) => {}
            Err(e)    => warn!("Failed to bootstrap operator token: {}", e),
        }
    }
    match db::count_operator_tokens(&db) {
        Ok(0) => warn!("No operator tokens in DB — admin panel is inaccessible until a token is added"),
        _ => {}
    }

    let ctfd_pool = ctfd_db::create_pool(&ctfd_db_url)?;

    let ctfd_mode = ctfd_db::detect_ctfd_mode(&ctfd_pool).await;
    match ctfd_mode {
        ctfd_db::CtfdMode::UserMode => {
            tracing::warn!(
                "CTFd is running in USER-MODE. All player instance requests will return 403 \
                 because token validation requires a non-null team_id. Switch CTFd to team-mode \
                 or run 'nervctf probe' to see the full capability report."
            );
        }
        ctfd_db::CtfdMode::Unknown => {
            tracing::warn!(
                "Could not determine CTFd mode (configs table inaccessible or key absent). \
                 If CTFd is in user-mode, all player instance requests will fail with 403."
            );
        }
        ctfd_db::CtfdMode::TeamMode => {
            tracing::info!("CTFd mode: team (player authentication will work correctly)");
        }
    }

    // Run the CTFd compatibility probe and persist the result to SQLite.
    tracing::info!("Running CTFd compatibility probe...");
    let probe_result = ctfd_db::run_probe(&ctfd_pool).await;
    if let Err(e) = db::save_probe_result(&db, &probe_result) {
        tracing::warn!("Failed to persist probe result: {}", e);
    }
    // Log a one-line summary; warn with per-note detail when any capability is broken.
    let broken: Vec<&str> = [
        ("challenge_crud", probe_result.cap_challenge_crud.as_str()),
        ("dynamic_scoring", probe_result.cap_dynamic_scoring.as_str()),
        ("player_auth",     probe_result.cap_player_auth.as_str()),
        ("instance_flags",  probe_result.cap_instance_flags.as_str()),
        ("redis_sync",      probe_result.cap_redis_sync.as_str()),
    ]
    .iter()
    .filter(|(_, s)| *s == "broken")
    .map(|(n, _)| *n)
    .collect();

    if broken.is_empty() {
        tracing::info!(
            "Probe: CTFd {} | mode:{} | CRUD={} dynamic={} auth={} flags={} redis={}",
            probe_result.ctfd_version_tag.as_deref().unwrap_or("unknown"),
            if probe_result.is_team_mode == Some(true) { "team" } else { "user/unknown" },
            probe_result.cap_challenge_crud,
            probe_result.cap_dynamic_scoring,
            probe_result.cap_player_auth,
            probe_result.cap_instance_flags,
            probe_result.cap_redis_sync,
        );
    } else {
        tracing::warn!(
            "Probe: CTFd {} | BROKEN capabilities: {}",
            probe_result.ctfd_version_tag.as_deref().unwrap_or("unknown"),
            broken.join(", ")
        );
        for note in &probe_result.probe_notes {
            tracing::warn!("  [probe] {}", note);
        }
    }

    let max_concurrent_provisions: usize = env::var("MAX_CONCURRENT_PROVISIONS")
        .ok().and_then(|s| s.parse().ok()).unwrap_or(4);
    info!("provision concurrency limit: {}", max_concurrent_provisions);

    let max_instances_per_team: u64 = env::var("MAX_INSTANCES_PER_TEAM")
        .ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    if max_instances_per_team > 0 {
        info!("per-team instance cap: {}", max_instances_per_team);
    }

    let state = Arc::new(AppState {
        public_host,
        db: db.clone(),
        ctfd_pool,
        challenges_base_dir,
        ctfd_uploads_dir,
        ctfd_url,
        runner_ssh_target,
        provision_sem: Arc::new(tokio::sync::Semaphore::new(max_concurrent_provisions)),
        max_instances_per_team,
    });

    // Spawn background CTFd solve sync task (read-only from MariaDB → SQLite cache)
    let sync_interval: u64 = env::var("CTFD_DB_SYNC_INTERVAL")
        .ok().and_then(|s| s.parse().ok()).unwrap_or(30);
    let sync_db = db.clone();
    let sync_pool = state.ctfd_pool.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(sync_interval)).await;
            if let Err(e) = ctfd_db::sync_solves(&sync_pool, &sync_db).await {
                warn!("ctfd sync solves: {}", e);
            }
            if let Err(e) = ctfd_db::sync_users_and_teams(&sync_pool, &sync_db).await {
                warn!("ctfd sync users/teams: {}", e);
            }
        }
    });

    // Spawn background expiry task
    let expiry_state = Arc::clone(&state);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;

            // ── Expire tracked instances ──────────────────────────────────────
            match db::get_expired_instances(&expiry_state.db) {
                Ok(expired) => {
                    for (challenge_name, container_id, team_id, ctfd_flag_id) in expired {
                        info!("expiry: cleaning up {}/{}", challenge_name, team_id);
                        let _ = db::delete_instance(&expiry_state.db, &challenge_name, team_id);
                        if let Some(flag_id) = ctfd_flag_id {
                            ctfd_db::delete_flag(&expiry_state.ctfd_pool, flag_id).await;
                        }
                        if let Some(cid) = container_id {
                            instance::cleanup_container(&cid, expiry_state.runner_ssh_target.as_deref()).await;
                        }
                    }
                }
                Err(e) => error!("expiry: db error: {}", e),
            }

            // ── Orphan cleanup: stop ctf-* compose projects not in DB ─────────
            let tracked = db::get_all_container_ids(&expiry_state.db).unwrap_or_default();
            let projects = instance::compose::list_ctf_projects().await;
            for project in projects {
                if !tracked.contains(&project) {
                    info!("orphan: stopping untracked compose project {}", project);
                    let _ = instance::compose::down(&project, expiry_state.runner_ssh_target.as_deref(), None).await;
                }
            }

            // ── Health check: remove DB entries for externally killed containers ──
            //
            // Both docker and compose backends store a `ctf-*` short name as
            // container_id, so we can't reliably infer the backend from the name alone.
            // Instead we check both lists: a container is alive if found in EITHER the
            // compose projects list OR the docker container names list.
            // If a query fails (SSH down, docker unreachable) we get None and treat that
            // list as unable to confirm anything — only mark dead when BOTH queries
            // succeed and confirm absence. This avoids false-deletes on transient errors.
            let running_instances = db::get_running_instances(&expiry_state.db).unwrap_or_default();
            if !running_instances.is_empty() {
                let running_projects = instance::compose::list_running_ctf_project_names().await;
                let running_names = instance::docker::list_running_container_names(
                    expiry_state.runner_ssh_target.as_deref(),
                ).await;
                for (challenge_name, team_id, container_id, ctfd_flag_id) in running_instances {
                    let Some(ref cid) = container_id else { continue };
                    // Determine liveness: Some(true) = confirmed present, Some(false) = confirmed absent, None = unknown
                    let in_projects  = running_projects.as_ref().map(|s| s.contains(cid));
                    let in_names     = running_names.as_ref().map(|s| s.contains(cid));
                    let is_dead = match (in_projects, in_names) {
                        // Both queries succeeded and neither list contains this id
                        (Some(false), Some(false)) => true,
                        // One query failed — only mark dead if the successful one
                        // confirmed absence AND the other was the only possible home
                        // (compose-only projects won't be in docker names and vice-versa)
                        (Some(false), None) | (None, Some(false)) => true,
                        // Found in at least one list, or both queries failed — assume alive
                        _ => false,
                    };
                    if is_dead {
                        info!("health: container gone for {}/{}, removing from DB", challenge_name, team_id);
                        let _ = db::delete_instance(&expiry_state.db, &challenge_name, team_id);
                        if let Some(flag_id) = ctfd_flag_id {
                            ctfd_db::delete_flag(&expiry_state.ctfd_pool, flag_id).await;
                        }
                        if let Some(cid) = container_id {
                            instance::cleanup_container(&cid, expiry_state.runner_ssh_target.as_deref()).await;
                        }
                    }
                }
            }
        }
    });

    let addr = format!("{}:{}", bind, port);
    info!("Starting remote-monitor on {}", addr);
    info!("PUBLIC_HOST={}", state.public_host);
    info!("CHALLENGES_BASE_DIR={}", state.challenges_base_dir);
    match &state.runner_ssh_target {
        Some(target) => info!("Split-machine mode: runner={} (challenges stored on runner at {})", target, state.challenges_base_dir),
        None => info!("Single-machine mode: challenges at {}", state.challenges_base_dir),
    }

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/", get(login_page_handler))
        .route("/auth/login", post(auth_login_handler))
        .route("/auth/logout", post(auth_logout_handler))
        .route("/admin", get(admin_dashboard_handler))
        .route("/instance/:name", get(instance_page_handler))
        .route("/api/v1/diff", get(diff_handler).post(diff_handler).patch(diff_handler).delete(diff_handler))
        // CTFd challenge CRUD (monitor token — used by nervctf CLI)
        .route("/api/v1/challenges", get(ctfd_challenges_list).post(ctfd_challenge_create))
        .route("/api/v1/challenges/:id", get(ctfd_challenge_get).patch(ctfd_challenge_update).delete(ctfd_challenge_delete))
        // Flags
        .route("/api/v1/flags", get(ctfd_flags_list).post(ctfd_flag_create))
        .route("/api/v1/flags/:id", delete(ctfd_flag_delete))
        // Hints
        .route("/api/v1/hints", get(ctfd_hints_list).post(ctfd_hint_create))
        .route("/api/v1/hints/:id", delete(ctfd_hint_delete))
        // Tags
        .route("/api/v1/tags", get(ctfd_tags_list).post(ctfd_tag_create))
        .route("/api/v1/tags/:id", delete(ctfd_tag_delete))
        // Files
        .route("/api/v1/files", get(ctfd_files_list).post(ctfd_files_upload))
        .route("/api/v1/files/:id", delete(ctfd_file_delete))
        // Topics
        .route("/api/v1/topics", get(ctfd_topics_list).post(ctfd_topic_create))
        .route("/api/v1/topics/:id", delete(ctfd_topic_delete))
        // Admin routes (monitor token)
        .route("/api/v1/instance/build", post(instance_build_handler))
        .route("/api/v1/instance/build-compose", post(build_compose_handler))
        .route("/api/v1/instance/build-compose-remote", post(build_compose_remote_handler))
        .route("/api/v1/instance/register", post(instance_register_handler))
        .route("/api/v1/instance/list", get(instance_list_handler))
        .route("/api/v1/admin/instances", get(admin_instances_handler))
        .route("/api/v1/admin/attempts", get(admin_attempts_handler))
        .route("/api/v1/admin/solves", get(admin_solves_handler))
        .route("/api/v1/admin/config", get(admin_config_handler))
        .route("/api/v1/admin/probe", get(admin_probe_handler))
        .route("/api/v1/admin/tokens", get(list_tokens_handler).post(create_token_handler))
        .route("/api/v1/admin/tokens/:id", delete(revoke_token_handler))
        // Plugin routes (monitor token + explicit team_id) — used by CTFd plugin
        .route("/api/v1/plugin/info", get(plugin_info_handler))
        .route("/api/v1/plugin/request", post(plugin_request_handler))
        .route("/api/v1/plugin/renew", post(plugin_renew_handler))
        .route("/api/v1/plugin/stop", delete(plugin_stop_handler))
        .route("/api/v1/plugin/stop_all", delete(plugin_stop_all_handler))
        .route("/api/v1/plugin/solve", post(plugin_solve_handler))
        .route("/api/v1/plugin/attempt", post(plugin_attempt_handler))
        // Player routes (CTFd user token) — for standalone monitor page
        .route("/api/v1/instance/request", post(instance_request_handler))
        .route("/api/v1/instance/info", get(instance_info_handler))
        .route("/api/v1/instance/renew", post(instance_renew_handler))
        .route("/api/v1/instance/stop", delete(instance_stop_handler))
        .with_state(Arc::clone(&state))
        .layer(DefaultBodyLimit::max(50 * 1024 * 1024));

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

// ── Auth helpers ──────────────────────────────────────────────────────────────

fn hash_token(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

fn extract_bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Token "))
}

fn extract_session_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            s.split(';').find_map(|part| {
                part.trim()
                    .strip_prefix("nervctf_session=")
                    .map(|v| v.to_string())
            })
        })
}

/// Check Authorization: Token header against hashed DB entries.
async fn check_monitor_auth(headers: &HeaderMap, db: &Db) -> bool {
    let token = match extract_bearer(headers) {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => return false,
    };
    let hash = hash_token(&token);
    let db = Arc::clone(db);
    tokio::task::spawn_blocking(move || db::validate_token_hash(&db, &hash))
        .await
        .ok()
        .and_then(|r| r.ok())
        .flatten()
        .is_some()
}

/// Check nervctf_session cookie against active DB sessions.
async fn check_session_auth(headers: &HeaderMap, db: &Db) -> bool {
    let session_id = match extract_session_cookie(headers) {
        Some(s) if !s.is_empty() => s,
        _ => return false,
    };
    let db = Arc::clone(db);
    tokio::task::spawn_blocking(move || db::validate_session(&db, &session_id))
        .await
        .ok()
        .and_then(|r| r.ok())
        .flatten()
        .is_some()
}

/// Accept either Authorization: Token header or session cookie.
async fn check_any_auth(headers: &HeaderMap, db: &Db) -> bool {
    check_monitor_auth(headers, db).await || check_session_auth(headers, db).await
}

fn session_expires_at() -> String {
    instance::expires_at_string(60 * 24)
}

/// Validate a CTFd user token and return team_id via direct MariaDB lookup.
async fn validate_ctfd_token(
    pool: &mysql_async::Pool,
    token: &str,
) -> Option<i64> {
    ctfd_db::validate_token(pool, token).await
}

// ── Basic handlers ────────────────────────────────────────────────────────────

async fn health_handler() -> impl IntoResponse {
    Json(json!({"status": "ok"}))
}

// ── Instance HTML page ────────────────────────────────────────────────────────

async fn instance_page_handler(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    // Check the challenge is registered
    let known = db::get_config(&state.db, &name)
        .ok()
        .flatten()
        .is_some();

    if !known {
        return (StatusCode::NOT_FOUND, Html("<h1>Challenge not found</h1>".to_string()))
            .into_response();
    }

    let monitor_origin = format!("http://{}:{}", state.public_host,
        env::var("MONITOR_PORT").unwrap_or_else(|_| "33133".to_string()));

    // Inline HTML — uses textContent exclusively (no innerHTML) to prevent XSS
    let html = format!(r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Instance: {name}</title>
<style>
  body {{ font-family: monospace; max-width: 600px; margin: 60px auto; padding: 0 20px; background: #111; color: #eee; }}
  h1 {{ font-size: 1.4rem; margin-bottom: 0.3rem; }}
  label {{ display: block; margin-top: 1rem; font-size: 0.85rem; color: #aaa; }}
  input {{ width: 100%; padding: 6px; background: #222; border: 1px solid #444; color: #eee; font-family: monospace; box-sizing: border-box; }}
  button {{ margin-top: 0.8rem; margin-right: 0.4rem; padding: 7px 16px; background: #1a6e3c; border: none; color: #fff; cursor: pointer; font-family: monospace; }}
  button.danger {{ background: #6e1a1a; }}
  button.secondary {{ background: #2c4a6e; }}
  #status {{ margin-top: 1.2rem; padding: 10px; background: #1a1a1a; border-left: 3px solid #444; white-space: pre-wrap; min-height: 2rem; }}
  #conn {{ margin-top: 1rem; padding: 10px; background: #1a2a1a; border-left: 3px solid #1a6e3c; display: none; }}
  #conn code {{ display: block; margin-top: 0.4rem; font-size: 1rem; color: #7fff7f; }}
</style>
</head>
<body>
<h1>Instance: {name}</h1>
<p id="challenge-name" style="color:#888;font-size:0.85rem"></p>

<label for="token">CTFd API Token</label>
<input type="password" id="token" placeholder="Paste your CTFd API token here">

<div>
  <button onclick="requestInstance()">Request Instance</button>
  <button class="secondary" onclick="getInfo()">Check Status</button>
  <button class="secondary" onclick="renewInstance()">Renew</button>
  <button class="danger" onclick="stopInstance()">Stop</button>
</div>

<div id="conn">
  <span id="conn-label">Connection:</span>
  <code id="conn-str"></code>
  <small id="conn-expires"></small>
</div>

<div id="status">Ready. Paste your token and click a button.</div>

<script>
const MONITOR = {monitor_origin_js};
const CHALLENGE = {challenge_name_js};

function setStatus(msg) {{
  document.getElementById('status').textContent = msg;
}}

function showConn(host, port, conn_type, expires_at) {{
  var connDiv = document.getElementById('conn');
  var connStr = document.getElementById('conn-str');
  var connExpires = document.getElementById('conn-expires');
  connDiv.style.display = 'block';
  if (conn_type === 'nc') {{
    connStr.textContent = 'nc ' + host + ' ' + port;
  }} else if (conn_type === 'http') {{
    connStr.textContent = 'http://' + host + ':' + port;
  }} else if (conn_type === 'ssh') {{
    connStr.textContent = 'ssh user@' + host + ' -p ' + port;
  }} else {{
    connStr.textContent = host + ':' + port;
  }}
  connExpires.textContent = 'Expires: ' + expires_at;
}}

function hideConn() {{
  document.getElementById('conn').style.display = 'none';
}}

function getToken() {{
  var t = document.getElementById('token').value.trim();
  if (!t) {{ setStatus('Please enter your CTFd API token.'); return null; }}
  return t;
}}

function apiCall(method, endpoint, body, token, onOk) {{
  setStatus('Working...');
  var opts = {{
    method: method,
    headers: {{ 'Authorization': 'Token ' + token, 'Content-Type': 'application/json' }}
  }};
  if (body) opts.body = JSON.stringify(body);
  fetch(MONITOR + endpoint, opts)
    .then(function(r) {{ return r.json().then(function(d) {{ return {{ok: r.ok, data: d}}; }}); }})
    .then(function(res) {{
      if (!res.ok) {{
        setStatus('Error: ' + (res.data.error || JSON.stringify(res.data)));
        hideConn();
      }} else {{
        onOk(res.data);
      }}
    }})
    .catch(function(e) {{ setStatus('Network error: ' + e.message); }});
}}

function requestInstance() {{
  var t = getToken(); if (!t) return;
  apiCall('POST', '/api/v1/instance/request', {{challenge_name: CHALLENGE}}, t, function(d) {{
    setStatus('Instance running!');
    showConn(d.host, d.port, d.connection_type, d.expires_at);
  }});
}}

function getInfo() {{
  var t = getToken(); if (!t) return;
  apiCall('GET', '/api/v1/instance/info?challenge_name=' + encodeURIComponent(CHALLENGE), null, t, function(d) {{
    if (d.status === 'running') {{
      setStatus('Running.');
      showConn(d.host, d.port, d.connection_type, d.expires_at);
    }} else {{
      setStatus('No active instance for this challenge.');
      hideConn();
    }}
  }});
}}

function renewInstance() {{
  var t = getToken(); if (!t) return;
  apiCall('POST', '/api/v1/instance/renew', {{challenge_name: CHALLENGE}}, t, function(d) {{
    setStatus('Renewed! New expiry: ' + d.expires_at);
    showConn(d.host, d.port, d.connection_type, d.expires_at);
  }});
}}

function stopInstance() {{
  var t = getToken(); if (!t) return;
  apiCall('DELETE', '/api/v1/instance/stop', {{challenge_name: CHALLENGE}}, t, function(d) {{
    setStatus('Instance stopped.');
    hideConn();
  }});
}}
</script>
</body>
</html>
"#,
        name = html_escape(&name),
        monitor_origin_js = serde_json::to_string(&monitor_origin).unwrap(),
        challenge_name_js = serde_json::to_string(&name).unwrap(),
    );

    Html(html).into_response()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
     .replace('<', "&lt;")
     .replace('>', "&gt;")
     .replace('"', "&quot;")
}

// ── Admin: instance register ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct RegisterRequest {
    challenge_name: String,
    ctfd_id: u32,
    backend: String,
    config_json: String,
}

async fn instance_register_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<RegisterRequest>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    match db::upsert_config(&state.db, &body.challenge_name, body.ctfd_id, &body.backend, &body.config_json) {
        Ok(_) => Json(json!({"ok": true})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

// ── Admin: instance list ──────────────────────────────────────────────────────

async fn instance_list_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    match db::list_configs(&state.db) {
        Ok(list) => Json(json!({"configs": list})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

// ── Admin: image build ────────────────────────────────────────────────────────

async fn instance_build_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }

    let mut challenge_name: Option<String> = None;
    let mut tar_bytes: Option<Vec<u8>> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name() {
            Some("challenge_name") => {
                challenge_name = field.text().await.ok();
            }
            Some("context") => {
                tar_bytes = field.bytes().await.ok().map(|b| b.to_vec());
            }
            _ => {}
        }
    }

    let challenge_name = match challenge_name {
        Some(n) => n,
        None => return (StatusCode::BAD_REQUEST, Json(json!({"error": "missing challenge_name"}))).into_response(),
    };
    let tar_bytes = match tar_bytes {
        Some(b) => b,
        None => return (StatusCode::BAD_REQUEST, Json(json!({"error": "missing context tar"}))).into_response(),
    };

    // Write tar to temp file
    let tmp = match tempfile::NamedTempFile::new() {
        Ok(f) => f,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    };
    if let Err(e) = std::fs::write(tmp.path(), &tar_bytes) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
    }

    let image_tag = format!("{}:latest", instance::sanitize_name(&challenge_name));

    if let Err(e) = instance::docker::build_image(tmp.path(), &image_tag, state.runner_ssh_target.as_deref()).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
    }

    if let Err(e) = db::update_image_tag(&state.db, &challenge_name, &image_tag) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
    }

    Json(json!({"ok": true, "image_tag": image_tag})).into_response()
}

// ── Admin: compose build ──────────────────────────────────────────────────────

async fn build_compose_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }

    let mut challenge_name: Option<String> = None;
    let mut tar_bytes: Option<Vec<u8>> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name() {
            Some("challenge_name") => {
                challenge_name = field.text().await.ok();
            }
            Some("context") => {
                tar_bytes = field.bytes().await.ok().map(|b| b.to_vec());
            }
            _ => {}
        }
    }

    let challenge_name = match challenge_name {
        Some(n) => n,
        None => return (StatusCode::BAD_REQUEST, Json(json!({"error": "missing challenge_name"}))).into_response(),
    };
    let tar_bytes = match tar_bytes {
        Some(b) => b,
        None => return (StatusCode::BAD_REQUEST, Json(json!({"error": "missing context tar"}))).into_response(),
    };

    let sanitized = instance::sanitize_name(&challenge_name);
    let extract_dir = format!("{}/{}", state.challenges_base_dir.trim_end_matches('/'), sanitized);

    // Determine compose file path from DB config (if challenge is already registered)
    let compose_file_str = db::get_config(&state.db, &challenge_name)
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
        .and_then(|v| v["compose_file"].as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "docker-compose.yml".to_string());

    let compose_path = if compose_file_str.starts_with('/') {
        compose_file_str.clone()
    } else {
        format!("{}/{}", extract_dir, compose_file_str)
    };

    if let Some(ref target) = state.runner_ssh_target {
        // ── Split-machine mode: extract tar and build on the runner via SSH ──
        info!("build-compose: uploading {} bytes to runner:{}", tar_bytes.len(), extract_dir);

        // Write tar to a temp file locally first
        let tmp = match tempfile::NamedTempFile::new() {
            Ok(f) => f,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
        };
        if let Err(e) = std::fs::write(tmp.path(), &tar_bytes) {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
        }

        // Wipe + create dir, extract tar, and build — all on the runner in one SSH session.
        // -p pins the project name to the challenge directory basename so built images are
        // tagged <sanitized>-<service>, matching the image: reference in compose::up overrides.
        let remote_cmd = format!(
            "rm -rf '{dir}' && mkdir -p '{dir}' && tar -xzf - -C '{dir}' && DOCKER_BUILDKIT=1 docker compose -f '{compose}' -p '{project}' build",
            dir = extract_dir,
            compose = compose_path,
            project = sanitized,
        );

        let extract_out = tokio::process::Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                "-o", "BatchMode=yes",
                target,
                &remote_cmd,
            ])
            .stdin(std::process::Stdio::from(
                std::fs::File::open(tmp.path()).unwrap(),
            ))
            .output()
            .await;

        match extract_out {
            Ok(out) if out.status.success() => {
                info!("build-compose: images built on runner for {}", challenge_name);
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr).to_string();
                error!("build-compose: remote build failed for {}: {}", challenge_name, err);
                return (StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": format!("remote build failed: {}", err)}))).into_response();
            }
            Err(e) => {
                error!("build-compose: ssh spawn failed: {}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
            }
        }
    } else {
        // ── Single-machine mode: extract locally and build locally ──

        // Write tar to temp file
        let tmp = match tempfile::NamedTempFile::new() {
            Ok(f) => f,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
        };
        if let Err(e) = std::fs::write(tmp.path(), &tar_bytes) {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
        }

        // Wipe any existing challenge directory so stale placeholder directories
        // cannot block tar from extracting files over them.
        if std::path::Path::new(&extract_dir).exists() {
            if let Err(e) = std::fs::remove_dir_all(&extract_dir) {
                error!("build-compose: failed to remove existing {}: {}", extract_dir, e);
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
            }
        }
        if let Err(e) = std::fs::create_dir_all(&extract_dir) {
            error!("build-compose: failed to create {}: {}", extract_dir, e);
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
        }
        info!("build-compose: extracting {} bytes to {}", tar_bytes.len(), extract_dir);

        let extract_out = tokio::process::Command::new("tar")
            .args(["-xzf", tmp.path().to_str().unwrap_or(""), "-C", &extract_dir])
            .output()
            .await;

        match extract_out {
            Ok(out) if out.status.success() => {
                info!("build-compose: extraction complete for {}", challenge_name);
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr).to_string();
                error!("build-compose: tar extraction failed for {}: {}", challenge_name, err);
                return (StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": format!("tar extraction failed: {}", err)}))).into_response();
            }
            Err(e) => {
                error!("build-compose: tar spawn failed: {}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
            }
        }

        info!("build-compose: building images with compose file {}", compose_path);

        if let Err(e) = instance::compose::build(&compose_path, None).await {
            error!("build-compose: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()}))).into_response();
        }
        info!("build-compose: images built successfully for {}", challenge_name);
    }

    Json(json!({"ok": true, "compose_dir": extract_dir})).into_response()
}

// ── Admin: build compose images on runner (split-machine mode) ───────────────
// Called by the CLI after it has rsynced challenge files directly to the runner.
// No multipart — just a JSON body telling us which challenge and compose file.

#[derive(Deserialize)]
struct BuildComposeRemoteBody {
    challenge_name: String,
    #[serde(default)]
    compose_file: Option<String>,
    #[serde(default)]
    challenges_dir: Option<String>,
}

async fn build_compose_remote_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<BuildComposeRemoteBody>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }

    let target = match &state.runner_ssh_target {
        Some(t) => t.clone(),
        None => return (StatusCode::BAD_REQUEST,
            Json(json!({"error": "build-compose-remote requires split-machine mode (RUNNER_SSH_TARGET)"}))).into_response(),
    };

    let sanitized = instance::sanitize_name(&body.challenge_name);
    let base_dir = body.challenges_dir.as_deref()
        .unwrap_or(&state.challenges_base_dir);
    let extract_dir = format!("{}/{}", base_dir.trim_end_matches('/'), sanitized);

    let compose_from_db = db::get_config(&state.db, &body.challenge_name)
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
        .and_then(|v| v["compose_file"].as_str().map(|s| s.to_string()));

    let compose_file_str = body.compose_file.as_deref()
        .or(compose_from_db.as_deref())
        .unwrap_or("docker-compose.yml");

    let compose_path = if compose_file_str.starts_with('/') {
        compose_file_str.to_string()
    } else {
        format!("{}/{}", extract_dir, compose_file_str)
    };

    info!("build-compose-remote: building images on runner for {} ({})", body.challenge_name, compose_path);

    if let Err(e) = instance::compose::build(&compose_path, Some(&target)).await {
        error!("build-compose-remote: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()}))).into_response();
    }

    info!("build-compose-remote: images built successfully for {}", body.challenge_name);
    Json(json!({"ok": true, "compose_dir": extract_dir})).into_response()
}

// ── Connection helpers ────────────────────────────────────────────────────────

/// Load and parse a challenge's config JSON from the DB, returning a default Value on error.
fn load_config_val(db: &crate::db::Db, challenge_name: &str) -> Value {
    db::get_config(db, challenge_name)
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

/// Build a per-service labeled connections array for `service_ports` challenges.
/// Returns `None` when the config has no `service_ports` key (single-service challenges).
/// Each entry: `{"label": "<service>", "type": "<connection_type>", "host": "...", "port": N}`.
fn build_connections(config: &Value, host: &str, port: u16, connection_type: &str, extra_ports: Option<&str>) -> Option<Value> {
    let svc_ports_obj = config["service_ports"].as_object()?;
    let extra: serde_json::Map<String, Value> = extra_ports
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let mut connections: Vec<Value> = Vec::new();
    for (svc, ports_val) in svc_ports_obj {
        let iports: Vec<u32> = ports_val.as_array()
            .map(|a| a.iter().filter_map(|v| v.as_u64().map(|p| p as u32)).collect())
            .unwrap_or_default();
        for ip in &iports {
            let host_port = extra.get(&ip.to_string())
                .and_then(|v| v.as_u64())
                .map(|p| p as u16)
                .unwrap_or(port);
            connections.push(json!({
                "label": svc,
                "type": connection_type,
                "host": host,
                "port": host_port
            }));
        }
    }
    if connections.is_empty() { None } else { Some(json!(connections)) }
}

// ── Player: request instance ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct ChallengeNameBody {
    challenge_name: String,
}

async fn instance_request_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ChallengeNameBody>,
) -> impl IntoResponse {
    let token = match extract_bearer(&headers) {
        Some(t) => t.to_string(),
        None => return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response(),
    };

    let team_id = match validate_ctfd_token(&state.ctfd_pool, &token).await {
        Some(id) => id,
        None => return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Invalid CTFd token or not in a team"}))).into_response(),
    };

    // Check for existing running or provisioning instance
    if let Ok(Some(inst)) = db::get_instance(&state.db, &body.challenge_name, team_id) {
        if inst.status == "running" {
            let cfg = load_config_val(&state.db, &body.challenge_name);
            let connections = build_connections(&cfg, &inst.host, inst.port as u16, &inst.connection_type, inst.extra_ports.as_deref());
            return Json(json!({
                "status": "running",
                "host": inst.host,
                "port": inst.port,
                "connection_type": inst.connection_type,
                "expires_at": inst.expires_at,
                "extra_ports": inst.extra_ports.as_deref().and_then(|s| serde_json::from_str::<Value>(s).ok()),
                "connections": connections,
            })).into_response();
        }
        if inst.status == "provisioning" {
            return (StatusCode::CONFLICT, Json(json!({"error": "Already provisioning", "status": "provisioning"}))).into_response();
        }
    }

    // Get config
    let config_json = match db::get_config(&state.db, &body.challenge_name) {
        Ok(Some(j)) => j,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"error": "Challenge not registered"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    };

    let config: Value = match serde_json::from_str(&config_json) {
        Ok(v) => v,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    };

    // Enforce global per-team instance cap.
    if state.max_instances_per_team > 0 {
        match db::count_active_instances_for_team(&state.db, team_id) {
            Ok(n) if n as u64 >= state.max_instances_per_team => {
                return (StatusCode::CONFLICT, Json(json!({
                    "error": format!("Instance limit reached: your team already has {} active instance(s) (max {})", n, state.max_instances_per_team)
                }))).into_response();
            }
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
            _ => {}
        }
    }

    // Derive placeholder values for the provisioning stub
    let connection_type = config["connection"].as_str().unwrap_or("nc").to_string();
    let timeout_minutes = config["timeout_minutes"].as_u64().unwrap_or(45);
    let expires_at = instance::expires_at_string(timeout_minutes);

    // Pre-generate the container/project name so the orphan checker sees it as tracked
    // immediately (before compose::up returns and updates the row).
    let pre_container_name = instance::container_name(&body.challenge_name);

    // Insert provisioning stub before spawning; UNIQUE constraint means a concurrent
    // request racing past the check above will hit INSERT OR IGNORE and we return 409.
    match db::insert_provisioning_stub(
        &state.db, &body.challenge_name, team_id, None,
        &state.public_host, &connection_type, &expires_at,
        Some(&pre_container_name),
    ) {
        Ok(()) => {}
        Err(e) => {
            // If a row already exists (race), treat as 409
            let msg = e.to_string();
            if msg.contains("UNIQUE") || msg.contains("unique") {
                return (StatusCode::CONFLICT, Json(json!({"error": "Already provisioning", "status": "provisioning"}))).into_response();
            }
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": msg}))).into_response();
        }
    }

    // Provision in background — compose up can take 30-60 s.
    let state_bg = Arc::clone(&state);
    let challenge_name_bg = body.challenge_name.clone();
    let container_name_bg = pre_container_name;
    tokio::spawn(async move {
        let _permit = match state_bg.provision_sem.acquire().await {
            Ok(p) => p,
            Err(_) => {
                error!("provision_bg (player): semaphore closed for '{}' team {}", challenge_name_bg, team_id);
                let _ = db::delete_instance(&state_bg.db, &challenge_name_bg, team_id);
                return;
            }
        };
        info!("provision_bg (player): starting '{}' team {}", challenge_name_bg, team_id);
        match instance::provision(
            &state_bg.db, &challenge_name_bg, team_id, None,
            &config, &state_bg.public_host, &state_bg.ctfd_pool,
            state_bg.runner_ssh_target.as_deref(),
            Some(container_name_bg),
        ).await {
            Ok((host, port, conn, _exp)) => {
                info!("provision_bg (player): done {}:{} ({}) for '{}' team {}", host, port, conn, challenge_name_bg, team_id);
            }
            Err(e) => {
                error!("provision_bg (player): '{}' team {} error: {}", challenge_name_bg, team_id, e);
                let _ = db::delete_instance(&state_bg.db, &challenge_name_bg, team_id);
            }
        }
    });

    Json(json!({
        "status": "provisioning",
        "host": state.public_host,
        "port": 0,
        "connection_type": connection_type,
        "expires_at": expires_at,
    })).into_response()
}

// ── Player: get instance info ─────────────────────────────────────────────────

async fn instance_info_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let token = match extract_bearer(&headers) {
        Some(t) => t.to_string(),
        None => return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response(),
    };

    let team_id = match validate_ctfd_token(&state.ctfd_pool, &token).await {
        Some(id) => id,
        None => return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Invalid CTFd token"}))).into_response(),
    };

    let challenge_name = match params.get("challenge_name") {
        Some(n) => n.clone(),
        None => return (StatusCode::BAD_REQUEST, Json(json!({"error": "missing challenge_name"}))).into_response(),
    };

    match db::get_instance(&state.db, &challenge_name, team_id) {
        Ok(Some(inst)) => {
            let cfg = load_config_val(&state.db, &challenge_name);
            let connections = build_connections(&cfg, &inst.host, inst.port as u16, &inst.connection_type, inst.extra_ports.as_deref());
            Json(json!({
                "status": inst.status,
                "host": inst.host,
                "port": inst.port,
                "connection_type": inst.connection_type,
                "expires_at": inst.expires_at,
                "renewals_used": inst.renewals_used,
                "extra_ports": inst.extra_ports.as_deref().and_then(|s| serde_json::from_str::<Value>(s).ok()),
                "connections": connections,
            })).into_response()
        }
        Ok(None) => Json(json!({"status": "none"})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

// ── Player: renew instance ────────────────────────────────────────────────────

async fn instance_renew_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ChallengeNameBody>,
) -> impl IntoResponse {
    let token = match extract_bearer(&headers) {
        Some(t) => t.to_string(),
        None => return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response(),
    };

    let team_id = match validate_ctfd_token(&state.ctfd_pool, &token).await {
        Some(id) => id,
        None => return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Invalid CTFd token"}))).into_response(),
    };

    let inst = match db::get_instance(&state.db, &body.challenge_name, team_id) {
        Ok(Some(i)) => i,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"error": "No active instance"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    };

    let config_val: Value = db::get_config(&state.db, &body.challenge_name)
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default();
    let timeout_minutes = config_val["timeout_minutes"].as_u64().unwrap_or(45);
    let max_renewals = config_val["max_renewals"].as_u64().unwrap_or(3);

    if inst.renewals_used >= max_renewals as i64 {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "Maximum renewals reached"}))).into_response();
    }

    let new_expires = instance::expires_at_string(timeout_minutes);
    if let Err(e) = db::update_expires_at(&state.db, &body.challenge_name, team_id, &new_expires) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
    }

    let connections = build_connections(&config_val, &inst.host, inst.port as u16, &inst.connection_type, inst.extra_ports.as_deref());
    Json(json!({
        "host": inst.host,
        "port": inst.port,
        "connection_type": inst.connection_type,
        "expires_at": new_expires,
        "extra_ports": inst.extra_ports.as_deref().and_then(|s| serde_json::from_str::<Value>(s).ok()),
        "connections": connections,
    })).into_response()
}

// ── Player: stop instance ─────────────────────────────────────────────────────

async fn instance_stop_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ChallengeNameBody>,
) -> impl IntoResponse {
    let token = match extract_bearer(&headers) {
        Some(t) => t.to_string(),
        None => return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response(),
    };

    let team_id = match validate_ctfd_token(&state.ctfd_pool, &token).await {
        Some(id) => id,
        None => return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Invalid CTFd token"}))).into_response(),
    };

    match db::delete_instance(&state.db, &body.challenge_name, team_id) {
        Ok(Some((container_id, ctfd_flag_id))) => {
            if let Some(cid) = container_id {
                instance::cleanup_container(&cid, state.runner_ssh_target.as_deref()).await;
            }
            if let Some(flag_id) = ctfd_flag_id {
                ctfd_db::delete_flag(&state.ctfd_pool, flag_id).await;
            }
            Json(json!({"ok": true})).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({"error": "No active instance"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

// ── Plugin handlers (monitor token + explicit team_id) ────────────────────────

#[derive(Deserialize)]
#[allow(dead_code)]
struct PluginTeamBody {
    challenge_name: String,
    team_id: i64,
    user_id: Option<i64>,
}


async fn plugin_info_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    let challenge_name = match params.get("challenge_name") {
        Some(n) => n.clone(),
        None => return (StatusCode::BAD_REQUEST, Json(json!({"error": "missing challenge_name"}))).into_response(),
    };
    let team_id: i64 = match params.get("team_id").and_then(|s| s.parse().ok()) {
        Some(id) => id,
        None => return (StatusCode::BAD_REQUEST, Json(json!({"error": "missing team_id"}))).into_response(),
    };
    match db::get_instance(&state.db, &challenge_name, team_id) {
        Ok(Some(inst)) => {
            let cfg = load_config_val(&state.db, &challenge_name);
            let connections = build_connections(&cfg, &inst.host, inst.port as u16, &inst.connection_type, inst.extra_ports.as_deref());
            Json(json!({
                "status": inst.status,
                "host": inst.host,
                "port": inst.port,
                "connection_type": inst.connection_type,
                "expires_at": inst.expires_at,
                "renewals_used": inst.renewals_used,
                "container_id": inst.container_id,
                "flag": inst.flag,
                "extra_ports": inst.extra_ports.as_deref().and_then(|s| serde_json::from_str::<Value>(s).ok()),
                "connections": connections,
            })).into_response()
        }
        Ok(None) => Json(json!({"status": "none"})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

async fn plugin_request_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PluginTeamBody>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        warn!("plugin_request: unauthorized");
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }

    info!("plugin_request: challenge={} team_id={}", body.challenge_name, body.team_id);

    // Reject if the team already solved this challenge
    if db::has_correct_solve(&state.db, &body.challenge_name, body.team_id).unwrap_or(false) {
        info!("plugin_request: team {} already solved '{}', rejecting provision", body.team_id, body.challenge_name);
        return (StatusCode::CONFLICT, Json(json!({"error": "Challenge already solved", "solved": true}))).into_response();
    }

    // Return existing running or in-progress instance
    if let Ok(Some(inst)) = db::get_instance(&state.db, &body.challenge_name, body.team_id) {
        if inst.status == "running" || inst.status == "provisioning" {
            info!("plugin_request: returning existing {} instance for {}/{}", inst.status, body.challenge_name, body.team_id);
            let cfg = load_config_val(&state.db, &body.challenge_name);
            let connections = if inst.status == "running" {
                build_connections(&cfg, &inst.host, inst.port as u16, &inst.connection_type, inst.extra_ports.as_deref())
            } else { None };
            return Json(json!({
                "status": inst.status,
                "host": inst.host,
                "port": inst.port,
                "connection_type": inst.connection_type,
                "expires_at": inst.expires_at,
                "extra_ports": inst.extra_ports.as_deref().and_then(|s| serde_json::from_str::<Value>(s).ok()),
                "connections": connections,
            })).into_response();
        }
    }

    let config_json = match db::get_config(&state.db, &body.challenge_name) {
        Ok(Some(j)) => j,
        Ok(None) => {
            warn!("plugin_request: challenge '{}' not registered in db", body.challenge_name);
            return (StatusCode::NOT_FOUND, Json(json!({"error": "Challenge not registered"}))).into_response();
        }
        Err(e) => {
            error!("plugin_request: db error getting config: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
        }
    };
    let config: serde_json::Value = match serde_json::from_str(&config_json) {
        Ok(v) => v,
        Err(e) => {
            error!("plugin_request: bad config json for '{}': {}", body.challenge_name, e);
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
        }
    };

    // Enforce global per-team instance cap.
    if state.max_instances_per_team > 0 {
        match db::count_active_instances_for_team(&state.db, body.team_id) {
            Ok(n) if n as u64 >= state.max_instances_per_team => {
                info!("plugin_request: team {} hit instance cap ({}/{}), rejecting", body.team_id, n, state.max_instances_per_team);
                return (StatusCode::CONFLICT, Json(json!({
                    "error": format!("Instance limit reached: your team already has {} active instance(s) (max {})", n, state.max_instances_per_team)
                }))).into_response();
            }
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
            _ => {}
        }
    }

    // Derive reasonable placeholder values for the provisioning stub
    let connection_type = config["connection"].as_str().unwrap_or("nc").to_string();
    let timeout_minutes = config["timeout_minutes"].as_u64().unwrap_or(45);
    let expires_at = instance::expires_at_string(timeout_minutes);

    // Pre-generate the container/project name so the orphan checker sees it as tracked
    // immediately (before compose::up returns and updates the row).
    let pre_container_name = instance::container_name(&body.challenge_name);

    // Insert stub immediately so the client can poll for status
    if let Err(e) = db::insert_provisioning_stub(
        &state.db, &body.challenge_name, body.team_id, body.user_id,
        &state.public_host, &connection_type, &expires_at,
        Some(&pre_container_name),
    ) {
        error!("plugin_request: failed to insert provisioning stub: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
    }

    // Provision in background — compose up can take 30-60 s.
    // Semaphore limits concurrent provisions to prevent port-pick races and
    // avoid overwhelming the Docker daemon socket under high concurrency.
    let state_bg = Arc::clone(&state);
    let challenge_name_bg = body.challenge_name.clone();
    let team_id_bg = body.team_id;
    let user_id_bg = body.user_id;
    let container_name_bg = pre_container_name;
    tokio::spawn(async move {
        let _permit = match state_bg.provision_sem.acquire().await {
            Ok(p) => p,
            Err(_) => {
                error!("provision_bg: semaphore closed for '{}' team {}", challenge_name_bg, team_id_bg);
                let _ = db::delete_instance(&state_bg.db, &challenge_name_bg, team_id_bg);
                return;
            }
        };
        info!("provision_bg: starting '{}' team {}", challenge_name_bg, team_id_bg);
        match instance::provision(
            &state_bg.db, &challenge_name_bg, team_id_bg, user_id_bg,
            &config, &state_bg.public_host, &state_bg.ctfd_pool,
            state_bg.runner_ssh_target.as_deref(),
            Some(container_name_bg),
        ).await {
            Ok((host, port, conn, _exp)) => {
                info!("provision_bg: done {}:{} ({}) for '{}' team {}", host, port, conn, challenge_name_bg, team_id_bg);
            }
            Err(e) => {
                error!("provision_bg: '{}' team {} error: {}", challenge_name_bg, team_id_bg, e);
                // Remove the stub so the client can retry
                let _ = db::delete_instance(&state_bg.db, &challenge_name_bg, team_id_bg);
            }
        }
        // _permit dropped here — releases the semaphore slot
    });

    Json(json!({
        "status": "provisioning",
        "host": state.public_host,
        "port": 0,
        "connection_type": connection_type,
        "expires_at": expires_at,
    })).into_response()
}

async fn plugin_renew_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PluginTeamBody>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }

    let inst = match db::get_instance(&state.db, &body.challenge_name, body.team_id) {
        Ok(Some(i)) => i,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"error": "No active instance"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    };

    let cfg = load_config_val(&state.db, &body.challenge_name);
    let timeout_minutes = cfg["timeout_minutes"].as_u64().unwrap_or(45);
    let max_renewals = cfg["max_renewals"].as_u64().unwrap_or(3);

    if inst.renewals_used >= max_renewals as i64 {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "Maximum renewals reached"}))).into_response();
    }

    let new_expires = instance::expires_at_string(timeout_minutes);
    if let Err(e) = db::update_expires_at(&state.db, &body.challenge_name, body.team_id, &new_expires) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
    }

    let connections = build_connections(&cfg, &inst.host, inst.port as u16, &inst.connection_type, inst.extra_ports.as_deref());
    Json(json!({
        "host": inst.host,
        "port": inst.port,
        "connection_type": inst.connection_type,
        "expires_at": new_expires,
        "extra_ports": inst.extra_ports.as_deref().and_then(|s| serde_json::from_str::<Value>(s).ok()),
        "connections": connections,
    })).into_response()
}

async fn plugin_stop_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PluginTeamBody>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    match db::delete_instance(&state.db, &body.challenge_name, body.team_id) {
        Ok(Some((container_id, ctfd_flag_id))) => {
            if let Some(flag_id) = ctfd_flag_id {
                ctfd_db::delete_flag(&state.ctfd_pool, flag_id).await;
            }
            // Container teardown in background — compose down is slow.
            if let Some(cid) = container_id {
                let ssh = state.runner_ssh_target.clone();
                tokio::spawn(async move { instance::cleanup_container(&cid, ssh.as_deref()).await; });
            }
            Json(json!({"ok": true})).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({"error": "No active instance"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

#[derive(Deserialize)]
struct PluginSolveBody {
    challenge_name: String,
    team_id: i64,
    user_id: Option<i64>,
    submitted_flag: Option<String>,
}

/// Called by the CTFd plugin when a team solves an instance challenge.
/// Deletes the DB record immediately and returns 200, then tears down the
/// container and CTFd flag in the background so the plugin doesn't time out
/// waiting for `docker compose down`.
/// Also records the correct solve in flag_attempts — this is the authoritative
/// source since solve() is only called by CTFd for genuine correct submissions.
async fn plugin_solve_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PluginSolveBody>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    match db::mark_instance_solved(&state.db, &body.challenge_name, body.team_id) {
        Ok(Some((container_id, ctfd_flag_id))) => {
            if let Some(flag_id) = ctfd_flag_id {
                ctfd_db::delete_flag(&state.ctfd_pool, flag_id).await;
            }
            // Record the correct solve. This happens after mark_instance_solved() purges
            // incorrect attempts, so the correct row is always persisted.
            if let (Some(flag), Some(uid)) = (&body.submitted_flag, body.user_id) {
                if !flag.is_empty() {
                    let _ = db::insert_flag_attempt(
                        &state.db, &body.challenge_name, body.team_id, uid, flag, true, false, None,
                    );
                }
            }
            // Container teardown in background — compose down is slow.
            if let Some(cid) = container_id {
                let ssh = state.runner_ssh_target.clone();
                tokio::spawn(async move { instance::cleanup_container(&cid, ssh.as_deref()).await; });
            }
            Json(json!({"ok": true})).into_response()
        }
        // Instance already gone (expired or manually stopped) — not an error
        Ok(None) => Json(json!({"ok": true})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

#[derive(Deserialize)]
struct ChallengeNameOnlyBody {
    challenge_name: String,
}

async fn plugin_stop_all_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ChallengeNameOnlyBody>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    match db::delete_all_instances_for_challenge(&state.db, &body.challenge_name) {
        Ok(pairs) => {
            // Tear down containers and delete CTFd flags in background so the
            // HTTP response is not blocked by potentially slow compose-down calls.
            let ssh = state.runner_ssh_target.clone();
            let pool = state.ctfd_pool.clone();
            tokio::spawn(async move {
                for (container_id, ctfd_flag_id) in pairs {
                    if let Some(cid) = container_id {
                        instance::cleanup_container(&cid, ssh.as_deref()).await;
                    }
                    if let Some(flag_id) = ctfd_flag_id {
                        let _ = ctfd_db::delete_flag(&pool, flag_id).await;
                    }
                }
            });
            Json(json!({"ok": true})).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

// ── Plugin: flag attempt ──────────────────────────────────────────────────────

#[derive(Deserialize)]
struct PluginAttemptBody {
    challenge_name: String,
    team_id: i64,
    user_id: i64,
    submitted_flag: String,
    is_correct: bool,
}

async fn plugin_attempt_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PluginAttemptBody>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }

    // Check for flag sharing: submitted flag belongs to a different team's instance
    let owner = db::find_flag_owner(&state.db, &body.challenge_name, &body.submitted_flag, body.team_id);
    let (is_flag_sharing, owner_team_id) = match owner {
        Ok(Some(owner_id)) => (true, Some(owner_id)),
        _ => (false, None),
    };

    if is_flag_sharing {
        warn!(
            "flag sharing detected: team {} (user {}) submitted flag belonging to team {} for challenge {}",
            body.team_id, body.user_id, owner_team_id.unwrap_or(-1), body.challenge_name
        );
    }

    match db::insert_flag_attempt(
        &state.db,
        &body.challenge_name,
        body.team_id,
        body.user_id,
        &body.submitted_flag,
        body.is_correct,
        is_flag_sharing,
        owner_team_id,
    ) {
        Ok(_) => Json(json!({"ok": true, "is_flag_sharing": is_flag_sharing})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

// ── Admin: dashboard + data endpoints ────────────────────────────────────────

async fn admin_dashboard_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !check_session_auth(&headers, &state.db).await {
        return Redirect::to("/").into_response();
    }
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_str!("../assets/admin.html"),
    ).into_response()
}

// ── Login page ────────────────────────────────────────────────────────────────

const LOGIN_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>NervCTF Monitor</title>
  <style>
    body{font-family:monospace;max-width:420px;margin:100px auto;padding:20px;background:#111;color:#eee}
    h1{font-size:1.4rem;margin-bottom:1.5rem}
    label{display:block;margin-top:1rem;font-size:.85rem;color:#aaa}
    input[type=password]{width:100%;padding:8px;background:#222;border:1px solid #444;color:#eee;font-family:monospace;box-sizing:border-box;margin-top:4px}
    button{margin-top:1.2rem;padding:8px 20px;background:#1a6e3c;border:none;color:#fff;cursor:pointer;font-family:monospace;width:100%;font-size:1rem}
    button:hover{background:#2a8e4c}
    #msg{margin-top:.8rem;color:#ff6b6b;font-size:.85rem;min-height:1rem}
  </style>
</head>
<body>
  <h1>NervCTF Monitor</h1>
  <form id="form">
    <label for="tok">Operator Token</label>
    <input type="password" id="tok" autocomplete="current-password" placeholder="Enter your operator token">
    <button type="submit">Login</button>
  </form>
  <div id="msg"></div>
  <script>
    document.getElementById('form').addEventListener('submit', async function(e) {
      e.preventDefault();
      var r = await fetch('/auth/login', {
        method: 'POST',
        headers: {'Content-Type': 'application/json'},
        body: JSON.stringify({token: document.getElementById('tok').value})
      });
      if (r.ok) { window.location.href = '/admin'; }
      else {
        var j = await r.json().catch(function(){return{};});
        document.getElementById('msg').textContent = j.error || 'Login failed';
      }
    });
  </script>
</body>
</html>"#;

async fn login_page_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        LOGIN_HTML,
    )
}

// ── Auth: login / logout ──────────────────────────────────────────────────────

#[derive(Deserialize)]
struct LoginBody {
    token: String,
}

async fn auth_login_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LoginBody>,
) -> impl IntoResponse {
    if body.token.is_empty() {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Token required"}))).into_response();
    }
    let hash = hash_token(&body.token);
    let db = Arc::clone(&state.db);
    let op_id = match tokio::task::spawn_blocking(move || db::validate_token_hash(&db, &hash)).await {
        Ok(Ok(Some(id))) => id,
        _ => return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Invalid token"}))).into_response(),
    };
    let session_id: String = rand::thread_rng()
        .sample_iter(rand::distributions::Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();
    let expires_at = session_expires_at();
    let db2 = Arc::clone(&state.db);
    let sid = session_id.clone();
    if let Err(e) = tokio::task::spawn_blocking(move || db::create_session(&db2, &sid, op_id, &expires_at))
        .await
        .unwrap_or_else(|e| Err(anyhow::anyhow!("spawn: {}", e)))
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
    }
    let cookie = format!("nervctf_session={}; HttpOnly; Path=/; SameSite=Strict; Max-Age=86400", session_id);
    (
        StatusCode::OK,
        [(axum::http::header::SET_COOKIE, cookie)],
        Json(json!({"ok": true})),
    ).into_response()
}

async fn auth_logout_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Some(session_id) = extract_session_cookie(&headers) {
        let db = Arc::clone(&state.db);
        let _ = tokio::task::spawn_blocking(move || db::delete_session(&db, &session_id)).await;
    }
    let cookie = "nervctf_session=; HttpOnly; Path=/; SameSite=Strict; Max-Age=0".to_string();
    (
        StatusCode::OK,
        [(axum::http::header::SET_COOKIE, cookie)],
        Json(json!({"ok": true})),
    ).into_response()
}

// ── Admin: token management ───────────────────────────────────────────────────

async fn list_tokens_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    let db = Arc::clone(&state.db);
    match tokio::task::spawn_blocking(move || db::list_operator_tokens(&db)).await {
        Ok(Ok(tokens)) => Json(json!(tokens)).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

#[derive(Deserialize)]
struct CreateTokenBody {
    label: String,
}

async fn create_token_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CreateTokenBody>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    let label = body.label.trim().to_string();
    if label.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "label is required"}))).into_response();
    }
    let plaintext: String = rand::thread_rng()
        .sample_iter(rand::distributions::Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();
    let hash = hash_token(&plaintext);
    let db = Arc::clone(&state.db);
    let lbl = label.clone();
    match tokio::task::spawn_blocking(move || db::insert_operator_token(&db, &lbl, &hash)).await {
        Ok(Ok(id)) => (StatusCode::OK, Json(json!({
            "id": id,
            "label": label,
            "token": plaintext,
        }))).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

async fn revoke_token_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    let db = Arc::clone(&state.db);
    match tokio::task::spawn_blocking(move || db::revoke_operator_token(&db, id)).await {
        Ok(Ok(true))  => Json(json!({"ok": true})).into_response(),
        Ok(Ok(false)) => (StatusCode::NOT_FOUND, Json(json!({"error": "Token not found"}))).into_response(),
        Ok(Err(e))    => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
        Err(e)        => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

// ── Admin: dashboard data endpoints ──────────────────────────────────────────

async fn admin_config_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    Json(json!({ "ctfd_url": state.ctfd_url })).into_response()
}

/// `GET /api/v1/admin/probe[?refresh=true]`
///
/// Returns the CTFd compatibility probe result as a JSON object.
/// Pass `?refresh=true` to force a fresh probe against MariaDB (default: return the
/// cached row from SQLite).  If no cached result exists the probe always runs.
///
/// Response envelope: `{"success": true, "data": <ProbeResult>}`
/// HTTP 503 if `refresh=true` and the MariaDB connection is unavailable.
async fn admin_probe_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return ctfd_err(StatusCode::UNAUTHORIZED, "Unauthorized");
    }

    let force_refresh = params.get("refresh").map(|v| v == "true").unwrap_or(false);

    // Try to load the cached result first (unless a fresh probe was requested).
    if !force_refresh {
        let db = Arc::clone(&state.db);
        match tokio::task::spawn_blocking(move || db::load_probe_result(&db)).await {
            Ok(Ok(Some(cached))) => {
                return ctfd_ok(serde_json::to_value(&cached).unwrap_or_default());
            }
            Ok(Ok(None)) => {
                // No cached result — fall through to run the probe.
            }
            Ok(Err(e)) => {
                tracing::warn!("admin_probe: failed to load cached result: {}", e);
                // Fall through to run the probe.
            }
            Err(e) => {
                tracing::warn!("admin_probe: spawn error loading cache: {}", e);
                // Fall through to run the probe.
            }
        }
    }

    // Run a fresh probe against MariaDB.
    let probe = ctfd_db::run_probe(&state.ctfd_pool).await;

    // Persist the new result (best-effort — do not fail the request if SQLite is broken).
    let db = Arc::clone(&state.db);
    let probe_for_save = probe.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(e) = db::save_probe_result(&db, &probe_for_save) {
            tracing::warn!("admin_probe: failed to save probe result: {}", e);
        }
    }).await.ok();

    // If the probe could not connect at all (all caps broken) return 503 when
    // the caller explicitly requested a refresh — a cached non-broken result was
    // unavailable and we could not produce a fresh one.
    if force_refresh
        && probe.cap_challenge_crud == "broken"
        && probe.cap_player_auth == "broken"
        && probe.cap_dynamic_scoring == "broken"
        && probe.cap_instance_flags == "broken"
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"success": false, "errors": {"message": "Could not connect to CTFd MariaDB"}})),
        ).into_response();
    }

    ctfd_ok(serde_json::to_value(&probe).unwrap_or_default())
}

async fn admin_instances_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    let db = Arc::clone(&state.db);
    match tokio::task::spawn_blocking(move || db::list_all_instances(&db)).await {
        Ok(Ok(list)) => Json(json!(list)).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

async fn admin_attempts_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    let alerts_only = params.get("alerts_only").map(|v| v == "true").unwrap_or(false);
    let db = Arc::clone(&state.db);
    let result = tokio::task::spawn_blocking(move || {
        if alerts_only { db::list_sharing_alerts(&db) } else { db::list_flag_attempts(&db, i64::MAX) }
    }).await;
    match result {
        Ok(Ok(list)) => Json(json!(list)).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

async fn admin_solves_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    let db = Arc::clone(&state.db);
    match tokio::task::spawn_blocking(move || db::list_correct_solves(&db)).await {
        Ok(Ok(list)) => Json(json!(list)).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

// ── Diff handler ──────────────────────────────────────────────────────────────

async fn diff_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }

    let local_challenges: Vec<Value> = match serde_json::from_slice::<Value>(&body) {
        Ok(v) => v
            .get("challenges")
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default(),
        Err(e) => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": format!("Invalid JSON: {}", e)}))).into_response();
        }
    };

    let remote_challenges = match ctfd_db::list_challenges(&state.ctfd_pool).await {
        Ok(v) => v,
        Err(e) => {
            return (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("Failed to query CTFd DB: {}", e)}))).into_response();
        }
    };

    let remote_map: HashMap<String, &Value> = remote_challenges
        .iter()
        .filter_map(|c| c["name"].as_str().map(|n| (n.to_string(), c)))
        .collect();

    let local_map: HashMap<String, &Value> = local_challenges
        .iter()
        .filter_map(|c| c["name"].as_str().map(|n| (n.to_string(), c)))
        .collect();

    let mut to_create: Vec<String> = Vec::new();
    let mut to_update: Vec<String> = Vec::new();
    let mut up_to_date: Vec<String> = Vec::new();
    let mut remote_only: Vec<String> = Vec::new();

    for (name, local) in &local_map {
        if let Some(remote) = remote_map.get(name) {
            let changed = local["category"] != remote["category"]
                || local["value"] != remote["value"]
                || local["description"] != remote["description"];
            if changed {
                to_update.push(name.clone());
            } else {
                up_to_date.push(name.clone());
            }
        } else {
            to_create.push(name.clone());
        }
    }

    for name in remote_map.keys() {
        if !local_map.contains_key(name) {
            remote_only.push(name.clone());
        }
    }

    Json(json!({
        "to_create": to_create,
        "to_update": to_update,
        "up_to_date": up_to_date,
        "remote_only": remote_only,
    }))
    .into_response()
}

// ── CTFd challenge CRUD handlers ──────────────────────────────────────────────

fn ctfd_ok(data: Value) -> Response {
    Json(json!({"success": true, "data": data})).into_response()
}

fn ctfd_list(data: Vec<Value>) -> Response {
    Json(json!({"success": true, "data": data, "meta": {"pagination": {"next": null}}})).into_response()
}

fn ctfd_err(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({"success": false, "errors": {"message": msg}}))).into_response()
}

fn ctfd_deleted() -> Response {
    Json(json!({"success": true})).into_response()
}

async fn ctfd_challenges_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return ctfd_err(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    match ctfd_db::list_challenges_full(&state.ctfd_pool).await {
        Ok(list) => ctfd_list(list),
        Err(e) => ctfd_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn ctfd_challenge_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return ctfd_err(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    match ctfd_db::create_challenge(&state.ctfd_pool, &body).await {
        Ok(v) => ctfd_ok(v),
        Err(e) => ctfd_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn ctfd_challenge_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return ctfd_err(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    match ctfd_db::get_challenge_full(&state.ctfd_pool, id).await {
        Ok(Some(v)) => ctfd_ok(v),
        Ok(None) => ctfd_err(StatusCode::NOT_FOUND, "Challenge not found"),
        Err(e) => ctfd_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn ctfd_challenge_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return ctfd_err(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    match ctfd_db::update_challenge(&state.ctfd_pool, id, &body).await {
        Ok(v) => ctfd_ok(v),
        Err(e) => ctfd_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn ctfd_challenge_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return ctfd_err(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    match ctfd_db::delete_challenge(&state.ctfd_pool, id).await {
        Ok(()) => ctfd_deleted(),
        Err(e) => ctfd_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

// ── Flags ─────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ChallengeIdQuery {
    challenge_id: Option<i64>,
}

async fn ctfd_flags_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<ChallengeIdQuery>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return ctfd_err(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    let cid = match params.challenge_id {
        Some(id) => id,
        None => return ctfd_err(StatusCode::BAD_REQUEST, "missing challenge_id"),
    };
    match ctfd_db::list_flags(&state.ctfd_pool, cid).await {
        Ok(list) => ctfd_list(list),
        Err(e) => ctfd_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn ctfd_flag_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return ctfd_err(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    match ctfd_db::create_flag_full(&state.ctfd_pool, &body).await {
        Ok(v) => ctfd_ok(v),
        Err(e) => ctfd_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn ctfd_flag_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return ctfd_err(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    match ctfd_db::delete_flag_by_id(&state.ctfd_pool, id).await {
        Ok(()) => ctfd_deleted(),
        Err(e) => ctfd_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

// ── Hints ─────────────────────────────────────────────────────────────────────

async fn ctfd_hints_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<ChallengeIdQuery>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return ctfd_err(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    let cid = match params.challenge_id {
        Some(id) => id,
        None => return ctfd_err(StatusCode::BAD_REQUEST, "missing challenge_id"),
    };
    match ctfd_db::list_hints(&state.ctfd_pool, cid).await {
        Ok(list) => ctfd_list(list),
        Err(e) => ctfd_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn ctfd_hint_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return ctfd_err(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    match ctfd_db::create_hint(&state.ctfd_pool, &body).await {
        Ok(v) => ctfd_ok(v),
        Err(e) => ctfd_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn ctfd_hint_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return ctfd_err(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    match ctfd_db::delete_hint(&state.ctfd_pool, id).await {
        Ok(()) => ctfd_deleted(),
        Err(e) => ctfd_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

// ── Tags ──────────────────────────────────────────────────────────────────────

async fn ctfd_tags_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<ChallengeIdQuery>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return ctfd_err(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    let cid = match params.challenge_id {
        Some(id) => id,
        None => return ctfd_err(StatusCode::BAD_REQUEST, "missing challenge_id"),
    };
    match ctfd_db::list_tags(&state.ctfd_pool, cid).await {
        Ok(list) => ctfd_list(list),
        Err(e) => ctfd_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn ctfd_tag_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return ctfd_err(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    match ctfd_db::create_tag(&state.ctfd_pool, &body).await {
        Ok(v) => ctfd_ok(v),
        Err(e) => ctfd_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn ctfd_tag_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return ctfd_err(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    match ctfd_db::delete_tag(&state.ctfd_pool, id).await {
        Ok(()) => ctfd_deleted(),
        Err(e) => ctfd_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

// ── Files ─────────────────────────────────────────────────────────────────────

async fn ctfd_files_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<ChallengeIdQuery>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return ctfd_err(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    let cid = match params.challenge_id {
        Some(id) => id,
        None => return ctfd_err(StatusCode::BAD_REQUEST, "missing challenge_id"),
    };
    match ctfd_db::list_files(&state.ctfd_pool, cid).await {
        Ok(list) => ctfd_list(list),
        Err(e) => ctfd_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn ctfd_files_upload(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return ctfd_err(StatusCode::UNAUTHORIZED, "Unauthorized");
    }

    let mut challenge_id: Option<i64> = None;
    let mut file_type = "challenge".to_string();
    let mut file_parts: Vec<(String, Vec<u8>)> = Vec::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name() {
            Some("challenge_id") => {
                challenge_id = field.text().await.ok().and_then(|s| s.parse().ok());
            }
            Some("type") => {
                file_type = field.text().await.unwrap_or_else(|_| "challenge".to_string());
            }
            Some("file") => {
                let fname = field.file_name().unwrap_or("upload").to_string();
                if let Ok(bytes) = field.bytes().await {
                    file_parts.push((fname, bytes.to_vec()));
                }
            }
            _ => {}
        }
    }

    let cid = match challenge_id {
        Some(id) => id,
        None => return ctfd_err(StatusCode::BAD_REQUEST, "missing challenge_id"),
    };

    if file_parts.is_empty() {
        return ctfd_err(StatusCode::BAD_REQUEST, "no files provided");
    }

    let mut results: Vec<Value> = Vec::new();

    for (filename, bytes) in file_parts {
        let uuid: String = {
            use rand::Rng;
            let mut rng = rand::thread_rng();
            (0..16).map(|_| format!("{:02x}", rng.gen::<u8>())).collect()
        };
        let location = format!("{}/{}", uuid, filename);

        if !state.ctfd_uploads_dir.is_empty() {
            let dir = format!("{}/{}", state.ctfd_uploads_dir.trim_end_matches('/'), uuid);
            if let Err(e) = std::fs::create_dir_all(&dir) {
                warn!("ctfd_files_upload: create dir {}: {}", dir, e);
            } else {
                let fpath = format!("{}/{}", dir, filename);
                if let Err(e) = std::fs::write(&fpath, &bytes) {
                    warn!("ctfd_files_upload: write {}: {}", fpath, e);
                }
            }
        }

        match ctfd_db::create_file_record(&state.ctfd_pool, cid, &file_type, &location).await {
            Ok(id) => results.push(json!({"id": id, "location": location, "type": file_type})),
            Err(e) => return ctfd_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        }
    }

    Json(json!({"success": true, "data": results})).into_response()
}

async fn ctfd_file_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return ctfd_err(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    match ctfd_db::delete_file_record(&state.ctfd_pool, id).await {
        Ok(Some(location)) => {
            if !state.ctfd_uploads_dir.is_empty() {
                let fpath = format!("{}/{}", state.ctfd_uploads_dir.trim_end_matches('/'), location);
                let _ = std::fs::remove_file(&fpath);
                if let Some(parent) = std::path::Path::new(&fpath).parent() {
                    let _ = std::fs::remove_dir(parent);
                }
            }
            ctfd_deleted()
        }
        Ok(None) => ctfd_deleted(),
        Err(e) => ctfd_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

// ── Topics ────────────────────────────────────────────────────────────────────

async fn ctfd_topics_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<ChallengeIdQuery>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return ctfd_err(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    let cid = match params.challenge_id {
        Some(id) => id,
        None => return ctfd_err(StatusCode::BAD_REQUEST, "missing challenge_id"),
    };
    match ctfd_db::list_topics(&state.ctfd_pool, cid).await {
        Ok(list) => ctfd_list(list),
        Err(e) => ctfd_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn ctfd_topic_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return ctfd_err(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    match ctfd_db::create_topic(&state.ctfd_pool, &body).await {
        Ok(v) => ctfd_ok(v),
        Err(e) => ctfd_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn ctfd_topic_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    if !check_any_auth(&headers, &state.db).await {
        return ctfd_err(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    match ctfd_db::delete_topic(&state.ctfd_pool, id).await {
        Ok(()) => ctfd_deleted(),
        Err(e) => ctfd_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    fn bearer_headers(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("authorization", HeaderValue::from_str(value).unwrap());
        h
    }

    fn cookie_headers(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("cookie", HeaderValue::from_str(value).unwrap());
        h
    }

    #[test]
    fn hash_token_deterministic() {
        let h1 = hash_token("secret123");
        let h2 = hash_token("secret123");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 → 64 hex chars
    }

    #[test]
    fn hash_token_different_inputs_differ() {
        assert_ne!(hash_token("secret123"), hash_token("wrongtoken"));
    }

    #[test]
    fn hash_token_empty_input() {
        assert_ne!(hash_token(""), hash_token("notempty"));
    }

    #[test]
    fn extract_bearer_valid() {
        let headers = bearer_headers("Token secret123");
        assert_eq!(extract_bearer(&headers), Some("secret123"));
    }

    #[test]
    fn extract_bearer_wrong_scheme() {
        let headers = bearer_headers("Bearer secret123");
        assert_eq!(extract_bearer(&headers), None);
    }

    #[test]
    fn extract_bearer_missing() {
        assert_eq!(extract_bearer(&HeaderMap::new()), None);
    }

    #[test]
    fn extract_session_cookie_present() {
        let headers = cookie_headers("nervctf_session=abc123; other=val");
        assert_eq!(extract_session_cookie(&headers), Some("abc123".to_string()));
    }

    #[test]
    fn extract_session_cookie_only_session() {
        let headers = cookie_headers("nervctf_session=xyz");
        assert_eq!(extract_session_cookie(&headers), Some("xyz".to_string()));
    }

    #[test]
    fn extract_session_cookie_missing() {
        assert_eq!(extract_session_cookie(&HeaderMap::new()), None);
    }

    #[test]
    fn extract_session_cookie_wrong_name() {
        let headers = cookie_headers("other=val");
        assert_eq!(extract_session_cookie(&headers), None);
    }
}

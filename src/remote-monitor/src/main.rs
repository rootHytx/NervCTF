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
//!   POST       /api/v1/topics            — create topic (monitor token)
//!   POST /api/v1/instance/build          — build Docker image (monitor token)
//!   POST /api/v1/instance/build-compose  — upload+extract compose dir + pre-build images (monitor token)
//!   POST /api/v1/instance/register       — register instance config (monitor token)
//!   GET  /api/v1/instance/list           — list configs (monitor token)
//!   GET  /api/v1/admin/instances         — list all active instances (monitor token)
//!   GET  /api/v1/admin/attempts          — list flag attempts; ?alerts_only=true for sharing alerts (monitor token)
//!   GET  /api/v1/admin/solves            — list correct solves (one per team+challenge) (monitor token)
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
use tracing::{error, info, warn};
use tracing_subscriber::{EnvFilter, fmt};
use axum::{
    extract::{Multipart, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use bytes::Bytes;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::env;
use std::sync::Arc;

use db::Db;

#[derive(Clone)]
struct AppState {
    monitor_token: String,
    public_host: String,
    db: Db,
    ctfd_pool: mysql_async::Pool,
    /// Directory where compose challenge sources are stored.
    /// In split-machine mode this path lives on the **runner** host.
    challenges_base_dir: String,
    /// Path to CTFd uploads directory for file writes (empty string if not set).
    ctfd_uploads_dir: String,
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

    let monitor_token = env::var("MONITOR_TOKEN").expect("MONITOR_TOKEN is required");
    let public_host = env::var("PUBLIC_HOST").expect("PUBLIC_HOST is required");
    let port = env::var("MONITOR_PORT").unwrap_or_else(|_| "33133".to_string());
    let bind = env::var("MONITOR_BIND").unwrap_or_else(|_| "0.0.0.0".to_string());
    let db_path = env::var("DB_PATH").unwrap_or_else(|_| "./monitor.db".to_string());
    let challenges_base_dir = env::var("CHALLENGES_BASE_DIR")
        .unwrap_or_else(|_| "/opt/nervctf/challenges".to_string());
    let ctfd_uploads_dir = env::var("CTFD_UPLOADS_DIR").unwrap_or_default();

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
    let ctfd_pool = ctfd_db::create_pool(&ctfd_db_url)?;

    let max_concurrent_provisions: usize = env::var("MAX_CONCURRENT_PROVISIONS")
        .ok().and_then(|s| s.parse().ok()).unwrap_or(4);
    info!("provision concurrency limit: {}", max_concurrent_provisions);

    let max_instances_per_team: u64 = env::var("MAX_INSTANCES_PER_TEAM")
        .ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    if max_instances_per_team > 0 {
        info!("per-team instance cap: {}", max_instances_per_team);
    }

    let state = Arc::new(AppState {
        monitor_token,
        public_host,
        db: db.clone(),
        ctfd_pool,
        challenges_base_dir,
        ctfd_uploads_dir,
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
                    let _ = instance::compose::down(&project).await;
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
        .route("/admin", get(admin_dashboard_handler))
        .route("/instance/{name}", get(instance_page_handler))
        .route("/api/v1/diff", get(diff_handler).post(diff_handler).patch(diff_handler).delete(diff_handler))
        // CTFd challenge CRUD (monitor token — used by nervctf CLI)
        .route("/api/v1/challenges", get(ctfd_challenges_list).post(ctfd_challenge_create))
        .route("/api/v1/challenges/{id}", get(ctfd_challenge_get).patch(ctfd_challenge_update).delete(ctfd_challenge_delete))
        // Flags
        .route("/api/v1/flags", get(ctfd_flags_list).post(ctfd_flag_create))
        .route("/api/v1/flags/{id}", delete(ctfd_flag_delete))
        // Hints
        .route("/api/v1/hints", get(ctfd_hints_list).post(ctfd_hint_create))
        .route("/api/v1/hints/{id}", delete(ctfd_hint_delete))
        // Tags
        .route("/api/v1/tags", get(ctfd_tags_list).post(ctfd_tag_create))
        .route("/api/v1/tags/{id}", delete(ctfd_tag_delete))
        // Files
        .route("/api/v1/files", get(ctfd_files_list).post(ctfd_files_upload))
        .route("/api/v1/files/{id}", delete(ctfd_file_delete))
        // Topics
        .route("/api/v1/topics", post(ctfd_topic_create))
        // Admin routes (monitor token)
        .route("/api/v1/instance/build", post(instance_build_handler))
        .route("/api/v1/instance/build-compose", post(build_compose_handler))
        .route("/api/v1/instance/build-compose-remote", post(build_compose_remote_handler))
        .route("/api/v1/instance/register", post(instance_register_handler))
        .route("/api/v1/instance/list", get(instance_list_handler))
        .route("/api/v1/admin/instances", get(admin_instances_handler))
        .route("/api/v1/admin/attempts", get(admin_attempts_handler))
        .route("/api/v1/admin/solves", get(admin_solves_handler))
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
        .with_state(Arc::clone(&state));

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

// ── Auth helpers ──────────────────────────────────────────────────────────────

fn check_monitor_auth(headers: &HeaderMap, expected_token: &str) -> bool {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Token "))
        .map(|t| t == expected_token)
        .unwrap_or(false)
}

/// Validate a CTFd user token and return team_id via direct MariaDB lookup.
async fn validate_ctfd_token(
    pool: &mysql_async::Pool,
    token: &str,
) -> Option<i64> {
    ctfd_db::validate_token(pool, token).await
}

fn extract_bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Token "))
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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

        // Wipe + create dir, extract tar, and build — all on the runner in one SSH session
        let remote_cmd = format!(
            "rm -rf '{dir}' && mkdir -p '{dir}' && tar -xzf - -C '{dir}' && DOCKER_BUILDKIT=1 docker compose -f '{compose}' build",
            dir = extract_dir,
            compose = compose_path,
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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

    // Check for existing running instance
    if let Ok(Some(inst)) = db::get_instance(&state.db, &body.challenge_name, team_id) {
        if inst.status == "running" {
            return Json(json!({
                "host": inst.host,
                "port": inst.port,
                "connection_type": inst.connection_type,
                "expires_at": inst.expires_at,
            })).into_response();
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

    match instance::provision(
        &state.db, &body.challenge_name, team_id, None, &config, &state.public_host,
        &state.ctfd_pool, state.runner_ssh_target.as_deref(),
    ).await {
        Ok((host, port, conn, expires_at)) => Json(json!({
            "host": host,
            "port": port,
            "connection_type": conn,
            "expires_at": expires_at,
        })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
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
        Ok(Some(inst)) => Json(json!({
            "status": inst.status,
            "host": inst.host,
            "port": inst.port,
            "connection_type": inst.connection_type,
            "expires_at": inst.expires_at,
            "renewals_used": inst.renewals_used,
        })).into_response(),
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

    Json(json!({
        "host": inst.host,
        "port": inst.port,
        "connection_type": inst.connection_type,
        "expires_at": new_expires,
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
        Ok(Some(inst)) => Json(json!({
            "status": inst.status,
            "host": inst.host,
            "port": inst.port,
            "connection_type": inst.connection_type,
            "expires_at": inst.expires_at,
            "renewals_used": inst.renewals_used,
            "container_id": inst.container_id,
            "flag": inst.flag,
        })).into_response(),
        Ok(None) => Json(json!({"status": "none"})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

async fn plugin_request_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PluginTeamBody>,
) -> impl IntoResponse {
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
            return Json(json!({
                "status": inst.status,
                "host": inst.host,
                "port": inst.port,
                "connection_type": inst.connection_type,
                "expires_at": inst.expires_at,
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

    // Insert stub immediately so the client can poll for status
    if let Err(e) = db::insert_provisioning_stub(
        &state.db, &body.challenge_name, body.team_id, body.user_id,
        &state.public_host, &connection_type, &expires_at,
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }

    let inst = match db::get_instance(&state.db, &body.challenge_name, body.team_id) {
        Ok(Some(i)) => i,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"error": "No active instance"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    };

    let timeout_minutes = db::get_config(&state.db, &body.challenge_name)
        .ok().flatten()
        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
        .and_then(|v| v["timeout_minutes"].as_u64())
        .unwrap_or(45);

    let max_renewals = db::get_config(&state.db, &body.challenge_name)
        .ok().flatten()
        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
        .and_then(|v| v["max_renewals"].as_u64())
        .unwrap_or(3);

    if inst.renewals_used >= max_renewals as i64 {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "Maximum renewals reached"}))).into_response();
    }

    let new_expires = instance::expires_at_string(timeout_minutes);
    if let Err(e) = db::update_expires_at(&state.db, &body.challenge_name, body.team_id, &new_expires) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
    }

    Json(json!({
        "host": inst.host,
        "port": inst.port,
        "connection_type": inst.connection_type,
        "expires_at": new_expires,
    })).into_response()
}

async fn plugin_stop_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PluginTeamBody>,
) -> impl IntoResponse {
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    match db::delete_all_instances_for_challenge(&state.db, &body.challenge_name) {
        Ok(pairs) => {
            for (container_id, ctfd_flag_id) in pairs {
                if let Some(cid) = container_id {
                    instance::cleanup_container(&cid, state.runner_ssh_target.as_deref()).await;
                }
                if let Some(flag_id) = ctfd_flag_id {
                    ctfd_db::delete_flag(&state.ctfd_pool, flag_id).await;
                }
            }
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> impl IntoResponse {
    // Accept token via ?token= query param or Authorization: Token <x> header
    let token_from_query = params.get("token").map(|s| s.as_str()).unwrap_or("");
    let authed = token_from_query == state.monitor_token
        || check_monitor_auth(&headers, &state.monitor_token);

    if !authed {
        return (StatusCode::FORBIDDEN, "Forbidden").into_response();
    }

    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_str!("../assets/admin.html"),
    ).into_response()
}

async fn admin_instances_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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
    if !check_monitor_auth(&headers, &state.monitor_token) {
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

async fn ctfd_topic_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    if !check_monitor_auth(&headers, &state.monitor_token) {
        return ctfd_err(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    match ctfd_db::create_topic(&state.ctfd_pool, &body).await {
        Ok(v) => ctfd_ok(v),
        Err(e) => ctfd_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

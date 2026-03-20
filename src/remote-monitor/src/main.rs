//! Remote Monitor — CTFd API proxy + instance lifecycle manager
//!
//! Routes:
//!   GET  /health                         — liveness check (no auth)
//!   GET  /admin                          — admin dashboard (monitor token via ?token= or header)
//!   GET  /instance/:name                 — HTML player page
//!   ANY  /api/v1/diff                    — local/remote diff (monitor token)
//!   POST /api/v1/instance/build          — build Docker image (monitor token)
//!   POST /api/v1/instance/build-compose  — upload+extract compose dir + pre-build images (monitor token)
//!   POST /api/v1/instance/register       — register instance config (monitor token)
//!   GET  /api/v1/instance/list           — list configs (monitor token)
//!   GET  /api/v1/admin/instances         — list all active instances (monitor token)
//!   GET  /api/v1/admin/attempts          — list flag attempts; ?alerts_only=true for sharing alerts (monitor token)
//!   POST /api/v1/plugin/attempt          — record flag submission attempt (monitor token)
//!   POST /api/v1/instance/request        — provision instance (CTFd user token)
//!   GET  /api/v1/instance/info           — get own instance (CTFd user token)
//!   POST /api/v1/instance/renew          — extend timeout (CTFd user token)
//!   DELETE /api/v1/instance/stop         — destroy instance (CTFd user token)
//!   ANY  /api/v1/*path                   — transparent proxy to CTFd (monitor token)
//!
//! Environment variables:
//!   CTFD_URL       — CTFd URL reachable from within the Docker network
//!                    (e.g. http://ctfd:8000 when running as a compose service)
//!   CTFD_API_KEY   — CTFd admin token (required)
//!   MONITOR_TOKEN  — admin token for nervctf CLI (required)
//!   PUBLIC_HOST    — hostname/IP returned in instance connection info (required)
//!   MONITOR_PORT   — bind port (default: 33133)
//!   MONITOR_BIND   — bind address (default: 0.0.0.0)
//!   DB_PATH        — SQLite database path (default: ./monitor.db)

mod db;
mod instance;

use anyhow::Result;
use tracing::{error, info, warn};
use tracing_subscriber::{EnvFilter, fmt};
use axum::{
    body::Body,
    extract::{Multipart, Path, Request, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{any, delete, get, post},
    Json, Router,
};
use bytes::Bytes;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::env;
use std::sync::Arc;

use db::Db;

#[derive(Clone)]
struct AppState {
    ctfd_url: String,
    ctfd_api_key: String,
    monitor_token: String,
    public_host: String,
    http_client: Client,
    db: Db,
    /// Host-visible directory where compose challenge sources are stored.
    /// Must be bind-mounted at the same path inside the container so that the
    /// host Docker daemon (accessed via /var/run/docker.sock) can reach the
    /// build context during `docker compose build`.
    challenges_base_dir: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("remote_monitor=debug,info")),
        )
        .init();

    let ctfd_url = env::var("CTFD_URL").expect("CTFD_URL is required");
    let ctfd_api_key = env::var("CTFD_API_KEY").expect("CTFD_API_KEY is required");
    let monitor_token = env::var("MONITOR_TOKEN").expect("MONITOR_TOKEN is required");
    let public_host = env::var("PUBLIC_HOST").expect("PUBLIC_HOST is required");
    let port = env::var("MONITOR_PORT").unwrap_or_else(|_| "33133".to_string());
    let bind = env::var("MONITOR_BIND").unwrap_or_else(|_| "0.0.0.0".to_string());
    let db_path = env::var("DB_PATH").unwrap_or_else(|_| "./monitor.db".to_string());
    let challenges_base_dir = env::var("CHALLENGES_BASE_DIR")
        .unwrap_or_else(|_| "/opt/nervctf/challenges".to_string());

    let db = db::open(&db_path)?;
    let http_client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let state = Arc::new(AppState {
        ctfd_url: ctfd_url.trim_end_matches('/').to_string(),
        ctfd_api_key,
        monitor_token,
        public_host,
        http_client,
        db: db.clone(),
        challenges_base_dir,
    });

    // Spawn background expiry task
    let expiry_state = Arc::clone(&state);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
            match db::get_expired_instances(&expiry_state.db) {
                Ok(expired) => {
                    for (challenge_name, container_id, team_id, ctfd_flag_id) in expired {
                        info!("expiry: cleaning up {}/{}", challenge_name, team_id);
                        let _ = db::delete_instance(&expiry_state.db, &challenge_name, team_id);
                        if let Some(flag_id) = ctfd_flag_id {
                            instance::delete_flag_from_ctfd(
                                &expiry_state.http_client,
                                &expiry_state.ctfd_url,
                                &expiry_state.ctfd_api_key,
                                flag_id,
                            ).await;
                        }
                        if let Some(cid) = container_id {
                            instance::cleanup_container(&cid).await;
                        }
                    }
                }
                Err(e) => error!("expiry: db error: {}", e),
            }
        }
    });

    let addr = format!("{}:{}", bind, port);
    info!("Starting remote-monitor on {}", addr);
    info!("CTFD_URL={}", state.ctfd_url);
    info!("PUBLIC_HOST={}", state.public_host);

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/admin", get(admin_dashboard_handler))
        .route("/instance/{name}", get(instance_page_handler))
        .route("/api/v1/diff", any(diff_handler))
        // Admin routes (monitor token)
        .route("/api/v1/instance/build", post(instance_build_handler))
        .route("/api/v1/instance/build-compose", post(build_compose_handler))
        .route("/api/v1/instance/register", post(instance_register_handler))
        .route("/api/v1/instance/list", get(instance_list_handler))
        .route("/api/v1/admin/instances", get(admin_instances_handler))
        .route("/api/v1/admin/attempts", get(admin_attempts_handler))
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
        // Fallthrough proxy — must be last
        .route("/api/v1/*path", any(proxy_handler))
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

/// Validate a CTFd user token and return team_id by calling GET /api/v1/users/me
async fn validate_ctfd_token(
    http_client: &Client,
    ctfd_url: &str,
    token: &str,
) -> Option<i64> {
    let resp = http_client
        .get(format!("{}/api/v1/users/me", ctfd_url))
        .header("Authorization", format!("Token {}", token))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let val: Value = resp.json().await.ok()?;
    val["data"]["team_id"].as_i64()
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

    if let Err(e) = instance::docker::build_image(tmp.path(), &image_tag).await {
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

    // Write tar to temp file
    let tmp = match tempfile::NamedTempFile::new() {
        Ok(f) => f,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    };
    if let Err(e) = std::fs::write(tmp.path(), &tar_bytes) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
    }

    // Wipe any existing challenge directory so stale placeholder directories
    // (created by Docker when bind-mount sources were missing on a previous run)
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

    // Extract tar.gz into the challenge directory
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

    // Determine compose file path from DB config (if challenge is already registered)
    let compose_file_str = db::get_config(&state.db, &challenge_name)
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
        .and_then(|v| v["compose_file"].as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "docker-compose.yml".to_string());

    let compose_path = if compose_file_str.starts_with('/') {
        compose_file_str
    } else {
        format!("{}/{}", extract_dir, compose_file_str)
    };

    info!("build-compose: building images with compose file {}", compose_path);

    // Pre-build all images (no --build at runtime)
    let build_out = instance::compose::compose_cmd().await
        .args(["-f", compose_path.as_str(), "build"])
        .output()
        .await;

    match build_out {
        Ok(out) if out.status.success() => {
            info!("build-compose: images built successfully for {}", challenge_name);
        }
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr).to_string();
            error!("build-compose: docker compose build failed for {}: {}", challenge_name, err);
            return (StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("docker compose build failed: {}", err)}))).into_response();
        }
        Err(e) => {
            error!("build-compose: compose spawn failed: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
        }
    }

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

    let team_id = match validate_ctfd_token(&state.http_client, &state.ctfd_url, &token).await {
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

    match instance::provision(
        &state.db, &body.challenge_name, team_id, None, &config, &state.public_host,
        &state.http_client, &state.ctfd_url, &state.ctfd_api_key,
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

    let team_id = match validate_ctfd_token(&state.http_client, &state.ctfd_url, &token).await {
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

    let team_id = match validate_ctfd_token(&state.http_client, &state.ctfd_url, &token).await {
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

    let team_id = match validate_ctfd_token(&state.http_client, &state.ctfd_url, &token).await {
        Some(id) => id,
        None => return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Invalid CTFd token"}))).into_response(),
    };

    match db::delete_instance(&state.db, &body.challenge_name, team_id) {
        Ok(Some((container_id, ctfd_flag_id))) => {
            if let Some(cid) = container_id {
                instance::cleanup_container(&cid).await;
            }
            if let Some(flag_id) = ctfd_flag_id {
                instance::delete_flag_from_ctfd(
                    &state.http_client, &state.ctfd_url, &state.ctfd_api_key, flag_id,
                ).await;
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

    // Return existing running instance
    if let Ok(Some(inst)) = db::get_instance(&state.db, &body.challenge_name, body.team_id) {
        if inst.status == "running" {
            info!("plugin_request: returning existing instance for {}/{}", body.challenge_name, body.team_id);
            return Json(json!({
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

    info!("plugin_request: provisioning '{}' for team {}", body.challenge_name, body.team_id);
    match instance::provision(
        &state.db, &body.challenge_name, body.team_id, body.user_id, &config, &state.public_host,
        &state.http_client, &state.ctfd_url, &state.ctfd_api_key,
    ).await {
        Ok((host, port, conn, expires_at)) => {
            info!("plugin_request: provisioned {}:{} ({})", host, port, conn);
            Json(json!({
                "host": host,
                "port": port,
                "connection_type": conn,
                "expires_at": expires_at,
            })).into_response()
        }
        Err(e) => {
            error!("plugin_request: provision error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response()
        }
    }
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
            // Delete CTFd flag synchronously first — fast HTTP call, must happen before response.
            if let Some(flag_id) = ctfd_flag_id {
                instance::delete_flag_from_ctfd(
                    &state.http_client, &state.ctfd_url, &state.ctfd_api_key, flag_id,
                ).await;
            }
            // Container teardown in background — compose down is slow.
            if let Some(cid) = container_id {
                tokio::spawn(async move { instance::cleanup_container(&cid).await; });
            }
            Json(json!({"ok": true})).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({"error": "No active instance"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

/// Called by the CTFd plugin when a team solves an instance challenge.
/// Deletes the DB record immediately and returns 200, then tears down the
/// container and CTFd flag in the background so the plugin doesn't time out
/// waiting for `docker compose down`.
async fn plugin_solve_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PluginTeamBody>,
) -> impl IntoResponse {
    if !check_monitor_auth(&headers, &state.monitor_token) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    match db::delete_instance(&state.db, &body.challenge_name, body.team_id) {
        Ok(Some((container_id, ctfd_flag_id))) => {
            // Delete CTFd flag synchronously first — fast HTTP call, must happen before response.
            if let Some(flag_id) = ctfd_flag_id {
                instance::delete_flag_from_ctfd(
                    &state.http_client, &state.ctfd_url, &state.ctfd_api_key, flag_id,
                ).await;
            }
            // Container teardown in background — compose down is slow.
            if let Some(cid) = container_id {
                tokio::spawn(async move { instance::cleanup_container(&cid).await; });
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
                    instance::cleanup_container(&cid).await;
                }
                if let Some(flag_id) = ctfd_flag_id {
                    instance::delete_flag_from_ctfd(
                        &state.http_client, &state.ctfd_url, &state.ctfd_api_key, flag_id,
                    ).await;
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
    match db::list_all_instances(&state.db) {
        Ok(list) => Json(json!(list)).into_response(),
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
    let result = if alerts_only {
        db::list_sharing_alerts(&state.db)
    } else {
        db::list_flag_attempts(&state.db, 200)
    };
    match result {
        Ok(list) => Json(json!(list)).into_response(),
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

    let resp = state
        .http_client
        .get(format!("{}/api/v1/challenges", state.ctfd_url))
        .header("Authorization", format!("Token {}", state.ctfd_api_key))
        .send()
        .await;

    let remote_challenges = match resp {
        Ok(r) => {
            let data: Value = r.json().await.unwrap_or_default();
            data["data"].as_array().cloned().unwrap_or_default()
        }
        Err(e) => {
            return (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("Failed to reach CTFd: {}", e)}))).into_response();
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

// ── Proxy handler ─────────────────────────────────────────────────────────────

async fn proxy_handler(
    State(state): State<Arc<AppState>>,
    Path(path): Path<String>,
    request: Request,
) -> impl IntoResponse {
    if !check_monitor_auth(request.headers(), &state.monitor_token) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }

    let method_str = request.method().as_str().to_string();
    let req_method =
        reqwest::Method::from_bytes(method_str.as_bytes()).unwrap_or(reqwest::Method::GET);
    info!("proxy: {} /api/v1/{}{}", method_str, path, request.uri().query().map(|q| format!("?{}", q)).unwrap_or_default());

    let query = request
        .uri()
        .query()
        .map(|q| format!("?{}", q))
        .unwrap_or_default();

    let ctfd_url = format!("{}/api/v1/{}{}", state.ctfd_url, path, query);
    let req_headers = request.headers().clone();

    let body_bytes = match axum::body::to_bytes(request.into_body(), 100 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::BAD_REQUEST, "Failed to read request body").into_response(),
    };

    let mut req_builder = state
        .http_client
        .request(req_method, &ctfd_url)
        .header("Authorization", format!("Token {}", state.ctfd_api_key));

    for (name, value) in &req_headers {
        let name_lower = name.as_str().to_lowercase();
        if name_lower == "authorization" || name_lower == "host" || name_lower == "content-length" {
            continue;
        }
        if let Ok(val) = value.to_str() {
            req_builder = req_builder.header(name.as_str(), val);
        }
    }

    if !body_bytes.is_empty() {
        req_builder = req_builder.body(body_bytes.to_vec());
    }

    match req_builder.send().await {
        Ok(resp) => {
            let status_u16 = resp.status().as_u16();
            let status = StatusCode::from_u16(status_u16).unwrap_or(StatusCode::BAD_GATEWAY);

            let resp_header_pairs: Vec<(String, Vec<u8>)> = resp
                .headers()
                .iter()
                .map(|(n, v)| (n.as_str().to_string(), v.as_bytes().to_vec()))
                .collect();

            let resp_body = match resp.bytes().await {
                Ok(b) => b,
                Err(_) => return (StatusCode::BAD_GATEWAY, "Failed to read CTFd response").into_response(),
            };

            if status_u16 >= 400 {
                warn!("proxy: upstream {} for {} — body: {}", status_u16, ctfd_url,
                    std::str::from_utf8(&resp_body).unwrap_or("<binary>").chars().take(200).collect::<String>());
            } else {
                info!("proxy: upstream {} for {}", status_u16, ctfd_url);
            }

            let mut builder = Response::builder().status(status);
            for (name_str, value_bytes) in &resp_header_pairs {
                let name_lower = name_str.to_lowercase();
                if name_lower == "transfer-encoding" || name_lower == "connection" {
                    continue;
                }
                if let (Ok(n), Ok(v)) = (
                    axum::http::HeaderName::from_bytes(name_str.as_bytes()),
                    axum::http::HeaderValue::from_bytes(value_bytes),
                ) {
                    builder = builder.header(n, v);
                }
            }

            builder.body(Body::from(resp_body)).unwrap_or_else(|_| {
                (StatusCode::INTERNAL_SERVER_ERROR, "Response build error").into_response()
            })
        }
        Err(e) => (StatusCode::BAD_GATEWAY, format!("Upstream error: {}", e)).into_response(),
    }
}

//! Remote Monitor — transparent CTFd API proxy
//!
//! Keeps the CTFd admin key on the server while letting clients
//! sync challenges via a separate monitor token.
//!
//! Routes:
//!   GET  /health              — liveness check (no auth)
//!   ANY  /api/v1/diff         — local/remote diff (auth required)
//!   ANY  /api/v1/*path        — transparent proxy to CTFd (auth required)
//!
//! Environment variables:
//!   CTFD_URL       — CTFd instance URL (required)
//!   CTFD_API_KEY   — CTFd admin token (required)
//!   MONITOR_TOKEN  — token clients must present (required)
//!   MONITOR_PORT   — bind port (default: 33133)
//!   MONITOR_BIND   — bind address (default: 0.0.0.0)

use anyhow::Result;
use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get},
    Json, Router,
};
use bytes::Bytes;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::env;

#[derive(Clone)]
struct AppState {
    ctfd_url: String,
    ctfd_api_key: String,
    monitor_token: String,
    http_client: Client,
}

#[tokio::main]
async fn main() -> Result<()> {
    let ctfd_url = env::var("CTFD_URL").expect("CTFD_URL environment variable is required");
    let ctfd_api_key =
        env::var("CTFD_API_KEY").expect("CTFD_API_KEY environment variable is required");
    let monitor_token =
        env::var("MONITOR_TOKEN").expect("MONITOR_TOKEN environment variable is required");
    let port = env::var("MONITOR_PORT").unwrap_or_else(|_| "33133".to_string());
    let bind = env::var("MONITOR_BIND").unwrap_or_else(|_| "0.0.0.0".to_string());

    let http_client = Client::new();

    let state = AppState {
        ctfd_url: ctfd_url.trim_end_matches('/').to_string(),
        ctfd_api_key,
        monitor_token,
        http_client,
    };

    let addr = format!("{}:{}", bind, port);
    println!("Starting remote-monitor on {}", addr);

    let app = Router::new()
        .route("/health", get(health_handler))
        // Specific route for diff must come before the wildcard
        .route("/api/v1/diff", any(diff_handler))
        .route("/api/v1/*path", any(proxy_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Liveness check — no authentication required
async fn health_handler() -> impl IntoResponse {
    Json(json!({"status": "ok"}))
}

/// Check that the Authorization: Token <value> header matches the monitor token
fn check_auth(headers: &HeaderMap, expected_token: &str) -> bool {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Token "))
        .map(|t| t == expected_token)
        .unwrap_or(false)
}

/// POST /api/v1/diff
///
/// Body: `{"challenges": [<Challenge objects>]}`
///
/// Queries CTFd for existing challenges, computes to_create / to_update /
/// up_to_date / remote_only, and returns the diff as JSON.
async fn diff_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.monitor_token) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Unauthorized"})),
        )
            .into_response();
    }

    // Parse body
    let local_challenges: Vec<Value> = match serde_json::from_slice::<Value>(&body) {
        Ok(v) => v
            .get("challenges")
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default(),
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("Invalid JSON: {}", e)})),
            )
                .into_response();
        }
    };

    // Query CTFd
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
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("Failed to reach CTFd: {}", e)})),
            )
                .into_response();
        }
    };

    // Build name-keyed maps
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

/// ANY /api/v1/*path — transparent proxy to CTFd
///
/// 1. Verifies monitor token
/// 2. Strips client Authorization header, adds CTFd key
/// 3. Forwards method + headers + body verbatim to CTFd
/// 4. Streams CTFd response back to client
///
/// Note: axum 0.7 uses http 1.x while reqwest 0.11 uses http 0.2.x.
/// We bridge between them by converting via string/byte representations.
async fn proxy_handler(
    State(state): State<AppState>,
    Path(path): Path<String>,
    request: Request,
) -> impl IntoResponse {
    // Auth check
    if !check_auth(request.headers(), &state.monitor_token) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }

    // Convert axum's http 1.x Method → reqwest's http 0.2.x Method via str
    let method_str = request.method().as_str().to_string();
    let req_method =
        reqwest::Method::from_bytes(method_str.as_bytes()).unwrap_or(reqwest::Method::GET);

    // Preserve query string from original URI
    let query = request
        .uri()
        .query()
        .map(|q| format!("?{}", q))
        .unwrap_or_default();

    let ctfd_url = format!("{}/api/v1/{}{}", state.ctfd_url, path, query);

    let req_headers = request.headers().clone();

    // Collect body bytes
    let body_bytes = match axum::body::to_bytes(request.into_body(), 100 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::BAD_REQUEST, "Failed to read request body").into_response(),
    };

    // Build forwarded request — strip auth and host, add CTFd key
    let mut req_builder = state
        .http_client
        .request(req_method, &ctfd_url)
        .header("Authorization", format!("Token {}", state.ctfd_api_key));

    // Forward headers using string conversion to bridge http versions
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

    // Send to CTFd and stream response back
    match req_builder.send().await {
        Ok(resp) => {
            let status_u16 = resp.status().as_u16();
            let status = StatusCode::from_u16(status_u16).unwrap_or(StatusCode::BAD_GATEWAY);

            // Convert reqwest headers (http 0.2.x) → axum response headers (http 1.x)
            // by going through string/bytes representation
            let resp_header_pairs: Vec<(String, Vec<u8>)> = resp
                .headers()
                .iter()
                .map(|(n, v)| (n.as_str().to_string(), v.as_bytes().to_vec()))
                .collect();

            let resp_body = match resp.bytes().await {
                Ok(b) => b,
                Err(_) => {
                    return (StatusCode::BAD_GATEWAY, "Failed to read CTFd response")
                        .into_response()
                }
            };

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

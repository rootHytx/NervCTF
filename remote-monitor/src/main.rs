use hyper::server::conn::AddrStream;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server, StatusCode};
use serde::{Deserialize, Serialize};
use std::env;
use std::net::SocketAddr;
use std::process::exit;

#[derive(Debug, Serialize, Deserialize)]
struct Challenge {
    name: String,
    category: String,
    description: String,
    value: u32,
    flags: Vec<String>,
    files: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Config {
    challenges: Vec<Challenge>,
    ctfd_url: String,
    ctfd_api_key: String,
}

#[tokio::main]
async fn main() {
    // Load environment variables
    dotenv::dotenv().ok();

    // Get listen address from environment or use default
    let listen_addr = env::var("LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8000".to_string())
        .parse::<SocketAddr>()
        .unwrap_or_else(|_| {
            eprintln!("Invalid LISTEN_ADDR format");
            exit(1);
        });

    println!("Starting server on {}", listen_addr);

    // Create service
    let make_svc = make_service_fn(|_socket: &AddrStream| async {
        Ok::<_, hyper::Error>(service_fn(handle_request))
    });

    // Start server
    let server = Server::bind(&listen_addr).serve(make_svc);

    if let Err(e) = server.await {
        eprintln!("Server error: {}", e);
        exit(1);
    }
}

async fn handle_request(req: Request<Body>) -> Result<Response<Body>, hyper::Error> {
    match (req.method(), req.uri().path()) {
        (&Method::POST, "/configure") => handle_configure(req).await,
        _ => {
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::NOT_FOUND;
            Ok(response)
        }
    }
}

async fn handle_configure(req: Request<Body>) -> Result<Response<Body>, hyper::Error> {
    // Read entire body
    let body_bytes = hyper::body::to_bytes(req.into_body()).await?;

    // Parse JSON
    let config: Config = match serde_json::from_slice(&body_bytes) {
        Ok(config) => config,
        Err(e) => {
            let response = Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::from(format!("Invalid JSON: {}", e)))
                .unwrap();
            return Ok(response);
        }
    };

    println!(
        "Received configuration with {} challenges",
        config.challenges.len()
    );
    println!("CTFd URL: {}", config.ctfd_url);
    println!(
        "Using CTFd API key: {}",
        if config.ctfd_api_key.is_empty() {
            "NOT SET"
        } else {
            "SET"
        }
    );

    // TODO: Implement actual CTFd synchronization here
    // This would call ctfcli to create/update challenges

    let response = Response::builder()
        .status(StatusCode::OK)
        .body(Body::from("Configuration received successfully"))
        .unwrap();

    Ok(response)
}

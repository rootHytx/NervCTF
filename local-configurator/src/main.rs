use bollard::container::{
    Config, CreateContainerOptions, InspectContainerOptions, StartContainerOptions,
};
use bollard::image::BuildImageOptions;
use bollard::Docker;
use clap::{Parser, Subcommand};
use futures_util::stream::TryStreamExt;
use hyper::{Body, Client, Method, Request};
use serde::{Deserialize, Serialize};
use serde_json;
use serde_yaml;
use std::env;
use std::fs;
use std::path::Path;
use tar::Builder;

mod challenge_manager;
use challenge_manager::{ChallengeManager, ChallengeOperation};

#[derive(Parser)]
#[command(name = "local-configurator")]
#[command(about = "NervCTF Local Configurator", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage CTF challenges
    Challenges {
        #[command(subcommand)]
        operation: ChallengeOperation,
        /// Path to the root directory containing challenges
        #[arg(short, long, default_value = "./challenges")]
        path: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Challenges { operation, path }) => {
            let manager = ChallengeManager::new(&path);
            match manager.execute_operation(operation) {
                Ok(_) => (),
                Err(e) => eprintln!("Error processing challenges: {}", e),
            }
        }
        None => {
            // Load environment variables
            dotenv::dotenv().ok();
            let docker_host =
                env::var("DOCKER_HOST").expect("DOCKER_HOST environment variable not set");
            let docker_url = format!("http://{}:2375", docker_host);

            // Connect to remote Docker daemon
            let docker =
                match Docker::connect_with_http(&docker_url, 4, bollard::API_DEFAULT_VERSION) {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!(
                            "Failed to connect to Docker daemon at {}: {}",
                            docker_url, e
                        );
                        return Ok(());
                    }
                };

            // Check container status
            let container_name = "remote-monitor";
            match get_container_status(&docker, container_name).await? {
                ContainerStatus::Running => println!("Container is already running"),
                ContainerStatus::Stopped => {
                    println!("Starting existing container");
                    docker
                        .start_container(container_name, None::<StartContainerOptions<String>>)
                        .await?;
                }
                ContainerStatus::NotFound => {
                    println!("Container not found, deploying new instance");
                    deploy_remote_monitor(&docker).await?;
                }
            }

            // Now that container is running, load and send challenge configuration
            let config = load_config().await?;
            send_configuration(&config).await?;
        }
    }

    Ok(())
}

enum ContainerStatus {
    Running,
    Stopped,
    NotFound,
}

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
struct ChallengeConfig {
    challenges: Vec<Challenge>,
    ctfd_url: String,
    ctfd_api_key: String,
}

async fn get_container_status(
    docker: &Docker,
    container_name: &str,
) -> Result<ContainerStatus, Box<dyn std::error::Error>> {
    // Try to inspect the container
    match docker
        .inspect_container(container_name, None::<InspectContainerOptions>)
        .await
    {
        Ok(container_info) => {
            if container_info
                .state
                .as_ref()
                .and_then(|s| s.running)
                .unwrap_or(false)
            {
                Ok(ContainerStatus::Running)
            } else {
                Ok(ContainerStatus::Stopped)
            }
        }
        Err(e) => {
            if e.to_string().contains("No such container") {
                Ok(ContainerStatus::NotFound)
            } else {
                Err(e.into())
            }
        }
    }
}

async fn load_config() -> Result<ChallengeConfig, Box<dyn std::error::Error>> {
    let config_path = "config.yml";
    if !Path::new(config_path).exists() {
        return Err("config.yml file not found in project root".into());
    }

    let config_content = fs::read_to_string(config_path)?;
    let mut config: ChallengeConfig = serde_yaml::from_str(&config_content)?;

    // Add CTFd credentials from environment
    config.ctfd_url = env::var("CTFD_URL")?;
    config.ctfd_api_key = env::var("CTFD_API_KEY")?;

    Ok(config)
}

async fn send_configuration(config: &ChallengeConfig) -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new();
    let uri = "http://remote-monitor:8000/configure";
    let body = serde_json::to_string(config)?;

    let req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body))?;

    let resp = client.request(req).await?;

    if resp.status().is_success() {
        println!("Configuration sent successfully to remote-monitor");
    } else {
        let body_bytes = hyper::body::to_bytes(resp.into_body()).await?;
        let body_str = String::from_utf8(body_bytes.to_vec())?;
        return Err(format!("Failed to send configuration: {}", body_str).into());
    }

    Ok(())
}

async fn deploy_remote_monitor(docker: &Docker) -> Result<(), Box<dyn std::error::Error>> {
    // Create tarball of build context
    let mut archive = Vec::new();
    {
        let mut tar = Builder::new(&mut archive);
        tar.append_dir_all(".", "../remote-monitor")?;
        tar.finish()?;
    }

    // Convert to stream for hyper Body
    use futures_util::stream::once;
    use hyper::Body;
    let archive_stream = Body::wrap_stream(once(async move { Ok::<_, std::io::Error>(archive) }));

    // Build options (no cache)
    let build_opts = BuildImageOptions {
        dockerfile: "Dockerfile",
        t: "remote-monitor:latest",
        rm: true,
        nocache: true,
        ..Default::default()
    };

    // Build image on remote Docker daemon
    let mut build_stream = docker.build_image(build_opts, None, Some(archive_stream));
    while let Some(msg) = build_stream.try_next().await? {
        if let Some(stream) = msg.stream {
            print!("{}", stream);
        }
    }
    println!("Image built on remote Docker daemon!");

    // Create container from the built image
    docker
        .create_container(
            Some(CreateContainerOptions {
                name: "remote-monitor",
                platform: None,
            }),
            Config {
                image: Some("remote-monitor:latest"),
                ..Default::default()
            },
        )
        .await?;

    // Start the container
    docker
        .start_container("remote-monitor", None::<StartContainerOptions<String>>)
        .await?;

    println!("Container 'remote-monitor' started on remote Docker daemon!");
    Ok(())
}

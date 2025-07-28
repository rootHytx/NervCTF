use bollard::container::{
    Config, CreateContainerOptions, RemoveContainerOptions, StartContainerOptions,
};
use bollard::image::BuildImageOptions;
use bollard::Docker;
use futures_util::stream::TryStreamExt;
use std::env;
use tar::Builder;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load environment variables from .env file if present
    dotenv::dotenv().ok();

    // Get DOCKER_HOST from environment variable
    let docker_host = env::var("DOCKER_HOST").expect("DOCKER_HOST environment variable not set");
    let docker_url = format!("http://{}:2375", docker_host);

    // Connect to remote Docker daemon
    let docker = Docker::connect_with_http(&docker_url, 4, bollard::API_DEFAULT_VERSION)?;

    // Create a tarball of your build context (e.g., ./remote-monitor)
    let mut archive = Vec::new();
    {
        let mut tar = Builder::new(&mut archive);
        tar.append_dir_all(".", "../remote-monitor")?;
        tar.finish()?;
    }

    // Convert Vec<u8> to a stream of bytes for hyper Body
    use futures_util::stream::once;
    use hyper::Body;
    let archive_stream = Body::wrap_stream(once(async move { Ok::<_, std::io::Error>(archive) }));

    // Set build options (no cache)
    let build_opts = BuildImageOptions {
        dockerfile: "Dockerfile",
        t: "remote-monitor:latest",
        rm: true,
        nocache: true,
        ..Default::default()
    };

    // Build the image on the remote Docker daemon
    let mut build_stream = docker.build_image(build_opts, None, Some(archive_stream));

    while let Some(msg) = build_stream.try_next().await? {
        if let Some(stream) = msg.stream {
            print!("{}", stream);
        }
    }

    println!("Image built on remote Docker daemon!");

    // Remove any existing container with the same name
    let container_name = "remote-monitor";
    let _ = docker
        .remove_container(
            container_name,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await;

    // Create the container from the newly built image
    docker
        .create_container(
            Some(CreateContainerOptions {
                name: container_name,
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
        .start_container(container_name, None::<StartContainerOptions<String>>)
        .await?;

    println!("Container 'remote-monitor' started on remote Docker daemon!");

    Ok(())
}

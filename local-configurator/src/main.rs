use bollard::image::BuildImageOptions;
use bollard::Docker;
use futures_util::stream::TryStreamExt;
use tar::Builder;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Connect to remote Docker daemon
    let docker = Docker::connect_with_http("http://testctf:2375", 4, bollard::API_DEFAULT_VERSION)?;

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

    // Set build options
    let build_opts = BuildImageOptions {
        dockerfile: "Dockerfile",
        t: "remote-monitor:latest",
        rm: true,
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
    Ok(())
}

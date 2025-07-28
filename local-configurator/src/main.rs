use bollard::{image::ListImagesOptions, Docker, API_DEFAULT_VERSION};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let docker = Docker::connect_with_http("http://testctf:2375", 4, API_DEFAULT_VERSION)?;
    let images = docker
        .list_images(Some(ListImagesOptions::<String> {
            all: true,
            ..Default::default()
        }))
        .await?;

    println!("Available Docker images (RepoTags):");
    for image in images {
        for tag in image.repo_tags {
            println!("{}", tag);
        }
    }

    Ok(())
}

use async_tempfile::TempDir;
use circe_lib::{daemon::Daemon, Reference};
use color_eyre::Result;
use simple_test_case::test_case;

#[test_case("docker.io/library/hello-world:latest"; "hello-world")]
#[test_case("docker.io/library/alpine:latest"; "alpine")]
#[test_log::test(tokio::test)]
async fn pull_from_daemon(image: &str) -> Result<()> {
    // Skip the test if Docker daemon is not available
    if !circe_lib::daemon::is_daemon_available().await {
        eprintln!("skipping test; Docker daemon not available");
        return Ok(());
    }

    let reference = image.parse::<Reference>()?;
    let daemon = Daemon::builder().reference(reference).build().await?;

    // List and verify layers
    let layers = daemon.layers().await?;
    assert!(!layers.is_empty(), "image should have at least one layer");

    // Apply layers to disk
    let tmp = TempDir::new().await?;
    for layer in layers {
        let path = tmp.dir_path().join(layer.digest.as_hex());
        daemon.apply_layer(&layer, &path).await?;
    }

    Ok(())
}

#[test_log::test(tokio::test)]
async fn list_daemon_images() -> Result<()> {
    // Skip the test if Docker daemon is not available
    if !circe_lib::daemon::is_daemon_available().await {
        eprintln!("skipping test; Docker daemon not available");
        return Ok(());
    }

    let reference = "docker.io/library/alpine:latest".parse::<Reference>()?;
    let daemon = Daemon::builder().reference(reference).build().await?;

    let images = daemon.list_images().await?;

    // We should have at least some images in the daemon
    assert!(
        !images.is_empty(),
        "Docker daemon should have at least one image"
    );

    // Print the first few images for debugging
    for (i, image) in images.iter().take(5).enumerate() {
        println!("Image {}: {}", i + 1, image);
    }

    Ok(())
}

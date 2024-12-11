use async_tempfile::TempDir;
use circe::{registry::Registry, Platform, Reference};
use color_eyre::Result;
use simple_test_case::test_case;

#[test_case("docker.io/library/alpine:latest", None; "docker.io/library/alpine:latest")]
#[test_case("docker.io/library/ubuntu:latest", None; "docker.io/library/ubuntu:latest")]
#[tokio::test]
async fn single_platform_layers(image: &str, platform: Option<Platform>) -> Result<()> {
    let reference = image.parse::<Reference>()?;
    let layers = Registry::builder()
        .maybe_platform(platform)
        .reference(reference)
        .build()
        .await?
        .layers()
        .await?;

    assert!(!layers.is_empty(), "image should have at least one layer");
    Ok(())
}

#[test_case("docker.io/library/golang:latest", Platform::linux_amd64(); "docker.io/library/golang:latest.linux_amd64")]
#[test_case("docker.io/library/golang:latest", Platform::linux_arm64(); "docker.io/library/golang:latest.linux_arm64")]
#[tokio::test]
async fn multi_platform_layers(image: &str, platform: Platform) -> Result<()> {
    let reference = image.parse::<Reference>()?;
    let layers = Registry::builder()
        .platform(platform)
        .reference(reference)
        .build()
        .await?
        .layers()
        .await?;

    assert!(!layers.is_empty(), "image should have at least one layer");
    Ok(())
}

#[test_case("docker.io/library/golang:latest", Platform::linux_amd64(); "docker.io/library/golang:latest.linux_amd64")]
#[test_log::test(tokio::test)]
async fn pull_layer(image: &str, platform: Platform) -> Result<()> {
    let reference = image.parse::<Reference>()?;
    let registry = Registry::builder()
        .platform(platform)
        .reference(reference)
        .build()
        .await?;

    let layers = registry.layers().await?;
    assert!(!layers.is_empty(), "image should have at least one layer");

    let tmp = TempDir::new().await?;

    for layer in layers {
        let path = tmp.dir_path().join(layer.digest.as_hex());
        registry.apply_layer(&layer, &path).await?;
    }

    Ok(())
}

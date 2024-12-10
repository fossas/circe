use circe::{Platform, Reference};
use color_eyre::Result;
use simple_test_case::test_case;

#[test_case("docker.io/library/alpine:latest", None; "docker.io/library/alpine:latest")]
#[test_case("docker.io/library/ubuntu:latest", None; "docker.io/library/ubuntu:latest")]
#[tokio::test]
async fn single_platform_layers(image: &str, platform: Option<Platform>) -> Result<()> {
    let reference = image.parse::<Reference>()?;
    let layers = circe::registry::layers(platform.as_ref(), &reference).await?;

    // Verify we got some layers back
    assert!(!layers.is_empty(), "image should have at least one layer");
    Ok(())
}

#[test_case("docker.io/library/golang:latest", Platform::linux_amd64(); "docker.io/library/golang:latest.linux_amd64")]
#[test_case("docker.io/library/golang:latest", Platform::linux_arm64(); "docker.io/library/golang:latest.linux_arm64")]
#[tokio::test]
async fn multi_platform_layers(image: &str, platform: Platform) -> Result<()> {
    let reference = image.parse::<Reference>()?;
    let layers = circe::registry::layers(Some(&platform), &reference).await?;

    // Verify we got some layers back
    assert!(!layers.is_empty(), "image should have at least one layer");
    Ok(())
}

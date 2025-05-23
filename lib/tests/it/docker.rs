use async_tempfile::TempDir;
use circe_lib::{registry::Registry, Authentication, Reference, Source};
use color_eyre::Result;
use simple_test_case::test_case;

// These tests require that your local docker instance is authenticated with the servers.
// This is performed before tests are run in CI, but you may need to `docker login` locally.
#[test_case("quay.io/fossa/hubble-api:latest"; "quay.io/fossa/hubble-api:latest")]
#[test_case("ghcr.io/fossas/sherlock/server:latest"; "ghcr.io/fossas/sherlock/server:latest")]
#[test_case("docker.io/fossaeng/hellotest:latest"; "docker.io/fossaeng/hellotest:latest")]
#[test_log::test(tokio::test)]
#[cfg_attr(
    not(feature = "test-docker-interop"),
    ignore = "ignoring tests that require docker to be installed"
)]
async fn pull_authed(image: &str) -> Result<()> {
    let reference = image.parse::<Reference>()?;
    let auth = Authentication::docker(&reference).await?;

    let registry = Registry::builder()
        .auth(auth)
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

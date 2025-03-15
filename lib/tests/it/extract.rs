use async_tempfile::TempDir;
use circe_lib::{
    extract::{extract, Report, Strategy},
    registry::Registry,
    Digest, Reference, Source,
};
use color_eyre::Result;
use serde_json::{json, Value};
use simple_test_case::test_case;
use std::{path::PathBuf, str::FromStr};

macro_rules! assert_layers_extracted {
    ($report:expr, $layers:expr) => {
        pretty_assertions::assert_eq!(
            $report
                .layers
                .iter()
                .map(|(l, _)| l.to_string())
                .collect::<Vec<_>>(),
            $layers.into_iter().collect::<Vec<_>>(),
            "expected layers not found in report",
        );
    };
}

#[test_log::test(tokio::test)]
async fn report_roundtrip() -> Result<()> {
    let digest_img = Digest::from_str(
        "sha256:931040ffeedc9148b2ed852bc1a9531af141a6bc1f4761ea96c1f2c13b8b6659",
    )?;
    let digest_layer_1 = Digest::from_str(
        "sha256:a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4",
    )?;
    let digest_layer_2 = Digest::from_str(
        "sha256:b3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4",
    )?;

    let report = Report::builder()
        .digest(digest_img.clone())
        .layers([
            (digest_layer_1.clone(), PathBuf::from("/tmp/layer1")),
            (digest_layer_2.clone(), PathBuf::from("/tmp/layer2")),
        ])
        .build();

    let json = report.render()?;
    let parsed = serde_json::from_str::<Value>(&json)?;

    pretty_assertions::assert_eq!(
        parsed,
        json!({
            "digest": digest_img.to_string(),
            "layers": [
                [digest_layer_1.to_string(), "/tmp/layer1"],
                [digest_layer_2.to_string(), "/tmp/layer2"],
            ],
        })
    );

    Ok(())
}

#[test_case("cgr.dev/chainguard/wolfi-base:latest"; "cgr.dev/chainguard/wolfi-base:latest")]
#[test_case("docker.io/contribsys/faktory:latest"; "docker.io/contribsys/faktory:latest")]
#[test_log::test(tokio::test)]
#[cfg_attr(
    not(feature = "test-docker-interop"),
    ignore = "ignoring tests that require docker to be installed"
)]
async fn report(image: &str) -> Result<()> {
    let reference = image.parse::<Reference>()?;
    let registry = Registry::builder().reference(reference).build().await?;

    let tmp = TempDir::new().await?;
    let layers = registry.layers().await?;
    assert!(!layers.is_empty(), "image should have at least one layer");

    let extracted = extract(
        &registry,
        tmp.dir_path(),
        layers.iter().cloned().map(Strategy::Separate),
    )
    .await?;

    let report = Report::builder()
        .digest(registry.digest().await?)
        .layers(extracted)
        .build();

    let actual_digest = registry.digest().await?;
    pretty_assertions::assert_eq!(report.digest, actual_digest.to_string());
    assert_layers_extracted!(report, layers.iter().map(|l| l.digest.to_string()));

    // Hard coded so that tests will notice if we accidentally change this path
    let path = tmp.dir_path().join("image.json");
    report.write(tmp.dir_path()).await?;

    let report_text = tokio::fs::read_to_string(&path).await?;
    pretty_assertions::assert_eq!(report_text, report.render()?);

    Ok(())
}

#[test_case("cgr.dev/chainguard/wolfi-base:latest"; "cgr.dev/chainguard/wolfi-base:latest")]
#[test_case("docker.io/contribsys/faktory:latest"; "docker.io/contribsys/faktory:latest")]
#[test_log::test(tokio::test)]
#[cfg_attr(
    not(feature = "test-docker-interop"),
    ignore = "ignoring tests that require docker to be installed"
)]
async fn squash(image: &str) -> Result<()> {
    let reference = image.parse::<Reference>()?;
    let registry = Registry::builder().reference(reference).build().await?;

    let tmp = TempDir::new().await?;
    let layers = registry.layers().await?;
    assert!(!layers.is_empty(), "image should have at least one layer");

    let extracted = extract(&registry, tmp.dir_path(), Strategy::Squash(layers)).await?;
    let report = Report::builder()
        .digest(registry.digest().await?)
        .layers(extracted)
        .build();

    // We don't really know what the contents of the images will be over time,
    // so we just check that the layers are as we expect for this test.
    // The rest of the report is tested more thoroughly in `report`.
    assert!(!report.layers.is_empty(), "layers must have been extracted");

    Ok(())
}

#[test_case("cgr.dev/chainguard/wolfi-base:latest"; "cgr.dev/chainguard/wolfi-base:latest")]
#[test_case("docker.io/contribsys/faktory:latest"; "docker.io/contribsys/faktory:latest")]
#[test_log::test(tokio::test)]
#[cfg_attr(
    not(feature = "test-docker-interop"),
    ignore = "ignoring tests that require docker to be installed"
)]
async fn base(image: &str) -> Result<()> {
    let reference = image.parse::<Reference>()?;
    let registry = Registry::builder().reference(reference).build().await?;

    let tmp = TempDir::new().await?;
    let base = registry
        .layers()
        .await?
        .first()
        .cloned()
        .expect("image should have at least one layer");
    let extracted = extract(&registry, tmp.dir_path(), Strategy::Separate(base.clone())).await?;
    let report = Report::builder()
        .digest(registry.digest().await?)
        .layers(extracted)
        .build();

    // We don't really know what the contents of the images will be over time,
    // so we just check that the layers are as we expect for this test.
    // The rest of the report is tested more thoroughly in `report`.
    assert_layers_extracted!(report, [base.digest.to_string()]);

    Ok(())
}

/// Requires an image with more than one layer
#[test_case("docker.io/contribsys/faktory:latest"; "docker.io/contribsys/faktory:latest")]
#[test_log::test(tokio::test)]
#[cfg_attr(
    not(feature = "test-docker-interop"),
    ignore = "ignoring tests that require docker to be installed"
)]
async fn squash_other(image: &str) -> Result<()> {
    let reference = image.parse::<Reference>()?;
    let registry = Registry::builder().reference(reference).build().await?;

    let tmp = TempDir::new().await?;
    let layers = registry.layers().await?;

    let extracted = extract(
        &registry,
        tmp.dir_path(),
        Strategy::Squash(layers.into_iter().skip(1).collect()),
    )
    .await?;
    let report = Report::builder()
        .digest(registry.digest().await?)
        .layers(extracted)
        .build();

    // We don't really know what the contents of the images will be over time,
    // so we just check that the layers are as we expect for this test.
    // The rest of the report is tested more thoroughly in `report`.
    assert!(!report.layers.is_empty(), "layers must have been extracted");

    Ok(())
}

/// Requires an image with more than one layer
#[test_case("docker.io/contribsys/faktory:latest"; "docker.io/contribsys/faktory:latest")]
#[test_log::test(tokio::test)]
#[cfg_attr(
    not(feature = "test-docker-interop"),
    ignore = "ignoring tests that require docker to be installed"
)]
async fn base_and_squash_other(image: &str) -> Result<()> {
    let reference = image.parse::<Reference>()?;
    let registry = Registry::builder().reference(reference).build().await?;

    let tmp = TempDir::new().await?;
    let layers = registry.layers().await?;

    let strategies = match layers.as_slice() {
        [] => unreachable!(),
        [base] => vec![Strategy::Separate(base.clone())],
        [base, rest @ ..] => vec![
            Strategy::Separate(base.clone()),
            Strategy::Squash(rest.to_vec()),
        ],
    };

    let extracted = extract(&registry, tmp.dir_path(), strategies).await?;
    let report = Report::builder()
        .digest(registry.digest().await?)
        .layers(extracted)
        .build();

    // We don't really know what the contents of the images will be over time,
    // so we just check that the layers are as we expect for this test.
    // The rest of the report is tested more thoroughly in `report`.
    assert!(!report.layers.is_empty(), "layers must have been extracted");

    Ok(())
}

#[test_case("cgr.dev/chainguard/wolfi-base:latest"; "cgr.dev/chainguard/wolfi-base:latest")]
#[test_case("docker.io/contribsys/faktory:latest"; "docker.io/contribsys/faktory:latest")]
#[test_log::test(tokio::test)]
#[cfg_attr(
    not(feature = "test-docker-interop"),
    ignore = "ignoring tests that require docker to be installed"
)]
async fn separate(image: &str) -> Result<()> {
    let reference = image.parse::<Reference>()?;
    let registry = Registry::builder().reference(reference).build().await?;

    let layers = registry.layers().await?;
    assert!(!layers.is_empty(), "image should have at least one layer");

    let tmp = TempDir::new().await?;
    let strategies = layers
        .iter()
        .cloned()
        .map(Strategy::Separate)
        .collect::<Vec<_>>();

    let extracted = extract(&registry, tmp.dir_path(), strategies).await?;
    let report = Report::builder()
        .digest(registry.digest().await?)
        .layers(extracted)
        .build();

    // We don't really know what the contents of the images will be over time,
    // so we just check that the layers are as we expect for this test.
    // The rest of the report is tested more thoroughly in `report`.
    assert_layers_extracted!(report, layers.iter().map(|l| l.digest.to_string()));

    Ok(())
}

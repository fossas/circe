use async_tempfile::TempDir;
use async_walkdir::WalkDir;
use circe_lib::{registry::Registry, Filters, Platform, Reference};
use color_eyre::Result;
use simple_test_case::test_case;

#[test_case("cgr.dev/chainguard/wolfi-base:latest", Some(Platform::linux_amd64()); "cgr.dev/chainguard/wolfi-base:latest.linux_amd64")]
#[test_case("cgr.dev/chainguard/wolfi-base:latest", Some(Platform::linux_arm64()); "cgr.dev/chainguard/wolfi-base:latest.linux_arm64")]
#[test_case("cgr.dev/chainguard/wolfi-base:latest", None; "cgr.dev/chainguard/wolfi-base:latest_default")]
#[test_case("docker.io/library/ubuntu:latest", None; "docker.io/library/ubuntu:latest_default")]
#[test_case("docker.io/library/alpine:latest", None; "docker.io/library/alpine:latest_default")]
#[test_log::test(tokio::test)]
async fn pull_layer(image: &str, platform: Option<Platform>) -> Result<()> {
    let reference = image.parse::<Reference>()?;
    let registry = Registry::builder()
        .maybe_platform(platform)
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

#[test_case(vec![], vec!["*.json"], vec![], vec![r".*\.so(?:\.\d+)*$"]; "file_filters")]
#[test_log::test(tokio::test)]
async fn pull_layer_filtered(
    layer_globs: Vec<&str>,
    file_globs: Vec<&str>,
    layer_regexes: Vec<&str>,
    file_regexes: Vec<&str>,
) -> Result<()> {
    use futures_lite::StreamExt;

    let platform = Platform::linux_amd64();
    let reference = Reference::builder()
        .host("cgr.dev")
        .repository("chainguard/wolfi-base")
        .tag("latest")
        .build();

    let layer_filters = Filters::parse_glob(layer_globs)?;
    let file_filters = Filters::parse_glob(file_globs)?;
    let layer_regexes = Filters::parse_regex(layer_regexes)?;
    let file_regexes = Filters::parse_regex(file_regexes)?;

    let registry = Registry::builder()
        .platform(platform)
        .reference(reference)
        .layer_filters(layer_filters + layer_regexes)
        .file_filters(file_filters + file_regexes)
        .build()
        .await?;

    let layers = registry.layers().await?;
    assert!(!layers.is_empty(), "image should have at least one layer");

    tracing::info!(?layers, "enumerated layers");
    let tmp = TempDir::new().await?;
    for layer in layers {
        tracing::info!(?layer, "applying layer");
        registry.apply_layer(&layer, tmp.dir_path()).await?;
    }

    tracing::info!("checking downloaded files for filters");
    let mut walker = WalkDir::new(tmp.dir_path());
    let mut read_any = false;
    while let Some(entry) = walker.next().await {
        let entry = entry.expect("walk directory");
        let ft = entry.file_type().await.expect("get file type");
        if !ft.is_file() {
            continue;
        }

        let path = entry.path();
        let name = path.file_name().expect("get file name").to_string_lossy();
        if !name.ends_with(".json") && !name.contains(".so") {
            panic!("unexpected file allowed by filters (name: {name:?}): {path:?}");
        }

        read_any = true;
    }

    assert!(
        read_any,
        "no files were read, but filters should have allowed some"
    );
    Ok(())
}

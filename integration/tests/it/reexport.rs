use std::collections::HashSet;

use assert_fs::prelude::*;
use color_eyre::{Result, eyre::Context};
use serde::Deserialize;
use serde_json::Value;
use simple_test_case::test_case;
use xshell::{Shell, cmd};

#[test_case(
    "nginx:latest";
    "nginx:latest"
)]
#[test_log::test(tokio::test)]
#[cfg_attr(
    not(feature = "test-integration"),
    ignore = "skipping integration tests"
)]
async fn daemon(image: &str) -> Result<()> {
    let workspace = crate::workspace_root();
    let temp = assert_fs::TempDir::new().context("create temp dir")?;
    let reexport = temp.child("reexport.tar").to_string_lossy().to_string();

    tracing::info!(workspace = %workspace.display(), "create shell");
    let sh = Shell::new().context("create shell")?;
    sh.change_dir(&workspace);
    sh.set_var("CIRCE_DISABLE_REGISTRY_OCI", "true");

    tracing::info!(image, "pull image");
    cmd!(sh, "docker pull {image}").run()?;

    tracing::info!(image, target = %reexport, "run circe reexport");
    cmd!(sh, "cargo run -- reexport {image} {reexport}").run()?;

    tracing::info!(target = %reexport, "run fossa container analyze");
    let reexport_output = cmd!(sh, "fossa container analyze {reexport} -o").read()?;

    tracing::info!(target = %reexport, "read cli output");
    let reexport_output = serde_json::from_str::<CliContainerOutput>(&reexport_output)?;

    // We don't have any images to compare, so we cannot do much in the way of assertions on the object.
    // But at minimum, we can expect that the image should have contained layers.
    assert!(
        !reexport_output.image.layers.is_empty(),
        "reexported image should have layers"
    );

    Ok(())
}

#[test_case(
    "nginx:latest";
    "nginx:latest"
)]
#[test_log::test(tokio::test)]
#[cfg_attr(
    not(feature = "test-integration"),
    ignore = "skipping integration tests"
)]
async fn pull_and_save(image: &str) -> Result<()> {
    let workspace = crate::workspace_root();
    let temp = assert_fs::TempDir::new().context("create temp dir")?;
    let output = temp.child("image.tar").to_string_lossy().to_string();
    let reexport = temp.child("reexport.tar").to_string_lossy().to_string();

    tracing::info!(workspace = %workspace.display(), "create shell");
    let sh = Shell::new().context("create shell")?;
    sh.change_dir(&workspace);
    sh.set_var("CIRCE_DISABLE_REGISTRY_OCI", "true");

    tracing::info!(image, output, "pull and save image");
    cmd!(sh, "docker pull {image}").run()?;
    cmd!(sh, "docker save {image} -o {output}").run()?;

    tracing::info!(image, output, target = %reexport, "run circe reexport");
    cmd!(sh, "cargo run -- reexport {output} {reexport}").run()?;

    tracing::info!(target = %reexport, "run fossa container analyze");
    let reexport_output = cmd!(sh, "fossa container analyze {reexport} -o").read()?;

    tracing::info!(target = %reexport, "read cli output");
    let reexport_output = serde_json::from_str::<CliContainerOutput>(&reexport_output)?;

    // We don't have any images to compare, so we cannot do much in the way of assertions on the object.
    // But at minimum, we can expect that the image should have contained layers.
    assert!(
        !reexport_output.image.layers.is_empty(),
        "reexported image should have layers"
    );

    Ok(())
}

/// Test that `circe reexport` then allows FOSSA CLI to scan images
/// that have previously been ticketed as failing to be analyzed.
#[test_case(
    "gcr.io/go-containerregistry/crane";
    "gcr.io/go-containerregistry/crane"
)]
#[test_case(
    "nvcr.io/nvidia/cloud-native/gpu-operator-validator:v24.9.0";
    "nvcr.io/nvidia/cloud-native/gpu-operator-validator:v24.9.0"
)]
#[test_case(
    "registry.access.redhat.com/ubi9/ubi-minimal@sha256:fb77e447ab97f3fecd15d2fa5361a99fe2f34b41422e8ebb3612eecd33922fa0";
    "registry.access.redhat.com/ubi9/ubi-minimal@sha256:fb77e447ab97f3fecd15d2fa5361a99fe2f34b41422e8ebb3612eecd33922fa0"
)]
#[test_case(
    "alpine:3.16.0";
    "alpine:3.16.0"
)]
#[test_case(
    "index.docker.io/library/alpine:latest";
    "index.docker.io/library/alpine:latest"
)]
#[test_log::test(tokio::test)]
#[cfg_attr(
    not(feature = "test-integration"),
    ignore = "skipping integration tests"
)]
async fn scannable(image: &str) -> Result<()> {
    let workspace = crate::workspace_root();
    let temp = assert_fs::TempDir::new().context("create temp dir")?;
    let reexport = temp.child("reexport.tar").to_string_lossy().to_string();

    tracing::info!(workspace = %workspace.display(), "create shell");
    let sh = Shell::new().context("create shell")?;
    sh.change_dir(&workspace);
    sh.set_var("CIRCE_DISABLE_DAEMON_DOCKER", "true");

    tracing::info!(image, target = %reexport, "run circe reexport");
    cmd!(sh, "cargo run -- reexport {image} {reexport}").run()?;

    tracing::info!(target = %reexport, "run fossa container analyze");
    let reexport_output = cmd!(sh, "fossa container analyze {reexport} -o").read()?;

    tracing::info!(target = %reexport, "read cli output");
    let reexport_output = serde_json::from_str::<CliContainerOutput>(&reexport_output)?;

    // We don't have any images to compare, so we cannot do much in the way of assertions on the object.
    // But at minimum, we can expect that the image should have contained layers.
    assert!(
        !reexport_output.image.layers.is_empty(),
        "reexported image should have layers"
    );

    Ok(())
}

/// Test that the `circe reexport` command creates a tarball that when scanned with FOSSA CLI
/// produces the same output as the original image (other than the `layerId`s).
///
/// Note: the actual vendored tarballs are copied from the tarballs with the same name
/// in the FOSSA CLI repository; the Dockerfiles used to build the versions in Docker Hub
/// are slightly different but conceptually the same.
#[test_case(
    "docker.io/fossaeng/changeset_example:latest",
    "integration/testdata/fossacli/changeset_example.tar";
    "fossaeng/changeset_example:latest"
)]
#[test_case(
    "docker.io/fossaeng/changesets_symlinked_entries:latest",
    "integration/testdata/fossacli/changesets_symlinked_entries.tar";
    "fossaeng/changesets_symlinked_entries:latest"
)]
#[test_case(
    "docker.io/fossaeng/app_deps_example:latest",
    "integration/testdata/fossacli/app_deps_example.tar";
    "fossaeng/app_deps_example:latest"
)]
#[test_log::test(tokio::test)]
#[cfg_attr(
    not(all(feature = "test-docker-interop", feature = "test-integration")),
    ignore = "skipping integration tests that require docker to be installed"
)]
async fn compare(image: &str, reference: &str) -> Result<()> {
    let workspace = crate::workspace_root();
    let temp = assert_fs::TempDir::new().context("create temp dir")?;
    let reexport = temp.child("reexport.tar").to_string_lossy().to_string();

    tracing::info!(workspace = %workspace.display(), "create shell");
    let sh = Shell::new().context("create shell")?;
    sh.change_dir(&workspace);
    sh.set_var("CIRCE_DISABLE_DAEMON_DOCKER", "true");

    tracing::info!(image, target = %reexport, "run circe reexport");
    cmd!(sh, "cargo run -- reexport {image} {reexport}").run()?;

    tracing::info!(target = %reexport, %reference, "run fossa container analyze");
    let reexport_output = cmd!(sh, "fossa container analyze {reexport} -o").read()?;
    let reference_output = cmd!(sh, "fossa container analyze {reference} -o").read()?;

    tracing::info!(target = %reexport, %reference, "compare cli output");
    let reexport_output = serde_json::from_str::<CliContainerOutput>(&reexport_output)?;
    let reference_output = serde_json::from_str::<CliContainerOutput>(&reference_output)?;

    pretty_assertions::assert_eq!(reference_output, reexport_output);

    Ok(())
}

/// The output of the `fossa container analyze` command.
#[derive(Debug, PartialEq, Eq, Deserialize)]
struct CliContainerOutput {
    image: CliContainerImage,
}

/// A container image reported by FOSSA CLI.
#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CliContainerImage {
    os: String,
    os_release: String,
    layers: Vec<CliContainerLayer>,
}

/// A layer reported by FOSSA CLI.
///
/// Contains a subset of the FOSSA CLI output fields that we want to compare for equality.
/// The order of observations and source units don't matter to FOSSA and therefore they don't matter to circe.
#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CliContainerLayer {
    observations: HashSet<Value>,
    src_units: HashSet<Value>,
}

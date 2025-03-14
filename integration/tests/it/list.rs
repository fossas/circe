use assert_fs::prelude::*;
use color_eyre::{Result, eyre::Context};
use simple_test_case::test_case;
use xshell::{Shell, cmd};

#[test_case(
    "nginx:latest";
    "nginx:latest"
)]
#[test_log::test(tokio::test)]
#[cfg_attr(
    not(all(feature = "test-integration", feature = "test-docker-interop")),
    ignore = "skipping integration tests that require docker to be installed"
)]
async fn daemon(image: &str) -> Result<()> {
    let workspace = crate::workspace_root();
    let temp = assert_fs::TempDir::new().context("create temp dir")?;
    let output = temp.path().to_string_lossy().to_string();

    tracing::info!(workspace = %workspace.display(), "create shell");
    let sh = Shell::new().context("create shell")?;
    sh.change_dir(&workspace);
    sh.set_var("CIRCE_DISABLE_REGISTRY_OCI", "true");

    tracing::info!(image, target = %output, "run circe list");
    cmd!(sh, "cargo run -- list {image}").run()?;

    Ok(())
}

#[test_case(
    "nginx:latest";
    "nginx:latest"
)]
#[test_log::test(tokio::test)]
#[cfg_attr(
    not(all(feature = "test-integration", feature = "test-docker-interop")),
    ignore = "skipping integration tests that require docker to be installed"
)]
async fn pull_and_save(image: &str) -> Result<()> {
    let workspace = crate::workspace_root();
    let temp = assert_fs::TempDir::new().context("create temp dir")?;
    let output = temp.child("image.tar").to_string_lossy().to_string();

    tracing::info!(workspace = %workspace.display(), "create shell");
    let sh = Shell::new().context("create shell")?;
    sh.change_dir(&workspace);
    sh.set_var("CIRCE_DISABLE_REGISTRY_OCI", "true");

    tracing::info!(image, output, "pull and save image");
    cmd!(sh, "docker pull {image}").run()?;
    cmd!(sh, "docker save {image} -o {output}").run()?;

    tracing::info!(image, output, "run circe list");
    cmd!(sh, "cargo run -- list {output}").run()?;

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
async fn oci_registry(image: &str) -> Result<()> {
    let workspace = crate::workspace_root();
    let temp = assert_fs::TempDir::new().context("create temp dir")?;
    let output = temp.path().to_string_lossy().to_string();

    tracing::info!(workspace = %workspace.display(), "create shell");
    let sh = Shell::new().context("create shell")?;
    sh.change_dir(&workspace);
    sh.set_var("CIRCE_DISABLE_DAEMON_DOCKER", "true");

    tracing::info!(image, target = %output, "run circe list");
    cmd!(sh, "cargo run -- list {image}").run()?;

    Ok(())
}

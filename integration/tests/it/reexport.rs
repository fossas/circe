use std::{collections::HashSet, path::PathBuf};

use assert_fs::prelude::*;
use color_eyre::{Result, eyre::Context};
use serde::Deserialize;
use serde_json::Value;
use simple_test_case::test_case;
use xshell::{Shell, cmd};

/// Test that the `circe reexport` command can create a tarball that is compatible
/// with FOSSA CLI's container scanning functionality.
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
    not(feature = "test-docker-interop"),
    ignore = "ignoring tests that require docker to be installed"
)]
async fn reexport(image: &str, reference: &str) -> Result<()> {
    let workspace = workspace_root();
    let temp = assert_fs::TempDir::new().context("create temp dir")?;
    let reexport = temp.child("reexport.tar").to_string_lossy().to_string();

    tracing::info!(workspace = %workspace.display(), "create shell");
    let sh = Shell::new().context("create shell")?;
    sh.change_dir(&workspace);

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

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

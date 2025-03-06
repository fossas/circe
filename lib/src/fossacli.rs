use std::path::Path;

use color_eyre::Result;

use crate::extract::Report;

/// The name of the tarball in the output directory.
// Note: if this changes, make sure to update the docs.
pub const TARBALL: &str = "image.tar";

/// Given an extracted directory and its report, re-exports the directory as a tarball that FOSSA CLI can scan.
///
/// The idea here is that FOSSA CLI has been built with the assumption that the tarball is the baseline unit
/// of container scanning. Untangling this and turning it into "scan the contents of a directory" is a larger lift
/// than this project currently has budget for.
///
/// Circe is intended to support FOSSA CLI in its ability to pull images from remote OCI hosts that use a different
/// container format than the one FOSSA CLI is built t support.
///
/// As such, Circe works around this by becoming a "middle layer" when invoked by FOSSA CLI:
/// it pulls the image, does the extraction, and then re-bundles the image into a tar format FOSSA CLI
/// knows how to support.
///
/// The image is saved at `image.tar` in the output directory, next to `image.json`.
pub fn reexport_tarball(report: &Report, output: &Path) -> Result<()> {
    todo!()
}

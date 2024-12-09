//! Interacts with remote OCI registries.

use color_eyre::eyre::Result;

use crate::Reference;

/// Enumerate layers for a reference in the remote registry.
pub async fn layers(reference: &Reference) -> Result<Vec<String>> {
    Ok(vec![])
}

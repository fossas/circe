//! Interacts with remote OCI registries.

use std::str::FromStr;

use color_eyre::eyre::{Context, Result};
use oci_client::{
    client::ClientConfig, manifest::ImageIndexEntry, secrets::RegistryAuth, Client,
    Reference as OciReference,
};

use crate::{ext::PriorityFind, LayerReference, Platform, Reference, Version};

/// Enumerate layers for a container reference in the remote registry.
/// Layers are returned in order from the base image to the application.
#[tracing::instrument]
pub async fn layers(
    platform: Option<&Platform>,
    reference: &Reference,
) -> Result<Vec<LayerReference>> {
    let client = client(platform.cloned());
    let auth = RegistryAuth::Anonymous;

    let oci_ref = OciReference::from(reference);
    let (manifest, _) = client
        .pull_image_manifest(&oci_ref, &auth)
        .await
        .context("pull image manifest: {oci_ref}")?;

    manifest
        .layers
        .into_iter()
        .map(|layer| LayerReference::from_str(&layer.digest))
        .collect()
}

impl From<&Reference> for OciReference {
    fn from(reference: &Reference) -> Self {
        match &reference.version {
            Version::Tag(tag) => Self::with_tag(
                reference.host.clone(),
                reference.repository.clone(),
                tag.clone(),
            ),
            Version::Digest(digest) => Self::with_digest(
                reference.host.clone(),
                reference.repository.clone(),
                digest.to_string(),
            ),
        }
    }
}

fn client(platform: Option<Platform>) -> Client {
    let mut config = ClientConfig::default();
    config.platform_resolver = match platform {
        Some(platform) => Some(Box::new(target_platform_resolver(platform))),
        None => Some(Box::new(current_platform_resolver)),
    };
    Client::new(config)
}

fn target_platform_resolver(target: Platform) -> impl Fn(&[ImageIndexEntry]) -> Option<String> {
    move |entries: &[ImageIndexEntry]| {
        entries
            .iter()
            .find(|entry| {
                entry.platform.as_ref().map_or(false, |platform| {
                    platform.os == target.os && platform.architecture == target.architecture
                })
            })
            .map(|entry| entry.digest.clone())
    }
}

fn current_platform_resolver(entries: &[ImageIndexEntry]) -> Option<String> {
    let current_os = go_os();
    let current_arch = go_arch();
    let linux = Platform::LINUX;
    let amd64 = Platform::AMD64;
    entries
        .iter()
        .priority_find(|entry| match entry.platform.as_ref() {
            None => 0,
            Some(p) if p.os == current_os && p.architecture == current_arch => 1,
            Some(p) if p.os == linux && p.architecture == current_arch => 2,
            Some(p) if p.os == linux && p.architecture == amd64 => 3,
            _ => 4,
        })
        .map(|entry| entry.digest.clone())
}

/// Returns the current OS as a string that matches a `GOOS` constant.
/// This is required because the OCI spec requires the OS to be a valid GOOS value.
// If you get a compile error here, you need to add a new `cfg` branch for your platform.
// Valid GOOS values may be gathered from here: https://go.dev/doc/install/source#environment
const fn go_os() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "linux"
    }
    #[cfg(target_os = "macos")]
    {
        "darwin"
    }
    #[cfg(target_os = "windows")]
    {
        "windows"
    }
}

/// Returns the current architecture as a string that matches a `GOARCH` constant.
/// This is required because the OCI spec requires the architecture to be a valid GOARCH value.
// If you get a compile error here, you need to add a new `cfg` branch for your platform.
// Valid GOARCH values may be gathered from here: https://go.dev/doc/install/source#environment
const fn go_arch() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        "amd64"
    }
    #[cfg(target_arch = "aarch64")]
    {
        "arm64"
    }
}

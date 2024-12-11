//! Core library for `circe`, a tool for extracting OCI images.

use bon::Builder;
use color_eyre::{
    eyre::{self, bail, eyre, Context},
    Result, Section, SectionExt,
};
use derive_more::derive::Display;
use itertools::Itertools;
use std::str::FromStr;
use strum::{AsRefStr, EnumIter, IntoEnumIterator};
use tap::Pipe;

mod ext;
pub mod registry;
pub mod transform;

/// Platform represents the platform a container image is built for.
/// This follows the OCI Image Spec's platform definition while also supporting
/// Docker's platform string format (e.g. "linux/amd64").
///
/// ```
/// # use circe::Platform;
/// # use std::str::FromStr;
/// let platform = Platform::from_str("linux/amd64").expect("parse platform");
/// assert_eq!(platform.to_string(), "linux/amd64");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Builder)]
pub struct Platform {
    /// Operating system the container runs on (e.g. "linux", "windows", "darwin").
    ///
    /// Per the OCI spec, OS values must correspond with GOOS.
    /// https://github.com/opencontainers/image-spec/blob/main/image-index.md
    #[builder(into)]
    pub os: String,

    /// CPU architecture (e.g. "amd64", "arm64").
    ///
    /// Per the OCI spec, architecture values must correspond with GOARCH.
    /// https://github.com/opencontainers/image-spec/blob/main/image-index.md
    #[builder(into)]
    pub architecture: String,

    /// Variant of the CPU (e.g. "v7" for armv7).
    ///
    /// Per the OCI spec, this is one of the following features:
    /// https://github.com/opencontainers/image-spec/blob/main/image-index.md#platform-variants
    #[builder(into)]
    pub variant: Option<String>,

    /// Operating system version (e.g. "10.0.14393.1066" for windows).
    ///
    /// Per the OCI spec, valid values are implementation defined.
    #[builder(into)]
    pub os_version: Option<String>,

    /// Additional platform features required.
    ///
    /// Per the OCI spec, the only official feature is "win32k", and only then when the OS is "windows".
    /// Otherwise, valid values are implementation defined.
    #[builder(into, default)]
    pub os_features: Vec<String>,
}

impl Platform {
    /// Canonical name for the linux operating system.
    pub const LINUX: &'static str = "linux";

    /// Canonical name for the macOS operating system.
    pub const DARWIN: &'static str = "darwin";

    /// Canonical name for the Windows operating system.
    pub const WINDOWS: &'static str = "windows";

    /// Canonical name for the AMD64 architecture.
    pub const AMD64: &'static str = "amd64";

    /// Canonical name for the ARM64 architecture.
    pub const ARM64: &'static str = "arm64";

    /// Clone the instance with the given variant.
    pub fn with_variant(self, variant: &str) -> Self {
        Self::builder()
            .os(self.os)
            .architecture(self.architecture)
            .os_features(self.os_features)
            .maybe_os_version(self.os_version)
            .variant(variant)
            .build()
    }

    /// Create an instance for Linux AMD64
    pub fn linux_amd64() -> Self {
        Self::builder()
            .os(Self::LINUX)
            .architecture(Self::AMD64)
            .build()
    }

    /// Create an instance for Linux ARM64
    pub fn linux_arm64() -> Self {
        Self::builder()
            .os(Self::LINUX)
            .architecture(Self::ARM64)
            .build()
    }

    /// Create an instance for Windows AMD64
    pub fn windows_amd64() -> Self {
        Self::builder()
            .os(Self::WINDOWS)
            .architecture(Self::AMD64)
            .build()
    }

    /// Create an instance for macOS ARM64
    pub fn macos_arm64() -> Self {
        Self::builder()
            .os(Self::DARWIN)
            .architecture(Self::ARM64)
            .build()
    }

    /// Create an instance for macOS AMD64
    pub fn macos_amd64() -> Self {
        Self::builder()
            .os(Self::DARWIN)
            .architecture(Self::AMD64)
            .build()
    }
}

impl FromStr for Platform {
    type Err = eyre::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let input_section = || s.to_string().header("Input:");
        let expected_section = || {
            "{os}/{architecture}[/{variant}]"
                .to_string()
                .header("Expected:")
        };
        let examples_section = || {
            vec!["linux/amd64/v7", "darwin/arm64"]
                .join("\n")
                .header("Examples:")
        };

        // Docker platform strings are of the form: os/arch[/variant]
        let parts = s.split('/').collect::<Vec<_>>();
        if parts.iter().any(|part| part.is_empty()) {
            return eyre!("invalid platform format")
                .with_section(input_section)
                .with_section(expected_section)
                .with_section(examples_section)
                .pipe(Err);
        }

        match parts.as_slice() {
            [os, architecture] => Self::builder()
                .os(os.to_string())
                .architecture(architecture.to_string())
                .build()
                .pipe(Ok),
            [os, architecture, variant] => Self::builder()
                .os(os.to_string())
                .architecture(architecture.to_string())
                .variant(variant.to_string())
                .build()
                .pipe(Ok),
            _ => eyre!("invalid platform format")
                .with_section(input_section)
                .with_section(expected_section)
                .with_section(examples_section)
                .pipe(Err),
        }
    }
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.os, self.architecture)?;
        if let Some(variant) = &self.variant {
            write!(f, "/{variant}")?;
        }
        Ok(())
    }
}

/// Create a [`Digest`] from a hex string at compile time.
/// ```
/// let digest = circe::digest!("sha256", "a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4");
/// assert_eq!(digest.algorithm, "sha256");
/// assert_eq!(digest.as_hex(), "a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4");
/// ```
///
/// If algorithm is not provided, it defaults to [`Digest::SHA256`].
/// ```
/// let digest = circe::digest!("a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4");
/// assert_eq!(digest.algorithm, "sha256");
/// assert_eq!(digest.as_hex(), "a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4");
/// ```
///
/// This macro currently assumes that the hash is 32 bytes long.
/// Providing a value of a different length will result in a compile-time error.
/// ```compile_fail
/// let digest = circe::digest!("a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4deadbeef");
/// ```
///
/// You can work around this by providing the size of the hash as a third argument.
/// ```
/// let digest = circe::digest!(circe::Digest::SHA256, "a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4deadbeef", 36);
/// assert_eq!(digest.algorithm, "sha256");
/// assert_eq!(digest.as_hex(), "a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4deadbeef");
/// ```
#[macro_export]
macro_rules! digest {
    ($hex:expr) => {{
        circe::digest!(circe::Digest::SHA256, $hex, 32)
    }};
    ($algorithm:expr, $hex:expr) => {{
        circe::digest!($algorithm, $hex, 32)
    }};
    ($algorithm:expr, $hex:expr, $size:expr) => {{
        const HASH: [u8; $size] = hex_magic::hex!($hex);
        static_assertions::const_assert_ne!(HASH.len(), 0);
        static_assertions::const_assert_ne!($algorithm.len(), 0);
        circe::Digest {
            algorithm: $algorithm.to_string(),
            hash: HASH.to_vec(),
        }
    }};
}

/// A content-addressable digest in the format `algorithm:hash`.
///
/// The `FromStr` implementation parses the format used in OCI containers by default,
/// which is `algorithm:hex`.
///
/// ```
/// # use std::str::FromStr;
/// let digest = circe::Digest::from_str("sha256:a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4").expect("parse digest");
/// assert_eq!(digest.algorithm, "sha256");
/// assert_eq!(digest.as_hex(), "a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Digest {
    /// The hashing algorithm used (e.g. "sha256")
    pub algorithm: String,

    /// The raw hash bytes
    pub hash: Vec<u8>,
}

impl Digest {
    /// The SHA256 algorithm
    pub const SHA256: &'static str = "sha256";

    /// Returns the hash as a hex string
    pub fn as_hex(&self) -> String {
        hex::encode(&self.hash)
    }
}

impl FromStr for Digest {
    type Err = color_eyre::Report;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let input_section = || s.to_string().header("Input:");
        let (algorithm, hex) = s.split_once(':').ok_or_else(|| {
            eyre!("invalid digest format: missing algorithm separator ':'")
                .with_section(input_section)
        })?;

        if algorithm.is_empty() {
            bail!("algorithm cannot be empty");
        }
        if hex.is_empty() {
            bail!("hex cannot be empty");
        }

        Ok(Self {
            algorithm: algorithm.to_string(),
            hash: hex::decode(hex).map_err(|e| eyre!("invalid hex string: {e}"))?,
        })
    }
}

impl std::fmt::Display for Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.algorithm, self.as_hex())
    }
}

impl From<&Digest> for Digest {
    fn from(digest: &Digest) -> Self {
        digest.clone()
    }
}

/// Version identifier for a container image.
///
/// This can be a named tag or a SHA256 digest.
///
/// ```
/// # use circe::{Version, Digest};
/// # use std::str::FromStr;
/// assert_eq!(Version::latest().to_string(), "latest");
/// assert_eq!(Version::tag("other").to_string(), "other");
///
/// let digest = Digest::from_str("sha256:a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4").expect("parse digest");
/// assert_eq!(Version::digest(digest).to_string(), "sha256:a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Display)]
pub enum Version {
    /// A named tag (e.g. "latest", "1.0.0")
    Tag(String),

    /// A SHA256 digest (e.g. "sha256:123abc...")
    Digest(Digest),
}

impl Version {
    /// Returns the tag for "latest".
    ///
    /// ```
    /// # use circe::Version;
    /// assert_eq!(Version::latest().to_string(), "latest");
    /// ```
    pub fn latest() -> Self {
        Self::Tag(String::from("latest"))
    }

    /// Create a tagged instance.
    ///
    /// ```
    /// # use circe::Version;
    /// assert_eq!(Version::tag("latest").to_string(), "latest");
    /// ```
    pub fn tag(tag: &str) -> Self {
        Self::Tag(tag.to_string())
    }

    /// Create a digest instance.
    ///
    /// ```
    /// # use circe::Version;
    /// let digest = circe::digest!("sha256", "a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4");
    /// let version = Version::digest(digest);
    /// assert_eq!(version.to_string(), "sha256:a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4");
    /// ```
    pub fn digest(digest: Digest) -> Self {
        Self::Digest(digest)
    }
}

/// A parsed container image reference.
///
/// ```
/// # use circe::{Reference, Version};
/// # use std::str::FromStr;
/// // Default to latest tag
/// let reference = Reference::from_str("docker.io/library/ubuntu").expect("parse reference");
/// assert_eq!(reference.host, "docker.io");
/// assert_eq!(reference.repository, "library/ubuntu");
/// assert_eq!(reference.version, Version::tag("latest"));
///
/// // Parse a tag
/// let reference = Reference::from_str("docker.io/library/ubuntu:other").expect("parse reference");
/// assert_eq!(reference.host, "docker.io");
/// assert_eq!(reference.repository, "library/ubuntu");
/// assert_eq!(reference.version, Version::tag("other"));
///
/// // Parse a digest
/// let reference = Reference::from_str("docker.io/library/ubuntu@sha256:a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4").expect("parse reference");
/// assert_eq!(reference.host, "docker.io");
/// assert_eq!(reference.repository, "library/ubuntu");
/// assert_eq!(reference.version.to_string(), "sha256:a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Builder)]
pub struct Reference {
    /// Registry host (e.g. "docker.io", "ghcr.io")
    #[builder(into)]
    pub host: String,

    /// Repository name including namespace (e.g. "library/ubuntu", "username/project")
    #[builder(into)]
    pub repository: String,

    /// Version identifier, either a tag or SHA digest
    #[builder(into, default = Version::latest())]
    pub version: Version,
}

impl<S: reference_builder::State> ReferenceBuilder<S> {
    /// Set the reference to a tag version.
    pub fn tag(self, tag: &str) -> ReferenceBuilder<reference_builder::SetVersion<S>>
    where
        S::Version: reference_builder::IsUnset,
    {
        self.version(Version::tag(tag))
    }

    /// Set the reference to a digest version.
    pub fn digest(
        self,
        digest: impl Into<Digest>,
    ) -> ReferenceBuilder<reference_builder::SetVersion<S>>
    where
        S::Version: reference_builder::IsUnset,
    {
        self.version(Version::Digest(digest.into()))
    }
}

impl FromStr for Reference {
    type Err = eyre::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let input_section = || s.to_string().header("Input:");
        let (host, remainder) = s.split_once('/').ok_or_else(|| {
            eyre!("invalid reference: missing host separator '/'").with_section(input_section)
        })?;

        // Find either ':' for tag or '@' for digest.
        // Check for '@' first since digest identifiers also contain ':'.
        let (repository, version) = if let Some((repo, digest)) = remainder.split_once('@') {
            let digest = Digest::from_str(digest).context("parse digest")?;
            (repo, Version::Digest(digest))
        } else if let Some((repo, tag)) = remainder.split_once(':') {
            (repo, Version::Tag(tag.to_string()))
        } else {
            (remainder, Version::latest())
        };

        if host.is_empty() {
            return Err(eyre!("host cannot be empty").with_section(input_section));
        }
        if repository.is_empty() {
            return Err(eyre!("repository cannot be empty").with_section(input_section));
        }

        Ok(Reference {
            host: host.to_string(),
            repository: repository.to_string(),
            version,
        })
    }
}

impl std::fmt::Display for Reference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.host, self.repository)?;
        match &self.version {
            Version::Tag(tag) => write!(f, ":{}", tag),
            Version::Digest(digest) => write!(f, "@{}", digest),
        }
    }
}

/// A descriptor for a specific layer within an OCI container image.
/// This follows the OCI Image Spec's layer descriptor format.
#[derive(Debug, Clone, PartialEq, Eq, Builder)]
pub struct LayerDescriptor {
    /// The content-addressable digest of the layer
    #[builder(into)]
    pub digest: Digest,

    /// The size of the layer in bytes
    pub size: i64,

    /// The media type of the layer
    pub media_type: LayerMediaType,
}

impl From<&LayerDescriptor> for LayerDescriptor {
    fn from(layer: &LayerDescriptor) -> Self {
        layer.clone()
    }
}

impl std::fmt::Display for LayerDescriptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.digest)
    }
}

/// Media types for OCI container image layers.
///
/// Each entry in this enum is a unique media type "base"; some of them then can have flags applied.
/// For example, even though `Foreign` is a valid [`LayerMediaTypeFlag`], [`LayerMediaType::DockerForeign`]
/// is distinct from [`LayerMediaType::Docker`] because it is an entirely different media type.
///
/// Spec reference: https://github.com/opencontainers/image-spec/blob/main/media-types.md
#[derive(Debug, Clone, PartialEq, Eq, AsRefStr, EnumIter)]
pub enum LayerMediaType {
    /// A standard Docker container layer in gzipped tar format.
    ///
    /// These layers contain filesystem changes that make up the container image.
    /// Each layer represents a Dockerfile instruction or equivalent build step.
    #[strum(serialize = "application/vnd.docker.image.rootfs.diff.tar.gzip")]
    Docker,

    /// A Docker container layer that was built for a different architecture or operating system.
    ///
    /// Foreign layers are used in multi-platform images where the same image can contain
    /// layers for different platforms (e.g. linux/amd64 vs linux/arm64).
    #[strum(serialize = "application/vnd.docker.image.rootfs.foreign.diff.tar.gzip")]
    DockerForeign,

    /// A standard OCI container layer.
    #[strum(serialize = "application/vnd.oci.image.layer.v1.tar")]
    Oci(Vec<LayerMediaTypeFlag>),

    /// An OCI container layer that has restrictions on distribution.
    ///
    /// Non-distributable layers typically contain licensed content, proprietary code,
    /// or other material that cannot be freely redistributed.
    /// Registry operators are not required to push or pull these layers.
    /// Instead, the layer data might need to be obtained through other means
    /// (e.g. direct download from a vendor).
    ///
    /// These are officially marked deprecated in the OCI spec, along with the directive
    /// that clients should download the layers as usual:
    /// https://github.com/opencontainers/image-spec/blob/main/layer.md#non-distributable-layers
    #[strum(serialize = "application/vnd.oci.image.layer.nondistributable.v1.tar")]
    OciNonDistributable(Vec<LayerMediaTypeFlag>),
}

impl LayerMediaType {
    /// Overwrite the flags for the media type.
    fn replace_flags(self, flags: Vec<LayerMediaTypeFlag>) -> Self {
        match self {
            LayerMediaType::Oci(_) => LayerMediaType::Oci(flags),
            LayerMediaType::OciNonDistributable(_) => LayerMediaType::OciNonDistributable(flags),
            LayerMediaType::Docker | LayerMediaType::DockerForeign => self,
        }
    }
}

impl FromStr for LayerMediaType {
    type Err = eyre::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (base, flags) = s.split_once('+').unwrap_or((s, ""));
        for media_type in LayerMediaType::iter() {
            if base == media_type.as_ref() {
                return match media_type {
                    // Docker layers don't have flags.
                    LayerMediaType::Docker | LayerMediaType::DockerForeign => Ok(media_type),

                    // OCI layers have flags; handle both bases the same way.
                    mt @ LayerMediaType::Oci(_) | mt @ LayerMediaType::OciNonDistributable(_) => {
                        flags
                            .split('+')
                            .map(LayerMediaTypeFlag::from_str)
                            .try_collect()
                            .map(|flags| mt.replace_flags(flags))
                    }
                };
            }

            // It's always possible for a future media type to be added that has a plus sign;
            // this is a fallback to catch that case.
            if s == media_type.as_ref() {
                return Ok(media_type);
            }
        }
        bail!("unknown media type: {s}");
    }
}

impl std::fmt::Display for LayerMediaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_ref())
    }
}

/// Flags for layer media types.
///
/// Some flags indicate the underlying media should be transformed, while some are informational.
/// This library interprets all flags as "transforming", and informational flags are simply identity transformations.
///
/// When multiple flags apply to a media type, this library applies transforms right-to-left.
/// For example, the hypothetical media type `application/vnd.oci.image.layer.v1.tar+foreign+zstd+gzip`
/// would be read with the following steps:
/// 1. Decompress the layer with gzip.
/// 2. Decompress the layer with zstd.
/// 3. Apply the foreign flag (this is an informational flag, so its transformation is a no-op).
/// 4. The underlying media type is now in effect `application/vnd.oci.image.layer.v1.tar`.
///
/// Note that this library is currently focused on _reading_ images; if you choose to use these
/// flags to _create_ media types make sure you consult the OCI spec for valid combinations.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, AsRefStr, EnumIter)]
pub enum LayerMediaTypeFlag {
    /// Foreign layers are used in multi-platform images where the same image can contain
    /// layers for different platforms (e.g. linux/amd64 vs linux/arm64).
    #[strum(serialize = "foreign")]
    Foreign,

    /// The layer is compressed with zstd.
    #[strum(serialize = "zstd")]
    Zstd,

    /// The layer is compressed with gzip.
    #[strum(serialize = "gzip")]
    Gzip,
}

impl FromStr for LayerMediaTypeFlag {
    type Err = eyre::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::iter()
            .find(|flag| flag.as_ref() == s)
            .ok_or_else(|| eyre!("unknown flag: {s}"))
    }
}

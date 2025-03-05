//! Core library for `circe`, a tool for extracting OCI images.

use bon::Builder;
use color_eyre::{
    eyre::{self, bail, ensure, eyre, Context},
    Result, Section, SectionExt,
};
use derive_more::derive::{Debug, Display, From};
use enum_assoc::Assoc;
use itertools::Itertools;
use serde::{Serialize, Serializer};
use std::{borrow::Cow, ops::Add, path::PathBuf, str::FromStr};
use strum::{AsRefStr, EnumIter, IntoEnumIterator};
use tap::{Pipe, Tap};
use tracing::{debug, warn};

mod docker;
mod ext;
pub mod extract;
pub mod registry;
pub mod transform;

/// Users can set this environment variable to specify the OCI base.
/// If not set, the default is [`OCI_DEFAULT_BASE`].
pub const OCI_BASE_VAR: &str = "OCI_DEFAULT_BASE";

/// Users can set this environment variable to specify the OCI namespace.
/// If not set, the default is [`OCI_DEFAULT_NAMESPACE`].
pub const OCI_NAMESPACE_VAR: &str = "OCI_DEFAULT_NAMESPACE";

/// The default OCI base.
pub const OCI_DEFAULT_BASE: &str = "docker.io";

/// The default OCI namespace.
pub const OCI_DEFAULT_NAMESPACE: &str = "library";

/// The OCI base.
pub fn oci_base() -> String {
    std::env::var(OCI_BASE_VAR).unwrap_or(OCI_DEFAULT_BASE.to_string())
}

/// The OCI namespace.
pub fn oci_namespace() -> String {
    std::env::var(OCI_NAMESPACE_VAR).unwrap_or(OCI_DEFAULT_NAMESPACE.to_string())
}

/// Authentication method for a registry.
#[derive(Debug, Clone, Default, Display)]
pub enum Authentication {
    /// No authentication
    #[default]
    #[display("none")]
    None,

    /// Basic authentication
    #[display("basic:{username}")]
    Basic {
        /// The username
        username: String,

        /// The password
        #[debug(skip)]
        password: String,
    },
}

impl Authentication {
    /// Create an instance for basic authentication
    pub fn basic(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self::Basic {
            username: username.into(),
            password: password.into(),
        }
    }
}

/// Platform represents the platform a container image is built for.
/// This follows the OCI Image Spec's platform definition while also supporting
/// Docker's platform string format (e.g. "linux/amd64").
///
/// ```
/// # use circe_lib::Platform;
/// # use std::str::FromStr;
/// let platform = Platform::from_str("linux/amd64").expect("parse platform");
/// assert_eq!(platform.to_string(), "linux/amd64");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Builder, Serialize)]
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
            ["linux/amd64/v7", "darwin/arm64"]
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
/// let digest = circe_lib::digest!("sha256", "a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4");
/// assert_eq!(digest.algorithm, "sha256");
/// assert_eq!(digest.as_hex(), "a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4");
/// ```
///
/// If algorithm is not provided, it defaults to [`Digest::SHA256`].
/// ```
/// let digest = circe_lib::digest!("a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4");
/// assert_eq!(digest.algorithm, "sha256");
/// assert_eq!(digest.as_hex(), "a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4");
/// ```
///
/// This macro currently assumes that the hash is 32 bytes long.
/// Providing a value of a different length will result in a compile-time error.
/// ```compile_fail
/// let digest = circe_lib::digest!("a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4deadbeef");
/// ```
///
/// You can work around this by providing the size of the hash as a third argument.
/// ```
/// let digest = circe_lib::digest!(circe_lib::Digest::SHA256, "a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4deadbeef", 36);
/// assert_eq!(digest.algorithm, "sha256");
/// assert_eq!(digest.as_hex(), "a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4deadbeef");
/// ```
#[macro_export]
macro_rules! digest {
    ($hex:expr) => {{
        circe_lib::digest!(circe_lib::Digest::SHA256, $hex, 32)
    }};
    ($algorithm:expr, $hex:expr) => {{
        circe_lib::digest!($algorithm, $hex, 32)
    }};
    ($algorithm:expr, $hex:expr, $size:expr) => {{
        const HASH: [u8; $size] = hex_magic::hex!($hex);
        static_assertions::const_assert_ne!(HASH.len(), 0);
        static_assertions::const_assert_ne!($algorithm.len(), 0);
        circe_lib::Digest {
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
/// let digest = circe_lib::Digest::from_str("sha256:a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4").expect("parse digest");
/// assert_eq!(digest.algorithm, "sha256");
/// assert_eq!(digest.as_hex(), "a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[debug("{}", self.to_string())]
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

impl Serialize for Digest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_string().serialize(serializer)
    }
}

/// Version identifier for a container image.
///
/// This can be a named tag or a SHA256 digest.
///
/// ```
/// # use circe_lib::{Version, Digest};
/// # use std::str::FromStr;
/// assert_eq!(Version::latest().to_string(), "latest");
/// assert_eq!(Version::tag("other").to_string(), "other");
///
/// let digest = Digest::from_str("sha256:a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4").expect("parse digest");
/// assert_eq!(Version::digest(digest).to_string(), "sha256:a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Display, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
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
    /// # use circe_lib::Version;
    /// assert_eq!(Version::latest().to_string(), "latest");
    /// ```
    pub fn latest() -> Self {
        Self::Tag(String::from("latest"))
    }

    /// Create a tagged instance.
    ///
    /// ```
    /// # use circe_lib::Version;
    /// assert_eq!(Version::tag("latest").to_string(), "latest");
    /// ```
    pub fn tag(tag: &str) -> Self {
        Self::Tag(tag.to_string())
    }

    /// Create a digest instance.
    ///
    /// ```
    /// # use circe_lib::Version;
    /// let digest = circe_lib::digest!("sha256", "a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4");
    /// let version = Version::digest(digest);
    /// assert_eq!(version.to_string(), "sha256:a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4");
    /// ```
    pub fn digest(digest: Digest) -> Self {
        Self::Digest(digest)
    }
}

/// A parsed container image reference.
#[derive(Debug, Clone, PartialEq, Eq, Builder, Serialize)]
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

impl Reference {
    /// The name of the container without the repository.
    pub fn name(&self) -> &str {
        self.repository
            .split('/')
            .last()
            .unwrap_or(&self.repository)
    }
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
        // Returns an owned string so that we can support multiple name segments.
        fn parse_name(name: &str) -> Result<(String, Version)> {
            if let Some((name, digest)) = name.split_once('@') {
                let digest = Digest::from_str(digest).context("parse digest")?;
                Ok((name.to_string(), Version::Digest(digest)))
            } else if let Some((name, tag)) = name.split_once(':') {
                Ok((name.to_string(), Version::Tag(tag.to_string())))
            } else {
                Ok((name.to_string(), Version::latest()))
            }
        }

        // Docker supports `docker pull ubuntu` and `docker pull library/ubuntu`,
        // both of which are parsed as `docker.io/library/ubuntu`.
        // The below recreates this behavior.
        let base = oci_base();
        let namespace = oci_namespace();
        let parts = s.split('/').collect::<Vec<_>>();
        let (host, namespace, name, version) = match parts.as_slice() {
            // For docker compatibility, `{name}` is parsed as `{base}/{namespace}/{name}`.
            [name] => {
                let (name, version) = parse_name(name)?;
                warn!("expanding '{name}' to '{base}/{namespace}/{name}'; fully specify the reference to avoid this behavior");
                (base, namespace, name, version)
            }

            // Two segments may mean "{namespace}/{name}" or may mean "{base}/{name}".
            // This is a special case for docker compatibility.
            [host, name] if *host == base => {
                let (name, version) = parse_name(name)?;
                warn!("expanding '{host}/{name}' to '{base}/{namespace}/{name}'; fully specify the reference to avoid this behavior");
                (host.to_string(), namespace, name, version)
            }
            [namespace, name] => {
                let (name, version) = parse_name(name)?;
                warn!("expanding '{namespace}/{name}' to '{base}/{namespace}/{name}'; fully specify the reference to avoid this behavior");
                (base, namespace.to_string(), name, version)
            }

            // Some names have multiple segments, e.g. `docker.io/library/ubuntu/foo`.
            // We can't handle multi-segment names in other branches since they conflict with the various shorthands,
            // but handle them here since they're not ambiguous.
            [host, namespace, name @ ..] => {
                let name = name.join("/");
                let (name, version) = parse_name(&name)?;
                (host.to_string(), namespace.to_string(), name, version)
            }
            _ => {
                return eyre!("invalid reference format: {s}")
                    .with_section(|| {
                        [
                            "Provide either a fully qualified OCI reference, or a short form.",
                            "Short forms are in the format `{name}` or `{namespace}/{name}`.",
                            "If you provide a short form, the default registry is `docker.io`.",
                        ]
                        .join("\n")
                        .header("Help:")
                    })
                    .with_section(|| {
                        ["docker.io/library/ubuntu", "library/ubuntu", "ubuntu"]
                            .join("\n")
                            .header("Examples:")
                    })
                    .pipe(Err)
            }
        };

        ensure!(!host.is_empty(), "host cannot be empty: {s}");
        ensure!(!namespace.is_empty(), "namespace cannot be empty: {s}");
        ensure!(!name.is_empty(), "name cannot be empty: {s}");

        Ok(Reference {
            host: host.to_string(),
            repository: format!("{namespace}/{name}"),
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
/// Note: some media types that are fully compatible are handled with [`LayerMediaType::compatibility_matrix`].
///
/// Spec reference: https://github.com/opencontainers/image-spec/blob/main/media-types.md
#[derive(Debug, Clone, PartialEq, Eq, AsRefStr, EnumIter, Assoc)]
pub enum LayerMediaType {
    /// A standard OCI container layer.
    #[strum(serialize = "application/vnd.oci.image.layer.v1.tar")]
    Oci(Vec<LayerMediaTypeFlag>),
}

impl LayerMediaType {
    /// Create the given media type with the given flags.
    fn oci(flags: impl IntoIterator<Item = LayerMediaTypeFlag>) -> Self {
        Self::Oci(flags.into_iter().collect())
    }

    /// Overwrite the flags for the media type.
    fn replace_flags(self, flags: Vec<LayerMediaTypeFlag>) -> Self {
        match self {
            LayerMediaType::Oci(_) => LayerMediaType::Oci(flags),
        }
    }

    /// Parse the media type from the known compatibility matrix.
    ///
    /// Reference: https://github.com/opencontainers/image-spec/blob/main/media-types.md#compatibility-matrix
    /// Note that this is only concerned with _layer_ media types.
    fn compatibility_matrix(s: &str) -> Result<Option<Self>> {
        // Some types are directly convertible.
        match s {
            "application/vnd.docker.image.rootfs.diff.tar.gzip" => {
                return Self::oci([LayerMediaTypeFlag::Gzip]).pipe(Some).pipe(Ok);
            }
            "application/vnd.docker.image.rootfs.foreign.diff.tar.gzip" => {
                return Self::oci([LayerMediaTypeFlag::Gzip, LayerMediaTypeFlag::Foreign])
                    .pipe(Some)
                    .pipe(Ok);
            }
            _ => {}
        }

        // Some need to have parsed flags.
        let (base, flags) = s.split_once('+').unwrap_or((s, ""));
        match base {
            // An OCI container layer that has restrictions on distribution.
            //
            // Non-distributable layers typically contain licensed content, proprietary code,
            // or other material that cannot be freely redistributed.
            // Registry operators are not required to push or pull these layers.
            // Instead, the layer data might need to be obtained through other means
            // (e.g. direct download from a vendor).
            //
            // These are officially marked deprecated in the OCI spec, along with the directive
            // that clients should download the layers as usual:
            // https://github.com/opencontainers/image-spec/blob/main/layer.md#non-distributable-layers
            //
            // For this reason, they're part of the "compatibility matrix" for OCI layers,
            // and are simply translated to the standard OCI layer media type.
            "application/vnd.oci.image.layer.nondistributable.v1.tar" => {
                let flags = LayerMediaTypeFlag::parse_set(flags).context("parse flags")?;
                return Self::Oci(flags).pipe(Some).pipe(Ok);
            }
            _ => {}
        }

        Ok(None)
    }
}

impl FromStr for LayerMediaType {
    type Err = eyre::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(mt) = Self::compatibility_matrix(s)? {
            debug!("translating layer media type from '{s}' to '{mt}' with compatibility matrix");
            return Ok(mt);
        }

        let (base, flags) = s.split_once('+').unwrap_or((s, ""));
        for mt in LayerMediaType::iter() {
            if base == mt.as_ref() {
                return match mt {
                    LayerMediaType::Oci(_) => {
                        let flags = LayerMediaTypeFlag::parse_set(flags)?;
                        Ok(mt.replace_flags(flags))
                    }
                };
            }

            // It's always possible for a future media type to be added that has a plus sign;
            // this is a fallback to catch that case.
            if s == mt.as_ref() {
                return Ok(mt);
            }
        }
        bail!("unknown media type: {s}");
    }
}

impl std::fmt::Display for LayerMediaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_ref())?;
        match self {
            LayerMediaType::Oci(flags) => {
                for flag in flags {
                    write!(f, "+{flag}")?;
                }
            }
        }
        Ok(())
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

impl LayerMediaTypeFlag {
    /// Parse a string into a set of flags, separated by `+` characters.
    fn parse_set(s: &str) -> Result<Vec<Self>> {
        s.split('+').map(Self::from_str).try_collect()
    }
}

impl FromStr for LayerMediaTypeFlag {
    type Err = eyre::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::iter()
            .find(|flag| flag.as_ref() == s)
            .ok_or_else(|| eyre!("unknown flag: {s}"))
    }
}

impl std::fmt::Display for LayerMediaTypeFlag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_ref())
    }
}

/// Trait for filtering.
pub trait FilterMatch<T> {
    /// Report whether the filter matches the given value.
    /// Values that match are included in program operation.
    fn matches(&self, value: T) -> bool;
}

/// A set of filters; if any filter in the set matches, the value is considered matched.
/// As a special case, if no filters are provided, the value is also considered matched.
#[derive(Debug, Clone, From, Default)]
pub struct Filters(Vec<Filter>);

impl Filters {
    /// Create glob filters from the given strings.
    pub fn parse_glob(globs: impl IntoIterator<Item = impl AsRef<str>>) -> Result<Self> {
        globs
            .into_iter()
            .map(|s| Filter::parse_glob(s.as_ref()))
            .collect::<Result<Vec<_>>>()
            .map(Self)
    }

    /// Create regex filters from the given strings.
    pub fn parse_regex(regexes: impl IntoIterator<Item = impl AsRef<str>>) -> Result<Self> {
        regexes
            .into_iter()
            .map(|s| Filter::parse_regex(s.as_ref()))
            .collect::<Result<Vec<_>>>()
            .map(Self)
    }
}

impl Add<Filter> for Filters {
    type Output = Self;

    fn add(mut self, filter: Filter) -> Self {
        self.0.push(filter);
        self
    }
}

impl Add<Filters> for Filters {
    type Output = Filters;

    fn add(mut self, filters: Filters) -> Filters {
        self.0.extend(filters.0);
        self
    }
}

impl<'a, T> FilterMatch<&'a T> for Filters
where
    Filter: FilterMatch<&'a T>,
{
    fn matches(&self, value: &'a T) -> bool {
        self.0.is_empty() || self.0.iter().any(|filter| filter.matches(value))
    }
}

/// Specifies general filtering options.
#[derive(Debug, Clone, From)]
pub enum Filter {
    /// A regular expression to filter
    Regex(Regex),

    /// A glob to filter
    Glob(Glob),
}

impl Filter {
    /// Create a glob filter from the given string.
    pub fn parse_glob(s: &str) -> Result<Self> {
        Glob::from_str(s).map(Self::Glob)
    }

    /// Create a regex filter from the given string.
    pub fn parse_regex(s: &str) -> Result<Self> {
        Regex::from_str(s).map(Self::Regex)
    }
}

impl FilterMatch<String> for Filter {
    fn matches(&self, value: String) -> bool {
        self.matches(&value)
    }
}

impl FilterMatch<&String> for Filter {
    fn matches(&self, value: &String) -> bool {
        self.matches(value.as_str())
    }
}

impl FilterMatch<Cow<'_, str>> for Filter {
    fn matches(&self, value: Cow<'_, str>) -> bool {
        self.matches(value.as_ref())
    }
}

impl FilterMatch<&str> for Filter {
    fn matches(&self, value: &str) -> bool {
        match self {
            Filter::Regex(regex) => regex.matches(value),
            Filter::Glob(glob) => glob.matches(value),
        }
    }
}

/// A regular expression filter.
#[derive(Debug, Clone)]
pub struct Regex(regex::Regex);

impl FilterMatch<&str> for Regex {
    fn matches(&self, value: &str) -> bool {
        self.0
            .is_match(value)
            .tap(|matched| debug!(?value, expr = ?self.0, %matched, "regex: check filter"))
    }
}

impl FromStr for Regex {
    type Err = eyre::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        regex::Regex::new(s)
            .map_err(|e| eyre!("invalid regex: {e}"))
            .map(Self)
    }
}

/// A glob filter.
#[derive(Debug, Clone)]
pub struct Glob(String);

impl FilterMatch<&str> for Glob {
    fn matches(&self, value: &str) -> bool {
        glob_match::glob_match(&self.0, value)
            .tap(|matched| debug!(?value, glob = ?self.0, %matched, "glob: check filter"))
    }
}

impl FromStr for Glob {
    type Err = eyre::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.to_string().pipe(Self).pipe(Ok)
    }
}

/// Get the current home directory for the current user.
///
/// This is a convenience function for `std::env::var("HOME")` or `std::env::var("USERPROFILE")`.
fn homedir() -> Result<PathBuf, std::env::VarError> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
}

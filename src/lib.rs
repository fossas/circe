use bon::Builder;
use color_eyre::{
    eyre::{self, eyre},
    Section, SectionExt,
};
use std::str::FromStr;
use tap::Pipe;

/// Platform represents the platform a container image is built for.
/// This follows the OCI Image Spec's platform definition while also supporting
/// Docker's platform string format (e.g. "linux/amd64").
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

/// A parsed container image reference of the form host/repository:tag or host/repository@sha256:digest
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
    pub fn digest(self, digest: &str) -> ReferenceBuilder<reference_builder::SetVersion<S>>
    where
        S::Version: reference_builder::IsUnset,
    {
        self.version(Version::digest(digest))
    }
}

/// Version identifier for a container image
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Version {
    /// A named tag (e.g. "latest", "1.0.0")
    Tag(String),

    /// A SHA256 digest (e.g. "sha256:123abc...")
    Digest(String),
}

impl Version {
    /// Returns the tag for "latest".
    pub fn latest() -> Self {
        Self::Tag(String::from("latest"))
    }

    /// Create a tagged instance.
    pub fn tag(tag: &str) -> Self {
        Self::Tag(tag.to_string())
    }

    /// Create a digest instance.
    pub fn digest(digest: &str) -> Self {
        Self::Digest(digest.to_string())
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
            (repo, Version::Digest(digest.to_string()))
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

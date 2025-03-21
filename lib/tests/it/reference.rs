use circe_lib::{Digest, Reference};
use proptest::prelude::*;
use simple_test_case::test_case;

#[test_case("docker.io/library/ubuntu:latest", Reference::builder().host("docker.io").namespace("library").name("ubuntu").tag("latest").build(); "docker.io/library/ubuntu:latest")]
#[test_case("ghcr.io/user/repo@sha256:123abc", Reference::builder().host("ghcr.io").namespace("user").name("repo").digest(circe_lib::digest!("sha256", "123abc", 3)).build(); "ghcr.io/user/repo@sha256:123abc")]
#[test_case("docker.io/library/ubuntu", Reference::builder().host("docker.io").namespace("library").name("ubuntu").build(); "docker.io/library/ubuntu")]
#[test]
fn parse(input: &str, expected: Reference) {
    let reference = input.parse::<Reference>().unwrap();
    pretty_assertions::assert_eq!(reference, expected);
}

#[test_case(Reference::builder().host("docker.io").namespace("library").name("ubuntu").tag("latest").build(), "docker.io/library/ubuntu:latest"; "docker.io/library/ubuntu:latest")]
#[test_case(Reference::builder().host("ghcr.io").namespace("user").name("repo").digest(circe_lib::digest!("sha256", "123abc", 3)).build(), "ghcr.io/user/repo@sha256:123abc"; "ghcr.io/user/repo@sha256:123abc")]
#[test_case(Reference::builder().host("ghcr.io").namespace("fossas").name("project/app").tag("sha-e01ce6b").build(), "ghcr.io/fossas/project/app:sha-e01ce6b"; "ghcr.io/fossas/project/app:sha-e01ce6b")]
#[test_case(Reference::builder().host("docker.io").namespace("library").name("ubuntu").build(), "docker.io/library/ubuntu:latest"; "docker.io/library/ubuntu")]
#[test]
fn display(reference: Reference, expected: &str) {
    pretty_assertions::assert_eq!(reference.to_string(), expected);
}

#[test_case("ubuntu", "docker.io/library/ubuntu:latest"; "ubuntu")]
#[test_case("ubuntu:14.04", "docker.io/library/ubuntu:14.04"; "ubuntu:14.04")]
#[test_case("ubuntu@sha256:123abc", "docker.io/library/ubuntu@sha256:123abc"; "ubuntu@sha256:123abc")]
#[test_case("library/ubuntu", "docker.io/library/ubuntu:latest"; "library/ubuntu")]
#[test_case("contribsys/faktory", "docker.io/contribsys/faktory:latest"; "contribsys/faktory")]
#[test_case("contribsys/faktory:1.0.0", "docker.io/contribsys/faktory:1.0.0"; "contribsys/faktory:1.0.0")]
#[test_case("library/ubuntu:14.04", "docker.io/library/ubuntu:14.04"; "library/ubuntu:14.04")]
#[test_case("library/ubuntu@sha256:123abc", "docker.io/library/ubuntu@sha256:123abc"; "library/ubuntu@sha256:123abc")]
#[test_case("docker.io/library/ubuntu:14.04", "docker.io/library/ubuntu:14.04"; "docker.io/library/ubuntu:14.04")]
#[test_case("docker.io/library/ubuntu@sha256:123abc", "docker.io/library/ubuntu@sha256:123abc"; "docker.io/library/ubuntu@sha256:123abc")]
#[test_case("host.dev/somecorp/someproject/someimage", "host.dev/somecorp/someproject/someimage:latest"; "host.dev/somecorp/someproject/someimage")]
#[test_case("host.dev/somecorp/someproject/someimage:1.0.0", "host.dev/somecorp/someproject/someimage:1.0.0"; "host.dev/somecorp/someproject/someimage:1.0.0")]
#[test_case("host.dev/somecorp/someproject/someimage@sha256:123abc", "host.dev/somecorp/someproject/someimage@sha256:123abc"; "host.dev/somecorp/someproject/someimage@sha256:123abc")]
#[test]
#[cfg_attr(
    feature = "test-custom-namespace",
    ignore = "ignoring standard namespace tests with 'test-custom-namespace' feature"
)]
fn docker_like(input: &str, expected: &str) {
    let reference = input.parse::<Reference>().unwrap();
    pretty_assertions::assert_eq!(reference.to_string(), expected);
}

#[test_case("ubuntu", "host.dev/somecorp/someproject/ubuntu:latest"; "ubuntu")]
#[test_case("ubuntu:14.04", "host.dev/somecorp/someproject/ubuntu:14.04"; "ubuntu:14.04")]
#[test_case("ubuntu@sha256:123abc", "host.dev/somecorp/someproject/ubuntu@sha256:123abc"; "ubuntu@sha256:123abc")]
#[test_case("library/ubuntu", "host.dev/library/ubuntu:latest"; "library/ubuntu")]
#[test_case("contribsys/faktory", "host.dev/contribsys/faktory:latest"; "contribsys/faktory")]
#[test_case("contribsys/faktory:1.0.0", "host.dev/contribsys/faktory:1.0.0"; "contribsys/faktory:1.0.0")]
#[test_case("library/ubuntu:14.04", "host.dev/library/ubuntu:14.04"; "library/ubuntu:14.04")]
#[test_case("library/ubuntu@sha256:123abc", "host.dev/library/ubuntu@sha256:123abc"; "library/ubuntu@sha256:123abc")]
#[test_case("docker.io/library/ubuntu:14.04", "docker.io/library/ubuntu:14.04"; "docker.io/library/ubuntu:14.04")]
#[test_case("docker.io/library/ubuntu@sha256:123abc", "docker.io/library/ubuntu@sha256:123abc"; "docker.io/library/ubuntu@sha256:123abc")]
#[test_case("host.dev/somecorp/someproject/someimage", "host.dev/somecorp/someproject/someimage:latest"; "host.dev/somecorp/someproject/someimage")]
#[test_case("host.dev/somecorp/someproject/someimage:1.0.0", "host.dev/somecorp/someproject/someimage:1.0.0"; "host.dev/somecorp/someproject/someimage:1.0.0")]
#[test_case("host.dev/somecorp/someproject/someimage@sha256:123abc", "host.dev/somecorp/someproject/someimage@sha256:123abc"; "host.dev/somecorp/someproject/someimage@sha256:123abc")]
#[test]
#[cfg_attr(
    not(feature = "test-custom-namespace"),
    ignore = "ignoring custom namespace tests without 'test-custom-namespace' feature"
)]
fn docker_like_custom_base_namespace(input: &str, expected: &str) {
    // The test cases above assume these values;
    // if something else is provided we need to give the correct error message.
    const REQUIRED_BASE: &str = "host.dev";
    const REQUIRED_NAMESPACE: &str = "somecorp/someproject";
    let base = std::env::var(circe_lib::OCI_BASE_VAR)
        .unwrap_or_else(|_| panic!("'{}' must be set", circe_lib::OCI_BASE_VAR));
    let ns = std::env::var(circe_lib::OCI_NAMESPACE_VAR)
        .unwrap_or_else(|_| panic!("'{}' must be set", circe_lib::OCI_NAMESPACE_VAR));
    pretty_assertions::assert_eq!(
        base,
        REQUIRED_BASE,
        "test must be run with {}={}",
        circe_lib::OCI_BASE_VAR,
        REQUIRED_BASE,
    );
    pretty_assertions::assert_eq!(
        ns,
        REQUIRED_NAMESPACE,
        "test must be run with {}={}",
        circe_lib::OCI_NAMESPACE_VAR,
        REQUIRED_NAMESPACE,
    );

    // Now that we're sure the correct variables are set,
    // test the parser.
    let reference = input.parse::<Reference>().unwrap();
    pretty_assertions::assert_eq!(reference.to_string(), expected);
}

#[test_case("/repo:tag"; "/repo:tag")]
#[test_case("host/:tag"; "host/tag")]
#[test_case("host/"; "host/")]
#[test]
fn invalid_references(input: &str) {
    let _ = input.parse::<Reference>().expect_err("must error");
}

// Strategy to generate valid host names
fn host_strategy() -> impl Strategy<Value = String> {
    // Generate reasonable hostnames like docker.io, ghcr.io, etc
    "[a-z][a-z0-9-]*(\\.[a-z0-9-]+)*\\.[a-z]{2,}"
        .prop_filter("Valid hostname required", |s| !s.contains(".."))
}

// Strategy to generate valid namespaces
fn namespace_strategy() -> impl Strategy<Value = String> {
    // Generate repository namespaces like library, user
    "[a-z][a-z0-9-]*"
}

// Strategy to generate valid names
fn name_strategy() -> impl Strategy<Value = String> {
    // Generate repository names like ubuntu, project
    "[a-z][a-z0-9-]*"
}

// Strategy to generate valid repositories
fn repository_strategy() -> impl Strategy<Value = String> {
    // Generate repository paths like library/ubuntu, user/project
    (namespace_strategy(), name_strategy())
        .prop_map(|(namespace, name)| format!("{namespace}/{name}"))
}

// Strategy to generate valid tags
fn tag_strategy() -> impl Strategy<Value = String> {
    // Generate reasonable tag names like latest, v1.0.0, etc
    "[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}"
}

// Strategy to generate valid SHA256 digests
fn digest_strategy() -> impl Strategy<Value = String> {
    // Generate SHA256 digest format strings
    "sha256:[a-f0-9]{64}"
}

// Strategy to generate complete Reference values
fn reference_strategy() -> impl Strategy<Value = Reference> {
    (
        host_strategy(),
        namespace_strategy(),
        name_strategy(),
        prop_oneof![
            tag_strategy().prop_map(circe_lib::Version::Tag),
            digest_strategy().prop_map(|digest| {
                circe_lib::Version::Digest(digest.parse::<Digest>().expect("parse digest"))
            })
        ],
    )
        .prop_map(|(host, namespace, name, version)| Reference {
            host,
            namespace,
            name,
            version,
        })
}

proptest! {
    // Property: parsing a formatted reference should yield the original reference
    #[test]
    fn roundtrip_parse_format(reference in reference_strategy()) {
        let formatted = reference.to_string();
        let parsed = formatted.parse::<Reference>().unwrap();
        prop_assert_eq!(reference, parsed);
    }

    // Property: parsing should reject empty hosts
    #[test]
    fn rejects_empty_host(repository in repository_strategy(), version in tag_strategy()) {
        let input = format!("/{repository}:{version}");
        prop_assert!(input.parse::<Reference>().is_err());
    }

    // Property: parsing should reject empty repositories
    #[test]
    fn rejects_empty_repository(host in host_strategy(), version in tag_strategy()) {
        let input = format!("{host}/:{version}");
        prop_assert!(input.parse::<Reference>().is_err());
    }

    // Property: default version should be "latest" when no tag/digest specified
    #[test]
    fn default_version_is_latest(host in host_strategy(), repository in repository_strategy()) {
        let input = format!("{host}/{repository}");
        let reference = input.parse::<Reference>().unwrap();
        prop_assert!(matches!(reference.version, circe_lib::Version::Tag(tag) if tag == "latest"));
    }
}

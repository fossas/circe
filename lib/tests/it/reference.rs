use circe_lib::{Digest, Reference};
use proptest::prelude::*;
use simple_test_case::test_case;

#[test_case("docker.io/library/ubuntu:latest", Reference::builder().host("docker.io").repository("library/ubuntu").tag("latest").build(); "docker.io/library/ubuntu:latest")]
#[test_case("ghcr.io/user/repo@sha256:123abc", Reference::builder().host("ghcr.io").repository("user/repo").digest(circe_lib::digest!("sha256", "123abc", 3)).build(); "ghcr.io/user/repo@sha256:123abc")]
#[test_case("docker.io/library/ubuntu", Reference::builder().host("docker.io").repository("library/ubuntu").build(); "docker.io/library/ubuntu")]
#[test]
fn parse(input: &str, expected: Reference) {
    let reference = input.parse::<Reference>().unwrap();
    pretty_assertions::assert_eq!(reference, expected);
}

#[test_case(Reference::builder().host("docker.io").repository("library/ubuntu").tag("latest").build(), "docker.io/library/ubuntu:latest"; "docker.io/library/ubuntu:latest")]
#[test_case(Reference::builder().host("ghcr.io").repository("user/repo").digest(circe_lib::digest!("sha256", "123abc", 3)).build(), "ghcr.io/user/repo@sha256:123abc"; "ghcr.io/user/repo@sha256:123abc")]
#[test_case(Reference::builder().host("ghcr.io").repository("fossas/project/app").tag("sha-e01ce6b").build(), "ghcr.io/fossas/project/app:sha-e01ce6b"; "ghcr.io/fossas/project/app:sha-e01ce6b")]
#[test_case(Reference::builder().host("docker.io").repository("library/ubuntu").build(), "docker.io/library/ubuntu:latest"; "docker.io/library/ubuntu")]
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
fn docker_like(input: &str, expected: &str) {
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

// Strategy to generate valid repository names
fn repository_strategy() -> impl Strategy<Value = String> {
    // Generate repository paths like library/ubuntu, user/project
    "[a-z][a-z0-9-]*/[a-z][a-z0-9-]*"
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
        repository_strategy(),
        prop_oneof![
            tag_strategy().prop_map(circe_lib::Version::Tag),
            digest_strategy().prop_map(|digest| {
                circe_lib::Version::Digest(digest.parse::<Digest>().expect("parse digest"))
            })
        ],
    )
        .prop_map(|(host, repository, version)| Reference {
            host,
            repository,
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

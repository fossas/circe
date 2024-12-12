use circe_lib::Platform;
use simple_test_case::test_case;

#[test_case("linux/amd64", Platform::linux_amd64(); "linux/amd64")]
#[test_case("linux/arm64/v8", Platform::linux_arm64().with_variant("v8"); "linux/arm64/v8")]
#[test_case("darwin/arm64", Platform::macos_arm64(); "darwin/arm64")]
#[test_case("darwin/amd64", Platform::macos_amd64(); "darwin/amd64")]
#[test_case("windows/amd64", Platform::windows_amd64(); "windows/amd64")]
#[test]
fn parse(input: &str, expected: Platform) {
    let platform = input.parse::<Platform>().unwrap();
    pretty_assertions::assert_eq!(platform, expected);
}

#[test_case("linux"; "linux")]
#[test_case("linux/"; "linux/")]
#[test_case("/arm64/v8"; "/arm64/v8")]
#[test_case("/amd64"; "/amd64")]
#[test_case("linux/amd64/v8/extra"; "linux/amd64/v8/extra")]
#[test]
fn parse_invalid(input: &str) {
    let parsed = input.parse::<Platform>();
    let _ = parsed.expect_err("must error");
}

#[test_case(Platform::linux_amd64(), "linux/amd64"; "linux/amd64")]
#[test_case(Platform::linux_arm64(), "linux/arm64"; "linux/arm64")]
#[test_case(Platform::linux_arm64().with_variant("v8"), "linux/arm64/v8"; "linux/arm64/v8")]
#[test_case(Platform::windows_amd64(), "windows/amd64"; "windows/amd64")]
#[test_case(Platform::macos_arm64(), "darwin/arm64"; "darwin/arm64")]
#[test_case(Platform::macos_amd64(), "darwin/amd64"; "darwin/amd64")]
#[test]
fn display(platform: Platform, expected: &str) {
    pretty_assertions::assert_eq!(platform.to_string(), expected);
}

#[test]
fn constructors() {
    assert_eq!(Platform::linux_amd64().to_string(), "linux/amd64");
    assert_eq!(Platform::linux_arm64().to_string(), "linux/arm64");
    assert_eq!(Platform::windows_amd64().to_string(), "windows/amd64");
    assert_eq!(Platform::macos_arm64().to_string(), "darwin/arm64");
    assert_eq!(Platform::macos_amd64().to_string(), "darwin/amd64");
}

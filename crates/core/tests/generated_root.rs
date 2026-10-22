//! Generated root resolution, admission, marker layout and fence layout, and marker parsing. The
//! golden rows come from `golden/generated-root.json`.

use jobniac_core::env::Environment;
use jobniac_core::generated_root::{
    RootPlatform, admit_root_path, asset_file_name, fence_bytes, fence_name,
    linux_capability_bytes, macos_capability_bytes, marker_bytes, parse_marker,
    resolve_default_generated_root, windows_capability_bytes,
};
use serde_json::Value;
use uuid::Uuid;

const GOLDEN: &str = include_str!("golden/generated-root.json");

fn golden() -> Value {
    serde_json::from_str(GOLDEN).expect("golden JSON parses")
}

fn environment(pairs: &Value) -> Environment {
    let map = pairs.as_object().expect("environment object");
    Environment::from_pairs(map.iter().map(|(key, value)| {
        (
            key.clone(),
            value.as_str().expect("string value").to_owned(),
        )
    }))
}

fn decode_hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).expect("hex byte"))
        .collect()
}

#[test]
fn default_root_matches_the_golden_vectors() {
    let document = golden();
    let rows = document["resolve"].as_array().expect("resolve rows");
    assert!(rows.len() > 100, "the golden file has a full case matrix");
    for row in rows {
        let platform = row["platform"].as_str().expect("platform");
        let env = environment(&row["environment"]);
        let expected = row["expected"].as_str().map(str::to_owned);
        let got = resolve_default_generated_root(&env, RootPlatform::from_platform_name(platform));
        assert_eq!(
            got, expected,
            "platform {platform}, environment {}",
            row["environment"]
        );
    }
}

#[test]
fn windows_default_refuses_unc_device_and_drive_relative_bases() {
    // UNC, device and drive-relative bases are refused.
    for base in [
        r"\\server\share\profile",
        r"\\?\C:\Users\u\AppData\Local",
        r"\\.\C:\Users\u",
        r"C:relative\profile",
    ] {
        let env = Environment::from_pairs([("LOCALAPPDATA", base)]);
        assert_eq!(
            resolve_default_generated_root(&env, RootPlatform::Windows),
            None,
            "base {base}"
        );
    }
}

#[test]
fn malformed_absolute_xdg_never_falls_back_to_home() {
    let env = Environment::from_pairs([("XDG_DATA_HOME", "/a/../b"), ("HOME", "/h")]);
    assert_eq!(
        resolve_default_generated_root(&env, RootPlatform::Other),
        None
    );
    let invalid = Environment::from_pairs([("HOME", "/h")]).with_invalid("XDG_DATA_HOME");
    assert_eq!(
        resolve_default_generated_root(&invalid, RootPlatform::Other),
        None
    );
}

#[test]
fn platform_names_classify_like_sys_platform() {
    assert_eq!(
        RootPlatform::from_platform_name("win32"),
        RootPlatform::Windows
    );
    assert_eq!(
        RootPlatform::from_platform_name("darwin"),
        RootPlatform::MacOs
    );
    assert_eq!(
        RootPlatform::from_platform_name("linux"),
        RootPlatform::Other
    );
    assert_eq!(
        RootPlatform::from_platform_name("freebsd14"),
        RootPlatform::Other
    );
}

#[test]
fn explicit_roots_are_admitted_exactly_or_refused() {
    assert_eq!(
        admit_root_path(r"C:\Generated\root", RootPlatform::Windows).as_deref(),
        Some(r"C:\Generated\root")
    );
    assert_eq!(
        admit_root_path("/srv/generated", RootPlatform::Other).as_deref(),
        Some("/srv/generated")
    );
    for bad in [
        "", "relative", " /x", "/x ", "/a/../b", "/a/./b", "/a\u{1}b", "/a\0b",
    ] {
        assert_eq!(
            admit_root_path(bad, RootPlatform::Other),
            None,
            "value {bad:?}"
        );
    }
    for bad in [r"C:\a\..\b", r"\\server\share", "C:foo", "/unix/style"] {
        assert_eq!(
            admit_root_path(bad, RootPlatform::Windows),
            None,
            "value {bad:?}"
        );
    }
    let long = format!("/{}", "x".repeat(4096));
    assert_eq!(admit_root_path(&long, RootPlatform::Other), None);
}

#[test]
fn marker_parsing_matches_the_golden_vectors() {
    let document = golden();
    let rows = document["marker_parse"].as_array().expect("marker rows");
    for row in rows {
        let content = decode_hex(row["content_hex"].as_str().expect("hex"));
        let expected = row["expected"]
            .as_str()
            .map(|text| Uuid::parse_str(text).expect("uuid"));
        assert_eq!(parse_marker(&content), expected, "content {content:?}");
    }
}

#[test]
fn marker_and_fence_layouts_match_the_golden_vectors() {
    let document = golden();
    let root: Uuid = "6f1c2a4e-3b5d-4e7f-8a9b-0c1d2e3f4a5b"
        .parse()
        .expect("uuid");
    let asset: Uuid = "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d"
        .parse()
        .expect("uuid");
    assert_eq!(
        String::from_utf8(marker_bytes(root)).expect("ascii"),
        document["marker_bytes"].as_str().expect("marker")
    );
    assert_eq!(
        fence_name(asset),
        document["fence_name"].as_str().expect("fence name")
    );
    assert_eq!(
        String::from_utf8(fence_bytes(root, asset, &"ab".repeat(32), 4096)).expect("ascii"),
        document["fence_bytes"].as_str().expect("fence bytes")
    );
    assert_eq!(asset_file_name(asset), format!("{asset}.pdf"));
}

#[test]
fn capability_lines_follow_the_three_platform_layouts() {
    let root: Uuid = "6f1c2a4e-3b5d-4e7f-8a9b-0c1d2e3f4a5b"
        .parse()
        .expect("uuid");
    assert_eq!(
        String::from_utf8(linux_capability_bytes(root, 2049, 4242)).expect("ascii"),
        "jobniac-generated-capability-v1\nlinux-anonymous-proc-link-fsync\n\
         6f1c2a4e-3b5d-4e7f-8a9b-0c1d2e3f4a5b\n2049\n4242\n"
    );
    assert_eq!(
        String::from_utf8(windows_capability_bytes(root, 1234, 99)).expect("ascii"),
        "jobniac-generated-capability-v1\nwindows-createnew-rename-noreplace\n\
         6f1c2a4e-3b5d-4e7f-8a9b-0c1d2e3f4a5b\n1234\n99\n"
    );
    assert_eq!(
        String::from_utf8(macos_capability_bytes(root, 16_777_232, 31_337)).expect("ascii"),
        "jobniac-generated-capability-v1\nmacos-tempfile-rename-excl-fsync\n\
         6f1c2a4e-3b5d-4e7f-8a9b-0c1d2e3f4a5b\n16777232\n31337\n"
    );
}

#[test]
fn marker_must_be_exact_canonical_and_bounded() {
    let good = b"jobniac-generated-root-v1\n00000000-0000-4000-8000-000000000001\n";
    assert!(parse_marker(good).is_some());
    let upper = b"jobniac-generated-root-v1\n00000000-0000-4000-8000-00000000000A\n";
    assert_eq!(parse_marker(upper), None);
    let braced = b"jobniac-generated-root-v1\n{00000000-0000-4000-8000-000000000001}\n";
    assert_eq!(parse_marker(braced), None);
    let mut oversized = good.to_vec();
    oversized.extend(std::iter::repeat_n(b' ', 128));
    assert_eq!(parse_marker(&oversized), None);
    assert_eq!(parse_marker(&[0xC3, 0xA9]), None);
}

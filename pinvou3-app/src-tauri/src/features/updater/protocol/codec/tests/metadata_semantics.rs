#[allow(dead_code)]
#[path = "support/download_fixture.rs"]
mod download_fixture;

use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use pinvou_upgrade_codec::{
    Document, InputKind, MetadataSemanticsError, RootWindowMode,
    assert_supplied_metadata_semantics, decode_canonical_envelope,
};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
const DAY: u64 = 86400000;
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(7)
        .unwrap()
        .to_owned()
}
fn fixtures() -> Vec<Value> {
    let mut vectors = serde_json::from_slice::<Value>(
        &std::fs::read(root().join("specs/upgrade/v1/vectors/crypto/envelopes.json")).unwrap(),
    )
    .unwrap()["vectors"]
        .as_array()
        .unwrap()
        .clone();
    download_fixture::replace_legacy_download(
        &mut vectors,
        &root().join("specs/upgrade/v1/reference/canonical-json.mjs"),
    );
    vectors
}
fn original(role: &str) -> (Value, u64) {
    let vector = fixtures().into_iter().find(|v| v["name"] == role).unwrap();
    (
        serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(vector["envelopeBase64Url"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap(),
        vector["now"].as_u64().unwrap(),
    )
}
fn wire(value: &Value) -> Vec<u8> {
    Document::parse(&serde_json::to_vec(value).unwrap(), InputKind::Metadata)
        .unwrap()
        .canonical_bytes()
        .unwrap()
}
fn check(value: &Value, now: &[u8], mode: RootWindowMode) -> Result<(), MetadataSemanticsError> {
    let envelope = decode_canonical_envelope(&wire(value), InputKind::Metadata)
        .unwrap()
        .check_full_schema()
        .unwrap();
    let now = Document::parse(now, InputKind::Metadata)
        .map_err(MetadataSemanticsError::Json)?
        .protocol_time_shape()?;
    assert_supplied_metadata_semantics(&envelope, &now, mode)
}
fn duration(role: &str) -> u64 {
    match role {
        "root" => 365 * DAY,
        "timestamp" => DAY,
        "snapshot" => 7 * DAY,
        "target" => 30 * DAY,
        "release" => 90 * DAY,
        _ => 0,
    }
}
#[test]
fn metadata_windows_keep_half_open_maximums_and_root_expiry_option_without_time_authority() {
    for role in ["root", "timestamp", "snapshot", "target", "release"] {
        let (mut value, _) = original(role);
        value["signed"]["issuedAt"] = json!(0);
        let maximum = duration(role);
        value["signed"]["expiresAt"] = json!(maximum);
        assert_eq!(check(&value, b"0", RootWindowMode::Current), Ok(()));
        assert_eq!(
            check(
                &value,
                (maximum - 1).to_string().as_bytes(),
                RootWindowMode::Current
            ),
            Ok(())
        );
        assert_eq!(
            check(
                &value,
                maximum.to_string().as_bytes(),
                RootWindowMode::Current
            ),
            Err(MetadataSemanticsError::ClosedWindow)
        );
        let allowed = check(
            &value,
            maximum.to_string().as_bytes(),
            RootWindowMode::AllowExpired,
        );
        assert_eq!(
            allowed,
            if role == "root" {
                Ok(())
            } else {
                Err(MetadataSemanticsError::ClosedWindow)
            }
        );
        value["signed"]["expiresAt"] = json!(maximum + 1);
        assert_eq!(
            check(&value, b"0", RootWindowMode::AllowExpired),
            Err(MetadataSemanticsError::InvalidWindow)
        );
        value["signed"]["issuedAt"] = json!(2);
        value["signed"]["expiresAt"] = json!(1);
        assert_eq!(
            check(&value, b"0", RootWindowMode::Current),
            Err(MetadataSemanticsError::InvalidWindow)
        );
    }
    let time = Document::parse(b"0", InputKind::Metadata)
        .unwrap()
        .protocol_time_shape()
        .unwrap();
    for vector in fixtures().into_iter().filter(|v| {
        !matches!(
            v["name"].as_str(),
            Some("root" | "timestamp" | "snapshot" | "target" | "release" | "package")
        )
    }) {
        let raw = URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap();
        let envelope = decode_canonical_envelope(&raw, InputKind::Credential)
            .unwrap()
            .check_full_schema()
            .unwrap();
        assert_eq!(
            assert_supplied_metadata_semantics(&envelope, &time, RootWindowMode::Current),
            Err(MetadataSemanticsError::UnsupportedRole)
        );
    }
    let (package, _) = original("package");
    for now in [b"0".as_slice(), b"-0", b"9007199254740991"] {
        assert_eq!(check(&package, now, RootWindowMode::Current), Ok(()));
    }
    for now in [
        b"null".as_slice(),
        b"true",
        b"\"1\"",
        b"-1",
        b"1.5",
        b"9007199254740992",
    ] {
        assert_eq!(
            check(&package, now, RootWindowMode::Current),
            Err(MetadataSemanticsError::InvalidTime)
        );
    }
}
#[test]
fn metadata_semantics_preserve_nested_expired_refs_and_exact_snapshot_identity_rules() {
    let (mut root_value, now) = original("root");
    root_value["signed"]["keys"][0]["keyId"] = json!("0".repeat(64));
    root_value["signed"]["roles"][0]["threshold"] = json!(1);
    // Metadata semantics deliberately do not duplicate validateTrustedRoot.
    assert_eq!(
        check(
            &root_value,
            now.to_string().as_bytes(),
            RootWindowMode::Current
        ),
        Ok(())
    );
    let (mut snapshot, now) = original("snapshot");
    snapshot["signed"]["entries"][0]["expiresAt"] = json!(0);
    assert_eq!(
        check(
            &snapshot,
            now.to_string().as_bytes(),
            RootWindowMode::Current
        ),
        Ok(())
    );
    let mut duplicate = snapshot["signed"]["entries"][0].clone();
    duplicate["version"] = json!(2);
    duplicate["envelopeSha256"] = json!("0".repeat(64));
    snapshot["signed"]["entries"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    assert_eq!(
        check(
            &snapshot,
            now.to_string().as_bytes(),
            RootWindowMode::Current
        ),
        Err(MetadataSemanticsError::DuplicateReference)
    );
    let (mut snapshot, now) = original("snapshot");
    let mut duplicate = snapshot["signed"]["entries"][2].clone();
    duplicate["scope"]["targetKey"] = json!("windows-x86_64-nsis-perMachine");
    snapshot["signed"]["entries"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    assert_eq!(
        check(
            &snapshot,
            now.to_string().as_bytes(),
            RootWindowMode::Current
        ),
        Err(MetadataSemanticsError::DuplicateReference)
    );
}
#[test]
fn metadata_relationships_bind_full_helper_publisher_and_production_package_constraints() {
    let (mut package, now) = original("package");
    package["signed"]["helpers"][0]["nativeIdentity"]["publisher"] = json!("different");
    assert_eq!(
        check(
            &package,
            now.to_string().as_bytes(),
            RootWindowMode::Current
        ),
        Err(MetadataSemanticsError::HelperBinding)
    );
    let (mut package, now) = original("package");
    package["signed"]["incrementalPackages"] = json!([{"packageId":"delta","size":1,"sha256":"0".repeat(64),"baseVersion":"1.0.0","baseArtifactSha256":"0".repeat(64),
        "resultArtifactSize":1,"resultArtifactSha256":"0".repeat(64),"algorithm":"delta","algorithmVersion":"1","format":"delta"}]);
    assert_eq!(
        check(
            &package,
            now.to_string().as_bytes(),
            RootWindowMode::Current
        ),
        Err(MetadataSemanticsError::IncrementalProduction)
    );
    let (mut release, now) = original("release");
    release["signed"]["targets"][0]["migrationMode"] = json!("irreversible");
    release["signed"]["targets"][0]["backupPolicy"] = json!("notRequired");
    assert_eq!(
        check(
            &release,
            now.to_string().as_bytes(),
            RootWindowMode::Current
        ),
        Err(MetadataSemanticsError::BackupPolicy)
    );
    release["signed"]["targets"][0]["backupPolicy"] = json!("required");
    assert_eq!(
        check(
            &release,
            now.to_string().as_bytes(),
            RootWindowMode::Current
        ),
        Ok(())
    );
    release["signed"]["targets"][0]["migrationMode"] = json!("none");
    assert_eq!(
        check(
            &release,
            now.to_string().as_bytes(),
            RootWindowMode::Current
        ),
        Ok(())
    );
}
fn relationship_variants(role: &str, original: &Value) -> Vec<Value> {
    let mut values = vec![original.clone()];
    let mut push = |pointer: &str, replacement: Value| {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement;
        values.push(changed);
    };
    match role {
        "root" => {
            push("/signed/roles/0/threshold", json!(1));
            push("/signed/keys/0/keyId", json!("0".repeat(64)));
        }
        "timestamp" => {
            for reference in ["root", "snapshot", "package"] {
                push("/signed/snapshot/role", json!(reference));
            }
        }
        "snapshot" => {
            for reference in ["root", "timestamp", "target", "release", "package"] {
                push("/signed/entries/0/role", json!(reference));
            }
            for key in ["version", "expiresAt"] {
                push(&format!("/signed/entries/0/{key}"), Value::Null);
            }
            push("/signed/entries/0/scope", json!({}));
            push(
                "/signed/entries/0/scope",
                json!({"channel":"custom","targetKey":"custom"}),
            );
            let mut entries = original["signed"]["entries"].clone();
            let mut target = entries[0].clone();
            target["version"] = json!(2);
            target["envelopeSha256"] = json!("0".repeat(64));
            entries.as_array_mut().unwrap().push(target);
            push("/signed/entries", entries);
            let mut entries = original["signed"]["entries"].clone();
            let mut package = entries[2].clone();
            package["scope"]["targetKey"] = json!("windows-x86_64-nsis-perMachine");
            entries.as_array_mut().unwrap().push(package);
            push("/signed/entries", entries);
        }
        "target" => {
            for value in [
                json!(["arm64"]),
                json!(["x86_64"]),
                json!(["x86_64", "arm64"]),
                json!(["arm64", "x86_64"]),
            ] {
                push("/signed/hostArchitectures", value);
            }
            for target in [
                "windows-x86_64-nsis-perMachine",
                "linux-x86_64-deb-perMachine",
                "linux-arm64-deb-perMachine",
                "macos-universal-dmg-perMachine",
            ] {
                push("/signed/scope/targetKey", json!(target));
            }
            for reference in ["root", "timestamp", "package", "release"] {
                push("/signed/baselineRelease/role", json!(reference));
            }
            push("/signed/baselineRelease/expiresAt", json!(0));
        }
        "release" => {
            for value in ["none", "backwardCompatible", "irreversible"] {
                push("/signed/targets/0/migrationMode", json!(value));
            }
            for value in ["notRequired", "required"] {
                push("/signed/targets/0/backupPolicy", json!(value));
            }
            push("/signed/targets/0/packageManifest/role", json!("release"));
            push(
                "/signed/targets/0/packageManifest/scope/targetKey",
                json!("windows-x86_64-nsis-perMachine"),
            );
            let mut targets = original["signed"]["targets"].clone();
            let mut duplicate = targets[0].clone();
            duplicate["backupPolicy"] = json!("required");
            targets.as_array_mut().unwrap().push(duplicate);
            push("/signed/targets", targets);
        }
        "package" => {
            push("/signed/executionHelperId", json!("missing"));
            for field in ["publisher", "identity", "sha256", "size", "format"] {
                let replacement = match field {
                    "sha256" => json!("0".repeat(64)),
                    "size" => json!(1),
                    "format" => json!("launcher"),
                    _ => json!("different"),
                };
                push(
                    &format!("/signed/helpers/0/nativeIdentity/{field}"),
                    replacement,
                );
            }
            let mut helpers = original["signed"]["helpers"].clone();
            let mut duplicate = helpers[0].clone();
            duplicate["nativeIdentity"]["publisher"] = json!("different");
            helpers.as_array_mut().unwrap().push(duplicate);
            push("/signed/helpers", helpers);
            push(
                "/signed/helpers/0/targetKey",
                json!("windows-x86_64-nsis-perMachine"),
            );
            for value in ["nsis", "deb", "dmg", "helper", "launcher"] {
                for field in ["finalInstaller", "helper", "launcher"] {
                    push(&format!("/signed/{field}/format"), json!(value));
                }
            }
            for algorithm in ["git-sha1", "git-sha256"] {
                push(
                    "/signed/provenance/sourceRevision/algorithm",
                    json!(algorithm),
                );
            }
            for length in [39, 40, 41, 63, 64, 65] {
                push(
                    "/signed/provenance/sourceRevision/value",
                    json!("a".repeat(length)),
                );
            }
            push(
                "/signed/provenance/finalInstallerSha256",
                json!("0".repeat(64)),
            );
        }
        _ => {}
    }
    values
}
#[test]
fn actual_node_metadata_semantics_matches_all_roles_windows_references_and_relationships() {
    let mut cases = Vec::new();
    for role in [
        "root",
        "timestamp",
        "snapshot",
        "target",
        "release",
        "package",
    ] {
        let (original, now) = original(role);
        let mut values = relationship_variants(role, &original);
        if role != "package" {
            for start in [0_u64, 1, 1700000000000, 9007199254740991 - duration(role)] {
                for duration in [0, 1, duration(role) - 1, duration(role), duration(role) + 1] {
                    let mut changed = original.clone();
                    changed["signed"]["issuedAt"] = json!(start);
                    changed["signed"]["expiresAt"] = json!(start + duration);
                    values.push(changed);
                }
            }
        }
        for value in values {
            let mut times = vec![
                json!(now),
                json!(0),
                json!(-0.0),
                json!(-1),
                Value::Null,
                json!("1"),
                json!(1.5),
                json!(9007199254740991_u64),
                json!(9007199254740992_u64),
            ];
            if let Some(start) = value["signed"]["issuedAt"].as_u64() {
                let end = value["signed"]["expiresAt"].as_u64().unwrap();
                for t in [
                    start.saturating_sub(1),
                    start,
                    start.saturating_add(1),
                    end.saturating_sub(1),
                    end,
                    end.saturating_add(1),
                ] {
                    times.push(json!(t));
                }
            }
            for now in times {
                for allow_expired in [false, true] {
                    let signed = &value["signed"];
                    cases.push(json!({"raw":STANDARD.encode(wire(&value)),"nowRaw":STANDARD.encode(serde_json::to_vec(&now).unwrap()),"allowExpired":allow_expired,
                    "expected":{"role":signed["role"],"product":signed["product"],"component":signed["component"],"scope":signed["scope"]}}));
                }
            }
        }
    }
    let expected: Vec<bool> = cases
        .iter()
        .map(|case| {
            let raw = STANDARD.decode(case["raw"].as_str().unwrap()).unwrap();
            let time = STANDARD.decode(case["nowRaw"].as_str().unwrap()).unwrap();
            let envelope = decode_canonical_envelope(&raw, InputKind::Metadata).and_then(|e| {
                e.check_full_schema()
                    .map_err(|_| pinvou_upgrade_codec::EnvelopeShapeError::InvalidEnvelope)
            });
            let now = Document::parse(&time, InputKind::Metadata)
                .ok()
                .and_then(|d| d.protocol_time_shape().ok());
            match (envelope, now) {
                (Ok(envelope), Some(now)) => assert_supplied_metadata_semantics(
                    &envelope,
                    &now,
                    if case["allowExpired"] == true {
                        RootWindowMode::AllowExpired
                    } else {
                        RootWindowMode::Current
                    },
                )
                .is_ok(),
                _ => false,
            }
        })
        .collect();
    let reference = root().join("specs/upgrade/v1/reference/signatures.mjs");
    let mut child = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            include_str!("metadata-semantics-oracle.mjs"),
            reference.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&cases).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "original metadata semantics oracle unavailable"
    );
    let actual: Vec<bool> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (index, (a, b)) in actual.iter().zip(expected.iter()).enumerate() {
        assert_eq!(a, b, "metadata semantics case {index}");
    }
    eprintln!(
        "{} original Node metadata semantics cases passed",
        cases.len()
    );
}

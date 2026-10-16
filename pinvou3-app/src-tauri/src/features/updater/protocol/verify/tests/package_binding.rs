use base64::{Engine, engine::general_purpose::STANDARD};
use pinvou_upgrade_codec::{Document, InputKind, decode_canonical_envelope};
use pinvou_upgrade_verifier::{PackageBindingInputs, verify_release_package};
use serde_json::Value;
use std::{path::Path, process::Command, sync::OnceLock};

fn document(value: &Value) -> Document {
    Document::parse(&serde_json::to_vec(value).unwrap(), InputKind::Metadata).unwrap()
}
fn cases() -> &'static [Value] {
    static CASES: OnceLock<Vec<Value>> = OnceLock::new();
    CASES.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(7)
            .unwrap();
        let output = Command::new("node")
            .args([
                "--input-type=module",
                "-e",
                include_str!("package-binding-oracle.mjs"),
            ])
            .arg(root.join("specs/upgrade/v1/reference/signatures.mjs"))
            .output()
            .expect("Node is required for the complete Package binding oracle");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    })
}
fn check_group(group: &str, count: usize) {
    let mut actual = 0;
    for case in cases().iter().filter(|c| c["group"] == group) {
        actual += 1;
        assert_eq!(
            case["releaseSchema"], true,
            "unreachable supplied Release {}",
            case["name"]
        );
        let root = document(&case["trustedRoot"]).root_policy_shape().unwrap();
        let context = document(&case["context"]);
        let now = document(&case["now"]).protocol_time_shape().unwrap();
        let release_raw = STANDARD
            .decode(case["releaseBytes"].as_str().unwrap())
            .unwrap();
        let release = decode_canonical_envelope(&release_raw, InputKind::Metadata)
            .unwrap()
            .check_full_schema()
            .unwrap();
        let package = STANDARD
            .decode(case["packageBytes"].as_str().unwrap())
            .unwrap();
        let denied: Vec<String> = serde_json::from_value(case["deniedKeyIds"].clone()).unwrap();
        let inputs = PackageBindingInputs::new(&root, &context, &now).with_denied_key_ids(&denied);
        match verify_release_package(&release, &package, inputs) {
            Ok(checked) => {
                assert_eq!(
                    case["accepted"], true,
                    "unexpected Package acceptance {}",
                    case["name"]
                );
                assert!(std::ptr::eq(checked.supplied_root(), &root));
                assert!(std::ptr::eq(checked.supplied_release(), &release));
                assert_eq!(checked.supplied_time_millis(), now.as_millis());
                assert_eq!(checked.checked_package().envelope().raw_bytes(), package);
                assert_eq!(
                    checked.supplied_release().envelope().raw_bytes(),
                    release_raw
                );
            }
            Err(error) => {
                assert_eq!(
                    case["accepted"], false,
                    "unexpected Package refusal {}: {error}",
                    case["name"]
                );
                let expected = match (
                    case["name"].as_str().unwrap(),
                    case["code"].as_str().unwrap(),
                ) {
                    // Existing closed reference factory rejects these invalid
                    // Package-role projections before original-byte comparison.
                    (
                        "original-reference-version"
                        | "original-reference-expiresAt"
                        | "original-reference-role",
                        "REFERENCE_MISMATCH",
                    ) => "METADATA_REFERENCE_INVALID",
                    (_, "REFERENCE_MISMATCH") => "METADATA_REFERENCE_MISMATCH",
                    (_, "SCHEMA_INVALID") => "ENVELOPE_SCHEMA_INVALID",
                    (_, "ROLE_NOT_AUTHORIZED") => "ROLE_POLICY_NOT_DECLARED",
                    (_, code) => code,
                };
                assert_eq!(
                    error.to_string(),
                    expected,
                    "wrong Package refusal {}",
                    case["name"]
                );
            }
        }
    }
    assert_eq!(actual, count, "changed Package group {group}");
}
#[test]
fn original_reference_and_business_identity_bindings() {
    check_group("binding", 13);
}
#[test]
fn supplied_release_is_not_reauthenticated_or_given_current_rights() {
    check_group("supplied_release", 4);
}
#[test]
fn independent_package_context_and_ignored_extras() {
    check_group("context", 3);
}
#[test]
fn actual_package_thresholds_all_extras_and_denial() {
    check_group("signatures", 8);
}
#[test]
fn original_windows_exact_policy_wire_and_production() {
    check_group("wire_policy", 7);
}

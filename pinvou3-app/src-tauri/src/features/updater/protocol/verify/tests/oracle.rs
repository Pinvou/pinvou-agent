use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pinvou_upgrade_codec::{Document, InputKind, PackageSemanticsMode};
use pinvou_upgrade_verifier::{VerificationInputs, verify_with_supplied_root};
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
                include_str!("envelope-oracle.mjs"),
            ])
            .arg(root.join("specs/upgrade/v1/reference/canonical-json.mjs"))
            .output()
            .expect("Node is required for the original full signature oracle");
        assert!(
            output.status.success(),
            "original full oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    })
}
fn check_group(group: &str, expected_count: usize) {
    let mut count = 0;
    for case in cases().iter().filter(|c| c["group"] == group) {
        count += 1;
        let root = document(&case["trustedRoot"]).root_policy_shape().unwrap();
        let expected = document(&case["expected"]);
        let now = document(&case["now"]).protocol_time_shape().unwrap();
        let high_water = case.get("highWater").map(document);
        let denied: Vec<String> = case
            .get("deniedKeyIds")
            .map(|ids| {
                ids.as_array()
                    .unwrap()
                    .iter()
                    .map(|id| id.as_str().unwrap().to_owned())
                    .collect()
            })
            .unwrap_or_default();
        let mut inputs =
            VerificationInputs::new(&root, &expected, &now).with_denied_key_ids(&denied);
        if let Some(record) = &high_water {
            inputs = inputs.with_high_water(record);
        }
        if case.get("production") == Some(&Value::Bool(false)) {
            inputs = inputs.with_package_mode(PackageSemanticsMode::Development);
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(case["bytes"].as_str().unwrap())
            .unwrap();
        match verify_with_supplied_root(&bytes, inputs) {
            Ok(checked) => {
                assert_eq!(
                    case["accepted"], true,
                    "unexpected acceptance: {group}/{}",
                    case["name"]
                );
                assert_eq!(checked.envelope().envelope().raw_bytes(), bytes);
                assert!(std::ptr::eq(checked.supplied_root(), &root));
                assert_eq!(
                    checked.verified_signer_ids().len(),
                    checked.envelope().envelope().signatures().len()
                );
            }
            Err(error) => {
                assert_eq!(
                    case["accepted"], false,
                    "unexpected refusal: {group}/{}: {error}",
                    case["name"]
                );
                assert_eq!(
                    error.to_string(),
                    case["rustCode"].as_str().unwrap(),
                    "wrong refusal: {group}/{}",
                    case["name"]
                );
            }
        }
    }
    assert_eq!(count, expected_count, "changed oracle coverage: {group}");
}
#[test]
fn original_twenty_full_envelopes() {
    check_group("fixed", 20);
}
#[test]
fn real_thresholds_all_extra_records_and_denial() {
    check_group("threshold", 10);
}
#[test]
fn credential_declared_key_and_ignored_high_water() {
    check_group("credential", 3);
}
#[test]
fn independent_complete_context_and_exact_root_policy() {
    check_group("context", 9);
}
#[test]
fn fresh_root_and_body_supplied_windows() {
    check_group("time", 7);
}
#[test]
fn original_bytes_high_water_rollback_and_equivocation() {
    check_group("high-water", 5);
}
#[test]
fn same_signed_incremental_requires_explicit_development() {
    check_group("package", 4);
}
#[test]
fn actual_two_of_three_root_and_genuine_successor_boundary() {
    check_group("root", 3);
}
#[test]
fn adversarial_a_r_and_s_match_the_frozen_provider_profile() {
    check_group("crypto-encoding", 39);
}
#[test]
fn original_input_budgets_canonical_bytes_and_closed_schema() {
    check_group("input", 11);
}

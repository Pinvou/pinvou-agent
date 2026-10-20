use base64::{Engine, engine::general_purpose::STANDARD};
use pinvou_upgrade_codec::{Document, InputKind};
use pinvou_upgrade_verifier::{CandidateVerificationInputs, verify_candidate_release};
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
                include_str!("candidate-oracle.mjs"),
            ])
            .arg(root.join("specs/upgrade/v1/reference/signatures.mjs"))
            .output()
            .expect("Node is required for the complete candidate verifier oracle");
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
        let root = document(&case["trustedRoot"]).root_policy_shape().unwrap();
        let context = document(&case["context"]);
        let opening = document(&case["opening"]);
        let now = document(&case["now"]).protocol_time_shape().unwrap();
        let target = STANDARD
            .decode(case["targetBytes"].as_str().unwrap())
            .unwrap();
        let release = STANDARD
            .decode(case["releaseBytes"].as_str().unwrap())
            .unwrap();
        let denied: Vec<String> = serde_json::from_value(case["deniedKeyIds"].clone()).unwrap();
        let inputs =
            CandidateVerificationInputs::new(&root, &context, &now).with_denied_key_ids(&denied);
        match verify_candidate_release(&target, &release, &opening, inputs) {
            Ok(checked) => {
                assert_eq!(
                    case["accepted"], true,
                    "unexpected candidate acceptance {}",
                    case["name"]
                );
                assert!(std::ptr::eq(checked.supplied_root(), &root));
                assert_eq!(checked.supplied_time_millis(), now.as_millis());
                assert_eq!(checked.target().envelope().raw_bytes(), target);
                assert_eq!(checked.release().envelope().raw_bytes(), release);
            }
            Err(error) => {
                assert_eq!(
                    case["accepted"], false,
                    "unexpected candidate refusal {}: {error}",
                    case["name"]
                );
                let expected = match (
                    case["name"].as_str().unwrap(),
                    case["code"].as_str().unwrap(),
                ) {
                    (_, "OPENING_MISMATCH") => "ROLLOUT_OPENING_MISMATCH",
                    ("opening-unknown-field", "SCHEMA_INVALID") => "ROLLOUT_OPENING_INVALID",
                    (_, "SCHEMA_INVALID") => "ENVELOPE_SCHEMA_INVALID",
                    (_, "ROLE_NOT_AUTHORIZED") => "ROLE_POLICY_NOT_DECLARED",
                    (_, code) => code,
                };
                assert_eq!(
                    error.to_string(),
                    expected,
                    "wrong candidate refusal {}",
                    case["name"]
                );
            }
        }
    }
    assert_eq!(actual, count, "changed candidate group {group}");
}
#[test]
fn complete_opening_baseline_and_release_target_membership() {
    check_group("binding", 13);
}
#[test]
fn shared_independent_context_and_release_identity_are_mandatory() {
    check_group("context", 7);
}
#[test]
fn both_actual_signatures_thresholds_extras_and_global_denial() {
    check_group("signatures", 8);
}
#[test]
fn root_and_each_original_body_must_be_fresh() {
    check_group("windows", 4);
}
#[test]
fn exact_policy_original_wire_and_closed_schema() {
    check_group("wire_policy", 6);
}

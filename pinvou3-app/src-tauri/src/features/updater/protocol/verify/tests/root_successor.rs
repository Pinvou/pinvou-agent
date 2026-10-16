use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pinvou_upgrade_codec::{Document, InputKind};
use pinvou_upgrade_verifier::{ActiveMetadataInput, verify_root_successor};
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
                include_str!("root-successor-oracle.mjs"),
            ])
            .arg(root.join("specs/upgrade/v1/reference/canonical-json.mjs"))
            .output()
            .expect("Node is required for the frozen Root successor oracle");
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
        let old = document(&case["trustedRoot"]).root_policy_shape().unwrap();
        let expected = document(&case["expected"]);
        let now = document(&case["now"]).protocol_time_shape().unwrap();
        let bytes = URL_SAFE_NO_PAD
            .decode(case["bytes"].as_str().unwrap())
            .unwrap();
        let active: Vec<_> = case["activeEnvelopes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    URL_SAFE_NO_PAD
                        .decode(e["bytes"].as_str().unwrap())
                        .unwrap(),
                    document(&e["expected"]),
                )
            })
            .collect();
        let repeat = case["activeRepeat"].as_u64().unwrap() as usize;
        let active_inputs: Vec<_> = (0..repeat)
            .flat_map(|_| active.iter().map(|(b, e)| ActiveMetadataInput::new(b, e)))
            .collect();
        match verify_root_successor(&bytes, &old, &expected, &now, &active_inputs) {
            Ok(checked) => {
                assert_eq!(
                    case["accepted"], true,
                    "unexpected acceptance: {}",
                    case["name"]
                );
                assert_eq!(checked.envelope().envelope().raw_bytes(), bytes);
                assert!(std::ptr::eq(checked.supplied_old_root(), &old));
                assert_eq!(
                    checked.checked_next_root().canonical_bytes(),
                    checked
                        .envelope()
                        .envelope()
                        .signed()
                        .canonical_bytes()
                        .unwrap()
                );
                assert_eq!(checked.checked_next_root().version(), old.version() + 1);
                assert_eq!(
                    checked.verified_signer_ids().len(),
                    checked.envelope().envelope().signatures().len()
                );
                assert_eq!(checked.supplied_time_millis(), now.as_millis());
            }
            Err(e) => {
                assert_eq!(
                    case["accepted"], false,
                    "unexpected refusal {}: {e}",
                    case["name"]
                );
                assert_eq!(
                    e.to_string(),
                    case["rustCode"].as_str().unwrap(),
                    "wrong refusal {}",
                    case["name"]
                );
            }
        }
    }
    assert_eq!(actual, count, "changed Root successor oracle group {group}");
}
#[test]
fn actual_both_thresholds_and_all_extra_signatures() {
    check_group("threshold", 10);
}
#[test]
fn consecutive_product_and_safe_version() {
    check_group("continuity", 7);
}
#[test]
fn expired_old_but_fresh_next_and_original_windows() {
    check_group("windows", 7);
}
#[test]
fn caller_supplied_active_metadata_key_preservation() {
    check_group("active", 14);
}
#[test]
fn original_per_object_bounds_and_full_schema() {
    check_group("input", 12);
}

#[test]
fn complete_independent_root_context_is_mandatory() {
    use pinvou_upgrade_codec::ContextBindingError;
    use pinvou_upgrade_verifier::VerificationError;
    let case = &cases()[0];
    let old = document(&case["trustedRoot"]).root_policy_shape().unwrap();
    let now = document(&case["now"]).protocol_time_shape().unwrap();
    let bytes = URL_SAFE_NO_PAD
        .decode(case["bytes"].as_str().unwrap())
        .unwrap();
    for (name, expected, error) in [
        (
            "missing role",
            serde_json::json!({}),
            ContextBindingError::InvalidExpected,
        ),
        (
            "non-Root role",
            serde_json::json!({"role":"timestamp"}),
            ContextBindingError::InvalidExpected,
        ),
        (
            "missing other fields",
            serde_json::json!({"role":"root"}),
            ContextBindingError::InvalidExpected,
        ),
        (
            "component mismatch",
            serde_json::json!({"role":"root","product":old.product(),"component":"agent","scope":{}}),
            ContextBindingError::Mismatch,
        ),
        (
            "scope mismatch",
            serde_json::json!({"role":"root","product":old.product(),"component":null,"scope":{"channel":"stable"}}),
            ContextBindingError::Mismatch,
        ),
    ] {
        let expected = document(&expected);
        assert!(
            matches!(verify_root_successor(&bytes,&old,&expected,&now,&[]),
            Err(VerificationError::Context(actual)) if actual == error),
            "{name}"
        );
    }
}

#[test]
fn next_root_descriptors_and_policies_are_validated_before_use() {
    check_group("structure", 5);
}

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pinvou_upgrade_codec::{Document, InputKind};
use pinvou_upgrade_verifier::{ActiveMetadataInput, verify_root_chain, verify_root_successor};
use serde_json::Value;
use std::{path::Path, process::Command, sync::OnceLock};
fn document(value: &Value) -> Document {
    Document::parse(&serde_json::to_vec(value).unwrap(), InputKind::Metadata).unwrap()
}
fn node_cases(script: &str) -> Vec<Value> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(7)
        .unwrap();
    let output = Command::new("node")
        .args(["--input-type=module", "-e", script])
        .arg(root.join("specs/upgrade/v1/reference/canonical-json.mjs"))
        .output()
        .expect("Node is required for the frozen complete Root-chain oracle");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn cases() -> &'static [Value] {
    static CASES: OnceLock<Vec<Value>> = OnceLock::new();
    CASES.get_or_init(|| node_cases(include_str!("root-chain-oracle.mjs")))
}
fn check_group(group: &str, count: usize) {
    let mut actual = 0;
    for case in cases().iter().filter(|c| c["group"] == group) {
        actual += 1;
        let old = document(&case["trustedRoot"]).root_policy_shape().unwrap();
        let expected = document(&case["expected"]);
        let now = document(&case["now"]).protocol_time_shape().unwrap();
        let bytes: Vec<_> = case["successors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| URL_SAFE_NO_PAD.decode(b.as_str().unwrap()).unwrap())
            .collect();
        let repeat = case["repeatSteps"].as_u64().unwrap() as usize;
        let successors: Vec<_> = (0..repeat)
            .flat_map(|_| bytes.iter().map(Vec::as_slice))
            .collect();
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
        let active_inputs: Vec<_> = active
            .iter()
            .map(|(b, e)| ActiveMetadataInput::new(b, e))
            .collect();
        match verify_root_chain(&successors, &old, &expected, &now, &active_inputs) {
            Ok(checked) => {
                assert_eq!(
                    case["accepted"], true,
                    "unexpected acceptance {}",
                    case["name"]
                );
                assert!(std::ptr::eq(checked.supplied_initial_root(), &old));
                assert_eq!(checked.checked_steps(), successors.len());
                assert_eq!(
                    checked.final_envelope().envelope().raw_bytes(),
                    *successors.last().unwrap()
                );
                assert_eq!(
                    checked.checked_final_root().canonical_bytes(),
                    checked
                        .final_envelope()
                        .envelope()
                        .signed()
                        .canonical_bytes()
                        .unwrap()
                );
                assert_eq!(
                    checked.checked_final_root().version(),
                    old.version() + successors.len() as u64
                );
                assert_eq!(
                    checked.verified_final_signer_ids().len(),
                    checked.final_envelope().envelope().signatures().len()
                );
                assert_eq!(checked.supplied_time_millis(), now.as_millis());
            }
            Err(error) => {
                assert_eq!(
                    case["accepted"], false,
                    "unexpected refusal {}: {error}",
                    case["name"]
                );
                assert_eq!(
                    error.to_string(),
                    case["rustCode"].as_str().unwrap(),
                    "wrong refusal {}",
                    case["name"]
                );
            }
        }
    }
    assert_eq!(actual, count, "changed complete-chain group {group}");
}
#[test]
fn full_consecutive_product_version_and_original_steps() {
    check_group("continuity", 6);
}
#[test]
fn exact_one_4096_and_4097_count_bounds() {
    check_group("bounds", 4);
}
#[test]
fn only_nonfinal_expiry_is_permitted() {
    check_group("windows", 7);
}
#[test]
fn only_final_active_read_set_is_checked_completely() {
    check_group("active", 5);
}
#[test]
fn every_step_needs_both_actual_signature_thresholds() {
    check_group("signatures", 4);
}
#[test]
fn every_step_retains_original_raw_and_full_schema_limits() {
    check_group("input", 4);
}
#[test]
fn one_step_chain_matches_single_successor_in_all_fifty_five_reference_cases() {
    let script = format!(
        "{}\n{}",
        include_str!("root-successor-oracle.mjs").replace("console.log(JSON.stringify(rows));", ""),
        include_str!("root-chain-one-step-parity.mjs")
    );
    let cases = node_cases(&script);
    assert_eq!(cases.len(), 55);
    for case in &cases {
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
        let single = verify_root_successor(&bytes, &old, &expected, &now, &active_inputs);
        let chain = verify_root_chain(&[&bytes], &old, &expected, &now, &active_inputs);
        assert_eq!(
            single.is_ok(),
            case["accepted"].as_bool().unwrap(),
            "single {}",
            case["name"]
        );
        assert_eq!(
            chain.is_ok(),
            case["chainReferenceAccepted"].as_bool().unwrap(),
            "chain {}",
            case["name"]
        );
        match (single, chain) {
            (Ok(single), Ok(chain)) => {
                assert_eq!(chain.checked_steps(), 1);
                assert_eq!(
                    single.envelope().envelope().raw_bytes(),
                    chain.final_envelope().envelope().raw_bytes()
                );
                assert_eq!(
                    single.checked_next_root().canonical_bytes(),
                    chain.checked_final_root().canonical_bytes()
                );
            }
            (Err(single), Err(chain)) => assert_eq!(single, chain, "{}", case["name"]),
            _ => panic!("single/chain acceptance differs: {}", case["name"]),
        }
    }
}

#[test]
fn complete_independent_chain_root_context_is_mandatory() {
    use pinvou_upgrade_codec::ContextBindingError;
    use pinvou_upgrade_verifier::VerificationError;
    let case = &cases()[0];
    let old = document(&case["trustedRoot"]).root_policy_shape().unwrap();
    let now = document(&case["now"]).protocol_time_shape().unwrap();
    let bytes: Vec<_> = case["successors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| URL_SAFE_NO_PAD.decode(b.as_str().unwrap()).unwrap())
        .collect();
    let successors: Vec<_> = bytes.iter().map(Vec::as_slice).collect();
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
            matches!(verify_root_chain(&successors,&old,&expected,&now,&[]),
            Err(VerificationError::Context(actual)) if actual == error),
            "{name}"
        );
    }
}

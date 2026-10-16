use base64::{Engine, engine::general_purpose::STANDARD};
use pinvou_upgrade_codec::{Document, InputKind};
use pinvou_upgrade_verifier::{ReleaseRenewalInputs, verify_release_renewal};
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
                include_str!("release-renewal-oracle.mjs"),
            ])
            .arg(root.join("specs/upgrade/v1/reference/signatures.mjs"))
            .output()
            .expect("Node required for composed Release renewal controls");
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
        let archived = document(&case["archivedRoot"]).root_policy_shape().unwrap();
        let current = document(&case["currentRoot"]).root_policy_shape().unwrap();
        // Identical supplied trust may serve both historical/current roles.
        let historical_root = if case["archivedRoot"] == case["currentRoot"] {
            &current
        } else {
            &archived
        };
        let context = document(&case["context"]);
        let now = document(&case["now"]).protocol_time_shape().unwrap();
        let previous = STANDARD
            .decode(case["previousBytes"].as_str().unwrap())
            .unwrap();
        let next = STANDARD
            .decode(case["nextBytes"].as_str().unwrap())
            .unwrap();
        let denied: Vec<String> = serde_json::from_value(case["deniedKeyIds"].clone()).unwrap();
        match verify_release_renewal(
            &previous,
            &next,
            ReleaseRenewalInputs::new(historical_root, &current, &context, &now)
                .with_denied_key_ids(&denied),
        ) {
            Ok(proof) => {
                assert_eq!(
                    case["accepted"], true,
                    "unexpected renewal acceptance {}",
                    case["name"]
                );
                assert!(std::ptr::eq(
                    proof.previous().supplied_archived_root(),
                    historical_root
                ));
                assert!(std::ptr::eq(proof.next().supplied_root(), &current));
                assert_eq!(proof.previous().envelope().envelope().raw_bytes(), previous);
                assert_eq!(proof.next().envelope().envelope().raw_bytes(), next);
                assert_eq!(proof.next().supplied_time_millis(), now.as_millis());
                assert!(!proof.previous().verified_signer_ids().is_empty());
                assert!(!proof.next().verified_signer_ids().is_empty());
            }
            Err(error) => {
                assert_eq!(
                    case["accepted"], false,
                    "unexpected renewal refusal {}: {error}",
                    case["name"]
                );
                let expected = match case["code"].as_str().unwrap() {
                    "SCHEMA_INVALID" => "ENVELOPE_SCHEMA_INVALID",
                    "ROLE_NOT_AUTHORIZED" => "ROLE_POLICY_NOT_DECLARED",
                    code => code,
                };
                assert_eq!(
                    error.to_string(),
                    expected,
                    "wrong renewal refusal {}",
                    case["name"]
                );
            }
        }
    }
    assert_eq!(actual, count, "changed renewal group {group}");
}
#[test]
fn authenticated_renewal_preserves_original_historical_and_revision_boundaries() {
    check_group("renewal", 8);
}
#[test]
fn immutable_business_projection_stays_bound() {
    check_group("business", 7);
}
#[test]
fn independent_product_component_release_scope() {
    check_group("context", 6);
}
#[test]
fn both_actual_thresholds_all_extras_exact_policies_and_current_deny() {
    check_group("signature", 12);
}
#[test]
fn original_historical_and_fresh_current_windows() {
    check_group("window", 7);
}
#[test]
fn original_canonical_schema_resource_and_safe_revision_bounds() {
    check_group("wire", 7);
}

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pinvou_upgrade_codec::{Document, InputKind};
use pinvou_upgrade_verifier::{
    VerificationInputs, verify_archived_package, verify_archived_release, verify_with_supplied_root,
};
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
                include_str!("archive-identity-oracle.mjs"),
            ])
            .arg(root.join("specs/upgrade/v1/reference/canonical-json.mjs"))
            .output()
            .expect("Node is required for the frozen historical identity oracle");
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
        let root = document(&case["archivedRoot"]).root_policy_shape().unwrap();
        let expected = document(&case["expected"]);
        let now = document(&case["now"]).protocol_time_shape().unwrap();
        let bytes = URL_SAFE_NO_PAD
            .decode(case["bytes"].as_str().unwrap())
            .unwrap();
        let result = match case["operation"].as_str().unwrap() {
            "package" => verify_archived_package(&bytes, &root, &expected, &now),
            "release" => verify_archived_release(&bytes, &root, &expected),
            other => panic!("unexpected archive operation {other}"),
        };
        match result {
            Ok(checked) => {
                assert_eq!(
                    case["accepted"], true,
                    "unexpected identity acceptance {}",
                    case["name"]
                );
                assert_eq!(checked.envelope().envelope().raw_bytes(), bytes);
                assert!(std::ptr::eq(checked.supplied_archived_root(), &root));
                assert_eq!(
                    checked.verified_signer_ids().len(),
                    checked.envelope().envelope().signatures().len()
                );
            }
            Err(error) => {
                assert_eq!(
                    case["accepted"], false,
                    "unexpected identity refusal {}: {error}",
                    case["name"]
                );
                assert_eq!(
                    error.to_string(),
                    case["rustCode"].as_str().unwrap(),
                    "wrong identity refusal {}",
                    case["name"]
                );
            }
        }
        if let Some(code) = case.get("ordinaryCode") {
            let result =
                verify_with_supplied_root(&bytes, VerificationInputs::new(&root, &expected, &now));
            match result {
                Err(error) => assert_eq!(
                    error.to_string(),
                    code.as_str().unwrap(),
                    "ordinary {}",
                    case["name"]
                ),
                Ok(_) => panic!(
                    "historical identity granted ordinary acceptance: {}",
                    case["name"]
                ),
            }
        }
    }
    assert_eq!(actual, count, "changed historical identity group {group}");
}
#[test]
fn package_keeps_original_root_window_and_production_rules() {
    check_group("package", 6);
}
#[test]
fn release_checks_separate_original_issued_time_points() {
    check_group("release", 8);
}
#[test]
fn both_archived_roles_need_actual_all_signatures_and_thresholds() {
    check_group("signatures", 8);
}
#[test]
fn archived_identities_bind_independent_full_context() {
    check_group("context", 8);
}
#[test]
fn both_roles_keep_original_wire_limits_and_full_schema() {
    check_group("input", 6);
}

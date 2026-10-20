use base64::{Engine, engine::general_purpose::STANDARD};
use pinvou_upgrade_codec::{Document, InputKind};
use pinvou_upgrade_verifier::{DownloadVerificationInputs, verify_download_info_response};
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
                include_str!("download-response-oracle.mjs"),
            ])
            .arg(root.join("specs/upgrade/v1/reference/signatures.mjs"))
            .output()
            .expect("Node required for composed signed-download controls");
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
        let now = document(&case["now"]).protocol_time_shape().unwrap();
        let request_bytes = STANDARD
            .decode(case["requestBytes"].as_str().unwrap())
            .unwrap();
        let request = Document::parse(&request_bytes, InputKind::Metadata).unwrap();
        let response = STANDARD
            .decode(case["responseBytes"].as_str().unwrap())
            .unwrap();
        let denied: Vec<String> = serde_json::from_value(case["deniedKeyIds"].clone()).unwrap();
        match verify_download_info_response(
            &response,
            DownloadVerificationInputs::new(&root, &request, &now).with_denied_key_ids(&denied),
        ) {
            Ok(proof) => {
                assert_eq!(
                    case["accepted"], true,
                    "unexpected response acceptance {}",
                    case["name"]
                );
                assert!(std::ptr::eq(proof.supplied_root(), &root));
                assert!(std::ptr::eq(proof.supplied_request(), &request));
                assert_eq!(proof.supplied_time_millis(), now.as_millis());
                assert_eq!(proof.original_response_bytes(), response);
                let transport = Document::parse(&response, InputKind::Metadata).unwrap();
                let original = document(&request.value()["decision"])
                    .canonical_bytes()
                    .unwrap();
                assert_eq!(proof.decision().envelope().raw_bytes(), original);
                let original = document(&transport.value()["credential"])
                    .canonical_bytes()
                    .unwrap();
                assert_eq!(proof.download_credential().envelope().raw_bytes(), original);
            }
            Err(error) => {
                assert_eq!(
                    case["accepted"], false,
                    "unexpected response refusal {}: {error}",
                    case["name"]
                );
                let expected = match case["code"].as_str().unwrap() {
                    "ROLE_NOT_AUTHORIZED" => "ROLE_POLICY_NOT_DECLARED",
                    code => code,
                };
                assert_eq!(
                    error.to_string(),
                    expected,
                    "wrong response refusal {}",
                    case["name"]
                );
            }
        }
    }
    assert_eq!(actual, count, "changed response group {group}");
}
#[test]
fn actual_download_response_and_distinct_role_identities() {
    check_group("positive", 5);
}
#[test]
fn every_shared_qualification_and_complete_update_is_inherited() {
    check_group("inheritance", 16);
}
#[test]
fn independent_scope_and_request_contract_binding() {
    check_group("context", 7);
}
#[test]
fn requested_package_current_decision_and_outer_byte_identity() {
    check_group("package", 7);
}
#[test]
fn both_actual_signatures_thresholds_all_extras_and_deny() {
    check_group("signature", 12);
}
#[test]
fn current_decision_new_download_and_url_windows() {
    check_group("window", 8);
}
#[test]
fn complete_contract_wire_and_resource_limits() {
    check_group("wire", 6);
}

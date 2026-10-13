use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use pinvou_upgrade_codec::{
    CheckResponseError, CheckResponseOperation, Document, Error, InputKind,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Command,
};
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(7)
        .unwrap()
        .to_owned()
}
fn original(role: &str) -> Value {
    let v: Value = serde_json::from_slice(
        &std::fs::read(root().join("specs/upgrade/v1/vectors/crypto/envelopes.json")).unwrap(),
    )
    .unwrap();
    let v = v["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == role)
        .unwrap();
    serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(v["envelopeBase64Url"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap()
}
fn base() -> Value {
    json!({"protocolVersion":1,"requestId":"request-1","decision":original("decision"),"telemetryCredential":original("telemetry-event")})
}
fn check(v: &Value, op: CheckResponseOperation) -> Result<(), CheckResponseError> {
    Document::parse(&serde_json::to_vec(v).unwrap(), InputKind::Metadata)
        .unwrap()
        .assert_supplied_check_response_contract(op)
}
#[test]
fn exact_closed_operations_bind_all_identities_and_keep_independent_response_fields() {
    let b = base();
    assert_eq!(check(&b, CheckResponseOperation::Check), Ok(()));
    let mut r = b.clone();
    r["workflowRevision"] = json!(0);
    r["writableSessionId"] = r["decision"]["signed"]["telemetrySessionId"].clone();
    assert_eq!(check(&r, CheckResponseOperation::Refresh), Ok(()));
    assert_eq!(
        check(&b, CheckResponseOperation::Refresh),
        Err(CheckResponseError::InvalidContract)
    );
    assert_eq!(
        check(&r, CheckResponseOperation::Check),
        Err(CheckResponseError::InvalidContract)
    );
    for field in [
        "product",
        "component",
        "installId",
        "decisionId",
        "decisionRevision",
        "telemetrySessionId",
        "selectionGeneration",
    ] {
        let mut v = b.clone();
        let old = v["telemetryCredential"]["signed"][field].clone();
        v["telemetryCredential"]["signed"][field] = if let Some(n) = old.as_u64() {
            json!(n + 1)
        } else {
            json!("other-identity")
        };
        assert_eq!(
            check(&v, CheckResponseOperation::Check),
            Err(CheckResponseError::BindingMismatch),
            "{field}"
        );
        v["telemetryCredential"]["signed"][field] = old;
        assert_eq!(check(&v, CheckResponseOperation::Check), Ok(()));
    }
    for (field, other) in [
        ("installationScopeId", json!("other-scope")),
        ("channel", json!("beta")),
        ("channelRevision", json!(2)),
        ("targetKey", json!("linux-arm64-deb-perMachine")),
    ] {
        let mut v = b.clone();
        let old = v["telemetryCredential"]["signed"]["scope"][field].clone();
        assert_ne!(old, other, "scope mutation must change {field}");
        v["telemetryCredential"]["signed"]["scope"][field] = other;
        assert_eq!(
            check(&v, CheckResponseOperation::Check),
            Err(CheckResponseError::BindingMismatch),
            "{field}"
        );
        v["telemetryCredential"]["signed"]["scope"][field] = old;
        assert_eq!(check(&v, CheckResponseOperation::Check), Ok(()));
    }
    r["writableSessionId"] = json!("other-session");
    assert_eq!(
        check(&r, CheckResponseOperation::Refresh),
        Err(CheckResponseError::BindingMismatch)
    );
    r["writableSessionId"] = r["decision"]["signed"]["telemetrySessionId"].clone();
    r["workflowRevision"] = json!(9007199254740991u64);
    r["requestId"] = json!("independent-request");
    assert_eq!(check(&r, CheckResponseOperation::Refresh), Ok(()));
}
#[test]
fn contract_time_and_signature_data_never_claim_current_authentication() {
    let mut b = base();
    for env in ["decision", "telemetryCredential"] {
        b[env]["signed"]["nbf"] = json!(b[env]["signed"]["iat"].as_u64().unwrap() + 1);
        b[env]["signatures"][0]["signature"] = json!("A".repeat(86));
        let sig = b[env]["signatures"][0].clone();
        b[env]["signatures"].as_array_mut().unwrap().push(sig);
    }
    assert_eq!(check(&b, CheckResponseOperation::Check), Ok(())); // API contract neither verifies nor sorts/deduplicates signatures.
    b["decision"]["signed"]["nbf"] = b["decision"]["signed"]["exp"].clone();
    assert!(matches!(
        check(&b, CheckResponseOperation::Check),
        Err(CheckResponseError::Semantics(_))
    ));
}
#[test]
fn original_credential_and_raw_input_limits_apply_before_any_trust_result() {
    let mut b = base();
    let sig = b["telemetryCredential"]["signatures"][0].clone();
    while Document::parse(
        &serde_json::to_vec(&b["telemetryCredential"]).unwrap(),
        InputKind::Metadata,
    )
    .unwrap()
    .canonical_bytes()
    .unwrap()
    .len()
        <= 65536
    {
        b["telemetryCredential"]["signatures"]
            .as_array_mut()
            .unwrap()
            .push(sig.clone());
    }
    // The greatest whole-signature count still within 64KiB is accepted;
    // adding the next identical schema-only signature crosses the original cap.
    let over_signature = b["telemetryCredential"]["signatures"]
        .as_array_mut()
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(check(&b, CheckResponseOperation::Check), Ok(()));
    b["telemetryCredential"]["signatures"]
        .as_array_mut()
        .unwrap()
        .push(over_signature);
    assert_eq!(
        check(&b, CheckResponseOperation::Check),
        Err(CheckResponseError::CredentialTooLarge)
    );
    let b = base();
    let mut padded = serde_json::to_vec_pretty(&b).unwrap();
    padded.resize(1024 * 1024, b' ');
    assert_eq!(
        Document::parse(&padded, InputKind::Metadata)
            .unwrap()
            .assert_supplied_check_response_contract(CheckResponseOperation::Check),
        Ok(())
    );
    padded.push(b' ');
    assert_eq!(
        Document::parse(&padded, InputKind::Metadata).err(),
        Some(Error::InputLimit)
    );
    let raw = serde_json::to_string(&b).unwrap().replacen(
        "\"requestId\":\"request-1\"",
        "\"requestId\":\"request-1\",\"requestId\":\"request-1\"",
        1,
    );
    assert_eq!(
        Document::parse(raw.as_bytes(), InputKind::Metadata).err(),
        Some(Error::DuplicateKey)
    );
}
#[test]
fn actual_full_original_api_contract_matches_all_nested_fields_and_relation_controls() {
    let reference = root().join("specs/upgrade/v1/reference/api-registry.mjs");
    let output = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            include_str!("check-response-oracle.mjs"),
            reference.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "original API oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cases: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert!(cases.len() > 1000);
    for case in &cases {
        let bytes = STANDARD.decode(case["bytes"].as_str().unwrap()).unwrap();
        let op = if case["operation"] == "check" {
            CheckResponseOperation::Check
        } else {
            CheckResponseOperation::Refresh
        };
        let actual = Document::parse(&bytes, InputKind::Metadata)
            .and_then(|d| {
                d.assert_supplied_check_response_contract(op)
                    .map_err(|_| Error::InvalidJson)
            })
            .is_ok();
        assert_eq!(
            actual,
            case["accepted"].as_bool().unwrap(),
            "{} {}",
            case["operation"],
            case["name"]
        );
    }
    eprintln!(
        "{} actual original check/refresh API response cases passed",
        cases.len()
    );
}

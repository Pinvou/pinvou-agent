use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use pinvou_upgrade_codec::{
    Document, Error, HelperCleanupResponseError as ContractError, InputKind,
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
fn original() -> Value {
    let v: Value = serde_json::from_slice(
        &std::fs::read(root().join("specs/upgrade/v1/vectors/crypto/envelopes.json")).unwrap(),
    )
    .unwrap();
    let v = v["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "helper-cleanup")
        .unwrap();
    serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(v["envelopeBase64Url"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap()
}
fn baseline() -> Value {
    json!({"protocolVersion":1,"requestId":"request-1","credential":original()})
}
fn check(v: &Value) -> Result<(), ContractError> {
    Document::parse(&serde_json::to_vec(v).unwrap(), InputKind::Metadata)
        .map_err(ContractError::Json)?
        .assert_supplied_helper_cleanup_response_contract()
}
#[test]
fn complete_cleanup_contract_is_closed_and_does_not_add_request_correlation() {
    let b = baseline();
    assert_eq!(check(&b), Ok(()));
    let mut v = b.clone();
    v["requestId"] = json!("other-request");
    assert_eq!(check(&v), Ok(()));
    v = b.clone();
    v["extra"] = Value::Null;
    assert_eq!(check(&v), Err(ContractError::InvalidContract));
    for field in ["protocolVersion", "requestId", "credential"] {
        v = b.clone();
        v.as_object_mut().unwrap().remove(field);
        assert_eq!(check(&v), Err(ContractError::InvalidContract));
    }
    v = b.clone();
    v["credential"]["signed"]["role"] = json!("download");
    assert_eq!(check(&v), Err(ContractError::InvalidContract));
    assert_eq!(check(&b), Ok(()));
}
#[test]
fn cleanup_signature_and_supplied_time_data_never_grant_deletion_rights() {
    let mut v = baseline();
    v["credential"]["signed"]["nbf"] =
        json!(v["credential"]["signed"]["iat"].as_u64().unwrap() + 1);
    v["credential"]["signatures"][0]["signature"] = json!("A".repeat(86));
    let sig = v["credential"]["signatures"][0].clone();
    v["credential"]["signatures"]
        .as_array_mut()
        .unwrap()
        .push(sig);
    assert_eq!(check(&v), Ok(()));
    v["credential"]["signed"]["nbf"] = v["credential"]["signed"]["exp"].clone();
    assert!(matches!(check(&v), Err(ContractError::Semantics(_))));
}
#[test]
fn cleanup_credential_and_whole_input_retain_original_limits_and_duplicate_rejection() {
    let mut v = baseline();
    let sig = v["credential"]["signatures"][0].clone();
    while Document::parse(
        &serde_json::to_vec(&v["credential"]).unwrap(),
        InputKind::Metadata,
    )
    .unwrap()
    .canonical_bytes()
    .unwrap()
    .len()
        <= 65536
    {
        v["credential"]["signatures"]
            .as_array_mut()
            .unwrap()
            .push(sig.clone());
    }
    let last = v["credential"]["signatures"]
        .as_array_mut()
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(check(&v), Ok(()));
    v["credential"]["signatures"]
        .as_array_mut()
        .unwrap()
        .push(last);
    assert_eq!(check(&v), Err(ContractError::CredentialTooLarge));
    let mut raw = serde_json::to_vec(&baseline()).unwrap();
    raw.resize(1024 * 1024, b' ');
    assert_eq!(
        Document::parse(&raw, InputKind::Metadata)
            .unwrap()
            .assert_supplied_helper_cleanup_response_contract(),
        Ok(())
    );
    raw.push(b' ');
    assert_eq!(
        Document::parse(&raw, InputKind::Metadata).err(),
        Some(Error::InputLimit)
    );
    let duplicate = serde_json::to_string(&baseline()).unwrap().replacen(
        "{",
        r#"{"requestId":"duplicate","#,
        1,
    );
    assert_eq!(
        Document::parse(duplicate.as_bytes(), InputKind::Metadata).err(),
        Some(Error::DuplicateKey)
    );
}
#[test]
fn actual_complete_original_cleanup_response_contract_matches_nested_mutations() {
    let output = Command::new("node")
        .args([
            "--input-type=module",
            "-e",
            include_str!("helper-cleanup-response-oracle.mjs"),
        ])
        .arg(root().join("specs/upgrade/v1/reference/canonical-json.mjs"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cases: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert!(cases.len() >= 288);
    for c in &cases {
        let bytes = STANDARD.decode(c["bytes"].as_str().unwrap()).unwrap();
        let actual = Document::parse(&bytes, InputKind::Metadata)
            .ok()
            .is_some_and(|d| d.assert_supplied_helper_cleanup_response_contract().is_ok());
        assert_eq!(actual, c["accepted"].as_bool().unwrap(), "{}", c["name"]);
    }
    eprintln!(
        "matched {} actual original helper-cleanup response API cases",
        cases.len()
    );
}

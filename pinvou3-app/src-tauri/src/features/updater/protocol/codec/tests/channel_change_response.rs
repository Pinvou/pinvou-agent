use base64::{Engine, engine::general_purpose::STANDARD};
use pinvou_upgrade_codec::{
    ChannelChangeResponseError as ContractError, Document, Error, InputKind,
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
fn baseline() -> Value {
    json!({"protocolVersion":1,"requestId":"request-1","channel":"stable","channelRevision":1})
}
fn check(v: &Value) -> Result<(), ContractError> {
    Document::parse(&serde_json::to_vec(v).unwrap(), InputKind::Metadata)
        .map_err(ContractError::Json)?
        .assert_supplied_channel_change_response_contract()
}
#[test]
fn original_channel_domain_revision_boundaries_and_closed_response_are_preserved() {
    let b = baseline();
    for channel in ["stable", "beta", "internal"] {
        let mut v = b.clone();
        v["channel"] = json!(channel);
        assert_eq!(check(&v), Ok(()));
    }
    for revision in [1_u64, 9007199254740991] {
        let mut v = b.clone();
        v["channelRevision"] = json!(revision);
        assert_eq!(check(&v), Ok(()));
    }
    for revision in [0_u64, 9007199254740992] {
        let mut v = b.clone();
        v["channelRevision"] = json!(revision);
        assert_eq!(check(&v), Err(ContractError::InvalidContract));
    }
    let mut v = b.clone();
    v["requestId"] = json!("other-request");
    assert_eq!(check(&v), Ok(()));
    v = b.clone();
    v["channel"] = json!("other");
    assert_eq!(check(&v), Err(ContractError::InvalidContract));
    v = b.clone();
    v["extra"] = Value::Null;
    assert_eq!(check(&v), Err(ContractError::InvalidContract));
    for field in ["protocolVersion", "requestId", "channel", "channelRevision"] {
        let mut v = b.clone();
        v.as_object_mut().unwrap().remove(field);
        assert_eq!(check(&v), Err(ContractError::InvalidContract));
    }
    assert_eq!(check(&b), Ok(()));
}
#[test]
fn channel_response_raw_limit_and_duplicate_keys_precede_schema() {
    let b = baseline();
    let mut bytes = serde_json::to_vec(&b).unwrap();
    bytes.resize(1024 * 1024, b' ');
    assert_eq!(
        Document::parse(&bytes, InputKind::Metadata)
            .unwrap()
            .assert_supplied_channel_change_response_contract(),
        Ok(())
    );
    bytes.push(b' ');
    assert_eq!(
        Document::parse(&bytes, InputKind::Metadata).err(),
        Some(Error::InputLimit)
    );
    let duplicate =
        serde_json::to_string(&b)
            .unwrap()
            .replacen("{", r#"{"requestId":"duplicate","#, 1);
    assert_eq!(
        Document::parse(duplicate.as_bytes(), InputKind::Metadata).err(),
        Some(Error::DuplicateKey)
    );
}
#[test]
fn actual_complete_original_channel_response_matches_all_controls() {
    let output = Command::new("node")
        .args([
            "--input-type=module",
            "-e",
            include_str!("channel-change-response-oracle.mjs"),
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
    assert!(cases.len() >= 40);
    for c in &cases {
        let bytes = STANDARD.decode(c["bytes"].as_str().unwrap()).unwrap();
        let actual = Document::parse(&bytes, InputKind::Metadata)
            .ok()
            .is_some_and(|d| d.assert_supplied_channel_change_response_contract().is_ok());
        assert_eq!(actual, c["accepted"].as_bool().unwrap(), "{}", c["name"]);
    }
    eprintln!(
        "matched {} actual original channel-change response API cases",
        cases.len()
    );
}

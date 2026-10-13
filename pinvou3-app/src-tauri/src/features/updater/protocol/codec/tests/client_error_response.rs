use base64::{Engine, engine::general_purpose::STANDARD};
use pinvou_upgrade_codec::{
    ClientErrorResponseError as ContractError, ClientResponseOperation as Operation, Document,
    Error, InputKind,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
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
fn check(v: &Value, op: Operation, status: u16) -> Result<(), ContractError> {
    Document::parse(&serde_json::to_vec(v).unwrap(), InputKind::Metadata)
        .map_err(ContractError::Json)?
        .assert_supplied_client_error_response_contract(op, status)
}
#[test]
fn operation_http_status_and_special_poll_delay_are_bound_without_outcome_rights() {
    let unactivated = json!({"protocolVersion":1,"requestId":"request-1",
        "code":"SCOPE_UNACTIVATED","retryable":true,"checkAfterMs":1});
    assert_eq!(check(&unactivated, Operation::Check, 409), Ok(()));
    assert_eq!(
        check(&unactivated, Operation::Refresh, 409),
        Err(ContractError::InvalidContract)
    );
    assert_eq!(
        check(&unactivated, Operation::Check, 503),
        Err(ContractError::InvalidContract)
    );
    let mut v = unactivated.clone();
    v["checkAfterMs"] = json!(86400000);
    assert_eq!(check(&v, Operation::Check, 409), Ok(()));
    v["checkAfterMs"] = json!(86400001);
    assert_eq!(
        check(&v, Operation::Check, 409),
        Err(ContractError::InvalidContract)
    );
    v = unactivated.clone();
    v["retryAfterMs"] = json!(1);
    assert_eq!(
        check(&v, Operation::Check, 409),
        Err(ContractError::InvalidContract)
    );
    let unknown = json!({"protocolVersion":1,"requestId":"request-1",
        "code":"UNKNOWN_CONSUME_OUTCOME","retryable":false});
    assert_eq!(check(&unknown, Operation::ConsumeStatus, 404), Ok(()));
    assert_eq!(
        check(&unknown, Operation::ConsumeInstall, 404),
        Err(ContractError::InvalidContract)
    );
    for status in [0, 200, 201, 302, 409, 65535] {
        assert_eq!(
            check(&unknown, Operation::ConsumeStatus, status),
            Err(ContractError::InvalidContract)
        );
    }
    let mut rate =
        json!({"protocolVersion":1,"requestId":"request-1","code":"RATE_LIMITED","retryable":true});
    for op in Operation::ALL {
        assert_eq!(check(&rate, op, 429), Ok(()));
    }
    rate["retryAfterMs"] = json!(300000);
    assert_eq!(check(&rate, Operation::DownloadInfo, 429), Ok(()));
    rate["retryAfterMs"] = json!(300001);
    assert_eq!(
        check(&rate, Operation::DownloadInfo, 429),
        Err(ContractError::InvalidContract)
    );
    rate["retryAfterMs"] = json!(1);
    assert_eq!(check(&rate, Operation::DownloadInfo, 429), Ok(()));
}
#[test]
fn error_responses_keep_whole_raw_profile_and_duplicate_rejection_before_membership() {
    let value = json!({"protocolVersion":1,"requestId":"request-1","code":"CONTRACT_INVALID","retryable":false});
    let mut raw = serde_json::to_vec(&value).unwrap();
    raw.resize(1024 * 1024, b' ');
    assert_eq!(
        Document::parse(&raw, InputKind::Metadata)
            .unwrap()
            .assert_supplied_client_error_response_contract(Operation::Check, 400),
        Ok(())
    );
    raw.push(b' ');
    assert_eq!(
        Document::parse(&raw, InputKind::Metadata).err(),
        Some(Error::InputLimit)
    );
    let duplicate =
        serde_json::to_string(&value)
            .unwrap()
            .replacen("{", r#"{"code":"duplicate","#, 1);
    assert_eq!(
        Document::parse(duplicate.as_bytes(), InputKind::Metadata).err(),
        Some(Error::DuplicateKey)
    );
    assert_eq!(
        check(&json!(["CONTRACT_INVALID"]), Operation::Check, 400),
        Err(ContractError::InvalidContract)
    );
}
#[test]
fn actual_original_openapi_error_schemas_match_all_operations_statuses_fields_and_delays() {
    let output = Command::new("node")
        .args([
            "--input-type=module",
            "-e",
            include_str!("client-error-response-oracle.mjs"),
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
    assert!(cases.len() >= 14464);
    let mut covered = BTreeSet::new();
    for c in &cases {
        let op = c["operation"]
            .as_str()
            .unwrap()
            .parse::<Operation>()
            .unwrap();
        let status = u16::try_from(c["status"].as_u64().unwrap()).unwrap();
        let bytes = STANDARD.decode(c["bytes"].as_str().unwrap()).unwrap();
        let result = Document::parse(&bytes, InputKind::Metadata)
            .map_err(ContractError::Json)
            .and_then(|document| {
                document.assert_supplied_client_error_response_contract(op, status)
            });
        assert_eq!(
            result.is_ok(),
            c["accepted"].as_bool().unwrap(),
            "{} {} {}",
            c["operation"],
            status,
            c["name"]
        );
        if result.is_ok() {
            covered.insert(op.as_str());
        }
    }
    assert_eq!(
        covered,
        Operation::ALL.into_iter().map(|op| op.as_str()).collect()
    );
    eprintln!(
        "matched {} actual original OpenAPI HTTP-error cases over all26 client operations",
        cases.len()
    );
}

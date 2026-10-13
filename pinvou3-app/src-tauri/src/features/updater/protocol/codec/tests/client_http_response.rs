use base64::{Engine, engine::general_purpose::STANDARD};
use pinvou_upgrade_codec::{
    ChannelChangeResponseError, ClientErrorResponseError, ClientHttpResponseError as ContractError,
    ClientResponseError, ClientResponseOperation as Operation, Document, Error, InputKind,
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
fn check(v: &Value, operation: Operation, status: u16) -> Result<(), ContractError> {
    Document::parse(&serde_json::to_vec(v).unwrap(), InputKind::Metadata)
        .map_err(ContractError::Json)?
        .assert_supplied_client_http_response_contract(operation, status)
}
#[test]
fn operation_and_error_branches_preserve_typed_errors_and_reject_undeclared_statuses() {
    let channel =
        json!({"protocolVersion":1,"requestId":"request-1","channel":"stable","channelRevision":1});
    assert_eq!(check(&channel, Operation::ChannelChange, 200), Ok(()));
    let mut invalid = channel.clone();
    invalid["channelRevision"] = json!(0);
    assert_eq!(
        check(&invalid, Operation::ChannelChange, 200),
        Err(ContractError::Operation(
            ClientResponseError::ChannelChange(ChannelChangeResponseError::InvalidContract)
        ))
    );
    let error = json!({"protocolVersion":1,"requestId":"request-1","code":"SERVICE_UNAVAILABLE","retryable":true});
    assert_eq!(check(&error, Operation::ChannelChange, 503), Ok(()));
    assert_eq!(
        check(&channel, Operation::ChannelChange, 503),
        Err(ContractError::ErrorResponse(
            ClientErrorResponseError::InvalidContract
        ))
    );
    assert_eq!(
        check(&error, Operation::ChannelChange, 200),
        Err(ContractError::Operation(
            ClientResponseError::ChannelChange(ChannelChangeResponseError::InvalidContract)
        ))
    );
    for status in [0, 201, 204, 302, 399, 600, 65535] {
        assert_eq!(
            check(&channel, Operation::ChannelChange, status),
            Err(ContractError::InvalidStatus)
        );
        assert_eq!(
            check(&error, Operation::ChannelChange, status),
            Err(ContractError::InvalidStatus)
        );
    }
    assert_eq!(
        ContractError::InvalidStatus.to_string(),
        "API_CONTRACT_INVALID"
    );
    assert_eq!(check(&channel, Operation::ChannelChange, 200), Ok(()));
}
#[test]
fn complete_http_entry_keeps_metadata_raw_profile_and_duplicate_rejection() {
    for (value, status) in [
        (
            json!({"protocolVersion":1,"requestId":"request-1","channel":"stable","channelRevision":1}),
            200,
        ),
        (
            json!({"protocolVersion":1,"requestId":"request-1","code":"SERVICE_UNAVAILABLE","retryable":true}),
            503,
        ),
    ] {
        let mut raw = serde_json::to_vec(&value).unwrap();
        raw.resize(1024 * 1024, b' ');
        assert_eq!(
            Document::parse(&raw, InputKind::Metadata)
                .unwrap()
                .assert_supplied_client_http_response_contract(Operation::ChannelChange, status),
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
                .replacen("{", r#"{"requestId":"duplicate","#, 1);
        assert_eq!(
            Document::parse(duplicate.as_bytes(), InputKind::Metadata).err(),
            Some(Error::DuplicateKey)
        );
    }
}
#[test]
fn actual_original_operation_and_openapi_cases_match_complete_http_dispatch() {
    let output = Command::new("node")
        .args([
            "--input-type=module",
            "-e",
            include_str!("client-http-response-oracle.mjs"),
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
    assert!(cases.len() >= 15497);
    let mut operations = BTreeSet::new();
    let mut errors = BTreeSet::new();
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
                document.assert_supplied_client_http_response_contract(op, status)
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
            if status == 200 {
                operations.insert(op.as_str());
            } else {
                errors.insert(op.as_str());
            }
        }
        if let (Err(error), Some(code)) = (result, c["code"].as_str()) {
            assert_eq!(
                error.to_string(),
                code,
                "{} {} {}",
                c["operation"],
                status,
                c["name"]
            );
        }
    }
    let all: BTreeSet<_> = Operation::ALL.into_iter().map(|op| op.as_str()).collect();
    assert_eq!(operations, all);
    assert_eq!(errors, all);
    eprintln!(
        "matched {} actual original complete HTTP response cases",
        cases.len()
    );
}

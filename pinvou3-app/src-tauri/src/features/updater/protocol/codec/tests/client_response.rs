use base64::{Engine, engine::general_purpose::STANDARD};
use pinvou_upgrade_codec::{
    ChannelChangeResponseError, ClientResponseError as ContractError,
    ClientResponseOperation as Operation, Document, Error, InputKind,
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
fn fixture() -> Value {
    let output = Command::new("node")
        .args([
            "--input-type=module",
            "-e",
            include_str!("client-response-oracle.mjs"),
        ])
        .arg(root().join("specs/upgrade/v1/reference/canonical-json.mjs"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn operation_names_are_exact_roundtrips_and_never_accept_aliases_or_management() {
    let mut names = BTreeSet::new();
    for op in Operation::ALL {
        assert_eq!(op.as_str().parse(), Ok(op));
        assert!(names.insert(op.as_str()));
        for alias in [
            format!(" {}", op.as_str()),
            format!("{} ", op.as_str()),
            op.as_str().to_ascii_uppercase(),
            format!("{}-response", op.as_str()),
        ] {
            assert_eq!(
                alias.parse::<Operation>(),
                Err(ContractError::UnknownOperation)
            );
        }
    }
    assert_eq!(names.len(), 26);
    for name in [
        "",
        "other",
        "../check",
        "reconciliation-create",
        "recovery-review-create",
        "management-command",
    ] {
        assert_eq!(
            name.parse::<Operation>(),
            Err(ContractError::UnknownOperation)
        );
    }
}
#[test]
fn router_preserves_typed_errors_and_never_returns_a_trusted_response() {
    let value =
        json!({"protocolVersion":1,"requestId":"request-1","channel":"stable","channelRevision":0});
    let document =
        Document::parse(&serde_json::to_vec(&value).unwrap(), InputKind::Metadata).unwrap();
    let result = document.assert_supplied_client_response_contract(Operation::ChannelChange);
    assert_eq!(
        result,
        Err(ContractError::ChannelChange(
            ChannelChangeResponseError::InvalidContract
        ))
    );
    assert_eq!(result.unwrap_err().to_string(), "API_CONTRACT_INVALID");
    let mut repaired = value;
    repaired["channelRevision"] = json!(1);
    assert_eq!(
        Document::parse(&serde_json::to_vec(&repaired).unwrap(), InputKind::Metadata)
            .unwrap()
            .assert_supplied_client_response_contract(Operation::ChannelChange),
        Ok(())
    );
    assert_eq!(
        ContractError::Json(Error::DuplicateKey).to_string(),
        Error::DuplicateKey.to_string()
    );
    assert_eq!(
        ContractError::UnknownOperation.to_string(),
        "API_OPERATION_UNKNOWN"
    );
}
#[test]
fn actual_original_inventory_and_all_cross_operation_acceptance_and_errors_match() {
    let original = fixture();
    let expected: BTreeSet<_> = original["operations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap())
        .collect();
    let actual: BTreeSet<_> = Operation::ALL.into_iter().map(|op| op.as_str()).collect();
    assert_eq!(actual, expected);
    assert_eq!(actual.len(), 26);
    let management = original["management"].as_array().unwrap();
    assert_eq!(management.len(), 11);
    for name in management {
        assert_eq!(
            name.as_str().unwrap().parse::<Operation>(),
            Err(ContractError::UnknownOperation)
        );
    }
    let cases = original["cases"].as_array().unwrap();
    assert!(cases.len() >= 773);
    let mut positives = BTreeSet::new();
    for c in cases {
        let op = c["operation"]
            .as_str()
            .unwrap()
            .parse::<Operation>()
            .unwrap();
        let bytes = STANDARD.decode(c["bytes"].as_str().unwrap()).unwrap();
        let result = Document::parse(&bytes, InputKind::Metadata)
            .map_err(ContractError::Json)
            .and_then(|document| document.assert_supplied_client_response_contract(op));
        assert_eq!(
            result.is_ok(),
            c["accepted"].as_bool().unwrap(),
            "{} {}",
            c["operation"],
            c["name"]
        );
        if c["name"] == "complete-positive" {
            positives.insert(op.as_str());
        }
        if let Err(error) = result {
            assert_eq!(
                error.to_string(),
                c["code"].as_str().unwrap(),
                "{} {}",
                c["operation"],
                c["name"]
            );
        }
    }
    assert_eq!(positives, expected);
    eprintln!(
        "matched {} actual original cases over 26 client and 11 excluded management operations",
        cases.len()
    );
}

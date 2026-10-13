use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use pinvou_upgrade_codec::{
    Document, Error, InputKind, PurposeResponseError as ResponseError,
    PurposeResponseOperation as Operation,
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
fn ops() -> [(Operation, &'static str, &'static str); 6] {
    [
        (
            Operation::ValidatePreinstall,
            "validate-preinstall",
            "authorization-preinstall",
        ),
        (
            Operation::ValidateInstall,
            "validate-install",
            "authorization-install",
        ),
        (
            Operation::ValidateActivate,
            "validate-activate",
            "authorization-activate",
        ),
        (
            Operation::ConsumePreinstall,
            "consume-preinstall",
            "preinstall-transaction-event",
        ),
        (
            Operation::ConsumeInstall,
            "consume-install",
            "install-transaction-event",
        ),
        (
            Operation::ConsumeActivate,
            "consume-activate",
            "activate-transaction-event",
        ),
    ]
}
fn consuming(op: Operation) -> bool {
    matches!(
        op,
        Operation::ConsumePreinstall | Operation::ConsumeInstall | Operation::ConsumeActivate
    )
}
fn field(op: Operation) -> &'static str {
    if consuming(op) {
        "transactionCredential"
    } else {
        "authorization"
    }
}
fn base(op: Operation, role: &str) -> Value {
    let c = original(role);
    let mut b = json!({"protocolVersion":1,"requestId":"request-1","authorizationRevision":0});
    b[field(op)] = c;
    if consuming(op) {
        b["authorizationState"] = json!("consumed");
        b["transactionId"] = b[field(op)]["signed"]["transactionId"].clone();
    }
    b
}
fn check(v: &Value, op: Operation) -> Result<(), ResponseError> {
    Document::parse(&serde_json::to_vec(v).unwrap(), InputKind::Metadata)
        .unwrap()
        .assert_supplied_purpose_response_contract(op)
}
#[test]
fn six_exact_operations_isolate_credential_roles_transaction_binding_and_independent_revision() {
    for (op, _, role) in ops() {
        let b = base(op, role);
        assert_eq!(check(&b, op), Ok(()));
        for (other, _, _) in ops() {
            if other != op {
                assert_eq!(check(&b, other), Err(ResponseError::InvalidContract));
            }
        }
        for (_, _, other_role) in ops() {
            if other_role != role {
                let mut v = b.clone();
                v[field(op)] = original(other_role);
                assert_eq!(check(&v, op), Err(ResponseError::InvalidContract));
            }
        }
        let mut v = b.clone();
        v["authorizationRevision"] = json!(9007199254740991u64);
        v["requestId"] = json!("independent-request");
        assert_eq!(check(&v, op), Ok(()));
        if consuming(op) {
            v["transactionId"] = json!("other-transaction");
            assert_ne!(v["transactionId"], b["transactionId"]);
            assert_eq!(check(&v, op), Err(ResponseError::BindingMismatch));
            v["transactionId"] = b["transactionId"].clone();
            assert_eq!(check(&v, op), Ok(()));
        }
    }
}
#[test]
fn supplied_semantics_and_signature_data_do_not_approve_an_authorization_or_durable_consume_outcome()
 {
    for (op, _, role) in ops() {
        let mut v = base(op, role);
        let f = field(op);
        v[f]["signed"]["nbf"] = json!(v[f]["signed"]["iat"].as_u64().unwrap() + 1);
        v[f]["signatures"][0]["signature"] = json!("A".repeat(86));
        let sig = v[f]["signatures"][0].clone();
        v[f]["signatures"].as_array_mut().unwrap().push(sig);
        assert_eq!(check(&v, op), Ok(()));
        v[f]["signed"]["nbf"] = v[f]["signed"]["exp"].clone();
        assert!(matches!(check(&v, op), Err(ResponseError::Semantics(_))));
    }
}
#[test]
fn each_credential_and_complete_raw_response_retain_original_resource_and_duplicate_limits() {
    for (op, _, role) in ops() {
        let mut v = base(op, role);
        let f = field(op);
        let sig = v[f]["signatures"][0].clone();
        while Document::parse(&serde_json::to_vec(&v[f]).unwrap(), InputKind::Metadata)
            .unwrap()
            .canonical_bytes()
            .unwrap()
            .len()
            <= 65536
        {
            v[f]["signatures"].as_array_mut().unwrap().push(sig.clone());
        }
        let last = v[f]["signatures"].as_array_mut().unwrap().pop().unwrap();
        assert_eq!(check(&v, op), Ok(()));
        v[f]["signatures"].as_array_mut().unwrap().push(last);
        assert_eq!(check(&v, op), Err(ResponseError::CredentialTooLarge));
        let v = base(op, role);
        let mut raw = serde_json::to_vec_pretty(&v).unwrap();
        raw.resize(1024 * 1024, b' ');
        assert_eq!(
            Document::parse(&raw, InputKind::Metadata)
                .unwrap()
                .assert_supplied_purpose_response_contract(op),
            Ok(())
        );
        raw.push(b' ');
        assert_eq!(
            Document::parse(&raw, InputKind::Metadata).err(),
            Some(Error::InputLimit)
        );
        let raw = serde_json::to_string(&v).unwrap().replacen(
            "\"requestId\":\"request-1\"",
            "\"requestId\":\"request-1\",\"requestId\":\"request-1\"",
            1,
        );
        assert_eq!(
            Document::parse(raw.as_bytes(), InputKind::Metadata).err(),
            Some(Error::DuplicateKey)
        );
    }
}
#[test]
fn actual_complete_original_six_api_contracts_match_nested_required_scalar_and_isolated_controls() {
    let reference = root().join("specs/upgrade/v1/reference/api-registry.mjs");
    let output = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            include_str!("purpose-response-oracle.mjs"),
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
    assert!(cases.len() > 3000);
    for c in &cases {
        let op = ops()
            .into_iter()
            .find(|(_, name, _)| c["operation"] == *name)
            .unwrap()
            .0;
        let bytes = STANDARD.decode(c["bytes"].as_str().unwrap()).unwrap();
        let actual = Document::parse(&bytes, InputKind::Metadata)
            .and_then(|d| {
                d.assert_supplied_purpose_response_contract(op)
                    .map_err(|_| Error::InvalidJson)
            })
            .is_ok();
        assert_eq!(
            actual,
            c["accepted"].as_bool().unwrap(),
            "{} {}",
            c["operation"],
            c["name"]
        );
    }
    eprintln!(
        "{} actual original six-purpose response cases passed",
        cases.len()
    );
}

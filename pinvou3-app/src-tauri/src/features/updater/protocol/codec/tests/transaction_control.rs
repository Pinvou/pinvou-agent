use base64::{Engine, engine::general_purpose::STANDARD};
use pinvou_upgrade_codec::{
    Document, Error, InputKind, TransactionControlError as ContractError,
    TransactionControlOperation as Operation,
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
fn operations() -> [(Operation, &'static str); 7] {
    [
        (Operation::ConsumeStatus, "consume-status"),
        (Operation::CancelPreinstall, "cancel-preinstall"),
        (Operation::CancelInstall, "cancel-install"),
        (Operation::CancelActivate, "cancel-activate"),
        (Operation::ReviewPreinstall, "execution-review-preinstall"),
        (Operation::ReviewInstall, "execution-review-install"),
        (Operation::ReviewActivate, "execution-review-activate"),
    ]
}
fn reviewing(op: Operation) -> bool {
    matches!(
        op,
        Operation::ReviewPreinstall | Operation::ReviewInstall | Operation::ReviewActivate
    )
}
fn reference() -> Value {
    json!({"recordKind":"transaction","recordId":"record-1","revision":1,"sha256":"a".repeat(64)})
}
fn baseline(op: Operation) -> Value {
    let mut v = json!({"protocolVersion":1,"requestId":"request-1"});
    if reviewing(op) {
        v["transactionId"] = json!("transaction-1");
        v["transactionRevision"] = json!(0);
        v["reviewedAt"] = json!(0);
        v["currentReadSet"] = json!([reference()]);
        v["verifiedObjectSha256"] = json!("a".repeat(64));
    } else if op == Operation::ConsumeStatus {
        v["state"] = json!("consumed");
        v["transactionId"] = json!("transaction-1");
    } else {
        v["authorizationState"] = json!("consumed");
        v["authorizationRevision"] = json!(0);
    }
    v
}
fn check(v: &Value, op: Operation) -> Result<(), ContractError> {
    Document::parse(&serde_json::to_vec(v).unwrap(), InputKind::Metadata)
        .map_err(ContractError::Json)?
        .assert_supplied_transaction_control_contract(op)
}
#[test]
fn seven_responses_keep_status_nullability_and_only_schema_correlation() {
    for (op, _) in operations() {
        let b = baseline(op);
        assert_eq!(check(&b, op), Ok(()));
        let mut extra = b.clone();
        extra["extra"] = Value::Null;
        assert_eq!(check(&extra, op), Err(ContractError::InvalidContract));
        let mut independent = b.clone();
        independent["requestId"] = json!("other-request");
        assert_eq!(check(&independent, op), Ok(()));
        if reviewing(op) {
            independent["reviewedAt"] = json!(9007199254740991_u64);
            independent["transactionRevision"] = json!(9007199254740991_u64);
            independent["verifiedObjectSha256"] = json!("b".repeat(64));
            assert_eq!(check(&independent, op), Ok(()));
        } else if op == Operation::ConsumeStatus {
            for state in ["cancelled", "expired"] {
                let mut v = b.clone();
                v["state"] = json!(state);
                assert_eq!(check(&v, op), Err(ContractError::InvalidContract));
                v["transactionId"] = Value::Null;
                assert_eq!(check(&v, op), Ok(()));
            }
            let mut null = b.clone();
            null["transactionId"] = Value::Null;
            assert_eq!(check(&null, op), Err(ContractError::InvalidContract));
            let mut pending = b.clone();
            pending["state"] = json!("pending");
            assert_eq!(check(&pending, op), Err(ContractError::InvalidContract));
        } else {
            for state in ["cancelled", "expired"] {
                independent["authorizationState"] = json!(state);
                independent["authorizationRevision"] = json!(9007199254740991_u64);
                assert_eq!(check(&independent, op), Ok(()));
            }
        }
        // Cancellation purposes share a Schema, as do review purposes.
        // Purpose isolation must not invent distinctions absent from the contract.
        for (other, _) in operations() {
            let same_family = reviewing(op) == reviewing(other)
                && (op == Operation::ConsumeStatus) == (other == Operation::ConsumeStatus);
            assert_eq!(check(&b, other).is_ok(), same_family, "{op:?}/{other:?}");
        }
    }
}
#[test]
fn review_set_compares_whole_objects_with_original_container_bounds() {
    for (op, _) in operations().into_iter().filter(|(op, _)| reviewing(*op)) {
        let b = baseline(op);
        let mut v = b.clone();
        v["currentReadSet"] = json!([reference(), reference()]);
        assert_eq!(check(&v, op), Err(ContractError::InvalidContract));
        for key in ["revision", "sha256"] {
            let mut second = reference();
            second[key] = if key == "revision" {
                json!(2)
            } else {
                json!("b".repeat(64))
            };
            v["currentReadSet"] = json!([reference(), second]);
            assert_eq!(check(&v, op), Ok(()));
        }
        v["currentReadSet"] = json!([
            {"sha256":"a".repeat(64),"revision":1,"recordId":"record-1","recordKind":"transaction"},
            reference()]);
        assert_eq!(check(&v, op), Err(ContractError::InvalidContract));
        let raw=serde_json::to_string(&b).unwrap().replace(
            &serde_json::to_string(&b["currentReadSet"]).unwrap(),
            r#"[{"recordKind":"transaction","recordId":"record-1","revision":0,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},{"recordKind":"transaction","recordId":"record-1","revision":-0,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]"#);
        assert_eq!(
            Document::parse(raw.as_bytes(), InputKind::Metadata)
                .unwrap()
                .assert_supplied_transaction_control_contract(op),
            Err(ContractError::InvalidContract)
        );
        v["currentReadSet"] = json!([]);
        assert_eq!(check(&v, op), Err(ContractError::InvalidContract));
        let distinct: Vec<_> = (0..4096)
            .map(|i| {
                let mut r = reference();
                r["recordId"] = json!(format!("record-{i}"));
                r
            })
            .collect();
        v["currentReadSet"] = json!(distinct);
        assert_eq!(check(&v, op), Ok(()));
        v["currentReadSet"]
            .as_array_mut()
            .unwrap()
            .push(reference());
        assert_eq!(
            Document::parse(&serde_json::to_vec(&v).unwrap(), InputKind::Metadata).err(),
            Some(Error::MemberLimit)
        );
        assert_eq!(check(&b, op), Ok(()));
    }
}
#[test]
fn all_response_raw_limits_and_duplicate_members_precede_contract() {
    for (op, _) in operations() {
        let b = baseline(op);
        let bytes = serde_json::to_vec(&b).unwrap();
        let mut at_limit = bytes.clone();
        at_limit.resize(1024 * 1024, b' ');
        assert_eq!(
            Document::parse(&at_limit, InputKind::Metadata)
                .unwrap()
                .assert_supplied_transaction_control_contract(op),
            Ok(())
        );
        at_limit.push(b' ');
        assert_eq!(
            Document::parse(&at_limit, InputKind::Metadata).err(),
            Some(Error::InputLimit)
        );
        let duplicate =
            String::from_utf8(bytes)
                .unwrap()
                .replacen("{", r#"{"requestId":"duplicate","#, 1);
        assert_eq!(
            Document::parse(duplicate.as_bytes(), InputKind::Metadata).err(),
            Some(Error::DuplicateKey)
        );
    }
}
#[test]
fn actual_complete_original_node_transaction_control_contract_matches_mutations() {
    let reference = root().join("specs/upgrade/v1/reference/canonical-json.mjs");
    let result = Command::new("node")
        .arg("--input-type=module")
        .arg("-e")
        .arg(include_str!("transaction-control-response-oracle.mjs"))
        .arg(reference)
        .output()
        .expect("actual Node22 contract oracle is required");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let cases: Vec<Value> = serde_json::from_slice(&result.stdout).unwrap();
    assert!(cases.len() >= 434, "oracle coverage lost");
    let mut count = [0; 7];
    for case in &cases {
        let (index, (op, _)) = operations()
            .into_iter()
            .enumerate()
            .find(|(_, (_, name))| *name == case["operation"].as_str().unwrap())
            .unwrap();
        count[index] += 1;
        let bytes = STANDARD.decode(case["bytes"].as_str().unwrap()).unwrap();
        let accepted = Document::parse(&bytes, InputKind::Metadata)
            .ok()
            .is_some_and(|d| d.assert_supplied_transaction_control_contract(op).is_ok());
        assert_eq!(
            accepted,
            case["accepted"].as_bool().unwrap(),
            "{} {}",
            case["operation"],
            case["name"]
        );
    }
    assert!(count.into_iter().all(|n| n >= 38));
    eprintln!(
        "matched {} actual original transaction-control API cases",
        cases.len()
    );
}

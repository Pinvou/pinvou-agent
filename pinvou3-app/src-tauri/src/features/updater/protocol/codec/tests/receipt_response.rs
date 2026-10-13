use base64::{Engine, engine::general_purpose::STANDARD};
use pinvou_upgrade_codec::{
    Document, Error, InputKind, ReceiptResponseError as ContractError,
    ReceiptResponseOperation as Operation,
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
fn operations() -> [(Operation, &'static str); 8] {
    [
        (Operation::TelemetryEvents, "events-telemetry-event"),
        (
            Operation::PreinstallEvents,
            "events-preinstall-transaction-event",
        ),
        (Operation::InstallEvents, "events-install-transaction-event"),
        (
            Operation::ActivateEvents,
            "events-activate-transaction-event",
        ),
        (
            Operation::ReconciliationEvents,
            "events-reconciliation-event",
        ),
        (
            Operation::RecoveryReviewEvents,
            "events-recovery-review-event",
        ),
        (Operation::ReconciliationAction, "reconciliation-action"),
        (Operation::RecoveryReviewAction, "recovery-review-action"),
    ]
}
fn events(op: Operation) -> bool {
    !matches!(
        op,
        Operation::ReconciliationAction | Operation::RecoveryReviewAction
    )
}
fn baseline(op: Operation) -> Value {
    let mut v = json!({"protocolVersion":1,"requestId":"request-1"});
    if events(op) {
        v["acknowledgements"] = json!([{"eventId":"event-1","disposition":"committed"}]);
    } else {
        v["taskId"] = json!("task-1");
        v["taskRevision"] = json!(0);
        v["disposition"] = json!("recorded");
    }
    v
}
fn check(v: &Value, op: Operation) -> Result<(), ContractError> {
    Document::parse(&serde_json::to_vec(v).unwrap(), InputKind::Metadata)
        .map_err(ContractError::Json)?
        .assert_supplied_receipt_response_contract(op)
}
#[test]
fn eight_receipts_keep_original_same_family_acceptance_and_disposition_domain() {
    for (op, _) in operations() {
        let b = baseline(op);
        assert_eq!(check(&b, op), Ok(()));
        for (other, _) in operations() {
            assert_eq!(check(&b, other).is_ok(), events(op) == events(other));
        }
        let mut v = b.clone();
        v["requestId"] = json!("other-request");
        assert_eq!(check(&v, op), Ok(()));
        v = b.clone();
        v["extra"] = Value::Null;
        assert_eq!(check(&v, op), Err(ContractError::InvalidContract));
        for disposition in if events(op) {
            ["committed", "replayed", "late-diagnostic"]
        } else {
            ["recorded", "protection-retained", "reconciled"]
        } {
            v = b.clone();
            if events(op) {
                v["acknowledgements"][0]["disposition"] = json!(disposition);
            } else {
                v["disposition"] = json!(disposition);
                v["taskRevision"] = json!(9007199254740991_u64);
            }
            assert_eq!(check(&v, op), Ok(()));
        }
        if events(op) {
            v["acknowledgements"][0]["disposition"] = json!("pending");
        } else {
            v["disposition"] = json!("approved");
        }
        assert_eq!(check(&v, op), Err(ContractError::InvalidContract));
        assert_eq!(check(&b, op), Ok(()));
    }
}
#[test]
fn event_receipts_allow_original_duplicates_but_keep_nonempty_container_bounds() {
    for (op, _) in operations().into_iter().filter(|(op, _)| events(*op)) {
        let b = baseline(op);
        let ack = b["acknowledgements"][0].clone();
        let mut v = b.clone();
        v["acknowledgements"] = json!([ack.clone(), ack.clone()]);
        assert_eq!(check(&v, op), Ok(()));
        v["acknowledgements"][1]["disposition"] = json!("late-diagnostic");
        assert_eq!(check(&v, op), Ok(()));
        v["acknowledgements"] = json!(vec![ack.clone(); 4096]);
        assert_eq!(check(&v, op), Ok(()));
        v["acknowledgements"].as_array_mut().unwrap().push(ack);
        assert_eq!(
            Document::parse(&serde_json::to_vec(&v).unwrap(), InputKind::Metadata).err(),
            Some(Error::MemberLimit)
        );
        v["acknowledgements"] = json!([]);
        assert_eq!(check(&v, op), Err(ContractError::InvalidContract));
        assert_eq!(check(&b, op), Ok(()));
    }
}
#[test]
fn all_receipt_raw_limits_and_duplicate_members_precede_schema() {
    for (op, _) in operations() {
        let b = baseline(op);
        let mut bytes = serde_json::to_vec(&b).unwrap();
        bytes.resize(1024 * 1024, b' ');
        assert_eq!(
            Document::parse(&bytes, InputKind::Metadata)
                .unwrap()
                .assert_supplied_receipt_response_contract(op),
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
}
#[test]
fn actual_complete_original_receipt_contract_matches_all_fields_and_boundaries() {
    let output = Command::new("node")
        .args([
            "--input-type=module",
            "-e",
            include_str!("receipt-response-oracle.mjs"),
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
    assert!(cases.len() >= 358);
    let mut counts = [0; 8];
    for c in &cases {
        let (index, (op, _)) = operations()
            .into_iter()
            .enumerate()
            .find(|(_, (_, name))| c["operation"] == *name)
            .unwrap();
        counts[index] += 1;
        let bytes = STANDARD.decode(c["bytes"].as_str().unwrap()).unwrap();
        let actual = Document::parse(&bytes, InputKind::Metadata)
            .ok()
            .is_some_and(|d| d.assert_supplied_receipt_response_contract(op).is_ok());
        assert_eq!(
            actual,
            c["accepted"].as_bool().unwrap(),
            "{} {}",
            c["operation"],
            c["name"]
        );
    }
    assert!(counts.into_iter().all(|n| n >= 40));
    eprintln!(
        "matched {} actual original receipt response API cases",
        cases.len()
    );
}

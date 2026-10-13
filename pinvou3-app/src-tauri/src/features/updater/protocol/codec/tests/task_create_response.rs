use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use pinvou_upgrade_codec::{
    Document, Error, InputKind, TaskCreateResponseError as ContractError,
    TaskCreateResponseOperation as Operation, assert_supplied_credential_semantics,
    decode_canonical_envelope,
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
fn operations() -> [(Operation, &'static str, &'static str); 2] {
    [
        (
            Operation::Reconciliation,
            "reconciliation-create",
            "reconciliation",
        ),
        (
            Operation::RecoveryReview,
            "recovery-review-create",
            "recovery-review",
        ),
    ]
}
fn original(name: &str) -> Value {
    let v: Value = serde_json::from_slice(
        &std::fs::read(root().join("specs/upgrade/v1/vectors/crypto/envelopes.json")).unwrap(),
    )
    .unwrap();
    let vector = v["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == name)
        .unwrap();
    serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap()
}
fn baseline(prefix: &str) -> Value {
    let action = original(&format!("{prefix}-action"));
    let event = original(&format!("{prefix}-event"));
    json!({"protocolVersion":1,"requestId":"request-1","task":action["signed"]["task"],
        "actionCredential":action,"eventCredential":event})
}
fn check(v: &Value, op: Operation) -> Result<(), ContractError> {
    Document::parse(&serde_json::to_vec(v).unwrap(), InputKind::Metadata)
        .map_err(ContractError::Json)?
        .assert_supplied_task_create_response_contract(op)
}
fn preflight(v: &Value) {
    for field in ["actionCredential", "eventCredential"] {
        let envelope = decode_canonical_envelope(
            &serde_json::to_vec(&v[field]).unwrap(),
            InputKind::Credential,
        )
        .unwrap()
        .check_full_schema()
        .unwrap();
        let signed = &v[field]["signed"];
        let time = signed["iat"]
            .as_u64()
            .unwrap()
            .max(signed["nbf"].as_u64().unwrap());
        let time = Document::parse(time.to_string().as_bytes(), InputKind::Metadata)
            .unwrap()
            .protocol_time_shape()
            .unwrap();
        assert_eq!(
            assert_supplied_credential_semantics(&envelope, &time),
            Ok(())
        );
    }
}
#[test]
fn two_complete_contracts_bind_tasks_full_scope_and_three_identities() {
    for (op, _, prefix) in operations() {
        let b = baseline(prefix);
        assert_eq!(check(&b, op), Ok(()));
        preflight(&b);
        for field in ["actionCredential", "eventCredential"] {
            let mut v = b.clone();
            v[field]["signed"]["task"]["revision"] = json!(2);
            assert_ne!(v, b);
            preflight(&v);
            assert_eq!(check(&v, op), Err(ContractError::BindingMismatch));
            v["task"]["revision"] = json!(2);
            v["actionCredential"]["signed"]["task"]["revision"] = json!(2);
            v["eventCredential"]["signed"]["task"]["revision"] = json!(2);
            preflight(&v);
            assert_eq!(check(&v, op), Ok(()));
        }
        for field in ["product", "component", "installId"] {
            let mut v = b.clone();
            v["eventCredential"]["signed"][field] = json!(format!("other-{field}"));
            assert_ne!(v, b);
            preflight(&v);
            assert_eq!(check(&v, op), Err(ContractError::BindingMismatch));
            v["actionCredential"]["signed"][field] = v["eventCredential"]["signed"][field].clone();
            preflight(&v);
            assert_eq!(check(&v, op), Ok(()));
        }
        for (field, replacement) in [
            ("installationScopeId", json!("other-scope")),
            ("channel", json!("beta")),
            ("channelRevision", json!(2)),
        ] {
            let mut v = b.clone();
            v["eventCredential"]["signed"]["scope"][field] = replacement;
            assert_ne!(v, b);
            preflight(&v);
            assert_eq!(check(&v, op), Err(ContractError::BindingMismatch));
            v["actionCredential"]["signed"]["scope"][field] =
                v["eventCredential"]["signed"]["scope"][field].clone();
            preflight(&v);
            assert_eq!(check(&v, op), Ok(()));
        }
        for (other, _, _) in operations() {
            assert_eq!(check(&b, other).is_ok(), op == other);
        }
        let mut extra = b.clone();
        extra["extra"] = Value::Null;
        assert_eq!(check(&extra, op), Err(ContractError::InvalidContract));
        let mut independent = b.clone();
        independent["requestId"] = json!("another-request");
        assert_eq!(check(&independent, op), Ok(()));
    }
}
#[test]
fn task_contract_signatures_and_supplied_times_do_not_grant_action_rights() {
    for (op, _, prefix) in operations() {
        for field in ["actionCredential", "eventCredential"] {
            let mut v = baseline(prefix);
            v[field]["signed"]["nbf"] = json!(v[field]["signed"]["iat"].as_u64().unwrap() + 1);
            v[field]["signatures"][0]["signature"] = json!("A".repeat(86));
            let sig = v[field]["signatures"][0].clone();
            v[field]["signatures"].as_array_mut().unwrap().push(sig);
            assert_eq!(check(&v, op), Ok(()));
            v[field]["signed"]["nbf"] = v[field]["signed"]["exp"].clone();
            assert!(matches!(check(&v, op), Err(ContractError::Semantics(_))));
        }
    }
}
#[test]
fn each_task_credential_and_whole_response_keep_original_byte_profiles() {
    for (op, _, prefix) in operations() {
        for field in ["actionCredential", "eventCredential"] {
            let mut v = baseline(prefix);
            let sig = v[field]["signatures"][0].clone();
            while Document::parse(&serde_json::to_vec(&v[field]).unwrap(), InputKind::Metadata)
                .unwrap()
                .canonical_bytes()
                .unwrap()
                .len()
                <= 65536
            {
                v[field]["signatures"]
                    .as_array_mut()
                    .unwrap()
                    .push(sig.clone());
            }
            let last = v[field]["signatures"]
                .as_array_mut()
                .unwrap()
                .pop()
                .unwrap();
            assert_eq!(check(&v, op), Ok(()));
            v[field]["signatures"].as_array_mut().unwrap().push(last);
            assert_eq!(check(&v, op), Err(ContractError::CredentialTooLarge));
        }
        let mut raw = serde_json::to_vec(&baseline(prefix)).unwrap();
        raw.resize(1024 * 1024, b' ');
        assert_eq!(
            Document::parse(&raw, InputKind::Metadata)
                .unwrap()
                .assert_supplied_task_create_response_contract(op),
            Ok(())
        );
        raw.push(b' ');
        assert_eq!(
            Document::parse(&raw, InputKind::Metadata).err(),
            Some(Error::InputLimit)
        );
        let duplicate = serde_json::to_string(&baseline(prefix)).unwrap().replacen(
            "{",
            r#"{"requestId":"duplicate","#,
            1,
        );
        assert_eq!(
            Document::parse(duplicate.as_bytes(), InputKind::Metadata).err(),
            Some(Error::DuplicateKey)
        );
    }
}
#[test]
fn actual_complete_original_node_task_response_contract_matches_nested_mutations() {
    let output = Command::new("node")
        .args([
            "--input-type=module",
            "-e",
            include_str!("task-response-oracle.mjs"),
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
    assert!(cases.len() >= 5694);
    let mut count = [0; 2];
    for c in &cases {
        let (index, (op, _, _)) = operations()
            .into_iter()
            .enumerate()
            .find(|(_, (_, name, _))| c["operation"] == *name)
            .unwrap();
        count[index] += 1;
        let bytes = STANDARD.decode(c["bytes"].as_str().unwrap()).unwrap();
        let actual = Document::parse(&bytes, InputKind::Metadata)
            .ok()
            .is_some_and(|d| d.assert_supplied_task_create_response_contract(op).is_ok());
        assert_eq!(
            actual,
            c["accepted"].as_bool().unwrap(),
            "{} {}",
            c["operation"],
            c["name"]
        );
    }
    assert!(count.into_iter().all(|n| n > 2000));
    eprintln!(
        "matched {} actual original task-create response API cases",
        cases.len()
    );
}

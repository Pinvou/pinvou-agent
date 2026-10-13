use base64::{Engine, engine::general_purpose::STANDARD};
use pinvou_upgrade_codec::{
    ClientRequestError as ContractError, ClientRequestOperation as Op, Document, Error, InputKind,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(7)
        .unwrap()
        .to_owned()
}
fn original() -> &'static Value {
    static FIXTURE: OnceLock<Value> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let output = Command::new("node")
            .args([
                "--input-type=module",
                "-e",
                include_str!("client-request-oracle.mjs"),
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
    })
}
fn baseline(op: Op) -> Value {
    let case = original()["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["operation"] == op.as_str() && c["name"] == "complete-positive")
        .unwrap();
    serde_json::from_slice(&STANDARD.decode(case["bytes"].as_str().unwrap()).unwrap()).unwrap()
}
fn check(v: &Value, op: Op) -> Result<(), ContractError> {
    Document::parse(&serde_json::to_vec(v).unwrap(), InputKind::Metadata)
        .map_err(ContractError::Json)?
        .assert_supplied_client_request_contract(op)
}
#[test]
fn exact_prepare_modes_recovery_spelling_and_scope_have_precise_rejections_and_restores() {
    for op in [
        Op::ValidatePreinstall,
        Op::ValidateInstall,
        Op::ValidateActivate,
    ] {
        let b = baseline(op);
        assert_eq!(check(&b, op), Ok(()));
        let mut v = b.clone();
        v["freezeEpoch"] = if op == Op::ValidatePreinstall {
            json!(1)
        } else {
            Value::Null
        };
        assert_eq!(check(&v, op), Err(ContractError::PurposeMismatch));
        assert_eq!(check(&b, op), Ok(()));
    }
    let b = baseline(Op::ConsumeStatus);
    let mut v = b.clone();
    v["recoverySecret"] = json!("A".repeat(42) + "B");
    assert_eq!(
        check(&v, Op::ConsumeStatus),
        Err(ContractError::InvalidContract)
    );
    assert_eq!(check(&b, Op::ConsumeStatus), Ok(()));
    let b = baseline(Op::Refresh);
    let mut v = b.clone();
    v["scope"]["installationScopeId"] = json!("other-scope");
    assert_eq!(check(&v, Op::Refresh), Err(ContractError::ScopeMismatch));
    assert_eq!(check(&b, Op::Refresh), Ok(()));
}
#[test]
fn request_contract_does_not_apply_later_post_auth_consume_hash_binding() {
    for op in [
        Op::ConsumePreinstall,
        Op::ConsumeInstall,
        Op::ConsumeActivate,
    ] {
        let c = original()["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| {
                c["operation"] == op.as_str()
                    && c["name"] == "later-envelope-hash-independent-positive"
            })
            .unwrap();
        let v: Value =
            serde_json::from_slice(&STANDARD.decode(c["bytes"].as_str().unwrap()).unwrap())
                .unwrap();
        assert_ne!(
            v["semanticRequest"]["authorizationEnvelopeSha256"],
            baseline(op)["semanticRequest"]["authorizationEnvelopeSha256"]
        );
        assert_eq!(check(&v, op), Ok(()));
    }
}
#[test]
fn every_attached_credential_and_complete_request_keep_original_raw_profiles() {
    for op in Op::ALL {
        let b = baseline(op);
        for field in ["decision", "authorization", "credential"] {
            if b.get(field).is_none() {
                continue;
            }
            let mut v = b.clone();
            let sig = v[field]["signatures"][0].clone();
            let canonical_length = |value: &Value| {
                Document::parse(&serde_json::to_vec(value).unwrap(), InputKind::Metadata)
                    .unwrap()
                    .canonical_bytes()
                    .unwrap()
                    .len()
            };
            let initial = canonical_length(&v[field]);
            let per_member = canonical_length(&sig) + 1;
            let maximum = 1 + (65536 - initial) / per_member;
            assert!((1..4096).contains(&maximum));
            v[field]["signatures"] = json!(vec![sig.clone(); maximum]);
            assert!(canonical_length(&v[field]) <= 65536);
            assert_eq!(check(&v, op), Ok(()));
            v[field]["signatures"].as_array_mut().unwrap().push(sig);
            assert!(canonical_length(&v[field]) > 65536);
            assert_eq!(check(&v, op), Err(ContractError::CredentialTooLarge));
        }
        let mut raw = serde_json::to_vec(&b).unwrap();
        raw.resize(1024 * 1024, b' ');
        assert_eq!(
            Document::parse(&raw, InputKind::Metadata)
                .unwrap()
                .assert_supplied_client_request_contract(op),
            Ok(())
        );
        raw.push(b' ');
        assert_eq!(
            Document::parse(&raw, InputKind::Metadata).err(),
            Some(Error::InputLimit)
        );
        let duplicate =
            serde_json::to_string(&b)
                .unwrap()
                .replacen("{", r#"{"protocolVersion":1,"#, 1);
        assert_eq!(
            Document::parse(duplicate.as_bytes(), InputKind::Metadata).err(),
            Some(Error::DuplicateKey)
        );
    }
}
#[test]
fn complete_original_api_requests_match_all_surrounding_fields_guards_and_cross_operations() {
    let fixture = original();
    let expected: BTreeSet<_> = fixture["operations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        expected,
        Op::ALL.into_iter().map(|op| op.as_str()).collect()
    );
    let cases = fixture["cases"].as_array().unwrap();
    assert!(cases.len() >= 7285);
    let mut covered = BTreeSet::new();
    for c in cases {
        let op = c["operation"].as_str().unwrap().parse::<Op>().unwrap();
        let bytes = STANDARD.decode(c["bytes"].as_str().unwrap()).unwrap();
        let result = Document::parse(&bytes, InputKind::Metadata)
            .map_err(ContractError::Json)
            .and_then(|document| document.assert_supplied_client_request_contract(op));
        assert_eq!(
            result.is_ok(),
            c["accepted"].as_bool().unwrap(),
            "{} {}",
            c["operation"],
            c["name"]
        );
        if c["name"] == "complete-positive" {
            covered.insert(op.as_str());
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
    assert_eq!(covered, expected);
    eprintln!(
        "matched {} actual complete original client request cases",
        cases.len()
    );
}

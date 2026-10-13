use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use pinvou_upgrade_codec::{
    Document, Error, InputKind, PolicyLookupError, RootPolicyShape, key_descriptor,
};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
const ROLES: [&str; 20] = [
    "root",
    "timestamp",
    "snapshot",
    "target",
    "release",
    "package",
    "decision",
    "download",
    "authorization-preinstall",
    "authorization-install",
    "authorization-activate",
    "helper-cleanup",
    "telemetry-event",
    "preinstall-transaction-event",
    "install-transaction-event",
    "activate-transaction-event",
    "reconciliation-action",
    "recovery-review-action",
    "reconciliation-event",
    "recovery-review-event",
];
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(7)
        .unwrap()
        .to_owned()
}
fn wire(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
fn descriptor(number: u32) -> Value {
    let mut bytes = [0; 32];
    bytes[..4].copy_from_slice(&number.to_le_bytes());
    let text = URL_SAFE_NO_PAD.encode(bytes);
    let key = key_descriptor(&text).unwrap();
    json!({"keyId":key.key_id(),"algorithm":"ed25519","publicKey":key.public_key()})
}
fn base() -> Value {
    let keys: Vec<_> = (0..4).map(descriptor).collect();
    let policies:Vec<_>=ROLES.into_iter().map(|role|{
        let global=["root","timestamp","snapshot","release"].contains(&role);
        json!({"role":role,"component":if role=="root"{Value::Null}else{json!("app")},
            "channel":if global||role=="package"{Value::Null}else{json!("stable")},
            "targetKey":if global{Value::Null}else{json!("linux-x86_64-deb-perMachine")},
            "threshold":if role=="root"{2}else{1},
            "keyIds":if role=="root"{json!(keys[..3].iter().map(|key|key["keyId"].clone()).collect::<Vec<_>>())}else{json!([keys[3]["keyId"]])}})
    }).collect();
    json!({"protocolVersion":1,"role":"root","product":"pinvou","component":null,"scope":{},"version":1,"issuedAt":0,"expiresAt":0,"keys":keys,"roles":policies})
}
fn context(policy: &Value) -> Value {
    let mut scope = json!({"ignored":"still raw checked"});
    for field in ["channel", "targetKey"] {
        if !policy[field].is_null() {
            scope[field] = policy[field].clone();
        }
    }
    json!({"role":policy["role"],"product":"pinvou","component":policy["component"],"scope":scope,"extra":"not full claims"})
}
fn root_shape(bytes: &[u8]) -> RootPolicyShape {
    Document::parse(bytes, InputKind::Metadata)
        .unwrap()
        .root_policy_shape()
        .unwrap()
}
fn selected(root: &RootPolicyShape, bytes: &[u8]) -> Result<Vec<u8>, PolicyLookupError> {
    let context = Document::parse(bytes, InputKind::Metadata)
        .map_err(PolicyLookupError::Json)?
        .role_context_shape()?;
    root.declared_policy_for(&context)?
        .canonical_bytes()
        .map_err(PolicyLookupError::Json)
}
#[test]
fn all_twenty_roles_keep_declared_member_order_thresholds_and_exact_scope() {
    let claims = base();
    let shape = root_shape(&wire(&claims));
    let canonical = shape.canonical_bytes().to_vec();
    for policy in claims["roles"].as_array().unwrap() {
        let bytes = wire(&context(policy));
        let ctxt = Document::parse(&bytes, InputKind::Metadata)
            .unwrap()
            .role_context_shape()
            .unwrap();
        let declared = shape.declared_policy_for(&ctxt).unwrap();
        assert_eq!(
            declared.threshold(),
            policy["threshold"].as_u64().unwrap() as u16
        );
        assert_eq!(
            declared.key_ids(),
            &policy["keyIds"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| id.as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            declared.canonical_bytes().unwrap(),
            Document::parse(&wire(policy), InputKind::Metadata)
                .unwrap()
                .canonical_bytes()
                .unwrap()
        );
    }
    assert_eq!(shape.canonical_bytes(), canonical);
}
#[test]
fn same_role_scopes_never_merge_fallback_or_default_missing_component() {
    let mut claims = base();
    let policy = claims["roles"][3].clone();
    let mut beta = policy.clone();
    beta["channel"] = json!("beta");
    beta["threshold"] = json!(2);
    beta["keyIds"] = json!([claims["keys"][2]["keyId"], claims["keys"][3]["keyId"]]);
    claims["roles"].as_array_mut().unwrap().push(beta.clone());
    let shape = root_shape(&wire(&claims));
    assert_eq!(
        serde_json::from_slice::<Value>(&selected(&shape, &wire(&context(&policy))).unwrap())
            .unwrap()["threshold"],
        json!(1)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&selected(&shape, &wire(&context(&beta))).unwrap())
            .unwrap()["threshold"],
        json!(2)
    );
    let mut unspecified = context(&policy);
    unspecified["scope"]
        .as_object_mut()
        .unwrap()
        .remove("channel");
    assert_eq!(
        selected(&shape, &wire(&unspecified)).err(),
        Some(PolicyLookupError::NotDeclared)
    );
    let mut root_ctx = context(&claims["roles"][0]);
    root_ctx.as_object_mut().unwrap().remove("component");
    assert_eq!(
        selected(&shape, &wire(&root_ctx)).err(),
        Some(PolicyLookupError::InvalidContext)
    );
    let mut explicit = context(&claims["roles"][0]);
    explicit["scope"]["channel"] = Value::Null;
    explicit["scope"]["targetKey"] = Value::Null;
    assert_eq!(
        selected(&shape, &wire(&explicit)).unwrap(),
        selected(&shape, &wire(&context(&claims["roles"][0]))).unwrap()
    );
}
#[test]
fn original_bounds_duplicates_and_large_declared_policy_member_caches_are_preserved() {
    let mut claims = base();
    let mut policy = claims["roles"][3].clone();
    while claims["roles"].as_array().unwrap().len() < 4096 {
        let number = claims["roles"].as_array().unwrap().len();
        policy["component"] = json!(format!("component-{number}"));
        claims["roles"].as_array_mut().unwrap().push(policy.clone());
    }
    let shape = root_shape(&wire(&claims));
    assert_eq!(
        selected(&shape, &wire(&context(&policy))).unwrap(),
        Document::parse(&wire(&policy), InputKind::Metadata)
            .unwrap()
            .canonical_bytes()
            .unwrap()
    );
    let mut root_ctx = context(&claims["roles"][0]);
    root_ctx["extra"] = json!("x".repeat(65_537));
    assert_eq!(
        selected(&shape, &wire(&root_ctx)).err(),
        Some(PolicyLookupError::Json(Error::StringLimit))
    );
    assert_eq!(selected(&shape,br#"{"role":"root","product":"pinvou","component":null,"scope":{},"extra":0,"e\u0078tra":1}"#).err(),Some(PolicyLookupError::Json(Error::DuplicateKey)));
    let keys: Vec<_> = (0..4096).map(descriptor).collect();
    let mut large = base();
    large["keys"] = json!(keys);
    large["roles"] = json!([large["roles"][0]]);
    large["roles"][0]["keyIds"] = json!(
        keys.iter()
            .rev()
            .map(|key| key["keyId"].clone())
            .collect::<Vec<_>>()
    );
    large["roles"][0]["threshold"] = json!(4096);
    let root = root_shape(&wire(&large));
    let bytes = wire(&context(&large["roles"][0]));
    let ctxt = Document::parse(&bytes, InputKind::Metadata)
        .unwrap()
        .role_context_shape()
        .unwrap();
    let policy = root.declared_policy_for(&ctxt).unwrap();
    assert_eq!(policy.threshold(), 4096);
    assert_eq!(policy.key_ids().len(), 4096);
    assert_eq!(policy.key_ids()[0], keys[4095]["keyId"].as_str().unwrap());
    assert_eq!(
        policy.canonical_bytes().unwrap(),
        Document::parse(&wire(&large["roles"][0]), InputKind::Metadata)
            .unwrap()
            .canonical_bytes()
            .unwrap()
    );
}
#[test]
fn actual_original_private_policy_function_matches_context_subset_and_twenty_roles() {
    let original = base();
    let shape = root_shape(&wire(&original));
    let mut cases: Vec<(Vec<u8>, Vec<u8>)> = vec![];
    for policy in original["roles"].as_array().unwrap() {
        let ctxt = context(policy);
        cases.push((wire(&original), wire(&ctxt)));
        let mut nulls = ctxt.clone();
        for field in ["channel", "targetKey"] {
            if !nulls["scope"].as_object().unwrap().contains_key(field) {
                nulls["scope"][field] = Value::Null;
            }
        }
        cases.push((wire(&original), wire(&nulls)));
        for field in ["role", "product", "component", "scope"] {
            let mut missing = ctxt.clone();
            missing.as_object_mut().unwrap().remove(field);
            cases.push((wire(&original), wire(&missing)));
            for bad in [
                Value::Null,
                json!(false),
                json!(0),
                json!(""),
                json!("unknown"),
                json!([]),
                json!({}),
            ] {
                let mut changed = ctxt.clone();
                changed[field] = bad;
                cases.push((wire(&original), wire(&changed)));
            }
        }
        for field in ["channel", "targetKey"] {
            for value in [
                Value::Null,
                json!(false),
                json!(0),
                json!(""),
                json!("unknown"),
                json!([]),
                json!({}),
            ] {
                let mut changed = ctxt.clone();
                changed["scope"][field] = value;
                cases.push((wire(&original), wire(&changed)));
            }
        }
    }
    let mut multi = original.clone();
    let mut changed = multi["roles"][3].clone();
    changed["channel"] = json!("beta");
    changed["threshold"] = json!(2);
    changed["keyIds"] = json!([multi["keys"][2]["keyId"], multi["keys"][3]["keyId"]]);
    multi["roles"].as_array_mut().unwrap().push(changed.clone());
    cases.push((wire(&multi), wire(&context(&changed))));
    changed["component"] = json!("helper");
    multi["roles"].as_array_mut().unwrap().push(changed.clone());
    cases.push((wire(&multi), wire(&context(&changed))));
    let ctxt = context(&original["roles"][0]);
    let mut excess = ctxt.clone();
    excess["extra"] = json!("x".repeat(65_537));
    cases.push((wire(&original), wire(&excess)));
    cases.push((wire(&original),br#"{"role":"root","product":"pinvou","component":null,"scope":{},"extra":0,"e\u0078tra":1}"#.to_vec()));
    let mut exact = vec![b' '; 1_048_576 - wire(&ctxt).len()];
    exact.extend_from_slice(&wire(&ctxt));
    cases.push((wire(&original), exact.clone()));
    exact.push(b' ');
    cases.push((wire(&original), exact));
    let expected: Vec<_> = cases
        .iter()
        .map(|(root, ctxt)| {
            let root = Document::parse(root, InputKind::Metadata)
                .ok()
                .and_then(|doc| doc.root_policy_shape().ok());
            root.and_then(|root| selected(&root, ctxt).ok())
                .map(|bytes| json!(STANDARD.encode(bytes)))
                .unwrap_or(Value::Null)
        })
        .collect();
    let samples: Vec<_> = cases
        .iter()
        .map(|(root, ctxt)| json!([STANDARD.encode(root), STANDARD.encode(ctxt)]))
        .collect();
    let reference = root()
        .join("specs/upgrade/v1/reference/signatures.mjs")
        .to_string_lossy()
        .into_owned();
    let mut child = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            include_str!("policy-oracle.mjs"),
            &reference,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&samples).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "original declared-policy oracle unavailable"
    );
    let actual: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
        assert!(
            actual == expected,
            "declared-policy oracle mismatch at case {index}"
        );
    }
    assert_eq!(
        shape.canonical_bytes(),
        root_shape(&wire(&original)).canonical_bytes()
    );
    eprintln!(
        "{} actual original Node declared-policy cases passed",
        expected.len()
    );
}

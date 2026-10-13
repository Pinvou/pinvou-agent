use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use pinvou_upgrade_codec::{Document, InputKind, RootPolicyError, key_descriptor};
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
fn base() -> Value {
    // Public raw descriptors only. Zero/identity bytes deliberately demonstrate
    // structure acceptance, never curve validity or cryptographic Root trust.
    let keys: Vec<_> = (0..4)
        .map(|byte| {
            let text = URL_SAFE_NO_PAD.encode([byte; 32]);
            let key = key_descriptor(&text).unwrap();
            json!({"keyId":key.key_id(),"algorithm":"ed25519","publicKey":key.public_key()})
        })
        .collect();
    let ids: Vec<_> = keys[..3].iter().map(|key| key["keyId"].clone()).collect();
    json!({"protocolVersion":1,"role":"root","product":"pinvou","component":null,"scope":{},"version":1,"issuedAt":0,"expiresAt":0,
        "keys":keys,"roles":[{"role":"root","component":null,"channel":null,"targetKey":null,"threshold":2,"keyIds":ids}]})
}
fn wire(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
fn canonical(bytes: &[u8]) -> Result<Vec<u8>, RootPolicyError> {
    Document::parse(bytes, InputKind::Metadata)
        .map_err(RootPolicyError::Json)?
        .root_policy_shape()
        .map(|shape| shape.canonical_bytes().to_vec())
}
#[test]
fn root_shape_canonicalizes_supplied_claims_without_time_point_or_anchor_authority() {
    let original = base();
    let expected = canonical(&wire(&original)).unwrap();
    assert_eq!(
        canonical(&serde_json::to_vec_pretty(&original).unwrap()).unwrap(),
        expected
    );
    let mut reverse = original.clone();
    reverse["issuedAt"] = json!(99);
    reverse["expiresAt"] = json!(0);
    assert!(canonical(&wire(&reverse)).is_ok());
    for version in ["1", "1.0", "1e0", "1.00000000000000001"] {
        let raw = String::from_utf8(wire(&original))
            .unwrap()
            .replace("\"version\":1", &format!("\"version\":{version}"));
        assert_eq!(canonical(raw.as_bytes()).unwrap(), expected);
    }
    let mut envelope = json!({"signed":original,"signatures":[]});
    assert!(canonical(&wire(&envelope)).is_err());
    envelope["signed"]["roles"][0]["threshold"] = json!(1);
    assert!(canonical(&wire(&envelope["signed"])).is_err());
}
#[test]
fn root_policy_identity_is_exact_scope_and_members_are_unique_present_and_threshold_bounded() {
    let mut claims = base();
    let mut policy = claims["roles"][0].clone();
    policy["role"] = json!("timestamp");
    policy["component"] = json!("app");
    policy["threshold"] = json!(1);
    claims["roles"].as_array_mut().unwrap().push(policy.clone());
    let mut scoped = policy.clone();
    scoped["channel"] = json!("beta");
    scoped["targetKey"] = json!("macos-universal-dmg-perMachine");
    claims["roles"].as_array_mut().unwrap().push(scoped);
    assert!(canonical(&wire(&claims)).is_ok());
    claims["roles"].as_array_mut().unwrap().push(policy);
    assert!(canonical(&wire(&claims)).is_err());
    for threshold in [0, 1, 4] {
        let mut changed = base();
        changed["roles"][0]["threshold"] = json!(threshold);
        assert!(canonical(&wire(&changed)).is_err());
    }
    let mut all = base();
    all["roles"][0]["threshold"] = json!(3);
    assert!(canonical(&wire(&all)).is_ok());
    let mut duplicate = base();
    duplicate["roles"][0]["keyIds"][2] = duplicate["roles"][0]["keyIds"][1].clone();
    assert!(canonical(&wire(&duplicate)).is_err());
    let mut absent = base();
    absent["roles"][0]["keyIds"][2] = json!("f".repeat(64));
    assert!(canonical(&wire(&absent)).is_err());
}
fn resource_cases() -> Vec<(Vec<u8>, bool)> {
    let original = wire(&base());
    let mut exact = vec![b' '; 1024 * 1024 - original.len()];
    exact.extend_from_slice(&original);
    let mut excess = exact.clone();
    excess.push(b' ');
    let mut cases = vec![(exact, true), (excess, false)];
    let mut many = base();
    let keys: Vec<_> = (0_u32..4097)
        .map(|number| {
            let mut bytes = [0; 32];
            bytes[..4].copy_from_slice(&number.to_le_bytes());
            let text = URL_SAFE_NO_PAD.encode(bytes);
            let key = key_descriptor(&text).unwrap();
            json!({"keyId":key.key_id(),"algorithm":"ed25519","publicKey":key.public_key()})
        })
        .collect();
    many["roles"][0]["keyIds"] = json!(
        keys[..3]
            .iter()
            .map(|key| key["keyId"].clone())
            .collect::<Vec<_>>()
    );
    many["keys"] = json!(keys[..4096]);
    cases.push((wire(&many), true));
    many["keys"] = json!(keys);
    cases.push((wire(&many), false));
    many["keys"].as_array_mut().unwrap().pop();
    many["roles"][0]["keyIds"] = json!(
        many["keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|key| key["keyId"].clone())
            .collect::<Vec<_>>()
    );
    cases.push((wire(&many), true));
    let last = many["roles"][0]["keyIds"][0].clone();
    many["roles"][0]["keyIds"]
        .as_array_mut()
        .unwrap()
        .push(last);
    cases.push((wire(&many), false));
    let mut many = base();
    let mut policy = many["roles"][0].clone();
    policy["role"] = json!("timestamp");
    policy["threshold"] = json!(1);
    policy["keyIds"] = json!([many["keys"][0]["keyId"]]);
    for number in 1..4096 {
        policy["component"] = json!(format!("component-{number}"));
        many["roles"].as_array_mut().unwrap().push(policy.clone());
    }
    cases.push((wire(&many), true));
    policy["component"] = json!("component-4096");
    many["roles"].as_array_mut().unwrap().push(policy);
    cases.push((wire(&many), false));
    cases
}
#[test]
fn root_wire_keys_policies_and_policy_members_preserve_exact_resource_boundaries() {
    for (bytes, valid) in resource_cases() {
        assert_eq!(canonical(&bytes).is_ok(), valid);
        if !valid {
            assert!(matches!(
                canonical(&bytes),
                Err(RootPolicyError::Json(
                    pinvou_upgrade_codec::Error::InputLimit
                        | pinvou_upgrade_codec::Error::MemberLimit
                ))
            ));
        }
    }
}
#[test]
fn original_node_root_schema_and_policy_match_all_role_scopes_and_adversarial_shapes() {
    let original = base();
    let mut cases = vec![
        wire(&original),
        serde_json::to_vec_pretty(&original).unwrap(),
    ];
    let mut reverse = original.clone();
    reverse["issuedAt"] = json!(99);
    reverse["expiresAt"] = json!(0);
    cases.push(wire(&reverse));
    for role in ROLES.into_iter().skip(1) {
        for channel in [
            Value::Null,
            json!("stable"),
            json!("beta"),
            json!("internal"),
        ] {
            for target in [
                Value::Null,
                json!("windows-x86_64-nsis-perMachine"),
                json!("linux-x86_64-deb-perMachine"),
                json!("linux-arm64-deb-perMachine"),
                json!("macos-universal-dmg-perMachine"),
            ] {
                let mut claims = original.clone();
                let mut policy = claims["roles"][0].clone();
                policy["role"] = json!(role);
                policy["component"] = json!("app");
                policy["threshold"] = json!(1);
                policy["channel"] = channel.clone();
                policy["targetKey"] = target;
                claims["roles"].as_array_mut().unwrap().push(policy.clone());
                let mut other = policy;
                other["component"] = json!("helper");
                claims["roles"].as_array_mut().unwrap().push(other);
                cases.push(wire(&claims));
            }
        }
    }
    let locations = [vec![], vec!["keys", "0"], vec!["roles", "0"]];
    for path in &locations {
        let node = match path.as_slice() {
            [] => &original,
            ["keys", "0"] => &original["keys"][0],
            _ => &original["roles"][0],
        };
        for field in node.as_object().unwrap().keys() {
            let mut missing = original.clone();
            node_mut(&mut missing, path)
                .as_object_mut()
                .unwrap()
                .remove(field);
            cases.push(wire(&missing));
            for invalid in [
                Value::Null,
                json!(false),
                json!(0),
                json!(1.5),
                json!(""),
                json!("bad\n"),
                json!([]),
                json!({}),
            ] {
                let mut changed = original.clone();
                node_mut(&mut changed, path)[field] = invalid;
                cases.push(wire(&changed));
            }
        }
        let mut extra = original.clone();
        node_mut(&mut extra, path)["extra"] = Value::Null;
        cases.push(wire(&extra));
    }
    for field in ["version", "issuedAt", "expiresAt"] {
        for number in [
            "-0",
            "-1",
            "0",
            "1",
            "2",
            "1.00000000000000001",
            "9007199254740991",
            "9007199254740991.1",
            "9007199254740992",
            "1e400",
        ] {
            let raw = String::from_utf8(wire(&original)).unwrap();
            let old = if field == "version" { 1 } else { 0 };
            cases.push(
                raw.replace(
                    &format!("\"{field}\":{old}"),
                    &format!("\"{field}\":{number}"),
                )
                .into_bytes(),
            );
        }
    }
    for threshold in [
        "-0",
        "0",
        "1",
        "2",
        "3",
        "4",
        "1.5",
        "2.00000000000000001",
        "9007199254740991",
        "9007199254740992",
    ] {
        let raw = String::from_utf8(wire(&original)).unwrap();
        cases.push(
            raw.replace("\"threshold\":2", &format!("\"threshold\":{threshold}"))
                .into_bytes(),
        );
    }
    let mut no_root = original.clone();
    no_root["roles"][0]["role"] = json!("timestamp");
    no_root["roles"][0]["component"] = json!("app");
    cases.push(wire(&no_root));
    let mut duplicate = original.clone();
    let key = duplicate["keys"][0].clone();
    duplicate["keys"].as_array_mut().unwrap().push(key);
    cases.push(wire(&duplicate));
    let mut duplicate = original.clone();
    let policy = duplicate["roles"][0].clone();
    duplicate["roles"].as_array_mut().unwrap().push(policy);
    cases.push(wire(&duplicate));
    let mut duplicate = original.clone();
    duplicate["roles"][0]["keyIds"][2] = duplicate["roles"][0]["keyIds"][1].clone();
    cases.push(wire(&duplicate));
    let mut absent = original.clone();
    absent["roles"][0]["keyIds"][2] = json!("f".repeat(64));
    cases.push(wire(&absent));
    let mut too_few = original.clone();
    too_few["roles"][0]["keyIds"].as_array_mut().unwrap().pop();
    cases.push(wire(&too_few));
    for field in ["publicKey", "keyId", "algorithm"] {
        for text in [
            "",
            "not-a-key",
            &"0".repeat(64),
            &"A".repeat(43),
            &format!("{}B", "A".repeat(42)),
            &format!("{}=", "A".repeat(43)),
            "Ed25519",
        ] {
            let mut claims = original.clone();
            claims["keys"][0][field] = json!(text);
            cases.push(wire(&claims));
        }
    }
    for path in ["product", "version"] {
        let raw = String::from_utf8(wire(&original)).unwrap();
        let spelling = if path == "product" {
            "\"product\":\"pinvou\""
        } else {
            "\"version\":1"
        };
        cases.push(
            raw.replace(spelling, &format!("{spelling},{spelling}"))
                .into_bytes(),
        );
    }
    for field in ["keys", "roles"] {
        let mut reordered = original.clone();
        reordered[field].as_array_mut().unwrap().reverse();
        cases.push(wire(&reordered));
    }
    let mut reordered = original.clone();
    reordered["roles"][0]["keyIds"]
        .as_array_mut()
        .unwrap()
        .reverse();
    cases.push(wire(&reordered));
    let mut nonroot = original.clone();
    let mut policy = nonroot["roles"][0].clone();
    policy["role"] = json!("timestamp");
    policy["component"] = Value::Null;
    policy["threshold"] = json!(1);
    nonroot["roles"].as_array_mut().unwrap().push(policy);
    cases.push(wire(&nonroot));
    let raw = String::from_utf8(wire(&original)).unwrap();
    cases.push(
        raw.replace(
            "\"product\":\"pinvou\"",
            "\"product\":\"pinvou\",\"pr\\u006fduct\":\"pinvou\"",
        )
        .into_bytes(),
    );
    cases.extend(resource_cases().into_iter().map(|(bytes, _)| bytes));
    let samples: Vec<_> = cases.iter().map(|bytes| STANDARD.encode(bytes)).collect();
    let expected: Vec<_> = cases
        .iter()
        .map(|bytes| {
            canonical(bytes)
                .map(|bytes| json!(STANDARD.encode(bytes)))
                .unwrap_or(Value::Null)
        })
        .collect();
    let reference = root()
        .join("specs/upgrade/v1/reference/signatures.mjs")
        .to_string_lossy()
        .into_owned();
    let mut child = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            include_str!("root-oracle.mjs"),
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
    assert!(output.status.success(), "original Root oracle unavailable");
    let actual: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
        assert!(actual == expected, "Root oracle mismatch at case {index}");
    }
    eprintln!(
        "{} original Node/Ajv Root policy cases passed",
        expected.len()
    );
}
fn node_mut<'a>(claims: &'a mut Value, path: &[&str]) -> &'a mut Value {
    match path {
        [] => claims,
        ["keys", "0"] => &mut claims["keys"][0],
        _ => &mut claims["roles"][0],
    }
}

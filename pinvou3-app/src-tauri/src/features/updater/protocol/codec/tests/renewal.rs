use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use pinvou_upgrade_codec::{
    Document, InputKind, ReleaseRenewalError, assert_supplied_release_renewal,
    decode_canonical_envelope,
};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(7)
        .unwrap()
        .to_owned()
}
fn fixtures() -> Vec<Value> {
    serde_json::from_slice::<Value>(
        &std::fs::read(root().join("specs/upgrade/v1/vectors/crypto/envelopes.json")).unwrap(),
    )
    .unwrap()["vectors"]
        .as_array()
        .unwrap()
        .clone()
}
fn original(role: &str) -> Value {
    let v = fixtures().into_iter().find(|v| v["name"] == role).unwrap();
    serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(v["envelopeBase64Url"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap()
}
fn wire(v: &Value) -> Vec<u8> {
    Document::parse(&serde_json::to_vec(v).unwrap(), InputKind::Metadata)
        .unwrap()
        .canonical_bytes()
        .unwrap()
}
fn kind(bytes: &[u8]) -> InputKind {
    let v: Value = serde_json::from_slice(bytes).unwrap_or(Value::Null);
    if [
        "root",
        "timestamp",
        "snapshot",
        "target",
        "release",
        "package",
    ]
    .contains(&v["signed"]["role"].as_str().unwrap_or(""))
    {
        InputKind::Metadata
    } else {
        InputKind::Credential
    }
}
fn check_raw(before: &[u8], after: &[u8]) -> Result<(), ReleaseRenewalError> {
    let before = decode_canonical_envelope(before, kind(before))
        .map_err(ReleaseRenewalError::Envelope)?
        .check_full_schema()
        .map_err(ReleaseRenewalError::Schema)?;
    let after = decode_canonical_envelope(after, kind(after))
        .map_err(ReleaseRenewalError::Envelope)?
        .check_full_schema()
        .map_err(ReleaseRenewalError::Schema)?;
    assert_supplied_release_renewal(&before, &after)
}
fn check(before: &Value, after: &Value) -> Result<(), ReleaseRenewalError> {
    check_raw(&wire(before), &wire(after))
}
fn renewed(before: &Value) -> Value {
    let mut next = before.clone();
    next["signed"]["revision"] = json!(before["signed"]["revision"].as_u64().unwrap() + 1);
    next
}
fn two_targets() -> Value {
    let mut e = original("release");
    let mut second = e["signed"]["targets"][0].clone();
    second["targetKey"] = json!("linux-arm64-deb-perMachine");
    second["packageManifest"]["scope"]["targetKey"] = second["targetKey"].clone();
    e["signed"]["targets"].as_array_mut().unwrap().push(second);
    e
}
#[test]
fn revision_only_requires_increase_and_three_mutable_fields_do_not_add_clock_rules() {
    let e = original("release");
    let mut next = renewed(&e);
    assert_eq!(check(&e, &next), Ok(()));
    next["signed"]["revision"] = json!(9007199254740991u64);
    next["signed"]["issuedAt"] = json!(9007199254740991u64);
    next["signed"]["expiresAt"] = json!(0);
    assert_eq!(check(&e, &next), Ok(())); // Schema-valid reversed time is outside renewal relationship.
    for revision in [0, 1, 9007199254740992u64] {
        next["signed"]["revision"] = json!(revision);
        assert!(check(&e, &next).is_err());
    }
    let mut before = e.clone();
    before["signed"]["revision"] = json!(9007199254740990u64);
    next = before.clone();
    next["signed"]["revision"] = json!(9007199254740991u64);
    assert_eq!(check(&before, &next), Ok(()));
    assert_eq!(
        check(&next, &before),
        Err(ReleaseRenewalError::RevisionInvalid)
    );
}
#[test]
fn every_business_field_and_array_order_binds_while_signatures_are_separate() {
    let e = two_targets();
    let next = renewed(&e);
    assert_eq!(check(&e, &next), Ok(()));
    let mut changed = next.clone();
    changed["signed"]["targets"]
        .as_array_mut()
        .unwrap()
        .reverse();
    assert_eq!(
        check(&e, &changed),
        Err(ReleaseRenewalError::BusinessChanged)
    );
    for (path, value) in [
        ("/signed/product", json!("another-product")),
        ("/signed/component", json!("engine")),
        ("/signed/scope/releaseId", json!("another-release")),
        ("/signed/appVersion", json!("9.0.0")),
        (
            "/signed/targets/0/packageManifest/envelopeSha256",
            json!("b".repeat(64)),
        ),
        ("/signed/targets/0/packageManifest/size", json!(1)),
    ] {
        let mut changed = next.clone();
        *changed.pointer_mut(path).unwrap() = value;
        assert_eq!(
            check(&e, &changed),
            Err(ReleaseRenewalError::BusinessChanged),
            "{path}"
        );
    }
    let mut signatures = next.clone();
    signatures["signatures"][0]["signature"] = json!(URL_SAFE_NO_PAD.encode([0; 64]));
    assert_eq!(check(&e, &signatures), Ok(())); // No authenticity or publication assertion.
}
#[test]
fn both_complete_envelopes_are_checked_and_other_roles_cannot_be_renewed() {
    let e = original("release");
    let next = renewed(&e);
    for v in fixtures() {
        let role = v["name"].as_str().unwrap();
        if role == "release" {
            continue;
        }
        let other = original(role);
        assert_eq!(
            check(&other, &next),
            Err(ReleaseRenewalError::RevisionInvalid)
        );
        assert_eq!(check(&e, &other), Err(ReleaseRenewalError::RevisionInvalid));
    }
    let mut missing = next.clone();
    missing["signed"].as_object_mut().unwrap().remove("product");
    assert!(check(&e, &missing).is_err());
    let mut extra = e.clone();
    extra["signed"]["extra"] = json!(null);
    assert!(check(&extra, &next).is_err());
    assert!(check_raw(&serde_json::to_vec_pretty(&e).unwrap(), &wire(&next)).is_err());
    let text = String::from_utf8(wire(&e)).unwrap();
    let duplicate = text.replacen("\"revision\":", "\"revi\\u0073ion\":1,\"revision\":", 1);
    assert!(check_raw(duplicate.as_bytes(), &wire(&next)).is_err());
}
fn leaves(v: &Value, path: &str, out: &mut Vec<String>) {
    match v {
        Value::Object(o) => {
            for (k, c) in o {
                leaves(c, &format!("{path}/{k}"), out);
            }
        }
        Value::Array(a) => {
            for (i, c) in a.iter().enumerate() {
                leaves(c, &format!("{path}/{i}"), out);
            }
        }
        _ => out.push(path.to_owned()),
    }
}
#[test]
fn actual_original_node_release_renewal_matches_all_business_and_revision_boundaries() {
    let mut cases = Vec::new();
    let mut add = |before: &Value, next: &Value| {
        let a = &before["signed"];
        let b = &next["signed"];
        cases.push(json!({"previous":STANDARD.encode(wire(before)),"next":STANDARD.encode(wire(next)),
   "previousExpected":{"role":a["role"],"product":a["product"],"component":a["component"],"scope":a["scope"]},
   "nextExpected":{"role":b["role"],"product":b["product"],"component":b["component"],"scope":b["scope"]}}));
    };
    for before in [original("release"), two_targets()] {
        let next = renewed(&before);
        assert_eq!(check(&before, &next), Ok(()));
        add(&before, &next);
        let mut paths = Vec::new();
        leaves(&next["signed"], "/signed", &mut paths);
        for path in paths {
            for value in [
                Value::Null,
                json!(false),
                json!(0),
                json!(1),
                json!(2),
                json!(9007199254740991u64),
                json!(9007199254740992u64),
                json!(1.5),
                json!("different"),
                json!("9.0.0"),
                json!("b".repeat(64)),
                json!({}),
                json!([]),
            ] {
                let mut changed = next.clone();
                *changed.pointer_mut(&path).unwrap() = value;
                add(&before, &changed);
            }
            let mut changed = next.clone();
            let parent = path.rsplit_once('/').unwrap().0;
            let key = path.rsplit_once('/').unwrap().1;
            if let Some(object) = changed.pointer_mut(parent).unwrap().as_object_mut() {
                object.remove(key);
                add(&before, &changed);
            }
        }
        for role in fixtures()
            .into_iter()
            .map(|v| v["name"].as_str().unwrap().to_owned())
        {
            add(&original(&role), &next);
            add(&before, &original(&role));
        }
        for a in [1, 2, 9007199254740990u64, 9007199254740991u64] {
            for b in [1, 2, 3, 9007199254740990u64, 9007199254740991u64] {
                let mut old = before.clone();
                old["signed"]["revision"] = json!(a);
                let mut new = next.clone();
                new["signed"]["revision"] = json!(b);
                add(&old, &new);
            }
        }
        let mut time = next.clone();
        time["signed"]["issuedAt"] = json!(9007199254740991u64);
        time["signed"]["expiresAt"] = json!(0);
        add(&before, &time);
        let mut sig = next.clone();
        sig["signatures"][0]["signature"] = json!(URL_SAFE_NO_PAD.encode([0; 64]));
        add(&before, &sig);
        let mut order = next.clone();
        order["signed"]["targets"].as_array_mut().unwrap().reverse();
        add(&before, &order);
    }
    let expected: Vec<bool> = cases
        .iter()
        .map(|c| {
            check_raw(
                &STANDARD.decode(c["previous"].as_str().unwrap()).unwrap(),
                &STANDARD.decode(c["next"].as_str().unwrap()).unwrap(),
            )
            .is_ok()
        })
        .collect();
    let reference = root().join("specs/upgrade/v1/reference/signatures.mjs");
    let mut child = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            include_str!("renewal-oracle.mjs"),
            reference.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&cases).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "original renewal oracle unavailable"
    );
    let actual: Vec<bool> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (i, (a, b)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(a, b, "release renewal case {i}");
    }
    eprintln!(
        "{} actual original Node Release renewal pairs passed",
        cases.len()
    );
}

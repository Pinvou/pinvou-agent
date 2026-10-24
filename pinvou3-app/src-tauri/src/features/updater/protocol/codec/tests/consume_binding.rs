#[allow(dead_code)]
#[path = "support/download_fixture.rs"]
mod download_fixture;

use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use pinvou_upgrade_codec::{
    ConsumeBindingError, Document, InputKind, assert_supplied_consume_binding,
    decode_canonical_envelope,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
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
    let mut vectors = serde_json::from_slice::<Value>(
        &std::fs::read(root().join("specs/upgrade/v1/vectors/crypto/envelopes.json")).unwrap(),
    )
    .unwrap()["vectors"]
        .as_array()
        .unwrap()
        .clone();
    download_fixture::replace_legacy_download(
        &mut vectors,
        &root().join("specs/upgrade/v1/reference/canonical-json.mjs"),
    );
    vectors
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
fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn request(envelope: &Value) -> Value {
    let mut r: Value = serde_json::from_slice::<Value>(
        &std::fs::read(root().join("specs/upgrade/v1/vectors/crypto/digests.json")).unwrap(),
    )
    .unwrap()["consumeRequest"]
        .clone();
    let s = &envelope["signed"];
    for field in [
        "product",
        "component",
        "purpose",
        "authorizationJti",
        "transactionId",
        "clientFactsDigest",
        "preparation",
        "freezeEpoch",
        "backup",
        "staged",
        "preinstallSlot",
    ] {
        r[field] = s[field].clone();
    }
    r["installationScopeId"] = s["scope"]["installationScopeId"].clone();
    r["plans"] = s["update"]["plans"].clone();
    r["authorizationEnvelopeSha256"] = json!(hash(&wire(envelope)));
    r
}
fn check_raw(raw: &[u8], rraw: &[u8]) -> Result<(), ConsumeBindingError> {
    let r = Document::parse(rraw, InputKind::Metadata).map_err(ConsumeBindingError::Json)?;
    let shape = r
        .consume_request_shape()
        .map_err(ConsumeBindingError::Request)?;
    let value: Value = serde_json::from_slice(raw).unwrap_or(Value::Null);
    let kind = if [
        "root",
        "timestamp",
        "snapshot",
        "target",
        "release",
        "package",
    ]
    .contains(&value["signed"]["role"].as_str().unwrap_or(""))
    {
        InputKind::Metadata
    } else {
        InputKind::Credential
    };
    let e = decode_canonical_envelope(raw, kind)
        .map_err(ConsumeBindingError::Envelope)?
        .check_full_schema()
        .map_err(ConsumeBindingError::Schema)?;
    assert_supplied_consume_binding(&shape, &e)
}
fn check(e: &Value, r: &Value) -> Result<(), ConsumeBindingError> {
    check_raw(&wire(e), &serde_json::to_vec(r).unwrap())
}
const PURPOSES: [&str; 3] = ["preinstall", "install", "activate"];
#[test]
fn three_purposes_bind_exact_original_bytes_without_claiming_cryptography_or_consumption() {
    for purpose in PURPOSES {
        let mut e = original(&format!("authorization-{purpose}"));
        let r = request(&e);
        assert_eq!(check(&e, &r), Ok(()));
        e["signatures"][0]["signature"] = json!(URL_SAFE_NO_PAD.encode([0; 64]));
        assert_eq!(check(&e, &r), Err(ConsumeBindingError::BindingMismatch));
        assert_eq!(check(&e, &request(&e)), Ok(())); // Encoding-valid zero signature is no authenticity.
        let mut body_hash = request(&e);
        body_hash["authorizationEnvelopeSha256"] = json!(hash(&wire(&e["signed"])));
        assert_eq!(
            check(&e, &body_hash),
            Err(ConsumeBindingError::BindingMismatch)
        );
    }
    let e = original("authorization-install");
    for role in [
        "root",
        "timestamp",
        "snapshot",
        "target",
        "release",
        "package",
        "decision",
        "download",
        "helper-cleanup",
        "telemetry-event",
        "preinstall-transaction-event",
        "install-transaction-event",
        "activate-transaction-event",
        "reconciliation-action",
        "recovery-review-action",
        "reconciliation-event",
        "recovery-review-event",
    ] {
        assert_eq!(
            check(&original(role), &request(&e)),
            Err(ConsumeBindingError::PurposeMismatch),
            "{role}"
        );
    }
}
#[test]
fn every_bound_leaf_is_complete_and_unbound_request_identities_remain_owner_inputs() {
    for purpose in PURPOSES {
        let e = original(&format!("authorization-{purpose}"));
        let r = request(&e);
        let mut paths = Vec::new();
        leaves(&r, "", &mut paths);
        for path in paths {
            if ["/protocolVersion", "/consumeKey", "/recoveryHandleHash"].contains(&path.as_str()) {
                continue;
            }
            let mut changed = r.clone();
            let old = changed.pointer(&path).unwrap();
            let new = if old.is_number() {
                json!(old.as_f64().unwrap() + 1.0)
            } else if old.is_null() {
                json!({})
            } else {
                json!("different")
            };
            *changed.pointer_mut(&path).unwrap() = new;
            assert!(check(&e, &changed).is_err(), "{purpose} {path}");
        }
        let mut independent = r.clone();
        independent["consumeKey"] = json!("new-owner-input");
        independent["recoveryHandleHash"] = json!("b".repeat(64));
        assert_eq!(check(&e, &independent), Ok(()));
        // Request schema permits another purpose, but exact authorization role refuses it.
        independent["purpose"] = json!(if purpose == "install" {
            "activate"
        } else {
            "install"
        });
        assert_eq!(
            check(&e, &independent),
            Err(ConsumeBindingError::PurposeMismatch)
        );
    }
}
#[test]
fn request_schema_is_closed_and_binding_does_not_add_credential_time_or_qualification_checks() {
    let mut e = original("authorization-install");
    let r = request(&e);
    for field in [
        "consumeRequestDigest",
        "recoverySecret",
        "requestId",
        "headers",
        "unexpected",
    ] {
        let mut extra = r.clone();
        extra[field] = json!(null);
        assert!(
            Document::parse(&serde_json::to_vec(&extra).unwrap(), InputKind::Metadata)
                .unwrap()
                .consume_request_shape()
                .is_err(),
            "{field}"
        );
    }
    let raw = serde_json::to_string(&r).unwrap();
    let duplicate = raw.replacen(
        "\"purpose\":",
        "\"purpo\\u0073e\":\"install\",\"purpose\":",
        1,
    );
    assert!(Document::parse(duplicate.as_bytes(), InputKind::Metadata).is_err());
    e["signed"]["nbf"] = e["signed"]["exp"].clone();
    assert_eq!(check(&e, &request(&e)), Ok(())); // Semantics/time must be checked separately.
    let reordered: Vec<String> = r
        .as_object()
        .unwrap()
        .iter()
        .rev()
        .map(|(k, v)| format!("{}:{}", serde_json::to_string(k).unwrap(), v))
        .collect();
    assert_eq!(
        check_raw(
            &wire(&original("authorization-install")),
            format!("{{{}}}", reordered.join(",")).as_bytes()
        ),
        Ok(())
    );
    let mut invalid = r.clone();
    invalid["preparation"]["ownerEpoch"] = json!(9007199254740992u64);
    assert!(check(&original("authorization-install"), &invalid).is_err());
    let large = format!("{{\"extra\":\"{}\"}}", "x".repeat(65537));
    assert!(Document::parse(large.as_bytes(), InputKind::Metadata).is_err());
}
fn leaves(v: &Value, path: &str, out: &mut Vec<String>) {
    if let Some(o) = v.as_object() {
        for (k, c) in o {
            leaves(c, &format!("{path}/{k}"), out);
        }
    } else {
        out.push(path.to_owned());
    }
}
fn objects(v: &Value, path: &str, out: &mut Vec<String>) {
    if let Some(o) = v.as_object() {
        out.push(path.to_owned());
        for (k, c) in o {
            objects(c, &format!("{path}/{k}"), out);
        }
    }
}
#[test]
fn original_node_post_verification_binding_fragment_matches_complete_request_schema_and_relationships()
 {
    let mut cases = Vec::new();
    let mut add = |e: &Value, rraw: Vec<u8>| {
        let s = &e["signed"];
        cases.push(json!({"raw":STANDARD.encode(wire(e)),"requestRaw":STANDARD.encode(rraw),
   "expected":{"role":s["role"],"product":s["product"],"component":s["component"],"scope":s["scope"]}}));
    };
    for purpose in PURPOSES {
        let original = original(&format!("authorization-{purpose}"));
        let mut full = original.clone();
        let s = &mut full["signed"];
        s["backup"] = json!({"recordId":"backup-1","freezeEpoch":1,"sha256":"a".repeat(64),"dataScopeDigest":"a".repeat(64),"preparationEpoch":1});
        s["staged"] = json!({"preinstallTransactionId":"preinstall-1","stagedRevision":1,"slotIdentity":"slot-1","stagedAt":0,"stagedValidUntil":0});
        s["preinstallSlot"] = json!({"slotIdentity":"slot-1","slotRevision":1,"state":"inactive"});
        let p = s["update"]["plans"]["installation"].clone();
        for field in [
            "migration",
            "backup",
            "helperMigration",
            "launcherMigration",
            "takeover",
        ] {
            s["update"]["plans"][field] = p.clone();
        }
        for e in [original, full] {
            let r = request(&e);
            assert_eq!(
                check(&e, &r),
                Ok(()),
                "full nullable-field positive control {purpose}"
            );
            add(&e, serde_json::to_vec(&r).unwrap());
            let mut paths = Vec::new();
            leaves(&r, "", &mut paths);
            for path in paths {
                let old = r.pointer(&path).unwrap();
                for new in [
                    Value::Null,
                    json!(false),
                    json!(0),
                    json!(1),
                    json!(9007199254740991u64),
                    json!(9007199254740992u64),
                    json!(1.5),
                    json!("different"),
                    json!("b".repeat(64)),
                    json!({}),
                    json!([]),
                ] {
                    if old == &new {
                        continue;
                    }
                    let mut changed = r.clone();
                    *changed.pointer_mut(&path).unwrap() = new;
                    add(&e, serde_json::to_vec(&changed).unwrap());
                }
            }
            let mut paths = Vec::new();
            objects(&r, "", &mut paths);
            for path in paths {
                let keys: Vec<_> = r
                    .pointer(&path)
                    .unwrap()
                    .as_object()
                    .unwrap()
                    .keys()
                    .cloned()
                    .collect();
                for key in keys {
                    let mut changed = r.clone();
                    changed
                        .pointer_mut(&path)
                        .unwrap()
                        .as_object_mut()
                        .unwrap()
                        .remove(&key);
                    add(&e, serde_json::to_vec(&changed).unwrap());
                }
                let mut changed = r.clone();
                changed.pointer_mut(&path).unwrap()["extra"] = json!(null);
                add(&e, serde_json::to_vec(&changed).unwrap());
            }
            for field in [
                "consumeRequestDigest",
                "recoverySecret",
                "requestId",
                "headers",
            ] {
                let mut changed = r.clone();
                changed[field] = json!(null);
                add(&e, serde_json::to_vec(&changed).unwrap());
            }
            let mut new_signature = e.clone();
            new_signature["signatures"][0]["signature"] = json!(URL_SAFE_NO_PAD.encode([0; 64]));
            add(&new_signature, serde_json::to_vec(&r).unwrap());
            add(
                &new_signature,
                serde_json::to_vec(&request(&new_signature)).unwrap(),
            );
            let mut no_time = e.clone();
            no_time["signed"]["nbf"] = no_time["signed"]["exp"].clone();
            add(&no_time, serde_json::to_vec(&request(&no_time)).unwrap());
            let mut unbound = r.clone();
            unbound["consumeKey"] = json!("owner-choice");
            unbound["recoveryHandleHash"] = json!("b".repeat(64));
            add(&e, serde_json::to_vec(&unbound).unwrap());
        }
    }
    let base = original("authorization-install");
    let r = request(&base);
    for v in fixtures() {
        let role = v["name"].as_str().unwrap();
        add(&original(role), serde_json::to_vec(&r).unwrap());
    }
    let raw = serde_json::to_string(&r).unwrap();
    add(
        &base,
        raw.replacen(
            "\"purpose\":",
            "\"purpo\\u0073e\":\"install\",\"purpose\":",
            1,
        )
        .into_bytes(),
    );
    let expected: Vec<bool> = cases
        .iter()
        .map(|c| {
            check_raw(
                &STANDARD.decode(c["raw"].as_str().unwrap()).unwrap(),
                &STANDARD.decode(c["requestRaw"].as_str().unwrap()).unwrap(),
            )
            .is_ok()
        })
        .collect();
    let reference = root().join("specs/upgrade/v1/reference/signatures.mjs");
    let mut child = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            include_str!("consume-binding-oracle.mjs"),
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
        "original binding fragment unavailable"
    );
    let actual: Vec<bool> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (i, (a, b)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(a, b, "consume binding case {i}");
    }
    eprintln!(
        "{} original Node post-verification consume binding cases passed",
        cases.len()
    );
}

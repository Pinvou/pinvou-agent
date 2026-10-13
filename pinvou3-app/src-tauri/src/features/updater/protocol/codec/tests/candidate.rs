use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use pinvou_upgrade_codec::{
    CandidateBindingError, Document, InputKind, OpeningBindingError, assert_rollout_opening,
    assert_supplied_candidate_binding, decode_canonical_envelope,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
const KEYS: [&str; 4] = [
    "windows-x86_64-nsis-perMachine",
    "linux-x86_64-deb-perMachine",
    "linux-arm64-deb-perMachine",
    "macos-universal-dmg-perMachine",
];
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
fn hash(e: &Value) -> String {
    Sha256::digest(wire(e))
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn commit(t: &mut Value, o: &Value) {
    let d = Document::parse(&serde_json::to_vec(o).unwrap(), InputKind::Metadata).unwrap();
    t["signed"]["rolloutSetCommitment"] = json!(d.rollout_commitment().unwrap().commitment_hash());
}
fn rebind(t: &mut Value, r: &Value, o: &mut Value) {
    o["releaseEnvelopeSha256"] = json!(hash(r));
    commit(t, o);
}
fn pair(key: &str) -> (Value, Value, Value) {
    let mut t = original("target");
    let mut r = original("release");
    t["signed"]["scope"]["targetKey"] = json!(key);
    t["signed"]["baselineRelease"]["envelopeSha256"] = json!("0".repeat(64));
    r["signed"]["targets"][0]["targetKey"] = json!(key);
    r["signed"]["targets"][0]["packageManifest"]["scope"]["targetKey"] = json!(key);
    let o = json!({"targetKey":key,"deploymentId":"qa-deployment","deploymentRevision":1,"rolloutId":"qa-rollout","rolloutRevision":1,"releaseEnvelopeSha256":hash(&r),"leafSalt":"0".repeat(64)});
    commit(&mut t, &o);
    (t, r, o)
}
fn kind(raw: &[u8]) -> InputKind {
    let v: Value = serde_json::from_slice(raw).unwrap_or(Value::Null);
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
fn check_raw(t: &[u8], r: &[u8], o: &[u8]) -> Result<(), CandidateBindingError> {
    let t = decode_canonical_envelope(t, kind(t))
        .map_err(CandidateBindingError::Envelope)?
        .check_full_schema()
        .map_err(CandidateBindingError::Schema)?;
    let r = decode_canonical_envelope(r, kind(r))
        .map_err(CandidateBindingError::Envelope)?
        .check_full_schema()
        .map_err(CandidateBindingError::Schema)?;
    let o = Document::parse(o, InputKind::Metadata).map_err(CandidateBindingError::Json)?;
    assert_supplied_candidate_binding(&t, &r, &o)
}
fn check(t: &Value, r: &Value, o: &Value) -> Result<(), CandidateBindingError> {
    check_raw(&wire(t), &wire(r), &serde_json::to_vec(o).unwrap())
}
fn prove_opening(t: &Value, r: &Value, o: &Value) {
    let target = Document::parse(
        &serde_json::to_vec(&t["signed"]).unwrap(),
        InputKind::Metadata,
    )
    .unwrap();
    let hash =
        Document::parse(&serde_json::to_vec(&hash(r)).unwrap(), InputKind::Metadata).unwrap();
    let opening = Document::parse(&serde_json::to_vec(o).unwrap(), InputKind::Metadata).unwrap();
    assert_rollout_opening(&target, &hash, &opening).unwrap();
}
#[test]
fn all_known_targets_keep_exact_raw_release_hash_without_claiming_context_or_entropy() {
    for key in KEYS {
        let (mut t, mut r, mut o) = pair(key);
        prove_opening(&t, &r, &o);
        assert_eq!(check(&t, &r, &o), Ok(()));
        r["signed"]["product"] = json!("another-product");
        r["signed"]["component"] = json!("engine");
        r["signed"]["scope"]["releaseId"] = json!("another-release");
        r["signatures"][0]["signature"] = json!(URL_SAFE_NO_PAD.encode([0; 64]));
        rebind(&mut t, &r, &mut o);
        assert_eq!(check(&t, &r, &o), Ok(())); // Context/crypto/entropy/selection remain outside.
        t["signed"]["issuedAt"] = json!(9007199254740991u64);
        t["signed"]["expiresAt"] = json!(0);
        assert_eq!(check(&t, &r, &o), Ok(()));
    }
}
#[test]
fn three_original_relationships_are_isolated_after_other_bindings_pass() {
    let key = KEYS[1];
    let (t, r, o) = pair(key);
    let mut baseline = t.clone();
    baseline["signed"]["baselineRelease"]["envelopeSha256"] = json!(hash(&r));
    prove_opening(&baseline, &r, &o);
    assert_eq!(
        check(&baseline, &r, &o),
        Err(CandidateBindingError::IsBaseline)
    );
    let mut different = o.clone();
    different["targetKey"] = json!(KEYS[2]);
    let mut target = t.clone();
    commit(&mut target, &different);
    assert_eq!(
        check(&target, &r, &different),
        Err(CandidateBindingError::Opening(
            OpeningBindingError::Mismatch
        ))
    );
    different = o.clone();
    different["releaseEnvelopeSha256"] = json!("b".repeat(64));
    target = t.clone();
    commit(&mut target, &different);
    assert_eq!(
        check(&target, &r, &different),
        Err(CandidateBindingError::Opening(
            OpeningBindingError::Mismatch
        ))
    );
    different = o.clone();
    different["leafSalt"] = json!("b".repeat(64));
    assert_eq!(
        check(&t, &r, &different),
        Err(CandidateBindingError::Opening(
            OpeningBindingError::Mismatch
        ))
    );
    commit(&mut target, &different);
    assert_eq!(check(&target, &r, &different), Ok(()));
    let mut release = r.clone();
    release["signed"]["targets"][0]["targetKey"] = json!(KEYS[2]);
    release["signed"]["targets"][0]["packageManifest"]["scope"]["targetKey"] = json!(KEYS[2]);
    let mut target = t.clone();
    let mut opening = o.clone();
    rebind(&mut target, &release, &mut opening);
    prove_opening(&target, &release, &opening);
    assert_ne!(
        target["signed"]["baselineRelease"]["envelopeSha256"],
        json!(hash(&release))
    );
    assert_eq!(
        check(&target, &release, &opening),
        Err(CandidateBindingError::TargetMissing)
    );
}
#[test]
fn signatures_closed_opening_roles_and_duplicate_resources_are_not_normalized() {
    let (mut t, mut r, mut o) = pair(KEYS[1]);
    r["signatures"][0]["signature"] = json!(URL_SAFE_NO_PAD.encode([0; 64]));
    assert_eq!(
        check(&t, &r, &o),
        Err(CandidateBindingError::Opening(
            OpeningBindingError::Mismatch
        ))
    );
    rebind(&mut t, &r, &mut o);
    assert_eq!(check(&t, &r, &o), Ok(()));
    for role in [
        "root",
        "timestamp",
        "snapshot",
        "release",
        "package",
        "decision",
        "download",
    ] {
        assert_eq!(
            check(&original(role), &r, &o),
            Err(CandidateBindingError::RoleInvalid)
        );
    }
    assert_eq!(
        check(&t, &original("package"), &o),
        Err(CandidateBindingError::RoleInvalid)
    );
    assert!(check(&t, &r, &Value::Null).is_err());
    let mut extra = o.clone();
    extra["extra"] = json!(null);
    assert!(check(&t, &r, &extra).is_err());
    let raw = serde_json::to_string(&o).unwrap();
    let duplicate = raw.replacen(
        "\"leafSalt\":",
        "\"leaf\\u0053alt\":\"duplicate\",\"leafSalt\":",
        1,
    );
    assert!(check_raw(&wire(&t), &wire(&r), duplicate.as_bytes()).is_err());
    let oversized = format!("{{\"extra\":\"{}\"}}", "x".repeat(65537));
    assert!(check_raw(&wire(&t), &wire(&r), oversized.as_bytes()).is_err());
}
#[test]
fn actual_original_node_three_guards_match_boundaries_and_coherent_rebound_controls() {
    let mut cases = Vec::new();
    let mut add = |t: &Value, r: &Value, o: &Value| {
        let a = &t["signed"];
        let b = &r["signed"];
        cases.push(json!({"targetRaw":STANDARD.encode(wire(t)),"releaseRaw":STANDARD.encode(wire(r)),"openingRaw":STANDARD.encode(serde_json::to_vec(o).unwrap()),
   "targetExpected":{"role":a["role"],"product":a["product"],"component":a["component"],"scope":a["scope"]},
   "releaseExpected":{"role":b["role"],"product":b["product"],"component":b["component"],"scope":b["scope"]}}));
    };
    for key in KEYS {
        let (t, r, o) = pair(key);
        assert_eq!(check(&t, &r, &o), Ok(()));
        add(&t, &r, &o);
        let fields: Vec<_> = o.as_object().unwrap().keys().cloned().collect();
        for field in fields {
            for value in [
                Value::Null,
                json!(0),
                json!(1),
                json!(2),
                json!(9007199254740991u64),
                json!(9007199254740992u64),
                json!(1.5),
                json!("different"),
                json!("b".repeat(64)),
                json!({}),
                json!([]),
            ] {
                let mut opening = o.clone();
                opening[&field] = value;
                add(&t, &r, &opening);
                let d =
                    Document::parse(&serde_json::to_vec(&opening).unwrap(), InputKind::Metadata)
                        .unwrap();
                if d.rollout_commitment().is_ok() {
                    let mut target = t.clone();
                    commit(&mut target, &opening);
                    add(&target, &r, &opening);
                }
            }
            let mut missing = o.clone();
            missing.as_object_mut().unwrap().remove(&field);
            add(&t, &r, &missing);
        }
        for other in KEYS {
            let mut release = r.clone();
            release["signed"]["targets"][0]["targetKey"] = json!(other);
            release["signed"]["targets"][0]["packageManifest"]["scope"]["targetKey"] = json!(other);
            let mut target = t.clone();
            let mut opening = o.clone();
            rebind(&mut target, &release, &mut opening);
            prove_opening(&target, &release, &opening);
            add(&target, &release, &opening);
            let mut changed = o.clone();
            changed["targetKey"] = json!(other);
            target = t.clone();
            commit(&mut target, &changed);
            add(&target, &r, &changed);
        }
        let mut baseline = t.clone();
        baseline["signed"]["baselineRelease"]["envelopeSha256"] = json!(hash(&r));
        add(&baseline, &r, &o);
        let mut sig = r.clone();
        sig["signatures"][0]["signature"] = json!(URL_SAFE_NO_PAD.encode([0; 64]));
        add(&t, &sig, &o);
        let mut target = t.clone();
        let mut opening = o.clone();
        rebind(&mut target, &sig, &mut opening);
        add(&target, &sig, &opening);
        let mut context = r.clone();
        context["signed"]["product"] = json!("different");
        context["signed"]["component"] = json!("engine");
        context["signed"]["scope"]["releaseId"] = json!("different-release");
        rebind(&mut target, &context, &mut opening);
        add(&target, &context, &opening);
        let mut time = t.clone();
        time["signed"]["issuedAt"] = json!(9007199254740991u64);
        time["signed"]["expiresAt"] = json!(0);
        add(&time, &r, &o);
        for role in fixtures()
            .into_iter()
            .map(|v| v["name"].as_str().unwrap().to_owned())
        {
            add(&original(&role), &r, &o);
            add(&t, &original(&role), &o);
        }
        let mut extra = o.clone();
        extra["extra"] = json!(null);
        add(&t, &r, &extra);
        add(&t, &r, &Value::Null);
    }
    let expected: Vec<bool> = cases
        .iter()
        .map(|c| {
            check_raw(
                &STANDARD.decode(c["targetRaw"].as_str().unwrap()).unwrap(),
                &STANDARD.decode(c["releaseRaw"].as_str().unwrap()).unwrap(),
                &STANDARD.decode(c["openingRaw"].as_str().unwrap()).unwrap(),
            )
            .is_ok()
        })
        .collect();
    let reference = root().join("specs/upgrade/v1/reference/signatures.mjs");
    let mut child = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            include_str!("candidate-oracle.mjs"),
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
        "original candidate guard oracle unavailable"
    );
    let actual: Vec<bool> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (i, (a, b)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(a, b, "candidate binding case {i}");
    }
    eprintln!(
        "{} actual original Node candidate guard cases passed",
        cases.len()
    );
}

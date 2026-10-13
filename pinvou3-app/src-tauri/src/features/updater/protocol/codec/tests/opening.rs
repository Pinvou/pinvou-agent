use base64::{Engine, engine::general_purpose::STANDARD};
use pinvou_upgrade_codec::{
    Document, Error, InputKind, OpeningBindingError, assert_rollout_opening,
};
use serde_json::{Value, json};
use std::{
    fs,
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
fn base() -> Value {
    let vector: Value = serde_json::from_slice(
        &fs::read(root().join("specs/upgrade/v1/vectors/crypto/digests.json")).unwrap(),
    )
    .unwrap();
    vector["opening"].clone()
}
fn wire(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
fn target(opening: &Value) -> Value {
    let doc = Document::parse(&wire(opening), InputKind::Metadata).unwrap();
    let hash = doc.rollout_commitment().unwrap();
    json!({"scope":{"targetKey":opening["targetKey"],"extra":"ignored"},"rolloutSetCommitment":hash.commitment_hash(),"role":"not-a-Target","ignored":true})
}
#[derive(Debug, PartialEq, Eq)]
enum Failure {
    Json(Error),
    Binding(OpeningBindingError),
}
fn check(bytes: [&[u8]; 3]) -> Result<(), Failure> {
    let target = Document::parse(bytes[0], InputKind::Metadata).map_err(Failure::Json)?;
    let hash = Document::parse(bytes[1], InputKind::Metadata).map_err(Failure::Json)?;
    let opening = Document::parse(bytes[2], InputKind::Metadata).map_err(Failure::Json)?;
    assert_rollout_opening(&target, &hash, &opening).map_err(Failure::Binding)
}
fn sample(target: &Value, hash: &Value, opening: &Value) -> [Vec<u8>; 3] {
    [wire(target), wire(hash), wire(opening)]
}
fn accepted(sample: &[Vec<u8>; 3]) -> bool {
    check([&sample[0], &sample[1], &sample[2]]).is_ok()
}
#[test]
fn supplied_bindings_match_without_full_target_or_current_membership_claims() {
    let opening = base();
    let target = target(&opening);
    let hash = opening["releaseEnvelopeSha256"].clone();
    assert!(accepted(&sample(&target, &hash, &opening)));
    let pretty = [
        serde_json::to_vec_pretty(&target).unwrap(),
        wire(&hash),
        serde_json::to_vec_pretty(&opening).unwrap(),
    ];
    assert!(accepted(&pretty));
    let escaped = wire(&target);
    let text = String::from_utf8(escaped)
        .unwrap()
        .replace("\"targetKey\"", "\"target\\u004bey\"");
    assert!(check([text.as_bytes(), &wire(&hash), &wire(&opening)]).is_ok());
    let empty = Document::parse(b"null", InputKind::Metadata)
        .unwrap()
        .rollout_commitment()
        .unwrap();
    let target = json!({"scope":{"targetKey":opening["targetKey"]},"rolloutSetCommitment":empty.commitment_hash()});
    assert_eq!(
        check([&wire(&target), &wire(&hash), b"null"]),
        Err(Failure::Binding(OpeningBindingError::Mismatch))
    );
}
#[test]
fn all_seven_leaves_bind_and_recalculation_adds_no_selection_or_entropy_rules() {
    let opening = base();
    let original_target = target(&opening);
    let original_hash = opening["releaseEnvelopeSha256"].clone();
    for field in opening.as_object().unwrap().keys() {
        let mut changed = opening.clone();
        changed[field] = match field.as_str() {
            "targetKey" => json!("windows-x86_64-nsis-perMachine"),
            "deploymentRevision" | "rolloutRevision" => json!(2),
            "releaseEnvelopeSha256" | "leafSalt" => json!("0".repeat(64)),
            _ => json!("changed-id"),
        };
        assert!(!accepted(&sample(
            &original_target,
            &original_hash,
            &changed
        )));
        let recalculated = target(&changed);
        let retained_target = json!({"scope":original_target["scope"],"rolloutSetCommitment":recalculated["rolloutSetCommitment"]});
        assert_eq!(
            accepted(&sample(&retained_target, &original_hash, &changed)),
            !["targetKey", "releaseEnvelopeSha256"].contains(&field.as_str())
        );
        assert!(accepted(&sample(
            &recalculated,
            &changed["releaseEnvelopeSha256"],
            &changed
        )));
    }
}
#[test]
fn all_three_documents_keep_ignored_extra_resource_and_duplicate_checks() {
    let opening = base();
    let target = target(&opening);
    let hash = opening["releaseEnvelopeSha256"].clone();
    let original = sample(&target, &hash, &opening);
    for index in 0..3 {
        let mut changed = original.clone();
        changed[index] = wire(&json!("x".repeat(65_537)));
        assert_eq!(
            check([&changed[0], &changed[1], &changed[2]]),
            Err(Failure::Json(Error::StringLimit))
        );
        let mut exact = vec![b' '; 1_048_576 - original[index].len()];
        exact.extend_from_slice(&original[index]);
        let mut changed = original.clone();
        changed[index] = exact;
        assert!(accepted(&changed));
        changed[index].push(b' ');
        assert_eq!(
            check([&changed[0], &changed[1], &changed[2]]),
            Err(Failure::Json(Error::InputLimit))
        );
        changed[index] = br#"{"extra":0,"e\u0078tra":1}"#.to_vec();
        assert_eq!(
            check([&changed[0], &changed[1], &changed[2]]),
            Err(Failure::Json(Error::DuplicateKey))
        );
    }
    let mut polluted = target.clone();
    polluted["ignored"] = json!("x".repeat(65_537));
    assert_eq!(
        check([&wire(&polluted), &wire(&hash), &wire(&opening)]),
        Err(Failure::Json(Error::StringLimit))
    );
}
#[test]
fn actual_original_node_opening_bindings_match_shapes_leaves_and_resources() {
    let opening = base();
    let target_base = target(&opening);
    let hash = opening["releaseEnvelopeSha256"].clone();
    let original = sample(&target_base, &hash, &opening);
    let mut cases = vec![original.clone()];
    for value in [
        Value::Null,
        json!(false),
        json!(0),
        json!("x"),
        json!([]),
        json!({}),
        json!({"signed":target_base}),
    ] {
        cases.push(sample(&value, &hash, &opening));
    }
    for scope in [
        Value::Null,
        json!(false),
        json!(0),
        json!("x"),
        json!([]),
        json!({}),
        json!({"targetKey":null}),
        json!({"targetKey":0}),
        json!({"targetKey":"other"}),
    ] {
        let mut target = target_base.clone();
        target["scope"] = scope;
        cases.push(sample(&target, &hash, &opening));
    }
    for value in [
        Value::Null,
        json!(false),
        json!(0),
        json!([]),
        json!({}),
        json!("A".repeat(64)),
        json!("a".repeat(63)),
        json!(format!("{}\n", "a".repeat(64))),
        json!(format!("{}\r", "a".repeat(64))),
        json!(format!("{}\r\n", "a".repeat(64))),
    ] {
        cases.push(sample(&target_base, &value, &opening));
        let mut target = target_base.clone();
        target["rolloutSetCommitment"] = value;
        cases.push(sample(&target, &hash, &opening));
    }
    for field in ["scope", "rolloutSetCommitment"] {
        let mut missing = target_base.clone();
        missing.as_object_mut().unwrap().remove(field);
        cases.push(sample(&missing, &hash, &opening));
    }
    let mut upper = target_base.clone();
    upper["scope"]["targetKey"] = json!(opening["targetKey"].as_str().unwrap().to_uppercase());
    cases.push(sample(&upper, &hash, &opening));
    upper = target_base.clone();
    upper["rolloutSetCommitment"] = json!(
        upper["rolloutSetCommitment"]
            .as_str()
            .unwrap()
            .to_uppercase()
    );
    cases.push(sample(&upper, &hash, &opening));
    let padded = json!(format!(" {} ", hash.as_str().unwrap()));
    cases.push(sample(&target_base, &padded, &opening));
    for field in opening.as_object().unwrap().keys() {
        let mut missing = opening.clone();
        missing.as_object_mut().unwrap().remove(field);
        cases.push(sample(&target_base, &hash, &missing));
        for value in [
            Value::Null,
            json!(false),
            json!(0),
            json!(1.5),
            json!(""),
            json!("x\n"),
            json!([]),
            json!({}),
        ] {
            let mut changed = opening.clone();
            changed[field] = value;
            cases.push(sample(&target_base, &hash, &changed));
        }
        let mut changed = opening.clone();
        changed[field] = match field.as_str() {
            "targetKey" => json!("windows-x86_64-nsis-perMachine"),
            "deploymentRevision" | "rolloutRevision" => json!(2),
            "releaseEnvelopeSha256" | "leafSalt" => json!("0".repeat(64)),
            _ => json!("changed-id"),
        };
        let recalculated = target(&changed);
        cases.push(sample(&target_base, &hash, &changed));
        cases.push(sample(&recalculated, &hash, &changed));
        cases.push(sample(
            &recalculated,
            &changed["releaseEnvelopeSha256"],
            &changed,
        ));
    }
    for value in [
        Value::Null,
        json!(false),
        json!(0),
        json!("x"),
        json!([]),
        json!({}),
        json!([opening.clone()]),
    ] {
        cases.push(sample(&target_base, &hash, &value));
    }
    let empty = Document::parse(b"null", InputKind::Metadata)
        .unwrap()
        .rollout_commitment()
        .unwrap();
    let target =
        json!({"scope":target_base["scope"],"rolloutSetCommitment":empty.commitment_hash()});
    cases.push(sample(&target, &hash, &Value::Null));
    let mut extra = opening.clone();
    extra["extra"] = Value::Null;
    cases.push(sample(&target_base, &hash, &extra));
    for index in 0..3 {
        let mut changed = original.clone();
        changed[index] = wire(&json!("x".repeat(65_537)));
        cases.push(changed);
        let mut exact = vec![b' '; 1_048_576 - original[index].len()];
        exact.extend_from_slice(&original[index]);
        let mut changed = original.clone();
        changed[index] = exact;
        cases.push(changed.clone());
        changed[index].push(b' ');
        cases.push(changed);
        changed = original.clone();
        changed[index] = br#"{"extra":0,"e\u0078tra":1}"#.to_vec();
        cases.push(changed);
        changed = original.clone();
        changed[index] = vec![b'"', 0xff, b'"'];
        cases.push(changed);
    }
    let mut pretty = original.clone();
    pretty[0] = serde_json::to_vec_pretty(&target_base).unwrap();
    pretty[2] = serde_json::to_vec_pretty(&opening).unwrap();
    cases.push(pretty);
    let text = String::from_utf8(original[0].clone())
        .unwrap()
        .replace("\"targetKey\"", "\"target\\u004bey\"");
    let mut escaped = original.clone();
    escaped[0] = text.into_bytes();
    cases.push(escaped);
    let samples: Vec<_> = cases
        .iter()
        .map(|values| {
            values
                .iter()
                .map(|bytes| STANDARD.encode(bytes))
                .collect::<Vec<_>>()
        })
        .collect();
    let expected: Vec<_> = cases.iter().map(accepted).collect();
    let reference = root()
        .join("specs/upgrade/v1/reference/semantics.mjs")
        .to_string_lossy()
        .into_owned();
    let mut child = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            include_str!("opening-oracle.mjs"),
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
        "original opening oracle unavailable"
    );
    let actual: Vec<bool> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
        assert!(
            actual == expected,
            "opening oracle mismatch at case {index}"
        );
    }
    eprintln!(
        "{} original Node/Ajv opening binding cases passed",
        expected.len()
    );
}

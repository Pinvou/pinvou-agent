use super::*;
use crate::{Import, manifest::Manifest};
use serde_json::Value;
use std::{path::PathBuf, process::Command, sync::OnceLock};

pub(crate) fn fixture() -> &'static Value {
    static INPUT: OnceLock<Value> = OnceLock::new();
    INPUT.get_or_init(|| {
        let leaf = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let reference = leaf
            .ancestors()
            .nth(7)
            .unwrap()
            .join("specs/upgrade/v1/reference/signatures.mjs");
        let output = Command::new("node")
            .arg(leaf.join("tests/resume-fixtures.mjs"))
            .arg(reference)
            .output()
            .expect("Node required for actual signed resume fixtures");
        assert!(
            output.status.success(),
            "signed resume fixture generation failed"
        );
        serde_json::from_slice(&output.stdout).unwrap()
    })
}
pub(crate) fn imported(name: &str) -> Import {
    serde_json::from_value(fixture()["cases"][name].clone()).unwrap()
}
fn manifest() -> Manifest {
    let f = fixture();
    crate::manifest::new(
        f["root"].as_str().unwrap().into(),
        f["rootSha256"].as_str().unwrap().into(),
        f["package"].as_str().unwrap().into(),
        f["expected"].as_str().unwrap().into(),
        &imported("original"),
    )
    .unwrap()
}
#[test]
fn resume_pin_accepts_only_an_explicit_lowercase_digest_not_a_path_or_authority() {
    assert!(ResumeCheckpointPin::from_independently_retained_sha256(&"a".repeat(64)).is_ok());
    for value in [
        "".into(),
        "a".repeat(63),
        "A".repeat(64),
        format!(" {}", "a".repeat(64)),
        "../resume.json".into(),
        "sameSource=true".into(),
    ] {
        assert!(matches!(
            ResumeCheckpointPin::from_independently_retained_sha256(&value),
            Err(Error::InvalidInput)
        ));
    }
}

#[test]
fn resume_checked_source_equality_ignores_only_the_new_signed_locator_and_window() {
    let manifest = manifest();
    let (old_url, original) = manifest.imported_source(&imported("original")).unwrap();
    let (new_url, rotated) = manifest.imported_source(&imported("rotated")).unwrap();
    assert_ne!(old_url, new_url);
    assert!(original == rotated);
    for name in ["sourceId", "objectId", "objectVersion"] {
        let changed = manifest.imported_source(&imported(name)).unwrap().1;
        assert!(
            original != changed,
            "a genuinely signed changed entity must remain distinct"
        );
    }
}

#[test]
fn resume_closed_checkpoint_data_cannot_supply_a_missing_or_alias_source_field() {
    let proof = manifest().imported_source(&imported("original")).unwrap().1;
    let body = serde_json::json!({"protocolVersion":1,"manifestSha256":"a".repeat(64),"attempt":1,
        "identity":{"volume":7,"indexHigh":9,"indexLow":11,"createdHigh":13,"createdLow":15},
        "source":{"sourceId":proof.source_id(),"objectId":proof.object_id(),"objectVersion":proof.object_version(),
            "packageId":proof.package_id(),"size":proof.size(),"sha256":proof.sha256()},
        "length":2,"prefixSha256":crate::hash(b"ab"),"validator":[34,118,34]});
    let record: checkpoint::Checkpoint = serde_json::from_value(body.clone()).unwrap();
    assert!(record.source.matches(&proof));
    for key in [
        "sourceId",
        "objectId",
        "objectVersion",
        "packageId",
        "size",
        "sha256",
    ] {
        let mut missing = body.clone();
        missing["source"].as_object_mut().unwrap().remove(key);
        assert!(serde_json::from_value::<checkpoint::Checkpoint>(missing).is_err());
        let mut null = body.clone();
        null["source"][key] = Value::Null;
        assert!(serde_json::from_value::<checkpoint::Checkpoint>(null).is_err());
    }
    let mut alias = body.clone();
    alias["source"]["SourceId"] = Value::String(proof.source_id().into());
    assert!(serde_json::from_value::<checkpoint::Checkpoint>(alias).is_err());
    let duplicate =
        serde_json::to_string(&body)
            .unwrap()
            .replacen('{', "{\"protocolVersion\":1,", 1);
    assert!(serde_json::from_str::<checkpoint::Checkpoint>(&duplicate).is_err());
}

#[test]
fn resume_range_keeps_strong_etag_octets_and_rejects_a_weak_validator() {
    use pinvou_upgrade_transfer::RequestKind;
    let proof = manifest().imported_source(&imported("original")).unwrap().1;
    let artifact = Artifact::new(proof.package_id(), proof.size(), proof.sha256()).unwrap();
    let prefix = RetainedPrefix {
        length: 2,
        validator: vec![34, 118, 92, 0xff, 34],
    };
    match prefix.plan(artifact).unwrap().kind() {
        RequestKind::Range { offset, validator } => {
            assert_eq!(offset, 2);
            assert_eq!(validator.as_bytes(), &[34, 118, 92, 0xff, 34]);
        }
        _ => panic!("verified partial must plan Range"),
    }
    let weak = RetainedPrefix {
        length: 2,
        validator: b"W/\"v\"".to_vec(),
    };
    assert!(matches!(weak.plan(artifact), Err(Error::InvalidInput)));
}

#[test]
fn resume_pre_get_revalidates_real_signed_windows_after_local_preparation() {
    let manifest = manifest();
    let import = imported("original");
    assert!(
        crate::runner::revalidate_before_get(&manifest, &import, std::time::Duration::ZERO).is_ok()
    );
    let request: Value = serde_json::from_str(&import.request).unwrap();
    let response: Value = serde_json::from_str(&import.response).unwrap();
    let end = [
        request["decision"]["signed"]["exp"].as_f64().unwrap() as u64,
        response["credential"]["signed"]["exp"].as_f64().unwrap() as u64,
        response["expiresAt"].as_f64().unwrap() as u64,
    ]
    .into_iter()
    .min()
    .unwrap();
    let remaining = end.checked_sub(import.supplied_time_millis).unwrap();
    assert!(remaining > 0);
    assert!(
        crate::runner::revalidate_before_get(
            &manifest,
            &import,
            std::time::Duration::from_millis(remaining - 1)
        )
        .is_ok()
    );
    for elapsed in [
        std::time::Duration::from_millis(remaining),
        std::time::Duration::from_millis(remaining - 1) + std::time::Duration::from_nanos(1),
    ] {
        assert!(matches!(
            crate::runner::revalidate_before_get(&manifest, &import, elapsed),
            Err(Error::SignatureRefused)
        ));
    }
    assert!(matches!(
        crate::runner::revalidate_before_get(&manifest, &import, std::time::Duration::MAX),
        Err(Error::InvalidInput)
    ));
    // Rechecking does not weaken original binding to gain a refreshed credential.
    let mut wrong = imported("original");
    let mut response: Value = serde_json::from_str(&wrong.response).unwrap();
    response["url"] = Value::String("https://files.example.invalid/not-signed".into());
    wrong.response = serde_json::to_string(&response).unwrap();
    assert!(matches!(
        crate::runner::revalidate_before_get(&manifest, &wrong, std::time::Duration::ZERO),
        Err(Error::SignatureRefused)
    ));
}

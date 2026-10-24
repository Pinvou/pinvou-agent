use base64::{Engine, engine::general_purpose::STANDARD};
use pinvou_upgrade_codec::{Document, DownloadInfoContractError, Error, InputKind};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
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
fn base() -> Value {
    // Generate a genuine new-contract credential with the existing reference
    // fixture. The unchanged fixed legacy Download is a rejection control.
    let output = Command::new("node")
        .args([
            "--input-type=module",
            "-e",
            r#"
import {pathToFileURL} from 'node:url';
const source=pathToFileURL(process.argv[1]);
const {createFixtureSet}=await import(source);
const {sha256}=await import(new URL('../digests.mjs',source));
const {parseJson}=await import(new URL('../canonical-json.mjs',source));
const f=createFixtureSet(),url='https://example.invalid/file';
const claims=structuredClone(f.claims.download);
claims.sourceBinding.urlSha256=sha256(Buffer.from(url,'utf8'));
const c=parseJson(f.sign(claims));
console.log(JSON.stringify({protocolVersion:1,requestId:'request-1',credential:c,
packageId:c.signed.packageId,size:c.signed.package.size,sha256:c.signed.package.sha256,
url,expiresAt:c.signed.exp,fileSourceClass:'primary'}));
"#,
        ])
        .arg(root().join("specs/upgrade/v1/reference/tests/fixtures.mjs"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "new-contract fixture unavailable: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn set_url(value: &mut Value, url: &str) {
    let hash: String = Sha256::digest(url.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    value["url"] = json!(url);
    value["credential"]["signed"]["sourceBinding"]["urlSha256"] = json!(hash);
}

fn check(v: &Value) -> Result<(), DownloadInfoContractError> {
    Document::parse(&serde_json::to_vec(v).unwrap(), InputKind::Metadata)
        .unwrap()
        .assert_supplied_download_info_contract()
}
#[test]
fn package_and_locator_window_bindings_have_isolated_negatives_and_restored_positives() {
    let b = base();
    assert_eq!(check(&b), Ok(()));
    for (field, other) in [
        ("packageId", json!("other-package")),
        ("size", json!(b["size"].as_u64().unwrap() + 1)),
        ("sha256", json!("0".repeat(64))),
        (
            "expiresAt",
            json!(b["credential"]["signed"]["exp"].as_u64().unwrap() + 1),
        ),
    ] {
        let mut v = b.clone();
        assert_ne!(v[field], other);
        v[field] = other;
        assert_eq!(
            check(&v),
            Err(DownloadInfoContractError::BindingMismatch),
            "{field}"
        );
        v[field] = b[field].clone();
        assert_eq!(check(&v), Ok(()));
    }
    let nbf = b["credential"]["signed"]["nbf"].as_u64().unwrap();
    let mut v = b.clone();
    v["expiresAt"] = json!(nbf);
    assert_eq!(check(&v), Err(DownloadInfoContractError::BindingMismatch));
    v["expiresAt"] = json!(nbf + 1);
    assert_eq!(check(&v), Ok(()));
    v["expiresAt"] = b["expiresAt"].clone();
    v["requestId"] = json!("another-request");
    v["fileSourceClass"] = json!("another-source");
    assert_eq!(check(&v), Ok(()));
}
#[test]
fn contract_data_does_not_approve_url_signature_current_time_or_a_download_deadline() {
    let mut v = base();
    v["credential"]["signed"]["nbf"] =
        json!(v["credential"]["signed"]["nbf"].as_u64().unwrap() + 1);
    v["credential"]["signatures"][0]["signature"] = json!("A".repeat(86));
    let sig = v["credential"]["signatures"][0].clone();
    v["credential"]["signatures"]
        .as_array_mut()
        .unwrap()
        .push(sig);
    for url in ["https://", "https://\n"] {
        set_url(&mut v, url);
        assert_eq!(check(&v), Ok(()));
    }
    set_url(&mut v, &("https://".to_owned() + &"a".repeat(8184)));
    assert_eq!(check(&v), Ok(()));
    set_url(&mut v, &("https://".to_owned() + &"a".repeat(8185)));
    assert_eq!(check(&v), Err(DownloadInfoContractError::InvalidContract));
    set_url(&mut v, &("https://".to_owned() + &"\u{1f600}".repeat(8184)));
    assert_eq!(check(&v), Ok(()));
    set_url(&mut v, &("https://".to_owned() + &"\u{1f600}".repeat(8185)));
    assert_eq!(check(&v), Err(DownloadInfoContractError::InvalidContract));
    set_url(&mut v, "https://example.invalid/file");
    v["credential"]["signed"]["nbf"] = v["credential"]["signed"]["exp"].clone();
    assert!(matches!(
        check(&v),
        Err(DownloadInfoContractError::Semantics(_))
    ));
}
#[test]
fn embedded_credential_and_complete_raw_input_bounds_cannot_be_bypassed() {
    let mut v = base();
    let sig = v["credential"]["signatures"][0].clone();
    while Document::parse(
        &serde_json::to_vec(&v["credential"]).unwrap(),
        InputKind::Metadata,
    )
    .unwrap()
    .canonical_bytes()
    .unwrap()
    .len()
        <= 65536
    {
        v["credential"]["signatures"]
            .as_array_mut()
            .unwrap()
            .push(sig.clone());
    }
    let last = v["credential"]["signatures"]
        .as_array_mut()
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(check(&v), Ok(()));
    v["credential"]["signatures"]
        .as_array_mut()
        .unwrap()
        .push(last);
    assert_eq!(
        check(&v),
        Err(DownloadInfoContractError::CredentialTooLarge)
    );
    let v = base();
    let mut raw = serde_json::to_vec(&v).unwrap();
    raw.resize(1024 * 1024, b' ');
    assert_eq!(
        Document::parse(&raw, InputKind::Metadata)
            .unwrap()
            .assert_supplied_download_info_contract(),
        Ok(())
    );
    raw.push(b' ');
    assert_eq!(
        Document::parse(&raw, InputKind::Metadata).err(),
        Some(Error::InputLimit)
    );
    let raw = serde_json::to_string(&v).unwrap().replacen(
        "\"requestId\":\"request-1\"",
        "\"requestId\":\"request-1\",\"requestId\":\"request-1\"",
        1,
    );
    assert_eq!(
        Document::parse(raw.as_bytes(), InputKind::Metadata).err(),
        Some(Error::DuplicateKey)
    );
}
#[test]
fn actual_original_complete_api_contract_matches_required_scalar_unicode_and_binding_controls() {
    let reference = root().join("specs/upgrade/v1/reference/api-registry.mjs");
    let output = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            include_str!("download-info-oracle.mjs"),
            reference.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "original API oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cases: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert!(cases.len() > 1000);
    for c in &cases {
        let bytes = STANDARD.decode(c["bytes"].as_str().unwrap()).unwrap();
        let actual = Document::parse(&bytes, InputKind::Metadata)
            .and_then(|d| {
                d.assert_supplied_download_info_contract()
                    .map_err(|_| Error::InvalidJson)
            })
            .is_ok();
        assert_eq!(actual, c["accepted"].as_bool().unwrap(), "{}", c["name"]);
    }
    eprintln!(
        "{} actual original download-info response cases passed",
        cases.len()
    );
}

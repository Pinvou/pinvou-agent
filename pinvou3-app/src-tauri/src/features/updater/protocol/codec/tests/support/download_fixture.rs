// Narrow bridge to the existing in-memory Ed25519 fixture, not a persisted key/vector.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pinvou_upgrade_codec::{InputKind, SchemaError, decode_canonical_envelope};
use serde_json::Value;
use std::{path::Path, process::Command, sync::OnceLock};

pub fn current_download(reference: &Path) -> Value {
    static FIXTURE: OnceLock<Value> = OnceLock::new();
    FIXTURE
        .get_or_init(|| current_download_for_url(reference, None))
        .clone()
}

pub fn current_download_for_url(reference: &Path, url: Option<&str>) -> Value {
    let mut command = Command::new("node");
    command.args([
        "--input-type=module",
        "--eval",
        include_str!("download-fixture.mjs"),
    ]);
    command.arg(reference);
    if let Some(url) = url {
        command.arg(url);
    }
    let output = command
        .output()
        .expect("Node is required for the real signed download fixture");
    assert!(
        output.status.success(),
        "current download fixture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let vector: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(vector["name"], "download");
    vector
}

pub fn replace_legacy_download(vectors: &mut [Value], reference: &Path) {
    let mut replaced = 0;
    for vector in vectors.iter_mut().filter(|v| v["name"] == "download") {
        // Keep the frozen old bytes as an explicit schema-rejection control.
        let bytes = URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap();
        let original: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(original["signed"].get("sourceBinding").is_none());
        assert!(matches!(
            decode_canonical_envelope(&bytes, InputKind::Credential)
                .unwrap()
                .check_full_schema(),
            Err(SchemaError::InvalidEnvelope)
        ));
        *vector = current_download(reference);
        replaced += 1;
    }
    assert_eq!(
        replaced, 1,
        "the original inventory has exactly one legacy download"
    );
}

use super::*;
use serde_json::Value;
use std::{path::PathBuf, process::Command, sync::OnceLock};

#[test]
fn missing_tokio_context_is_a_typed_error_before_job_or_network_access() {
    use std::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    let path = std::env::temp_dir().join(format!(
        "pinvou-no-runtime-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    assert!(!path.exists());
    let mut future = Box::pin(run(
        path.clone(),
        "0".repeat(64),
        Import {
            request: String::new(),
            response: String::new(),
            supplied_time_millis: 0,
        },
        NetworkConsent {
            host: String::new(),
            port: 0,
            addresses: Vec::new(),
        },
        false,
        |_| panic!("no observer may run without runtime support"),
    ));
    assert!(matches!(
        future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Err(Error::IoUnavailable))
    ));
    drop(future);
    assert!(!path.exists());
}

pub(crate) fn fixture() -> &'static Value {
    static FIXTURE: OnceLock<Value> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let reference = crate_dir
            .ancestors()
            .nth(7)
            .unwrap()
            .join("specs/upgrade/v1/reference/signatures.mjs");
        let output = Command::new("node")
            .arg(crate_dir.join("tests/fixtures.mjs"))
            .arg(reference)
            .output()
            .expect("Node is required for actual signed QA fixtures");
        assert!(output.status.success(), "signed fixture generation failed");
        serde_json::from_slice(&output.stdout).unwrap()
    })
}
pub(crate) fn imported() -> Import {
    serde_json::from_value(fixture()["imported"].clone()).unwrap()
}

#[test]
fn signed_import_matches_original_package_and_rejects_changed_request_or_signature() {
    let f = fixture();
    let build = |input: &Import, pin: &str| {
        manifest::new(
            f["root"].as_str().unwrap().into(),
            pin.into(),
            f["package"].as_str().unwrap().into(),
            f["expected"].as_str().unwrap().into(),
            input,
        )
    };
    assert!(build(&imported(), f["rootSha256"].as_str().unwrap()).is_ok());
    assert!(matches!(
        build(&imported(), &"0".repeat(64)),
        Err(Error::PinMismatch)
    ));
    let mut input = imported();
    let mut response: Value = serde_json::from_str(&input.response).unwrap();
    response["credential"]["signatures"][0]["signature"] = Value::String("A".repeat(86));
    input.response = response.to_string();
    assert!(build(&input, f["rootSha256"].as_str().unwrap()).is_err());
    let mut input = imported();
    input.request = input.request.replace("package-2", "package-3");
    assert!(build(&input, f["rootSha256"].as_str().unwrap()).is_err());
    let mut input = imported();
    input.supplied_time_millis += 900_000;
    assert!(build(&input, f["rootSha256"].as_str().unwrap()).is_err());
    assert!(build(&imported(), f["rootSha256"].as_str().unwrap()).is_ok());
}

#[test]
fn actual_resigned_new_session_keeps_original_workflow_and_stage() {
    use pinvou_upgrade_codec::{Document, InputKind};
    use pinvou_upgrade_verifier::{DownloadVerificationInputs, verify_download_info_response};
    let f = fixture();
    let manifest = manifest::new(
        f["root"].as_str().unwrap().into(),
        f["rootSha256"].as_str().unwrap().into(),
        f["package"].as_str().unwrap().into(),
        f["expected"].as_str().unwrap().into(),
        &imported(),
    )
    .unwrap();
    for kind in ["sessionImport", "workflowImport", "stageImport"] {
        let input: Import = serde_json::from_value(f[kind].clone()).unwrap();
        let root = Document::parse(f["root"].as_str().unwrap().as_bytes(), InputKind::Metadata)
            .unwrap()
            .root_policy_shape()
            .unwrap();
        let request = Document::parse(input.request.as_bytes(), InputKind::Metadata).unwrap();
        let now = Document::parse(
            input.supplied_time_millis.to_string().as_bytes(),
            InputKind::Metadata,
        )
        .unwrap()
        .protocol_time_shape()
        .unwrap();
        assert!(
            verify_download_info_response(
                input.response.as_bytes(),
                DownloadVerificationInputs::new(&root, &request, &now)
            )
            .is_ok(),
            "negative must first be genuinely signed and contract valid: {kind}"
        );
        let result = manifest.imported_url(&input);
        if kind == "sessionImport" {
            assert!(result.is_ok());
        } else {
            assert!(matches!(result, Err(Error::BindingMismatch)));
        }
    }
    assert!(manifest.imported_url(&imported()).is_ok());
}

use super::*;
use crate::{
    Import, manifest,
    tests::{fixture, imported},
};
use serde_json::Value;

fn original_manifest() -> manifest::Manifest {
    let f = fixture();
    manifest::new(
        f["root"].as_str().unwrap().into(),
        f["rootSha256"].as_str().unwrap().into(),
        f["package"].as_str().unwrap().into(),
        f["expected"].as_str().unwrap().into(),
        &imported(),
    )
    .unwrap()
}
fn input(import: &Import) -> ControlAttempt {
    ControlAttempt {
        request: import.request.clone(),
        supplied_time_millis: import.supplied_time_millis,
    }
}
fn consent() -> NetworkConsent {
    NetworkConsent {
        host: "127.0.0.1".into(),
        port: 1,
        addresses: vec!["127.0.0.1".parse().unwrap()],
    }
}

#[test]
fn control_input_closed_shape_cannot_supply_origin_response_or_authority() {
    let valid = r#"{"request":"{}","suppliedTimeMillis":1}"#;
    assert!(serde_json::from_str::<ControlAttempt>(valid).is_ok());
    for field in [
        "response",
        "url",
        "headers",
        "root",
        "sameSource",
        "currentQualificationChecked",
    ] {
        let value = format!("{{\"request\":\"{{}}\",\"suppliedTimeMillis\":1,\"{field}\":true}}");
        assert!(serde_json::from_str::<ControlAttempt>(&value).is_err());
    }
    for value in [
        r#"{"request":"{}"}"#,
        r#"{"request":"{}","suppliedTimeMillis":null}"#,
        r#"{"request":"{}","suppliedTimeMillis":-1}"#,
        r#"{"request":"{}","request":"{}","suppliedTimeMillis":1}"#,
    ] {
        assert!(serde_json::from_str::<ControlAttempt>(value).is_err());
    }
}

#[test]
fn control_original_signatures_and_resigned_session_retain_same_package_projection() {
    let manifest = original_manifest();
    for name in ["imported", "sessionImport"] {
        let original: Import = serde_json::from_value(fixture()[name].clone()).unwrap();
        let prepared = PreparedControl::new(&manifest, input(&original)).unwrap();
        let result = prepared
            .finish_for_test(
                &manifest,
                original.response.as_bytes().to_vec(),
                original.supplied_time_millis,
            )
            .unwrap();
        assert_eq!(result.request, original.request);
        assert_eq!(result.response, original.response);
        assert_eq!(
            manifest.imported_url(&result).unwrap(),
            "https://127.0.0.1:1/file"
        );
    }
}

#[test]
fn control_genuinely_signed_other_workflow_and_stage_fail_before_post() {
    let manifest = original_manifest();
    for name in ["workflowImport", "stageImport"] {
        let original: Import = serde_json::from_value(fixture()[name].clone()).unwrap();
        // First prove these are genuinely signed original contract fixtures.
        let root = pinvou_upgrade_codec::Document::parse(
            manifest.root.as_bytes(),
            pinvou_upgrade_codec::InputKind::Metadata,
        )
        .unwrap()
        .root_policy_shape()
        .unwrap();
        let request = pinvou_upgrade_codec::Document::parse(
            original.request.as_bytes(),
            pinvou_upgrade_codec::InputKind::Metadata,
        )
        .unwrap();
        let now = pinvou_upgrade_codec::Document::parse(
            original.supplied_time_millis.to_string().as_bytes(),
            pinvou_upgrade_codec::InputKind::Metadata,
        )
        .unwrap()
        .protocol_time_shape()
        .unwrap();
        assert!(
            pinvou_upgrade_verifier::verify_download_info_response(
                original.response.as_bytes(),
                pinvou_upgrade_verifier::DownloadVerificationInputs::new(&root, &request, &now),
            )
            .is_ok()
        );
        assert!(matches!(
            PreparedControl::new(&manifest, input(&original)),
            Err(Error::BindingMismatch)
        ));
    }
}

#[test]
fn control_request_signature_scope_package_and_duplicate_wire_are_not_normalized() {
    let manifest = original_manifest();
    let mut source = imported();
    let mut request: Value = serde_json::from_str(&source.request).unwrap();
    request["decision"]["signatures"][0]["signature"] = Value::String("A".repeat(86));
    source.request = request.to_string();
    assert!(matches!(
        PreparedControl::new(&manifest, input(&source)),
        Err(Error::SignatureRefused)
    ));
    for change in ["scope", "package"] {
        let mut source = imported();
        let mut request: Value = serde_json::from_str(&source.request).unwrap();
        if change == "scope" {
            request["scope"]["installId"] = Value::String("other-install".into());
        } else {
            request["packageId"] = Value::String("other-package".into());
        }
        source.request = request.to_string();
        assert!(PreparedControl::new(&manifest, input(&source)).is_err());
    }
    let mut source = imported();
    source.request = source.request.replacen('{', "{\"protocolVersion\":1,", 1);
    assert!(matches!(
        PreparedControl::new(&manifest, input(&source)),
        Err(Error::InvalidInput)
    ));
    let mut source = imported();
    source.request = " ".repeat(crate::DOCUMENT_LIMIT + 1);
    assert!(matches!(
        PreparedControl::new(&manifest, input(&source)),
        Err(Error::InvalidInput)
    ));
}

#[test]
fn control_response_errors_bad_signatures_duplicates_and_expiry_never_become_imports() {
    let manifest = original_manifest();
    let original = imported();
    let prepared = PreparedControl::new(&manifest, input(&original)).unwrap();
    for status in [201, 302, 400, 401, 403, 409, 429, 500, 503] {
        assert!(matches!(
            prepared.finish(&manifest, status, original.response.as_bytes().to_vec()),
            Err(Error::ControlRefused)
        ));
    }
    let mut body: Value = serde_json::from_str(&original.response).unwrap();
    let expiry = body["expiresAt"].as_u64().unwrap();
    assert!(
        prepared
            .finish_for_test(&manifest, original.response.as_bytes().to_vec(), expiry)
            .is_err()
    );
    body["credential"]["signatures"][0]["signature"] = Value::String("A".repeat(86));
    assert!(matches!(
        prepared.finish_for_test(
            &manifest,
            body.to_string().into_bytes(),
            original.supplied_time_millis
        ),
        Err(Error::SignatureRefused)
    ));
    let duplicate = original
        .response
        .replacen('{', "{\"protocolVersion\":1,", 1);
    assert!(
        prepared
            .finish_for_test(
                &manifest,
                duplicate.into_bytes(),
                original.supplied_time_millis
            )
            .is_err()
    );
    assert!(
        prepared
            .finish_for_test(
                &manifest,
                vec![b' '; crate::DOCUMENT_LIMIT + 1],
                original.supplied_time_millis
            )
            .is_err()
    );
    assert!(
        prepared
            .finish_for_test(&manifest, vec![0xff], original.supplied_time_millis)
            .is_err()
    );
    assert!(
        prepared
            .finish_for_test(
                &manifest,
                original.response.into_bytes(),
                original.supplied_time_millis
            )
            .is_ok()
    );
}

#[test]
fn control_supplied_time_is_bounded_and_cannot_rewind_original_observation() {
    let manifest = original_manifest();
    let original = imported();
    for supplied_time_millis in [
        manifest.initial_supplied_time_millis - 1,
        crate::MAX_SAFE + 1,
        u64::MAX,
    ] {
        let request = ControlAttempt {
            request: original.request.clone(),
            supplied_time_millis,
        };
        assert!(matches!(
            PreparedControl::new(&manifest, request),
            Err(Error::BindingMismatch)
        ));
    }
}

#[tokio::test]
async fn control_wait_returns_original_failure_and_has_no_retry() {
    let calls = std::cell::Cell::new(0);
    let result: Result<(), Error> = await_control(
        async {
            calls.set(calls.get() + 1);
            Err(Error::ControlUnavailable)
        },
        &AtomicBool::new(false),
    )
    .await;
    assert!(matches!(result, Err(Error::ControlUnavailable)));
    assert_eq!(calls.get(), 1);
    assert_eq!(
        Error::ControlUnavailable.to_string(),
        "DOWNLOAD_JOB_CONTROL_UNAVAILABLE"
    );
    assert_eq!(
        Error::ControlRefused.to_string(),
        "DOWNLOAD_JOB_CONTROL_REFUSED"
    );
}

#[tokio::test]
async fn control_cancellation_drops_the_actual_pending_future_without_detached_request() {
    struct Held(Arc<AtomicBool>);
    impl Drop for Held {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let closed = Arc::new(AtomicBool::new(false));
    let held = Held(closed.clone());
    let stop = Arc::new(AtomicBool::new(false));
    let signal = stop.clone();
    let cancel = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        signal.store(true, Ordering::Release);
    });
    let result: Result<(), Error> = await_control(
        async move {
            let _held = held;
            std::future::pending().await
        },
        &stop,
    )
    .await;
    cancel.await.unwrap();
    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(closed.load(Ordering::Acquire));
}

#[test]
fn control_without_runtime_fails_before_files_or_network() {
    use std::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    let path =
        std::env::temp_dir().join(format!("pinvou-control-no-runtime-{}", std::process::id()));
    let mut future = Box::pin(run_control(
        path,
        "0".repeat(64),
        ControlAttempt {
            request: String::new(),
            supplied_time_millis: 0,
        },
        consent(),
        false,
        |_| panic!("unexpected observation"),
    ));
    assert!(matches!(
        future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Err(Error::IoUnavailable))
    ));
}

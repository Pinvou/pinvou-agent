use super::*;
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::{Value, json};
use std::{path::Path, process::Command};

fn wire_cases() -> &'static Vec<Value> {
    static CASES: OnceLock<Vec<Value>> = OnceLock::new();
    CASES.get_or_init(|| {
        if let Some(path) = std::env::var_os("PINVOU_CONTROL_TLS_FIXTURE") {
            let fixture: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            return serde_json::from_slice(
                &std::fs::read(fixture["wireCasesPath"].as_str().unwrap()).unwrap(),
            )
            .unwrap();
        }
        let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let reference = crate_root
            .ancestors()
            .nth(7)
            .unwrap()
            .join("specs/upgrade/v1/reference/canonical-json.mjs");
        let result = Command::new("node")
            .arg(crate_root.join("tests/wire-oracle.mjs"))
            .arg(reference)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        serde_json::from_slice(&result.stdout).unwrap()
    })
}
fn request(mode: &str) -> Vec<u8> {
    let baseline = wire_cases()
        .iter()
        .find(|c| c["operation"] == "consume-status")
        .unwrap();
    let mut value: Value = serde_json::from_str(baseline["request"].as_str().unwrap()).unwrap();
    value["transactionId"] = json!(format!("case-{mode}"));
    serde_json::to_vec(&value).unwrap()
}
#[test]
fn every_original_client_route_and_fixture_is_complete_exact_and_non_management() {
    let cases = wire_cases();
    assert_eq!(cases.len(), Operation::ALL.len());
    let mut seen = std::collections::BTreeSet::new();
    for case in cases {
        let operation = case["operation"]
            .as_str()
            .unwrap()
            .parse::<Operation>()
            .unwrap();
        assert!(seen.insert(operation.as_str()));
        assert_eq!(
            routes::path(operation).unwrap(),
            case["path"].as_str().unwrap()
        );
        Document::parse(
            case["request"].as_str().unwrap().as_bytes(),
            InputKind::Metadata,
        )
        .unwrap()
        .assert_supplied_client_request_contract(operation)
        .unwrap();
        Document::parse(
            case["response"].as_str().unwrap().as_bytes(),
            InputKind::Metadata,
        )
        .unwrap()
        .assert_supplied_client_http_response_contract(operation, 200)
        .unwrap();
    }
    assert_eq!(CONTROL_ORIGIN, "https://update.pinvou.com");
}
#[tokio::test]
async fn invalid_and_duplicate_request_bytes_fail_before_any_transport() {
    assert!(matches!(
        post_once(Operation::ConsumeStatus, b"{}").await,
        Err(ControlHttpError::RequestContract(_))
    ));
    assert!(matches!(
        post_once(
            Operation::ConsumeStatus,
            b"{\"protocolVersion\":1,\"protocolVersion\":1}"
        )
        .await,
        Err(ControlHttpError::RequestJson(Error::DuplicateKey))
    ));
}
#[test]
fn header_boundaries_reject_encoding_framing_media_type_and_oversize() {
    let mut base = HeaderMap::new();
    base.insert(
        reqwest::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    assert_eq!(check_headers(&base), Ok(()));
    for (name, value, expected) in [
        (
            reqwest::header::CONTENT_TYPE,
            "text/html",
            ControlHttpError::InvalidHeaders,
        ),
        (
            reqwest::header::CONTENT_ENCODING,
            "gzip",
            ControlHttpError::InvalidHeaders,
        ),
        (
            reqwest::header::TRANSFER_ENCODING,
            "gzip",
            ControlHttpError::InvalidHeaders,
        ),
        (
            reqwest::header::CONTENT_LENGTH,
            "1048577",
            ControlHttpError::ResponseTooLarge,
        ),
    ] {
        let mut bad = base.clone();
        bad.insert(name, HeaderValue::from_static(value));
        assert_eq!(check_headers(&bad), Err(expected));
        assert_eq!(check_headers(&base), Ok(()));
    }
    let mut exact = base.clone();
    exact.insert(
        reqwest::header::CONTENT_LENGTH,
        HeaderValue::from_static("1048576"),
    );
    assert_eq!(check_headers(&exact), Ok(()));
    exact.insert(
        reqwest::header::TRANSFER_ENCODING,
        HeaderValue::from_static("chunked"),
    );
    assert_eq!(check_headers(&exact), Err(ControlHttpError::InvalidHeaders));
    for spelling in [
        "application/json",
        "APPLICATION/JSON; CHARSET=UTF-8",
        "application/json; charset=\"utf-8\"",
        " application/json\t;\tcharset = utf-8 ",
    ] {
        let mut good = base.clone();
        good.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_str(spelling).unwrap(),
        );
        assert_eq!(check_headers(&good), Ok(()));
    }
    for spelling in [
        "application/json; charset=\"utf-8",
        "application/json; charset=utf-8\"",
        "application/json; charset=\"\"utf-8\"\"",
        "application/json; charset=",
        "application/json; charset=iso-8859-1",
        "application/json; charset=utf-8; extra=1",
    ] {
        let mut bad = base.clone();
        bad.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_str(spelling).unwrap(),
        );
        assert_eq!(check_headers(&bad), Err(ControlHttpError::InvalidHeaders));
        assert_eq!(check_headers(&base), Ok(()));
    }
    for spelling in ["+1", "", "1 ", "18446744073709551616"] {
        let mut bad = base.clone();
        bad.insert(
            reqwest::header::CONTENT_LENGTH,
            HeaderValue::from_str(spelling).unwrap(),
        );
        assert_eq!(check_headers(&bad), Err(ControlHttpError::InvalidHeaders));
    }
    let mut repeated = base.clone();
    repeated.append(
        reqwest::header::CONTENT_LENGTH,
        HeaderValue::from_static("1"),
    );
    repeated.append(
        reqwest::header::CONTENT_LENGTH,
        HeaderValue::from_static("1"),
    );
    assert_eq!(
        check_headers(&repeated),
        Err(ControlHttpError::InvalidHeaders)
    );
    let mut duplicate = base.clone();
    duplicate.append(
        reqwest::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    assert_eq!(
        check_headers(&duplicate),
        Err(ControlHttpError::InvalidHeaders)
    );
}
pub(super) fn fixture_profile(trusted: bool) -> Profile {
    let path = std::env::var_os("PINVOU_CONTROL_TLS_FIXTURE").expect("run isolated HTTPS harness");
    let v: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let cert =
        reqwest::Certificate::from_pem(v["certificatePem"].as_str().unwrap().as_bytes()).unwrap();
    let address = v["address"].as_str().unwrap().parse().unwrap();
    let mut profile = Profile {
        connect: Duration::from_secs(2),
        read_idle: Duration::from_millis(150),
        fixture: Some((address, cert)),
    };
    if !trusted {
        // A separate in-memory certificate from the peer does not trust its TLS leaf.
        let other = reqwest::Certificate::from_pem(
            v["untrustedCertificatePem"].as_str().unwrap().as_bytes(),
        )
        .unwrap();
        profile.fixture = Some((address, other));
    }
    profile
}
#[tokio::test]
#[ignore = "opt in via tests/https_e2e.py; real isolated TLS peer"]
async fn real_https_posts_all_client_routes_and_rejects_unsafe_wire_results() {
    for case in wire_cases() {
        let operation = case["operation"]
            .as_str()
            .unwrap()
            .parse::<Operation>()
            .unwrap();
        let raw = format!(" \n{} \n", case["request"].as_str().unwrap());
        let result = post_with_profile(operation, raw.as_bytes(), fixture_profile(true))
            .await
            .unwrap();
        assert_eq!(result.operation(), operation);
        assert_eq!(result.status(), 200);
        assert!(
            result.bytes() == case["response"].as_str().unwrap().as_bytes(),
            "original response mismatch for {}",
            operation.as_str()
        );
        assert!(
            result.document().canonical_bytes().unwrap()
                == case["response"].as_str().unwrap().as_bytes(),
            "canonical response mismatch for {}",
            operation.as_str()
        );
    }
    let operation = Operation::ConsumeStatus;
    for (mode, expected) in [
        (
            "redirect",
            ControlHttpError::ResponseContract(ClientHttpResponseError::InvalidStatus),
        ),
        (
            "foreign-redirect",
            ControlHttpError::ResponseContract(ClientHttpResponseError::InvalidStatus),
        ),
        (
            "status",
            ControlHttpError::ResponseContract(ClientHttpResponseError::InvalidStatus),
        ),
        ("media", ControlHttpError::InvalidHeaders),
        ("charset", ControlHttpError::InvalidHeaders),
        ("gzip", ControlHttpError::InvalidHeaders),
        ("oversize", ControlHttpError::ResponseTooLarge),
        ("chunk-oversize", ControlHttpError::ResponseTooLarge),
        (
            "duplicate",
            ControlHttpError::ResponseJson(Error::DuplicateKey),
        ),
        ("headers-wait", ControlHttpError::RequestStalled),
        ("body-wait", ControlHttpError::RequestStalled),
    ] {
        let result = post_with_profile(operation, &request(mode), fixture_profile(true)).await;
        assert!(
            matches!(result,Err(actual) if actual==expected),
            "mode {mode}"
        );
        let restored = post_with_profile(operation, &request("restore"), fixture_profile(true))
            .await
            .unwrap();
        assert_eq!(restored.status(), 200);
    }
    for mode in [
        "cl-same",
        "cl-conflict",
        "cl-plus",
        "te-cl",
        "te-encoding",
        "te-duplicate",
        "truncated",
    ] {
        let result = post_with_profile(operation, &request(mode), fixture_profile(true)).await;
        assert!(
            matches!(
                result,
                Err(ControlHttpError::InvalidHeaders | ControlHttpError::NetworkFailed)
            ),
            "unsafe actual framing mode {mode}"
        );
        assert_eq!(
            post_with_profile(operation, &request("restore"), fixture_profile(true))
                .await
                .unwrap()
                .status(),
            200
        );
    }
    assert_eq!(
        post_with_profile(operation, &request("chunk-valid"), fixture_profile(true))
            .await
            .unwrap()
            .status(),
        200
    );
    assert!(matches!(
        post_with_profile(
            operation,
            &request("connection-drop"),
            fixture_profile(true)
        )
        .await,
        Err(ControlHttpError::NetworkFailed | ControlHttpError::ConnectionFailed)
    ));
    assert_eq!(
        post_with_profile(operation, &request("restore"), fixture_profile(true))
            .await
            .unwrap()
            .status(),
        200
    );
    let mut profile = fixture_profile(true);
    profile.read_idle = Duration::from_millis(500);
    let minimum = profile.read_idle * 2;
    let began = std::time::Instant::now();
    assert_eq!(
        post_with_profile(operation, &request("progress"), profile)
            .await
            .unwrap()
            .status(),
        200
    );
    assert!(
        began.elapsed() >= minimum,
        "ongoing progress must outlive multiple read-idle periods"
    );
    let declared = post_with_profile(operation, &request("declared-error"), fixture_profile(true))
        .await
        .unwrap();
    assert_eq!(declared.status(), 404);
    assert_eq!(
        declared.document().value()["code"],
        "UNKNOWN_CONSUME_OUTCOME"
    );
    let cap = post_with_profile(operation, &request("exact-cap"), fixture_profile(true))
        .await
        .unwrap();
    assert_eq!(cap.status(), 200);
    assert_eq!(cap.bytes().len(), MAX_RESPONSE_BYTES);
    assert!(matches!(
        post_with_profile(operation, &request("tls"), fixture_profile(false)).await,
        Err(ControlHttpError::ConnectionFailed)
    ));
    assert_eq!(
        post_with_profile(operation, &request("restore"), fixture_profile(true))
            .await
            .unwrap()
            .status(),
        200
    );
    // Observe that the real peer received this request before canceling. This
    // avoids treating a pre-connect timeout as a submitted request cancellation.
    let path = std::env::var_os("PINVOU_CONTROL_TLS_FIXTURE").unwrap();
    let fixture: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let seen = std::path::PathBuf::from(fixture["cancellationSeenPath"].as_str().unwrap());
    let pending = tokio::spawn(async move {
        post_with_profile(
            Operation::ConsumeStatus,
            &request("cancel"),
            fixture_profile(true),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !seen.exists() {
            assert!(
                !pending.is_finished(),
                "request completed before peer observation"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    pending.abort();
    assert!(matches!(pending.await, Err(error) if error.is_cancelled()));
}

#[tokio::test]
async fn dedicated_time_request_wire_limit_is_enforced_before_network() {
    let mut raw = wire_cases()
        .iter()
        .find(|c| c["operation"] == "time")
        .unwrap()["request"]
        .as_str()
        .unwrap()
        .as_bytes()
        .to_vec();
    raw.resize(65_537, b' ');
    assert!(matches!(
        post_once(Operation::Time, &raw).await,
        Err(ControlHttpError::RequestJson(Error::InputLimit))
    ));
}

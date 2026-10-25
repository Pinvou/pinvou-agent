use super::*;
use serde_json::{Value, json};

fn vectors() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../../../../specs/upgrade/v1/vectors/time/anchor.json"
    )))
    .unwrap()
}
fn root(version: usize) -> Vec<u8> {
    let vectors = vectors();
    if version == 1 {
        vectors["cases"][0]["trustedRootBytes"]
            .as_str()
            .unwrap()
            .as_bytes()
            .to_vec()
    } else {
        vectors["cases"][0]["rootSuccessors"][0]
            .as_str()
            .unwrap()
            .as_bytes()
            .to_vec()
    }
}
fn canonical(value: &Value) -> Vec<u8> {
    Document::parse(&serde_json::to_vec(value).unwrap(), InputKind::Metadata)
        .unwrap()
        .canonical_bytes()
        .unwrap()
}
#[test]
fn root_selectors_are_closed_safe_and_have_one_decimal_spelling() {
    assert_eq!(
        path("Pinvou.app_x:prod-1", 1).unwrap(),
        "/v1/upgrade/roots/Pinvou.app_x:prod-1/1.json"
    );
    assert!(path(&"a".repeat(128), MAX_VERSION).is_ok());
    for product in [
        "", ".a", "-a", "a/b", "a%2fb", "a?x", "a#x", "a\\b", "a b", "中", "a\n",
    ] {
        assert_eq!(path(product, 1), Err(RootGetError::InvalidSelector));
    }
    assert!(path(&"a".repeat(129), 1).is_err());
    for version in [0, MAX_VERSION + 1, u64::MAX] {
        assert!(path("pinvou", version).is_err());
    }
    assert!(
        path("pinvou", MAX_VERSION)
            .unwrap()
            .ends_with("/9007199254740991.json")
    );
}
#[test]
fn root_data_preserves_expired_timeless_and_historical_cross_signatures_without_trust() {
    for version in [1, 2] {
        let bytes = root(version);
        let found = received("pinvou", version as u64, &bytes).unwrap();
        assert_eq!(found.bytes(), bytes);
        assert_eq!(found.product(), "pinvou");
        assert_eq!(found.version(), version as u64);
    }
    // No crypto/currentness claim: changing a signature to another canonical
    // 64-byte encoding still returns structural DATA, never a trusted Root.
    let mut value: Value = serde_json::from_slice(&root(1)).unwrap();
    value["signatures"][0]["signature"] = json!("A".repeat(86));
    assert!(received("pinvou", 1, &canonical(&value)).is_ok());
}
#[test]
fn root_and_missing_bodies_reject_wrong_context_noncanonical_and_open_shapes() {
    let bytes = root(1);
    assert!(received("other", 1, &bytes).is_err());
    assert!(received("pinvou", 2, &bytes).is_err());
    let mut noncanonical = bytes.clone();
    noncanonical.push(b'\n');
    assert!(received("pinvou", 1, &noncanonical).is_err());
    let mut value: Value = serde_json::from_slice(&bytes).unwrap();
    for (field, changed) in [
        ("role", json!("time")),
        ("component", json!("app")),
        ("extra", json!(0)),
    ] {
        let old = value["signed"]
            .as_object_mut()
            .unwrap()
            .insert(field.to_owned(), changed);
        assert!(received("pinvou", 1, &canonical(&value)).is_err());
        match old {
            Some(old) => {
                value["signed"][field] = old;
            }
            None => {
                value["signed"].as_object_mut().unwrap().remove(field);
            }
        }
    }
    assert!(received("pinvou", 1, &vec![b' '; MAX_RESPONSE_BYTES + 1]).is_err());
    assert!(missing(br#"{"protocolVersion":1,"code":"ROOT_NOT_FOUND"}"#).is_ok());
    for invalid in [
        br#"{"protocolVersion":1,"code":"ROOT_NOT_FOUND","latest":true}"#.as_slice(),
        br#"{"protocolVersion":1,"protocolVersion":1,"code":"ROOT_NOT_FOUND"}"#,
        br#"{"protocolVersion":1,"code":"SERVICE_UNAVAILABLE"}"#,
        br#"{}"#,
    ] {
        assert_eq!(missing(invalid), Err(RootGetError::InvalidMissing));
    }
}
#[tokio::test]
#[ignore = "opt in via tests/https_e2e.py; real isolated TLS peer"]
async fn root_get_real_tls_preserves_bytes_and_rejects_transport_and_context_faults() {
    for version in [1, 2] {
        let result = get_with_profile("pinvou", version, crate::tests::fixture_profile(true))
            .await
            .unwrap();
        let RootGetResult::Found(found) = result else {
            panic!("Root unexpectedly missing")
        };
        assert_eq!(found.bytes(), root(version as usize));
    }
    assert!(matches!(
        get_with_profile("pinvou", 3, crate::tests::fixture_profile(true))
            .await
            .unwrap(),
        RootGetResult::Missing
    ));
    for (product, expected) in [
        ("redirect", RootGetError::UnexpectedStatus),
        ("foreign-redirect", RootGetError::UnexpectedStatus),
        ("status", RootGetError::UnexpectedStatus),
        (
            "gzip",
            RootGetError::Transport(ControlHttpError::InvalidHeaders),
        ),
        (
            "media",
            RootGetError::Transport(ControlHttpError::InvalidHeaders),
        ),
        (
            "oversize",
            RootGetError::Transport(ControlHttpError::ResponseTooLarge),
        ),
        (
            "chunk-oversize",
            RootGetError::Transport(ControlHttpError::ResponseTooLarge),
        ),
        (
            "body-wait",
            RootGetError::Transport(ControlHttpError::RequestStalled),
        ),
        ("noncanonical", RootGetError::InvalidRoot),
        ("wrong-context", RootGetError::InvalidRoot),
        ("bad-missing", RootGetError::InvalidMissing),
    ] {
        assert!(
            matches!(get_with_profile(product, 1, crate::tests::fixture_profile(true)).await, Err(error) if error == expected),
            "{product}"
        );
    }
    assert!(matches!(
        get_with_profile("truncated", 1, crate::tests::fixture_profile(true)).await,
        Err(RootGetError::Transport(ControlHttpError::NetworkFailed))
    ));
    assert!(matches!(
        get_with_profile("connection-drop", 1, crate::tests::fixture_profile(true)).await,
        Err(RootGetError::Transport(ControlHttpError::NetworkFailed))
    ));
    // Active progress longer than the idle limit remains valid: no total timer.
    let result = get_with_profile("progress", 1, crate::tests::fixture_profile(true))
        .await
        .unwrap();
    let RootGetResult::Found(found) = result else {
        panic!("Root unexpectedly missing")
    };
    let mut progress: Value = serde_json::from_slice(&root(1)).unwrap();
    progress["signed"]["product"] = json!("progress");
    assert_eq!(found.bytes(), canonical(&progress));
    assert!(matches!(
        get_with_profile("pinvou", 1, crate::tests::fixture_profile(false)).await,
        Err(RootGetError::Transport(ControlHttpError::ConnectionFailed))
    ));
}

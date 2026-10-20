use super::*;
#[test]
fn invalid_limits_and_original_cancellation_do_not_touch_any_path() {
    let limits = Limits {
        file_bytes: 0,
        expanded_bytes: 1,
        entries: 1,
        metadata_bytes: 1,
        path_state_bytes: 1,
        ratio: 1,
    };
    assert!(limits.archive().is_err());
    assert!(matches!(
        inspect(
            Path::new("unused"),
            "",
            0,
            Path::new("unused"),
            "",
            &limits,
            &AtomicBool::new(true)
        ),
        Err(Error::Cancelled)
    ));
    assert!(serde_json::from_str::<Limits>(r#"{"fileBytes":1,"fileBytes":2,"expandedBytes":1,"entries":1,"metadataBytes":1,"pathStateBytes":1,"ratio":1}"#).is_err());
}

use adapter_gaia::{
    GAIA_ADAPTER_VERSION, GAIA_DATASET_REVISION, GAIA_SCORER_REVISION, GAIA_SCORER_RUNTIME_PROFILE,
    GAIA_SPLIT, GaiaAdapter, question_scorer,
};
use benchmark_core::{
    BenchmarkAdapter, ExecutionKind, PredictionRetention, PrivatePredictionContentType,
};

#[test]
fn gaia_adapter_descriptor_is_available_in_the_default_test_gate() {
    let adapter = GaiaAdapter::new();
    let descriptor = adapter.descriptor();

    assert_eq!(descriptor.id().as_str(), "gaia");
    assert_eq!(descriptor.adapter_version(), GAIA_ADAPTER_VERSION);
    assert_eq!(descriptor.dataset_revision(), GAIA_DATASET_REVISION);
    assert_eq!(descriptor.scorer_revision(), GAIA_SCORER_REVISION);
    assert_eq!(descriptor.supported_splits().len(), 1);
    assert_eq!(descriptor.supported_splits()[0].as_str(), GAIA_SPLIT);
    assert_eq!(descriptor.execution_kind(), ExecutionKind::NativeTurn);
}

#[test]
fn gaia_private_prediction_contract_is_available_in_the_default_test_gate() {
    let adapter = GaiaAdapter::new();

    assert_eq!(
        adapter.private_output_retention(),
        PredictionRetention::DurableUntilPurge
    );
    assert_eq!(
        adapter.private_prediction_content_type(),
        PrivatePredictionContentType::Utf8TextV1
    );
}

#[test]
fn gaia_default_public_api_keeps_custom_digest_verification_feature_gated() {
    let source = include_str!("../src/dataset.rs").replace("\r\n", "\n");
    let gated_signature = concat!(
        "#[cfg(feature = \"test-support\")]\n",
        "    #[doc(hidden)]\n",
        "    pub fn verify_with_expected_parquet("
    );
    assert!(source.contains(gated_signature));
    assert!(!source.contains("pub fn verify_with_expected_parquet_for_tests"));
}

#[test]
fn official_question_scorer_matches_the_locked_numeric_contract() {
    assert!(question_scorer(Some("$1,250%"), "1250"));
    assert!(question_scorer(Some("0"), "0"));
    assert!(question_scorer(Some("-42"), "-42"));
    assert!(!question_scorer(Some("1250.1"), "1250"));
    assert!(!question_scorer(Some("NaN"), "NaN"));
    assert!(question_scorer(Some("not-a-number"), "inf"));
}

#[test]
fn official_question_scorer_matches_string_and_list_normalization() {
    assert!(question_scorer(Some("  Hello, WORLD! "), "hello world"));
    assert!(question_scorer(Some("1; $2000"), "1, 2000"));
    assert!(question_scorer(Some("Alpha ; BETA"), "alpha,beta"));
    assert!(!question_scorer(Some("Alpha!;beta"), "alpha;beta"));
    assert!(!question_scorer(Some("alpha"), "alpha,beta"));
    assert!(question_scorer(None, "None"));
    assert!(!question_scorer(None, "something else"));
}

#[test]
fn official_question_scorer_accepts_python_unicode_decimal_digits() {
    assert!(question_scorer(Some("1"), "١"));
    assert!(question_scorer(Some("12.5"), "１２.５"));
    assert!(question_scorer(Some("100"), "1e٢"));
    assert!(question_scorer(Some("7"), "৭"));
    assert!(question_scorer(Some("7"), "\u{1fbf7}"));
    assert!(!question_scorer(Some("1"), "\u{11f51}"));
    assert!(!question_scorer(Some("7"), "\u{1e4f7}"));
    assert!(!question_scorer(Some("2"), "²"));
}

#[test]
fn official_question_scorer_removes_python_regex_control_whitespace() {
    assert!(question_scorer(Some("alpha\u{001c}beta"), "alphabeta"));
    assert!(question_scorer(
        Some("alpha\u{001f}beta; gamma"),
        "alphabeta;gamma"
    ));
}

#[test]
fn official_question_scorer_uses_python_whole_string_lowercasing() {
    assert!(question_scorer(Some("ΟΣ"), "ος"));
    assert!(question_scorer(Some("ΟΣ;ΟΣ"), "ος;ος"));
    assert!(question_scorer(Some("ΑΣ-Α"), "αςα"));
}

#[test]
fn scorer_runtime_profile_is_pinned_without_changing_the_official_revision() {
    assert_eq!(
        GAIA_SCORER_RUNTIME_PROFILE,
        "hf-spaces-python-3.10-unicode-13.0"
    );
    assert_eq!(
        GAIA_SCORER_REVISION,
        "1349a17979f0aca0ee9c46cd7ec26eb2fb41102e"
    );
}

#[test]
fn docs_gaia_benchmark_covers_required_sections() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let doc_path = std::path::Path::new(manifest_dir).join("../../../docs/gaia-benchmark.md");
    let content = std::fs::read_to_string(&doc_path)
        .unwrap_or_else(|_| panic!("docs/gaia-benchmark.md must exist at {:?}", doc_path));
    for section in [
        "Access and gating",
        "Pinned revisions",
        "Fetch or import",
        "Validation Level 1",
        "Official scorer compatibility",
        "Submission",
        "Privacy",
        "Not a leaderboard score",
        "Known platform limits",
    ] {
        assert!(
            content.contains(section),
            "docs/gaia-benchmark.md must contain section: {}",
            section
        );
    }
}

#[test]
fn docs_gaia_benchmark_records_current_integrity_and_platform_boundaries() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let doc_path = std::path::Path::new(manifest_dir).join("../../../docs/gaia-benchmark.md");
    let content = std::fs::read_to_string(&doc_path)
        .unwrap_or_else(|_| panic!("docs/gaia-benchmark.md must exist at {:?}", doc_path));

    for required in [
        ".pinvou-gaia-integrity-v1",
        "当前用户、SYSTEM 和 Administrators",
        "attachments_platform_security_unsupported",
        "真实 Python scorer 逐题交叉验证尚未完成",
        "旧版 ready marker",
    ] {
        assert!(
            content.contains(required),
            "docs/gaia-benchmark.md must record current boundary: {}",
            required
        );
    }
    for stale_claim in [
        "已在固定 revision 上逐题验证等价",
        "当前附件解析无 Windows 平台 gate",
    ] {
        assert!(
            !content.contains(stale_claim),
            "docs/gaia-benchmark.md must not retain stale claim: {}",
            stale_claim
        );
    }
}

use pinvou_upgrade_transfer::{
    Artifact, BodyPlan, PrefixState, RequestKind, RequestPlan, ResponseDecision, ResponseHeaders,
    RestartReason, SourceContinuity, StrongEtag, TransferError, WriteMode, plan_request,
    plan_response,
};

const HASH: &str =
    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const MAX: u64 = 9_007_199_254_740_991;
fn artifact(size: u64) -> Artifact<'static> {
    Artifact::new("package-1", size, &HASH[..64]).unwrap()
}
fn error<T>(result: Result<T, TransferError>, expected: TransferError) {
    match result {
        Err(actual) => assert_eq!(actual, expected),
        Ok(_) => panic!("unexpected acceptance"),
    }
}
fn headers<'a>(fields: &[(&'a str, &'a [u8])]) -> ResponseHeaders<'a> {
    ResponseHeaders::from_fields(fields.iter().copied()).unwrap()
}
fn full(size: u64) -> RequestPlan<'static> {
    plan_request(artifact(size), None, SourceContinuity::Unproven)
}
fn resume(size: u64, offset: u64) -> RequestPlan<'static> {
    let target = artifact(size);
    let prefix = PrefixState::new(
        Some(target),
        offset,
        Some(StrongEtag::parse(b"\"original\"").unwrap()),
    )
    .unwrap();
    plan_request(target, Some(prefix), SourceContinuity::SameImmutableEntity)
}
fn body<'a>(request: RequestPlan<'a>, status: u16, fields: &[(&'a str, &'a [u8])]) -> BodyPlan<'a> {
    match plan_response(request, status, headers(fields)).unwrap() {
        ResponseDecision::Transfer(plan) => plan,
        _ => panic!("unexpected non-body plan"),
    }
}

#[test]
fn artifact_identity_obeys_the_frozen_positive_safe_size_and_ascii_bindings() {
    for size in [1, MAX] {
        assert_eq!(artifact(size).size(), size);
    }
    for size in [0, MAX + 1, u64::MAX] {
        error(
            Artifact::new("p", size, &HASH[..64]),
            TransferError::InvalidArtifact,
        );
    }
    let longest = "p".repeat(128);
    assert_eq!(
        Artifact::new(&longest, 1, &HASH[..64])
            .unwrap()
            .package_id(),
        longest
    );
    for name in ["", "-p", ":p", "p/1", "p 1", "品悟", &"p".repeat(129)] {
        error(
            Artifact::new(name, 1, &HASH[..64]),
            TransferError::InvalidArtifact,
        );
    }
    for hash in [
        "",
        &HASH[..63],
        &HASH[..65],
        &HASH[..64].to_uppercase(),
        &"g".repeat(64),
    ] {
        error(Artifact::new("p", 1, hash), TransferError::InvalidArtifact);
    }
    assert_eq!(
        artifact(1).sha256()[..8],
        [1, 35, 69, 103, 137, 171, 205, 239]
    );
    error(
        PrefixState::new(None, MAX + 1, None),
        TransferError::InvalidPrefixLength,
    );
    error(
        PrefixState::new(None, u64::MAX, None),
        TransferError::InvalidPrefixLength,
    );
}

#[test]
fn strong_etags_preserve_opaque_bytes_and_only_trim_http_ows() {
    for tag in [b"\"\"".as_slice(), b"\"a\\b\"", b"\"!#~\"", b"\"\x80\xff\""] {
        assert_eq!(StrongEtag::parse(tag).unwrap().as_bytes(), tag);
    }
    assert_eq!(
        StrongEtag::parse(b" \t\"opaque\"\t ").unwrap().as_bytes(),
        b"\"opaque\""
    );
    for tag in [
        b"".as_slice(),
        b"W/\"weak\"",
        b"w/\"weak\"",
        b"plain",
        b"\"a\"b\"",
        b"\"a b\"",
        b"\"a\tb\"",
        b"\"a\nb\"",
        b"\"\x7f\"",
        b"\"\x00\"",
        b"\n\"a\"",
        b"\"a\"\r",
    ] {
        error(StrongEtag::parse(tag), TransferError::InvalidEtag);
    }
    let mut exact = vec![b'a'; 65_536];
    exact[0] = b'"';
    exact[65_535] = b'"';
    assert_eq!(StrongEtag::parse(&exact).unwrap().as_bytes().len(), 65_536);
    exact.push(b' ');
    error(StrongEtag::parse(&exact), TransferError::HeadersAboveLimit);
}

#[test]
fn resume_requires_the_original_target_validator_and_external_continuity() {
    let target = artifact(10);
    let tag = Some(StrongEtag::parse(b"\"original\"").unwrap());
    let original = PrefixState::new(Some(target), 4, tag).unwrap();
    assert!(matches!(
        plan_request(
            target,
            Some(original),
            SourceContinuity::SameImmutableEntity
        )
        .kind(),
        RequestKind::Range { offset: 4, .. }
    ));
    assert!(matches!(
        plan_request(target, Some(original), SourceContinuity::Unproven).kind(),
        RequestKind::Full
    ));
    for binding in [
        None,
        Some(Artifact::new("other", 10, &HASH[..64]).unwrap()),
        Some(artifact(11)),
        Some(Artifact::new("package-1", 10, &"f".repeat(64)).unwrap()),
    ] {
        let prefix = PrefixState::new(binding, 4, tag).unwrap();
        assert!(matches!(
            plan_request(target, Some(prefix), SourceContinuity::SameImmutableEntity).kind(),
            RequestKind::Full
        ));
    }
    let no_validator = PrefixState::new(Some(target), 4, None).unwrap();
    assert!(matches!(
        plan_request(
            target,
            Some(no_validator),
            SourceContinuity::SameImmutableEntity
        )
        .kind(),
        RequestKind::Full
    ));
    for length in [0, 11] {
        let prefix = PrefixState::new(Some(target), length, tag).unwrap();
        assert!(matches!(
            plan_request(target, Some(prefix), SourceContinuity::SameImmutableEntity).kind(),
            RequestKind::Full
        ));
    }
    assert!(matches!(
        resume(10, 9).kind(),
        RequestKind::Range { offset: 9, .. }
    ));
    let unbound_complete = PrefixState::new(None, 10, None).unwrap();
    assert!(matches!(
        plan_request(target, Some(unbound_complete), SourceContinuity::Unproven).kind(),
        RequestKind::VerifyExisting
    ));
}

#[test]
fn full_200_and_ignored_range_replace_from_zero_while_unsolicited_partial_refuses() {
    for request in [full(10), resume(10, 4)] {
        let plan = body(request, 200, &[("Content-Length", b"10")]);
        assert_eq!(plan.write_mode(), WriteMode::ReplaceFromZero);
        assert_eq!(plan.expected_response_length(), 10);
        assert_eq!(plan.target().package_id(), "package-1");
        let mut progress = plan.receive();
        progress.observe_bytes(10).unwrap();
        let receipt = progress.finish().unwrap();
        assert_eq!(receipt.next_offset(), 10);
        assert!(receipt.has_full_byte_count());
    }
    error(
        plan_response(
            full(10),
            206,
            headers(&[("Content-Range", b"bytes 0-9/10")]),
        ),
        TransferError::UnexpectedPartialResponse,
    );
    error(
        plan_response(
            full(10),
            200,
            headers(&[("Content-Range", b"bytes 0-9/10")]),
        ),
        TransferError::UnexpectedPartialResponse,
    );
    for length in [b"9".as_slice(), b"11", b"10,10", b"+10", b"10\n"] {
        error(
            plan_response(full(10), 200, headers(&[("Content-Length", length)])),
            TransferError::InvalidContentLength,
        );
    }
    assert_eq!(body(full(10), 200, &[]).expected_response_length(), 10);
}

#[test]
fn correct_short_and_tail_segments_keep_the_original_artifact_and_distinct_completion() {
    let short = body(
        resume(10, 4),
        206,
        &[
            ("ETag", b"\"original\""),
            ("Content-Range", b"bytes 4-6/10"),
            ("Content-Length", b"3"),
        ],
    );
    assert_eq!(short.write_mode(), WriteMode::Append { offset: 4 });
    let mut progress = short.receive();
    progress.observe_bytes(1).unwrap();
    progress.observe_bytes(2).unwrap();
    let receipt = progress.finish().unwrap();
    assert_eq!(receipt.next_offset(), 7);
    assert!(!receipt.has_full_byte_count());
    assert_eq!(receipt.target().sha256(), artifact(10).sha256());
    let tail = body(
        resume(10, 7),
        206,
        &[
            ("ETag", b"\"original\""),
            ("Content-Range", b"bytes 7-9/10"),
        ],
    );
    let mut progress = tail.receive();
    progress.observe_bytes(3).unwrap();
    let receipt = progress.finish().unwrap();
    assert_eq!(receipt.next_offset(), 10);
    assert!(receipt.has_full_byte_count());
    let ows = body(
        resume(10, 4),
        206,
        &[
            ("eTAG", b" \t\"original\"\t"),
            ("Content-Range", b"\tBYTES 0004-0009/0010 "),
            ("Content-Length", b" 0006\t"),
        ],
    );
    assert_eq!(ows.expected_response_length(), 6);
}

#[test]
fn range_boundaries_unknown_lengths_malformed_units_and_wrong_validators_refuse() {
    for range in [
        b"bytes 3-9/10".as_slice(),
        b"bytes 5-9/10",
        b"bytes 4-8/9",
        b"bytes 4-9/11",
    ] {
        error(
            plan_response(
                resume(10, 4),
                206,
                headers(&[("ETag", b"\"original\""), ("Content-Range", range)]),
            ),
            TransferError::RangeMismatch,
        );
    }
    for range in [
        b"bytes 4-3/10".as_slice(),
        b"bytes 4-10/10",
        b"bytes 4-9/*",
        b"bytes */10",
        b"items 4-9/10",
        b"bytes  4-9/10",
        b"bytes +4-9/10",
        b"bytes 4 -9/10",
        b"bytes 4-9/10/10",
        b"bytes 4-9/10\n",
        b"bytes 4-9/9007199254740992",
        b"bytes 4-9/18446744073709551616",
        b"bytes 4-9/10, 11-12/13",
    ] {
        error(
            plan_response(
                resume(10, 4),
                206,
                headers(&[("ETag", b"\"original\""), ("Content-Range", range)]),
            ),
            TransferError::InvalidContentRange,
        );
    }
    error(
        plan_response(resume(10, 4), 206, headers(&[("ETag", b"\"original\"")])),
        TransferError::InvalidContentRange,
    );
    for tag in [
        None,
        Some(b"\"Original\"".as_slice()),
        Some(b"\"changed\"".as_slice()),
    ] {
        let mut fields = vec![("Content-Range", b"bytes 4-9/10".as_slice())];
        if let Some(tag) = tag {
            fields.push(("ETag", tag));
        }
        error(
            plan_response(resume(10, 4), 206, headers(&fields)),
            TransferError::ValidatorMismatch,
        );
    }
    for length in [b"5".as_slice(), b"7", b"9007199254740992"] {
        error(
            plan_response(
                resume(10, 4),
                206,
                headers(&[
                    ("ETag", b"\"original\""),
                    ("Content-Range", b"bytes 4-9/10"),
                    ("Content-Length", length),
                ]),
            ),
            TransferError::InvalidContentLength,
        );
    }
}

#[test]
fn duplicate_critical_headers_and_nonidentity_content_encoding_refuse() {
    for (lower, mixed) in [
        ("etag", "ETag"),
        ("content-range", "Content-Range"),
        ("content-length", "Content-Length"),
        ("content-encoding", "Content-Encoding"),
    ] {
        for second in [b"same".as_slice(), b"different"] {
            error(
                ResponseHeaders::from_fields([(lower, b"same".as_slice()), (mixed, second)]),
                TransferError::DuplicateHeader,
            );
        }
    }
    assert_eq!(
        body(full(10), 200, &[("Content-Encoding", b"\tIdEnTiTy ")]).expected_response_length(),
        10
    );
    for encoding in [
        b"gzip".as_slice(),
        b"br",
        b"",
        b"identity,gzip",
        b"identity\n",
    ] {
        error(
            plan_response(full(10), 200, headers(&[("Content-Encoding", encoding)])),
            TransferError::UnsupportedEncoding,
        );
    }
    // MIME, filenames and unrelated response fields do not replace the target.
    let plan = body(
        full(10),
        200,
        &[
            ("Content-Type", b"application/octet-stream"),
            ("Content-Disposition", b"filename=other.exe"),
        ],
    );
    assert_eq!(plan.target().size(), 10);
    assert_eq!(plan.target().sha256(), artifact(10).sha256());
}

#[test]
fn header_profile_bounds_count_each_raw_occurrence_even_when_ignored() {
    let exact_value = vec![b'a'; 65_536];
    ResponseHeaders::from_fields([("x", exact_value.as_slice())]).unwrap();
    let excessive_value = vec![b'a'; 65_537];
    error(
        ResponseHeaders::from_fields([("x", excessive_value.as_slice())]),
        TransferError::HeadersAboveLimit,
    );
    ResponseHeaders::from_fields(std::iter::repeat_n(("x", b"".as_slice()), 4096)).unwrap();
    error(
        ResponseHeaders::from_fields(std::iter::repeat_n(("x", b"".as_slice()), 4097)),
        TransferError::HeadersAboveLimit,
    );
    let value = vec![b'a'; 65_535];
    ResponseHeaders::from_fields(std::iter::repeat_n(("x", value.as_slice()), 16)).unwrap();
    error(
        ResponseHeaders::from_fields(std::iter::repeat_n(("x", value.as_slice()), 17)),
        TransferError::HeadersAboveLimit,
    );
}

#[test]
fn precondition_and_unsatisfiable_range_recover_without_claiming_verified_completion() {
    assert!(matches!(
        plan_response(resume(10, 4), 412, headers(&[])).unwrap(),
        ResponseDecision::Restart(RestartReason::PreconditionFailed)
    ));
    assert!(matches!(
        plan_response(resume(10, 4), 416, headers(&[])).unwrap(),
        ResponseDecision::Restart(RestartReason::RangeUnsatisfiable)
    ));
    assert!(matches!(
        plan_response(resume(10, 10), 416, headers(&[])).unwrap(),
        ResponseDecision::VerifyExisting(_)
    ));
    error(
        plan_response(resume(10, 10), 200, headers(&[])),
        TransferError::LocalVerificationRequired,
    );
    error(
        plan_response(resume(10, 10), 412, headers(&[])),
        TransferError::LocalVerificationRequired,
    );
    for status in [301, 304, 401, 403, 404, 408, 429, 500] {
        error(
            plan_response(full(10), status, headers(&[])),
            TransferError::HttpStatusRefused(status),
        );
    }
    assert_eq!(
        TransferError::HttpStatusRefused(403).to_string(),
        "FILE_HTTP_STATUS_REFUSED"
    );
}

#[test]
fn byte_counter_requires_exact_eof_and_latches_every_failure_or_completion() {
    let plan = body(full(10), 200, &[]);
    let mut progress = plan.receive();
    progress.observe_bytes(0).unwrap();
    progress.observe_bytes(4).unwrap();
    progress.observe_bytes(6).unwrap();
    assert!(progress.finish().unwrap().has_full_byte_count());
    error(progress.observe_bytes(0), TransferError::BodyClosed);
    error(progress.finish(), TransferError::BodyClosed);
    error(progress.abort(), TransferError::BodyClosed);
    for chunks in [vec![11], vec![10, 1], vec![u64::MAX], vec![1, u64::MAX]] {
        let mut progress = plan.receive();
        for chunk in chunks {
            if progress.observe_bytes(chunk).is_err() {
                break;
            }
        }
        error(progress.observe_bytes(0), TransferError::BodyTooLong);
        error(progress.finish(), TransferError::BodyTooLong);
    }
    let mut truncated = plan.receive();
    truncated.observe_bytes(9).unwrap();
    error(truncated.finish(), TransferError::BodyTruncated);
    error(truncated.observe_bytes(1), TransferError::BodyTruncated);
    error(truncated.finish(), TransferError::BodyTruncated);
    let mut aborted = plan.receive();
    aborted.observe_bytes(10).unwrap();
    error(aborted.abort(), TransferError::BodyAborted);
    error(aborted.observe_bytes(0), TransferError::BodyAborted);
    error(aborted.finish(), TransferError::BodyAborted);
}

#[test]
fn safe_maximum_ranges_and_counts_do_not_overflow_or_establish_file_trust() {
    let fields = [
        ("ETag", b"\"original\"".as_slice()),
        (
            "Content-Range",
            b"bytes 9007199254740990-9007199254740990/9007199254740991",
        ),
    ];
    let tail = body(resume(MAX, MAX - 1), 206, &fields);
    assert_eq!(tail.expected_response_length(), 1);
    let mut progress = tail.receive();
    progress.observe_bytes(1).unwrap();
    let receipt = progress.finish().unwrap();
    assert_eq!(receipt.next_offset(), MAX);
    assert!(receipt.has_full_byte_count());
    let mut whole = body(full(MAX), 200, &[]).receive();
    whole.observe_bytes(MAX - 1).unwrap();
    whole.observe_bytes(1).unwrap();
    assert_eq!(whole.finish().unwrap().next_offset(), MAX);
    let mut too_long = body(full(MAX), 200, &[]).receive();
    too_long.observe_bytes(MAX).unwrap();
    error(too_long.observe_bytes(1), TransferError::BodyTooLong);
    error(too_long.finish(), TransferError::BodyTooLong);
}

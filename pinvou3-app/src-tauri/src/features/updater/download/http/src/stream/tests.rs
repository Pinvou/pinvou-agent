use super::*;
use crate::destination::{FileDestination, FileOrigin};
use pinvou_upgrade_transfer::{
    PrefixState, SourceContinuity, StrongEtag, TransferError, WriteMode, plan_request,
};
use std::sync::atomic::{AtomicUsize, Ordering};

const HASH: &str = "84d89877f0d4041efb6bf91a16f0248f2fd573e6af05c19f96bedb9f882f7882";
fn full(size: u64) -> RequestPlan<'static> {
    plan_request(
        Artifact::new("package-1", size, HASH).unwrap(),
        None,
        SourceContinuity::Unproven,
    )
}
fn range(offset: u64) -> RequestPlan<'static> {
    let target = Artifact::new("package-1", 10, HASH).unwrap();
    let prefix = PrefixState::new(
        Some(target),
        offset,
        Some(StrongEtag::parse(b"\"original\"").unwrap()),
    )
    .unwrap();
    plan_request(target, Some(prefix), SourceContinuity::SameImmutableEntity)
}
fn error<T>(result: Result<T, FileHttpError>, expected: FileHttpError) {
    match result {
        Err(actual) => assert_eq!(actual, expected),
        Ok(_) => panic!("unexpected acceptance"),
    }
}
struct Policy {
    host: String,
    port: u16,
    allowed: Vec<SocketAddr>,
    allow_origin: bool,
}
impl DestinationPolicy for Policy {
    fn permits_origin(&self, origin: &FileOrigin) -> bool {
        self.allow_origin && origin.host() == self.host && origin.port() == self.port
    }
    fn permits_address(&self, _: &FileOrigin, address: SocketAddr) -> bool {
        self.allowed.contains(&address)
    }
}
struct Resolver {
    addresses: Vec<SocketAddr>,
    calls: AtomicUsize,
    delay: Duration,
}
impl AddressResolver for Resolver {
    fn resolve<'a>(
        &'a self,
        _: &'a FileOrigin,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<SocketAddr>, ResolveError>> + Send + 'a>,
    > {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            tokio::time::sleep(self.delay).await;
            Ok(self.addresses.clone())
        })
    }
}
fn resolver(addresses: Vec<SocketAddr>) -> Resolver {
    Resolver {
        addresses,
        calls: AtomicUsize::new(0),
        delay: Duration::ZERO,
    }
}

#[test]
fn uri_shape_rejects_credential_or_parser_normalization_bypasses() {
    for raw in [
        "http://file.test.invalid/a",
        "https://@file.test.invalid/a",
        "https://a:b@file.test.invalid/a",
        "https://file.test.invalid/#fragment",
        "https://file.test.invalid/\npath",
        "https://file.test.invalid/ path",
        "https://file.test.invalid:0/a",
    ] {
        error(FileDestination::parse(raw), FileHttpError::InvalidUrl);
    }
    let idna = FileDestination::parse("https://BÜCHER.example:444/a").unwrap();
    assert_eq!(idna.origin.host(), "xn--bcher-kva.example");
    assert_eq!(idna.origin.port(), 444);
    let ipv6 = FileDestination::parse("https://[::1]:444/a").unwrap();
    assert_eq!(ipv6.origin.host(), "::1");
    assert_eq!(ipv6.literal, Some("::1".parse().unwrap()));
    assert_eq!(ipv6.origin.port(), 444);
    let decimal_ip = FileDestination::parse("https://2130706433/a").unwrap();
    assert_eq!(decimal_ip.origin.host(), "127.0.0.1");
    assert_eq!(decimal_ip.literal, Some("127.0.0.1".parse().unwrap()));
}

#[tokio::test]
async fn denied_origins_and_invalid_resolutions_do_not_start_a_connection() {
    let address: SocketAddr = "127.0.0.1:443".parse().unwrap();
    let mut policy = Policy {
        host: "file.test.invalid".into(),
        port: 443,
        allowed: vec![address],
        allow_origin: false,
    };
    let dns = resolver(vec![address]);
    error(
        fetch("https://file.test.invalid/file", full(10), &policy, &dns).await,
        FileHttpError::OriginDenied,
    );
    assert_eq!(dns.calls.load(Ordering::SeqCst), 0);
    policy.allow_origin = true;
    for addresses in [
        vec![],
        vec!["127.0.0.1:444".parse().unwrap()],
        vec![address; 4097],
        vec!["0.0.0.0:443".parse().unwrap()],
    ] {
        error(
            fetch(
                "https://file.test.invalid/file",
                full(10),
                &policy,
                &resolver(addresses),
            )
            .await,
            FileHttpError::InvalidResolution,
        );
    }
    error(
        fetch(
            "https://file.test.invalid/file",
            full(10),
            &policy,
            &resolver(vec![address, "127.0.0.2:443".parse().unwrap()]),
        )
        .await,
        FileHttpError::AddressDenied,
    );
    let literal_policy = Policy {
        host: "127.0.0.2".into(),
        port: 443,
        allowed: vec![address],
        allow_origin: true,
    };
    error(
        fetch("https://127.0.0.2/file", full(10), &literal_policy, &dns).await,
        FileHttpError::AddressDenied,
    );
    assert_eq!(dns.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn dns_wait_has_a_single_operation_timeout_and_unknown_fallback_always_refuses() {
    let address = "127.0.0.1:443".parse().unwrap();
    let policy = Policy {
        host: "file.test.invalid".into(),
        port: 443,
        allowed: vec![address],
        allow_origin: true,
    };
    let dns = Resolver {
        addresses: vec![address],
        calls: AtomicUsize::new(0),
        delay: Duration::from_secs(1),
    };
    let limits = StallLimits {
        connect: Duration::from_millis(20),
        read_idle: Duration::from_millis(100),
        fixture_root: None,
    };
    error(
        fetch_with_profile(
            "https://file.test.invalid/file",
            full(10),
            &policy,
            &dns,
            limits,
        )
        .await,
        FileHttpError::DnsTimeout,
    );
    let deny = DenyFallbackResolver;
    use reqwest::dns::Resolve;
    assert!(
        deny.resolve("unapproved.test.invalid".parse().unwrap())
            .await
            .is_err()
    );
}

struct Fixture {
    origin: String,
    address: SocketAddr,
    certificate: reqwest::Certificate,
    policy: Policy,
}
impl Fixture {
    fn load() -> Self {
        let path =
            std::env::var_os("PINVOU_UPGRADE_TLS_FIXTURE").expect("run the isolated HTTPS harness");
        let bytes = std::fs::read(path).unwrap();
        assert!(bytes.len() <= 65_536);
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let origin = value["origin"].as_str().unwrap().to_string();
        let address: SocketAddr = value["address"].as_str().unwrap().parse().unwrap();
        let certificate =
            reqwest::Certificate::from_pem(value["certificatePem"].as_str().unwrap().as_bytes())
                .unwrap();
        assert_eq!(value["sha256"].as_str().unwrap(), HASH);
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        Self {
            origin,
            address,
            certificate,
            policy: Policy {
                host: "file.test.invalid".into(),
                port: address.port(),
                allowed: vec![address],
                allow_origin: true,
            },
        }
    }
    fn limits(&self, idle: Duration) -> StallLimits {
        StallLimits {
            connect: Duration::from_secs(2),
            read_idle: idle,
            fixture_root: Some(self.certificate.clone()),
        }
    }
    async fn get(
        &self,
        path: &str,
        request: RequestPlan<'static>,
    ) -> Result<FetchResult<'static>, FileHttpError> {
        fetch_with_profile(
            &format!("{}{path}", self.origin),
            request,
            &self.policy,
            &resolver(vec![self.address]),
            self.limits(Duration::from_secs(2)),
        )
        .await
    }
}
async fn collect(
    result: FetchResult<'static>,
) -> Result<(Vec<u8>, SegmentReceipt<'static>), FileHttpError> {
    let FetchResult::Transfer(mut body) = result else {
        panic!("expected a body")
    };
    let mut bytes = Vec::new();
    loop {
        match body.next_chunk().await? {
            FileChunk::Data(chunk) => {
                assert!(!chunk.is_empty() && chunk.len() <= 65_536);
                bytes.extend_from_slice(&chunk);
            }
            FileChunk::SegmentFinished(receipt) => {
                error(body.next_chunk().await, FileHttpError::BodyClosed);
                return Ok((bytes, receipt));
            }
        }
    }
}

#[tokio::test]
#[ignore = "requires isolated HTTPS fixture"]
async fn real_tls_bound_range_replacement_short_segment_and_cookie_isolation() {
    let fixture = Fixture::load();
    for path in ["/file", "/cookie", "/ignored-range"] {
        let result = fixture.get(path, range(4)).await.unwrap();
        let FetchResult::Transfer(body) = &result else {
            panic!("expected body")
        };
        assert_eq!(body.plan().write_mode(), WriteMode::ReplaceFromZero);
        assert_eq!(body.plan().expected_response_length(), 10);
        assert_eq!(body.plan().target().package_id(), "package-1");
        assert_eq!(
            body.strong_validator_bytes(),
            Some(b"\"original\"".as_slice())
        );
        let (bytes, receipt) = collect(result).await.unwrap();
        assert_eq!(bytes, b"0123456789");
        assert_eq!(receipt.next_offset(), 10);
    }
    let (bytes, receipt) = collect(fixture.get("/partial", range(4)).await.unwrap())
        .await
        .unwrap();
    assert_eq!(bytes, b"456789");
    assert_eq!(receipt.next_offset(), 10);
    let result = fixture.get("/short-segment", range(4)).await.unwrap();
    let FetchResult::Transfer(body) = &result else {
        panic!("expected body")
    };
    assert_eq!(body.plan().write_mode(), WriteMode::Append { offset: 4 });
    assert_eq!(body.plan().expected_response_length(), 3);
    let (bytes, receipt) = collect(result).await.unwrap();
    assert_eq!(bytes, b"456");
    assert_eq!(receipt.next_offset(), 7);
    assert!(!receipt.has_full_byte_count());
    let (bytes, _) = collect(fixture.get("/partial", range(7)).await.unwrap())
        .await
        .unwrap();
    assert_eq!(bytes, b"789");
}

#[tokio::test]
#[ignore = "requires isolated HTTPS fixture"]
async fn real_tls_name_trust_and_wrong_pins_fail_without_unapproved_fallback() {
    let fixture = Fixture::load();
    let dns = resolver(vec![fixture.address]);
    error(
        fetch(
            &format!("{}/file?opaque=value", fixture.origin),
            full(10),
            &fixture.policy,
            &dns,
        )
        .await,
        FileHttpError::ConnectionFailed,
    );
    let wrong_origin = format!("https://wrong.test.invalid:{}/file", fixture.address.port());
    let wrong_policy = Policy {
        host: "wrong.test.invalid".into(),
        port: fixture.address.port(),
        allowed: vec![fixture.address],
        allow_origin: true,
    };
    error(
        fetch_with_profile(
            &wrong_origin,
            full(10),
            &wrong_policy,
            &dns,
            fixture.limits(Duration::from_secs(1)),
        )
        .await,
        FileHttpError::ConnectionFailed,
    );
    let bad_pin = SocketAddr::new("127.0.0.2".parse().unwrap(), fixture.address.port());
    let bad_policy = Policy {
        host: "file.test.invalid".into(),
        port: fixture.address.port(),
        allowed: vec![bad_pin],
        allow_origin: true,
    };
    let result = fetch_with_profile(
        &format!("{}/unapproved-pin", fixture.origin),
        full(10),
        &bad_policy,
        &resolver(vec![bad_pin]),
        fixture.limits(Duration::from_secs(1)),
    )
    .await;
    assert!(matches!(
        result,
        Err(FileHttpError::ConnectionFailed | FileHttpError::RequestTimeout)
    ));
    let denied = Policy {
        host: fixture.policy.host.clone(),
        port: fixture.address.port(),
        allowed: vec![fixture.address],
        allow_origin: false,
    };
    let dns = resolver(vec![fixture.address]);
    error(
        fetch_with_profile(
            &format!("{}/denied", fixture.origin),
            full(10),
            &denied,
            &dns,
            fixture.limits(Duration::from_secs(1)),
        )
        .await,
        FileHttpError::OriginDenied,
    );
    assert_eq!(dns.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
#[ignore = "requires isolated HTTPS fixture"]
async fn real_http_redirects_encodings_duplicate_headers_and_recovery_are_closed() {
    let fixture = Fixture::load();
    for path in [
        "/duplicates/length-equal",
        "/duplicates/range",
        "/duplicates/encoding",
    ] {
        error(
            fixture.get(path, full(10)).await,
            FileHttpError::Transfer(TransferError::DuplicateHeader),
        );
    }
    // The SDK rejects conflicting lengths before exposing the parsed response.
    error(
        fixture.get("/duplicates/length-conflict", full(10)).await,
        FileHttpError::NetworkFailed,
    );
    let (bytes, _) = collect(fixture.get("/framing/chunked", full(10)).await.unwrap())
        .await
        .unwrap();
    assert_eq!(bytes, b"0123456789");
    for path in ["/framing/conflict", "/framing/coding", "/framing/duplicate"] {
        error(
            fixture.get(path, full(10)).await,
            FileHttpError::InvalidFraming,
        );
    }
    for status in [301, 302, 307, 308] {
        error(
            fixture.get(&format!("/redirect/{status}"), full(10)).await,
            FileHttpError::Transfer(TransferError::HttpStatusRefused(status)),
        );
    }
    for path in ["/gzip", "/br"] {
        error(
            fixture.get(path, full(10)).await,
            FileHttpError::Transfer(TransferError::UnsupportedEncoding),
        );
    }
    error(
        fixture.get("/duplicate-etag", full(10)).await,
        FileHttpError::Transfer(TransferError::DuplicateHeader),
    );
    for path in [
        "/wrong-etag",
        "/wrong-total",
        "/wrong-offset",
        "/wrong-length",
    ] {
        assert!(fixture.get(path, range(4)).await.is_err());
    }
    assert!(matches!(
        fixture.get("/precondition", range(4)).await.unwrap(),
        FetchResult::Restart(RestartReason::PreconditionFailed)
    ));
    assert!(matches!(
        fixture.get("/unsatisfiable", range(4)).await.unwrap(),
        FetchResult::Restart(RestartReason::RangeUnsatisfiable)
    ));
}

#[tokio::test]
#[ignore = "requires isolated HTTPS fixture"]
async fn real_body_excess_truncation_network_error_and_consumer_abort_never_finish() {
    let fixture = Fixture::load();
    for (path, expected) in [
        ("/overlong", Some(TransferError::BodyTooLong)),
        ("/truncated", Some(TransferError::BodyTruncated)),
        ("/network-error", None),
    ] {
        let result = collect(fixture.get(path, full(10)).await.unwrap()).await;
        match expected {
            Some(expected) => error(result, FileHttpError::Transfer(expected)),
            None => assert!(result.is_err()),
        }
    }
    let FetchResult::Transfer(mut body) = fixture.get("/file", full(10)).await.unwrap() else {
        panic!("expected body")
    };
    error(body.abort_by_consumer(), FileHttpError::ConsumerAborted);
    error(body.next_chunk().await, FileHttpError::ConsumerAborted);
}

#[tokio::test]
#[ignore = "requires isolated HTTPS fixture"]
async fn real_continuous_progress_exceeds_idle_duration_while_a_gap_stalls() {
    let fixture = Fixture::load();
    let dns = resolver(vec![fixture.address]);
    let start = std::time::Instant::now();
    let result = fetch_with_profile(
        &format!("{}/slow-progress", fixture.origin),
        full(10),
        &fixture.policy,
        &dns,
        fixture.limits(Duration::from_millis(150)),
    )
    .await
    .unwrap();
    let (bytes, _) = collect(result).await.unwrap();
    assert_eq!(bytes, b"0123456789");
    assert!(start.elapsed() > Duration::from_millis(300));
    let result = fetch_with_profile(
        &format!("{}/stall", fixture.origin),
        full(10),
        &fixture.policy,
        &dns,
        fixture.limits(Duration::from_millis(100)),
    )
    .await
    .unwrap();
    error(collect(result).await, FileHttpError::RequestTimeout);
}

#[tokio::test]
#[ignore = "requires isolated HTTPS fixture"]
async fn real_output_chunks_are_bounded_without_collecting_the_entire_response() {
    let fixture = Fixture::load();
    let (bytes, receipt) = collect(fixture.get("/large-chunk", full(131_073)).await.unwrap())
        .await
        .unwrap();
    assert_eq!(bytes.len(), 131_073);
    assert!(bytes.iter().all(|byte| *byte == b'x'));
    assert!(receipt.has_full_byte_count());
}

mod cache_tests;

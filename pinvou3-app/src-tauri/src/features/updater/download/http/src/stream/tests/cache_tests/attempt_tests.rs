use super::super::{Resolver, resolver};
use super::{CacheFile, Fixture, HASH, full, range};
use crate::attempt::fetch_with_cache_profile;
use crate::{CacheFetchOutcome, CacheFetchRequest, CacheReceiveError, ReplacementPermission};
use pinvou_upgrade_byte_check::{ByteCheckError, SegmentWriteError, check_file};
use pinvou_upgrade_transfer::{Artifact, PrefixState, SourceContinuity, plan_request};
use std::io::{Seek, SeekFrom};
use std::sync::atomic::Ordering;
use std::time::Duration;

async fn attempt(
    path: &'static str,
    plan: pinvou_upgrade_transfer::RequestPlan<'static>,
    mut owned: CacheFile,
    replacement: ReplacementPermission,
) -> (
    CacheFile,
    Result<CacheFetchOutcome<'static>, CacheReceiveError>,
) {
    let fixture = Fixture::load();
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let url = format!("{}{path}", fixture.origin);
        let dns = resolver(vec![fixture.address]);
        let request = CacheFetchRequest::new(&url, plan, &fixture.policy, &dns, replacement);
        let result = fetch_with_cache_profile(
            &runtime,
            request,
            owned.file(),
            &mut || false,
            fixture.limits(Duration::from_secs(2)),
        );
        (owned, result)
    })
    .await
    .unwrap()
}
#[tokio::test]
#[ignore = "requires isolated HTTPS fixture"]
async fn real_single_attempt_replacement_short_range_and_terminal_headers_preserve_binding() {
    let (mut owned, result) = attempt(
        "/ignored-range",
        range(4),
        CacheFile::new(b"old cache"),
        ReplacementPermission::Refuse,
    )
    .await;
    assert!(matches!(
        result,
        Err(CacheReceiveError::Write(
            SegmentWriteError::WritePermissionMismatch
        ))
    ));
    assert_eq!(owned.bytes(), b"old cache");
    let (mut owned, result) = attempt(
        "/ignored-range",
        range(4),
        owned,
        ReplacementPermission::Allow,
    )
    .await;
    let CacheFetchOutcome::Segment(receipt) = result.unwrap() else {
        panic!("expected actual segment");
    };
    assert!(check_file(owned.file(), receipt.target(), || false).is_ok());
    let (mut owned, result) = attempt(
        "/short-segment",
        range(4),
        CacheFile::new(b"0123"),
        ReplacementPermission::Refuse,
    )
    .await;
    let CacheFetchOutcome::Segment(receipt) = result.unwrap() else {
        panic!("expected actual segment");
    };
    assert_eq!(receipt.next_offset(), 7);
    assert_eq!(owned.bytes(), b"0123456");
    for path in ["/precondition", "/unsatisfiable"] {
        let mut owned = CacheFile::new(b"keep");
        owned.file().seek(SeekFrom::Start(7)).unwrap();
        let (mut owned, result) =
            attempt(path, range(4), owned, ReplacementPermission::Allow).await;
        assert!(matches!(result, Ok(CacheFetchOutcome::Restart(_))));
        assert_eq!(owned.file().stream_position().unwrap(), 7);
        assert_eq!(owned.bytes(), b"keep");
    }
    let (mut owned, result) = attempt(
        "/wrong-etag",
        range(4),
        CacheFile::new(b"keep"),
        ReplacementPermission::Allow,
    )
    .await;
    assert!(matches!(
        result,
        Err(CacheReceiveError::Http(crate::FileHttpError::Transfer(
            pinvou_upgrade_transfer::TransferError::ValidatorMismatch
        )))
    ));
    assert_eq!(owned.bytes(), b"keep");
}
#[tokio::test]
async fn complete_cache_without_association_requests_local_verification_without_network_or_mutation()
 {
    let fixture_target = Artifact::new("package-1", 10, HASH).unwrap();
    let prefix = PrefixState::new(None, 10, None).unwrap();
    let plan = plan_request(fixture_target, Some(prefix), SourceContinuity::Unproven);
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let mut owned = CacheFile::new(b"0123456789");
        owned.file().seek(SeekFrom::Start(7)).unwrap();
        let dns = resolver(vec![]);
        struct Deny;
        impl crate::DestinationPolicy for Deny {
            fn permits_origin(&self, _: &crate::FileOrigin) -> bool {
                false
            }
            fn permits_address(&self, _: &crate::FileOrigin, _: std::net::SocketAddr) -> bool {
                false
            }
        }
        let request = CacheFetchRequest::new(
            "invalid-unused-url",
            plan,
            &Deny,
            &dns,
            ReplacementPermission::Refuse,
        );
        let result =
            crate::fetch_cache_once_on_worker(&runtime, request, owned.file(), || false).unwrap();
        let CacheFetchOutcome::VerifyExisting(target) = result else {
            panic!("expected local verification request");
        };
        assert!(target == fixture_target);
        assert_eq!(dns.calls.load(Ordering::SeqCst), 0);
        assert_eq!(owned.file().stream_position().unwrap(), 7);
        assert_eq!(owned.bytes(), b"0123456789");
        assert!(check_file(owned.file(), target, || false).is_ok());
        let mut polls = 0;
        let request = CacheFetchRequest::new(
            "invalid-unused-url",
            plan,
            &Deny,
            &dns,
            ReplacementPermission::Refuse,
        );
        let result = crate::fetch_cache_once_on_worker(&runtime, request, owned.file(), || {
            polls += 1;
            polls == 2
        });
        assert!(matches!(
            result,
            Err(CacheReceiveError::Write(SegmentWriteError::File(
                ByteCheckError::Cancelled
            )))
        ));
        assert_eq!(dns.calls.load(Ordering::SeqCst), 0);
    })
    .await
    .unwrap();
}
#[tokio::test]
#[ignore = "requires isolated HTTPS fixture"]
async fn real_dns_and_header_wait_cancellation_happens_before_any_cache_mutation() {
    let fixture = Fixture::load();
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let url = format!("{}/file", fixture.origin);
        let dns = Resolver {
            addresses: vec![fixture.address],
            calls: std::sync::atomic::AtomicUsize::new(0),
            delay: Duration::from_secs(5),
        };
        let mut owned = CacheFile::new(b"keep");
        owned.file().seek(SeekFrom::Start(7)).unwrap();
        let mut began = None;
        let mut cancellation = || {
            if dns.calls.load(Ordering::SeqCst) == 0 {
                return false;
            }
            began.get_or_insert_with(std::time::Instant::now).elapsed()
                >= Duration::from_millis(120)
        };
        let request = CacheFetchRequest::new(
            &url,
            full(10),
            &fixture.policy,
            &dns,
            ReplacementPermission::Allow,
        );
        let result = fetch_with_cache_profile(
            &runtime,
            request,
            owned.file(),
            &mut cancellation,
            fixture.limits(Duration::from_secs(2)),
        );
        assert!(matches!(
            result,
            Err(CacheReceiveError::Write(SegmentWriteError::File(
                ByteCheckError::Cancelled
            )))
        ));
        assert_eq!(dns.calls.load(Ordering::SeqCst), 1);
        assert_eq!(owned.file().stream_position().unwrap(), 7);
        assert_eq!(owned.bytes(), b"keep");
        // A completed resolver remains a legal one-attempt positive path.
        let dns = resolver(vec![fixture.address]);
        let request = CacheFetchRequest::new(
            &url,
            full(10),
            &fixture.policy,
            &dns,
            ReplacementPermission::Allow,
        );
        let result = fetch_with_cache_profile(
            &runtime,
            request,
            owned.file(),
            &mut || false,
            fixture.limits(Duration::from_secs(2)),
        )
        .unwrap();
        let CacheFetchOutcome::Segment(receipt) = result else {
            panic!("expected actual segment");
        };
        assert!(check_file(owned.file(), receipt.target(), || false).is_ok());
        assert_eq!(dns.calls.load(Ordering::SeqCst), 1);
    })
    .await
    .unwrap();

    let fixture = Fixture::load();
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let url = format!("{}/cache-header-wait", fixture.origin);
        let dns = resolver(vec![fixture.address]);
        let mut owned = CacheFile::new(b"keep");
        owned.file().seek(SeekFrom::Start(7)).unwrap();
        let mut first_poll = None;
        let request = CacheFetchRequest::new(
            &url,
            full(10),
            &fixture.policy,
            &dns,
            ReplacementPermission::Allow,
        );
        let result = fetch_with_cache_profile(
            &runtime,
            request,
            owned.file(),
            &mut || {
                first_poll
                    .get_or_insert_with(std::time::Instant::now)
                    .elapsed()
                    >= Duration::from_millis(120)
            },
            fixture.limits(Duration::from_secs(2)),
        );
        assert!(matches!(
            result,
            Err(CacheReceiveError::Write(SegmentWriteError::File(
                ByteCheckError::Cancelled
            )))
        ));
        assert_eq!(owned.file().stream_position().unwrap(), 7);
        assert_eq!(owned.bytes(), b"keep");
        // The independent wire verifier requires this exact request to reach
        // the peer. It sends no headers or bytes, so this cancellation precedes
        // response planning/writer start rather than occurring after a body read.
    })
    .await
    .unwrap();
}

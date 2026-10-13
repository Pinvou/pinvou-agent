use super::super::{Resolver, resolver};
use super::{CacheFile, Fixture, full, range};
use crate::attempt::fetch_with_cache_observer_profile;
use crate::{
    CacheFetchOutcome, CacheFetchRequest, CacheNotice, CacheObserver, CacheReceiveError,
    ObserverControl, ReplacementPermission, WaitPhase,
};
use pinvou_upgrade_byte_check::{
    ByteCheckError, CacheWritePermission, SegmentWriteError, check_file,
};
use pinvou_upgrade_transfer::{Artifact, WriteMode};
use std::io::{Seek, SeekFrom, Write};
use std::sync::atomic::Ordering;
use std::time::Duration;

#[derive(Default)]
struct Observer {
    responses: Vec<(Artifact<'static>, WriteMode, u64, Option<Vec<u8>>)>,
    progress: Vec<(u64, u64)>,
    waits: Vec<WaitPhase>,
    order: Vec<&'static str>,
    stop: Option<&'static str>,
}
impl CacheObserver for Observer {
    fn observe(&mut self, notice: CacheNotice<'_, '_>) -> ObserverControl {
        let kind = match notice {
            CacheNotice::Response(fact) => {
                assert!(fact.target() == full(10).target());
                // Copy the known fixture target; callback fields are borrowed.
                self.responses.push((
                    full(10).target(),
                    fact.write_mode(),
                    fact.response_span(),
                    fact.strong_validator_bytes().map(Vec::from),
                ));
                "response"
            }
            CacheNotice::Progress(fact) => {
                assert!(fact.target() == full(10).target());
                self.progress
                    .push((fact.response_bytes(), fact.next_offset()));
                "progress"
            }
            CacheNotice::Waiting(fact) => {
                assert!(fact.target() == full(10).target());
                self.waits.push(fact.phase());
                "waiting"
            }
        };
        self.order.push(kind);
        if self.stop == Some(kind) {
            ObserverControl::Stop
        } else {
            ObserverControl::Continue
        }
    }
}
async fn observed_attempt(
    path: &'static str,
    plan: pinvou_upgrade_transfer::RequestPlan<'static>,
    mut owned: CacheFile,
    mut observer: Observer,
) -> (
    CacheFile,
    Observer,
    Result<CacheFetchOutcome<'static>, CacheReceiveError>,
) {
    let fixture = Fixture::load();
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let url = format!("{}{path}", fixture.origin);
        let dns = resolver(vec![fixture.address]);
        let request = CacheFetchRequest::new(
            &url,
            plan,
            &fixture.policy,
            &dns,
            ReplacementPermission::Allow,
        );
        let result = fetch_with_cache_observer_profile(
            &runtime,
            request,
            owned.file(),
            &mut || false,
            &mut observer,
            fixture.limits(Duration::from_secs(2)),
        );
        (owned, observer, result)
    })
    .await
    .unwrap()
}
#[tokio::test]
#[ignore = "requires isolated HTTPS fixture"]
async fn actual_response_and_whole_write_observations_do_not_bypass_stop_or_permissions() {
    let mut owned = CacheFile::new(b"old cache");
    owned.file().seek(SeekFrom::Start(7)).unwrap();
    let (mut owned, observer, result) = observed_attempt(
        "/file",
        full(10),
        owned,
        Observer {
            stop: Some("response"),
            ..Observer::default()
        },
    )
    .await;
    assert!(matches!(result, Err(CacheReceiveError::OwnerStopped)));
    assert_eq!(observer.order, ["response"]);
    assert_eq!(
        observer.responses[0].3.as_deref(),
        Some(b"\"original\"".as_slice())
    );
    assert_eq!(owned.file().stream_position().unwrap(), 7);
    assert_eq!(owned.bytes(), b"old cache");

    let (mut owned, observer, result) = observed_attempt(
        "/file",
        full(10),
        CacheFile::new(b""),
        Observer {
            stop: Some("progress"),
            ..Observer::default()
        },
    )
    .await;
    assert!(matches!(result, Err(CacheReceiveError::OwnerStopped)));
    assert_eq!(observer.progress, [(10, 10)]);
    assert_eq!(owned.bytes(), b"0123456789");
    assert_eq!(observer.order[0], "response");
    // Even all bytes plus observer progress do not yield the EOF/sync receipt.

    let (mut owned, observer, result) = observed_attempt(
        "/short-segment",
        range(4),
        CacheFile::new(b"0123"),
        Observer::default(),
    )
    .await;
    let CacheFetchOutcome::Segment(receipt) = result.unwrap() else {
        panic!("expected segment");
    };
    assert_eq!(receipt.next_offset(), 7);
    assert_eq!(observer.responses[0].1, WriteMode::Append { offset: 4 });
    assert_eq!(observer.responses[0].2, 3);
    assert_eq!(observer.progress, [(3, 7)]);
    assert_eq!(owned.bytes(), b"0123456");
}
#[tokio::test]
#[ignore = "requires isolated HTTPS fixture"]
async fn actual_partial_network_failure_preserves_tag_and_only_written_progress_without_receipt() {
    let (mut owned, observer, result) = observed_attempt(
        "/cache-partial-error",
        range(4),
        CacheFile::new(b"0123"),
        Observer::default(),
    )
    .await;
    assert!(matches!(result, Err(CacheReceiveError::Http(_))));
    assert_eq!(
        observer.responses[0].3.as_deref(),
        Some(b"\"original\"".as_slice())
    );
    assert_eq!(observer.responses[0].2, 6);
    assert_eq!(observer.progress, [(2, 6)]);
    assert_eq!(owned.bytes(), b"012345");
    assert_eq!(observer.order[0], "response");
    assert!(matches!(
        check_file(owned.file(), full(10).target(), || false),
        Err(ByteCheckError::TooShort)
    ));
}
#[tokio::test]
#[ignore = "requires isolated HTTPS fixture"]
async fn actual_waiting_observations_stop_the_same_fetch_body_and_dns_future() {
    for (path, expected, original) in [
        ("/cache-header-wait", WaitPhase::Fetch, b"keep".as_slice()),
        ("/cache-cancel-wait", WaitPhase::Body, b"".as_slice()),
    ] {
        let (mut owned, observer, result) = observed_attempt(
            path,
            full(10),
            CacheFile::new(b"keep"),
            Observer {
                stop: Some("waiting"),
                ..Observer::default()
            },
        )
        .await;
        assert!(matches!(result, Err(CacheReceiveError::OwnerStopped)));
        assert_eq!(observer.waits, [expected]);
        assert_eq!(owned.bytes(), original);
        assert_eq!(
            observer.responses.len(),
            usize::from(expected == WaitPhase::Body)
        );
    }
    let fixture = Fixture::load();
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let dns = Resolver {
            addresses: vec![fixture.address],
            calls: std::sync::atomic::AtomicUsize::new(0),
            delay: Duration::from_secs(5),
        };
        let url = format!("{}/file", fixture.origin);
        let request = CacheFetchRequest::new(
            &url,
            full(10),
            &fixture.policy,
            &dns,
            ReplacementPermission::Allow,
        );
        let mut owned = CacheFile::new(b"keep");
        let mut observer = Observer {
            stop: Some("waiting"),
            ..Observer::default()
        };
        let result = fetch_with_cache_observer_profile(
            &runtime,
            request,
            owned.file(),
            &mut || false,
            &mut observer,
            fixture.limits(Duration::from_secs(2)),
        );
        assert!(matches!(result, Err(CacheReceiveError::OwnerStopped)));
        assert_eq!(dns.calls.load(Ordering::SeqCst), 1);
        assert_eq!(observer.waits, [WaitPhase::Fetch]);
        assert!(observer.responses.is_empty() && observer.progress.is_empty());
        assert_eq!(owned.bytes(), b"keep");
    })
    .await
    .unwrap();
    let (mut owned, observer, result) = observed_attempt(
        "/slow-progress",
        full(10),
        CacheFile::new(b""),
        Observer::default(),
    )
    .await;
    let CacheFetchOutcome::Segment(receipt) = result.unwrap() else {
        panic!("expected segment");
    };
    assert!(check_file(owned.file(), receipt.target(), || false).is_ok());
    assert_eq!(observer.progress.last(), Some(&(10, 10)));
    assert!(
        observer
            .progress
            .windows(2)
            .all(|pair| pair[0].0 < pair[1].0 && pair[0].1 < pair[1].1)
    );
}
#[tokio::test]
#[ignore = "requires isolated HTTPS fixture"]
async fn private_partial_file_write_failure_emits_no_complete_block_progress() {
    let fixture = Fixture::load();
    let crate::FetchResult::Transfer(body) = fixture.get("/file", full(10)).await.unwrap() else {
        panic!("expected real body");
    };
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let mut owned = CacheFile::new(b"");
        let mut alias = owned.file().try_clone().unwrap();
        let mut observer = Observer::default();
        let result = crate::cache::receive_cache_with_write(
            &runtime,
            *body,
            owned.file(),
            CacheWritePermission::ReplaceExisting,
            &mut || false,
            &mut observer,
            &mut |_, bytes, _| {
                alias.write_all(&bytes[..1]).unwrap();
                Err(SegmentWriteError::File(ByteCheckError::IoUnavailable))
            },
        );
        assert!(matches!(
            result,
            Err(CacheReceiveError::Write(SegmentWriteError::File(
                ByteCheckError::IoUnavailable
            )))
        ));
        assert_eq!(observer.order, ["response"]);
        assert_eq!(owned.bytes(), b"0");
    })
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "requires isolated HTTPS fixture"]
async fn observer_continue_cannot_authorize_replacement_or_complete_a_pending_body() {
    let fixture = Fixture::load();
    let crate::FetchResult::Transfer(body) = fixture.get("/file", full(10)).await.unwrap() else {
        panic!("expected real body");
    };
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let mut owned = CacheFile::new(b"keep");
        let mut observer = Observer::default();
        let result = crate::receive_cache_body_with_observer_on_worker(
            &runtime,
            *body,
            owned.file(),
            CacheWritePermission::AppendExisting,
            || false,
            &mut observer,
        );
        assert!(matches!(
            result,
            Err(CacheReceiveError::Write(
                SegmentWriteError::WritePermissionMismatch
            ))
        ));
        assert_eq!(observer.order, ["response"]);
        assert_eq!(owned.bytes(), b"keep");
    })
    .await
    .unwrap();
    let (mut owned, observer, result) = observed_attempt(
        "/cache-cancel-wait",
        full(10),
        CacheFile::new(b"keep"),
        Observer::default(),
    )
    .await;
    assert!(matches!(
        result,
        Err(CacheReceiveError::Http(
            crate::FileHttpError::RequestTimeout
        ))
    ));
    assert!(observer.waits.contains(&WaitPhase::Body));
    assert!(observer.progress.is_empty());
    assert_eq!(owned.bytes(), b"");
}
#[tokio::test]
async fn complete_cache_verification_request_fabricates_no_response_progress_or_waiting_notice() {
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let mut owned = CacheFile::new(b"0123456789");
        let target = full(10).target();
        let prefix = pinvou_upgrade_transfer::PrefixState::new(None, 10, None).unwrap();
        let plan = pinvou_upgrade_transfer::plan_request(
            target,
            Some(prefix),
            pinvou_upgrade_transfer::SourceContinuity::Unproven,
        );
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
        let mut observer = Observer::default();
        let result = crate::fetch_cache_once_with_observer_on_worker(
            &runtime,
            request,
            owned.file(),
            || false,
            &mut observer,
        )
        .unwrap();
        assert!(matches!(result, CacheFetchOutcome::VerifyExisting(actual) if actual == target));
        assert!(observer.order.is_empty());
        assert_eq!(dns.calls.load(Ordering::SeqCst), 0);
        assert_eq!(owned.bytes(), b"0123456789");
    })
    .await
    .unwrap();
}

use super::super::resolver;
use super::{CacheFile, Fixture, HASH, full, range};
use crate::complete::fetch_and_check_with_profile;
use crate::{
    CacheCheckError, CacheFetchRequest, CacheNotice, CacheObserver, CacheReceiveError,
    CacheValidationOwner, CheckedCacheOutcome, ObserverControl, ReplacementPermission,
    ValidationDecision, ValidationStop,
};
use pinvou_upgrade_byte_check::{ByteCheckError, SegmentWriteError};
use pinvou_upgrade_transfer::{Artifact, PrefixState, RequestPlan, SourceContinuity, plan_request};
use std::io::{Seek, SeekFrom};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

struct Validation {
    begin: ValidationDecision,
    stop_at: Option<(usize, ValidationStop)>,
    begun: usize,
    polls: usize,
    original_deadline: u64,
    cancel_at: Option<usize>,
    cancel: Arc<AtomicBool>,
}
impl Validation {
    fn new() -> Self {
        Self {
            begin: ValidationDecision::Continue,
            stop_at: None,
            begun: 0,
            polls: 0,
            original_deadline: 300000,
            cancel_at: None,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
    fn assert_target(target: Artifact<'_>) {
        assert!(target == Artifact::new("package-1", 10, HASH).unwrap());
    }
}
impl CacheValidationOwner for Validation {
    fn begin_or_resume(&mut self, target: Artifact<'_>) -> ValidationDecision {
        // Scripted internal owner: no real clock/store certification is claimed.
        Self::assert_target(target);
        assert_eq!(self.original_deadline, 300000);
        self.begun += 1;
        self.begin
    }
    fn poll(&mut self, target: Artifact<'_>) -> ValidationDecision {
        Self::assert_target(target);
        assert_eq!(self.original_deadline, 300000);
        self.polls += 1;
        if self.cancel_at == Some(self.polls) {
            self.cancel.store(true, Ordering::SeqCst);
        }
        match self.stop_at {
            Some((count, reason)) if count == self.polls => ValidationDecision::Stop(reason),
            _ => ValidationDecision::Continue,
        }
    }
}
struct Observe {
    stop_on_response: bool,
    stop_on_progress: bool,
    responses: usize,
    progress: usize,
}
impl Observe {
    fn new() -> Self {
        Self {
            stop_on_response: false,
            stop_on_progress: false,
            responses: 0,
            progress: 0,
        }
    }
}
impl CacheObserver for Observe {
    fn observe(&mut self, notice: CacheNotice<'_, '_>) -> ObserverControl {
        match notice {
            CacheNotice::Response(_) => {
                self.responses += 1;
                if self.stop_on_response {
                    return ObserverControl::Stop;
                }
            }
            CacheNotice::Progress(_) => {
                self.progress += 1;
                if self.stop_on_progress {
                    return ObserverControl::Stop;
                }
            }
            CacheNotice::Waiting(_) => {}
        }
        ObserverControl::Continue
    }
}
async fn attempt(
    path: &'static str,
    plan: RequestPlan<'static>,
    mut file: CacheFile,
    replacement: ReplacementPermission,
    mut validation: Validation,
    mut observer: Observe,
) -> (
    CacheFile,
    Result<CheckedCacheOutcome<'static>, CacheCheckError>,
    Validation,
    Observe,
) {
    let fixture = Fixture::load();
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let url = format!("{}{path}", fixture.origin);
        let dns = resolver(vec![fixture.address]);
        let request = CacheFetchRequest::new(&url, plan, &fixture.policy, &dns, replacement);
        let cancel = validation.cancel.clone();
        let result = fetch_and_check_with_profile(
            &runtime,
            request,
            file.file(),
            &mut || cancel.load(Ordering::SeqCst),
            &mut observer,
            &mut validation,
            fixture.limits(Duration::from_secs(2)),
        );
        (file, result, validation, observer)
    })
    .await
    .unwrap()
}
#[tokio::test]
#[ignore = "requires isolated HTTPS fixture"]
async fn real_checked_cache_full_range_short_range_and_restart_keep_original_target() {
    let (mut file, result, owner, _) = attempt(
        "/ignored-range",
        range(4),
        CacheFile::new(b"keep"),
        ReplacementPermission::Refuse,
        Validation::new(),
        Observe::new(),
    )
    .await;
    assert!(matches!(
        result,
        Err(CacheCheckError::Transfer(CacheReceiveError::Write(
            SegmentWriteError::WritePermissionMismatch
        )))
    ));
    assert_eq!(owner.begun, 0);
    assert_eq!(file.bytes(), b"keep");
    for (path, plan, bytes) in [
        ("/ignored-range", range(4), b"old cache".as_slice()),
        ("/partial", range(4), b"0123".as_slice()),
        ("/partial", range(7), b"0123456".as_slice()),
    ] {
        let (mut file, result, owner, observer) = attempt(
            path,
            plan,
            CacheFile::new(bytes),
            ReplacementPermission::Allow,
            Validation::new(),
            Observe::new(),
        )
        .await;
        let CheckedCacheOutcome::Complete(matched) = result.unwrap() else {
            panic!("expected actual whole-file match");
        };
        Validation::assert_target(matched.target());
        assert_eq!(file.file().stream_position().unwrap(), 10);
        assert_eq!(file.bytes(), b"0123456789");
        assert_eq!(owner.begun, 1);
        assert_eq!(owner.polls, 5);
        assert_eq!(observer.responses, 1);
    }
    let (mut file, result, owner, _) = attempt(
        "/short-segment",
        range(4),
        CacheFile::new(b"0123"),
        ReplacementPermission::Refuse,
        Validation::new(),
        Observe::new(),
    )
    .await;
    let CheckedCacheOutcome::Partial(receipt) = result.unwrap() else {
        panic!("short response is incomplete");
    };
    assert_eq!(receipt.next_offset(), 7);
    assert_eq!(file.bytes(), b"0123456");
    assert_eq!((owner.begun, owner.polls), (0, 0));
    for path in ["/precondition", "/unsatisfiable"] {
        let mut file = CacheFile::new(b"keep");
        file.file().seek(SeekFrom::Start(7)).unwrap();
        let (mut file, result, owner, _) = attempt(
            path,
            range(4),
            file,
            ReplacementPermission::Allow,
            Validation::new(),
            Observe::new(),
        )
        .await;
        assert!(matches!(result, Ok(CheckedCacheOutcome::Restart(_))));
        assert_eq!(file.file().stream_position().unwrap(), 7);
        assert_eq!(file.bytes(), b"keep");
        assert_eq!((owner.begun, owner.polls), (0, 0));
    }
}
#[tokio::test]
#[ignore = "requires isolated HTTPS fixture"]
async fn real_checked_cache_complete_count_pollution_and_network_failure_never_match() {
    let (mut file, result, owner, _) = attempt(
        "/partial",
        range(4),
        CacheFile::new(b"XXXX"),
        ReplacementPermission::Refuse,
        Validation::new(),
        Observe::new(),
    )
    .await;
    assert!(matches!(
        result,
        Err(CacheCheckError::Check(ByteCheckError::HashMismatch))
    ));
    assert_eq!(file.bytes(), b"XXXX456789");
    assert_eq!(owner.begun, 1);
    // Wrong original expected hash is not replaced with a newly observed hash.
    let fixture = Fixture::load();
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        struct WrongHashOwner;
        impl CacheValidationOwner for WrongHashOwner {
            fn begin_or_resume(&mut self, target: Artifact<'_>) -> ValidationDecision {
                assert!(target == Artifact::new("package-1", 10, &"0".repeat(64)).unwrap());
                ValidationDecision::Continue
            }
            fn poll(&mut self, _: Artifact<'_>) -> ValidationDecision {
                ValidationDecision::Continue
            }
        }
        let mut file = CacheFile::new(b"old");
        let dns = resolver(vec![fixture.address]);
        let url = format!("{}/ignored-range", fixture.origin);
        let target = Artifact::new("package-1", 10, &"0".repeat(64)).unwrap();
        let request = CacheFetchRequest::new(
            &url,
            plan_request(target, None, SourceContinuity::Unproven),
            &fixture.policy,
            &dns,
            ReplacementPermission::Allow,
        );
        let result = fetch_and_check_with_profile(
            &runtime,
            request,
            file.file(),
            &mut || false,
            &mut Observe::new(),
            &mut WrongHashOwner,
            fixture.limits(Duration::from_secs(2)),
        );
        assert!(matches!(
            result,
            Err(CacheCheckError::Check(ByteCheckError::HashMismatch))
        ));
        assert_eq!(file.bytes(), b"0123456789");
    })
    .await
    .unwrap();
    for path in ["/network-error", "/truncated"] {
        let (_, result, owner, _) = attempt(
            path,
            full(10),
            CacheFile::new(b"old"),
            ReplacementPermission::Allow,
            Validation::new(),
            Observe::new(),
        )
        .await;
        assert!(matches!(result, Err(CacheCheckError::Transfer(_))));
        assert_eq!((owner.begun, owner.polls), (0, 0));
    }
}
#[tokio::test]
#[ignore = "requires isolated HTTPS fixture"]
async fn real_complete_download_cannot_start_validation_on_unproven_owner_budget() {
    let mut owner = Validation::new();
    owner.begin = ValidationDecision::Stop(ValidationStop::Unproven);
    let (mut file, result, owner, _) = attempt(
        "/ignored-range",
        full(10),
        CacheFile::new(b"old"),
        ReplacementPermission::Allow,
        owner,
        Observe::new(),
    )
    .await;
    assert!(matches!(
        result,
        Err(CacheCheckError::Validation(ValidationStop::Unproven))
    ));
    assert_eq!((owner.begun, owner.polls), (1, 0));
    assert_eq!(file.file().stream_position().unwrap(), 10);
    assert_eq!(file.bytes(), b"0123456789");
}
#[tokio::test]
#[ignore = "requires isolated HTTPS fixture"]
async fn real_response_or_progress_stop_does_not_begin_validation() {
    for on_response in [true, false] {
        let mut observe = Observe::new();
        observe.stop_on_response = on_response;
        observe.stop_on_progress = !on_response;
        let (mut file, result, owner, observer) = attempt(
            "/ignored-range",
            full(10),
            CacheFile::new(b"old"),
            ReplacementPermission::Allow,
            Validation::new(),
            observe,
        )
        .await;
        assert!(matches!(
            result,
            Err(CacheCheckError::Transfer(CacheReceiveError::OwnerStopped))
        ));
        assert_eq!((owner.begun, owner.polls), (0, 0));
        assert_eq!(observer.responses, 1);
        if on_response {
            assert_eq!(file.bytes(), b"old");
            assert_eq!(observer.progress, 0);
        } else {
            assert_eq!(file.bytes(), b"0123456789");
            assert_eq!(observer.progress, 1);
        }
    }
}
#[tokio::test]
#[ignore = "requires isolated HTTPS fixture"]
async fn local_complete_cache_actual_scan_and_owner_stops_need_no_url_dns_or_new_budget() {
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let target = Artifact::new("package-1", 10, HASH).unwrap();
        let fixture = Fixture::load();
        let dns = resolver(vec![]);
        let plan = || {
            plan_request(
                target,
                Some(PrefixState::new(None, 10, None).unwrap()),
                SourceContinuity::Unproven,
            )
        };
        for (bytes, error) in [
            (b"0123456789".as_slice(), None),
            (b"XXXXXXXXXX".as_slice(), Some(ByteCheckError::HashMismatch)),
            (b"012345678".as_slice(), Some(ByteCheckError::TooShort)),
            (b"01234567890".as_slice(), Some(ByteCheckError::TooLong)),
        ] {
            let mut file = CacheFile::new(bytes);
            let mut owner = Validation::new();
            let mut observe = Observe::new();
            let request = CacheFetchRequest::new(
                "invalid unused URL",
                plan(),
                &fixture.policy,
                &dns,
                ReplacementPermission::Refuse,
            );
            let result = fetch_and_check_with_profile(
                &runtime,
                request,
                file.file(),
                &mut || false,
                &mut observe,
                &mut owner,
                fixture.limits(Duration::from_secs(2)),
            );
            match error {
                Some(error) => {
                    assert!(matches!(result,Err(CacheCheckError::Check(actual)) if actual==error))
                }
                None => assert!(matches!(result, Ok(CheckedCacheOutcome::Complete(_)))),
            }
            assert_eq!(owner.begun, 1);
            assert_eq!(observe.responses, 0);
        }
        for reason in [
            ValidationStop::Expired,
            ValidationStop::Unproven,
            ValidationStop::TimeFault,
            ValidationStop::OwnerStopped,
        ] {
            let mut file = CacheFile::new(b"0123456789");
            file.file().seek(SeekFrom::Start(7)).unwrap();
            let mut owner = Validation::new();
            owner.begin = ValidationDecision::Stop(reason);
            let request = CacheFetchRequest::new(
                "invalid unused URL",
                plan(),
                &fixture.policy,
                &dns,
                ReplacementPermission::Refuse,
            );
            let result = fetch_and_check_with_profile(
                &runtime,
                request,
                file.file(),
                &mut || false,
                &mut Observe::new(),
                &mut owner,
                fixture.limits(Duration::from_secs(2)),
            );
            assert!(matches!(result,Err(CacheCheckError::Validation(actual)) if actual==reason));
            assert_eq!(file.file().stream_position().unwrap(), 7);
            assert_eq!((owner.begun, owner.polls), (1, 0));
            for count in 1..=5 {
                let mut file = CacheFile::new(b"0123456789");
                let mut owner = Validation::new();
                owner.stop_at = Some((count, reason));
                let request = CacheFetchRequest::new(
                    "invalid unused URL",
                    plan(),
                    &fixture.policy,
                    &dns,
                    ReplacementPermission::Refuse,
                );
                let result = fetch_and_check_with_profile(
                    &runtime,
                    request,
                    file.file(),
                    &mut || false,
                    &mut Observe::new(),
                    &mut owner,
                    fixture.limits(Duration::from_secs(2)),
                );
                assert!(
                    matches!(result,Err(CacheCheckError::Validation(actual)) if actual==reason)
                );
                assert_eq!(owner.polls, count);
                assert_eq!(owner.original_deadline, 300000);
            }
        }
        for count in 1..=5 {
            let mut file = CacheFile::new(b"0123456789");
            let mut owner = Validation::new();
            owner.cancel_at = Some(count);
            file.file().seek(SeekFrom::Start(7)).unwrap();
            let cancel = owner.cancel.clone();
            let request = CacheFetchRequest::new(
                "invalid unused URL",
                plan(),
                &fixture.policy,
                &dns,
                ReplacementPermission::Refuse,
            );
            let result = fetch_and_check_with_profile(
                &runtime,
                request,
                file.file(),
                &mut || cancel.load(Ordering::SeqCst),
                &mut Observe::new(),
                &mut owner,
                fixture.limits(Duration::from_secs(2)),
            );
            assert!(matches!(
                result,
                Err(CacheCheckError::Check(ByteCheckError::Cancelled))
            ));
            assert_eq!(owner.polls, count);
            if count == 1 {
                assert_eq!(file.file().stream_position().unwrap(), 7);
            }
        }
        assert_eq!(dns.calls.load(Ordering::SeqCst), 0);
    })
    .await
    .unwrap();
}

use crate::journal::Phase;
use crate::resume::tests::{fixture, imported};
use crate::resume::{ResumeCheckpointPin, prepare, publish_notice};
use crate::{Error, Job, platform};
use pinvou_upgrade_verifier::SignatureCheckedDownloadSource;
use serde_json::Value;
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
};
use std::{fs::File, path::PathBuf};
fn source(job: &Job, name: &str) -> SignatureCheckedDownloadSource {
    job.manifest.imported_source(&imported(name)).unwrap().1
}

struct Seed {
    path: PathBuf,
    pin: String,
    checkpoint: ResumeCheckpointPin,
}
impl Drop for Seed {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).unwrap();
    }
}
fn seed() -> Seed {
    let path = std::env::temp_dir().join(format!(
        "pinvou-resume-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let f = fixture();
    let pin = crate::create(
        &path,
        f["root"].as_str().unwrap().into(),
        f["rootSha256"].as_str().unwrap().into(),
        f["package"].as_str().unwrap().into(),
        f["expected"].as_str().unwrap().into(),
        &imported("original"),
    )
    .unwrap();
    let mut job = Job::open(&path, &pin).unwrap();
    let mut file = advance(&mut job);
    file.write_all(b"ab").unwrap();
    let proof = source(&job, "original");
    let mut checkpoint = None;
    publish_notice(
        &job,
        &mut file,
        &proof,
        Some(b"\"stable\\validator\""),
        &mut || false,
        &mut |notice| {
            let crate::Notice::Checkpoint { checkpoint_sha256 } = notice else {
                panic!("flushed checkpoint required");
            };
            checkpoint = Some(ResumeCheckpointPin::from_independently_retained_sha256(
                &checkpoint_sha256,
            )?);
            Ok(())
        },
    )
    .unwrap();
    let checkpoint = checkpoint.unwrap();
    job.journal
        .append(Phase::Paused, 1, job.journal.identity)
        .unwrap();
    drop(file);
    drop(job);
    Seed {
        path,
        pin,
        checkpoint,
    }
}
fn advance(job: &mut Job) -> File {
    let next = job.journal.attempt + 1;
    job.journal.append(Phase::Running, next, None).unwrap();
    let file = job
        .directory
        .file(crate::store::attempt_name(next), true, true, true)
        .unwrap();
    job.journal
        .append(
            Phase::Opened,
            next,
            Some(platform::identity(&file).unwrap()),
        )
        .unwrap();
    file
}
fn bytes(file: &mut File) -> Vec<u8> {
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut raw = Vec::new();
    file.read_to_end(&mut raw).unwrap();
    raw
}
#[test]
fn resume_real_flushed_checkpoint_reopens_prefix_and_accepts_new_signed_url() {
    let seed = seed();
    let mut job = Job::open(&seed.path, &seed.pin).unwrap();
    let proof = source(&job, "rotated");
    let mut destination = advance(&mut job);
    let prefix = prepare(
        &job,
        Some(&seed.checkpoint),
        &proof,
        &mut destination,
        &mut || false,
    )
    .unwrap()
    .unwrap();
    assert_eq!(bytes(&mut destination), b"ab");
    let artifact = job.target.artifact().unwrap();
    match prefix.plan(artifact).unwrap().kind() {
        pinvou_upgrade_transfer::RequestKind::Range { offset, validator } => {
            assert_eq!(offset, 2);
            assert_eq!(validator.as_bytes(), b"\"stable\\validator\"");
        }
        _ => panic!("same checked entity must resume the real copied prefix"),
    }
    assert_eq!(
        fs::read(seed.path.join(crate::store::attempt_name(1))).unwrap(),
        b"ab"
    );
    let raw = fs::read(
        seed.path
            .join(format!("resume-{}.json", seed.checkpoint.sha256())),
    )
    .unwrap();
    let record: Value = serde_json::from_slice(&raw).unwrap();
    assert!(record.get("url").is_none() && record.get("credential").is_none());
    assert_eq!(record["prefixSha256"], crate::hash(b"ab"));
}
#[test]
fn resume_changed_signed_entity_with_equal_etag_and_absent_pin_use_fresh_files() {
    for name in ["sourceId", "objectId", "objectVersion", "no-pin"] {
        let seed = seed();
        let mut job = Job::open(&seed.path, &seed.pin).unwrap();
        let proof = source(&job, if name == "no-pin" { "original" } else { name });
        let mut destination = advance(&mut job);
        let pin = if name == "no-pin" {
            None
        } else {
            Some(&seed.checkpoint)
        };
        assert!(
            prepare(&job, pin, &proof, &mut destination, &mut || false)
                .unwrap()
                .is_none()
        );
        assert_eq!(destination.metadata().unwrap().len(), 0);
        assert_eq!(
            fs::read(seed.path.join(crate::store::attempt_name(1))).unwrap(),
            b"ab"
        );
    }
}
#[test]
fn resume_bad_external_pin_checkpoint_bytes_and_closed_shape_are_refused() {
    for mode in ["missing-pin-object", "changed-bytes", "closed-shape"] {
        let seed = seed();
        let mut job = Job::open(&seed.path, &seed.pin).unwrap();
        let proof = source(&job, "original");
        let mut destination = advance(&mut job);
        let pin = match mode {
            "missing-pin-object" => {
                ResumeCheckpointPin::from_independently_retained_sha256(&"0".repeat(64)).unwrap()
            }
            "changed-bytes" => {
                fs::write(
                    seed.path
                        .join(format!("resume-{}.json", seed.checkpoint.sha256())),
                    b"{}",
                )
                .unwrap();
                seed.checkpoint.clone()
            }
            _ => {
                let raw = b"{\"protocolVersion\":1,\"unrecognized\":true}";
                let pin =
                    ResumeCheckpointPin::from_independently_retained_sha256(&crate::hash(raw))
                        .unwrap();
                fs::write(seed.path.join(format!("resume-{}.json", pin.sha256())), raw).unwrap();
                pin
            }
        };
        let result = prepare(&job, Some(&pin), &proof, &mut destination, &mut || false);
        assert!(matches!(
            (mode, result),
            ("missing-pin-object", Err(Error::NotFound))
                | ("changed-bytes", Err(Error::PinMismatch))
                | ("closed-shape", Err(Error::InvalidInput))
        ));
        assert_eq!(destination.metadata().unwrap().len(), 0);
    }
}
#[test]
fn resume_prefix_pollution_length_change_and_replacement_never_append_or_heal() {
    for mode in ["pollution", "length", "identity"] {
        let seed = seed();
        let path = seed.path.join(crate::store::attempt_name(1));
        match mode {
            "pollution" => fs::write(&path, b"ax").unwrap(),
            "length" => fs::write(&path, b"a").unwrap(),
            _ => {
                fs::rename(&path, path.with_extension("retained")).unwrap();
                fs::write(&path, b"ab").unwrap();
            }
        }
        let damaged = fs::read(&path).unwrap();
        let mut job = Job::open(&seed.path, &seed.pin).unwrap();
        let proof = source(&job, "original");
        let mut destination = advance(&mut job);
        assert!(matches!(
            prepare(
                &job,
                Some(&seed.checkpoint),
                &proof,
                &mut destination,
                &mut || false
            ),
            Err(Error::ByteMismatch | Error::UnsafeObject)
        ));
        assert_eq!(fs::read(&path).unwrap(), damaged);
        assert_eq!(destination.metadata().unwrap().len(), 0);
    }
}
#[test]
fn resume_copy_cancellation_preserves_original_prefix_and_never_authorizes_range() {
    let seed = seed();
    let mut job = Job::open(&seed.path, &seed.pin).unwrap();
    let proof = source(&job, "original");
    let mut destination = advance(&mut job);
    let mut polls = 0;
    let result = prepare(
        &job,
        Some(&seed.checkpoint),
        &proof,
        &mut destination,
        &mut || {
            polls += 1;
            polls >= 5
        },
    );
    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(polls >= 5);
    assert_eq!(
        fs::read(seed.path.join(crate::store::attempt_name(1))).unwrap(),
        b"ab"
    );
}
#[test]
fn resume_real_sync_failure_and_missing_validator_never_emit_checkpoint_notice() {
    let seed = seed();
    let job = Job::open(&seed.path, &seed.pin).unwrap();
    let proof = source(&job, "original");
    // A read-only actual Windows file cannot FlushFileBuffers. This is a
    // real native sync refusal, not a fabricated success/protected object.
    let mut read_only = job.attempt_file().unwrap();
    let mut notices = 0;
    let result = publish_notice(
        &job,
        &mut read_only,
        &proof,
        Some(b"\"etag\""),
        &mut || false,
        &mut |_| {
            notices += 1;
            Ok(())
        },
    );
    assert!(matches!(result, Err(Error::IoUnavailable)));
    assert_eq!(notices, 0);
    assert!(
        publish_notice(
            &job,
            &mut read_only,
            &proof,
            None,
            &mut || false,
            &mut |_| {
                notices += 1;
                Ok(())
            }
        )
        .is_ok()
    );
    assert_eq!(notices, 0);
}
#[tokio::test]
async fn resume_started_pause_and_cancel_commit_original_terminal_instead_of_opened() {
    for stop_request in [crate::StopRequest::Pause, crate::StopRequest::Cancel] {
        let seed = seed();
        let path = seed.path.clone();
        let pin = seed.pin.clone();
        let mut observed = 0;
        // An actual Started callback writes the original fixed-name control
        // record while the real job writer is held by the worker.
        let result = crate::run_with_resume(
            seed.path.clone(),
            seed.pin.clone(),
            imported("original"),
            crate::NetworkConsent {
                host: "files.example.invalid".into(),
                port: 443,
                addresses: vec!["127.0.0.1".parse().unwrap()],
            },
            false,
            Some(seed.checkpoint.clone()),
            move |notice| {
                match notice {
                    crate::Notice::Started { snapshot } => {
                        observed += 1;
                        assert_eq!(observed, 1);
                        crate::request_stop(
                            &path,
                            &pin,
                            snapshot.run_id.as_deref().unwrap(),
                            stop_request,
                        )?;
                    }
                    _ => panic!(
                        "copy stopped before any body progress, waiting or checkpoint notice"
                    ),
                }
                Ok(())
            },
        )
        .await
        .unwrap();
        let expected = if stop_request == crate::StopRequest::Pause {
            "paused"
        } else {
            "cancelled"
        };
        assert_eq!(result.recorded_phase, expected);
        assert!(
            !result.observed_bytes_match
                && !result.current_qualification_checked
                && !result.native_authenticated
                && !result.verified_execution_object
        );
        let job = Job::open(&seed.path, &seed.pin).unwrap();
        assert_eq!(job.snapshot().recorded_phase, expected);
        assert_eq!(job.journal.attempt, 2);
        assert_eq!(
            fs::metadata(seed.path.join(crate::store::attempt_name(2)))
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            fs::read(seed.path.join(crate::store::attempt_name(1))).unwrap(),
            b"ab"
        );
    }
}
#[test]
fn resume_existing_checkpoint_requires_real_write_access_and_sync_before_notice() {
    struct Permissions {
        path: PathBuf,
        original: fs::Permissions,
    }
    impl Drop for Permissions {
        fn drop(&mut self) {
            fs::set_permissions(&self.path, self.original.clone()).unwrap();
        }
    }
    let seed = seed();
    let job = Job::open(&seed.path, &seed.pin).unwrap();
    let proof = source(&job, "original");
    let mut writable = job
        .directory
        .file(crate::store::attempt_name(1), true, false, true)
        .unwrap();
    let path = seed
        .path
        .join(format!("resume-{}.json", seed.checkpoint.sha256()));
    let restore = Permissions {
        original: fs::metadata(&path).unwrap().permissions(),
        path,
    };
    let mut permissions = restore.original.clone();
    permissions.set_readonly(true);
    fs::set_permissions(&restore.path, permissions).unwrap();
    let mut notices = 0;
    let result = publish_notice(
        &job,
        &mut writable,
        &proof,
        Some(b"\"stable\\validator\""),
        &mut || false,
        &mut |_| {
            notices += 1;
            Ok(())
        },
    );
    assert!(matches!(result, Err(Error::IoUnavailable)));
    assert_eq!(notices, 0);
}
#[tokio::test]
async fn resume_invalid_pin_and_changed_prefix_close_attempt_as_io_unknown() {
    for mode in ["invalid-pin", "changed-prefix"] {
        let seed = seed();
        let expected_pin = if mode == "invalid-pin" {
            ResumeCheckpointPin::from_independently_retained_sha256(&"0".repeat(64)).unwrap()
        } else {
            fs::write(seed.path.join(crate::store::attempt_name(1)), b"ax").unwrap();
            seed.checkpoint.clone()
        };
        let original = fs::read(seed.path.join(crate::store::attempt_name(1))).unwrap();
        let result = crate::run_with_resume(
            seed.path.clone(),
            seed.pin.clone(),
            imported("original"),
            crate::NetworkConsent {
                host: "files.example.invalid".into(),
                port: 443,
                addresses: vec!["127.0.0.1".parse().unwrap()],
            },
            false,
            Some(expected_pin),
            |notice| match notice {
                crate::Notice::Started { .. } => Ok(()),
                _ => panic!("invalid prefix must not reach GET or checkpoint"),
            },
        )
        .await;
        if mode == "invalid-pin" {
            assert!(matches!(result, Err(Error::NotFound)));
        } else {
            assert!(matches!(result, Err(Error::ByteMismatch)));
        }
        let job = Job::open(&seed.path, &seed.pin).unwrap();
        assert_eq!(job.snapshot().recorded_phase, "ioUnknown");
        assert_eq!(job.journal.attempt, 2);
        assert_eq!(
            fs::read(seed.path.join(crate::store::attempt_name(1))).unwrap(),
            original
        );
    }
}
#[test]
fn resume_checkpoint_scan_observes_real_cancel_and_emits_no_pin() {
    let seed = seed();
    let job = Job::open(&seed.path, &seed.pin).unwrap();
    let proof = source(&job, "original");
    let run_id = job.journal.run_id.clone().unwrap();
    let mut writable = job
        .directory
        .file(crate::store::attempt_name(1), true, false, true)
        .unwrap();
    let mut polls = 0;
    let mut notices = 0;
    let result = publish_notice(
        &job,
        &mut writable,
        &proof,
        Some(b"\"stable\\validator\""),
        &mut || {
            polls += 1;
            if polls == 2 {
                crate::request_stop(&seed.path, &seed.pin, &run_id, crate::StopRequest::Cancel)
                    .unwrap();
            }
            matches!(
                job.control(&run_id).unwrap(),
                Some(crate::StopRequest::Cancel)
            )
        },
        &mut |_| {
            notices += 1;
            Ok(())
        },
    );
    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(polls >= 2);
    assert_eq!(notices, 0);
    assert!(matches!(
        job.control(&run_id).unwrap(),
        Some(crate::StopRequest::Cancel)
    ));
    assert_eq!(bytes(&mut writable), b"ab");
}
#[test]
fn resume_real_pause_allows_flushed_notice_and_callback_cancelled_still_propagates() {
    for callback_error in [false, true] {
        let seed = seed();
        let job = Job::open(&seed.path, &seed.pin).unwrap();
        let proof = source(&job, "original");
        let run_id = job.journal.run_id.clone().unwrap();
        crate::request_stop(&seed.path, &seed.pin, &run_id, crate::StopRequest::Pause).unwrap();
        let mut writable = job
            .directory
            .file(crate::store::attempt_name(1), true, false, true)
            .unwrap();
        let mut notices = 0;
        let result = publish_notice(
            &job,
            &mut writable,
            &proof,
            Some(b"\"stable\\validator\""),
            &mut || match job.control(&run_id).unwrap() {
                Some(crate::StopRequest::Cancel) => true,
                Some(crate::StopRequest::Pause) | None => false,
            },
            &mut |notice| {
                let crate::Notice::Checkpoint { checkpoint_sha256 } = notice else {
                    panic!("checkpoint notice required");
                };
                notices += 1;
                assert_eq!(checkpoint_sha256, seed.checkpoint.sha256());
                if callback_error {
                    Err(Error::Cancelled)
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(notices, 1);
        if callback_error {
            assert!(matches!(result, Err(Error::Cancelled)));
        } else {
            assert!(result.is_ok());
        }
        assert_eq!(job.snapshot().recorded_phase, "paused");
        assert_eq!(bytes(&mut writable), b"ab");
    }
}
#[test]
fn resume_stop_bookkeeping_preserves_an_observed_real_cancel_over_later_pause() {
    use std::{
        cell::Cell,
        sync::atomic::{AtomicBool, Ordering},
    };
    let seed = seed();
    let job = Job::open(&seed.path, &seed.pin).unwrap();
    let run_id = job.journal.run_id.as_deref().unwrap();
    crate::request_stop(&seed.path, &seed.pin, run_id, crate::StopRequest::Cancel).unwrap();
    let observed = job.control(run_id).unwrap().unwrap();
    let requested = Cell::new(None);
    // Exercise the same production bookkeeping after a real control read.
    // This does not claim that a caller-future drop ran end to end.
    crate::runner::remember_stop(&requested, observed);
    assert!(requested.get() == Some(crate::StopRequest::Cancel));
    let stop = AtomicBool::new(true);
    if stop.load(Ordering::Acquire) {
        crate::runner::remember_stop(&requested, crate::StopRequest::Pause);
    }
    assert!(requested.get() == Some(crate::StopRequest::Cancel));
    assert!(matches!(
        job.control(run_id).unwrap(),
        Some(crate::StopRequest::Cancel)
    ));
    assert_eq!(
        fs::read(seed.path.join(crate::store::attempt_name(1))).unwrap(),
        b"ab"
    );
}
#[test]
fn resume_200_replacement_and_416_leave_original_partial_unchanged() {
    use pinvou_upgrade_transfer::{ResponseDecision, ResponseHeaders, plan_response};
    let seed = seed();
    let mut job = Job::open(&seed.path, &seed.pin).unwrap();
    let proof = source(&job, "original");
    let mut destination = advance(&mut job);
    let prefix = prepare(
        &job,
        Some(&seed.checkpoint),
        &proof,
        &mut destination,
        &mut || false,
    )
    .unwrap()
    .unwrap();
    let artifact = job.target.artifact().unwrap();
    assert!(matches!(
        plan_response(
            prefix.plan(artifact).unwrap(),
            416,
            ResponseHeaders::from_fields([]).unwrap()
        )
        .unwrap(),
        ResponseDecision::Restart(_)
    ));
    assert_eq!(bytes(&mut destination), b"ab");
    let ResponseDecision::Transfer(body) = plan_response(
        prefix.plan(artifact).unwrap(),
        200,
        ResponseHeaders::from_fields([]).unwrap(),
    )
    .unwrap() else {
        panic!("legal full response required");
    };
    let writer = pinvou_upgrade_byte_check::FileSegmentWriter::start(
        &mut destination,
        body,
        pinvou_upgrade_byte_check::CacheWritePermission::ReplaceExisting,
        || false,
    )
    .unwrap();
    drop(writer);
    assert_eq!(destination.metadata().unwrap().len(), 0);
    assert_eq!(
        fs::read(seed.path.join(crate::store::attempt_name(1))).unwrap(),
        b"ab"
    );
}

#[tokio::test]
async fn resume_public_run_refuses_expiry_after_started_before_network() {
    let seed = seed();
    let mut import = imported("original");
    let request: Value = serde_json::from_str(&import.request).unwrap();
    let response: Value = serde_json::from_str(&import.response).unwrap();
    let end = [
        request["decision"]["signed"]["exp"].as_f64().unwrap() as u64,
        response["credential"]["signed"]["exp"].as_f64().unwrap() as u64,
        response["expiresAt"].as_f64().unwrap() as u64,
    ]
    .into_iter()
    .min()
    .unwrap();
    import.supplied_time_millis = end.checked_sub(1).unwrap();
    // This is still valid at the first observation; a real local delay then
    // consumes the last millisecond. No fake production time source is used.
    {
        let job = Job::open(&seed.path, &seed.pin).unwrap();
        assert!(job.manifest.imported_source(&import).is_ok());
    }
    let result = crate::run_with_resume(
        seed.path.clone(),
        seed.pin.clone(),
        import,
        crate::NetworkConsent {
            host: "127.0.0.1".into(),
            port: 443,
            addresses: vec!["127.0.0.1".parse().unwrap()],
        },
        false,
        Some(seed.checkpoint.clone()),
        |notice| match notice {
            crate::Notice::Started { .. } => {
                std::thread::sleep(std::time::Duration::from_millis(2));
                Ok(())
            }
            _ => panic!("expired preparation must not reach transfer or checkpoint"),
        },
    )
    .await;
    // The adapter's destination-policy refusal has a different typed error.
    // The signature gate must finish this attempt before any network action.
    assert!(matches!(result, Err(Error::SignatureRefused)));
    let job = Job::open(&seed.path, &seed.pin).unwrap();
    assert_eq!(job.snapshot().recorded_phase, "sourceRefused");
    assert_eq!(job.journal.attempt, 2);
    for attempt in [1, 2] {
        assert_eq!(
            fs::read(seed.path.join(crate::store::attempt_name(attempt))).unwrap(),
            b"ab"
        );
    }
}

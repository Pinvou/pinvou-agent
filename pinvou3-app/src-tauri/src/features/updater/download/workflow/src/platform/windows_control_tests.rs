use crate::{
    ControlAttempt, Error, Import, Job, NetworkConsent,
    journal::Phase,
    run_control,
    tests::{fixture, imported},
};
use std::{fs, io::Write, path::PathBuf};
fn actual_job() -> (PathBuf, String) {
    let path = std::env::temp_dir().join(format!(
        "pinvou-control-caller-{}-{}",
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
        &imported(),
    )
    .unwrap();
    (path, pin)
}
#[tokio::test]
async fn control_preflight_refusal_preserves_original_journal_and_releases_real_writer() {
    let (path, pin) = actual_job();
    let before = fs::read(path.join("journal.jsonl")).unwrap();
    let result = run_control(
        path.clone(),
        pin.clone(),
        ControlAttempt {
            request: "{}".into(),
            supplied_time_millis: imported().supplied_time_millis,
        },
        consent(),
        false,
        |_| panic!("no file attempt before control acceptance"),
    )
    .await;
    assert!(matches!(result, Err(Error::InvalidInput)));
    assert_eq!(fs::read(path.join("journal.jsonl")).unwrap(), before);
    let job = Job::open(&path, &pin).unwrap();
    assert_eq!(job.snapshot().attempt, 0);
    drop(job);
    fs::remove_dir_all(path).unwrap();
}
#[tokio::test]
async fn control_complete_reuse_scans_actual_bytes_and_never_converts_record_to_rights() {
    let (path, pin) = actual_job();
    let mut job = Job::open(&path, &pin).unwrap();
    job.journal.append(Phase::Running, 1, None).unwrap();
    let mut file =
        crate::platform::file(&path.join(crate::store::attempt_name(1)), true, true, true).unwrap();
    let identity = crate::platform::identity(&file).unwrap();
    job.journal
        .append(Phase::Opened, 1, Some(identity))
        .unwrap();
    file.write_all(b"abc").unwrap();
    file.sync_all().unwrap();
    drop(file);
    job.journal
        .append(Phase::BytesMatch, 1, Some(identity))
        .unwrap();
    drop(job);
    // Deliberately invalid input proves the successful cache branch does not
    // perform any POST; it still scans the original retained actual file.
    let request = || ControlAttempt {
        request: String::new(),
        supplied_time_millis: 0,
    };
    let result = run_control(
        path.clone(),
        pin.clone(),
        request(),
        consent(),
        false,
        |_| Ok(()),
    )
    .await
    .unwrap();
    assert!(result.observed_bytes_match);
    assert!(
        !result.current_qualification_checked
            && !result.native_authenticated
            && !result.verified_execution_object
    );
    fs::write(path.join(crate::store::attempt_name(1)), b"axc").unwrap();
    assert!(matches!(
        run_control(path.clone(), pin, request(), consent(), false, |_| Ok(())).await,
        Err(Error::ByteMismatch)
    ));
    fs::remove_dir_all(path).unwrap();
}
#[tokio::test]
async fn control_caller_cannot_post_while_an_original_writer_owns_job() {
    let (path, pin) = actual_job();
    let held = Job::open(&path, &pin).unwrap();
    assert!(matches!(
        run_control(
            path.clone(),
            pin.clone(),
            input(&imported()),
            consent(),
            false,
            |_| panic!("busy writer cannot emit started")
        )
        .await,
        Err(Error::Busy)
    ));
    drop(held);
    assert!(Job::open(&path, &pin).is_ok());
    fs::remove_dir_all(path).unwrap();
}

fn input(import: &Import) -> ControlAttempt {
    ControlAttempt {
        request: import.request.clone(),
        supplied_time_millis: import.supplied_time_millis,
    }
}
fn consent() -> NetworkConsent {
    NetworkConsent {
        host: "127.0.0.1".into(),
        port: 1,
        addresses: vec!["127.0.0.1".parse().unwrap()],
    }
}

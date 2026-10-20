//! Actual role selection against independently signed, multiple-native FullPack.
use super::{EXE, Fixture, JOB_EXE};
use pinvou_upgrade_native_inspection::{Role, RoleRequest, inspect_role};
use serde_json::Value;
use std::{fs, process::Command, sync::atomic::AtomicBool};

fn fixture(kind: &str) -> Fixture {
    let (f, created) = Fixture::prepare(kind, "roles", JOB_EXE);
    assert!(
        created.status.success(),
        "actual original create refused valid role fixture: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    f
}
fn command(f: &Fixture, kind: &str, helper: Option<&str>, member: &str) -> Command {
    let mut cmd = Command::new(EXE);
    cmd.arg(kind)
        .arg(&f.job)
        .arg(&f.pin)
        .arg("1")
        .arg(&f.output);
    if let Some(id) = helper {
        cmd.arg(id);
    }
    cmd.arg(member).arg(f.root.join("limits.json"));
    cmd
}
fn collect(f: &Fixture, kind: &str, helper: Option<&str>, member: &str) -> Value {
    let result = command(f, kind, helper, member).output().unwrap();
    assert!(
        result.status.success(),
        "actual role observation failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    for key in [
        "publisherQualification",
        "currentQualification",
        "installationRights",
        "verifiedExecutionObject",
        "trustedDeadlineEvidence",
    ] {
        assert_eq!(report[key], false);
        assert_eq!(report["observation"][key], false);
        assert_eq!(report["observation"]["native"][key], false);
    }
    let core = &report["observation"];
    assert_eq!(core["retained"]["manifestSha256"], f.pin);
    assert_eq!(core["memberName"], member);
    assert_eq!(core["declaredNative"]["size"], core["native"]["size"]);
    assert_eq!(core["declaredNative"]["sha256"], core["native"]["sha256"]);
    assert_eq!(core["retained"]["recordedPhase"], "journalNotConsulted");
    assert_eq!(core["retained"]["nativeAuthenticated"], false);
    report
}
#[test]
fn actual_execution_helper_is_signed_second_declaration_and_named_first_is_unsigned() {
    let f = fixture("signed");
    let package: Value =
        serde_json::from_slice(&fs::read(f.root.join("package.json")).unwrap()).unwrap();
    assert_eq!(package["signed"]["helpers"][0]["helperId"], "helper-first");
    assert_eq!(package["signed"]["executionHelperId"], "helper-execution");
    assert_ne!(
        package["signed"]["helpers"][0]["nativeIdentity"]["sha256"],
        package["signed"]["helpers"][1]["nativeIdentity"]["sha256"]
    );
    let execution = collect(&f, "inspect-execution-helper", None, "helper/execution.exe");
    assert_eq!(execution["selection"]["kind"], "executionHelper");
    assert_eq!(execution["selection"]["helperId"], "helper-execution");
    assert_eq!(
        execution["observation"]["declaredNative"],
        package["signed"]["helper"]
    );
    assert_eq!(
        execution["observation"]["declaredNative"]["format"],
        "helper"
    );
    assert!(
        !execution["observation"]["native"]["signatures"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let first = collect(
        &f,
        "inspect-helper-artifact",
        Some("helper-first"),
        "helper/first.exe",
    );
    assert_eq!(first["selection"]["kind"], "helperArtifact");
    assert_eq!(first["selection"]["helperId"], "helper-first");
    assert_eq!(
        first["observation"]["declaredNative"],
        package["signed"]["helpers"][0]["nativeIdentity"]
    );
    assert!(
        first["observation"]["native"]["signatures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["verification"] == "unsigned")
    );
    // A matching byte alias is a locator; installationTarget is never one.
    let alias = collect(
        &f,
        "inspect-helper-artifact",
        Some("helper-execution"),
        "aliases/execution.exe",
    );
    assert_eq!(alias["selection"]["kind"], "helperArtifact");
    assert_eq!(alias["selection"]["helperId"], "helper-execution");
    assert_eq!(
        alias["observation"]["native"]["sha256"],
        execution["observation"]["native"]["sha256"]
    );
    let launcher = collect(&f, "inspect-launcher", None, "launcher/launcher.exe");
    assert_eq!(launcher["selection"]["kind"], "launcher");
    assert_eq!(launcher["selection"]["helperId"], Value::Null);
    assert_eq!(
        launcher["observation"]["declaredNative"],
        package["signed"]["launcher"]
    );
}
#[test]
fn actual_role_refusals_and_damaged_journal_preserve_original_legacy_report_profile() {
    let f = fixture("signed");
    for (action, id, member) in [
        (
            "inspect-helper-artifact",
            Some("not-declared"),
            "helper/first.exe",
        ),
        (
            "inspect-helper-artifact",
            Some("helper-first"),
            "helper/execution.exe",
        ),
        ("inspect-execution-helper", None, "helper/first.exe"),
        ("inspect-launcher", None, "helper/execution.exe"),
        (
            "inspect-execution-helper",
            None,
            "logical/execution-not-a-zip-member",
        ),
    ] {
        let result = command(&f, action, id, member).output().unwrap();
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(fs::read_dir(&f.output).unwrap().next().is_none());
    }
    let mut override_request =
        command(&f, "inspect-execution-helper", None, "helper/execution.exe");
    override_request.arg("helper-first");
    assert!(!override_request.output().unwrap().status.success());
    let mut missing_id = command(&f, "inspect-helper-artifact", None, "helper/first.exe");
    assert!(!missing_id.output().unwrap().status.success());
    fs::write(f.job.join("journal.jsonl"), b"damaged").unwrap();
    collect(&f, "inspect-execution-helper", None, "helper/execution.exe");
    let legacy = f.collect();
    let expected: std::collections::BTreeSet<_> = [
        "protocolVersion",
        "jobId",
        "rootSha256",
        "targetKey",
        "originalSuppliedTimeMillis",
        "retained",
        "fileKind",
        "packageId",
        "packageSize",
        "packageSha256",
        "packageEnvelopeSha256",
        "sourceIdentity",
        "memberName",
        "declaredNative",
        "destinationName",
        "native",
        "publisherQualification",
        "currentQualification",
        "installationRights",
        "verifiedExecutionObject",
        "trustedDeadlineEvidence",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(
        legacy
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        expected
    );
    assert!(legacy.get("selection").is_none());
    assert!(legacy.get("observation").is_none());
}
#[test]
fn signed_missing_duplicate_wrong_target_and_canonical_helper_declarations_refuse_before_job_creation()
 {
    for profile in [
        "missing-helper",
        "duplicate-helper",
        "helper-target",
        "missing-execution-id",
        "helper-canonical",
    ] {
        let (f, created) = Fixture::prepare("signed", profile, JOB_EXE);
        assert!(
            !created.status.success(),
            "original Package semantics must refuse {profile}"
        );
        assert!(created.stdout.is_empty());
        assert!(!f.job.exists());
        assert!(fs::read_dir(&f.output).unwrap().next().is_none());
        assert_eq!(
            String::from_utf8(created.stderr).unwrap().trim(),
            "DOWNLOAD_JOB_SIGNATURE_REFUSED"
        );
    }
}
#[test]
fn actual_rehashed_execution_helper_tamper_and_unsigned_launcher_reach_fixed_native_policy() {
    let f = fixture("role-tampered");
    let report = collect(&f, "inspect-execution-helper", None, "helper/execution.exe");
    let signatures = report["observation"]["native"]["signatures"]
        .as_array()
        .unwrap();
    assert!(!signatures.is_empty());
    assert!(
        signatures
            .iter()
            .all(|s| s["verification"] == "invalidSignature" && s["nativeStatus"] == "80096010")
    );
    let f = fixture("launcher-unsigned");
    let report = collect(&f, "inspect-launcher", None, "launcher/launcher.exe");
    assert!(
        report["observation"]["native"]["signatures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["verification"] == "unsigned")
    );
}
#[test]
fn actual_role_library_guard_and_cancel_keep_original_job_and_target_lifetimes() {
    use std::os::windows::fs::OpenOptionsExt;
    let f = fixture("signed");
    let limits = serde_json::from_slice(&fs::read(f.root.join("limits.json")).unwrap()).unwrap();
    let request = || RoleRequest {
        role: Role::ExecutionHelper,
        member_name: "helper/execution.exe",
    };
    assert!(matches!(
        inspect_role(
            &f.job,
            &f.pin,
            1,
            &f.output,
            request(),
            &limits,
            &AtomicBool::new(true)
        ),
        Err(pinvou_upgrade_native_inspection::Error::Cancelled)
    ));
    assert!(fs::read_dir(&f.output).unwrap().next().is_none());
    let observed = inspect_role(
        &f.job,
        &f.pin,
        1,
        &f.output,
        request(),
        &limits,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(observed.selection().kind(), "executionHelper");
    assert_eq!(observed.selection().helper_id(), Some("helper-execution"));
    let report = serde_json::to_value(observed.report()).unwrap();
    let target = f
        .output
        .join(report["observation"]["destinationName"].as_str().unwrap());
    assert!(
        fs::OpenOptions::new()
            .write(true)
            .share_mode(7)
            .open(&target)
            .is_err()
    );
    assert!(fs::rename(&target, f.output.join("forbidden.exe")).is_err());
    assert!(
        !Command::new(JOB_EXE)
            .arg("status")
            .arg(&f.job)
            .arg(&f.pin)
            .output()
            .unwrap()
            .status
            .success()
    );
    drop(observed);
    assert!(
        fs::OpenOptions::new()
            .write(true)
            .share_mode(7)
            .open(&target)
            .is_ok()
    );
    fs::rename(target, f.output.join("released.exe")).unwrap();
    assert!(
        Command::new(JOB_EXE)
            .arg("status")
            .arg(&f.job)
            .arg(&f.pin)
            .output()
            .unwrap()
            .status
            .success()
    );
}
#[test]
fn actual_execution_helper_partial_copy_cancel_is_joined_with_real_retained_prefix() {
    let f = fixture("role-large");
    super::partial_copy_proof(
        &f,
        Some(Role::ExecutionHelper),
        "helper/execution.exe",
        "helper-execution.exe",
    );
}
#[test]
fn actual_execution_helper_cli_death_releases_original_source_and_writer() {
    let f = fixture("role-large");
    super::kill_join_proof(
        &f,
        command(&f, "inspect-execution-helper", None, "helper/execution.exe"),
    );
}
#[test]
#[ignore = "requires independently retained original52b3 T29 CLI; no substitute binary"]
fn original_52b3_job_can_observe_exact_execution_helper_and_old_final_installer() {
    use sha2::{Digest, Sha256};
    let creator = std::env::var("PINVOU_UPGRADE_LEGACY_DOWNLOAD_CLI")
        .expect("supply the retained original binary path");
    let bytes = fs::read(&creator).unwrap();
    let hash: String = Sha256::digest(bytes)
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect();
    assert_eq!(
        hash,
        "52b3b639e9f64a557813b37090255706e375eeb31013c29b57ccadcf684168b3"
    );
    let (f, created) = Fixture::prepare("signed", "roles", &creator);
    assert!(created.status.success());
    let report = collect(&f, "inspect-execution-helper", None, "helper/execution.exe");
    assert_eq!(report["selection"]["helperId"], "helper-execution");
    f.collect();
}
#[test]
#[ignore = "requires existing complete cache-only system signature/timestamp chains"]
fn cache_only_strict_execution_helper_positive() {
    let f = fixture("signed");
    let report = collect(&f, "inspect-execution-helper", None, "helper/execution.exe");
    assert!(
        report["observation"]["native"]["signatures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["verification"] == "verified"
                && !s["timestamps"].as_array().unwrap().is_empty())
    );
}

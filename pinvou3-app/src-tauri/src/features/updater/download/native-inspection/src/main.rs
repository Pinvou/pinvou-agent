use pinvou_upgrade_native_inspection::{Error, Limits, Role, RoleRequest, inspect, inspect_role};
use std::path::Path;
fn emit(value: &impl serde::Serialize) -> Result<(), Error> {
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer(&mut stdout, value).map_err(|_| Error::IoUnavailable)?;
    use std::io::Write;
    stdout
        .write_all(b"\n")
        .and_then(|_| stdout.flush())
        .map_err(|_| Error::IoUnavailable)
}
fn execute(args: &[String]) -> Result<(), Error> {
    if args.len() == 1 && args[0] == "help" {
        println!(
            "Explicit ordinary-cache observation; no install rights.\ninspect JOB MANIFEST_SHA256 ATTEMPT OUTPUT_DIRECTORY MEMBER LIMITS_JSON\nFullPack finalInstaller only; no execution, network or protected handoff.\nFailure can retain untrusted partial output. Successful output names the fresh ordinary cache file.\ninspect-execution-helper JOB MANIFEST_SHA256 ATTEMPT OUTPUT_DIRECTORY MEMBER LIMITS_JSON\ninspect-launcher JOB MANIFEST_SHA256 ATTEMPT OUTPUT_DIRECTORY MEMBER LIMITS_JSON\ninspect-helper-artifact JOB MANIFEST_SHA256 ATTEMPT OUTPUT_DIRECTORY HELPER_ID MEMBER LIMITS_JSON\nRole commands return a separate declaration-selection wrapper; all qualification flags remain false."
        );
        return Ok(());
    }
    let (role, member, limits_index) = match args.first().map(String::as_str) {
        Some("inspect") if args.len() == 7 => (None, 5, 6),
        Some("inspect-execution-helper") if args.len() == 7 => (Some(Role::ExecutionHelper), 5, 6),
        Some("inspect-launcher") if args.len() == 7 => (Some(Role::Launcher), 5, 6),
        Some("inspect-helper-artifact") if args.len() == 8 => {
            (Some(Role::HelperArtifact(&args[5])), 6, 7)
        }
        _ => return Err(Error::InvalidInput),
    };
    let attempt: u64 = args[3].parse().map_err(|_| Error::InvalidInput)?;
    if attempt == 0 || args[3] != attempt.to_string() {
        return Err(Error::InvalidInput);
    }
    let cancel = pinvou_upgrade_native_evidence::ConsoleCancellation::install().map_err(
        |error| match error {
            pinvou_upgrade_native_evidence::Error::Unsupported => Error::Unsupported,
            _ => Error::IoUnavailable,
        },
    )?;
    let raw = pinvou_upgrade_download_job::read_input(Path::new(&args[limits_index]), 4096)
        .map_err(|_| Error::InvalidInput)?;
    let limits: Limits = serde_json::from_str(&raw).map_err(|_| Error::InvalidInput)?;
    if let Some(role) = role {
        let observation = inspect_role(
            Path::new(&args[1]),
            &args[2],
            attempt,
            Path::new(&args[4]),
            RoleRequest {
                role,
                member_name: &args[member],
            },
            &limits,
            cancel.flag(),
        )?;
        return emit(&observation.report());
    }
    let observation = inspect(
        Path::new(&args[1]),
        &args[2],
        attempt,
        Path::new(&args[4]),
        &args[member],
        &limits,
        cancel.flag(),
    )?;
    emit(&observation.report())
}
fn main() {
    let args: Result<Vec<_>, _> = std::env::args_os()
        .skip(1)
        .map(|a| a.into_string().map_err(|_| Error::InvalidInput))
        .collect();
    if let Err(error) = args.and_then(|args| execute(&args)) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

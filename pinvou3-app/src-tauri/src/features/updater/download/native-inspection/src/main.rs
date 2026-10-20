use pinvou_upgrade_native_inspection::{Error, Limits, inspect};
use std::path::Path;
fn execute(args: &[String]) -> Result<(), Error> {
    if args.len() == 1 && args[0] == "help" {
        println!(
            "Explicit ordinary-cache observation; no install rights.\ninspect JOB MANIFEST_SHA256 ATTEMPT OUTPUT_DIRECTORY MEMBER LIMITS_JSON\nFullPack finalInstaller only; no execution, network or protected handoff.\nFailure can retain untrusted partial output. Successful output names the fresh ordinary cache file."
        );
        return Ok(());
    }
    if args.len() != 7 || args[0] != "inspect" {
        return Err(Error::InvalidInput);
    }
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
    let raw = pinvou_upgrade_download_job::read_input(Path::new(&args[6]), 4096)
        .map_err(|_| Error::InvalidInput)?;
    let limits: Limits = serde_json::from_str(&raw).map_err(|_| Error::InvalidInput)?;
    let observation = inspect(
        Path::new(&args[1]),
        &args[2],
        attempt,
        Path::new(&args[4]),
        &args[5],
        &limits,
        cancel.flag(),
    )?;
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer(&mut stdout, &observation.report()).map_err(|_| Error::IoUnavailable)?;
    use std::io::Write;
    stdout
        .write_all(b"\n")
        .and_then(|_| stdout.flush())
        .map_err(|_| Error::IoUnavailable)
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

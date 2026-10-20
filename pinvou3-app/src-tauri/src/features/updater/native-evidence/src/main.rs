use pinvou_upgrade_native_evidence::{ConsoleCancellation, Error, ExpectedFile, observe_path};
use std::{io::Write, path::PathBuf};
fn run() -> Result<(), Error> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "help" {
        println!(
            "Read-only embedded Authenticode evidence; no install or qualification rights.\ninspect FILE EXPECTED_SIZE EXPECTED_SHA256 [EXPECTED_IDENTITY_SHA256]\nCache-only strict revocation; no network fallback or certificate changes.\nExit 0 means a report was collected, including unsigned/refused/unknown results.\nVerified is one OS/provider signature observation, not V1 publisher or timestamp policy.\nCancellation joins the synchronous native call and closes its state before returning."
        );
        return Ok(());
    }
    if !matches!(args.len(), 4 | 5) || args[0] != "inspect" {
        return Err(Error::InvalidInput);
    }
    let size = args[2].to_str().ok_or(Error::InvalidInput)?;
    let parsed = size.parse::<u64>().map_err(|_| Error::InvalidInput)?;
    if parsed.to_string() != size {
        return Err(Error::InvalidInput);
    }
    let mut expected = ExpectedFile::new(parsed, args[3].to_str().ok_or(Error::InvalidInput)?)?;
    if let Some(value) = args.get(4) {
        expected = expected.with_identity_sha256(value.to_str().ok_or(Error::InvalidInput)?)?;
    }
    let cancel = ConsoleCancellation::install()?;
    let observed = observe_path(&PathBuf::from(&args[1]), &expected, cancel.flag())?;
    let bytes = serde_json::to_vec(observed.report()).map_err(|_| Error::EvidenceUnavailable)?;
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(&bytes)
        .and_then(|_| stdout.write_all(b"\n"))
        .and_then(|_| stdout.flush())
        .map_err(|_| Error::IoUnavailable)
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

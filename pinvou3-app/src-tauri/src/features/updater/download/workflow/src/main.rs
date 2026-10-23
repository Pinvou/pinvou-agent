//! Thin explicit QA/operator interface. No app install command, CA override,
//! updater namespace, credential logging or high-privilege execution is exposed.
use pinvou_upgrade_download_job::{
    ControlAttempt, Error, Import, Job, NetworkConsent, StopRequest, create, observe_retained,
    request_stop, run, run_control,
};
use std::path::Path;

fn read(path: &str, maximum: usize) -> Result<String, Error> {
    pinvou_upgrade_download_job::read_input(Path::new(path), maximum)
}
fn decode<T: serde::de::DeserializeOwned>(path: &str) -> Result<T, Error> {
    serde_json::from_str(&read(path, 16 * 1_048_576)?).map_err(|_| Error::InvalidInput)
}
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
    match args.first().map(String::as_str) {
        Some("create") if args.len() == 7 => {
            let import: Import = decode(&args[6])?;
            let pin = create(
                Path::new(&args[1]),
                read(&args[2], 1_048_576)?,
                args[3].clone(),
                read(&args[4], 1_048_576)?,
                read(&args[5], 1_048_576)?,
                &import,
            )?;
            emit(
                &serde_json::json!({"protocolVersion":1,"manifestSha256":pin,"currentQualificationChecked":false,"verifiedExecutionObject":false}),
            )
        }
        Some("status") if args.len() == 3 => {
            emit(&Job::open(Path::new(&args[1]), &args[2])?.snapshot())
        }
        Some("check") if args.len() == 3 => {
            emit(&Job::open(Path::new(&args[1]), &args[2])?.check(|| false)?)
        }
        Some("check-retained") if args.len() == 4 => {
            let observed = observe_retained(
                Path::new(&args[1]),
                &args[2],
                args[3].parse().map_err(|_| Error::InvalidInput)?,
                || false,
            )?;
            emit(observed.facts())
        }
        Some("pause" | "cancel") if args.len() == 4 => {
            request_stop(
                Path::new(&args[1]),
                &args[2],
                &args[3],
                if args[0] == "pause" {
                    StopRequest::Pause
                } else {
                    StopRequest::Cancel
                },
            )?;
            emit(
                &serde_json::json!({"protocolVersion":1,"requestStored":true,"writerJoined":false}),
            )
        }
        Some("run-control" | "restart-control") if args.len() == 5 => {
            let input: ControlAttempt = decode(&args[3])?;
            let consent: NetworkConsent = decode(&args[4])?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| Error::IoUnavailable)?;
            let result = runtime.block_on(run_control(
                args[1].clone().into(),
                args[2].clone(),
                input,
                consent,
                args[0] == "restart-control",
                |notice| emit(&notice),
            ))?;
            emit(&result)
        }
        Some("run" | "restart") if args.len() == 5 => {
            let import: Import = decode(&args[3])?;
            let consent: NetworkConsent = decode(&args[4])?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| Error::IoUnavailable)?;
            let result = runtime.block_on(run(
                args[1].clone().into(),
                args[2].clone(),
                import,
                consent,
                args[0] == "restart",
                |notice| emit(&notice),
            ))?;
            emit(&result)
        }
        Some("help") if args.len() == 1 => {
            println!(
                "Explicit QA cache tool; no current qualification or install rights.\ncreate JOB ROOT_JSON ROOT_SHA256 PACKAGE_JSON EXPECTED_JSON IMPORT_JSON\nstatus JOB MANIFEST_SHA256\ncheck JOB MANIFEST_SHA256\ncheck-retained JOB MANIFEST_SHA256 ATTEMPT_NUMBER\nrun|restart JOB MANIFEST_SHA256 IMPORT_JSON NETWORK_JSON\nrun-control|restart-control JOB MANIFEST_SHA256 CONTROL_JSON NETWORK_JSON\nFixed control service DownloadInfo POST; supplied time is not trusted time.\npause|cancel JOB MANIFEST_SHA256 RUN_ID\nNetwork use requires explicit exact host/port/address consent. No total download deadline.\nCancel/pause stores an attempt request; only the joined run result confirms stopped handles."
            );
            Ok(())
        }
        _ => Err(Error::InvalidInput),
    }
}
fn main() {
    let args: Result<Vec<String>, Error> = std::env::args_os()
        .skip(1)
        .map(|value| value.into_string().map_err(|_| Error::InvalidInput))
        .collect();
    if let Err(error) = args.and_then(|args| execute(&args)) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

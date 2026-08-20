use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use pinvou_host_supervisor_protocol::{ManagedHostWork, SupervisorRequest};

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("daemon") if std::env::args().len() == 2 => match pinvou_supervisor::run_daemon() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("pinvou-supervisor daemon failed: {error}");
                ExitCode::FAILURE
            }
        },
        Some("launch") if std::env::args().len() == 2 => {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            let request = SupervisorRequest::launch_pinvou_app(format!(
                "desktop-launch:{}:{timestamp}",
                std::process::id()
            ));
            match pinvou_supervisor::send_client_request(&request) {
                Ok(receipt)
                    if matches!(
                        receipt.outcome,
                        pinvou_host_supervisor_protocol::SupervisorOutcome::Applied
                            | pinvou_host_supervisor_protocol::SupervisorOutcome::AlreadyApplied
                            | pinvou_host_supervisor_protocol::SupervisorOutcome::Reconciled
                    ) =>
                {
                    ExitCode::SUCCESS
                }
                Ok(receipt) => {
                    eprintln!(
                        "pinvou-supervisor launch was not confirmed: {:?}: {}",
                        receipt.outcome, receipt.detail
                    );
                    ExitCode::FAILURE
                }
                Err(error) => {
                    eprintln!("pinvou-supervisor launch failed: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some(command @ ("status" | "snapshot-app" | "snapshot-asr"))
            if std::env::args().len() == 2 =>
        {
            let target = if command == "snapshot-asr" {
                ManagedHostWork::PinvouAsr
            } else {
                ManagedHostWork::PinvouApp
            };
            let request = SupervisorRequest::status(
                format!("{command}:{}:{:?}", std::process::id(), target),
                target,
            );
            match pinvou_supervisor::send_client_request(&request) {
                Ok(receipt) => match serde_json::to_string(&receipt) {
                    Ok(json) => {
                        println!("{json}");
                        ExitCode::SUCCESS
                    }
                    Err(error) => {
                        eprintln!("pinvou-supervisor status encode failed: {error}");
                        ExitCode::FAILURE
                    }
                },
                Err(error) => {
                    eprintln!("pinvou-supervisor status failed: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!("usage: pinvou-supervisor <daemon|launch|status|snapshot-app|snapshot-asr>");
            ExitCode::FAILURE
        }
    }
}

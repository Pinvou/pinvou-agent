//! Fixed App-only startup admission over the existing authenticated bounded
//! control connection. Public response facts are checked, never native inputs.
use super::client::Client;
use crate::{
    error::{Error, Result},
    handoff::Role,
    identity::ProcessSelector,
    ipc, json,
};

/// Only the actual authenticated constructor below can create this value. Keep
/// the connection, sealed image leases and exact response alive for App runtime.
pub(crate) struct AppWriterAdmission {
    _client: Client,
    _reply: ipc::WriterAdmissionReply,
}
impl AppWriterAdmission {
    pub(crate) fn recheck(&self) -> Result<()> {
        self._client.recheck()
    }
}

fn validate_reply(
    reply: &ipc::WriterAdmissionReply,
    current: &ProcessSelector,
    scope: &str,
    host_epoch: u64,
) -> Result<()> {
    reply.process.validate()?;
    if reply.installation_scope_id != scope
        || reply.host_epoch != host_epoch
        || reply.process != *current
        || !json::digest(&reply.participant.id)
        || !json::digest(&reply.participant.instance_sha256)
    {
        return Err(Error::PeerRefused);
    }
    Ok(())
}

pub(crate) fn admit_current_app_writer() -> Result<AppWriterAdmission> {
    admit_current_writer(Role::App)
}

pub(crate) fn admit_current_cli_writer() -> Result<AppWriterAdmission> {
    admit_current_writer(Role::Cli)
}
/// Classify only through the authenticated service's native child proof.
/// This is not itself a writer admission or a fallback authorization.
pub(crate) fn is_current_cli_worker() -> Result<bool> {
    let (_client, kind) = Client::connect_cli_entry()?;
    Ok(matches!(kind, super::client::CliConnectionKind::Worker))
}

fn admit_current_writer(role: Role) -> Result<AppWriterAdmission> {
    let mut client = Client::connect(role)?;
    // No external PID, path, user, role, write scope or source facts enter this
    // command. The service observes the actual pipe/process/token itself.
    let reply: ipc::WriterAdmissionReply = client.request("writer-admit", &ipc::Empty {})?;
    validate_reply(
        &reply,
        &client.current.selector,
        &client.record.installation_scope_id,
        client.record.host_epoch,
    )?;
    client.recheck()?;
    Ok(AppWriterAdmission {
        _client: client,
        _reply: reply,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::FileTime;

    fn reply() -> ipc::WriterAdmissionReply {
        // Pure wire DATA only. This cannot construct Client or native admission.
        ipc::WriterAdmissionReply {
            installation_scope_id: "a".repeat(64),
            host_epoch: 7,
            process: ProcessSelector {
                process_id: 123,
                creation_file_time: FileTime::from_native(1, 2),
            },
            participant: pinvou_upgrade_ownership::live::Participant {
                id: "b".repeat(64),
                instance_sha256: "c".repeat(64),
            },
        }
    }
    #[test]
    fn response_must_match_scope_epoch_and_exact_native_incarnation() {
        let expected = reply();
        assert!(
            validate_reply(
                &expected,
                &expected.process,
                &expected.installation_scope_id,
                7
            )
            .is_ok()
        );
        for field in 0..6 {
            let mut actual = reply();
            match field {
                0 => actual.installation_scope_id = "d".repeat(64),
                1 => actual.host_epoch = 8,
                2 => actual.process.process_id = 124,
                3 => actual.process.creation_file_time = FileTime::from_native(1, 3),
                4 => actual.participant.id = "not-a-domain-hash".into(),
                5 => actual.participant.instance_sha256 = "C".repeat(64),
                _ => unreachable!(),
            }
            assert!(
                validate_reply(
                    &actual,
                    &expected.process,
                    &expected.installation_scope_id,
                    7
                )
                .is_err()
            );
        }
    }
    #[test]
    fn admission_response_is_closed_and_request_has_no_selector_input() {
        let valid = json::encode(&reply(), true).unwrap();
        let mut value: serde_json::Value = json::decode(&valid, true).unwrap();
        value.as_object_mut().unwrap().insert(
            "censusDigest".into(),
            serde_json::Value::String("d".repeat(64)),
        );
        assert!(
            json::decode::<ipc::WriterAdmissionReply>(&json::encode(&value, true).unwrap(), true)
                .is_err()
        );
        assert_eq!(json::encode(&ipc::Empty {}, true).unwrap(), b"{}");
    }
}

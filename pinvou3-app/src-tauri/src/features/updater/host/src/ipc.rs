//! Closed bounded private control wire. It carries no native capability.
use crate::{
    error::{Error, Result},
    identity::FileTime,
    json,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
/// Maximum raw UTF-16 units in a CLI environment (32 MiB).
pub(crate) const CLI_ENV_MAX_UNITS: usize = 32 * 1024 * 1024 / 2;
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Hello {
    pub(crate) host_ipc_version: u8,
    pub(crate) kind: String,
    pub(crate) client_nonce: String,
    pub(crate) expected_registration_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) operation_id: Option<String>,
}
impl Hello {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self> {
        let value: Self = json::decode(bytes, true)?;
        // Distinguish ABSENT from explicit-null: both hello shapes are closed.
        let object: Value = json::decode(bytes, true)?;
        let object = object.as_object().ok_or(Error::IpcInvalid)?;
        if value.host_ipc_version != 1
            || !json::digest(&value.client_nonce)
            || !json::digest(&value.expected_registration_digest)
            || match value.kind.as_str() {
                "hello" => value.operation_id.is_some() || object.len() != 4,
                "bootstrap-hello" => {
                    !value
                        .operation_id
                        .as_ref()
                        .is_some_and(|id| json::digest(id))
                        || object.len() != 5
                }
                _ => true,
            }
        {
            return Err(Error::IpcInvalid);
        }
        Ok(value)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Request {
    pub(crate) host_ipc_version: u8,
    pub(crate) session_id: String,
    pub(crate) sequence: u64,
    pub(crate) kind: String,
    pub(crate) body: Value,
}
impl Request {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self> {
        let value: Self = json::decode(bytes, true)?;
        if value.host_ipc_version != 1
            || !json::digest(&value.session_id)
            || value.sequence == 0
            || value.sequence > json::SAFE_MAX
            || value.kind.is_empty()
            || value.kind.len() > 64
            || !value
                .kind
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b == b'-')
            || !value.body.is_object()
        {
            return Err(Error::IpcInvalid);
        }
        Ok(value)
    }
    pub(crate) fn body<T: DeserializeOwned>(&self) -> Result<T> {
        json::decode(&json::encode(&self.body, true)?, true)
    }
    pub(crate) fn body_digest(&self) -> Result<String> {
        Ok(json::sha256(&json::encode(&self.body, true)?))
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Success<'a, T: Serialize> {
    host_ipc_version: u8,
    session_id: &'a str,
    sequence: u64,
    ok: bool,
    kind: &'a str,
    body: T,
}
pub(crate) fn success<T: Serialize>(
    session: &str,
    sequence: u64,
    kind: &str,
    body: T,
) -> Result<Vec<u8>> {
    if !json::digest(session) || sequence > json::SAFE_MAX {
        return Err(Error::IpcInvalid);
    }
    json::encode(
        &Success {
            host_ipc_version: 1,
            session_id: session,
            sequence,
            ok: true,
            kind,
            body,
        },
        true,
    )
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Failure<'a> {
    host_ipc_version: u8,
    session_id: Option<&'a str>,
    sequence: Option<u64>,
    ok: bool,
    code: &'static str,
    retryable: bool,
    outcome: &'static str,
}
pub(crate) fn failure(
    session: Option<&str>,
    sequence: Option<u64>,
    error: Error,
) -> Result<Vec<u8>> {
    let unknown = matches!(
        error,
        Error::HandoffOutcomeUnknown | Error::StateCommitUnknown
    );
    json::encode(
        &Failure {
            host_ipc_version: 1,
            session_id: session,
            sequence,
            ok: false,
            code: error.code(),
            retryable: matches!(error, Error::Busy | Error::Unavailable),
            outcome: if unknown { "unknown" } else { "none" },
        },
        true,
    )
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Capabilities {
    pub(crate) status: bool,
    pub(crate) launch_registered_application: bool,
    pub(crate) writer_admission: bool,
    pub(crate) prepare: bool,
    pub(crate) freeze: bool,
    pub(crate) backup: bool,
    pub(crate) consume: bool,
    pub(crate) install: bool,
    pub(crate) activate: bool,
    pub(crate) helper_evolution: bool,
    pub(crate) protected_metadata_acceptance: bool,
    pub(crate) trusted_deadline: bool,
}
impl Capabilities {
    pub(crate) fn writer_only() -> Self {
        let mut value = Self::ready(false);
        value.status = false;
        value.writer_admission = true;
        value
    }
    pub(crate) fn ready(launch: bool) -> Self {
        Self {
            status: true,
            launch_registered_application: launch,
            writer_admission: launch,
            prepare: false,
            freeze: false,
            backup: false,
            consume: false,
            install: false,
            activate: false,
            helper_evolution: false,
            protected_metadata_acceptance: false,
            trusted_deadline: false,
        }
    }
    pub(crate) fn setup() -> Self {
        let mut value = Self::ready(false);
        value.status = false;
        value
    }
}
macro_rules! body {
    ($name:ident{$($field:ident:$ty:ty),*$(,)?})=>{
        #[derive(Serialize,Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]pub(crate) struct $name{$(pub(crate) $field:$ty),*}
    }
}
body!(Empty {});
body!(CliBegin {
    arguments: Vec<String>,
    cwd_utf16: Vec<u16>,
    environment_units: u32
});
body!(WriterAdmissionReply {
    installation_scope_id: String,
    host_epoch: u64,
    process: crate::identity::ProcessSelector,
    participant: pinvou_upgrade_ownership::live::Participant
});
body!(BeginBody {
    client_intent_id: String
});
body!(JoinBody {
    restart_id: String,
    client_intent_id: String
});
body!(ReadyBody {
    restart_id: String,
    wait_association_id: String,
    expected_generation: u64
});
body!(RestartBody { restart_id: String });
body!(ArmBody {
    restart_id: String,
    expected_generation: u64
});
body!(CommitBody {
    restart_id: String,
    expected_generation: u64,
    commit_id: String
});
body!(TakeBody {
    restart_id: String,
    expected_generation: u64,
    exit_commit_id: String
});
body!(ObservedBody {
    handoff_id: String,
    child_process_id: u32,
    child_creation_time: FileTime
});
body!(ProbeBody {
    operation_id: String,
    expected_revision: u64,
    challenge: String
});

/// Gate before any state-sensitive replay. Cached failures are immutable:
/// freed capacity or later readiness never turns an original refusal into IO.
pub(crate) fn cached_success(bytes: &[u8]) -> Result<bool> {
    let value: Value = json::decode(bytes, true)?;
    value
        .get("ok")
        .and_then(Value::as_bool)
        .ok_or(Error::IpcInvalid)
}

/// One original response only, per connection. Sensitive grant replies are
/// NEVER stored/replayed. Duplicate sequence cannot reenter business dispatch.
pub(crate) struct Sequencer {
    session: String,
    next: u64,
    last: Option<(u64, String, Option<Vec<u8>>)>,
}
pub(crate) enum Dispatch<'a> {
    New,
    Replay(&'a [u8]),
}
impl Sequencer {
    pub(crate) fn new(session: String) -> Result<Self> {
        if !json::digest(&session) {
            return Err(Error::IpcInvalid);
        }
        Ok(Self {
            session,
            next: 1,
            last: None,
        })
    }
    pub(crate) fn check<'a>(&'a self, request: &Request) -> Result<Dispatch<'a>> {
        if request.session_id != self.session {
            return Err(Error::IpcInvalid);
        }
        if let Some((sequence, digest, reply)) = &self.last {
            if request.sequence == *sequence {
                if &json::sha256(&json::encode(request, true)?) != digest {
                    return Err(Error::IpcConflict);
                }
                return reply
                    .as_ref()
                    .map(|reply| Dispatch::Replay(reply))
                    .ok_or(Error::HandoffOutcomeUnknown);
            }
        }
        if request.sequence != self.next || self.next >= json::SAFE_MAX {
            return Err(Error::IpcConflict);
        }
        Ok(Dispatch::New)
    }
    pub(crate) fn completed(
        &mut self,
        request: &Request,
        reply: Vec<u8>,
        contains_grant: bool,
    ) -> Result<()> {
        if request.sequence != self.next || self.next >= json::SAFE_MAX || reply.len() > 16 * 1024 {
            return Err(Error::IpcConflict);
        }
        self.last = Some((
            request.sequence,
            json::sha256(&json::encode(request, true)?),
            if contains_grant { None } else { Some(reply) },
        ));
        self.next += 1;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn wire(sequence: u64) -> Vec<u8> {
        format!("{{\"hostIpcVersion\":1,\"sessionId\":\"{}\",\"sequence\":{sequence},\"kind\":\"status\",\"body\":{{}}}}","1".repeat(64)).into_bytes()
    }
    #[test]
    fn exact_sequence_replay_never_reissues_a_grant() {
        let mut sequence = Sequencer::new("1".repeat(64)).unwrap();
        let bytes = wire(1);
        let request = Request::parse(&bytes).unwrap();
        assert!(matches!(sequence.check(&request), Ok(Dispatch::New)));
        sequence
            .completed(&request, b"ordinary".to_vec(), false)
            .unwrap();
        assert!(matches!(
            sequence.check(&request),
            Ok(Dispatch::Replay(b"ordinary"))
        ));
        let bytes = wire(2);
        let request = Request::parse(&bytes).unwrap();
        sequence
            .completed(&request, b"grant".to_vec(), true)
            .unwrap();
        assert!(matches!(
            sequence.check(&request),
            Err(Error::HandoffOutcomeUnknown)
        ));
    }
    #[test]
    fn explicit_null_and_unknown_hello_fields_never_create_a_session() {
        let ordinary = format!(
            "{{\"hostIpcVersion\":1,\"kind\":\"hello\",\"clientNonce\":\"{}\",\"expectedRegistrationDigest\":\"{}\"}}",
            "1".repeat(64),
            "2".repeat(64)
        );
        Hello::parse(ordinary.as_bytes()).unwrap();
        let bad = ordinary.replace('}', ",\"operationId\":null}");
        assert!(Hello::parse(bad.as_bytes()).is_err());
        assert!(Request::parse(&wire(0)).is_err());
        let req = Request::parse(&wire(1)).unwrap();
        assert!(req.body::<Empty>().is_ok());
        let bad = String::from_utf8(wire(1))
            .unwrap()
            .replace("\"body\":{}", "\"body\":{\"path\":\"x\"}");
        assert!(
            Request::parse(bad.as_bytes())
                .unwrap()
                .body::<Empty>()
                .is_err()
        );
    }
}

// Exact successful bodies shared by both native endpoints. None of these DTOs
// can construct a retained peer, registered image lease or installer context.
body!(HelloReply {
    profile_kind: String,
    profile_id: String,
    installation_scope_id: String,
    registration_revision: u64,
    registration_digest: String,
    host_epoch: u64,
    host_instance_nonce: String,
    client_nonce: String,
    server_nonce: String,
    capabilities: Capabilities
});
body!(BootstrapHelloReply {
    profile_kind: String,
    profile_id: String,
    installation_scope_id: String,
    operation_id: String,
    phase: crate::registration::Phase,
    registration_revision: u64,
    registration_digest: String,
    host_epoch: u64,
    host_instance_nonce: String,
    client_nonce: String,
    server_nonce: String,
    capabilities: Capabilities
});
body!(StatusReply {
    phase: crate::registration::Phase,
    helper_build_id: String,
    launcher_build_id: String,
    app_build_id: String,
    host_epoch: u64,
    registration_revision: u64,
    capabilities: Capabilities
});
body!(LaunchReply {
    handoff_id: String,
    image_binding_sha256: String,
    host_epoch: u64
});
body!(BeginReply {
    restart_id: String,
    generation: u64,
    host_epoch: u64
});
body!(JoinReply {
    wait_association_id: String,
    old_process_id: u32,
    old_process_creation_time: FileTime,
    old_image_binding_sha256: String,
    generation: u64,
    host_epoch: u64
});
body!(StateReply {
    restart_id: String,
    state: String,
    generation: u64,
    host_epoch: u64
});
body!(RestartStatusReply{restart_id:String,state:String,generation:u64,host_epoch:u64,old_process_exited:bool,exit_commit_id:Option<String>,remaining_exit_grace_ms:Option<u64>});
body!(CommitReply {
    restart_id: String,
    state: String,
    generation: u64,
    host_epoch: u64,
    exit_commit_id: String,
    remaining_exit_grace_ms: u64
});
body!(CancelReply {
    restart_id: String,
    state: String,
    generation: u64,
    host_epoch: u64,
    outcome: String
});
body!(TakeReply {
    restart_id: String,
    state: String,
    generation: u64,
    host_epoch: u64,
    handoff_id: String,
    image_binding_sha256: String
});
body!(ObservedReply {
    observation: String
});
body!(ProbeReply {
    operation_id: String,
    installation_scope_id: String,
    phase: crate::registration::Phase,
    registration_revision: u64,
    host_epoch: u64,
    instance_nonce: String,
    process_id: u32,
    creation_file_time: FileTime,
    helper_image_sha256: String,
    manifest_sha256: String,
    challenge: String,
    probe_state: String
});

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IncomingSuccess {
    host_ipc_version: u8,
    session_id: String,
    sequence: u64,
    ok: bool,
    kind: String,
    body: Value,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IncomingFailure {
    host_ipc_version: u8,
    session_id: Option<String>,
    sequence: Option<u64>,
    ok: bool,
    code: String,
    retryable: bool,
    outcome: String,
}
/// Parse a matching authenticated response. Native server authentication must
/// happen first; neither successful parsing nor a session string is authority.
pub(crate) fn response<T: DeserializeOwned>(
    bytes: &[u8],
    session: Option<&str>,
    sequence: u64,
    kind: &str,
) -> Result<(String, T)> {
    let object: Value = json::decode(bytes, true)?;
    match object.get("ok").and_then(Value::as_bool) {
        Some(true) => {
            let reply: IncomingSuccess = json::decode(bytes, true)?;
            if reply.host_ipc_version != 1
                || !reply.ok
                || !json::digest(&reply.session_id)
                || reply.sequence != sequence
                || session.is_some_and(|id| id != reply.session_id)
                || reply.kind != kind
                || !reply.body.is_object()
            {
                return Err(Error::IpcInvalid);
            }
            let value = json::decode(&json::encode(&reply.body, true)?, true)?;
            Ok((reply.session_id, value))
        }
        Some(false) => {
            let reply: IncomingFailure = json::decode(bytes, true)?;
            let matching = if sequence == 0 && session.is_none() {
                reply.sequence.is_none_or(|value| value == 0)
                    && reply.session_id.as_ref().is_none_or(|id| json::digest(id))
            } else {
                reply.sequence == Some(sequence) && reply.session_id.as_deref() == session
            };
            if reply.host_ipc_version != 1 || reply.ok || !matching {
                return Err(Error::IpcInvalid);
            }
            let error = Error::from_code(&reply.code).ok_or(Error::IpcInvalid)?;
            let unknown = matches!(
                error,
                Error::HandoffOutcomeUnknown | Error::StateCommitUnknown
            );
            if reply.retryable != matches!(error, Error::Busy | Error::Unavailable)
                || reply.outcome != if unknown { "unknown" } else { "none" }
            {
                return Err(Error::IpcInvalid);
            }
            Err(error)
        }
        _ => Err(Error::IpcInvalid),
    }
}
impl Capabilities {
    pub(crate) fn is_writer_only(&self) -> bool {
        !self.status
            && !self.launch_registered_application
            && self.writer_admission
            && !self.prepare
            && !self.freeze
            && !self.backup
            && !self.consume
            && !self.install
            && !self.activate
            && !self.helper_evolution
            && !self.protected_metadata_acceptance
            && !self.trusted_deadline
    }
    pub(crate) fn is_exact(&self, status: bool, launch: bool) -> bool {
        self.status == status
            && self.launch_registered_application == launch
            && self.writer_admission == (status && launch)
            && !self.prepare
            && !self.freeze
            && !self.backup
            && !self.consume
            && !self.install
            && !self.activate
            && !self.helper_evolution
            && !self.protected_metadata_acceptance
            && !self.trusted_deadline
    }
}
#[cfg(test)]
mod reply_tests {
    use super::*;
    #[test]
    fn cli_capability_is_writer_only_and_refuses_other_grants() {
        let mut capability = Capabilities::writer_only();
        assert!(capability.is_writer_only());
        capability.launch_registered_application = true;
        assert!(!capability.is_writer_only());
        let mut capability = Capabilities::writer_only();
        capability.install = true;
        assert!(!capability.is_writer_only());
        assert!(!Capabilities::ready(true).is_writer_only());
    }
    #[test]
    fn exact_authenticated_reply_fields_are_closed_and_errors_keep_unknown() {
        let session = "1".repeat(64);
        let body = BeginReply {
            restart_id: "2".repeat(64),
            generation: 1,
            host_epoch: 1,
        };
        let bytes = success(&session, 1, "begin-restart", body).unwrap();
        let (actual, parsed) =
            response::<BeginReply>(&bytes, Some(&session), 1, "begin-restart").unwrap();
        assert_eq!(actual, session);
        assert_eq!(parsed.generation, 1);
        for (sequence, kind) in [(2, "begin-restart"), (1, "begin-launch")] {
            assert!(response::<BeginReply>(&bytes, Some(&session), sequence, kind).is_err());
        }
        let mut value: Value = json::decode(&bytes, true).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("extra".into(), Value::Bool(false));
        assert!(
            response::<BeginReply>(
                &json::encode(&value, true).unwrap(),
                Some(&session),
                1,
                "begin-restart"
            )
            .is_err()
        );
        for error in [
            Error::Busy,
            Error::HandoffOutcomeUnknown,
            Error::StateCommitUnknown,
            Error::RestartCanceled,
        ] {
            let bytes = failure(Some(&session), Some(1), error).unwrap();
            assert!(
                matches!(response::<Empty>(&bytes,Some(&session),1,"status"),Err(actual) if actual==error)
            );
        }
        let initial = failure(None, None, Error::PeerRefused).unwrap();
        assert!(matches!(
            response::<Empty>(&initial, None, 0, "hello"),
            Err(Error::PeerRefused)
        ));
        assert!(matches!(
            response::<Empty>(&initial, Some(&session), 1, "status"),
            Err(Error::IpcInvalid)
        ));
        assert!(Capabilities::setup().is_exact(false, false));
        assert!(Capabilities::ready(true).is_exact(true, true));
    }
}

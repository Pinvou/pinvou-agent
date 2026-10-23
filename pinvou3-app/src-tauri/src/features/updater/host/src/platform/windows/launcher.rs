//! Ordinary interactive launcher. Fixed registered images only, never a command line port.
use super::{
    binding,
    client::Client,
    launch::{self, Target},
    layout::ordinal_equal,
    process::RetainedProcess,
};
use crate::{
    error::{Error, Result},
    handoff::Role,
    ipc, json,
};
use std::time::Duration;

fn binding_matches(client: &Client, claimed: &str) -> Result<()> {
    let actual = binding::image(
        &client.record,
        &client.app,
        &client.server.selector,
        &client.instance,
    )?;
    if claimed != actual {
        return Err(Error::ImageRefused);
    }
    Ok(())
}
fn observe(client: &mut Client, handoff: String, child: RetainedProcess) -> Result<()> {
    let reply: ipc::ObservedReply = client.request(
        "launch-observed",
        &ipc::ObservedBody {
            handoff_id: handoff,
            child_process_id: child.selector.process_id,
            child_creation_time: child.selector.creation_file_time,
        },
    )?;
    if !matches!(reply.observation.as_str(), "matched" | "unknown") {
        return Err(Error::IpcInvalid);
    }
    Ok(())
}
fn normal(mut client: Client) -> Result<()> {
    let intent = client.intent()?;
    let reply: ipc::LaunchReply = client.request(
        "begin-launch",
        &ipc::BeginBody {
            client_intent_id: intent,
        },
    )?;
    if !json::digest(&reply.handoff_id) || reply.host_epoch != client.record.host_epoch {
        return Err(Error::IpcInvalid);
    }
    binding_matches(&client, &reply.image_binding_sha256)?;
    // The only grant was consumed before the original server send. Neither a
    // failed native CreateProcess nor a lost observation is retried.
    let child = launch::spawn(&client, Target::App)?;
    observe(&mut client, reply.handoff_id, child)
}
struct Restart {
    client: Client,
    id: String,
    committed_take: bool,
}
impl Drop for Restart {
    fn drop(&mut self) {
        if !self.committed_take {
            let _: Result<ipc::CancelReply> = self.client.request(
                "restart-cancel",
                &ipc::RestartBody {
                    restart_id: self.id.clone(),
                },
            );
        }
    }
}
fn restart(mut client: Client, id: &str) -> Result<()> {
    let intent = client.intent()?;
    let joined: ipc::JoinReply = client.request(
        "join-restart",
        &ipc::JoinBody {
            restart_id: id.into(),
            client_intent_id: intent,
        },
    )?;
    if !json::digest(&joined.wait_association_id)
        || joined.generation == 0
        || joined.generation >= json::SAFE_MAX
        || joined.host_epoch != client.record.host_epoch
    {
        return Err(Error::IpcInvalid);
    }
    binding_matches(&client, &joined.old_image_binding_sha256)?;
    let old = RetainedProcess::open(joined.old_process_id, None)?;
    if old.selector.creation_file_time != joined.old_process_creation_time
        || old.token.user != client.current.token.user
        || !old.token.interactive
        || old.token.app_container
        || old.token.network
        || old.token.integrity != client.current.token.integrity
        || old.token.elevated != client.current.token.elevated
        || !ordinal_equal(
            old.image.to_str().ok_or(Error::ImageRefused)?,
            client
                .layout
                .app_path()
                .to_str()
                .ok_or(Error::ImageRefused)?,
        )?
        || old.selector.creation_file_time.ticks()
            >= client.current.selector.creation_file_time.ticks()
        || old.exited()?
    {
        return Err(Error::PeerRefused);
    }
    // Ready is sent only after the real held wait handle and exact old image,
    // token, creation identity and current owner were independently checked.
    let mut original = Restart {
        client,
        id: id.into(),
        committed_take: false,
    };
    let ready: ipc::StateReply = original.client.request(
        "restart-ready",
        &ipc::ReadyBody {
            restart_id: id.into(),
            wait_association_id: joined.wait_association_id,
            expected_generation: joined.generation,
        },
    )?;
    if ready.restart_id != id
        || ready.state != "ready"
        || ready.generation != joined.generation + 1
        || ready.host_epoch != original.client.record.host_epoch
    {
        return Err(Error::IpcInvalid);
    }
    loop {
        let current: ipc::RestartStatusReply = original.client.request(
            "restart-status",
            &ipc::RestartBody {
                restart_id: id.into(),
            },
        )?;
        if current.restart_id != id
            || current.host_epoch != original.client.record.host_epoch
            || current.generation < ready.generation
            || current.generation > json::SAFE_MAX
        {
            return Err(Error::IpcInvalid);
        }
        match current.state.as_str() {
            "ready" | "armed" => {
                if old.exited()? {
                    return Err(Error::RestartCanceled);
                }
            }
            "exit_committed" => {
                let commit = current.exit_commit_id.ok_or(Error::IpcInvalid)?;
                if !json::digest(&commit)
                    || !current
                        .remaining_exit_grace_ms
                        .is_some_and(|value| value > 0 && value <= 30_000)
                {
                    return Err(Error::RestartCanceled);
                }
                if old.exited()? {
                    let take: ipc::TakeReply = original.client.request(
                        "restart-take-launch",
                        &ipc::TakeBody {
                            restart_id: id.into(),
                            expected_generation: current.generation,
                            exit_commit_id: commit,
                        },
                    )?;
                    if take.restart_id != id
                        || take.state != "launch_released"
                        || take.generation != current.generation + 1
                        || take.host_epoch != original.client.record.host_epoch
                        || !json::digest(&take.handoff_id)
                    {
                        return Err(Error::IpcInvalid);
                    }
                    binding_matches(&original.client, &take.image_binding_sha256)?;
                    original.committed_take = true;
                    let child = launch::spawn(&original.client, Target::App)?;
                    return observe(&mut original.client, take.handoff_id, child);
                }
            }
            _ => return Err(Error::RestartCanceled),
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
pub(super) fn run(restart_id: Option<&str>) -> Result<()> {
    if restart_id.is_some_and(|id| !json::digest(id)) {
        return Err(Error::IpcInvalid);
    }
    let client = Client::connect(Role::Launcher)?;
    match restart_id {
        Some(id) => restart(client, id),
        None => normal(client),
    }
}

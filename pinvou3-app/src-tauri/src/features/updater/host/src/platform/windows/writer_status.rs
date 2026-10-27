//! Read-only fixed launcher route, usable without initializing GUI writers.
use super::client::Client;
use crate::{WriterJournalStatus, error::Result, handoff::Role, ipc};

pub(crate) fn inspect_writer_journal() -> Result<WriterJournalStatus> {
    let mut client = Client::connect(Role::Launcher)?;
    let status: WriterJournalStatus = client.request("writer-status", &ipc::Empty {})?;
    status.validate_for(
        &client.record.installation_scope_id,
        client.record.host_epoch,
    )?;
    client.recheck()?;
    Ok(status)
}

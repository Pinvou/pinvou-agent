//! Finite, incarnation-local ordinary launch/restart coordination.
//! Native roles/leases are supplied only by the private Windows adapter. No
//! request PID, image path, saved-data fact or installation capability exists.
use crate::{
    error::{Error, Result},
    identity::{LogonId, ProcessSelector},
    json,
};
use std::time::{Duration, Instant};
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PeerKey {
    pub(crate) process: ProcessSelector,
    pub(crate) user_sid: String,
    pub(crate) logon: LogonId,
    pub(crate) session_id: u32,
}
impl PeerKey {
    fn same_user(&self, other: &Self) -> bool {
        self.user_sid == other.user_sid
            && self.logon == other.logon
            && self.session_id == other.session_id
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    App,
    Cli,
    CliRequester,
    Launcher,
    SourceInstaller,
}
impl Role {
    pub(crate) fn is_writer(self) -> bool {
        matches!(self, Self::App | Self::Cli)
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    Reserved,
    Created,
    Joined,
    Ready,
    Armed,
    ExitCommitted,
    LaunchReleased,
    Observed,
    OutcomeUnknown,
    Canceled,
    Failed,
}
impl State {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::Created => "created",
            Self::Joined => "joined",
            Self::Ready => "ready",
            Self::Armed => "armed",
            Self::ExitCommitted => "exit_committed",
            Self::LaunchReleased => "launch_released",
            Self::Observed => "observed",
            Self::OutcomeUnknown => "outcome_unknown",
            Self::Canceled => "canceled",
            Self::Failed => "failed",
        }
    }
    fn live(self) -> bool {
        matches!(
            self,
            Self::Reserved
                | Self::Created
                | Self::Joined
                | Self::Ready
                | Self::Armed
                | Self::ExitCommitted
        )
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Launch,
    Restart,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConnectionId {
    slot: usize,
    incarnation: u64,
}
struct Link {
    connection: ConnectionId,
    peer: PeerKey,
    intent: String,
    body_sha256: String,
    connected: bool,
}
struct ExitCommit {
    id: String,
    body_sha256: String,
    end: Instant,
}
struct Cell<L> {
    incarnation: u64,
    kind: Kind,
    id: String,
    handoff_id: String,
    image_binding_sha256: String,
    generation: u64,
    state: State,
    original: Link,
    launcher: Option<Link>,
    wait_id: Option<String>,
    exit: Option<ExitCommit>,
    leases: Option<L>,
    failure: Option<Error>,
}
struct Connection {
    incarnation: u64,
    peer: PeerKey,
    role: Role,
    server_nonce: String,
    link: Option<usize>,
}
/// All storage has an absolute bound. There is no seen-id history or eviction.
pub(crate) struct Actions<L> {
    connections: Vec<Option<Connection>>,
    cells: Vec<Option<Cell<L>>>,
    next_connection_incarnation: u64,
    next_cell_incarnation: u64,
}
#[derive(Clone, Copy)]
pub(crate) struct Reservation {
    slot: usize,
    incarnation: u64,
    generation: u64,
}
pub(crate) struct Begin {
    pub(crate) id: String,
    pub(crate) generation: u64,
}
pub(crate) struct LaunchRelease {
    pub(crate) handoff_id: String,
    pub(crate) image_binding_sha256: String,
    pub(crate) generation: u64,
}
pub(crate) struct Joined {
    pub(crate) wait_id: String,
    pub(crate) original_process: ProcessSelector,
    pub(crate) image_binding_sha256: String,
    pub(crate) generation: u64,
}
pub(crate) struct Snapshot {
    pub(crate) state: State,
    pub(crate) generation: u64,
    pub(crate) exit_commit_id: Option<String>,
    pub(crate) remaining_exit_grace_ms: Option<u64>,
}
impl<L> Actions<L> {
    pub(crate) fn new() -> Self {
        Self {
            connections: (0..64).map(|_| None).collect(),
            cells: (0..32).map(|_| None).collect(),
            next_connection_incarnation: 1,
            next_cell_incarnation: 1,
        }
    }
    pub(crate) fn connect(
        &mut self,
        peer: PeerKey,
        role: Role,
        server_nonce: String,
    ) -> Result<ConnectionId> {
        if !json::digest(&server_nonce) {
            return Err(Error::IpcInvalid);
        }
        let index = self
            .connections
            .iter()
            .position(Option::is_none)
            .ok_or(Error::Busy)?;
        let incarnation = self.next_connection_incarnation;
        self.next_connection_incarnation = incarnation.checked_add(1).ok_or(Error::Busy)?;
        self.connections[index] = Some(Connection {
            incarnation,
            peer,
            role,
            server_nonce,
            link: None,
        });
        Ok(ConnectionId {
            slot: index,
            incarnation,
        })
    }
    fn connection(&self, id: ConnectionId) -> Result<&Connection> {
        self.connections
            .get(id.slot)
            .and_then(Option::as_ref)
            .filter(|c| c.incarnation == id.incarnation)
            .ok_or(Error::HandoffOutcomeUnknown)
    }
    fn cell_mut(&mut self, index: usize) -> Result<&mut Cell<L>> {
        self.cells
            .get_mut(index)
            .and_then(Option::as_mut)
            .ok_or(Error::HandoffOutcomeUnknown)
    }
    fn valid_intent(connection: &Connection, intent: &str) -> bool {
        intent.len() == 97
            && intent.as_bytes().get(64) == Some(&b'-')
            && intent.starts_with(&connection.server_nonce)
            && intent.as_bytes()[65..]
                .iter()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
    }
    fn reserve(
        &mut self,
        connection_id: ConnectionId,
        kind: Kind,
        intent: &str,
        body_sha256: &str,
        id: String,
        handoff_id: String,
    ) -> Result<Reservation> {
        let connection = self.connection(connection_id)?;
        if connection.role
            != if kind == Kind::Restart {
                Role::App
            } else {
                Role::Launcher
            }
        {
            return Err(Error::PeerRefused);
        }
        if !Self::valid_intent(connection, intent)
            || !json::digest(body_sha256)
            || !json::digest(&id)
            || !json::digest(&handoff_id)
        {
            return Err(Error::IpcInvalid);
        }
        if let Some(slot) = connection.link {
            let cell = self.cells[slot]
                .as_ref()
                .ok_or(Error::HandoffOutcomeUnknown)?;
            if cell.kind != kind
                || cell.original.intent != intent
                || cell.original.body_sha256 != body_sha256
            {
                return Err(if cell.original.intent == intent {
                    if kind == Kind::Restart {
                        Error::RestartConflict
                    } else {
                        Error::IpcConflict
                    }
                } else {
                    Error::Busy
                });
            }
            if let Some(error) = cell.failure {
                return Err(error);
            }
            // Normal grants are never replayed. Restart replay is handled by
            // begin_restart_replay without reacquiring any native resources.
            return Err(Error::HandoffOutcomeUnknown);
        }
        if self.cells.iter().flatten().any(|cell| {
            cell.original.peer == connection.peer
                || cell
                    .launcher
                    .as_ref()
                    .is_some_and(|l| l.peer == connection.peer)
        }) {
            return Err(Error::Busy);
        }
        let slot = self
            .cells
            .iter()
            .position(Option::is_none)
            .ok_or(Error::Busy)?;
        let link = Link {
            connection: connection_id,
            peer: connection.peer.clone(),
            intent: intent.into(),
            body_sha256: body_sha256.into(),
            connected: true,
        };
        let incarnation = self.next_cell_incarnation;
        self.next_cell_incarnation = incarnation.checked_add(1).ok_or(Error::Busy)?;
        self.cells[slot] = Some(Cell {
            incarnation,
            kind,
            id,
            handoff_id,
            image_binding_sha256: String::new(),
            generation: 1,
            state: State::Reserved,
            original: link,
            launcher: None,
            wait_id: None,
            exit: None,
            leases: None,
            failure: None,
        });
        self.connections[connection_id.slot]
            .as_mut()
            .ok_or(Error::HandoffOutcomeUnknown)?
            .link = Some(slot);
        Ok(Reservation {
            slot,
            incarnation,
            generation: 1,
        })
    }
    pub(crate) fn reserve_launch(
        &mut self,
        connection: ConnectionId,
        intent: &str,
        body_sha256: &str,
        id: String,
    ) -> Result<Reservation> {
        self.reserve(
            connection,
            Kind::Launch,
            intent,
            body_sha256,
            id.clone(),
            id,
        )
    }
    pub(crate) fn reserve_restart(
        &mut self,
        connection: ConnectionId,
        intent: &str,
        body_sha256: &str,
        id: String,
        handoff_id: String,
    ) -> Result<Reservation> {
        self.reserve(
            connection,
            Kind::Restart,
            intent,
            body_sha256,
            id,
            handoff_id,
        )
    }
    pub(crate) fn fail_reservation(&mut self, reservation: Reservation, error: Error) {
        if let Ok(cell) = self.cell_mut(reservation.slot) {
            if cell.incarnation == reservation.incarnation
                && cell.state == State::Reserved
                && cell.generation == reservation.generation
            {
                cell.state = State::Failed;
                cell.failure = Some(error);
                cell.leases = None;
            }
        }
    }
    pub(crate) fn finish_restart(
        &mut self,
        reservation: Reservation,
        leases: L,
        binding: String,
    ) -> Result<Begin> {
        if !json::digest(&binding) {
            return Err(Error::ImageRefused);
        }
        let cell = self.cell_mut(reservation.slot)?;
        if cell.kind != Kind::Restart
            || cell.incarnation != reservation.incarnation
            || cell.state != State::Reserved
            || cell.generation != reservation.generation
            || !cell.original.connected
        {
            return Err(Error::RestartCanceled);
        }
        cell.leases = Some(leases);
        cell.image_binding_sha256 = binding;
        cell.state = State::Created;
        Ok(Begin {
            id: cell.id.clone(),
            generation: cell.generation,
        })
    }
    pub(crate) fn finish_launch(
        &mut self,
        reservation: Reservation,
        leases: L,
        binding: String,
    ) -> Result<LaunchRelease> {
        if !json::digest(&binding) {
            return Err(Error::ImageRefused);
        }
        let cell = self.cell_mut(reservation.slot)?;
        if cell.kind != Kind::Launch
            || cell.incarnation != reservation.incarnation
            || cell.state != State::Reserved
            || cell.generation != reservation.generation
            || !cell.original.connected
        {
            return Err(Error::RestartCanceled);
        }
        cell.leases = Some(leases);
        cell.image_binding_sha256 = binding;
        // Released before the first send attempt. Only fixed non-grant replay
        // remains; the launcher already owns its independent native image lease.
        cell.state = State::LaunchReleased;
        cell.leases = None;
        Ok(LaunchRelease {
            handoff_id: cell.handoff_id.clone(),
            image_binding_sha256: cell.image_binding_sha256.clone(),
            generation: cell.generation,
        })
    }
    pub(crate) fn begin_restart_replay(
        &self,
        connection: ConnectionId,
        intent: &str,
        body_sha256: &str,
    ) -> Result<Option<Begin>> {
        let connection = self.connection(connection)?;
        let Some(slot) = connection.link else {
            return Ok(None);
        };
        let cell = self.cells[slot]
            .as_ref()
            .ok_or(Error::HandoffOutcomeUnknown)?;
        if cell.kind != Kind::Restart
            || cell.original.peer != connection.peer
            || cell.original.intent != intent
            || cell.original.body_sha256 != body_sha256
        {
            return Err(Error::RestartConflict);
        }
        if cell.state.live() && cell.state != State::Reserved {
            Ok(Some(Begin {
                id: cell.id.clone(),
                generation: cell.generation,
            }))
        } else {
            Err(Error::RestartCanceled)
        }
    }
    pub(crate) fn join(
        &mut self,
        connection_id: ConnectionId,
        restart_id: &str,
        intent: &str,
        body_sha256: &str,
        wait_id: String,
    ) -> Result<Joined> {
        let connection = self.connection(connection_id)?;
        if connection.role != Role::Launcher
            || !Self::valid_intent(connection, intent)
            || !json::digest(body_sha256)
            || !json::digest(&wait_id)
        {
            return Err(Error::PeerRefused);
        }
        if let Some(slot) = connection.link {
            let cell = self.cells[slot]
                .as_ref()
                .ok_or(Error::HandoffOutcomeUnknown)?;
            if cell.id != restart_id {
                return Err(Error::Busy);
            }
            let link = cell.launcher.as_ref().ok_or(Error::RestartConflict)?;
            if link.intent != intent || link.body_sha256 != body_sha256 {
                return Err(Error::RestartConflict);
            }
            if !cell.state.live() {
                return Err(Error::RestartCanceled);
            }
            return Ok(Joined {
                wait_id: cell.wait_id.clone().ok_or(Error::RestartConflict)?,
                original_process: cell.original.peer.process.clone(),
                image_binding_sha256: cell.image_binding_sha256.clone(),
                generation: cell.generation,
            });
        }
        if self.cells.iter().flatten().any(|cell| {
            cell.original.peer == connection.peer
                || cell
                    .launcher
                    .as_ref()
                    .is_some_and(|l| l.peer == connection.peer)
        }) {
            return Err(Error::Busy);
        }
        let slot = self
            .cells
            .iter()
            .position(|cell| cell.as_ref().is_some_and(|c| c.id == restart_id))
            .ok_or(Error::HandoffOutcomeUnknown)?;
        let cell = self.cells[slot]
            .as_ref()
            .ok_or(Error::HandoffOutcomeUnknown)?;
        if cell.kind != Kind::Restart
            || cell.state != State::Created
            || !cell.original.connected
            || !cell.original.peer.same_user(&connection.peer)
        {
            return Err(Error::RestartNotReady);
        }
        let peer = connection.peer.clone();
        let cell = self.cell_mut(slot)?;
        bump(cell)?;
        cell.launcher = Some(Link {
            connection: connection_id,
            peer,
            intent: intent.into(),
            body_sha256: body_sha256.into(),
            connected: true,
        });
        cell.wait_id = Some(wait_id.clone());
        cell.state = State::Joined;
        let response = Joined {
            wait_id,
            original_process: cell.original.peer.process.clone(),
            image_binding_sha256: cell.image_binding_sha256.clone(),
            generation: cell.generation,
        };
        self.connections[connection_id.slot]
            .as_mut()
            .ok_or(Error::HandoffOutcomeUnknown)?
            .link = Some(slot);
        Ok(response)
    }
    fn locate(
        &self,
        connection_id: ConnectionId,
        restart_id: &str,
        reduction: bool,
    ) -> Result<usize> {
        let connection = self.connection(connection_id)?;
        let slot = self
            .cells
            .iter()
            .position(|cell| {
                cell.as_ref()
                    .is_some_and(|c| c.id == restart_id && c.kind == Kind::Restart)
            })
            .ok_or(Error::HandoffOutcomeUnknown)?;
        let cell = self.cells[slot]
            .as_ref()
            .ok_or(Error::HandoffOutcomeUnknown)?;
        if cell.original.connection == connection_id && cell.original.connected
            || cell
                .launcher
                .as_ref()
                .is_some_and(|l| l.connection == connection_id && l.connected)
            || reduction && connection.role == Role::App && cell.original.peer == connection.peer
        {
            Ok(slot)
        } else {
            Err(Error::PeerRefused)
        }
    }
    pub(crate) fn ready(
        &mut self,
        connection: ConnectionId,
        id: &str,
        wait_id: &str,
        generation: u64,
    ) -> Result<Snapshot> {
        let slot = self.locate(connection, id, false)?;
        let cell = self.cell_mut(slot)?;
        if !cell
            .launcher
            .as_ref()
            .is_some_and(|l| l.connection == connection && l.connected)
            || cell.wait_id.as_deref() != Some(wait_id)
            || cell.state != State::Joined
            || cell.generation != generation
        {
            return Err(Error::RestartConflict);
        }
        bump(cell)?;
        cell.state = State::Ready;
        Ok(snapshot(cell, Instant::now()))
    }
    pub(crate) fn arm(
        &mut self,
        connection: ConnectionId,
        id: &str,
        generation: u64,
    ) -> Result<Snapshot> {
        let slot = self.locate(connection, id, false)?;
        let cell = self.cell_mut(slot)?;
        if cell.original.connection != connection
            || !cell.original.connected
            || cell.state != State::Ready
            || cell.generation != generation
            || !cell.launcher.as_ref().is_some_and(|l| l.connected)
        {
            return Err(Error::RestartNotReady);
        }
        bump(cell)?;
        cell.state = State::Armed;
        Ok(snapshot(cell, Instant::now()))
    }
    pub(crate) fn commit_exit(
        &mut self,
        connection: ConnectionId,
        id: &str,
        generation: u64,
        commit_id: &str,
        body_sha256: &str,
        now: Instant,
    ) -> Result<Snapshot> {
        if !json::digest(commit_id) || !json::digest(body_sha256) {
            return Err(Error::IpcInvalid);
        }
        let slot = self.locate(connection, id, false)?;
        let cell = self.cell_mut(slot)?;
        if cell.original.connection != connection || !cell.original.connected {
            return Err(Error::PeerRefused);
        }
        expire(cell, now)?;
        // Locate the exact original request before checking its old generation.
        if let Some(exit) = &cell.exit {
            if exit.id != commit_id || exit.body_sha256 != body_sha256 {
                return Err(Error::RestartConflict);
            }
            return if cell.state == State::ExitCommitted {
                Ok(snapshot(cell, now))
            } else {
                Err(Error::RestartCanceled)
            };
        }
        if cell.state != State::Armed
            || cell.generation != generation
            || !cell.launcher.as_ref().is_some_and(|l| l.connected)
        {
            return Err(Error::RestartNotReady);
        }
        bump(cell)?;
        cell.state = State::ExitCommitted;
        cell.exit = Some(ExitCommit {
            id: commit_id.into(),
            body_sha256: body_sha256.into(),
            end: now + Duration::from_secs(30),
        });
        Ok(snapshot(cell, now))
    }
    pub(crate) fn status(
        &mut self,
        connection: ConnectionId,
        id: &str,
        now: Instant,
    ) -> Result<Snapshot> {
        let slot = self.locate(connection, id, true)?;
        let cell = self.cell_mut(slot)?;
        expire(cell, now)?;
        Ok(snapshot(cell, now))
    }
    pub(crate) fn cancel(
        &mut self,
        connection: ConnectionId,
        id: &str,
        now: Instant,
    ) -> Result<Snapshot> {
        let slot = self.locate(connection, id, true)?;
        let cell = self.cell_mut(slot)?;
        if cell.state.live() {
            terminal(cell, State::Canceled)?;
        }
        Ok(snapshot(cell, now))
    }
    /// Adapter has just checked SAME held old process signaled, live launcher
    /// and registered image/current host. These booleans never come from IPC.
    pub(crate) fn take(
        &mut self,
        connection: ConnectionId,
        id: &str,
        generation: u64,
        commit_id: &str,
        old_process_signaled: bool,
        now: Instant,
    ) -> Result<LaunchRelease> {
        let slot = self.locate(connection, id, false)?;
        let cell = self.cell_mut(slot)?;
        expire(cell, now)?;
        if cell.state == State::LaunchReleased
            || cell.state == State::Observed
            || cell.state == State::OutcomeUnknown
        {
            return Err(Error::HandoffOutcomeUnknown);
        }
        if cell.state != State::ExitCommitted
            || cell.generation != generation
            || !old_process_signaled
            || cell.exit.as_ref().is_none_or(|e| e.id != commit_id)
            || !cell
                .launcher
                .as_ref()
                .is_some_and(|l| l.connection == connection && l.connected)
        {
            return Err(Error::RestartNotReady);
        }
        bump(cell)?;
        cell.state = State::LaunchReleased;
        cell.leases = None;
        Ok(LaunchRelease {
            handoff_id: cell.handoff_id.clone(),
            image_binding_sha256: cell.image_binding_sha256.clone(),
            generation: cell.generation,
        })
    }
    pub(crate) fn with_leases<T>(
        &self,
        connection: ConnectionId,
        id: &str,
        f: impl FnOnce(&L) -> Result<T>,
    ) -> Result<T> {
        let slot = self.locate(connection, id, false)?;
        let cell = self.cells[slot]
            .as_ref()
            .ok_or(Error::HandoffOutcomeUnknown)?;
        f(cell.leases.as_ref().ok_or(Error::RestartCanceled)?)
    }
    /// Native adapter attaches the held authenticated launcher only through
    /// the actual join connection, never a caller-supplied process selector.
    pub(crate) fn with_launcher_leases_mut<T>(
        &mut self,
        connection: ConnectionId,
        id: &str,
        f: impl FnOnce(&mut L) -> Result<T>,
    ) -> Result<T> {
        let slot = self.locate(connection, id, false)?;
        let cell = self.cell_mut(slot)?;
        if !cell
            .launcher
            .as_ref()
            .is_some_and(|link| link.connection == connection && link.connected)
        {
            return Err(Error::PeerRefused);
        }
        f(cell.leases.as_mut().ok_or(Error::RestartCanceled)?)
    }
    pub(crate) fn disconnect(&mut self, connection: ConnectionId) -> Result<()> {
        let Some(slot) = self.connections.get_mut(connection.slot) else {
            return Ok(());
        };
        if slot
            .as_ref()
            .is_none_or(|c| c.incarnation != connection.incarnation)
        {
            return Ok(());
        }
        let value = slot.take().ok_or(Error::HandoffOutcomeUnknown)?;
        if let Some(slot) = value.link {
            let cell = self.cell_mut(slot)?;
            let is_original = cell.original.connection == connection;
            if is_original {
                cell.original.connected = false;
            }
            if let Some(launcher) = &mut cell.launcher {
                if launcher.connection == connection {
                    launcher.connected = false;
                }
            }
            if cell.state.live() && !(is_original && cell.state == State::ExitCommitted) {
                terminal(cell, State::Canceled)?;
            } else if matches!(cell.state, State::LaunchReleased) {
                terminal(cell, State::OutcomeUnknown)?;
            }
            if !cell.original.connected && cell.launcher.as_ref().is_none_or(|l| !l.connected) {
                self.cells[slot] = None;
            }
        }
        Ok(())
    }
    pub(crate) fn original_process(
        &self,
        connection: ConnectionId,
        id: &str,
    ) -> Result<ProcessSelector> {
        let slot = self.locate(connection, id, true)?;
        Ok(self.cells[slot]
            .as_ref()
            .ok_or(Error::HandoffOutcomeUnknown)?
            .original
            .peer
            .process
            .clone())
    }
    pub(crate) fn observed(
        &mut self,
        connection: ConnectionId,
        handoff_id: &str,
        matched: bool,
    ) -> Result<()> {
        let current = self.connection(connection)?;
        if current.role != Role::Launcher {
            return Err(Error::PeerRefused);
        }
        let slot = current.link.ok_or(Error::HandoffOutcomeUnknown)?;
        let cell = self.cell_mut(slot)?;
        if cell.handoff_id != handoff_id || cell.state != State::LaunchReleased {
            return Err(Error::HandoffOutcomeUnknown);
        }
        if cell.original.connection != connection
            && !cell
                .launcher
                .as_ref()
                .is_some_and(|link| link.connection == connection && link.connected)
        {
            return Err(Error::PeerRefused);
        }
        terminal(
            cell,
            if matched {
                State::Observed
            } else {
                State::OutcomeUnknown
            },
        )
    }
    pub(crate) fn reap_expired(&mut self, now: Instant) -> Result<()> {
        for cell in self.cells.iter_mut().flatten() {
            expire(cell, now)?;
        }
        Ok(())
    }
}
fn bump<L>(cell: &mut Cell<L>) -> Result<()> {
    cell.generation = cell
        .generation
        .checked_add(1)
        .filter(|n| *n <= json::SAFE_MAX)
        .ok_or(Error::RestartConflict)?;
    Ok(())
}
fn terminal<L>(cell: &mut Cell<L>, state: State) -> Result<()> {
    bump(cell)?;
    cell.state = state;
    cell.leases = None;
    Ok(())
}
fn expire<L>(cell: &mut Cell<L>, now: Instant) -> Result<()> {
    if cell.state == State::ExitCommitted && cell.exit.as_ref().is_some_and(|e| now >= e.end) {
        terminal(cell, State::Canceled)?;
    }
    Ok(())
}
fn snapshot<L>(cell: &Cell<L>, now: Instant) -> Snapshot {
    Snapshot {
        state: cell.state,
        generation: cell.generation,
        exit_commit_id: cell.exit.as_ref().map(|e| e.id.clone()),
        remaining_exit_grace_ms: cell
            .exit
            .as_ref()
            .map(|e| e.end.saturating_duration_since(now).as_millis().min(30_000) as u64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::FileTime;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Lease(Arc<AtomicUsize>);
    impl Drop for Lease {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    fn peer(id: u32) -> PeerKey {
        PeerKey {
            process: ProcessSelector {
                process_id: id,
                creation_file_time: FileTime::from_native(1, id),
            },
            user_sid: "S-1-5-21-1".into(),
            logon: LogonId { high: 0, low: 9 },
            session_id: 1,
        }
    }
    fn hex(i: usize) -> String {
        format!("{i:064x}")
    }
    fn connect(actions: &mut Actions<Lease>, id: u32, role: Role) -> ConnectionId {
        actions
            .connect(peer(id), role, hex(id as usize + 1000))
            .unwrap()
    }
    fn intent(id: u32) -> String {
        format!("{}-{}", hex(id as usize + 1000), "1".repeat(32))
    }
    fn lease(count: &Arc<AtomicUsize>) -> Lease {
        count.fetch_add(1, Ordering::SeqCst);
        Lease(count.clone())
    }
    #[test]
    fn admission_is_bounded_before_any_native_lease_and_has_no_eviction() {
        let mut actions = Actions::<Lease>::new();
        let count = Arc::new(AtomicUsize::new(0));
        let mut first_connection = None;
        for id in 1..=32 {
            let connection = connect(&mut actions, id, Role::App);
            if first_connection.is_none() {
                first_connection = Some(connection);
            }
            let reservation = actions
                .reserve_restart(
                    connection,
                    &intent(id),
                    &hex(id as usize),
                    hex(id as usize),
                    hex(id as usize + 50),
                )
                .unwrap();
            actions
                .finish_restart(reservation, lease(&count), hex(80))
                .unwrap();
            assert!(matches!(
                actions.reserve_restart(
                    connection,
                    &format!("{}-{}", hex(id as usize + 1000), "2".repeat(32)),
                    &hex(3),
                    hex(4),
                    hex(5)
                ),
                Err(Error::Busy)
            ));
        }
        assert_eq!(count.load(Ordering::SeqCst), 32);
        let connection = connect(&mut actions, 33, Role::App);
        assert!(matches!(
            actions.reserve_restart(connection, &intent(33), &hex(33), hex(33), hex(83)),
            Err(Error::Busy)
        ));
        assert_eq!(count.load(Ordering::SeqCst), 32);
        // A second connection cannot bypass actual process ownership.
        let duplicate = connect(&mut actions, 1, Role::App);
        assert!(matches!(
            actions.reserve_restart(duplicate, &intent(1), &hex(1), hex(99), hex(199)),
            Err(Error::Busy)
        ));
        actions.disconnect(first_connection.unwrap()).unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 31);
        let reservation = actions
            .reserve_restart(connection, &intent(33), &hex(33), hex(33), hex(83))
            .unwrap();
        actions
            .finish_restart(reservation, lease(&count), hex(80))
            .unwrap();
        drop(actions);
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn cached_busy_stays_busy_after_capacity_frees_and_only_new_sequence_can_reserve() {
        use crate::ipc;
        let mut actions = Actions::<Lease>::new();
        let count = Arc::new(AtomicUsize::new(0));
        let mut first = None;
        for id in 1..=32 {
            let connection = connect(&mut actions, id, Role::App);
            if first.is_none() {
                first = Some(connection);
            }
            let reservation = actions
                .reserve_restart(
                    connection,
                    &intent(id),
                    &hex(id as usize),
                    hex(id as usize),
                    hex(id as usize + 50),
                )
                .unwrap();
            actions
                .finish_restart(reservation, lease(&count), hex(80))
                .unwrap();
        }
        let caller = connect(&mut actions, 33, Role::App);
        let session = hex(99);
        let mut sequence = ipc::Sequencer::new(session.clone()).unwrap();
        let mut request = ipc::Request {
            host_ipc_version: 1,
            session_id: session.clone(),
            sequence: 1,
            kind: "begin-restart".into(),
            body: serde_json::json!({"clientIntentId":intent(33)}),
        };
        let digest = request.body_digest().unwrap();
        assert!(matches!(
            actions.reserve_restart(caller, &intent(33), &digest, hex(33), hex(83)),
            Err(Error::Busy)
        ));
        let refusal = ipc::failure(Some(&session), Some(1), Error::Busy).unwrap();
        sequence
            .completed(&request, refusal.clone(), false)
            .unwrap();
        actions.disconnect(first.unwrap()).unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 31);
        let ipc::Dispatch::Replay(cached) = sequence.check(&request).unwrap() else {
            panic!("original sequence not replayed")
        };
        assert!(!ipc::cached_success(cached).unwrap());
        assert_eq!(cached, refusal);
        assert!(
            actions
                .begin_restart_replay(caller, &intent(33), &digest)
                .unwrap()
                .is_none()
        );
        assert_eq!(count.load(Ordering::SeqCst), 31);
        request.sequence = 2;
        assert!(matches!(sequence.check(&request), Ok(ipc::Dispatch::New)));
        let reservation = actions
            .reserve_restart(caller, &intent(33), &digest, hex(33), hex(83))
            .unwrap();
        actions
            .finish_restart(reservation, lease(&count), hex(80))
            .unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 32);
        drop(actions);
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn normal_grant_is_released_once_and_terminal_connection_cannot_start_again() {
        let mut actions = Actions::<Lease>::new();
        let count = Arc::new(AtomicUsize::new(0));
        let c = connect(&mut actions, 1, Role::Launcher);
        let reservation = actions
            .reserve_launch(c, &intent(1), &hex(5), hex(6))
            .unwrap();
        let release = actions
            .finish_launch(reservation, lease(&count), hex(7))
            .unwrap();
        assert_eq!(release.handoff_id, hex(6));
        assert_eq!(count.load(Ordering::SeqCst), 0);
        assert!(matches!(
            actions.reserve_launch(c, &intent(1), &hex(5), hex(6)),
            Err(Error::HandoffOutcomeUnknown)
        ));
        let another = format!("{}-{}", hex(1001), "2".repeat(32));
        assert!(matches!(
            actions.reserve_launch(c, &another, &hex(8), hex(9)),
            Err(Error::Busy)
        ));
        actions.disconnect(c).unwrap();
        assert_eq!(actions.cells.iter().flatten().count(), 0);
    }
    #[test]
    fn restart_requires_explicit_ready_arm_commit_real_signal_and_original_grace() {
        let mut actions = Actions::<Lease>::new();
        let count = Arc::new(AtomicUsize::new(0));
        let app = connect(&mut actions, 1, Role::App);
        let launcher = connect(&mut actions, 2, Role::Launcher);
        let reservation = actions
            .reserve_restart(app, &intent(1), &hex(1), hex(3), hex(4))
            .unwrap();
        actions
            .finish_restart(reservation, lease(&count), hex(5))
            .unwrap();
        assert!(matches!(
            actions.arm(app, &hex(3), 1),
            Err(Error::RestartNotReady)
        ));
        let joined = actions
            .join(launcher, &hex(3), &intent(2), &hex(6), hex(7))
            .unwrap();
        assert_eq!(joined.original_process.process_id, 1);
        let ready = actions
            .ready(launcher, &hex(3), &hex(7), joined.generation)
            .unwrap();
        let armed = actions.arm(app, &hex(3), ready.generation).unwrap();
        let now = Instant::now();
        let committed = actions
            .commit_exit(app, &hex(3), armed.generation, &hex(8), &hex(9), now)
            .unwrap();
        let replay = actions
            .commit_exit(
                app,
                &hex(3),
                armed.generation,
                &hex(8),
                &hex(9),
                now + Duration::from_secs(1),
            )
            .unwrap();
        assert_eq!(replay.generation, committed.generation);
        assert_eq!(replay.remaining_exit_grace_ms, Some(29_000));
        assert!(matches!(
            actions.take(
                launcher,
                &hex(3),
                committed.generation,
                &hex(8),
                false,
                now + Duration::from_secs(1)
            ),
            Err(Error::RestartNotReady)
        ));
        // Expected pipe teardown retains the old process lease and launcher.
        actions.disconnect(app).unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        actions
            .take(
                launcher,
                &hex(3),
                committed.generation,
                &hex(8),
                true,
                now + Duration::from_secs(2),
            )
            .unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 0);
        assert!(matches!(
            actions.take(
                launcher,
                &hex(3),
                committed.generation,
                &hex(8),
                true,
                now + Duration::from_secs(3)
            ),
            Err(Error::HandoffOutcomeUnknown)
        ));
    }
    #[test]
    fn cancel_and_expiry_cannot_be_revived_by_late_ready_commit_or_signal() {
        for late in [false, true] {
            let mut actions = Actions::<Lease>::new();
            let count = Arc::new(AtomicUsize::new(0));
            let app = connect(&mut actions, 1, Role::App);
            let launcher = connect(&mut actions, 2, Role::Launcher);
            let reservation = actions
                .reserve_restart(app, &intent(1), &hex(1), hex(3), hex(4))
                .unwrap();
            actions
                .finish_restart(reservation, lease(&count), hex(5))
                .unwrap();
            let joined = actions
                .join(launcher, &hex(3), &intent(2), &hex(6), hex(7))
                .unwrap();
            let ready = actions
                .ready(launcher, &hex(3), &hex(7), joined.generation)
                .unwrap();
            let armed = actions.arm(app, &hex(3), ready.generation).unwrap();
            let now = Instant::now();
            let commit = actions
                .commit_exit(app, &hex(3), armed.generation, &hex(8), &hex(9), now)
                .unwrap();
            if late {
                actions.reap_expired(now + Duration::from_secs(30)).unwrap();
            } else {
                actions.cancel(app, &hex(3), now).unwrap();
            }
            assert!(matches!(
                actions.take(
                    launcher,
                    &hex(3),
                    commit.generation,
                    &hex(8),
                    true,
                    now + Duration::from_secs(30)
                ),
                Err(Error::RestartNotReady)
            ));
            assert!(matches!(
                actions.commit_exit(app, &hex(3), armed.generation, &hex(8), &hex(9), now),
                Err(Error::RestartCanceled)
            ));
            assert_eq!(count.load(Ordering::SeqCst), 0);
        }
    }
    #[test]
    fn foreign_session_intent_cannot_be_treated_as_fresh_or_create_a_slot() {
        let mut actions = Actions::<Lease>::new();
        let c = connect(&mut actions, 1, Role::App);
        assert!(matches!(
            actions.reserve_restart(c, &intent(2), &hex(1), hex(2), hex(3)),
            Err(Error::IpcInvalid)
        ));
        assert_eq!(actions.cells.iter().flatten().count(), 0);
    }
    #[test]
    fn stale_cell_completion_cannot_cross_allocation_incarnation() {
        for normal in [false, true] {
            let mut actions = Actions::<Lease>::new();
            let count = Arc::new(AtomicUsize::new(0));
            let a = connect(
                &mut actions,
                1,
                if normal { Role::Launcher } else { Role::App },
            );
            let old = if normal {
                actions
                    .reserve_launch(a, &intent(1), &hex(1), hex(2))
                    .unwrap()
            } else {
                actions
                    .reserve_restart(a, &intent(1), &hex(1), hex(2), hex(3))
                    .unwrap()
            };
            actions.disconnect(a).unwrap();
            let b = connect(
                &mut actions,
                2,
                if normal { Role::Launcher } else { Role::App },
            );
            let current = if normal {
                actions
                    .reserve_launch(b, &intent(2), &hex(4), hex(5))
                    .unwrap()
            } else {
                actions
                    .reserve_restart(b, &intent(2), &hex(4), hex(5), hex(6))
                    .unwrap()
            };
            assert_eq!(old.slot, current.slot);
            assert!(old.incarnation != current.incarnation);
            let stale = if normal {
                actions
                    .finish_launch(old, lease(&count), hex(7))
                    .map(|_| ())
            } else {
                actions
                    .finish_restart(old, lease(&count), hex(7))
                    .map(|_| ())
            };
            assert!(stale.is_err());
            assert_eq!(count.load(Ordering::SeqCst), 0);
            actions.fail_reservation(old, Error::Unavailable);
            let cell = actions.cells[current.slot].as_ref().unwrap();
            assert!(cell.state == State::Reserved);
            assert!(cell.failure.is_none());
            assert!(cell.leases.is_none());
            if normal {
                actions
                    .finish_launch(current, lease(&count), hex(8))
                    .unwrap();
                assert_eq!(count.load(Ordering::SeqCst), 0);
            } else {
                actions
                    .finish_restart(current, lease(&count), hex(8))
                    .unwrap();
                assert_eq!(count.load(Ordering::SeqCst), 1);
            }
            actions.fail_reservation(old, Error::ImageRefused);
            assert!(
                actions.cells[current.slot]
                    .as_ref()
                    .unwrap()
                    .failure
                    .is_none()
            );
            drop(actions);
            assert_eq!(count.load(Ordering::SeqCst), 0);
        }
    }
    #[test]
    fn stale_disconnect_cannot_remove_new_connection_in_same_slot() {
        let mut actions = Actions::<Lease>::new();
        let a = connect(&mut actions, 1, Role::App);
        actions.disconnect(a).unwrap();
        let b = connect(&mut actions, 2, Role::App);
        assert_eq!(a.slot, b.slot);
        assert!(a.incarnation != b.incarnation);
        actions.disconnect(a).unwrap();
        assert!(actions.connection(b).is_ok());
        assert!(actions.connection(a).is_err());
        let current = actions
            .reserve_restart(b, &intent(2), &hex(4), hex(5), hex(6))
            .unwrap();
        actions.disconnect(a).unwrap();
        assert!(actions.connection(b).is_ok());
        assert!(actions.cells[current.slot].is_some());
        actions.disconnect(b).unwrap();
        assert!(actions.cells[current.slot].is_none());
    }
    #[test]
    fn completion_incarnation_counters_never_wrap_to_an_old_token() {
        let mut actions = Actions::<Lease>::new();
        actions.next_connection_incarnation = u64::MAX;
        assert!(matches!(
            actions.connect(peer(1), Role::App, hex(1)),
            Err(Error::Busy)
        ));
        assert_eq!(actions.connections.iter().flatten().count(), 0);
        actions.next_connection_incarnation = 1;
        let c = connect(&mut actions, 1, Role::App);
        actions.next_cell_incarnation = u64::MAX;
        assert!(matches!(
            actions.reserve_restart(c, &intent(1), &hex(2), hex(3), hex(4)),
            Err(Error::Busy)
        ));
        assert_eq!(actions.cells.iter().flatten().count(), 0);
        assert!(actions.connection(c).unwrap().link.is_none());
    }
}

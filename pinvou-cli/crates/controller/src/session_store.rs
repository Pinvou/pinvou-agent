use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use pinvou_runtime_api::{LogicalSessionId, SessionDescriptor, SessionSnapshot};
use pinvou_seglog::{Config, Cursor, SegmentLog};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

const SESSION_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Error)]
pub enum SessionStoreError {
    #[error("session storage I/O failed")]
    Io(#[from] std::io::Error),
    #[error("session storage JSON is invalid")]
    Json(#[from] serde_json::Error),
    #[error("session event log failed")]
    Seglog(#[from] pinvou_seglog::Error),
    #[error("session does not exist")]
    NotFound,
    #[error("session storage is inconsistent: {0}")]
    Corrupt(&'static str),
    #[error("session already exists")]
    AlreadyExists,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StoredSessionMetadata {
    pub schema_version: u16,
    pub descriptor: SessionDescriptor,
    pub attachment_epoch: u64,
    pub snapshot_cursor: u64,
}

impl StoredSessionMetadata {
    pub fn new(descriptor: SessionDescriptor, attachment_epoch: u64) -> Self {
        Self {
            schema_version: SESSION_SCHEMA_VERSION,
            descriptor,
            attachment_epoch,
            snapshot_cursor: 0,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct StoredEvent {
    schema_version: u16,
    sequence: u64,
    event: Value,
}

#[derive(Debug)]
struct SessionState {
    directory: PathBuf,
    metadata: StoredSessionMetadata,
    snapshot: SessionSnapshot,
    events: Vec<StoredEvent>,
    log: SegmentLog,
}

#[derive(Debug)]
pub struct SessionStore {
    sessions_root: PathBuf,
    sessions: BTreeMap<String, SessionState>,
}

impl SessionStore {
    pub fn open(data_root: impl AsRef<Path>) -> Result<Self, SessionStoreError> {
        let sessions_root = data_root.as_ref().join("sessions");
        std::fs::create_dir_all(&sessions_root)?;
        let mut sessions = BTreeMap::new();
        for entry in std::fs::read_dir(&sessions_root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                return Err(SessionStoreError::Corrupt(
                    "unexpected file in sessions root",
                ));
            }
            let state = open_state(entry.path())?;
            let id = state.metadata.descriptor.id.as_str().to_owned();
            if sessions.insert(id, state).is_some() {
                return Err(SessionStoreError::Corrupt("duplicate logical session id"));
            }
        }
        Ok(Self {
            sessions_root,
            sessions,
        })
    }

    pub fn create_session(
        &mut self,
        metadata: StoredSessionMetadata,
    ) -> Result<(), SessionStoreError> {
        validate_metadata(&metadata)?;
        let id = metadata.descriptor.id.as_str().to_owned();
        if self.sessions.contains_key(&id) {
            return Err(SessionStoreError::AlreadyExists);
        }
        let directory = self.sessions_root.join(stable_key(id.as_bytes()));
        if directory.exists() {
            return Err(SessionStoreError::Corrupt(
                "session directory hash collision",
            ));
        }
        std::fs::create_dir_all(&directory)?;
        let snapshot = SessionSnapshot {
            descriptor: metadata.descriptor.clone(),
            cursor: 0,
            normalized_events: Vec::new(),
        };
        let log = SegmentLog::open(
            Config::new(directory.join("events.seglog"))
                .with_stream_metadata(b"pinvou-session-events-v1".to_vec()),
        )?
        .log;
        atomic_write_json(&directory.join("snapshot.json"), &snapshot)?;
        atomic_write_json(&directory.join("metadata.json"), &metadata)?;
        self.sessions.insert(
            id,
            SessionState {
                directory,
                metadata,
                snapshot,
                events: Vec::new(),
                log,
            },
        );
        Ok(())
    }

    pub fn append_event(
        &mut self,
        session_id: &LogicalSessionId,
        event: Value,
    ) -> Result<u64, SessionStoreError> {
        let state = self
            .sessions
            .get_mut(session_id.as_str())
            .ok_or(SessionStoreError::NotFound)?;
        let sequence = state.events.len() as u64 + 1;
        let stored = StoredEvent {
            schema_version: SESSION_SCHEMA_VERSION,
            sequence,
            event,
        };
        let bytes = serde_json::to_vec(&stored)?;
        state.log.append_batch([bytes.as_slice()])?;
        state.log.durable_barrier()?;
        state.events.push(stored);
        Ok(sequence)
    }

    pub fn write_snapshot(
        &mut self,
        session_id: &LogicalSessionId,
        snapshot: SessionSnapshot,
    ) -> Result<(), SessionStoreError> {
        let state = self
            .sessions
            .get_mut(session_id.as_str())
            .ok_or(SessionStoreError::NotFound)?;
        if snapshot.descriptor.id != *session_id || snapshot.cursor > state.events.len() as u64 {
            return Err(SessionStoreError::Corrupt(
                "snapshot cursor or id is invalid",
            ));
        }
        state.log.durable_barrier()?;
        atomic_write_json(&state.directory.join("snapshot.json"), &snapshot)?;
        state.metadata.descriptor = snapshot.descriptor.clone();
        state.metadata.snapshot_cursor = snapshot.cursor;
        atomic_write_json(&state.directory.join("metadata.json"), &state.metadata)?;
        state.snapshot = snapshot;
        Ok(())
    }

    pub fn restore(
        &self,
        session_id: &LogicalSessionId,
    ) -> Result<SessionSnapshot, SessionStoreError> {
        let state = self
            .sessions
            .get(session_id.as_str())
            .ok_or(SessionStoreError::NotFound)?;
        if state.snapshot.cursor > state.events.len() as u64 {
            return Err(SessionStoreError::Corrupt("snapshot cursor exceeds WAL"));
        }
        let mut restored = state.snapshot.clone();
        restored.normalized_events.extend(
            state
                .events
                .iter()
                .filter(|event| event.sequence > restored.cursor)
                .map(|event| event.event.clone()),
        );
        restored.cursor = state
            .events
            .last()
            .map_or(restored.cursor, |event| event.sequence);
        Ok(restored)
    }

    pub fn list(&self) -> Vec<SessionDescriptor> {
        let mut sessions = self
            .sessions
            .values()
            .map(|state| state.metadata.descriptor.clone())
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| right.last_active_at.cmp(&left.last_active_at));
        sessions
    }
}

fn open_state(directory: PathBuf) -> Result<SessionState, SessionStoreError> {
    let metadata: StoredSessionMetadata = read_json(&directory.join("metadata.json"))?;
    validate_metadata(&metadata)?;
    let snapshot: SessionSnapshot = read_json(&directory.join("snapshot.json"))?;
    if snapshot.descriptor.id != metadata.descriptor.id
        || snapshot.cursor != metadata.snapshot_cursor
    {
        return Err(SessionStoreError::Corrupt("metadata and snapshot disagree"));
    }
    let opened = SegmentLog::open(
        Config::new(directory.join("events.seglog"))
            .with_stream_metadata(b"pinvou-session-events-v1".to_vec()),
    )?;
    if opened.recovery.issue.is_some() {
        return Err(SessionStoreError::Corrupt("session WAL required recovery"));
    }
    let mut events = Vec::new();
    for record in opened.log.replay_from(Cursor::new(1))? {
        let event: StoredEvent = serde_json::from_slice(&record.payload)?;
        if event.schema_version != SESSION_SCHEMA_VERSION || event.sequence != record.cursor.get() {
            return Err(SessionStoreError::Corrupt(
                "session WAL sequence is invalid",
            ));
        }
        events.push(event);
    }
    if snapshot.cursor > events.len() as u64 {
        return Err(SessionStoreError::Corrupt("snapshot cursor exceeds WAL"));
    }
    Ok(SessionState {
        directory,
        metadata,
        snapshot,
        events,
        log: opened.log,
    })
}

fn validate_metadata(metadata: &StoredSessionMetadata) -> Result<(), SessionStoreError> {
    if metadata.schema_version != SESSION_SCHEMA_VERSION {
        return Err(SessionStoreError::Corrupt("unsupported session schema"));
    }
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, SessionStoreError> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

pub(crate) fn atomic_write_json<T: Serialize>(
    path: &Path,
    value: &T,
) -> Result<(), SessionStoreError> {
    let parent = path
        .parent()
        .ok_or(SessionStoreError::Corrupt("storage path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = parent.join(format!(".write-{}-{nonce}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(value)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    replace_file(&temp, path)?;
    Ok(())
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let source = source
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect::<Vec<_>>();
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

pub(crate) fn stable_key(bytes: &[u8]) -> String {
    let hash = bytes.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    format!("{hash:016x}")
}

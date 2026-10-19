use crate::{Error, RECORD_LIMIT, hash, json, platform};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{BufRead, BufReader, Seek, SeekFrom, Write},
};

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Phase {
    Created,
    Running,
    Opened,
    Paused,
    Cancelled,
    NetworkUnknown,
    SourceRefused,
    IoUnknown,
    BytesMatch,
    BytesMismatch,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Record {
    pub sequence: u64,
    pub manifest_sha256: String,
    pub previous_sha256: String,
    pub attempt: u64,
    pub phase: Phase,
    pub identity: Option<platform::Identity>,
}
pub(crate) struct Journal {
    pub file: File,
    pub sequence: u64,
    pub previous: String,
    pub attempt: u64,
    pub phase: Phase,
    pub identity: Option<platform::Identity>,
    pub run_id: Option<String>,
    pin: String,
}
impl Journal {
    pub fn open(mut file: File, pin: &str) -> Result<Self, Error> {
        file.seek(SeekFrom::Start(0))
            .map_err(|_| Error::IoUnavailable)?;
        let mut state = Journal {
            file,
            sequence: 0,
            previous: String::new(),
            attempt: 0,
            phase: Phase::Created,
            identity: None,
            run_id: None,
            pin: pin.into(),
        };
        let mut reader = BufReader::new(&state.file);
        loop {
            // read_until alone could allocate an arbitrary hostile journal line.
            let mut raw = Vec::new();
            let mut limited = (&mut reader).take(RECORD_LIMIT as u64 + 1);
            use std::io::Read;
            limited
                .read_until(b'\n', &mut raw)
                .map_err(|_| Error::JournalDamaged)?;
            if raw.is_empty() {
                break;
            }
            if raw.len() > RECORD_LIMIT || raw.last() != Some(&b'\n') {
                return Err(Error::JournalDamaged);
            }
            raw.pop();
            let record: Record = serde_json::from_slice(&raw).map_err(|_| Error::JournalDamaged)?;
            if json(&record)? != raw
                || record.manifest_sha256 != pin
                || record.sequence != state.sequence + 1
                || record.previous_sha256 != state.previous
            {
                return Err(Error::JournalDamaged);
            }
            let first = state.sequence == 0;
            let legal = if first {
                record.phase == Phase::Created && record.attempt == 0 && record.identity.is_none()
            } else {
                match record.phase {
                    Phase::Running => {
                        record.attempt
                            == state.attempt.checked_add(1).ok_or(Error::JournalDamaged)?
                            && record.identity.is_none()
                            && state.phase != Phase::Cancelled
                    }
                    Phase::Opened => {
                        state.phase == Phase::Running
                            && record.attempt == state.attempt
                            && record.identity.is_some()
                    }
                    Phase::Paused
                    | Phase::Cancelled
                    | Phase::NetworkUnknown
                    | Phase::SourceRefused
                    | Phase::IoUnknown
                    | Phase::BytesMatch
                    | Phase::BytesMismatch => {
                        matches!(state.phase, Phase::Running | Phase::Opened)
                            && record.attempt == state.attempt
                            && record.identity == state.identity
                    }
                    Phase::Created => false,
                }
            };
            if !legal {
                return Err(Error::JournalDamaged);
            }
            if record.phase == Phase::Running {
                state.run_id = Some(hash(&raw));
            }
            state.sequence = record.sequence;
            state.previous = hash(&raw);
            state.attempt = record.attempt;
            state.phase = record.phase;
            state.identity = record.identity;
        }
        drop(reader);
        state
            .file
            .seek(SeekFrom::End(0))
            .map_err(|_| Error::IoUnavailable)?;
        Ok(state)
    }
    pub fn append(
        &mut self,
        phase: Phase,
        attempt: u64,
        identity: Option<platform::Identity>,
    ) -> Result<(), Error> {
        let sequence = self.sequence.checked_add(1).ok_or(Error::JournalDamaged)?;
        let record = Record {
            sequence,
            manifest_sha256: self.pin.clone(),
            previous_sha256: self.previous.clone(),
            attempt,
            phase,
            identity,
        };
        let mut bytes = json(&record)?;
        if bytes.len() + 1 > RECORD_LIMIT {
            return Err(Error::JournalDamaged);
        }
        let next = hash(&bytes);
        bytes.push(b'\n');
        self.file
            .write_all(&bytes)
            .map_err(|_| Error::IoUnavailable)?;
        self.file.sync_all().map_err(|_| Error::IoUnavailable)?;
        if phase == Phase::Running {
            self.run_id = Some(next.clone());
        }
        self.sequence = sequence;
        self.previous = next;
        self.attempt = attempt;
        self.phase = phase;
        self.identity = identity;
        Ok(())
    }
}

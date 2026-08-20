use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

pub struct RollingLog {
    path: PathBuf,
    file: File,
    max_bytes: u64,
    backups: usize,
    pending: Vec<u8>,
}

impl RollingLog {
    pub const DEFAULT_MAX_BYTES: u64 = 50 * 1024 * 1024;
    pub const DEFAULT_FILE_COUNT: usize = 5;

    pub fn open(path: PathBuf) -> io::Result<Self> {
        // Five files total: the active file plus four rotated generations.
        Self::with_limits(path, Self::DEFAULT_MAX_BYTES, Self::DEFAULT_FILE_COUNT - 1)
    }

    pub fn with_limits(path: PathBuf, max_bytes: u64, backups: usize) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            path,
            file,
            max_bytes,
            backups,
            pending: Vec::new(),
        })
    }

    fn rotate_if_needed(&mut self, incoming: usize) -> io::Result<()> {
        if self.file.metadata()?.len() == 0
            || self.file.metadata()?.len().saturating_add(incoming as u64) <= self.max_bytes
        {
            return Ok(());
        }
        self.file.flush()?;
        for index in (1..=self.backups).rev() {
            let target = backup_path(&self.path, index);
            let source = if index == 1 {
                self.path.clone()
            } else {
                backup_path(&self.path, index - 1)
            };
            if target.exists() {
                std::fs::remove_file(&target)?;
            }
            if source.exists() {
                std::fs::rename(source, target)?;
            }
        }
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        Ok(())
    }

    fn write_record(&mut self, record: &[u8]) -> io::Result<()> {
        let mut redacted = redact(record);
        if redacted.len() as u64 > self.max_bytes {
            let marker = b" [TRUNCATED]\n";
            let keep = (self.max_bytes as usize).saturating_sub(marker.len());
            redacted.truncate(keep);
            redacted.extend_from_slice(&marker[..marker.len().min(self.max_bytes as usize)]);
            redacted.truncate(self.max_bytes as usize);
        }
        self.rotate_if_needed(redacted.len())?;
        self.file.write_all(&redacted)
    }
}

impl Write for RollingLog {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buffer);
        while let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
            let record = self.pending.drain(..=end).collect::<Vec<_>>();
            self.write_record(&record)?;
        }
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            self.write_record(&pending)?;
        }
        self.file.flush()
    }
}

impl Drop for RollingLog {
    fn drop(&mut self) {
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            let _ = self.write_record(&pending);
        }
        let _ = self.file.flush();
    }
}

fn backup_path(path: &Path, index: usize) -> PathBuf {
    PathBuf::from(format!("{}.{}", path.display(), index))
}

fn redact(buffer: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(buffer);
    let lower = text.to_ascii_lowercase();
    let markers = [
        "authorization",
        "token",
        "cookie",
        "api_key",
        "chatgpt_access_token",
    ];
    let first = markers.iter().filter_map(|marker| lower.find(marker)).min();
    match first {
        Some(index) => {
            let prefix = &text[..index];
            format!("{prefix}[REDACTED]\n").into_bytes()
        }
        None => buffer.to_vec(),
    }
}

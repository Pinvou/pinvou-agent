/// Temporary WAV file: `NamedTempFile` generates an unpredictable file name
/// with 0600 permissions on Unix (the old hand-built pid+millisecond name was
/// predictable and world-readable at 0644); the file is deleted on drop.
pub(crate) struct VoiceTempWav {
    file: tempfile::NamedTempFile,
}

impl VoiceTempWav {
    pub(crate) fn create() -> std::io::Result<Self> {
        let file = tempfile::Builder::new()
            .prefix("pinvou3-voice-")
            .suffix(".wav")
            .tempfile()?;
        Ok(Self { file })
    }

    /// Finish writing before handing the path to an external recognizer. On
    /// Windows, keeping the `NamedTempFile` write handle alive can prevent a
    /// backend whose share mode denies write sharing from reopening the WAV.
    pub(crate) fn write_and_close(
        mut self,
        audio_bytes: &[u8],
    ) -> std::io::Result<tempfile::TempPath> {
        use std::io::Write;

        self.file.write_all(audio_bytes)?;
        self.file.flush()?;
        Ok(self.file.into_temp_path())
    }
}

#[cfg(test)]
mod tests {
    use super::VoiceTempWav;

    #[test]
    fn closed_temp_wav_keeps_contents_and_cleans_up_on_drop() {
        let wav_path = VoiceTempWav::create()
            .expect("create temporary WAV")
            .write_and_close(b"RIFF-test-WAVE")
            .expect("write and close temporary WAV");
        let path = wav_path.to_path_buf();

        assert_eq!(
            std::fs::read(&path).expect("read temporary WAV"),
            b"RIFF-test-WAVE"
        );
        drop(wav_path);
        assert!(!path.exists(), "temporary WAV must be removed on drop");
    }
}

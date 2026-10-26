//! Exact committed registry readback for a budget publication. The caller owns
//! the real BootstrapGuard and commit; this readback never grants stage action.
use super::*;

impl Publication {
    /// Call only after the outer transaction has committed and been dropped.
    /// Preserve the guard through every composed owner's readback. Any failure
    /// is commit-unknown, including changed host, failed flush or nonexact bytes.
    #[allow(unsafe_code)]
    pub(in crate::platform::windows::registry) fn readback_after_commit(
        &self,
        store: &Store,
        host: &StartedServiceContext,
    ) -> Result<()> {
        let verify = || -> Result<()> {
            let current = store
                .read(&host.context.seed)?
                .ok_or(Error::StateCommitUnknown)?;
            host.authenticate_running(&current)?;
            if current.digest()? != self.expected_host_sha256 {
                return Err(Error::StateCommitUnknown);
            }
            let private = store.key(false, None)?.ok_or(Error::StateCommitUnknown)?;
            let key = open(private.0, NAMESPACE, false, None)?.ok_or(Error::StateCommitUnknown)?;
            let history = open(
                key.0,
                &format!("history\\{}", revision_name(self.head_revision)),
                false,
                None,
            )?
            .ok_or(Error::StateCommitUnknown)?;
            let written = open(
                key.0,
                &format!(
                    "windows\\{}\\{}",
                    self.window_key,
                    revision_name(self.snapshot.revision)
                ),
                false,
                None,
            )?
            .ok_or(Error::StateCommitUnknown)?;
            if unsafe { RegFlushKey(key.0) } != 0
                || unsafe { RegFlushKey(history.0) } != 0
                || unsafe { RegFlushKey(written.0) } != 0
            {
                return Err(Error::StateCommitUnknown);
            }
            let (readback, bytes) = read_head(&key, self.id.binding(), None)?;
            let (_, window_bytes) =
                read_window(&key, &readback, &self.id, None)?.ok_or(Error::StateCommitUnknown)?;
            if bytes != self.head_bytes || window_bytes != self.window_bytes {
                return Err(Error::StateCommitUnknown);
            }
            Ok(())
        };
        verify().map_err(|_| Error::StateCommitUnknown)
    }
}

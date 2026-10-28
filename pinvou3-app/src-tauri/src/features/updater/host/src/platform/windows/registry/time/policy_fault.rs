//! The SCM owner records only positively read invalid Policy bytes. Missing,
//! inaccessible, raced or unreadable values cannot enter this publisher.
use super::*;

/// A successful bounded native read is essential: a generic StateCorrupt can
/// also mean a missing/wrong-type value. Such cases remain diagnosed/refused,
/// without claiming a complete stored-byte observation or recording an incident.
fn confirmed_invalid_policy(key: &Key, binding: &Binding) -> Result<String> {
    match policy_in(key, binding) {
        Ok(_) => return Err(Error::ScopeConflict),
        Err(Error::StateCorrupt) => {}
        Err(error) => return Err(error),
    }
    let raw = read(key, "Policy")?;
    match Policy::parse(&raw, binding) {
        Err(Error::StateCorrupt) => Ok(json::sha256(&raw)),
        Ok(_) => Err(Error::ScopeConflict),
        Err(error) => Err(error),
    }
}
impl Store {
    /// Fixed fault side effect, never a generic repair or initialization port.
    /// The intact original time/retention/budget state is authenticated again;
    /// incident and Unknown/barrier successor commit in the SAME native TxR.
    #[allow(unsafe_code)]
    pub(in crate::platform::windows) fn latch_time_policy_fault(
        &self,
        host: &StartedServiceContext,
        expected: &TimeSnapshot,
    ) -> Result<()> {
        let seed = &host.context.seed;
        let _guard = BootstrapGuard::acquire(seed, false)?;
        let _product_guard = super::super::product_catalog::ProductGuard::acquire(seed)?;
        let tx = Handle::owned(unsafe {
            CreateTransaction(ptr::null_mut(), ptr::null_mut(), 0, 0, 0, 0, ptr::null())
        })?;
        let actual = self
            .read_in(seed, Some(&tx))?
            .ok_or(Error::SetupIncomplete)?;
        host.authenticate_running(&actual)?;
        let private = self.key(true, Some(&tx))?.ok_or(Error::SetupIncomplete)?;
        let current = state_in(&private, &actual, Some(&tx))?;
        if current.raw != expected.raw || current.original_root != expected.original_root {
            return Err(Error::ScopeConflict);
        }
        // This delivered recovery profile has genuine never-enabled custody.
        // Future nonzero producers require their own secret/backup fault actions.
        retention::read_in(&private, &actual, Some(&tx))?;
        budgets::floor_in(&private, &actual, Some(&tx))?;
        let time = open(private.0, NAMESPACE, false, Some(&tx))?.ok_or(Error::SetupIncomplete)?;
        let policy_sha256 = confirmed_invalid_policy(&time, &current.state.binding)?;
        if current
            .state
            .policy_fault
            .as_ref()
            .is_some_and(PolicyFault::is_active)
        {
            return Ok(()); // Original incident stays immutable across retries.
        }
        let next = current.state.next_policy_fault(
            Incarnation::from_host(&actual)?,
            &current.raw,
            policy_sha256,
        )?;
        let raw = stage_state_in(&private, &actual, &current, &next, &tx)?;
        drop(time);
        drop(private);
        if unsafe { CommitTransaction(tx.raw()) } == 0 {
            return Err(Error::StateCommitUnknown);
        }
        drop(tx);
        let verify = || -> Result<()> {
            let after = self.read(seed)?.ok_or(Error::StateCommitUnknown)?;
            host.authenticate_running(&after)?;
            if after.digest()? != actual.digest()? {
                return Err(Error::StateCommitUnknown);
            }
            let private = self.key(false, None)?.ok_or(Error::StateCommitUnknown)?;
            let time = open(private.0, NAMESPACE, false, None)?.ok_or(Error::StateCommitUnknown)?;
            let history = open(
                time.0,
                &format!("history\\{}", revision_name(next.revision)),
                false,
                None,
            )?
            .ok_or(Error::StateCommitUnknown)?;
            if unsafe { RegFlushKey(time.0) } != 0 || unsafe { RegFlushKey(history.0) } != 0 {
                return Err(Error::StateCommitUnknown);
            }
            if state_in(&private, &after, None)?.raw != raw {
                return Err(Error::StateCommitUnknown);
            }
            Ok(())
        };
        verify().map_err(|_| Error::StateCommitUnknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    #[allow(unsafe_code)]
    fn only_positive_bounded_invalid_policy_bytes_confirm_a_policy_fault() {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        struct OwnedKey {
            key: Option<Key>,
            path: Vec<u16>,
        }
        impl Drop for OwnedKey {
            fn drop(&mut self) {
                drop(self.key.take());
                if !self.path.is_empty() {
                    unsafe {
                        RegDeleteTreeW(HKEY_CURRENT_USER, self.path.as_ptr());
                    }
                }
            }
        }
        let path = wide(&format!(
            "Software\\PinvouTimePolicyFaultTest-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut handle = ptr::null_mut();
        let mut disposition = 0;
        assert_eq!(
            unsafe {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    path.as_ptr(),
                    0,
                    ptr::null(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_READ | KEY_WRITE,
                    ptr::null(),
                    &mut handle,
                    &mut disposition,
                )
            },
            0
        );
        let key = Key(handle);
        assert_eq!(disposition, REG_CREATED_NEW_KEY);
        let mut owned = OwnedKey {
            key: Some(key),
            path,
        };
        let key = owned.key.as_ref().unwrap();
        let binding = Binding {
            installation_scope_id: "scope".into(),
            product: "pinvou".into(),
            component: "app".into(),
            target_key: "windows-x86_64-nsis-perMachine".into(),
            profile_kind: "production".into(),
            profile_id: "host".into(),
            genesis_seed_sha256: "a".repeat(64),
            genesis_manifest_sha256: "b".repeat(64),
            original_install_operation_id: "original".into(),
        };
        assert!(matches!(
            confirmed_invalid_policy(key, &binding),
            Err(Error::PrerequisiteUnavailable)
        ));
        let valid = initial_policy_bytes(&binding).unwrap();
        write(key, "Policy", &valid).unwrap();
        assert!(matches!(
            confirmed_invalid_policy(key, &binding),
            Err(Error::ScopeConflict)
        ));
        write(key, "Policy", b"{").unwrap();
        assert_eq!(
            confirmed_invalid_policy(key, &binding).unwrap(),
            json::sha256(b"{")
        );
        let mut other = binding.clone();
        other.original_install_operation_id = "other".into();
        write(key, "Policy", &initial_policy_bytes(&other).unwrap()).unwrap();
        assert_eq!(
            confirmed_invalid_policy(key, &binding).unwrap(),
            json::sha256(&initial_policy_bytes(&other).unwrap())
        );
        // Native read failure is unavailable, never a positive Policy observation.
        let invalid = Key(ptr::null_mut());
        assert!(matches!(
            confirmed_invalid_policy(&invalid, &binding),
            Err(Error::Unavailable)
        ));
        drop(invalid);
        drop(owned.key.take());
        assert_eq!(
            unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, owned.path.as_ptr()) },
            0
        );
        owned.path.clear();
    }
}

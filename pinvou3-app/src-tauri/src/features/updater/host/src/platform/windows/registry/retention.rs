//! Fresh-original-only, fixed native retention custody. Never initializes on
//! load and never interprets an absent producer namespace as a legacy census.
//! There is no activation/nonzero owner port. Package download caches are not
//! time obligations and are neither scanned nor deleted by this namespace.
use super::*;
use crate::{
    retention_custody::{Completion, Genesis},
    trusted_time::{Binding, State, Status},
};

pub(super) const NAMESPACE: &str = "retention-custody-v1";
// These exact storage names are reserved for future real obligation owners.
// A future owner must replace this never-enabled profile through a reviewed
// native enrollment before writing any secret, backup byte or key reference.
const FORBIDDEN_PRODUCERS: &[&str] = &[
    "consume-attempts-v1",
    "consume-query-secret-copies-v1",
    "local-backups-v1",
    "backup-retention-staging-v1",
    "backup-decryption-keys-v1",
];
pub(super) const MAX_COMPLETIONS: u32 = 4096; // bounded adapter capacity, not a retention policy.

fn exact_names(key: &Key, values: bool, expected: &[&str]) -> Result<()> {
    let mut actual = names(key, values, expected.len() as u32)?;
    actual.sort();
    let mut want: Vec<_> = expected.iter().map(|s| (*s).to_owned()).collect();
    want.sort();
    if actual != want {
        return Err(Error::StateCorrupt);
    }
    Ok(())
}
#[allow(unsafe_code)]
fn absent_producer_value(private: &Key, name: &str) -> Result<()> {
    let name = wide(name);
    let mut size = 0;
    let status = unsafe {
        RegQueryValueExW(
            private.0,
            name.as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut size,
        )
    };
    match status {
        ERROR_FILE_NOT_FOUND => Ok(()),
        0 => Err(Error::StateCorrupt), // An actual forbidden producer value exists.
        _ => Err(Error::Unavailable),  // Access/I/O/race is not confirmed corruption.
    }
}
fn no_producer_storage(private: &Key, tx: Option<&Handle>) -> Result<()> {
    for name in FORBIDDEN_PRODUCERS {
        if open(private.0, name, false, tx)?.is_some() {
            return Err(Error::StateCorrupt);
        }
        absent_producer_value(private, name)?;
    }
    Ok(())
}
fn completion_name(value: &str) -> Result<u64> {
    let revision: u64 = value.parse().map_err(|_| Error::StateCorrupt)?;
    if revision < 2 || revision > json::SAFE_MAX || revision_name(revision) != value {
        return Err(Error::StateCorrupt);
    }
    Ok(revision)
}
/// Full bounded census of the exact fixed custody namespace, including retained
/// completions and their actual Time-history reverse links. No recursion into
/// time::state_in: the latter invokes this census for a completed current state.
pub(super) fn read_in(
    private: &Key,
    host: &HostRegistration,
    tx: Option<&Handle>,
) -> Result<Vec<u8>> {
    let key = open(private.0, NAMESPACE, false, tx)?.ok_or(Error::SetupIncomplete)?;
    exact_names(&key, true, &["Genesis"])?;
    exact_names(&key, false, &["completions"])?;
    let raw = read(&key, "Genesis")?;
    Genesis::parse(&raw, &Binding::from_host(host))?;
    no_producer_storage(private, tx)?;
    let history = open(key.0, "completions", false, tx)?.ok_or(Error::StateCorrupt)?;
    exact_names(&history, true, &[])?;
    let mut actual_references = Vec::new();
    for name in names(&history, false, MAX_COMPLETIONS)? {
        let revision = completion_name(&name)?;
        let row = open(history.0, &name, false, tx)?.ok_or(Error::StateCorrupt)?;
        exact_names(&row, true, &["Record"])?;
        exact_names(&row, false, &[])?;
        let body = read(&row, "Record")?;
        let c = Completion::parse(&body, &Binding::from_host(host), &raw)?;
        if c.completed_time_revision != revision {
            return Err(Error::StateCorrupt);
        }
        time::check_retained_completion_in(private, &c, &body, tx)?;
        actual_references.push(c.reference()?);
    }
    let retained_references = time::retained_completion_references_in(private, host, tx)?;
    exact_completion_references(&retained_references, &actual_references)?;
    Ok(raw)
}
/// Compare both authenticated native read collections. DTO references by
/// themselves are DATA; this function cannot mint a cleanup capability.
pub(super) fn exact_completion_references(
    retained: &[crate::retention_custody::ZeroRetentionReference],
    actual: &[crate::retention_custody::ZeroRetentionReference],
) -> Result<()> {
    fn ordered(
        values: &[crate::retention_custody::ZeroRetentionReference],
    ) -> Result<Vec<(u64, String)>> {
        let mut result = Vec::with_capacity(values.len());
        for reference in values {
            reference.validate()?;
            result.push((
                reference.completion_revision,
                reference.completion_sha256.clone(),
            ));
        }
        result.sort_unstable();
        if result.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(Error::StateCorrupt);
        }
        Ok(result)
    }
    if ordered(retained)? != ordered(actual)? {
        return Err(Error::StateCorrupt);
    }
    Ok(())
}
pub(super) fn initialize_in(private: &Key, tx: &Handle, host: &HostRegistration) -> Result<()> {
    // Caller is ONLY Store::commit(None), after actual original absence in this
    // transaction. Existing root/subkey/orphan refuses; no implicit migration.
    no_producer_storage(private, Some(tx))?;
    let raw = Genesis::new(Binding::from_host(host))?.bytes()?;
    let key = create(private.0, NAMESPACE, tx, false, true)?;
    create(key.0, "completions", tx, false, true)?;
    write(&key, "Genesis", &raw)?;
    if read_in(private, host, Some(tx))? != raw {
        return Err(Error::StateCorrupt);
    }
    Ok(())
}
pub(super) fn verify_fresh_in(private: &Key, host: &HostRegistration) -> Result<()> {
    read_in(private, host, None)?;
    let key = open(private.0, NAMESPACE, false, None)?.ok_or(Error::StateCommitUnknown)?;
    let history = open(key.0, "completions", false, None)?.ok_or(Error::StateCommitUnknown)?;
    if !names(&history, false, MAX_COMPLETIONS)?.is_empty() {
        return Err(Error::StateCommitUnknown);
    }
    Ok(())
}
pub(super) fn stage_completion_in(
    private: &Key,
    tx: &Handle,
    c: &Completion,
    inventory: &[u8],
) -> Result<()> {
    c.validate(&c.binding, inventory)?;
    let key = open(private.0, NAMESPACE, true, Some(tx))?.ok_or(Error::SetupIncomplete)?;
    if read(&key, "Genesis")? != inventory {
        return Err(Error::ScopeConflict);
    }
    let history = open(key.0, "completions", true, Some(tx))?.ok_or(Error::StateCorrupt)?;
    if names(&history, false, MAX_COMPLETIONS)?.len() >= MAX_COMPLETIONS as usize {
        return Err(Error::Unavailable);
    }
    let row = create(
        history.0,
        &revision_name(c.completed_time_revision),
        tx,
        false,
        true,
    )?;
    write(&row, "Record", &c.bytes()?)?;
    Ok(())
}
pub(super) fn validate_completed_in(
    private: &Key,
    host: &HostRegistration,
    state: &State,
    tx: Option<&Handle>,
) -> Result<()> {
    let inventory = read_in(private, host, tx)?;
    let reference = state.zero_retention.as_ref().ok_or(Error::StateCorrupt)?;
    if state.status != Status::CleanupVerifiedZero || state.cleanup_pending {
        return Err(Error::StateCorrupt);
    }
    let row = open(
        private.0,
        &format!(
            "{}\\completions\\{}",
            NAMESPACE,
            revision_name(reference.completion_revision)
        ),
        false,
        tx,
    )?
    .ok_or(Error::StateCorrupt)?;
    let body = read(&row, "Record")?;
    let c = Completion::parse(&body, &state.binding, &inventory)?;
    if c.reference()? != *reference
        || c.completed_time_revision != state.revision
        || state.previous_sha256.as_ref() != Some(&c.anchor_time_sha256)
        || state.incarnation.as_ref() != Some(&c.incarnation)
        || json::sha256(
            state
                .evidence
                .as_ref()
                .ok_or(Error::StateCorrupt)?
                .policy
                .as_bytes(),
        ) != c.policy_sha256
    {
        return Err(Error::StateCorrupt);
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_canonical_revision_names_can_select_a_completion() {
        assert_eq!(completion_name(&revision_name(2)).unwrap(), 2);
        for bad in ["", "2", "../2", "0", "9007199254740992", "-1"] {
            assert!(completion_name(bad).is_err());
        }
        assert!(
            !FORBIDDEN_PRODUCERS
                .iter()
                .any(|s| s.contains("download") || s.contains("cache"))
        );
    }
    #[test]
    #[allow(unsafe_code)]
    fn native_name_census_rejects_actual_extra_values_and_subkeys() {
        // Real HKCU storage mechanics only, not a fake protected scope, native
        // SCM context, custody initialization or successful cleanup capability.
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let path = format!(
            "Software\\PinvouRetentionCodecTest-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let path = wide(&path);
        let mut raw = ptr::null_mut();
        let mut disposition = 0;
        assert_eq!(
            unsafe {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    path.as_ptr(),
                    0,
                    ptr::null_mut(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_READ | KEY_WRITE,
                    ptr::null(),
                    &mut raw,
                    &mut disposition,
                )
            },
            0
        );
        assert!(!raw.is_null());
        if disposition != REG_CREATED_NEW_KEY {
            drop(Key(raw));
            panic!("test key was not newly owned");
        }
        struct Owned {
            key: Option<Key>,
            path: Vec<u16>,
        }
        impl Drop for Owned {
            fn drop(&mut self) {
                drop(self.key.take());
                // The only deletion target is the exact freshly created test
                // key above, under HKCU. No producer or actual host path is used.
                if !self.path.is_empty() {
                    unsafe {
                        RegDeleteTreeW(HKEY_CURRENT_USER, self.path.as_ptr());
                    }
                }
            }
        }
        let mut own = Owned {
            key: Some(Key(raw)),
            path,
        };
        let key = own.key.as_ref().unwrap();
        exact_names(key, true, &[]).unwrap();
        exact_names(key, false, &[]).unwrap();
        write(key, "Genesis", b"{}").unwrap();
        exact_names(key, true, &["Genesis"]).unwrap();
        write(key, "UnexpectedSecretCopy", b"not-a-real-secret").unwrap();
        assert!(exact_names(key, true, &["Genesis"]).is_err());
        let mut child = ptr::null_mut();
        let mut child_disposition = 0;
        assert_eq!(
            unsafe {
                RegCreateKeyExW(
                    key.0,
                    wide("orphan").as_ptr(),
                    0,
                    ptr::null_mut(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_READ | KEY_WRITE,
                    ptr::null(),
                    &mut child,
                    &mut child_disposition,
                )
            },
            0
        );
        drop(Key(child));
        assert!(exact_names(key, false, &[]).is_err());
        // Confirm exact owned cleanup on success; RAII separately covers unwind.
        drop(own.key.take());
        assert_eq!(
            unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, own.path.as_ptr()) },
            0
        );
        let mut reopened = ptr::null_mut();
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                own.path.as_ptr(),
                0,
                KEY_READ,
                &mut reopened,
            )
        };
        if status == 0 {
            drop(Key(reopened));
        }
        assert_eq!(status, ERROR_FILE_NOT_FOUND);
        own.path.clear();
    }
}

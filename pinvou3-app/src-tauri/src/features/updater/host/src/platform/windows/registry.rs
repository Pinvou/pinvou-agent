//! Protected native64 registry transaction owner; no ordinary-file fallback.
//! Files/SCM are outside this transaction and are recovered from original intent.
pub(super) mod time;

use super::{
    handle::Handle,
    security::{NativeSecurity, inspect_registry},
};
use crate::{
    bootstrap::OwnerBootstrap,
    error::{Error, Result},
    inventory::{InventoryBytes, MAX_SHARDS},
    json,
    profile::{InstallationManifest, RootManifest, Seed},
    registration::{HostRegistration, PublicRegistration},
};
use std::{marker::PhantomData, ptr, rc::Rc};
use windows_sys::Win32::{
    Foundation::{
        ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_PATH_NOT_FOUND, WAIT_ABANDONED, WAIT_OBJECT_0,
        WAIT_TIMEOUT,
    },
    Storage::FileSystem::{CommitTransaction, CreateTransaction, SYNCHRONIZE},
    System::{
        Registry::*,
        Threading::{CreateMutexExW, MUTEX_MODIFY_STATE, ReleaseMutex, WaitForSingleObject},
    },
};
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
struct Key(HKEY);
impl Drop for Key {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}
/// Every component is opened with OPEN_LINK before inspecting the actual
/// object. Private parent READ_CONTROL permits ordinary client SD inspection,
/// without granting private value enumeration. No intermediate link is followed.
fn open(
    parent: HKEY,
    name: &str,
    write: bool,
    transaction: Option<&Handle>,
) -> Result<Option<Key>> {
    let parts: Vec<_> = name.split('\\').collect();
    if parts.is_empty()
        || parts.len() > 8
        || parts
            .iter()
            .any(|p| p.is_empty() || p.contains(['/', ':', '\0']) || *p == "." || *p == "..")
    {
        return Err(Error::StateCorrupt);
    }
    let mut current = None;
    for (i, part) in parts.iter().enumerate() {
        let handle = current.as_ref().map_or(parent, |key: &Key| key.0);
        let rights = KEY_WOW64_64KEY
            | KEY_READ
            | if i + 1 == parts.len() && write {
                KEY_WRITE
            } else {
                0
            };
        let Some(key) = open_component(handle, part, rights, transaction)? else {
            return Ok(None);
        };
        inspect_registry(
            key.0,
            !(parent == HKEY_LOCAL_MACHINE && i == 0 && *part == "Software"),
        )?;
        current = Some(key);
    }
    Ok(current)
}
#[allow(unsafe_code)]
fn open_component(
    parent: HKEY,
    name: &str,
    rights: u32,
    transaction: Option<&Handle>,
) -> Result<Option<Key>> {
    let mut result = ptr::null_mut();
    let name = wide(name);
    let status = if let Some(transaction) = transaction {
        unsafe {
            RegOpenKeyTransactedW(
                parent,
                name.as_ptr(),
                REG_OPTION_OPEN_LINK,
                rights,
                &mut result,
                transaction.raw(),
                ptr::null(),
            )
        }
    } else {
        unsafe {
            RegOpenKeyExW(
                parent,
                name.as_ptr(),
                REG_OPTION_OPEN_LINK,
                rights,
                &mut result,
            )
        }
    };
    match status {
        0 => {
            let key = Key(result);
            if result.is_null() {
                return Err(Error::Unavailable);
            }
            // The traversed parent namespace contains no private values. It grants
            // read/query only, so every exact component can reject REG_LINK before
            // any child is opened. Head/manifest/history live in private sibling.
            if rights & KEY_QUERY_VALUE != 0 {
                let mut kind = 0;
                let mut size = 0;
                let value = wide("SymbolicLinkValue");
                let result = unsafe {
                    RegQueryValueExW(
                        key.0,
                        value.as_ptr(),
                        ptr::null(),
                        &mut kind,
                        ptr::null_mut(),
                        &mut size,
                    )
                };
                if result != ERROR_FILE_NOT_FOUND {
                    return Err(Error::SecurityRefused);
                }
            }
            Ok(Some(key))
        }
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Ok(None),
        _ => Err(Error::Unavailable),
    }
}
#[allow(unsafe_code)]
fn create(
    parent: HKEY,
    name: &str,
    transaction: &Handle,
    public_read: bool,
    fresh: bool,
) -> Result<Key> {
    if let Some(existing) = open(parent, name, true, Some(transaction))? {
        if fresh {
            return Err(Error::ScopeConflict);
        }
        return Ok(existing);
    }
    let security = NativeSecurity::registry(public_read)?;
    let attributes = security.attributes();
    let name = wide(name);
    let mut result = ptr::null_mut();
    let mut disposition = 0;
    let status = unsafe {
        RegCreateKeyTransactedW(
            parent,
            name.as_ptr(),
            0,
            ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE | KEY_WOW64_64KEY,
            &attributes,
            &mut result,
            &mut disposition,
            transaction.raw(),
            ptr::null(),
        )
    };
    if status != 0 || result.is_null() {
        return Err(Error::Unavailable);
    }
    let key = Key(result);
    if disposition != REG_CREATED_NEW_KEY {
        return Err(Error::ScopeConflict);
    }
    inspect_registry(result, true)?;
    let mut kind = 0;
    let mut size = 0;
    let link = wide("SymbolicLinkValue");
    if unsafe {
        RegQueryValueExW(
            result,
            link.as_ptr(),
            ptr::null(),
            &mut kind,
            ptr::null_mut(),
            &mut size,
        )
    } != ERROR_FILE_NOT_FOUND
    {
        return Err(Error::SecurityRefused);
    }
    Ok(key)
}
#[allow(unsafe_code)]
fn read(key: &Key, name: &str) -> Result<Vec<u8>> {
    let name = wide(name);
    let mut kind = 0;
    let mut size = 0;
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            ptr::null(),
            &mut kind,
            ptr::null_mut(),
            &mut size,
        )
    };
    if status != 0 || kind != REG_BINARY || size == 0 || size as usize > json::RECORD_MAX {
        return Err(Error::StateCorrupt);
    }
    let mut bytes = vec![0u8; size as usize];
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            ptr::null(),
            &mut kind,
            bytes.as_mut_ptr(),
            &mut size,
        )
    };
    if status == ERROR_MORE_DATA
        || status != 0
        || kind != REG_BINARY
        || size as usize != bytes.len()
    {
        return Err(Error::StateCorrupt);
    }
    Ok(bytes)
}
#[allow(unsafe_code)]
fn write(key: &Key, name: &str, bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() || bytes.len() > json::RECORD_MAX {
        return Err(Error::StateCorrupt);
    }
    let name = wide(name);
    if unsafe {
        RegSetValueExW(
            key.0,
            name.as_ptr(),
            0,
            REG_BINARY,
            bytes.as_ptr(),
            bytes.len() as u32,
        )
    } != 0
    {
        return Err(Error::Unavailable);
    }
    Ok(())
}

const METADATA_BOOTSTRAP: &str = "MetadataBootstrap";
const METADATA_BOOTSTRAP_ROOT: &str = "MetadataBootstrapRoot";
const PREPARATION_BOOTSTRAP: &str = "PreparationBootstrap";

/// A legacy record cannot hide partial genesis or silently acquire new owners.
#[allow(unsafe_code)]
fn require_absent_value(key: &Key, name: &str) -> Result<()> {
    let name = wide(name);
    let mut size = 0;
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut size,
        )
    };
    if status != ERROR_FILE_NOT_FOUND {
        return Err(Error::StateCorrupt);
    }
    Ok(())
}

fn owner_bootstrap_in(key: &Key, seed: &Seed, record: &HostRegistration) -> Result<()> {
    if !crate::bootstrap::initialized(&record.known_owner_namespaces) {
        for name in [
            METADATA_BOOTSTRAP,
            METADATA_BOOTSTRAP_ROOT,
            PREPARATION_BOOTSTRAP,
        ] {
            require_absent_value(key, name)?;
        }
        return Ok(());
    }
    OwnerBootstrap::new(seed, &record.installation_scope_id)?.check_existing(
        &read(key, METADATA_BOOTSTRAP)?,
        &read(key, METADATA_BOOTSTRAP_ROOT)?,
        &read(key, PREPARATION_BOOTSTRAP)?,
    )
}

fn decode_record(bytes: &[u8], seed: &Seed) -> Result<HostRegistration> {
    let record: HostRegistration = json::decode(bytes, false)?;
    record.validate(seed)?;
    if json::encode(&record, false)? != bytes {
        return Err(Error::StateCorrupt);
    }
    record.validate(seed)?;
    Ok(record)
}
fn revision_name(revision: u64) -> String {
    format!("{revision:016}")
}

fn inventory_name(index: usize) -> Result<String> {
    if index >= MAX_SHARDS {
        return Err(Error::StateCorrupt);
    }
    Ok(format!("ManifestInventory-{index}"))
}
/// Registry value names use native case-insensitive comparison. Detect even
/// Unicode aliases of the reserved prefix before enforcing the exact spelling.
fn inventory_slot(name: &str, used: usize) -> Result<Option<usize>> {
    if used == 0 || used > MAX_SHARDS {
        return Err(Error::StateCorrupt);
    }
    let prefix = "ManifestInventory-";
    let candidate: String = name.chars().take(prefix.len()).collect();
    if super::layout::ordinal_equal(&candidate, prefix)? {
        (0..used)
            .find(|slot| name == format!("ManifestInventory-{slot}"))
            .map(Some)
            .ok_or(Error::StateCorrupt)
    } else {
        Ok(None)
    }
}
#[allow(unsafe_code)]
fn reserved_inventory_values(key: &Key, used: usize) -> Result<()> {
    if used == 0 || used > MAX_SHARDS {
        return Err(Error::StateCorrupt);
    }
    let mut seen = [false; MAX_SHARDS];
    for index in 0..=4096 {
        let mut name = [0u16; 1024];
        let mut length = name.len() as u32;
        let status = unsafe {
            RegEnumValueW(
                key.0,
                index,
                name.as_mut_ptr(),
                &mut length,
                ptr::null(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        if status == windows_sys::Win32::Foundation::ERROR_NO_MORE_ITEMS {
            if seen[..used].iter().all(|value| *value) && seen[used..].iter().all(|value| !*value) {
                return Ok(());
            }
            return Err(Error::StateCorrupt);
        }
        if status != 0 || index == 4096 || length as usize >= name.len() {
            return Err(Error::StateCorrupt);
        }
        let name = String::from_utf16(&name[..length as usize]).map_err(|_| Error::StateCorrupt)?;
        if let Some(found) = inventory_slot(&name, used)? {
            if seen[found] {
                return Err(Error::StateCorrupt);
            }
            seen[found] = true;
        }
    }
    Err(Error::StateCorrupt)
}

/// Actual bounded registry payloads, not a service/source capability. The sealed
/// StartedServiceContext getter authenticates custody and current incarnation.
pub(super) struct OriginalRootRead {
    pub(super) metadata_record: Vec<u8>,
    pub(super) root: Vec<u8>,
}

pub(super) struct Store {
    path: String,
}
impl Store {
    pub(super) fn new(seed: &Seed) -> Self {
        Self {
            path: format!(
                "Software\\Pinvou\\{}\\{}",
                seed.updater_leaf(),
                seed.profile_id
            ),
        }
    }
    fn key(&self, write: bool, transaction: Option<&Handle>) -> Result<Option<Key>> {
        let key = open(
            HKEY_LOCAL_MACHINE,
            &format!("{}\\private", self.path),
            write,
            transaction,
        )?;
        if let Some(key) = &key {
            inspect_registry(key.0, true)?;
        }
        Ok(key)
    }
    pub(super) fn read(&self, seed: &Seed) -> Result<Option<HostRegistration>> {
        self.read_in(seed, None)
    }
    fn read_in(
        &self,
        seed: &Seed,
        transaction: Option<&Handle>,
    ) -> Result<Option<HostRegistration>> {
        // Only complete absence of the profile leaf is a fresh namespace.
        // An orphan public/private fragment must never be adopted/reset.
        let Some(_profile) = open(HKEY_LOCAL_MACHINE, &self.path, false, transaction)? else {
            return Ok(None);
        };
        let key = self.key(false, transaction)?.ok_or(Error::StateCorrupt)?;
        let bytes = read(&key, "Head")?;
        let record = decode_record(&bytes, seed)?;
        let history = open(
            key.0,
            &format!("history\\{}", revision_name(record.revision)),
            false,
            transaction,
        )?
        .ok_or(Error::StateCorrupt)?;
        inspect_registry(history.0, true)?;
        if read(&history, "Record")? != bytes {
            return Err(Error::StateCorrupt);
        }
        if record.revision > 1 {
            let previous = open(
                key.0,
                &format!("history\\{}", revision_name(record.revision - 1)),
                false,
                transaction,
            )?
            .ok_or(Error::StateCorrupt)?;
            inspect_registry(previous.0, true)?;
            let bytes = read(&previous, "Record")?;
            let prior = decode_record(&bytes, seed)?;
            if prior.revision + 1 != record.revision
                || Some(json::sha256(&bytes)) != record.previous_record_sha256
                || prior.installation_scope_id != record.installation_scope_id
                || prior.known_owner_namespaces != record.known_owner_namespaces
                || prior.genesis_manifest_sha256 != record.genesis_manifest_sha256
                || json::encode(&prior.original_installer_identity, false)?
                    != json::encode(&record.original_installer_identity, false)?
            {
                return Err(Error::StateCorrupt);
            }
        }
        let public = self.public_key(transaction)?.ok_or(Error::StateCorrupt)?;
        if read(&public, "Head")? != json::encode(&record.public(), false)? {
            return Err(Error::StateCorrupt);
        }
        self.inventory_in(&key, seed, &record)?;
        owner_bootstrap_in(&key, seed, &record)?;
        Ok(Some(record))
    }
    /// Every private read authenticates the SAME complete committed inventory.
    /// Values cannot be lazily filled from a source installer or public DTO.
    fn inventory_in(
        &self,
        key: &Key,
        seed: &Seed,
        record: &HostRegistration,
    ) -> Result<InstallationManifest> {
        let bytes = read(key, "Manifest")?;
        if json::sha256(&bytes) != record.genesis_manifest_sha256 {
            return Err(Error::StateCorrupt);
        }
        let root = RootManifest::parse(&bytes, seed).map_err(|_| Error::StateCorrupt)?;
        reserved_inventory_values(key, root.app_inventory.shards.len())?;
        let mut shards = Vec::with_capacity(root.app_inventory.shards.len());
        for index in 0..root.app_inventory.shards.len() {
            shards.push(read(key, &inventory_name(index)?)?);
        }
        let manifest = InventoryBytes {
            root: bytes,
            shards,
        }
        .parse(seed)
        .map_err(|_| Error::StateCorrupt)?;
        super::layout::validate_native_catalog(&manifest.app_files)
            .map_err(|_| Error::StateCorrupt)?;
        Ok(manifest)
    }
    /// Read the original three bootstraps from one held private key. The caller
    /// supplies its actual Store.read snapshot, never a replacement Root/pin.
    /// Head is checked both before and after these reads; a concurrent legitimate
    /// status update safely requires recapture, not mutation or genesis repair.
    pub(super) fn original_root(
        &self,
        seed: &Seed,
        expected: &HostRegistration,
    ) -> Result<OriginalRootRead> {
        expected.validate(seed)?;
        if !crate::bootstrap::initialized(&expected.known_owner_namespaces) {
            return Err(Error::InitialTrustUnavailable);
        }
        let key = self
            .key(false, None)?
            .ok_or(Error::InitialTrustUnavailable)?;
        let head = read(&key, "Head")?;
        let actual = decode_record(&head, seed)?;
        if actual.digest()? != expected.digest()? {
            return Err(Error::ScopeConflict);
        }
        let metadata_record = read(&key, METADATA_BOOTSTRAP)?;
        let root = read(&key, METADATA_BOOTSTRAP_ROOT)?;
        let ownership = read(&key, PREPARATION_BOOTSTRAP)?;
        // Expected genesis only verifies the actual stored bytes. Never return
        // expected.metadata.root_bytes()/Seed bytes instead of the actual read.
        OwnerBootstrap::new(seed, &actual.installation_scope_id)?.check_existing(
            &metadata_record,
            &root,
            &ownership,
        )?;
        if read(&key, "Head")? != head {
            return Err(Error::ScopeConflict);
        }
        Ok(OriginalRootRead {
            metadata_record,
            root,
        })
    }

    pub(super) fn manifest(
        &self,
        seed: &Seed,
        record: &HostRegistration,
    ) -> Result<InstallationManifest> {
        let key = self.key(false, None)?.ok_or(Error::StateCorrupt)?;
        self.inventory_in(&key, seed, record)
    }
    fn public_key(&self, transaction: Option<&Handle>) -> Result<Option<Key>> {
        open(
            HKEY_LOCAL_MACHINE,
            &format!("{}\\public", self.path),
            false,
            transaction,
        )
    }
    pub(super) fn read_public(&self, seed: &Seed) -> Result<PublicRegistration> {
        let key = self.public_key(None)?.ok_or(Error::SetupIncomplete)?;
        inspect_registry(key.0, true)?;
        let bytes = read(&key, "Head")?;
        let record: PublicRegistration = json::decode(&bytes, false)?;
        if json::encode(&record, false)? != bytes {
            return Err(Error::StateCorrupt);
        }
        record.validate(seed)?;
        Ok(record)
    }
    /// Serial callers hold the checked short native mutex, never across IO/SCM.
    /// Original key/head is read INSIDE this same registry transaction for CAS.
    #[allow(unsafe_code)]
    pub(super) fn commit(
        &self,
        seed: &Seed,
        expected: Option<&HostRegistration>,
        next: &HostRegistration,
        initial_manifest: Option<&InventoryBytes>,
    ) -> Result<()> {
        next.validate(seed)?;
        // Both owners are initialized only by a genuinely new original intent.
        // Existing/unknown outcomes are read back, never filled or reset.
        let initial_owners = if expected.is_none() {
            if !crate::bootstrap::initialized(&next.known_owner_namespaces) {
                return Err(Error::StateCorrupt);
            }
            Some(OwnerBootstrap::new(seed, &next.installation_scope_id)?)
        } else {
            None
        };
        if let Some(input) = initial_manifest {
            let parsed = input.parse(seed).map_err(|_| Error::StateCorrupt)?;
            super::layout::validate_native_catalog(&parsed.app_files)?;
            if json::sha256(&input.root) != next.genesis_manifest_sha256
                || parsed.helper != next.helper
                || parsed.launcher != next.launcher
                || parsed.app != next.app
            {
                return Err(Error::StateCorrupt);
            }
        }
        let transaction = Handle::owned(unsafe {
            CreateTransaction(ptr::null_mut(), ptr::null_mut(), 0, 0, 0, 0, ptr::null())
        })?;
        let actual = self.read_in(seed, Some(&transaction))?;
        match (expected, actual.as_ref()) {
            (None, None) if next.revision == 1 && initial_manifest.is_some() => {}
            (Some(expected), Some(actual))
                if expected.digest()? == actual.digest()?
                    && next.revision == actual.revision + 1
                    && next.previous_record_sha256 == Some(actual.digest()?)
                    && next.installation_scope_id == actual.installation_scope_id
                    && next.genesis_seed_sha256 == actual.genesis_seed_sha256
                    && next.genesis_manifest_sha256 == actual.genesis_manifest_sha256
                    && next.known_owner_namespaces == actual.known_owner_namespaces
                    && json::encode(&next.original_installer_identity, false)?
                        == json::encode(&actual.original_installer_identity, false)?
                    && (actual
                        .service_identity
                        .creation_operation_id
                        .as_ref()
                        .is_none_or(|id| {
                            next.service_identity.creation_operation_id.as_ref() == Some(id)
                        })
                        || actual.phase == crate::registration::Phase::RemovedTombstone
                            && next.phase == crate::registration::Phase::Provisioning
                            && next.last_transition.kind == "reinstall-same-build"
                            && next.service_identity.creation_operation_id.as_ref()
                                == Some(&next.installer_identity.operation_id))
                    && next.helper == actual.helper
                    && next.launcher == actual.launcher
                    && next.app == actual.app
                    && next.source_set == actual.source_set
                    && actual
                        .service_identity
                        .service_sid
                        .as_ref()
                        .is_none_or(|sid| {
                            next.service_identity.service_sid.as_ref() == Some(sid)
                        })
                    && (next.host_epoch == actual.host_epoch
                        && next.startup_attestation == actual.startup_attestation
                        || next.host_epoch == actual.host_epoch + 1
                            && next.last_transition.kind == "startup-incarnation") => {}
            _ => return Err(Error::ScopeConflict),
        }
        let mut parent = open(HKEY_LOCAL_MACHINE, "Software", true, Some(&transaction))?
            .ok_or(Error::Unavailable)?;
        inspect_registry(parent.0, false)?;
        for part in ["Pinvou", seed.updater_leaf(), seed.profile_id.as_str()] {
            parent = create(
                parent.0,
                part,
                &transaction,
                true,
                part == seed.profile_id && expected.is_none(),
            )?;
        }
        let public = create(parent.0, "public", &transaction, true, expected.is_none())?;
        let private = create(parent.0, "private", &transaction, false, expected.is_none())?;
        let history_root = create(
            private.0,
            "history",
            &transaction,
            false,
            expected.is_none(),
        )?;
        if open(
            history_root.0,
            &revision_name(next.revision),
            false,
            Some(&transaction),
        )?
        .is_some()
        {
            return Err(Error::StateCorrupt);
        }
        let history_revision = create(
            history_root.0,
            &revision_name(next.revision),
            &transaction,
            false,
            true,
        )?;
        let bytes = json::encode(next, false)?;
        write(&private, "Head", &bytes)?;
        write(&history_revision, "Record", &bytes)?;
        write(&public, "Head", &json::encode(&next.public(), false)?)?;
        if let Some(owners) = initial_owners {
            // Same native transaction as the first scope, intent and manifest.
            // The owner modules define and validate their bounded genesis bytes.
            write(&private, METADATA_BOOTSTRAP, owners.metadata.record_bytes())?;
            write(
                &private,
                METADATA_BOOTSTRAP_ROOT,
                owners.metadata.root_bytes(),
            )?;
            write(
                &private,
                PREPARATION_BOOTSTRAP,
                owners.ownership.record_bytes(),
            )?;
            owner_bootstrap_in(&private, seed, next)?;
            time::initialize_in(&private, &transaction, next)?;
        }
        if let Some(manifest) = initial_manifest {
            if expected.is_some() || json::sha256(&manifest.root) != next.genesis_manifest_sha256 {
                return Err(Error::StateCorrupt);
            }
            write(&private, "Manifest", &manifest.root)?;
            for (index, bytes) in manifest.shards.iter().enumerate() {
                write(&private, &inventory_name(index)?, bytes)?;
            }
            // Root and every used shard are in this SAME native transaction.
            // Read exact namespace/payloads before closing all tx handles.
            self.inventory_in(&private, seed, next)?;
        }
        // Every native transaction key is closed BEFORE the definitive commit.
        drop(public);
        drop(history_revision);
        drop(history_root);
        drop(private);
        drop(parent);
        if unsafe { CommitTransaction(transaction.raw()) } == 0 {
            return Err(Error::StateCommitUnknown);
        }
        drop(transaction);
        // Flush ONLY fresh real committed nontransacted handles. Never claim
        // directory/power-loss certification, nor retry an ambiguous commit.
        let verify_committed = || -> Result<()> {
            let key = self.key(false, None)?.ok_or(Error::StateCommitUnknown)?;
            let public = self.public_key(None)?.ok_or(Error::StateCommitUnknown)?;
            let history = open(
                key.0,
                &format!("history\\{}", revision_name(next.revision)),
                false,
                None,
            )?
            .ok_or(Error::StateCommitUnknown)?;
            if unsafe { RegFlushKey(key.0) } != 0
                || unsafe { RegFlushKey(public.0) } != 0
                || unsafe { RegFlushKey(history.0) } != 0
            {
                return Err(Error::StateCommitUnknown);
            }
            let actual = self.read(seed)?.ok_or(Error::StateCommitUnknown)?;
            if actual.digest()? != next.digest()? {
                return Err(Error::StateCommitUnknown);
            }
            if expected.is_none() {
                // Fresh time genesis is part of the original definitive commit,
                // not a later best-effort initializer or an adopted empty scope.
                let time = self.time_load(seed, &actual)?;
                if time.state.revision != 1
                    || time.state.floor.is_some()
                    || !time.state.cleanup_pending
                {
                    return Err(Error::StateCommitUnknown);
                }
            }
            Ok(())
        };
        // A successful native COMMIT is the side-effect boundary. EVERY later
        // reopen/ACL/query/flush/readback/encoding error is outcome-unknown,
        // never an ordinary no-write error that could invite a fresh operation.
        verify_committed().map_err(|_| Error::StateCommitUnknown)
    }
}

/// !Send: mutex ownership belongs to the thread that successfully waited.
pub(super) struct BootstrapGuard {
    handle: Handle,
    _thread: PhantomData<Rc<()>>,
    pub(super) abandoned: bool,
}
impl BootstrapGuard {
    #[allow(unsafe_code)]
    pub(super) fn acquire(seed: &Seed, lifetime: bool) -> Result<Self> {
        use windows_sys::Win32::Security::Authorization::SE_KERNEL_OBJECT;
        let security = NativeSecurity::mutex()?;
        let attributes = security.attributes();
        let name = wide(&seed.bootstrap_mutex_name(lifetime));
        let handle = Handle::owned(unsafe {
            CreateMutexExW(
                &attributes,
                name.as_ptr(),
                0,
                MUTEX_MODIFY_STATE | SYNCHRONIZE | 0x00020000,
            )
        })?;
        super::security::inspect_kernel(handle.raw(), SE_KERNEL_OBJECT)?;
        let abandoned = match unsafe { WaitForSingleObject(handle.raw(), 0) } {
            WAIT_OBJECT_0 => false,
            WAIT_ABANDONED => true,
            WAIT_TIMEOUT => return Err(Error::Busy),
            _ => return Err(Error::Unavailable),
        };
        Ok(Self {
            handle,
            _thread: PhantomData,
            abandoned,
        })
    }
}
impl Drop for BootstrapGuard {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.handle.raw());
        }
    }
}

#[cfg(test)]
mod inventory_namespace_tests {
    use super::*;
    #[test]
    fn reserved_inventory_namespace_uses_native_comparison_then_exact_fixed_slot() {
        assert_eq!(inventory_slot("Head", 3).unwrap(), None);
        assert_eq!(inventory_slot("ManifestInventory-0", 3).unwrap(), Some(0));
        for name in [
            "ManifestInventory-3",
            "ManifestInventory-8",
            "ManifestInventory-00",
            "MANIFESTINVENTORY-0",
            "ManifestInventory-other",
        ] {
            assert!(inventory_slot(name, 3).is_err());
        }
        let alias = "Manife\u{17f}tInventory-7";
        let prefix: String = alias.chars().take("ManifestInventory-".len()).collect();
        if super::super::layout::ordinal_equal(&prefix, "ManifestInventory-").unwrap() {
            assert!(inventory_slot(alias, 3).is_err());
        }
        assert!(inventory_slot("ManifestInventory-0", 0).is_err());
    }
}

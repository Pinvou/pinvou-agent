//! Original elevated source-installer owns native intent and precise stage CAS.
//! Neither a manifest DTO nor an IPC caller can construct this installation path.
use super::{
    image::{self, SealedImage},
    layout::{ProtectedDirectory, Targets},
    pipe::nonce,
    process::prove_prior_installer_gone,
    registry::{BootstrapGuard, Store},
    scm,
    source::SourceInstallerContext as Source,
};
use crate::{
    error::{Error, Result},
    json,
    registration::{HostRegistration, InstallerIdentity, Phase},
};

fn installer(source: &Source, operation: String) -> InstallerIdentity {
    InstallerIdentity {
        process_id: source.installer.selector.process_id,
        creation_file_time: source.installer.selector.creation_file_time,
        token_user_sid: source.installer.token.user.sid.clone(),
        logon_authentication_id: source.installer.token.user.logon,
        session_id: source.installer.token.user.session_id,
        source_nsis_sha256: source.installer_image.sha256.clone(),
        source_nsis_file_identity: source.installer_image.identity,
        manifest_sha256: source.manifest_sha256.clone(),
        operation_id: operation,
    }
}
fn same_installer(record: &HostRegistration, source: &Source) -> bool {
    let original = &record.installer_identity;
    original.process_id == source.installer.selector.process_id
        && original.creation_file_time == source.installer.selector.creation_file_time
        && original.token_user_sid == source.installer.token.user.sid
        && original.logon_authentication_id == source.installer.token.user.logon
        && original.session_id == source.installer.token.user.session_id
        && original.source_nsis_file_identity == source.installer_image.identity
        && original.source_nsis_sha256 == source.installer_image.sha256
}
fn bound(record: &HostRegistration, source: &Source) -> Result<()> {
    record.validate(&source.seed)?;
    if record.genesis_manifest_sha256 != source.manifest_sha256
        || record.source_set.nsis_sha256 != source.installer_image.sha256
        || record.helper != source.manifest.helper
        || record.launcher != source.manifest.launcher
        || record.app != source.manifest.app
    {
        return Err(Error::ScopeConflict);
    }
    if json::encode(&source.manifest.root, false)? != source.manifest_bytes.root
        || source.manifest_bytes.parse(&source.seed)?.app_files != source.manifest.app_files
    {
        return Err(Error::ImageRefused);
    }
    source.recheck()
}
fn claim(source: &Source, store: &Store, record: HostRegistration) -> Result<HostRegistration> {
    bound(&record, source)?;
    // Revision one is the original first-install intent, including an unknown
    // commit outcome. Old unfinished revision-one installs are indistinguishable
    // and must not be silently initialized or adopted here.
    if record.revision == 1 {
        store.verify_initial_metadata(&source.seed, &record)?;
    }
    if same_installer(&record, source) {
        return Ok(record);
    }
    let identity = &record.installer_identity;
    if !prove_prior_installer_gone(&crate::identity::ProcessSelector {
        process_id: identity.process_id,
        creation_file_time: identity.creation_file_time,
    })? {
        return Err(Error::Busy);
    }
    let mut next = record.next(record.phase, "takeover-original-source")?;
    next.installer_identity = installer(source, nonce()?);
    next.last_transition.operation_id = next.installer_identity.operation_id.clone();
    store.commit(&source.seed, Some(&record), &next, None)?;
    Ok(next)
}
fn copy_member(
    source: &ProtectedDirectory,
    source_name: &str,
    target: &ProtectedDirectory,
    target_name: &str,
    size: u64,
    hash: &str,
    repair: bool,
) -> Result<()> {
    // Scope/source/closed inventory and persisted original intent were proved
    // by the sole caller. This never repairs an ACL, reparse object or link.
    let mut retained_source = source.relative_file(source_name)?;
    image::verify_bytes(&mut retained_source, size, hash)?;
    let (parent, leaf) = target.relative_parent(target_name)?;
    let parent = parent.as_ref().unwrap_or(target);
    match parent.file(&leaf) {
        Ok(mut existing) => {
            let identity = pinvou_upgrade_custody::identity(&existing)
                .map_err(super::layout::custody_error)?;
            match image::verify_bytes(&mut existing, size, hash) {
                Ok(_) => return Ok(()),
                Err(Error::ImageRefused) if repair => {
                    drop(existing);
                    // A live image/read-only lease denies DELETE. No force,
                    // POSIX disposition, arbitrary fallback or termination.
                    parent.remove_file_exact(&leaf, &identity)?;
                }
                Err(error) => return Err(error),
            }
        }
        Err(Error::SourceUnavailable) => {}
        Err(error) => return Err(error),
    }
    image::copy_exact(source, source_name, target, target_name, size, hash)
}
fn copy_files(source: &Source, targets: &Targets) -> Result<()> {
    source.recheck()?;
    targets.validate_known_contents(&source.seed, &source.manifest, false)?;
    let helper = targets.code.child("helpers", true, false, None)?.child(
        &source.seed.build_id,
        true,
        false,
        None,
    )?;
    copy_member(
        &source.source,
        "pinvou-upgrade-helper.exe",
        &helper,
        "pinvou-upgrade-helper.exe",
        source.manifest.helper.size,
        &source.manifest.helper.sha256,
        true,
    )?;
    copy_member(
        &source.source,
        "pinvou-upgrade-launcher.exe",
        &targets.code,
        "pinvou-upgrade-launcher.exe",
        source.manifest.launcher.size,
        &source.manifest.launcher.sha256,
        true,
    )?;
    let app = source.source.child("app", false, false, None)?;
    for entry in &source.manifest.app_files {
        copy_member(
            &app,
            &entry.relative_name,
            &targets.app,
            &entry.relative_name,
            entry.size,
            &entry.sha256,
            true,
        )?;
    }
    source.recheck()
}
fn original_service(source: &Source) -> Result<Option<super::process::RetainedProcess>> {
    if !scm::exists(&source.seed)? {
        return Ok(None);
    }
    let current = Store::new(&source.seed)
        .read(&source.seed)?
        .ok_or(Error::StateCorrupt)?;
    bound(&current, source)?;
    if !same_installer(&current, source) || current.service_identity.creation_operation_id.is_none()
    {
        return Err(Error::ServiceIdentityRefused);
    }
    match scm::live_process(&source.seed, &source.layout) {
        Ok(process) => Ok(Some(process)),
        Err(error) => {
            if scm::original_stopped(&source.seed, &source.layout, true)? {
                Ok(None)
            } else {
                Err(error)
            }
        }
    }
}
fn quiesce_original(source: &Source) -> Result<()> {
    use std::time::{Duration, Instant};
    source.recheck()?;
    let process = original_service(source)?;
    super::references::no_image_references(
        source,
        process.as_ref().map(|process| &process.selector),
    )?;
    let current = Store::new(&source.seed)
        .read(&source.seed)?
        .ok_or(Error::StateCorrupt)?;
    bound(&current, source)?;
    if !same_installer(&current, source) {
        return Err(Error::ScopeConflict);
    }
    scm::request_stop(
        &source.seed,
        &source.layout,
        current.service_identity.creation_operation_id.is_some(),
    )?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        source.recheck()?;
        if scm::stopped(&source.seed)?
            && process
                .as_ref()
                .map(|process| process.exited())
                .transpose()?
                .unwrap_or(true)
        {
            break;
        }
        if Instant::now() >= deadline {
            return Err(Error::Referenced);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(process);
    super::references::no_image_references(source, None)
}
fn prepare_copy_phase(
    source: &Source,
    store: &Store,
    original: HostRegistration,
) -> Result<HostRegistration> {
    if original.phase == Phase::RemovedTombstone {
        super::preflight::runtime(source)?;
        // Original protected tombstone and native absence are both required.
        // Only the current creation intent is rotated; every prior intent,
        // scope/genesis/epoch remains in the immutable native history chain.
        if source.layout.any_target_exists()? || scm::exists(&source.seed)? {
            return Err(Error::Referenced);
        }
        let _guard = BootstrapGuard::acquire(&source.seed, false)?;
        source.recheck()?;
        let next = original.reinstall_same_build()?;
        store.commit(&source.seed, Some(&original), &next, None)?;
        return Ok(next);
    }
    if matches!(
        original.phase,
        Phase::Ready | Phase::FilesVerified | Phase::ServiceConfigured | Phase::RepairRequired
    ) {
        source
            .layout
            .existing_targets(original.service_identity.service_sid.as_deref())?
            .validate_known_contents(&source.seed, &source.manifest)?;
        let service = original_service(source)?;
        super::references::no_image_references(
            source,
            service.as_ref().map(|process| &process.selector),
        )?;
        let next = {
            let _guard = BootstrapGuard::acquire(&source.seed, false)?;
            source.recheck()?;
            let next = original.next(Phase::Repairing, "repair-intent")?;
            store.commit(&source.seed, Some(&original), &next, None)?;
            next
        };
        drop(service);
        quiesce_original(source)?;
        return Ok(next);
    }
    if !matches!(original.phase, Phase::Provisioning | Phase::Repairing) {
        return Err(Error::Referenced);
    }
    if original.phase == Phase::Repairing {
        quiesce_original(source)?;
    }
    Ok(original)
}
/// First durable native intent precedes every target directory/file/SCM effect.
/// A source invocation remains necessary after a process/crash boundary.
pub(super) fn prepare_install() -> Result<()> {
    prepare_install_source(Source::from_actual_invocation()?)
}
pub(super) fn prepare_local_install() -> Result<()> {
    prepare_install_source(Source::from_local_initial_invocation()?)
}
fn prepare_install_source(mut source: Source) -> Result<()> {
    let store = Store::new(&source.seed);
    let previously_registered = store.read(&source.seed)?.is_some();
    if !previously_registered {
        super::preflight::first_install(&source)?;
    }
    let original = {
        let _guard = BootstrapGuard::acquire(&source.seed, false)?;
        source.recheck()?;
        match store.read(&source.seed)? {
            Some(record) => {
                source.require_existing_intent()?;
                claim(&source, &store, record)?
            }
            None => {
                if previously_registered {
                    return Err(Error::ScopeConflict);
                }
                // These are read-only native namespace probes. No pathname
                // exists() result is promoted into a protected root/context.
                if source.layout.any_target_exists()? || scm::exists(&source.seed)? {
                    return Err(Error::ScopeConflict);
                }
                let local_initial_origin = source.consume_initial_origin()?;
                let record = HostRegistration::initial(
                    &source.seed,
                    &source.manifest,
                    nonce()?,
                    installer(&source, nonce()?),
                    local_initial_origin,
                )?;
                store.commit(&source.seed, None, &record, Some(&source.manifest_bytes))?;
                record
            }
        }
    };
    let original = prepare_copy_phase(&source, &store, original)?;
    let targets = source.layout.create_targets(
        &source.seed,
        original.service_identity.service_sid.as_deref(),
    )?;
    copy_files(&source, &targets)?;
    // prepare-install does not claim files_verified, SCM identity, or Ready.
    Ok(())
}
pub(super) fn verify_files() -> Result<()> {
    let source = Source::from_actual_invocation()?;
    let store = Store::new(&source.seed);
    let original = {
        let _guard = BootstrapGuard::acquire(&source.seed, false)?;
        let record = store.read(&source.seed)?.ok_or(Error::SetupIncomplete)?;
        claim(&source, &store, record)?
    };
    if !matches!(
        original.phase,
        Phase::Provisioning | Phase::Repairing | Phase::RepairRequired | Phase::FilesVerified
    ) {
        return Err(Error::ScopeConflict);
    }
    let targets = source
        .layout
        .open_targets_optional(original.service_identity.service_sid.as_deref())?;
    targets.validate_known_contents(&source.seed, &source.manifest, true)?;
    for entry in &source.manifest.app_files {
        let mut file = targets.app.relative_file(&entry.relative_name)?;
        image::verify_bytes(&mut file, entry.size, &entry.sha256)?;
    }
    let helper = targets.code.child("helpers", false, false, None)?.child(
        &source.seed.build_id,
        false,
        false,
        None,
    )?;
    let _helper = SealedImage::open(
        &helper,
        "pinvou-upgrade-helper.exe",
        &source.manifest.helper,
        &source.seed,
    )?;
    let _launcher = SealedImage::open(
        &targets.code,
        "pinvou-upgrade-launcher.exe",
        &source.manifest.launcher,
        &source.seed,
    )?;
    let _app = SealedImage::open(
        &targets.app,
        "pinvou3-tauri.exe",
        &source.manifest.app,
        &source.seed,
    )?;
    let _guard = BootstrapGuard::acquire(&source.seed, false)?;
    source.recheck()?;
    if original.phase != Phase::FilesVerified {
        let next = original.next(Phase::FilesVerified, "verify-files")?;
        store.commit(&source.seed, Some(&original), &next, None)?;
    } else if store
        .read(&source.seed)?
        .ok_or(Error::StateCorrupt)?
        .digest()?
        != original.digest()?
    {
        return Err(Error::ScopeConflict);
    }
    Ok(())
}

/// Persist original service creation provenance BEFORE the SCM side effect.
/// An existing same-named service without that original intent is never adopted.
pub(super) fn configure_service(source: &Source) -> Result<HostRegistration> {
    let store = Store::new(&source.seed);
    let original = {
        let _guard = BootstrapGuard::acquire(&source.seed, false)?;
        let record = store.read(&source.seed)?.ok_or(Error::SetupIncomplete)?;
        if !matches!(
            record.phase,
            Phase::FilesVerified | Phase::ServiceConfigured
        ) {
            return Err(Error::ScopeConflict);
        }
        let record = claim(source, &store, record)?;
        if record.service_identity.creation_operation_id.is_some() {
            record
        } else {
            if scm::exists(&source.seed)? {
                return Err(Error::ScopeConflict);
            }
            let mut next = record.next(record.phase, "service-create-intent")?;
            next.service_identity.creation_operation_id =
                Some(next.installer_identity.operation_id.clone());
            store.commit(&source.seed, Some(&record), &next, None)?;
            next
        }
    };
    source.recheck()?;
    let targets = source
        .layout
        .open_targets_optional(original.service_identity.service_sid.as_deref())?;
    let helper = targets.code.child("helpers", false, false, None)?.child(
        &source.seed.build_id,
        false,
        false,
        None,
    )?;
    let _helper = SealedImage::open(
        &helper,
        "pinvou-upgrade-helper.exe",
        &original.helper,
        &source.seed,
    )?;
    let sid = scm::configure_original(&source.seed, &source.layout, true, |stage| {
        super::acceptance::checkpoint(&source, stage)
    })?;
    let _guard = BootstrapGuard::acquire(&source.seed, false)?;
    source.recheck()?;
    let mut next = original.next(Phase::ServiceConfigured, "service-configured")?;
    if original
        .service_identity
        .service_sid
        .as_ref()
        .is_some_and(|prior| prior != &sid)
    {
        return Err(Error::ServiceIdentityRefused);
    }
    next.service_identity.service_sid = Some(sid);
    store.commit(&source.seed, Some(&original), &next, None)?;
    Ok(next)
}

/// A short setup-control wait is separate from copying/preparation/downloads.
/// Timeout preserves ServiceConfigured and its original intent for explicit
/// recovery; it never grants Ready or stops an ambiguously started process.
pub(super) fn start_service() -> Result<()> {
    use std::time::{Duration, Instant};
    let source = Source::from_actual_invocation()?;
    let store = Store::new(&source.seed);
    let current = {
        let _guard = BootstrapGuard::acquire(&source.seed, false)?;
        let record = store.read(&source.seed)?.ok_or(Error::SetupIncomplete)?;
        if !matches!(
            record.phase,
            Phase::FilesVerified | Phase::ServiceConfigured | Phase::Ready
        ) {
            return Err(Error::ScopeConflict);
        }
        claim(&source, &store, record)?
    };
    if current.phase == Phase::Ready {
        return super::bootstrap::ready_status(&source, &current);
    }
    if current.phase == Phase::FilesVerified {
        configure_service(&source)?;
    }
    source.recheck()?;
    scm::start(&source.seed, &source.layout)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        source.recheck()?;
        if Instant::now() >= deadline {
            return Err(Error::SetupIncomplete);
        }
        let current = store.read(&source.seed)?.ok_or(Error::StateCorrupt)?;
        bound(&current, &source)?;
        if !same_installer(&current, &source) || current.phase != Phase::ServiceConfigured {
            return Err(Error::ScopeConflict);
        }
        let running = match scm::live_process(&source.seed, &source.layout) {
            Ok(process) => process,
            Err(Error::SetupIncomplete) => {
                if scm::stopped(&source.seed)? {
                    return Err(Error::SetupIncomplete);
                }
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(error) => return Err(error),
        };
        let attested = current
            .startup_attestation
            .as_ref()
            .is_some_and(|attestation| {
                attestation.process_id == running.selector.process_id
                    && attestation.creation_file_time == running.selector.creation_file_time
                    && attestation.host_epoch == current.host_epoch
                    && attestation.observed_registration_revision < current.revision
            });
        if !attested {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        match super::bootstrap::probe(&source, &current) {
            Ok(()) => {}
            // Read-only setup probe may start just before the service creates
            // its first pipe. No mutation or grant is retried by this branch.
            Err(Error::Unavailable) => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(error) => return Err(error),
        }
        if Instant::now() >= deadline {
            return Err(Error::SetupIncomplete);
        }
        let next = {
            let _guard = BootstrapGuard::acquire(&source.seed, false)?;
            source.recheck()?;
            if running.exited()?
                || scm::live_process(&source.seed, &source.layout)?.selector != running.selector
            {
                return Err(Error::ServiceIdentityRefused);
            }
            let actual = store.read(&source.seed)?.ok_or(Error::StateCorrupt)?;
            if actual.digest()? != current.digest()? {
                return Err(Error::ScopeConflict);
            }
            let next = actual.next(Phase::Ready, "ready")?;
            store.commit(&source.seed, Some(&actual), &next, None)?;
            next
        };
        // Setup success alone is insufficient. Prove a new normal session on
        // the exact Ready projection before the original installer succeeds.
        return super::bootstrap::ready_status(&source, &next);
    }
}

/// Explicit original-source removal only. No arbitrary process termination or
/// reference reset is performed, and the registration tombstone is retained.
pub(super) fn prepare_remove() -> Result<()> {
    let source = Source::from_actual_invocation()?;
    let store = Store::new(&source.seed);
    let original = {
        let _guard = BootstrapGuard::acquire(&source.seed, false)?;
        let record = store.read(&source.seed)?.ok_or(Error::SetupIncomplete)?;
        claim(&source, &store, record)?
    };
    if original.phase == Phase::RemovedTombstone {
        if source.layout.any_target_exists()? || scm::exists(&source.seed)? {
            return Err(Error::Referenced);
        }
        return Ok(());
    }
    source
        .layout
        .existing_targets(original.service_identity.service_sid.as_deref())?
        .validate_known_contents(&source.seed, &source.manifest)?;
    let service = original_service(&source)?;
    super::references::no_image_references(
        &source,
        service.as_ref().map(|process| &process.selector),
    )?;
    if original.phase != Phase::Removing {
        let _guard = BootstrapGuard::acquire(&source.seed, false)?;
        source.recheck()?;
        let next = original.next(Phase::Removing, "removing")?;
        store.commit(&source.seed, Some(&original), &next, None)?;
    }
    drop(service);
    quiesce_original(&source)
}
pub(super) fn remove_target() -> Result<()> {
    let source = Source::from_actual_invocation()?;
    let store = Store::new(&source.seed);
    let original = {
        let _guard = BootstrapGuard::acquire(&source.seed, false)?;
        let record = store.read(&source.seed)?.ok_or(Error::SetupIncomplete)?;
        claim(&source, &store, record)?
    };
    if original.phase == Phase::RemovedTombstone {
        if source.layout.any_target_exists()? || scm::exists(&source.seed)? {
            return Err(Error::Referenced);
        }
        return Ok(());
    }
    if original.phase != Phase::Removing {
        return Err(Error::ScopeConflict);
    }
    let targets = source
        .layout
        .existing_targets(original.service_identity.service_sid.as_deref())?;
    targets.validate_known_contents(&source.seed, &source.manifest)?;
    quiesce_original(&source)?;
    // DeleteService may remain pending while another actual SCM handle lives.
    // Never pretend absence, force-close another process's handle, or reset it.
    scm::remove_stopped(
        &source.seed,
        &source.layout,
        original.service_identity.creation_operation_id.is_some(),
    )?;
    if scm::exists(&source.seed)? {
        return Err(Error::Referenced);
    }
    source.recheck()?;
    super::references::no_image_references(&source, None)?;
    targets.remove_known(&source.seed, &source.manifest)?;
    if source.layout.any_target_exists()? {
        return Err(Error::Referenced);
    }
    let _guard = BootstrapGuard::acquire(&source.seed, false)?;
    source.recheck()?;
    if scm::exists(&source.seed)? {
        return Err(Error::Referenced);
    }
    let next = original.next(Phase::RemovedTombstone, "removed-tombstone")?;
    store.commit(&source.seed, Some(&original), &next, None)
}

/// Optional presentation only. No mutex or transaction spans external COM.
pub(super) fn launch_desktop() -> Result<()> {
    let source = Source::from_actual_invocation()?;
    let store = Store::new(&source.seed);
    let current = {
        let _guard = BootstrapGuard::acquire(&source.seed, false)?;
        let record = store.read(&source.seed)?.ok_or(Error::SetupIncomplete)?;
        if record.phase != Phase::Ready {
            return Err(Error::SetupIncomplete);
        }
        claim(&source, &store, record)?
    };
    let ready = || {
        source.recheck()?;
        let actual = store.read(&source.seed)?.ok_or(Error::StateCorrupt)?;
        if actual.digest()? != current.digest()? {
            return Err(Error::ScopeConflict);
        }
        super::bootstrap::ready_status(&source, &current)
    };
    super::desktop::dispatch(&source, ready)
}

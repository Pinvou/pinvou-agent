//! Persistent configuration, the durable RPC idempotency ledger, pending
//! revocation replay, and user-custom Relay settings.
//!
//! These helpers are stateless and touch only the private JSON files under the
//! pinvou3 home directory. They never lock `Inner`; the facade reads the result
//! and updates `Inner` under its own guard.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{
    ActiveEndpoint, PendingRevocation, PendingRevocationLedger, RpcLedger, WebAccessConfig,
    WebAccessInfo, platform,
};
use crate::platform::paths;

/// Round-15 review: a recognizable prefix for queue-full errors — stop and
/// refresh are recovery paths, and this prefix downgrades that error class to
/// "log and continue" instead of wedging disable/rotate; conflicts with the
/// same key but a different secret stay fail-closed (design boundary recorded
/// in round-14). A string prefix rather than an enum: the module's error
/// surface is plain String, consistent with existing style; callers
/// discriminate with `starts_with`.
pub(super) const PENDING_REVOCATION_QUEUE_FULL_PREFIX: &str =
    "pending Web revocation queue is full";

pub(super) fn pairing_info(endpoint: &ActiveEndpoint) -> WebAccessInfo {
    WebAccessInfo {
        endpoint_id: endpoint.config.endpoint_id.clone(),
        url: endpoint.url.clone(),
        qr_data_url: crate::features::connectors::connector_cli::make_qr(&endpoint.url),
        status: endpoint.status,
    }
}

pub(super) fn fresh_config(allow_host_workspace: bool) -> WebAccessConfig {
    WebAccessConfig {
        relay_url: remote_relay_ws_url(),
        endpoint_id: format!("ep_{}", crate::features::remote_control::short_token(24)),
        access_token: crate::features::remote_control::short_token(48),
        desktop_secret: crate::features::remote_control::short_token(48),
        allow_host_workspace,
    }
}

/// Uniform ws(s) URL validation: length cap, `ws://` or `wss://` prefix, no
/// whitespace; the caller keeps the failure message verbatim.
fn validate_ws_url(url: &str, invalid_message: &str) -> Result<(), String> {
    if url.len() > 2_048
        || !(url.starts_with("ws://") || url.starts_with("wss://"))
        || url.chars().any(char::is_whitespace)
    {
        return Err(invalid_message.to_string());
    }
    Ok(())
}

/// Uniform http(s) page-base validation, sharing `validate_ws_url`'s
/// baseline.
fn validate_http_base_url(url: &str, invalid_message: &str) -> Result<(), String> {
    if url.len() > 2_048
        || !(url.starts_with("http://") || url.starts_with("https://"))
        || url.chars().any(char::is_whitespace)
    {
        return Err(invalid_message.to_string());
    }
    Ok(())
}

pub(super) fn validate_relay_settings(settings: &RelaySettings) -> Result<(), String> {
    validate_ws_url(
        &settings.relay_url,
        "relay address does not normalize to a valid ws(s) URL",
    )?;
    validate_http_base_url(
        &settings.public_base_url,
        "relay address does not normalize to a valid http(s) base",
    )
}

pub(super) fn validate_config(config: &WebAccessConfig) -> Result<(), String> {
    validate_ws_url(&config.relay_url, "Web access relay_url is invalid")?;
    if config.endpoint_id.len() < 8
        || config.endpoint_id.len() > 128
        || !config
            .endpoint_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("Web access endpoint_id is invalid".to_string());
    }
    if !(24..=1_024).contains(&config.access_token.len())
        || !(24..=1_024).contains(&config.desktop_secret.len())
    {
        return Err("Web access credentials are invalid".to_string());
    }
    Ok(())
}

pub(super) fn config_path() -> PathBuf {
    paths::pinvou3_home().join("web-access.json")
}

pub(super) fn process_lock_path() -> PathBuf {
    paths::pinvou3_home().join("web-access.lock")
}

pub(super) fn pending_revocations_path() -> PathBuf {
    paths::pinvou3_home().join("web-access-pending-revocations.json")
}

/// Acquire the process-ownership lock (single desktop owner for Web access).
///
/// Uses `fd_lock` (already in the dependency graph via codewhale-config)
/// instead of fs2; both wrap the same OS primitive (flock on Unix,
/// LockFileEx on Windows). On success the backing allocation is kept alive
/// for the guard's `'static` lifetime: the ownership lock lives until process
/// exit anyway, and dropping the guard (or the process) releases it. On the
/// contended path the allocation is reclaimed so a failed attempt does not
/// strand an unlocked fd for the rest of the process (fs2 closed its file
/// there as well).
pub(super) fn acquire_process_lock(
    path: &Path,
) -> Result<fd_lock::RwLockWriteGuard<'static, File>, String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid Web access lock path: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    platform::configure_private_open_options(&mut options);
    let file = options
        .open(path)
        .map_err(|error| format!("open {}: {error}", path.display()))?;
    platform::enforce_private_permissions(&file, path)
        .map_err(|error| format!("set private permissions on {}: {error}", path.display()))?;
    // SAFETY: `lock` is an owned `Box::into_raw` allocation with no other
    // references. The `&mut` below lives only for the `try_write` call unless
    // a guard is returned; a returned guard carries it for `'static`, which
    // is intended — the process-ownership lock is held until process exit and
    // the allocation is deliberately never freed on that path.
    let lock = Box::into_raw(Box::new(fd_lock::RwLock::new(file)));
    // SAFETY: lock is the sole owning pointer handed out by Box::into_raw with no
    // other references; the &mut lives only for the try_write call, a successfully
    // returned guard carries that borrow (deliberately held until process exit,
    // with the allocation never freed), and the failure path reclaims it via
    // Box::from_raw below.
    let guard = match unsafe { &mut *lock }.try_write() {
        Ok(guard) => guard,
        Err(error) => {
            // SAFETY: `try_write` failed, so no guard borrows the lock and the
            // `&mut` above has expired. Re-taking the Box is sound; dropping
            // it closes the fd instead of stranding it for the process
            // lifetime.
            unsafe { drop(Box::from_raw(lock)) };
            return Err(format!(
                "Web access is already owned by another desktop process ({}): {error}",
                path.display()
            ));
        }
    };
    Ok(guard)
}

pub(super) fn rpc_ledger_path() -> PathBuf {
    paths::pinvou3_home().join("web-access-rpc-ledger.json")
}

pub(super) fn load_config() -> Result<Option<WebAccessConfig>, String> {
    load_config_from(&config_path())
}

pub(super) fn load_config_from(path: &Path) -> Result<Option<WebAccessConfig>, String> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    // Repair on read, before any content read: the combination of corruption
    // plus relaxed permissions must never leave a world-readable credential
    // file on disk (a backup restore or external tool may have made it
    // group/world readable). Round-6 review: the repair must attach to the
    // **first** handle, before `read_to_string` — non-UTF-8 corrupt content
    // makes the read leg return early with an error, so a repair attached
    // after the read would never run and the loose corrupt file would stay
    // exposed on disk indefinitely. Consistent with the write path's 0600
    // baseline; a failed repair surfaces as a read error and also avoids the
    // second open's TOCTOU (the path can be swapped between the two opens).
    // On Windows/fallback this call is an explicit no-op.
    platform::enforce_private_permissions(&file, path).map_err(|error| {
        // Round-8 review minor: when the repair itself fails (fchmod needs
        // owner permissions; a backup restore can leave a world-readable
        // secret owned by someone else), the file must not stay at the
        // well-known path forever. Round-9 review: a plain rename preserves
        // the relaxed mode bits, so the quarantined copy remains
        // world-readable under its new name — use a 0600 copy + unlink of
        // the original instead (the quarantined copy is created by this
        // process, so fchmod on it must succeed; permission over the
        // original comes from the parent directory's write bit, not file
        // ownership). Non-regular files such as a directory placeholder
        // (EISDIR fixture) fall back to rename semantics; if quarantine
        // fails entirely (read-only directory / Windows file lock), keep the
        // original error — that is on par with an unwritable directory, and
        // subsequent writes would fail the same way.
        match crate::platform::filesystem::quarantine_secret_file(path) {
            Ok(quarantined) => eprintln!(
                "[web-access] could not restrict permissions on {} ({error}); \
                     quarantined to {} (copy tightened to 0600) and the next \
                     start creates a fresh 0600 file",
                path.display(),
                quarantined.display()
            ),
            Err(quarantine_error) => eprintln!(
                "[web-access] could not restrict permissions on {} ({error}) and the \
                     quarantine also failed ({quarantine_error}); the loose file \
                     stays in place",
                path.display()
            ),
        }
        format!("restrict {}: {error}", path.display())
    })?;
    let mut text = String::new();
    file.take((super::MAX_WEB_ACCESS_CONFIG_BYTES + 1) as u64)
        .read_to_string(&mut text)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    if text.len() > super::MAX_WEB_ACCESS_CONFIG_BYTES {
        return Err(format!(
            "Web access config is larger than {} KiB: {}",
            super::MAX_WEB_ACCESS_CONFIG_BYTES / 1024,
            path.display()
        ));
    }
    let config: WebAccessConfig = serde_json::from_str(&text)
        .map_err(|error| format!("parse {}: {error}", path.display()))?;
    validate_config(&config)?;
    Ok(Some(config))
}

pub(super) fn persist_config(config: &WebAccessConfig) -> Result<(), String> {
    validate_config(config)?;
    atomic_write_private_json(&config_path(), config, "Web access config")
}

pub(super) fn remove_config() -> Result<(), String> {
    remove_private_file(&config_path())
}

// ── User-customized Relay addresses ───────────────────────────────────
// No in-app settings entry: web-relay.json is maintained by hand, holding a
// "WebSocket address + page base" pair; precedence: runtime env override >
// PINVOU_REMOTE_* env > these settings > built-in defaults.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelaySettings {
    pub relay_url: String,
    pub public_base_url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RelaySettingsInfo {
    pub relay_url: String,
    pub public_base_url: String,
    pub custom: bool,
    pub default_relay_url: String,
    pub default_public_base_url: String,
}

pub(super) fn relay_settings_path() -> PathBuf {
    paths::pinvou3_home().join("web-relay.json")
}

/// Any read failure (missing/corrupt/invalid) falls back to the default
/// relay: the settings file is not a credential, and degrading beats
/// wedging "enable Web access" entirely; a corrupt file is repaired or
/// deleted by hand.
pub(super) fn load_relay_settings() -> Option<RelaySettings> {
    let file = File::open(relay_settings_path()).ok()?;
    let mut text = String::new();
    file.take((super::MAX_WEB_ACCESS_CONFIG_BYTES + 1) as u64)
        .read_to_string(&mut text)
        .ok()?;
    if text.len() > super::MAX_WEB_ACCESS_CONFIG_BYTES {
        return None;
    }
    let settings: RelaySettings = serde_json::from_str(&text).ok()?;
    validate_relay_settings(&settings).ok()?;
    Some(settings)
}

pub(super) fn pending_revocation_key(pending: &PendingRevocation) -> String {
    format!("{}\n{}", pending.relay_url, pending.endpoint_id)
}

pub(super) fn validate_pending_revocation(pending: &PendingRevocation) -> Result<(), String> {
    validate_ws_url(
        &pending.relay_url,
        "pending Web revocation has an invalid relay_url",
    )?;
    if pending.endpoint_id.len() < 4
        || pending.endpoint_id.len() > 128
        || !pending
            .endpoint_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("pending Web revocation has an invalid endpoint_id".to_string());
    }
    if pending.desktop_secret.len() < 24 || pending.desktop_secret.len() > 1_024 {
        return Err("pending Web revocation has an invalid desktop secret".to_string());
    }
    Ok(())
}

pub(super) fn validate_pending_revocation_ledger(
    ledger: &PendingRevocationLedger,
) -> Result<(), String> {
    if ledger.version != super::PENDING_REVOCATIONS_VERSION {
        return Err(format!(
            "unsupported pending Web revocation version {}",
            ledger.version
        ));
    }
    if ledger.entries.len() > super::MAX_PENDING_REVOCATIONS {
        return Err(format!(
            "pending Web revocation count exceeds {}",
            super::MAX_PENDING_REVOCATIONS
        ));
    }
    let mut keys = HashSet::new();
    for pending in &ledger.entries {
        validate_pending_revocation(pending)?;
        if !keys.insert(pending_revocation_key(pending)) {
            return Err(format!(
                "duplicate pending Web revocation for {}",
                pending.endpoint_id
            ));
        }
    }
    Ok(())
}

/// Dedicated to the replay startup path: a Corrupt original has already been
/// quarantined at read time and we continue with an empty ledger — the same
/// "recovery paths are not wedged by a bad file" convention as
/// queue/acknowledge, so enable/resume/rotate no longer hard-fail once over
/// a file that has already been cleaned up (the original threw the parse
/// error straight at the user; a retry would self-heal, but the root cause
/// no longer exists). Unavailable is still transient IO and keeps
/// propagating.
pub(super) fn load_pending_revocations_for_replay() -> Result<PendingRevocationLedger, String> {
    match load_pending_revocations_classified(&pending_revocations_path()) {
        RevocationLedgerLoad::Ledger(ledger) => Ok(ledger),
        RevocationLedgerLoad::Corrupt(error) => {
            eprintln!("[web-access] pending-revocation ledger is corrupt, starting fresh: {error}");
            Ok(PendingRevocationLedger::default())
        }
        RevocationLedgerLoad::Unavailable(error) => Err(error),
    }
}

/// Strict read (both Corrupt and Unavailable error): production paths all go
/// through the classified version or the replay wrapper; this function only
/// serves tests asserting directly against real on-disk files.
#[cfg(test)]
pub(super) fn load_pending_revocations_from(
    path: &Path,
) -> Result<PendingRevocationLedger, String> {
    match load_pending_revocations_classified(path) {
        RevocationLedgerLoad::Ledger(ledger) => Ok(ledger),
        RevocationLedgerLoad::Corrupt(error) | RevocationLedgerLoad::Unavailable(error) => {
            Err(error)
        }
    }
}

/// Three-way classification of pending-revocation ledger reads: Corrupt
/// means unparseable or oversize content (nothing salvageable; the original
/// was quarantined at read time as evidence) and retries are pointless; a
/// parseable ledger that fails whole-file validation is salvaged entry by
/// entry into a Ledger at read time and does not come through here.
/// Unavailable is transient IO where retrying helps. The queue/acknowledge
/// paths use it to decide "move the bad ledger aside and continue" vs
/// "propagate and wait for a retry" — disable/rotate are recovery paths and
/// must not be wedged permanently by one bad file.
enum RevocationLedgerLoad {
    Ledger(PendingRevocationLedger),
    Corrupt(String),
    Unavailable(String),
}

/// Names an endpoint id inside a `[web-access]` log line without opening a
/// log-forging surface: capacity paths name entries whose endpoint_id values
/// are not per-entry validated yet, so a planted/corrupt ledger could
/// otherwise carry control characters and ANSI sequences straight into the
/// desktop log. Clip each id to the same charset the validation uses,
/// marking lossily clipped entries corrupt. Shared by the salvage
/// truncation and the backlog cap retire path.
pub(super) fn clip_pending_endpoint_id_for_log(endpoint_id: &str) -> String {
    let clipped: String = endpoint_id
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || *character == '-' || *character == '_'
        })
        .collect();
    if clipped == endpoint_id {
        clipped
    } else {
        format!("{clipped}~corrupt")
    }
}

/// Salvages, entry by entry, a parseable ledger that failed whole-file
/// validation: a version drift, an oversize entry, a corrupt entry, or a
/// duplicate key invalidates only its own entry, not the remaining valid
/// pending revocations — dropping everything would keep the old endpoint
/// joinable at the relay. Each entry goes through the same
/// `validate_pending_revocation` strictness as a normal load; the first of a
/// duplicate key wins; capacity caps the salvage.
fn salvage_pending_revocations(ledger: &PendingRevocationLedger) -> PendingRevocationLedger {
    let mut salvaged = PendingRevocationLedger::default();
    let mut keys = HashSet::new();
    for (index, pending) in ledger.entries.iter().enumerate() {
        if salvaged.entries.len() >= super::MAX_PENDING_REVOCATIONS {
            // Round-15 review NIT: a capacity truncation must report the
            // count explicitly — silently dropped entries mean the
            // corresponding endpoints stay joinable at the relay until
            // manual cleanup.
            // Round-16 review: beyond the count, name each one (which
            // credentials are still relay-side joinable is the input to
            // manual cleanup); the truncated set is at most the total pending
            // count and this path is already the exceptional branch, so the
            // per-entry log volume is acceptable.
            // Round-20 review (S10-2): the truncation runs before per-entry
            // validation, so the named ids go through the shared log-forging
            // clip helper (round-29 review: extracted so the backlog cap
            // retire path names ids with the same strictness).
            let dropped: Vec<String> = ledger
                .entries
                .iter()
                .skip(index)
                .map(|pending| clip_pending_endpoint_id_for_log(&pending.endpoint_id))
                .collect();
            eprintln!(
                "[web-access] salvage dropped {} pending revocations past the capacity of {} (endpoints stay joinable at the relay until revoked manually): {}",
                ledger.entries.len() - index,
                super::MAX_PENDING_REVOCATIONS,
                dropped.join(", ")
            );
            break;
        }
        if validate_pending_revocation(pending).is_err() {
            continue;
        }
        if !keys.insert(pending_revocation_key(pending)) {
            continue;
        }
        salvaged.entries.push_back(pending.clone());
    }
    salvaged
}

/// Quarantines the corrupt ledger original, preserving the evidence. Returns
/// whether quarantine succeeded — callers use that to decide whether an empty
/// salvage set may remove the path under empty-ledger semantics (Round-12
/// review: when quarantine fails outright the original is the only remaining
/// evidence and must not be deleted afterwards).
fn quarantine_pending_ledger(path: &Path, data: &[u8]) -> bool {
    match crate::platform::filesystem::quarantine_corrupt_file(path) {
        Ok(quarantined) => {
            eprintln!(
                "[web-access] quarantined corrupt pending-revocation ledger to {}",
                quarantined.display()
            );
            true
        }
        Err(rename_error) => {
            // A failed rename (Windows AV/backup locks etc.) must not
            // silently allow the evidence to be overwritten/deleted in place
            // by a later fresh-start write. First copy the whole file (in the
            // oversize case the in-memory `data` holds only the truncated
            // prefix — the complete evidence is on disk); if the copy fails
            // too, fall back to the in-memory bytes; only if both fail
            // degrade to logging alone.
            match crate::platform::filesystem::quarantine_corrupt_file_copy(path) {
                Ok(quarantined) => eprintln!(
                    "[web-access] rename failed ({rename_error}); copied corrupt pending-revocation ledger to {}",
                    quarantined.display()
                ),
                Err(copy_error) => {
                    match crate::platform::filesystem::quarantine_corrupt_file_bytes(path, data) {
                        Ok(quarantined) => eprintln!(
                            "[web-access] rename failed ({rename_error}, copy failed: {copy_error}); \
                             wrote in-memory bytes of corrupt pending-revocation ledger to {}",
                            quarantined.display()
                        ),
                        Err(error) => {
                            eprintln!(
                                "[web-access] failed to quarantine corrupt pending-revocation ledger {}: {error}",
                                path.display()
                            );
                            return false;
                        }
                    }
                }
            }
            true
        }
    }
}

fn load_pending_revocations_classified(path: &Path) -> RevocationLedgerLoad {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return RevocationLedgerLoad::Ledger(PendingRevocationLedger::default());
        }
        Err(error) => {
            return RevocationLedgerLoad::Unavailable(format!("read {}: {error}", path.display()));
        }
    };
    // Repair on read, before any content read (added in Round-7 review, same
    // shape as load_config_from): this file holds the desktop_secret, and the
    // corruption + relaxed-permission combination must not leave a
    // world-readable secret on disk. The repair attaches to the **first**
    // handle — a real I/O error on the read leg (EISDIR/EACCES etc.) returns
    // early, so a repair attached after the read would never run and the
    // loose corrupt file would stay exposed on disk indefinitely; it also
    // avoids the second open's TOCTOU (the path can be swapped between the
    // two opens, so the repaired file may not be the one that was read).
    // (On Windows/fallback this call is an explicit no-op.)
    if let Err(error) = platform::enforce_private_permissions(&file, path) {
        // Round-8 review minor: a failed repair likewise quarantines the
        // loose-permission secret away from the well-known path (same cause,
        // same strategy as load_config_from). Round-9 review: switched to a
        // 0600 copy + unlink of the original, so the quarantined copy no
        // longer inherits the relaxed mode bits (see the same-named comment
        // inside load_config_from); non-regular files fall back to rename;
        // if quarantine fails, keep the original Unavailable error.
        match crate::platform::filesystem::quarantine_secret_file(path) {
            Ok(quarantined) => eprintln!(
                "[web-access] could not restrict permissions on {} ({error}); \
                 quarantined to {} (copy tightened to 0600) and the next \
                 write creates a fresh 0600 ledger",
                path.display(),
                quarantined.display()
            ),
            Err(quarantine_error) => eprintln!(
                "[web-access] could not restrict permissions on {} ({error}) and the \
                     quarantine also failed ({quarantine_error}); the loose file \
                     stays in place",
                path.display()
            ),
        }
        return RevocationLedgerLoad::Unavailable(format!("restrict {}: {error}", path.display()));
    }
    // The same take()-bounded read as the config loader: truncate at the cap
    // first, then check — a corrupt/oversize file must not trigger an
    // unbounded allocation.
    let mut data = Vec::new();
    if let Err(error) = file
        .take((super::MAX_PENDING_REVOCATIONS_FILE_BYTES + 1) as u64)
        .read_to_end(&mut data)
    {
        return RevocationLedgerLoad::Unavailable(format!("read {}: {error}", path.display()));
    }
    if data.len() > super::MAX_PENDING_REVOCATIONS_FILE_BYTES {
        quarantine_pending_ledger(path, &data);
        return RevocationLedgerLoad::Corrupt(format!(
            "pending Web revocation file is larger than {} KiB: {}",
            super::MAX_PENDING_REVOCATIONS_FILE_BYTES / 1024,
            path.display()
        ));
    }
    let ledger = match serde_json::from_slice::<PendingRevocationLedger>(&data) {
        Ok(ledger) => ledger,
        Err(error) => {
            // Entirely unparseable: nothing is salvageable, so quarantine the
            // original as evidence and treat it as corrupt (queue/acknowledge
            // continue from an empty ledger and rewrite it with valid
            // content).
            quarantine_pending_ledger(path, &data);
            return RevocationLedgerLoad::Corrupt(format!("parse {}: {error}", path.display()));
        }
    };
    if let Err(error) = validate_pending_revocation_ledger(&ledger) {
        // Parseable but failing whole-file validation (version drift/oversize/
        // corrupt entry/duplicate key): salvage the still-valid entries one by
        // one and quarantine the original instead of dropping everything —
        // discarded pending revocations would keep the old endpoint joinable
        // at the relay.
        let salvaged = salvage_pending_revocations(&ledger);
        eprintln!(
            "[web-access] pending-revocation ledger invalid ({error}); salvaged {} of {} entries",
            salvaged.entries.len(),
            ledger.entries.len()
        );
        let quarantined = quarantine_pending_ledger(path, &data);
        // The salvaged entries must be persisted immediately: a crash before
        // the worker ack would lose this batch of pending revocations (the
        // quarantined copy is evidence only, never replayed automatically)
        // and the old endpoint would stay joinable at the relay — exactly the
        // outcome this branch exists to avoid. An empty salvage set removes
        // the already-quarantined path under empty-ledger semantics so the
        // next load does not quarantine again; but when quarantine failed
        // outright, the original is the only remaining evidence and must not
        // be deleted under empty-ledger semantics (Round-12 review: keep the
        // original, log loudly, and let the next load retry the quarantine).
        // If persisting fails, degrade to in-memory state: within this
        // process the replay's authoritative ledger
        // (`pending_revocation_backlog`) catches it, and acknowledging
        // against it persists wholesale, rebuilding the on-disk ledger; if
        // the process restarts before that, these entries are unrecoverable
        // (a residual on par with the crash window, recorded in Round-11
        // review minor).
        if salvaged.entries.is_empty() && !quarantined {
            eprintln!(
                "[web-access] quarantine of the invalid pending-revocation ledger {} failed and \
                 nothing was salvaged; the original file is kept in place as evidence",
                path.display()
            );
            return RevocationLedgerLoad::Ledger(salvaged);
        }
        if let Err(persist_error) = persist_pending_revocations_to(path, &salvaged) {
            eprintln!(
                "[web-access] failed to persist salvaged pending revocations: {persist_error}"
            );
        }
        return RevocationLedgerLoad::Ledger(salvaged);
    }
    RevocationLedgerLoad::Ledger(ledger)
}

pub(super) fn persist_pending_revocations_to(
    path: &Path,
    ledger: &PendingRevocationLedger,
) -> Result<(), String> {
    validate_pending_revocation_ledger(ledger)?;
    if ledger.entries.is_empty() {
        remove_private_file(path)
    } else {
        atomic_write_private_json(path, ledger, "pending Web revocations")
    }
}

pub(super) fn queue_pending_revocation(config: &WebAccessConfig) -> Result<(), String> {
    queue_pending_revocation_at(&pending_revocations_path(), PendingRevocation::from(config))
}

pub(super) fn queue_pending_revocation_at(
    path: &Path,
    pending: PendingRevocation,
) -> Result<(), String> {
    validate_pending_revocation(&pending)?;
    let mut ledger = match load_pending_revocations_classified(path) {
        RevocationLedgerLoad::Ledger(ledger) => ledger,
        RevocationLedgerLoad::Corrupt(error) => {
            // Disable/rotate are recovery paths: by this point Corrupt is only
            // the wholly unparseable/oversize classes (a parseable but invalid
            // ledger was already salvaged entry by entry at read time with the
            // original quarantined), so log it and continue from an empty
            // ledger; the subsequent persist rewrites it with valid content
            // and the old endpoint's revocation is not wedged forever.
            eprintln!("[web-access] pending-revocation ledger is corrupt, starting fresh: {error}");
            PendingRevocationLedger::default()
        }
        RevocationLedgerLoad::Unavailable(error) => return Err(error),
    };
    let key = pending_revocation_key(&pending);
    if let Some(existing) = ledger
        .entries
        .iter()
        .find(|entry| pending_revocation_key(entry) == key)
    {
        // Round-14 review minor (recording an existing design boundary): a
        // same-key/different-secret conflict makes queue_pending_revocation
        // return Err, and refresh/stop_current propagate it via `?` — so the
        // residual state of "rotate crashed between queue and persist"
        // (config=(E,S0), ledger=(E,S1)) wedges stop/rotate while the relay
        // is unreachable, until the relay recovers and the worker acks S1.
        // This is deliberate fail-closed (round-9: dropping the entry could
        // leave credentials that are never revoked); fixing it needs
        // multi-secret-per-key ledger semantics (cascading format/ack-logic
        // changes) or a product-level tradeoff, recorded as a residual and
        // out of scope for this wave.
        return if existing == &pending {
            Ok(())
        } else {
            Err(format!(
                "conflicting pending Web revocation credentials for {}",
                pending.endpoint_id
            ))
        };
    }
    if ledger.entries.len() >= super::MAX_PENDING_REVOCATIONS {
        return Err(format!(
            "{PENDING_REVOCATION_QUEUE_FULL_PREFIX} ({})",
            super::MAX_PENDING_REVOCATIONS
        ));
    }
    ledger.entries.push_back(pending);
    persist_pending_revocations_to(path, &ledger)
}

/// Round-11 review minor: the persistence primitive for replay
/// acknowledgments — writes the caller-held authoritative ledger (the
/// in-process backlog) wholesale, replacing "re-read from disk → remove one
/// entry → rewrite"; semantics live in the manager's
/// `finish_pending_revocation`. An empty set removes the file (same as the
/// `_to` variant).
pub(super) fn persist_pending_revocations(ledger: &PendingRevocationLedger) -> Result<(), String> {
    persist_pending_revocations_to(&pending_revocations_path(), ledger)
}

pub(super) fn acknowledge_pending_revocation_at(
    path: &Path,
    pending: &PendingRevocation,
) -> Result<(), String> {
    let mut ledger = match load_pending_revocations_classified(path) {
        RevocationLedgerLoad::Ledger(ledger) => ledger,
        RevocationLedgerLoad::Corrupt(error) => {
            // Same reasoning as queue: the revocation replay's acknowledgment
            // must not be wedged by a bad ledger; continuing from an empty
            // ledger means the ack result (entry removal) overwrites the bad
            // file.
            eprintln!("[web-access] pending-revocation ledger is corrupt, starting fresh: {error}");
            PendingRevocationLedger::default()
        }
        RevocationLedgerLoad::Unavailable(error) => return Err(error),
    };
    ledger.entries.retain(|entry| entry != pending);
    persist_pending_revocations_to(path, &ledger)
}

pub(super) fn eligible_pending_revocations(
    ledger: &PendingRevocationLedger,
    current: Option<&WebAccessConfig>,
) -> Vec<PendingRevocation> {
    ledger
        .entries
        .iter()
        .filter(|pending| {
            current.is_none_or(|config| {
                pending.relay_url != config.relay_url
                    || pending.endpoint_id != config.endpoint_id
                    || pending.desktop_secret != config.desktop_secret
            })
        })
        .cloned()
        .collect()
}

pub(super) fn load_or_initialize_rpc_ledger(endpoint_id: &str) -> Result<RpcLedger, String> {
    load_or_initialize_rpc_ledger_at(&rpc_ledger_path(), endpoint_id)
}

/// Path-injected variant for tests; the initialization branches still persist
/// through the global `persist_rpc_ledger`, so tests only exercise the
/// read/validation paths against injected paths.
pub(super) fn load_or_initialize_rpc_ledger_at(
    path: &Path,
    endpoint_id: &str,
) -> Result<RpcLedger, String> {
    // take()-bounded read: a ledger of 512 tombstones is far below 1 MiB, so
    // an oversize file is necessarily corrupt and must not be read into
    // memory unbounded before parsing.
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let ledger = RpcLedger::for_endpoint(endpoint_id);
            persist_rpc_ledger(&ledger)?;
            return Ok(ledger);
        }
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    // Round-12 review P3: 0600 repair on the first handle, same shape as the
    // config/revocations loaders — this file is written 0600 and a backup
    // restore can leave a relaxed-permission copy, so repair on read and
    // before any content read. Content sensitivity is lower than
    // desktop_secret (endpoint_id + request fingerprints); a failed repair
    // fails closed as a read error (consistent with this loader's other IO
    // errors); on Windows/fallback it is an explicit no-op.
    platform::enforce_private_permissions(&file, path)
        .map_err(|error| format!("restrict {}: {error}", path.display()))?;
    let mut text = String::new();
    file.take((super::MAX_RPC_LEDGER_FILE_BYTES + 1) as u64)
        .read_to_string(&mut text)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    if text.len() > super::MAX_RPC_LEDGER_FILE_BYTES {
        return Err(format!(
            "Web RPC ledger is larger than {} KiB: {}",
            super::MAX_RPC_LEDGER_FILE_BYTES / 1024,
            path.display()
        ));
    }
    let mut ledger: RpcLedger = serde_json::from_str(&text)
        .map_err(|error| format!("parse {}: {error}", path.display()))?;
    if ledger.endpoint_id != endpoint_id {
        let ledger = RpcLedger::for_endpoint(endpoint_id);
        persist_rpc_ledger(&ledger)?;
        return Ok(ledger);
    }
    if ledger.version != super::RPC_LEDGER_VERSION {
        return Err(format!(
            "unsupported Web RPC ledger version {} in {}",
            ledger.version,
            path.display()
        ));
    }
    let mut request_ids = HashSet::new();
    for entry in &ledger.entries {
        if entry.request_id.is_empty()
            || entry.request_id.len() > 256
            || entry.fingerprint.len() != 64
            || !entry
                .fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(format!(
                "invalid Web RPC ledger entry in {}",
                path.display()
            ));
        }
        if !request_ids.insert(entry.request_id.as_str()) {
            return Err(format!(
                "duplicate Web RPC request id in {}: {}",
                path.display(),
                entry.request_id
            ));
        }
    }
    if ledger.entries.len() > super::RPC_CACHE_CAPACITY {
        let overflow = ledger.entries.len() - super::RPC_CACHE_CAPACITY;
        ledger.entries.drain(..overflow);
        persist_rpc_ledger(&ledger)?;
    }
    Ok(ledger)
}

pub(super) fn persist_rpc_ledger(ledger: &RpcLedger) -> Result<(), String> {
    if ledger.version != super::RPC_LEDGER_VERSION || ledger.endpoint_id.is_empty() {
        return Err("refusing to persist an invalid Web RPC ledger".to_string());
    }
    atomic_write_private_json(&rpc_ledger_path(), ledger, "Web RPC ledger")
}

pub(super) fn remove_rpc_ledger() -> Result<(), String> {
    remove_private_file(&rpc_ledger_path())
}

pub(super) fn atomic_write_private_json<T: Serialize>(
    path: &Path,
    value: &T,
    label: &str,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid {label} path: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    let data =
        serde_json::to_vec_pretty(value).map_err(|error| format!("serialize {label}: {error}"))?;
    // Private temp file + fsync + atomic replace + failure cleanup are provided
    // by the shared platform filesystem helper.
    crate::platform::filesystem::atomic_write_private(path, &data)
        .map_err(|error| format!("write {}: {error}", path.display()))
}

pub(super) fn remove_private_file(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => {
            if let Some(parent) = path.parent() {
                // Best-effort durability for the directory-entry deletion; on
                // platforms where a directory cannot be opened as a file the
                // sync is skipped.
                if let Ok(dir) = std::fs::File::open(parent) {
                    dir.sync_all()
                        .map_err(|error| format!("sync {}: {error}", parent.display()))?;
                }
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove {}: {error}", path.display())),
    }
}

/// Single entry point for env-var overrides: empty after trim or failing
/// validation always returns None (falling back to the stored value/built-in
/// default) without erroring — env is only an override mechanism, and an
/// invalid override must degrade rather than wedge Web access. A pure
/// function, convenient for tests without env vars.
fn normalized_env_url(
    raw: Option<String>,
    env_name: &str,
    validate: impl Fn(&str) -> Result<(), String>,
) -> Option<String> {
    let value = raw?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Err(error) = validate(trimmed) {
        // An operator's explicit override being discarded must leave a trace:
        // a silent fallback would make the operator believe they are talking
        // to their own relay while actually working on the stored value /
        // built-in default.
        eprintln!("[web-access] ignoring invalid {env_name} override, falling back: {error}");
        return None;
    }
    Some(trimmed.to_string())
}

pub(super) fn remote_public_base_url() -> String {
    if let Some(url) = normalized_env_url(
        std::env::var("PINVOU_REMOTE_PUBLIC_URL").ok(),
        "PINVOU_REMOTE_PUBLIC_URL",
        |url| validate_http_base_url(url, "PINVOU_REMOTE_PUBLIC_URL is invalid"),
    ) {
        return url;
    }
    if let Some(settings) = load_relay_settings() {
        return settings.public_base_url;
    }
    super::DEFAULT_PUBLIC_BASE_URL.to_string()
}

pub(super) fn remote_relay_ws_url() -> String {
    // Same rule as the other two env overrides (empty after trim/invalid
    // always falls back, no error): an invalid override degrades to the
    // stored value/built-in default instead of wedging enable inside
    // validate_config.
    if let Some(url) = normalized_env_url(
        std::env::var("PINVOU_REMOTE_RELAY_WS_URL").ok(),
        "PINVOU_REMOTE_RELAY_WS_URL",
        |url| validate_ws_url(url, "PINVOU_REMOTE_RELAY_WS_URL is invalid"),
    ) {
        return url;
    }
    if let Some(settings) = load_relay_settings() {
        return settings.relay_url;
    }
    super::DEFAULT_RELAY_WS_URL.to_string()
}

/// Development/manual-test override for the active connection only. Unlike
/// `PINVOU_REMOTE_RELAY_WS_URL`, this value is never written to the persistent
/// Web access config, so testing a local relay cannot strand the normal link.
/// An invalid override (whitespace, non-ws(s), over-long, empty after trim)
/// is ignored and falls back to the stored value.
pub(super) fn apply_runtime_relay_override(mut config: WebAccessConfig) -> WebAccessConfig {
    if let Some(relay_url) = normalized_env_url(
        std::env::var("PINVOU_REMOTE_RUNTIME_RELAY_WS_URL").ok(),
        "PINVOU_REMOTE_RUNTIME_RELAY_WS_URL",
        |url| validate_ws_url(url, "PINVOU_REMOTE_RUNTIME_RELAY_WS_URL is invalid"),
    ) {
        config.relay_url = relay_url;
    }
    config
}

pub(super) fn public_url(config: &WebAccessConfig) -> String {
    let mut url = format!(
        "{}/#endpoint={}&token={}",
        remote_public_base_url().trim_end_matches('/'),
        percent_encode(&config.endpoint_id),
        percent_encode(&config.access_token),
    );
    if config.relay_url != super::DEFAULT_RELAY_WS_URL {
        url.push_str("&relay=");
        url.push_str(&percent_encode(&config.relay_url));
    }
    url
}

pub(super) fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// Fresh opaque epoch token for the stream journal. Used by `StreamState` in the
/// parent module, so it stays `pub(super)`.
pub(super) fn new_stream_epoch() -> String {
    format!("epoch_{}", crate::features::remote_control::short_token(24))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn salvage_truncation_log_clips_dropped_endpoint_ids_to_the_validated_charset() {
        // Round-20 review (S10-2) source pin: the capacity truncation runs
        // before per-entry validation, so named entries' endpoint_id values
        // are unvalidated — naming them directly would carry planted control
        // characters/ANSI sequences into the desktop log (log forging). The
        // shared clip helper (round-29 review: extracted so the backlog cap
        // retire path names ids with the same strictness) must keep the
        // charset clipping and the `~corrupt` marking; reverting it to raw
        // naming turns red.
        let source = include_str!("persistence.rs");
        let helper_start = source
            .find("fn clip_pending_endpoint_id_for_log(")
            .expect("the shared log-clip helper must exist");
        let rest = &source[helper_start..];
        let terminator = rest
            .find("\npub(super) fn ")
            .into_iter()
            .chain(rest.find("\nfn ").into_iter())
            .min()
            .expect("the clip helper body must be terminated by the next fn");
        let helper_body = crate::platform::test_support::comment_filtered(&rest[..terminator]);
        assert!(
            helper_body.contains("filter(|character| {"),
            "dropped endpoint ids must be clipped to the validated charset before they are named in the log"
        );
        assert!(
            helper_body.contains("~corrupt"),
            "lossy-clipped ids must be marked corrupt instead of passing \
             through unvalidated"
        );
        // The truncation log line must name ids through the helper, not raw
        // clones: binding the salvage window to the helper call keeps "what
        // gets logged is the clipped output" true across the extraction
        // (the round-27 m2 concern, restated for the shared-helper shape).
        let start = source
            .find("salvage dropped {} pending revocations")
            .expect("the truncation log line must exist");
        // The window's lower bound must land on a char boundary (this file
        // carries multibyte CJK comment text; a bare minus 900 would slice
        // into a multibyte char — a slice panic rather than an assertion
        // failure, the round-17 lesson).
        let mut window_start = start.saturating_sub(900);
        while window_start > 0 && !source.is_char_boundary(window_start) {
            window_start -= 1;
        }
        let window = crate::platform::test_support::comment_filtered(&source[window_start..start]);
        assert!(
            window.contains("clip_pending_endpoint_id_for_log("),
            "the salvage truncation must name dropped ids through the shared \
             clip helper; naming raw clones revives the log-forging surface"
        );
        // Round-27 review m2: the window previously stopped at the format-
        // string needle — "the clipping code exists" was pinned, but "what
        // gets logged is exactly the clipped output" was not: a mutation
        // swapping the eprintln! argument from `dropped.join(", ")` back to
        // naming the unclipped `ledger.entries` compiles and stays green
        // (the log-forging surface revives). The window extends past the log
        // line, pinning that the argument is the clipped vector itself.
        let mut window_end = (start + 800).min(source.len());
        while window_end < source.len() && !source.is_char_boundary(window_end) {
            window_end -= 1;
        }
        let logged = crate::platform::test_support::comment_filtered(&source[start..window_end]);
        assert!(
            logged.contains("dropped.join(\", \")"),
            "the log must name the clipped dropped vector, not the raw \
             unvalidated ledger entries"
        );
    }

    #[test]
    fn env_url_overrides_are_trimmed_and_fail_safe() {
        // Overrides take effect only when they pass the same validation as
        // the settings file; invalid/empty values always fall back (None)
        // without propagating — env overrides degrade gracefully rather than
        // wedging Web access.
        let http = |url: &str| validate_http_base_url(url, "invalid");
        assert_eq!(
            normalized_env_url(
                Some("  https://relay.example.test/pinvou3/remote \n".into()),
                "PINVOU_REMOTE_TEST_URL",
                http
            ),
            Some("https://relay.example.test/pinvou3/remote".into())
        );
        assert_eq!(
            normalized_env_url(
                Some("ftp://not-http".into()),
                "PINVOU_REMOTE_TEST_URL",
                http
            ),
            None
        );
        assert_eq!(
            normalized_env_url(
                Some("https://has space".into()),
                "PINVOU_REMOTE_TEST_URL",
                http
            ),
            None
        );
        assert_eq!(
            normalized_env_url(
                Some(format!("https://{}", "a".repeat(2_049))),
                "PINVOU_REMOTE_TEST_URL",
                http
            ),
            None,
            "the length cap matches the relay settings validation"
        );
        assert_eq!(
            normalized_env_url(Some("   ".into()), "PINVOU_REMOTE_TEST_URL", http),
            None
        );
        assert_eq!(
            normalized_env_url(None, "PINVOU_REMOTE_TEST_URL", http),
            None
        );

        let ws = |url: &str| validate_ws_url(url, "invalid");
        assert_eq!(
            normalized_env_url(
                Some(" ws://127.0.0.1:8787/ws ".into()),
                "PINVOU_REMOTE_TEST_URL",
                ws
            ),
            Some("ws://127.0.0.1:8787/ws".into())
        );
        assert_eq!(
            normalized_env_url(Some("http://not-ws".into()), "PINVOU_REMOTE_TEST_URL", ws),
            None
        );
        assert_eq!(
            normalized_env_url(Some("".into()), "PINVOU_REMOTE_TEST_URL", ws),
            None
        );
    }

    /// Round-6 review: all three env-override read points must route through
    /// `normalized_env_url` (trim + validation + invalid fallback); falling
    /// back to a bare `std::env::var` would let invalid overrides take effect
    /// silently. Helper-level tests (above) cannot see the wiring, so per
    /// this module's convention the three call sites' bodies are pinned with
    /// source contracts.
    #[test]
    fn env_override_readers_route_through_the_normalized_helper() {
        let source = include_str!("persistence.rs");
        for (fn_name, env_name) in [
            (
                "fn remote_public_base_url() -> String",
                "PINVOU_REMOTE_PUBLIC_URL",
            ),
            (
                "fn remote_relay_ws_url() -> String",
                "PINVOU_REMOTE_RELAY_WS_URL",
            ),
            (
                "fn apply_runtime_relay_override(mut config: WebAccessConfig)",
                "PINVOU_REMOTE_RUNTIME_RELAY_WS_URL",
            ),
        ] {
            let start = source
                .find(fn_name)
                .unwrap_or_else(|| panic!("{fn_name} must exist"));
            let body = &source[start..];
            let body = &body[..body
                .find("\npub(super) fn ")
                .or_else(|| body.find("\nfn "))
                .unwrap_or_else(|| panic!("{fn_name} body must be terminated by the next fn"))];
            assert!(
                body.contains("normalized_env_url("),
                "{fn_name} must route its env override through normalized_env_url"
            );
            assert!(body.contains(env_name), "{fn_name} must read {env_name}");
        }
    }

    /// Round-9 review minor: both loaders' repair-failure legs must go
    /// through `quarantine_secret_file` (0600 copy + unlink of the original)
    /// rather than the plain `quarantine_corrupt_file` — rename preserves the
    /// relaxed mode bits, leaving a world-readable secret owned by someone
    /// else just as exposed under its quarantined name. Triggering this
    /// branch requires foreign ownership (CI cannot drive it; same class as
    /// round-8's review-pinned residual), so per this module's convention the
    /// two call sites are pinned with source contracts; the helper's own
    /// 0600/unlink semantics are covered by executing tests in the
    /// filesystem module.
    #[test]
    fn repair_failure_legs_quarantine_secret_files() {
        let source = include_str!("persistence.rs");
        for fn_name in [
            "pub(super) fn load_config_from(path: &Path) -> Result<Option<WebAccessConfig>, String> {",
            "fn load_pending_revocations_classified(path: &Path) -> RevocationLedgerLoad {",
        ] {
            let start = source
                .find(fn_name)
                .unwrap_or_else(|| panic!("{fn_name} must exist"));
            let body = &source[start..];
            let body = &body[..body
                .find("\npub(super) fn ")
                .or_else(|| body.find("\nfn "))
                .unwrap_or_else(|| panic!("{fn_name} body must be terminated by the next fn"))];
            assert!(
                body.contains("quarantine_secret_file(path)"),
                "{fn_name} must quarantine a failed-permission repair via quarantine_secret_file"
            );
            assert!(
                !body.contains("quarantine_corrupt_file(path)"),
                "{fn_name} must not keep the loose-mode rename on its repair-failure leg"
            );
        }
    }
}

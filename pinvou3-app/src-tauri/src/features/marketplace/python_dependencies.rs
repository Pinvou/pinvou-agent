//! On-demand installer for locked MCP Python dependencies.
//!
//! Each manifest carries a complete wheel lock per target platform (URL + SHA-256).
//! The installer never invokes the system pip: verified wheels are unpacked into
//! `~/.pinvou3/marketplace/python-envs/<lock-hash>`; identical locks reuse the environment, different locks stay isolated, and the download cache is shared by wheel content hash.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::platform::hashing::sha256_file;

const MAX_WHEEL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ENVIRONMENT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_WHEEL_ENTRIES: usize = 50_000;
const PYTHON_PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const COMPLETE_MARKER: &str = "environment.json";
/// Automatic retry cooldown after a failed startup-repair download. Without it, offline
/// or blocked machines replay the whole serial download timeout chain on the setup path at every startup, stalling engine pool init.
const REPAIR_RETRY_COOLDOWN_SECS: u64 = 600;
static INSTALL_LOCK: Mutex<()> = Mutex::new(());
#[cfg(test)]
static FAIL_NEXT_DOWNLOAD: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
#[cfg(test)]
static FAIL_NEXT_PRUNE_REMOVAL: Mutex<Option<PathBuf>> = Mutex::new(None);

#[cfg(test)]
pub(crate) fn fail_next_download_for_test() {
    FAIL_NEXT_DOWNLOAD.store(true, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
pub(crate) fn take_pending_download_failure_for_test() -> bool {
    FAIL_NEXT_DOWNLOAD.swap(false, std::sync::atomic::Ordering::SeqCst)
}

#[cfg(test)]
pub(crate) fn fail_next_prune_removal_for_test(path: PathBuf) {
    *FAIL_NEXT_PRUNE_REMOVAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(path);
}

#[cfg(test)]
pub(crate) fn repair_cooldown_path_for_test() -> PathBuf {
    repair_cooldown_path()
}

#[cfg(test)]
pub(crate) fn environments_root_for_test() -> PathBuf {
    environments_root()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct PythonDependencyLock {
    pub schema_version: u32,
    pub targets: Vec<PythonDependencyTarget>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct PythonDependencyTarget {
    /// Matches `paths::connector_platform_dir`, e.g. `windows-x64`.
    pub platform: String,
    /// Must match the running interpreter's major.minor, e.g. `3.13`.
    pub python: String,
    #[serde(default)]
    pub imports: Vec<String>,
    pub wheels: Vec<PythonWheel>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct PythonWheel {
    pub name: String,
    pub version: String,
    pub filename: String,
    pub url: String,
    pub sha256: String,
}

#[derive(Debug)]
pub(super) struct InstalledPythonEnvironment {
    pub site_packages: PathBuf,
    pub python_command: String,
    /// Held from environment readiness until Marketplace finishes the mcp.json and
    /// installed.json registration, so a concurrent uninstall cannot prune the not-yet-referenced environment.
    _install_guard: std::sync::MutexGuard<'static, ()>,
}

/// Installs and returns the isolated environment when the lock has a target for the current platform; returns `None` when it does not, and the caller
/// takes the legacy platform compatibility path. An invalid lock or a failed install must error — never degrade silently to unverified pip.
///
/// `respect_retry_cooldown`: startup repair passes `true` — after a download failure the
/// environment enters a cooldown so offline or blocked machines do not replay download timeouts on every startup; explicit UI installs pass `false` and user
/// retries stay unrestricted. Once the environment exists both paths take the marker fast path and the cooldown no longer applies.
pub(super) fn ensure_installed(
    lock: &PythonDependencyLock,
    python_command: &str,
    respect_retry_cooldown: bool,
) -> Result<Option<InstalledPythonEnvironment>, String> {
    validate_lock(lock)?;
    let Some(target) = target_for_current_platform(lock) else {
        return Ok(None);
    };

    let install_guard = INSTALL_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let environment_key = environment_key(target)?;
    let environment_root = environments_root();
    if let Err(error) = cleanup_stale_install_artifacts(&environment_root, &wheel_cache_root()) {
        log::warn!("[marketplace] recover stale Python dependency artifacts failed: {error}");
    }
    let destination = environment_root.join(&environment_key);
    let site_packages = destination.join("site-packages");

    if marker_matches(&destination, &environment_key)
        && verify_environment(python_command, target, &site_packages).is_ok()
    {
        return Ok(Some(InstalledPythonEnvironment {
            site_packages,
            python_command: python_command.to_string(),
            _install_guard: install_guard,
        }));
    }

    if respect_retry_cooldown {
        if let Some(remaining) = repair_cooldown_remaining(&environment_key) {
            return Err(format!(
                "MCP Python dependency download is in the automatic retry cooldown (last startup repair failed), about {remaining} s remaining"
            ));
        }
    }

    fs::create_dir_all(&environment_root)
        .map_err(|e| format!("failed to create the MCP Python environment directory: {e}"))?;
    let cache_root = wheel_cache_root();
    fs::create_dir_all(&cache_root)
        .map_err(|e| format!("failed to create the Python wheel cache: {e}"))?;

    let staging = environment_root.join(format!(
        ".installing-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let _ = safe_remove_dir(&staging, &environment_root);
    fs::create_dir_all(staging.join("site-packages"))
        .map_err(|e| format!("failed to create the Python dependency staging directory: {e}"))?;

    let result: Result<(), String> = (|| {
        let mut extracted_bytes = 0_u64;
        for wheel in &target.wheels {
            validate_wheel(wheel)?;
            let cached = cache_root.join(format!("{}.whl", wheel.sha256));
            ensure_cached(wheel, &cached)?;
            extract_wheel(&cached, &staging, &mut extracted_bytes)?;
        }
        verify_environment(python_command, target, &staging.join("site-packages"))?;
        write_marker(&staging, &environment_key, target)?;

        if destination.exists() {
            safe_remove_dir(&destination, &environment_root)?;
        }
        fs::rename(&staging, &destination)
            .map_err(|e| format!("failed to finalize the MCP Python dependency install: {e}"))?;
        Ok(())
    })();

    if let Err(error) = result {
        let _ = safe_remove_dir(&staging, &environment_root);
        record_repair_cooldown(&environment_key);
        return Err(error);
    }
    clear_repair_cooldown(&environment_key);
    Ok(Some(InstalledPythonEnvironment {
        site_packages: destination.join("site-packages"),
        python_command: python_command.to_string(),
        _install_guard: install_guard,
    }))
}

/// Removes isolated environments and wheel caches no longer referenced by any installed MCP. When files are held by a running MCP,
/// the caller logs the error and retries on the next uninstall.
pub(super) fn prune_unused(active_locks: &[PythonDependencyLock]) -> Result<(), String> {
    let _guard = INSTALL_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut environment_keys = HashSet::new();
    let mut wheel_hashes = HashSet::new();
    for lock in active_locks {
        validate_lock(lock)?;
        let Some(target) = target_for_current_platform(lock) else {
            continue;
        };
        environment_keys.insert(environment_key(target)?);
        wheel_hashes.extend(target.wheels.iter().map(|wheel| wheel.sha256.clone()));
    }

    let environment_root = environments_root();
    let cache_root = wheel_cache_root();
    let mut cleanup_errors = Vec::new();
    if let Err(error) = cleanup_stale_install_artifacts(&environment_root, &cache_root) {
        cleanup_errors.push(error);
    }
    if let Ok(entries) = fs::read_dir(&environment_root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir() && is_sha256(&name) && !environment_keys.contains(&name) {
                if let Err(error) = remove_prunable_dir(&entry.path(), &environment_root) {
                    cleanup_errors.push(error);
                }
            }
        }
    }

    if let Ok(entries) = fs::read_dir(&cache_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            if path.extension().and_then(|value| value.to_str()) == Some("whl")
                && is_sha256(stem)
                && !wheel_hashes.contains(stem)
            {
                if let Err(error) = remove_prunable_file(&path) {
                    cleanup_errors.push(error);
                }
            }
        }
    }
    if cleanup_errors.is_empty() {
        Ok(())
    } else {
        Err(cleanup_errors.join("; "))
    }
}

pub(super) fn validate_lock(lock: &PythonDependencyLock) -> Result<(), String> {
    if lock.schema_version != 1 {
        return Err(format!(
            "unsupported MCP Python dependency lock version: {}",
            lock.schema_version
        ));
    }
    let mut platforms = HashSet::new();
    for target in &lock.targets {
        if target.platform.trim().is_empty() || target.python.trim().is_empty() {
            return Err("MCP Python dependency lock is missing platform or python".to_string());
        }
        if !platforms.insert(&target.platform) {
            return Err(format!(
                "MCP Python dependency lock has duplicate platforms: {}",
                target.platform
            ));
        }
        if target.wheels.is_empty() {
            return Err(format!(
                "MCP Python dependency lock platform {} has no wheels",
                target.platform
            ));
        }
        for wheel in &target.wheels {
            validate_wheel(wheel)?;
        }
    }
    Ok(())
}

fn target_for_current_platform(lock: &PythonDependencyLock) -> Option<&PythonDependencyTarget> {
    let platform = crate::platform::paths::connector_platform_dir(
        std::env::consts::OS,
        std::env::consts::ARCH,
    )?;
    target_for_platform(lock, platform)
}

fn target_for_platform<'a>(
    lock: &'a PythonDependencyLock,
    platform: &str,
) -> Option<&'a PythonDependencyTarget> {
    lock.targets
        .iter()
        .find(|target| target.platform == platform)
}

fn validate_wheel(wheel: &PythonWheel) -> Result<(), String> {
    if wheel.name.trim().is_empty() || wheel.version.trim().is_empty() {
        return Err("Python wheel is missing name or version".to_string());
    }
    if !wheel.filename.ends_with(".whl")
        || wheel.filename.contains('/')
        || wheel.filename.contains('\\')
    {
        return Err(format!("invalid Python wheel filename: {}", wheel.filename));
    }
    if !is_sha256(&wheel.sha256) {
        return Err(format!("invalid Python wheel SHA-256: {}", wheel.name));
    }
    let url = reqwest::Url::parse(&wheel.url)
        .map_err(|e| format!("invalid Python wheel download URL: {e}"))?;
    if url.scheme() != "https" || !is_allowed_wheel_host(&url) {
        return Err(format!(
            "Python wheel '{}' must come from the trusted HTTPS host",
            wheel.name
        ));
    }
    if url.path_segments().and_then(Iterator::last) != Some(wheel.filename.as_str()) {
        return Err(format!(
            "Python wheel '{}' filename does not match its download URL",
            wheel.name
        ));
    }
    Ok(())
}

fn is_allowed_wheel_host(url: &reqwest::Url) -> bool {
    matches!(url.host_str(), Some("files.pythonhosted.org"))
}

fn environment_key(target: &PythonDependencyTarget) -> Result<String, String> {
    let serialized = serde_json::to_vec(target)
        .map_err(|e| format!("failed to serialize MCP Python dependency lock: {e}"))?;
    Ok(crate::platform::encoding::hex_lower(&Sha256::digest(
        serialized,
    )))
}

fn environments_root() -> PathBuf {
    crate::platform::paths::pinvou3_home()
        .join("marketplace")
        .join("python-envs")
}

fn wheel_cache_root() -> PathBuf {
    crate::platform::paths::pinvou3_home()
        .join("cache")
        .join("python-wheels")
}

fn repair_cooldown_path() -> PathBuf {
    crate::platform::paths::pinvou3_home()
        .join("marketplace")
        .join("python-repair-cooldown.json")
}

/// Retry cooldown markers: environment key -> last automatic repair failure in Unix seconds. Corrupt or unreadable state is treated as empty:
/// the cooldown only gates when an automatic retry may run and never escalates into an install failure.
fn read_repair_cooldown() -> std::collections::HashMap<String, u64> {
    match fs::read_to_string(repair_cooldown_path()) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|error| {
            log::warn!("[marketplace] ignoring invalid Python repair cooldown state: {error}");
            std::collections::HashMap::new()
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::collections::HashMap::new()
        }
        Err(error) => {
            log::warn!("[marketplace] failed to read Python repair cooldown state: {error}");
            std::collections::HashMap::new()
        }
    }
}

fn write_repair_cooldown(entries: &std::collections::HashMap<String, u64>) {
    let path = repair_cooldown_path();
    if entries.is_empty() {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                log::warn!("[marketplace] failed to clear Python repair cooldown state: {error}")
            }
        }
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    match serde_json::to_vec(entries) {
        Ok(bytes) => {
            if let Err(error) = deepseek_tui::utils::write_atomic(&path, &bytes) {
                log::warn!("[marketplace] failed to persist Python repair cooldown state: {error}");
            }
        }
        Err(error) => {
            log::warn!("[marketplace] failed to serialize Python repair cooldown state: {error}")
        }
    }
}

fn repair_cooldown_remaining(environment_key: &str) -> Option<u64> {
    let failed_at = *read_repair_cooldown().get(environment_key)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    (failed_at + REPAIR_RETRY_COOLDOWN_SECS).checked_sub(now)
}

fn record_repair_cooldown(environment_key: &str) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let mut entries = read_repair_cooldown();
    entries.insert(environment_key.to_string(), now);
    write_repair_cooldown(&entries);
}

fn clear_repair_cooldown(environment_key: &str) {
    let mut entries = read_repair_cooldown();
    if entries.remove(environment_key).is_some() {
        write_repair_cooldown(&entries);
    }
}

/// The legacy pip fallback has no per-platform environment, so key its cooldown
/// on the whole lock payload instead of a single target.
fn fallback_cooldown_key(lock: &PythonDependencyLock) -> Result<String, String> {
    let serialized = serde_json::to_vec(lock)
        .map_err(|e| format!("failed to serialize MCP Python dependency lock: {e}"))?;
    Ok(crate::platform::encoding::hex_lower(&Sha256::digest(
        serialized,
    )))
}

/// Cooldown plumbing for the legacy pip fallback taken when the lock has no
/// target for the current platform (non-Windows). Same semantics as the wheel
/// path: automatic startup repair defers, explicit installs stay unrestricted
/// and clear the marker on success.
pub(super) fn pip_fallback_cooldown_remaining(lock: &PythonDependencyLock) -> Option<u64> {
    repair_cooldown_remaining(&fallback_cooldown_key(lock).ok()?)
}

pub(super) fn record_pip_fallback_cooldown(lock: &PythonDependencyLock) {
    if let Ok(key) = fallback_cooldown_key(lock) {
        record_repair_cooldown(&key);
    }
}

pub(super) fn clear_pip_fallback_cooldown(lock: &PythonDependencyLock) {
    if let Ok(key) = fallback_cooldown_key(lock) {
        clear_repair_cooldown(&key);
    }
}

fn marker_matches(environment: &Path, expected_key: &str) -> bool {
    let Ok(content) = fs::read_to_string(environment.join(COMPLETE_MARKER)) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };
    value
        .get("environment_key")
        .and_then(|value| value.as_str())
        == Some(expected_key)
        && environment.join("site-packages").is_dir()
}

fn write_marker(
    environment: &Path,
    environment_key: &str,
    target: &PythonDependencyTarget,
) -> Result<(), String> {
    let marker = serde_json::json!({
        "schema_version": 1,
        "environment_key": environment_key,
        "platform": target.platform,
        "python": target.python,
        "wheels": target.wheels,
    });
    let json = serde_json::to_vec_pretty(&marker)
        .map_err(|e| format!("failed to serialize the Python environment marker: {e}"))?;
    let mut file = File::create(environment.join(COMPLETE_MARKER))
        .map_err(|e| format!("failed to create the Python environment marker: {e}"))?;
    file.write_all(&json)
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("failed to write the Python environment marker: {e}"))
}

fn ensure_cached(wheel: &PythonWheel, destination: &Path) -> Result<(), String> {
    #[cfg(test)]
    if FAIL_NEXT_DOWNLOAD.swap(false, std::sync::atomic::Ordering::SeqCst) {
        return Err(format!(
            "test injection: failed to download Python dependency {}",
            wheel.name
        ));
    }
    if sha256_file(destination).is_ok_and(|actual| actual == wheel.sha256) {
        return Ok(());
    }

    let url = reqwest::Url::parse(&wheel.url)
        .map_err(|e| format!("invalid Python wheel download URL: {e}"))?;
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(180))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 10
                || attempt.url().scheme() != "https"
                || !is_allowed_wheel_host(attempt.url())
            {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .user_agent("Pinvou-Agent python-dependency-installer")
        .build()
        .map_err(|e| format!("failed to build the Python dependency download client: {e}"))?;
    let response = client
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|e| format!("failed to download Python dependency {}: {e}", wheel.name))?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_WHEEL_BYTES)
    {
        return Err(format!(
            "Python dependency {} exceeds the 64 MiB safety limit",
            wheel.name
        ));
    }

    let mut reader = response.take(MAX_WHEEL_BYTES + 1);
    persist_wheel_download(&mut reader, destination, wheel)
}

fn persist_wheel_download<R: Read>(
    reader: &mut R,
    destination: &Path,
    wheel: &PythonWheel,
) -> Result<(), String> {
    persist_wheel_download_with_sync(reader, destination, wheel, File::sync_all)
}

fn persist_wheel_download_with_sync<R, S>(
    reader: &mut R,
    destination: &Path,
    wheel: &PythonWheel,
    sync: S,
) -> Result<(), String>
where
    R: Read,
    S: FnOnce(&File) -> io::Result<()>,
{
    let partial = destination.with_extension(format!("part-{}", std::process::id()));
    let _ = fs::remove_file(&partial);
    let mut cleanup = PartialFileCleanup::new(partial.clone());
    let mut file = File::create(&partial)
        .map_err(|e| format!("failed to create the wheel staging file: {e}"))?;
    let copied =
        io::copy(reader, &mut file).map_err(|e| format!("failed to save the wheel: {e}"))?;
    sync(&file).map_err(|e| format!("failed to sync the wheel staging file: {e}"))?;
    drop(file);
    if copied > MAX_WHEEL_BYTES {
        return Err(format!(
            "Python dependency {} exceeds the 64 MiB safety limit",
            wheel.name
        ));
    }
    let actual = sha256_file(&partial).map_err(|e| format!("failed to read the wheel: {e}"))?;
    if actual != wheel.sha256 {
        return Err(format!(
            "Python dependency {} checksum mismatch (expected {}, got {})",
            wheel.name, wheel.sha256, actual
        ));
    }
    if destination.exists() {
        fs::remove_file(destination)
            .map_err(|e| format!("failed to replace the wheel cache: {e}"))?;
    }
    fs::rename(&partial, destination)
        .map_err(|e| format!("failed to save the wheel cache: {e}"))?;
    cleanup.disarm();
    Ok(())
}

fn extract_wheel(
    wheel_path: &Path,
    environment: &Path,
    extracted_bytes: &mut u64,
) -> Result<(), String> {
    let file = File::open(wheel_path).map_err(|e| format!("failed to open the wheel: {e}"))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("failed to read the wheel: {e}"))?;
    if archive.len() > MAX_WHEEL_ENTRIES {
        return Err("Python wheel exceeds the entry count safety limit".to_string());
    }
    let site_packages = environment.join("site-packages");
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| format!("failed to read the wheel entry: {e}"))?;
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(format!(
                "Python wheel contains a disallowed symlink: {}",
                entry.name()
            ));
        }
        let Some(enclosed) = entry.enclosed_name() else {
            return Err(format!(
                "Python wheel contains an unsafe path: {}",
                entry.name()
            ));
        };
        let Some(relative) = wheel_install_relative_path(&enclosed) else {
            continue;
        };
        *extracted_bytes = extracted_bytes
            .checked_add(entry.size())
            .ok_or_else(|| "Python wheel extraction size overflow".to_string())?;
        if *extracted_bytes > MAX_ENVIRONMENT_BYTES {
            return Err(
                "MCP Python dependencies exceed the 512 MiB extraction safety limit".to_string(),
            );
        }
        let output = site_packages.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&output)
                .map_err(|e| format!("failed to create the wheel directory: {e}"))?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create the wheel directory: {e}"))?;
        }
        let mut output_file = File::create(&output)
            .map_err(|e| format!("failed to create the wheel file {}: {e}", output.display()))?;
        io::copy(&mut entry, &mut output_file)
            .map_err(|e| format!("failed to extract the wheel file {}: {e}", output.display()))?;
    }
    Ok(())
}

fn wheel_install_relative_path(path: &Path) -> Option<PathBuf> {
    let mut components = path.components();
    let first = components.next()?.as_os_str().to_string_lossy();
    if first.ends_with(".data") {
        let kind = components.next()?.as_os_str().to_string_lossy();
        if kind != "purelib" && kind != "platlib" {
            return None;
        }
        let rest = components.collect::<PathBuf>();
        (!rest.as_os_str().is_empty()).then_some(rest)
    } else {
        Some(path.to_path_buf())
    }
}

fn verify_environment(
    python_command: &str,
    target: &PythonDependencyTarget,
    site_packages: &Path,
) -> Result<(), String> {
    let (major, minor) = parse_python_version(&target.python)?;
    let code = "import importlib,sys; expected=(int(sys.argv[1]),int(sys.argv[2])); assert sys.version_info[:2] == expected, f'expected Python {expected[0]}.{expected[1]}, got {sys.version_info[0]}.{sys.version_info[1]}'; sys.path.insert(0,sys.argv[3]); [importlib.import_module(name) for name in sys.argv[4:]]";
    let mut command = Command::new(python_command);
    command
        .args(["-I", "-S", "-B"])
        .arg("-c")
        .arg(code)
        .arg(major.to_string())
        .arg(minor.to_string())
        .arg(site_packages)
        .args(&target.imports);
    let output = run_python_probe(command, "Pinvou Python dependency verification")?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail.trim();
    let detail = utf8_tail(detail, 4096);
    Err(if detail.is_empty() {
        format!(
            "MCP Python dependency verification failed (exit={})",
            output.status
        )
    } else {
        format!("MCP Python dependency verification failed: {detail}")
    })
}

fn run_python_probe(command: Command, operation: &str) -> Result<Output, String> {
    crate::platform::process::output_with_timeout_and_kill_tree(command, PYTHON_PROBE_TIMEOUT)
        .map_err(|error| format!("{operation} failed: {error}"))
}

fn parse_python_version(value: &str) -> Result<(u8, u8), String> {
    let mut parts = value.split('.');
    let major = parts
        .next()
        .and_then(|value| value.parse::<u8>().ok())
        .ok_or_else(|| format!("invalid Python version: {value}"))?;
    let minor = parts
        .next()
        .and_then(|value| value.parse::<u8>().ok())
        .ok_or_else(|| format!("invalid Python version: {value}"))?;
    if parts.next().is_some() {
        return Err(format!("invalid Python version: {value}"));
    }
    Ok((major, minor))
}

fn utf8_tail(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut start = value.len() - max_bytes;
    while !value.is_char_boundary(start) {
        start += 1;
    }
    &value[start..]
}

fn safe_remove_dir(path: &Path, root: &Path) -> Result<(), String> {
    if !path.starts_with(root) || path == root {
        return Err(format!(
            "refusing to remove Python dependency directory: {}",
            path.display()
        ));
    }
    if path.exists() {
        fs::remove_dir_all(path)
            .map_err(|e| format!("failed to remove the Python dependency directory: {e}"))?;
    }
    Ok(())
}

fn cleanup_stale_install_artifacts(
    environment_root: &Path,
    cache_root: &Path,
) -> Result<(), String> {
    let mut cleanup_errors = Vec::new();
    if let Ok(entries) = fs::read_dir(environment_root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_directory = entry.file_type().is_ok_and(|kind| kind.is_dir());
            if is_directory && is_staging_environment_name(&name) {
                if let Err(error) = remove_prunable_dir(&entry.path(), environment_root) {
                    cleanup_errors.push(error);
                }
            }
        }
    }
    if let Ok(entries) = fs::read_dir(cache_root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_file = entry.file_type().is_ok_and(|kind| kind.is_file());
            if is_file && is_partial_wheel_name(&name) {
                if let Err(error) = remove_prunable_file(&entry.path()) {
                    cleanup_errors.push(error);
                }
            }
        }
    }
    if cleanup_errors.is_empty() {
        Ok(())
    } else {
        Err(cleanup_errors.join("; "))
    }
}

fn is_staging_environment_name(name: &str) -> bool {
    let Some(suffix) = name.strip_prefix(".installing-") else {
        return false;
    };
    let Some((process_id, unique_id)) = suffix.split_once('-') else {
        return false;
    };
    !process_id.is_empty()
        && process_id.bytes().all(|byte| byte.is_ascii_digit())
        && !unique_id.is_empty()
        && unique_id.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_partial_wheel_name(name: &str) -> bool {
    let Some((hash, process_id)) = name.split_once(".part-") else {
        return false;
    };
    is_sha256(hash)
        && !process_id.is_empty()
        && process_id.bytes().all(|byte| byte.is_ascii_digit())
}

fn remove_prunable_dir(path: &Path, root: &Path) -> Result<(), String> {
    #[cfg(test)]
    {
        let mut fail_path = FAIL_NEXT_PRUNE_REMOVAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if fail_path.as_deref() == Some(path) {
            *fail_path = None;
            return Err(
                "test injection: failed to remove the Python dependency directory".to_string(),
            );
        }
    }
    safe_remove_dir(path, root)
}

fn remove_prunable_file(path: &Path) -> Result<(), String> {
    fs::remove_file(path).map_err(|e| format!("failed to remove the unused Python wheel: {e}"))
}

struct PartialFileCleanup {
    path: PathBuf,
    armed: bool,
}

impl PartialFileCleanup {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PartialFileCleanup {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use zip::write::SimpleFileOptions;

    struct FailingReader;

    impl Read for FailingReader {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("injected copy failure"))
        }
    }

    fn with_temp_home<F: FnOnce()>(test: F) {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = std::env::var_os("PINVOU3_HOME");
        let root = std::env::temp_dir().join(format!(
            "pinvou-python-cleanup-test-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        std::env::set_var("PINVOU3_HOME", &root);
        test();
        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = fs::remove_dir_all(root);
    }

    fn sample_lock() -> PythonDependencyLock {
        PythonDependencyLock {
            schema_version: 1,
            targets: vec![PythonDependencyTarget {
                platform: "windows-x64".to_string(),
                python: "3.13".to_string(),
                imports: vec!["example".to_string()],
                wheels: vec![PythonWheel {
                    name: "example".to_string(),
                    version: "1.0.0".to_string(),
                    filename: "example-1.0.0-py3-none-any.whl".to_string(),
                    url: "https://files.pythonhosted.org/packages/example-1.0.0-py3-none-any.whl"
                        .to_string(),
                    sha256: "a".repeat(64),
                }],
            }],
        }
    }

    #[test]
    fn platform_target_is_explicit_and_environment_key_is_stable() {
        let lock = sample_lock();
        let target = target_for_platform(&lock, "windows-x64").unwrap();
        assert!(target_for_platform(&lock, "linux-x64").is_none());
        assert_eq!(
            environment_key(target).unwrap(),
            environment_key(target).unwrap()
        );
        assert_eq!(environment_key(target).unwrap().len(), 64);
    }

    #[test]
    fn lock_rejects_untrusted_or_unpinned_wheels() {
        let mut lock = sample_lock();
        lock.targets[0].wheels[0].url = "http://example.com/example.whl".to_string();
        assert!(validate_lock(&lock)
            .unwrap_err()
            .contains("trusted HTTPS host"));

        let mut lock = sample_lock();
        lock.targets[0].wheels[0].sha256 = "not-a-hash".to_string();
        assert!(validate_lock(&lock).unwrap_err().contains("SHA-256"));
    }

    /// 重试冷却只约束启动修复的自动重试（`respect_retry_cooldown=true`），
    /// UI 显式安装不受限；被延期的自动修复不得触达下载器。
    #[test]
    fn repair_cooldown_defers_auto_retry_but_not_explicit_install() {
        with_temp_home(|| {
            let mut lock = sample_lock();
            lock.targets[0].platform = crate::platform::paths::connector_platform_dir(
                std::env::consts::OS,
                std::env::consts::ARCH,
            )
            .unwrap()
            .to_string();
            let environment_key = environment_key(&lock.targets[0]).unwrap();
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0);
            write_repair_cooldown(&[(environment_key.clone(), now)].into_iter().collect());

            fail_next_download_for_test();
            let deferred = ensure_installed(&lock, "python3", true).unwrap_err();
            assert!(deferred.contains("cooldown"));
            assert!(
                take_pending_download_failure_for_test(),
                "deferred repair must not reach the downloader"
            );

            let explicit = ensure_installed(&lock, "python3", false).unwrap_err();
            assert!(!explicit.contains("cooldown"));
            assert!(
                !take_pending_download_failure_for_test(),
                "explicit install consumed the injected download failure"
            );

            clear_repair_cooldown(&environment_key);
            assert!(!repair_cooldown_path().exists());
        });
    }

    #[test]
    fn wheel_extraction_installs_root_and_data_purelib_only() {
        let root = std::env::temp_dir().join(format!(
            "pinvou-python-wheel-test-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        let wheel = root.join("example.whl");
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut bytes);
            writer
                .start_file("example/__init__.py", SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"VALUE = 1\n").unwrap();
            writer
                .start_file(
                    "example-1.0.data/purelib/shared.py",
                    SimpleFileOptions::default(),
                )
                .unwrap();
            writer.write_all(b"SHARED = True\n").unwrap();
            writer
                .start_file(
                    "example-1.0.data/scripts/ignored.exe",
                    SimpleFileOptions::default(),
                )
                .unwrap();
            writer.write_all(b"ignored").unwrap();
            writer.finish().unwrap();
        }
        fs::write(&wheel, bytes.into_inner()).unwrap();
        let environment = root.join("environment");
        fs::create_dir_all(environment.join("site-packages")).unwrap();
        let mut extracted_bytes = 0;
        extract_wheel(&wheel, &environment, &mut extracted_bytes).unwrap();

        assert!(environment
            .join("site-packages/example/__init__.py")
            .is_file());
        assert!(environment.join("site-packages/shared.py").is_file());
        assert!(!environment.join("site-packages/ignored.exe").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn python_version_requires_major_minor() {
        assert_eq!(parse_python_version("3.13").unwrap(), (3, 13));
        assert!(parse_python_version("3").is_err());
        assert!(parse_python_version("3.13.1").is_err());
    }

    #[test]
    fn stderr_tail_preserves_utf8_boundaries() {
        let value = format!("{}结尾", "a".repeat(4095));
        let tail = utf8_tail(&value, 4096);
        assert!(tail.ends_with("结尾"));
        assert!(tail.len() <= 4096);
    }

    #[test]
    fn prune_recovers_strictly_named_staging_and_partial_artifacts_only() {
        with_temp_home(|| {
            let mut active_lock = sample_lock();
            active_lock.targets[0].platform = crate::platform::paths::connector_platform_dir(
                std::env::consts::OS,
                std::env::consts::ARCH,
            )
            .unwrap()
            .to_string();
            let active_key = environment_key(&active_lock.targets[0]).unwrap();
            let environment_root = environments_root();
            let cache_root = wheel_cache_root();
            fs::create_dir_all(environment_root.join(&active_key)).unwrap();
            fs::create_dir_all(environment_root.join(".installing-123-456")).unwrap();
            fs::create_dir_all(environment_root.join(".installing-current")).unwrap();
            fs::create_dir_all(environment_root.join("notes")).unwrap();
            fs::create_dir_all(environment_root.join("b".repeat(64))).unwrap();
            fs::create_dir_all(&cache_root).unwrap();
            fs::write(
                cache_root.join(format!("{}.whl", "a".repeat(64))),
                b"active",
            )
            .unwrap();
            fs::write(
                cache_root.join(format!("{}.part-789", "c".repeat(64))),
                b"partial",
            )
            .unwrap();
            fs::write(
                cache_root.join(format!("{}.part-worker", "d".repeat(64))),
                b"unrelated",
            )
            .unwrap();
            fs::write(cache_root.join("README.txt"), b"user file").unwrap();

            prune_unused(&[active_lock]).unwrap();

            assert!(environment_root.join(&active_key).is_dir());
            assert!(!environment_root.join(".installing-123-456").exists());
            assert!(environment_root.join(".installing-current").is_dir());
            assert!(environment_root.join("notes").is_dir());
            assert!(!environment_root.join("b".repeat(64)).exists());
            assert!(cache_root.join(format!("{}.whl", "a".repeat(64))).is_file());
            assert!(!cache_root
                .join(format!("{}.part-789", "c".repeat(64)))
                .exists());
            assert!(cache_root
                .join(format!("{}.part-worker", "d".repeat(64)))
                .is_file());
            assert!(cache_root.join("README.txt").is_file());
        });
    }

    #[test]
    fn partial_file_guard_cleans_copy_and_sync_failures() {
        let root = std::env::temp_dir().join(format!(
            "pinvou-python-partial-test-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        let bytes = b"wheel fixture";
        let hash = crate::platform::encoding::hex_lower(&Sha256::digest(bytes));
        let destination = root.join(format!("{hash}.whl"));
        let partial = destination.with_extension(format!("part-{}", std::process::id()));
        let mut wheel = sample_lock().targets.remove(0).wheels.remove(0);
        wheel.sha256 = hash;

        assert!(persist_wheel_download(&mut FailingReader, &destination, &wheel).is_err());
        assert!(!partial.exists());
        assert!(!destination.exists());

        assert!(persist_wheel_download_with_sync(
            &mut Cursor::new(bytes),
            &destination,
            &wheel,
            |_| Err(io::Error::other("injected sync failure")),
        )
        .is_err());
        assert!(!partial.exists());
        assert!(!destination.exists());

        persist_wheel_download(&mut Cursor::new(bytes), &destination, &wheel).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), bytes);
        assert!(!partial.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bundled_document_mcp_manifests_have_complete_windows_locks() {
        let cases = [
            (
                include_str!("../../../../resources/mcp-servers/gongwen/manifest.json"),
                3,
                "docx",
            ),
            (
                include_str!("../../../../resources/mcp-servers/pptx/manifest.json"),
                5,
                "pptx",
            ),
        ];
        for (json, wheel_count, expected_import) in cases {
            let manifest: super::super::ToolManifest = serde_json::from_str(json).unwrap();
            let lock = manifest.python_dependencies.unwrap();
            validate_lock(&lock).unwrap();
            let target = target_for_platform(&lock, "windows-x64").unwrap();
            assert_eq!(target.python, "3.13");
            assert_eq!(target.wheels.len(), wheel_count);
            assert!(target.imports.iter().any(|name| name == expected_import));
        }
    }

    /// 手动发布前验证：使用实际 Pinvou 内置 Python 和真实 PyPI wheel 完成安装、复用与清理。
    /// 默认忽略，避免普通单测依赖网络；运行时显式提供 `PINVOU3_TEST_PYTHON`。
    #[test]
    #[ignore = "requires network and PINVOU3_TEST_PYTHON"]
    fn installs_document_locks_with_real_bundled_python() {
        if std::env::consts::OS != "windows" || std::env::consts::ARCH != "x86_64" {
            return;
        }
        let python = std::env::var("PINVOU3_TEST_PYTHON")
            .expect("PINVOU3_TEST_PYTHON must point to bundled python.exe");
        let gongwen_manifest_json =
            include_str!("../../../../resources/mcp-servers/gongwen/manifest.json");
        let manifest: super::super::ToolManifest =
            serde_json::from_str(gongwen_manifest_json).unwrap();
        let lock = manifest.python_dependencies.unwrap();
        let _environment_guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let root = std::env::temp_dir().join(format!(
            "pinvou-python-dependency-e2e-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let previous_home = std::env::var_os("PINVOU3_HOME");
        std::env::set_var("PINVOU3_HOME", &root);

        let first = ensure_installed(&lock, &python, false).unwrap().unwrap();
        let gongwen_site_packages = first.site_packages.clone();
        assert!(gongwen_site_packages.join("docx/__init__.py").is_file());
        drop(first);
        let second = ensure_installed(&lock, &python, false).unwrap().unwrap();
        assert_eq!(gongwen_site_packages, second.site_packages);
        drop(second);

        let app_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let gongwen_dir = crate::platform::paths::bundle_mcp_servers_dir().join("gongwen");
        fs::create_dir_all(&gongwen_dir).unwrap();
        fs::write(gongwen_dir.join("manifest.json"), gongwen_manifest_json).unwrap();
        fs::write(
            gongwen_dir.join("server.py"),
            fs::read(app_root.join("resources/mcp-servers/gongwen/server.py")).unwrap(),
        )
        .unwrap();
        fs::write(
            gongwen_dir.join("gbt9704_styles.py"),
            fs::read(app_root.join("resources/mcp-servers/gongwen/gbt9704_styles.py")).unwrap(),
        )
        .unwrap();
        fs::write(
            crate::platform::paths::bundle_mcp_python_runner(),
            include_str!(
                "../../../resources/common/bundle/mcp-servers/python_dependency_runner.py"
            ),
        )
        .unwrap();

        let manager = super::super::MarketplaceManager::with_store(
            crate::platform::credential_store::MemoryCredentialStore::default(),
        );
        manager
            .install_with_python("gongwen", &std::collections::HashMap::new(), &python)
            .unwrap();
        let mcp: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(crate::platform::paths::mcp_config_path()).unwrap(),
        )
        .unwrap();
        let entry = &mcp["servers"]["gongwen"];
        let command = entry["command"].as_str().unwrap();
        assert_eq!(Path::new(command), Path::new(&python));
        let args = entry["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(args[0], "-I");
        assert_eq!(args[1], "-S");
        assert_eq!(args[2], "-B");
        assert_eq!(
            Path::new(args[3]),
            crate::platform::paths::bundle_mcp_python_runner()
        );
        assert_eq!(Path::new(args[4]), gongwen_site_packages);

        let mut child = Command::new(command)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(
                concat!(
                    "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n",
                    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n"
                )
                .as_bytes(),
            )
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "gongwen MCP failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("make_gongwen"),
            "gongwen MCP tools/list did not expose make_gongwen"
        );

        let pptx_manifest: super::super::ToolManifest = serde_json::from_str(include_str!(
            "../../../../resources/mcp-servers/pptx/manifest.json"
        ))
        .unwrap();
        let pptx_lock = pptx_manifest.python_dependencies.unwrap();
        let pptx_environment = ensure_installed(&pptx_lock, &python, false)
            .unwrap()
            .unwrap();
        let pptx_site_packages = pptx_environment.site_packages.clone();
        assert!(pptx_site_packages.join("pptx/__init__.py").is_file());
        assert_ne!(gongwen_site_packages, pptx_site_packages);
        drop(pptx_environment);

        let wheel_count = fs::read_dir(wheel_cache_root())
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry.path().extension().and_then(|value| value.to_str()) == Some("whl")
            })
            .count();
        assert_eq!(
            wheel_count, 6,
            "shared lxml/typing wheels must not be downloaded twice"
        );

        prune_unused(std::slice::from_ref(&pptx_lock)).unwrap();
        assert!(!gongwen_site_packages.exists());
        assert!(pptx_site_packages.exists());
        prune_unused(&[]).unwrap();
        assert!(!pptx_site_packages.exists());

        match previous_home {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        fs::remove_dir_all(root).unwrap();
    }
}

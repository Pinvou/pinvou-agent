use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};

#[cfg(windows)]
use crate::capture::harden_windows_handle_acl;
use crate::capture::{JsonlRecorder, create_capture_file};
use crate::clock::{HostMonotonicClock, MonotonicClock};
use crate::protocol::CaptureChannel;
use crate::s2::{
    EventSizeDistribution, PerformanceEvidence, S2Evidence, S2Report, ScenarioEvidence,
    TerminalState, validate,
};

const CAPTURE_FILE: &str = "capture.jsonl";
const EVIDENCE_FILE: &str = "evidence.json";
const REPORT_FILE: &str = "validation-report.json";
const SUMMARY_FILE: &str = "summary.txt";
const APPROVAL_MARKER_NAME: &str = ".codex-s2-approval-marker";
const APPROVAL_MARKER_CONTENT: &str = "S2_APPROVED";
const APPROVAL_MARKER_BYTES: &[u8] = APPROVAL_MARKER_CONTENT.as_bytes();
const APPROVAL_MARKER_MAX_BYTES: u64 = 64;
const AUTH_FILE_MAX_BYTES: u64 = 1024 * 1024;
const VERSION_PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(15);
const MINIMAL_CONFIG: &str = concat!(
    "cli_auth_credentials_store = \"file\"\n",
    "\n",
    "[analytics]\n",
    "enabled = false\n",
    "\n",
    "[otel]\n",
    "exporter = \"none\"\n",
    "trace_exporter = \"none\"\n",
    "metrics_exporter = \"none\"\n",
    "\n",
    "[skills]\n",
    "include_instructions = false\n",
    "\n",
    "[skills.bundled]\n",
    "enabled = false\n",
);
static ISOLATED_HOME_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const VERSION_ARGS: &[&str] = &["--version"];
const APP_SERVER_ARGS: &[&str] = &[
    "app-server",
    "--strict-config",
    "--disable",
    "hooks",
    "--disable",
    "plugins",
    "--disable",
    "apps",
    "--disable",
    "shell_snapshot",
    "--disable",
    "memories",
    "-c",
    "notify=[]",
    "-c",
    "project_root_markers=['.codex-s2-root']",
    "-c",
    "project_doc_max_bytes=0",
    "-c",
    "skills.include_instructions=false",
    "-c",
    "skills.bundled.enabled=false",
    "-c",
    "analytics.enabled=false",
    "-c",
    "otel.exporter='none'",
    "-c",
    "otel.trace_exporter='none'",
    "-c",
    "otel.metrics_exporter='none'",
    "--stdio",
];

#[derive(Clone, Debug)]
pub struct S2RunConfig {
    pub output_dir: Option<PathBuf>,
    pub executable: Option<OsString>,
    pub trusted_approval_wrapper: Option<PathBuf>,
    pub model: Option<String>,
    pub scenario_timeout: Duration,
    pub global_timeout: Duration,
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub test_child_env: Vec<(OsString, OsString)>,
}

#[derive(Debug)]
pub struct S2RunOutcome {
    pub output_dir: PathBuf,
    pub report: S2Report,
}

#[derive(Clone, Copy, Debug)]
struct RunnerThresholds {
    a_min_span: Duration,
    a_min_bytes: u64,
    a_min_events: usize,
    b_min_bytes: u64,
    b_min_events: usize,
    d_min_bytes: u64,
    d_min_events: usize,
}

impl RunnerThresholds {
    const PRODUCTION: Self = Self {
        a_min_span: Duration::from_secs(30),
        a_min_bytes: 2 * 1024,
        a_min_events: 8,
        b_min_bytes: 32 * 1024,
        b_min_events: 32,
        d_min_bytes: 2 * 1024,
        d_min_events: 8,
    };

    #[cfg(debug_assertions)]
    const FAST_TEST: Self = Self {
        a_min_span: Duration::ZERO,
        a_min_bytes: 64,
        a_min_events: 2,
        b_min_bytes: 64,
        b_min_events: 2,
        d_min_bytes: 64,
        d_min_events: 2,
    };
}

#[derive(Clone, Debug)]
struct ContentEvent {
    timestamp_ns: u64,
    bytes: u64,
}

#[derive(Debug)]
enum Inbound {
    Frame { timestamp_ns: u64, value: Value },
    Malformed(String),
    Closed,
}

type Recorder = JsonlRecorder<BufWriter<std::fs::File>, Box<dyn FnMut() -> u64 + Send>>;
const INBOUND_CAPACITY: usize = 1024;

pub fn run_s2(config: S2RunConfig) -> Result<S2RunOutcome> {
    run_s2_with_thresholds(
        config,
        RunnerThresholds::PRODUCTION,
        Arc::new(MarkerHelperInvocation::current()?),
        None,
    )
}

/// Deterministic debug-build seam for the fake app-server integration tests.
/// Production/release builds and the CLI cannot lower the real S2 gates.
#[cfg(debug_assertions)]
pub fn run_s2_for_test(config: S2RunConfig) -> Result<S2RunOutcome> {
    let test_auth_home = create_test_auth_home()?;
    let result = run_s2_with_thresholds(
        config,
        RunnerThresholds::FAST_TEST,
        Arc::new(MarkerHelperInvocation::test_harness()?),
        Some(&test_auth_home),
    );
    let _ = std::fs::remove_dir_all(test_auth_home);
    result
}

fn run_s2_with_thresholds(
    config: S2RunConfig,
    thresholds: RunnerThresholds,
    marker_helper: Arc<MarkerHelperInvocation>,
    source_codex_home: Option<&Path>,
) -> Result<S2RunOutcome> {
    let output_dir = match config.output_dir.clone() {
        Some(path) => path,
        None => default_output_dir(),
    };
    std::fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create output directory {}", output_dir.display()))?;
    let explicit_approval_wrapper =
        validate_explicit_approval_wrapper(config.trusted_approval_wrapper.as_deref())?;
    let global_deadline = Instant::now() + config.global_timeout;
    let mut evidence = empty_evidence();
    let execution = SafeInvocation::resolve(config.executable.as_ref()).and_then(|invocation| {
        ensure_supported_isolation_platform()?;
        validate_host_managed_surfaces()?;
        verify_executable_version(&invocation, global_deadline, &config)?;
        let isolated_home = IsolatedCodexHome::prepare(source_codex_home)?;
        let run = execute(
            &config,
            &invocation,
            &output_dir,
            isolated_home.workspace(),
            &mut evidence,
            thresholds,
            global_deadline,
            explicit_approval_wrapper.as_ref(),
            &marker_helper,
            isolated_home.path(),
            isolated_home.root(),
        );
        let cleanup = isolated_home.cleanup();
        match (run, cleanup) {
            (_, Err(error)) => Err(error),
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        }
    });
    if let Err(error) = &execution {
        classify_failure(error, &mut evidence);
    }
    let report = validate(evidence.clone());
    write_artifacts(&output_dir, &evidence, &report, execution.as_ref().err())?;
    if let Err(error) = execution {
        return Err(error.context(format!("S2 artifacts: {}", output_dir.display())));
    }
    if !report.valid {
        bail!("S2 run is INVALID; artifacts: {}", output_dir.display());
    }
    Ok(S2RunOutcome { output_dir, report })
}

#[cfg(debug_assertions)]
fn create_test_auth_home() -> Result<PathBuf> {
    let path = unique_isolated_home_path("codex-s2-test-auth");
    std::fs::create_dir(&path)?;
    let auth = path.join("auth.json");
    std::fs::write(&auth, b"{}")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&auth, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(path)
}

fn unique_isolated_home_path(prefix: &str) -> PathBuf {
    let sequence = ISOLATED_HOME_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "{prefix}-{}-{timestamp}-{sequence}",
        std::process::id()
    ))
}

#[derive(Debug)]
struct IsolatedCodexHome {
    root: PathBuf,
    path: PathBuf,
    workspace: PathBuf,
    auth: Option<File>,
    auth_identity: Option<PrivateFileIdentity>,
    config: Option<File>,
    config_identity: Option<PrivateFileIdentity>,
    directory: Option<File>,
    directory_identity: Option<PrivateFileIdentity>,
    cleaned: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PrivateFileIdentity {
    device: u64,
    file: u64,
}

impl IsolatedCodexHome {
    fn prepare(source_override: Option<&Path>) -> Result<Self> {
        let root = unique_isolated_home_path("codex-s2-run");
        create_private_directory(&root)
            .map_err(|_| anyhow!("isolated Codex home preparation failed"))?;
        let path = root.join("home");
        let workspace = root.join("workspace");
        let mut home = Self {
            root,
            path,
            workspace,
            auth: None,
            auth_identity: None,
            config: None,
            config_identity: None,
            directory: None,
            directory_identity: None,
            cleaned: false,
        };
        let prepared: Result<()> = (|| {
            let directory = open_and_harden_private_directory(&home.root)
                .map_err(|_| anyhow!("isolated Codex home preparation failed"))?;
            home.directory_identity = Some(private_file_identity(&directory)?);
            home.directory = Some(directory);
            create_private_directory(&home.path)
                .map_err(|_| anyhow!("isolated Codex home preparation failed"))?;
            create_private_directory(&home.workspace)
                .map_err(|_| anyhow!("isolated Codex home preparation failed"))?;
            std::fs::write(home.root.join(".codex-s2-root"), b"")
                .map_err(|_| anyhow!("isolated Codex home preparation failed"))?;
            let source_home = match source_override {
                Some(path) => path.to_path_buf(),
                None => resolve_source_codex_home()?,
            };
            let auth_bytes = read_locked_auth(&source_home.join("auth.json"))?;
            let auth = create_locked_private_file(&home.path.join("auth.json"), &auth_bytes, true)?;
            home.auth_identity = Some(private_file_identity(&auth)?);
            home.auth = Some(auth);
            let config = create_locked_private_file(
                &home.path.join("config.toml"),
                MINIMAL_CONFIG.as_bytes(),
                false,
            )?;
            home.config_identity = Some(private_file_identity(&config)?);
            home.config = Some(config);
            Ok(())
        })();
        if prepared.is_err() {
            let _ = home.cleanup_inner();
            return Err(anyhow!("isolated Codex home preparation failed"));
        }
        Ok(home)
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn workspace(&self) -> &Path {
        &self.workspace
    }

    fn cleanup(mut self) -> Result<()> {
        self.cleanup_inner()
            .map_err(|_| anyhow!("isolated Codex home cleanup failed"))
    }

    fn cleanup_inner(&mut self) -> Result<()> {
        let auth_content_valid =
            self.auth
                .as_mut()
                .zip(self.auth_identity)
                .is_some_and(|(auth, identity)| {
                    validate_mutable_auth_after_run(auth, &self.path.join("auth.json"), identity)
                        .is_ok()
                });
        let directory_matches = self
            .directory_identity
            .zip(path_private_identity(&self.root, true).ok())
            .is_some_and(|(expected, observed)| expected == observed);
        let observed_auth = path_private_identity(&self.path.join("auth.json"), false);
        let auth_matches = self
            .auth_identity
            .zip(observed_auth.as_ref().ok().copied())
            .is_some_and(|(expected, observed)| expected == observed);
        let observed_config = path_private_identity(&self.path.join("config.toml"), false);
        let config_matches = self
            .config_identity
            .zip(observed_config.as_ref().ok().copied())
            .is_some_and(|(expected, observed)| expected == observed);
        self.auth.take();
        self.config.take();
        self.directory.take();
        if directory_matches {
            std::fs::remove_dir_all(&self.root)?;
        }
        self.cleaned = directory_matches && !self.root.exists();
        if !directory_matches
            || !auth_matches
            || !config_matches
            || !auth_content_valid
            || !self.cleaned
        {
            bail!("isolated Codex home integrity check failed");
        }
        Ok(())
    }
}

impl Drop for IsolatedCodexHome {
    fn drop(&mut self) {
        if !self.cleaned {
            let _ = self.cleanup_inner();
        }
    }
}

fn resolve_source_codex_home() -> Result<PathBuf> {
    let candidate = match std::env::var_os("CODEX_HOME") {
        Some(path) => PathBuf::from(path),
        None => {
            #[cfg(windows)]
            let home = std::env::var_os("USERPROFILE");
            #[cfg(unix)]
            let home = std::env::var_os("HOME");
            #[cfg(not(any(windows, unix)))]
            let home: Option<OsString> = None;
            PathBuf::from(home.ok_or_else(|| anyhow!("isolated Codex auth unavailable"))?)
                .join(".codex")
        }
    };
    if !candidate.is_absolute() || !candidate.is_dir() {
        bail!("isolated Codex auth unavailable");
    }
    Ok(candidate)
}

fn ensure_supported_isolation_platform() -> Result<()> {
    #[cfg(any(windows, target_os = "linux"))]
    return Ok(());
    #[cfg(not(any(windows, target_os = "linux")))]
    bail!("S2 isolation is unsupported on this platform");
}

fn validate_host_managed_surfaces() -> Result<()> {
    #[cfg(windows)]
    let paths = {
        let program_data =
            known_program_data().map_err(|_| anyhow!("S2 managed configuration audit failed"))?;
        let codex_system = program_data.join("OpenAI").join("Codex");
        vec![
            codex_system.join("config.toml"),
            codex_system.join("requirements.toml"),
            codex_system.join("managed_config.toml"),
            codex_system.join("skills"),
        ]
    };
    #[cfg(target_os = "linux")]
    let paths = vec![
        PathBuf::from("/etc/codex/config.toml"),
        PathBuf::from("/etc/codex/requirements.toml"),
        PathBuf::from("/etc/codex/managed_config.toml"),
        PathBuf::from("/etc/codex/skills"),
    ];
    #[cfg(not(any(windows, target_os = "linux")))]
    let paths: Vec<PathBuf> = Vec::new();
    validate_managed_surfaces(&paths)
}

fn validate_managed_surfaces(paths: &[PathBuf]) -> Result<()> {
    for path in paths {
        match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) | Ok(_) => bail!("S2 managed configuration audit failed"),
        }
    }
    Ok(())
}

fn sanitize_child_environment(command: &mut Command) {
    const EXACT: &[&str] = &[
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
        "CODEX_ACCESS_TOKEN",
        "CODEX_REFRESH_TOKEN_URL_OVERRIDE",
        "CODEX_REVOKE_TOKEN_URL_OVERRIDE",
        "CODEX_AUTHAPI_BASE_URL",
        "CODEX_AGENT_IDENTITY_AUTHAPI_BASE_URL",
        "CODEX_APP_SERVER_LOGIN_ISSUER",
        "CODEX_EXEC_SERVER_URL",
        "CODEX_OSS_BASE_URL",
        "CODEX_OSS_PORT",
        "CODEX_CONNECTORS_TOKEN",
        "CODEX_CLOUD_TASKS_BASE_URL",
        "CODEX_CLOUD_TASKS_FORCE_INTERNAL",
        "CODEX_INTERNAL_ORIGINATOR_OVERRIDE",
        "CODEX_SQLITE_HOME",
        "CODEX_ROLLOUT_TRACE_ROOT",
        "CODEX_APP_SERVER_MANAGED_CONFIG_PATH",
        "CODEX_TUI_SESSION_LOG_PATH",
        "TRACEPARENT",
        "TRACESTATE",
    ];
    for key in EXACT {
        command.env_remove(key);
    }
    for (key, _) in std::env::vars_os() {
        if key
            .to_str()
            .is_some_and(|value| value.to_ascii_uppercase().starts_with("OTEL_"))
        {
            command.env_remove(key);
        }
    }
}

fn apply_neutral_home_environment(command: &mut Command, neutral_root: &Path) -> Result<()> {
    command.env("HOME", neutral_root);
    #[cfg(windows)]
    {
        let rendered = neutral_root
            .to_str()
            .ok_or_else(|| anyhow!("neutral home environment setup failed"))?;
        let bytes = rendered.as_bytes();
        if bytes.len() < 3
            || !bytes[0].is_ascii_alphabetic()
            || bytes[1] != b':'
            || !matches!(bytes[2], b'\\' | b'/')
        {
            bail!("neutral home environment setup failed");
        }
        command.env("USERPROFILE", neutral_root);
        command.env("HOMEDRIVE", &rendered[..2]);
        command.env("HOMEPATH", &rendered[2..]);
    }
    #[cfg(not(windows))]
    {
        command.env_remove("USERPROFILE");
        command.env_remove("HOMEDRIVE");
        command.env_remove("HOMEPATH");
    }
    Ok(())
}

fn read_locked_auth(path: &Path) -> Result<Vec<u8>> {
    read_locked_auth_with_post_open(path, || {})
}

fn read_locked_auth_with_post_open(path: &Path, post_open: impl FnOnce()) -> Result<Vec<u8>> {
    let mut file = open_locked_nofollow_file(path, false)
        .map_err(|_| anyhow!("isolated Codex auth unavailable"))?;
    let identity_before =
        private_file_identity(&file).map_err(|_| anyhow!("isolated Codex auth unavailable"))?;
    post_open();
    let metadata = file
        .metadata()
        .map_err(|_| anyhow!("isolated Codex auth unavailable"))?;
    if !metadata.file_type().is_file()
        || metadata.len() == 0
        || metadata.len() > AUTH_FILE_MAX_BYTES
    {
        bail!("isolated Codex auth unavailable");
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            bail!("isolated Codex auth unavailable");
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o077 != 0
        {
            bail!("isolated Codex auth unavailable");
        }
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    std::io::Read::by_ref(&mut file)
        .take(AUTH_FILE_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow!("isolated Codex auth unavailable"))?;
    if bytes.len() as u64 != metadata.len()
        || !serde_json::from_slice::<Value>(&bytes).is_ok_and(|value| value.is_object())
        || private_file_identity(&file).ok() != Some(identity_before)
    {
        bail!("isolated Codex auth unavailable");
    }
    Ok(bytes)
}

fn open_locked_nofollow_file(path: &Path, share_existing_write: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Foundation::GENERIC_READ;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        let share_mode = FILE_SHARE_READ
            | if share_existing_write {
                FILE_SHARE_WRITE
            } else {
                0
            };
        options
            .access_mode(GENERIC_READ)
            .share_mode(share_mode)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let _ = share_existing_write;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    Ok(options.open(path)?)
}

fn private_file_identity(file: &File) -> Result<PrivateFileIdentity> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        let information = wrapper_file_information(file.as_raw_handle())?;
        return Ok(PrivateFileIdentity {
            device: information.dwVolumeSerialNumber as u64,
            file: ((information.nFileIndexHigh as u64) << 32) | information.nFileIndexLow as u64,
        });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        return Ok(PrivateFileIdentity {
            device: metadata.dev(),
            file: metadata.ino(),
        });
    }
    #[cfg(not(any(windows, unix)))]
    bail!("isolated Codex home is unsupported");
}

fn path_private_identity(path: &Path, directory: bool) -> Result<PrivateFileIdentity> {
    let file = if directory {
        open_private_directory(path, false)?
    } else {
        open_locked_nofollow_file(path, true)?
    };
    private_file_identity(&file)
}

fn create_private_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700).create(path)?;
        return Ok(());
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir(path)?;
        Ok(())
    }
}

fn open_and_harden_private_directory(path: &Path) -> Result<File> {
    let directory = open_private_directory(path, true)?;
    #[cfg(windows)]
    harden_windows_handle_acl(&directory)?;
    Ok(directory)
}

fn open_private_directory(path: &Path, for_acl_update: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(not(windows))]
    let _ = for_acl_update;
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
            READ_CONTROL, WRITE_DAC,
        };
        let access = if for_acl_update {
            READ_CONTROL | WRITE_DAC
        } else {
            FILE_READ_ATTRIBUTES | READ_CONTROL
        };
        options
            .access_mode(access)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let directory = options.open(path)?;
    if !directory.metadata()?.is_dir() {
        bail!("isolated directory was not a directory");
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        if directory.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            bail!("isolated directory was a reparse point");
        }
    }
    Ok(directory)
}

fn create_locked_private_file(path: &Path, bytes: &[u8], mutable_auth: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, READ_CONTROL, WRITE_DAC};
        let share = FILE_SHARE_READ
            | if mutable_auth {
                windows_sys::Win32::Storage::FileSystem::FILE_SHARE_WRITE
            } else {
                0
            };
        options
            .access_mode(GENERIC_READ | GENERIC_WRITE | READ_CONTROL | WRITE_DAC)
            .share_mode(share);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if mutable_auth { 0o600 } else { 0o400 });
    }
    let mut file = options.open(path)?;
    #[cfg(windows)]
    harden_windows_handle_acl(&file)?;
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(if mutable_auth {
            0o600
        } else {
            0o400
        }))?;
    }
    Ok(file)
}

fn validate_mutable_auth_after_run(
    file: &mut File,
    path: &Path,
    expected_identity: PrivateFileIdentity,
) -> Result<()> {
    if private_file_identity(file)? != expected_identity
        || path_private_identity(path, false)? != expected_identity
    {
        bail!("isolated Codex auth integrity check failed");
    }
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file()
        || metadata.len() == 0
        || metadata.len() > AUTH_FILE_MAX_BYTES
    {
        bail!("isolated Codex auth integrity check failed");
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            bail!("isolated Codex auth integrity check failed");
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o077 != 0
        {
            bail!("isolated Codex auth integrity check failed");
        }
    }
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(AUTH_FILE_MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != metadata.len()
        || !serde_json::from_slice::<Value>(&bytes).is_ok_and(|value| value.is_object())
    {
        bail!("isolated Codex auth integrity check failed");
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct SafeInvocation {
    program: OsString,
    prefix_args: Vec<OsString>,
    #[cfg(windows)]
    cmd_script: Option<PathBuf>,
}

#[derive(Debug)]
struct MarkerHelperInvocation {
    program: OsString,
    args: Vec<OsString>,
    #[cfg(windows)]
    path: Option<PathBuf>,
    #[cfg(windows)]
    handle: Option<windows_sys::Win32::Foundation::HANDLE>,
    #[cfg(windows)]
    identity: Option<WindowsFileIdentity>,
}

impl MarkerHelperInvocation {
    fn current() -> Result<Self> {
        #[cfg(windows)]
        {
            let path = std::env::current_exe()
                .map_err(|_| anyhow!("marker helper executable validation failed"))?;
            return Self::acquire_windows_path_with_post_open(&path, || {});
        }
        #[cfg(target_os = "linux")]
        {
            let program = linux_marker_helper_program();
            if !program.is_file() {
                bail!("marker helper executable validation failed");
            }
            return Ok(Self {
                program: program.into_os_string(),
                args: vec![OsString::from("__marker-helper")],
            });
        }
        #[cfg(all(unix, not(target_os = "linux")))]
        bail!("marker helper executable validation unsupported on this platform");
        #[cfg(not(any(windows, unix)))]
        bail!("marker helper executable validation unsupported on this platform");
    }

    fn command(&self) -> Result<Command> {
        #[cfg(windows)]
        if let (Some(path), Some(handle), Some(identity)) = (&self.path, self.handle, self.identity)
        {
            let held_identity = wrapper_file_identity(handle)
                .map_err(|_| anyhow!("marker helper executable validation failed"))?;
            let reopened = open_locked_wrapper_handle(path)
                .map_err(|_| anyhow!("marker helper executable validation failed"))?;
            let final_path = final_path_from_wrapper_handle(reopened.0)
                .map_err(|_| anyhow!("marker helper executable validation failed"))?;
            let reopened_identity = wrapper_file_identity(reopened.0)
                .map_err(|_| anyhow!("marker helper executable validation failed"))?;
            if held_identity != identity || reopened_identity != identity || final_path != *path {
                bail!("marker helper executable validation failed");
            }
        }
        let mut command = Command::new(&self.program);
        command.args(&self.args);
        Ok(command)
    }

    #[cfg(windows)]
    fn acquire_windows_path_with_post_open(path: &Path, post_open: impl FnOnce()) -> Result<Self> {
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_DIRECTORY, FILE_TYPE_DISK, GetFileType,
        };

        let handle = open_locked_wrapper_handle(path)
            .map_err(|_| anyhow!("marker helper executable validation failed"))?;
        post_open();
        let final_path = final_path_from_wrapper_handle(handle.0)
            .map_err(|_| anyhow!("marker helper executable validation failed"))?;
        let info = wrapper_file_information(handle.0)
            .map_err(|_| anyhow!("marker helper executable validation failed"))?;
        if !final_path.is_absolute()
            || unsafe { GetFileType(handle.0) } != FILE_TYPE_DISK
            || info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0
            || final_path.to_str().is_none_or(|value| {
                value
                    .chars()
                    .any(|character| matches!(character, '\r' | '\n'))
            })
        {
            bail!("marker helper executable validation failed");
        }
        let identity = WindowsFileIdentity {
            volume_serial: info.dwVolumeSerialNumber,
            file_index: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
        };
        let raw = handle.into_raw();
        Ok(Self {
            program: final_path.clone().into_os_string(),
            args: vec![OsString::from("__marker-helper")],
            path: Some(final_path),
            handle: Some(raw),
            identity: Some(identity),
        })
    }

    #[cfg(debug_assertions)]
    fn test_harness() -> Result<Self> {
        let current = std::env::current_exe()
            .map_err(|_| anyhow!("marker helper executable validation failed"))?;
        let debug_dir = current
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| anyhow!("marker helper executable validation failed"))?;
        let program = debug_dir.join(format!(
            "{}{}",
            env!("CARGO_PKG_NAME"),
            std::env::consts::EXE_SUFFIX
        ));
        let program = program
            .canonicalize()
            .map_err(|_| anyhow!("marker helper executable validation failed"))?;
        if !program.is_file() {
            bail!("marker helper executable validation failed");
        }
        #[cfg(windows)]
        return Self::acquire_windows_path_with_post_open(&program, || {});
        #[cfg(not(windows))]
        Ok(Self {
            program: program.into_os_string(),
            args: vec![OsString::from("__marker-helper")],
        })
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_marker_helper_program() -> PathBuf {
    PathBuf::from("/proc/self/exe")
}

#[cfg(windows)]
impl Drop for MarkerHelperInvocation {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
        }
    }
}

impl SafeInvocation {
    fn resolve(explicit: Option<&OsString>) -> Result<Self> {
        #[cfg(windows)]
        {
            let candidate = match explicit {
                Some(value) => PathBuf::from(value),
                None => find_windows_path_command("codex.cmd")?,
            };
            if candidate
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("cmd"))
            {
                let script = resolve_safe_cmd_script(&candidate)?;
                return Ok(Self {
                    program: trusted_system_cmd()?.into_os_string(),
                    prefix_args: ["/d", "/s", "/c"].map(OsString::from).to_vec(),
                    cmd_script: Some(script),
                });
            }
            if explicit.is_none()
                || !candidate
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("exe"))
            {
                bail!("Windows executable override must be a regular .exe or .cmd file");
            }
            let candidate = if candidate.components().count() == 1 {
                find_windows_path_command(
                    candidate
                        .to_str()
                        .context("explicit .exe filename was not valid Unicode")?,
                )?
            } else {
                candidate
            };
            let candidate = candidate.canonicalize().with_context(|| {
                format!(
                    "failed to canonicalize .exe executable {}",
                    candidate.display()
                )
            })?;
            if !candidate.is_file() {
                bail!("Windows executable override was not a regular .exe file");
            }
            return Ok(Self {
                program: candidate.into_os_string(),
                prefix_args: Vec::new(),
                cmd_script: None,
            });
        }
        #[cfg(not(windows))]
        Ok(Self {
            program: explicit.cloned().unwrap_or_else(|| OsString::from("codex")),
            prefix_args: Vec::new(),
        })
    }

    fn command(&self, requested_args: &[&str]) -> Result<Command> {
        if requested_args != VERSION_ARGS && requested_args != APP_SERVER_ARGS {
            bail!("unexpected executable invocation arguments");
        }
        let mut command = Command::new(&self.program);
        #[cfg(windows)]
        if let Some(script) = self.cmd_script.as_ref() {
            use std::os::windows::process::CommandExt;

            if requested_args.iter().any(|arg| {
                arg.is_empty()
                    || !arg.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric()
                            || matches!(
                                byte,
                                b'-' | b'_' | b'=' | b'[' | b']' | b'{' | b'}' | b'.' | b'\''
                            )
                    })
            }) {
                bail!("unsafe requested argument for cmd invocation");
            }
            let script = script
                .to_str()
                .context("canonical .cmd path was not valid Unicode")?;
            let mut line = String::new();
            for prefix in &self.prefix_args {
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(
                    prefix
                        .to_str()
                        .context("cmd prefix argument was not valid Unicode")?,
                );
            }
            line.push_str(" \"\"");
            line.push_str(script);
            line.push('"');
            for arg in requested_args {
                line.push(' ');
                line.push_str(arg);
            }
            line.push('"');
            // cmd.exe has its own quoting grammar; Rust's ordinary Windows argv
            // escaping would insert backslashes that cmd treats literally.
            // The script path was canonicalized and metacharacter-checked above,
            // and requested arguments are restricted to a fixed safe alphabet.
            command.raw_arg(line);
            return Ok(command);
        }
        command.args(&self.prefix_args);
        command.args(requested_args);
        Ok(command)
    }
}

#[cfg(windows)]
fn find_windows_path_command(name: &str) -> Result<PathBuf> {
    let path = std::env::var_os("PATH").context("PATH is unavailable for codex.cmd lookup")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    bail!("{name} was not found on PATH")
}

#[cfg(windows)]
fn resolve_safe_cmd_script(candidate: &Path) -> Result<PathBuf> {
    let candidate = if candidate.components().count() == 1 {
        find_windows_path_command(
            candidate
                .to_str()
                .context("explicit .cmd filename was not valid Unicode")?,
        )?
    } else {
        candidate.to_owned()
    };
    let canonical = candidate.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize .cmd executable {}",
            candidate.display()
        )
    })?;
    if !canonical.is_file()
        || !canonical
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("cmd"))
    {
        bail!("resolved command is not a regular .cmd file");
    }
    let canonical = normalize_windows_command_path(canonical)?;
    let rendered = canonical
        .to_str()
        .context("canonical .cmd path was not valid Unicode")?;
    if rendered.chars().any(|character| {
        matches!(
            character,
            '\r' | '\n' | '"' | '&' | '|' | '<' | '>' | '^' | '%' | '!' | '(' | ')' | ';'
        )
    }) {
        bail!("unsafe command path metacharacter in .cmd executable");
    }
    Ok(canonical)
}

#[cfg(windows)]
fn normalize_windows_command_path(canonical: PathBuf) -> Result<PathBuf> {
    let rendered = canonical
        .to_str()
        .context("canonical .cmd path was not valid Unicode")?;
    if let Some(path) = rendered.strip_prefix(r"\\?\UNC\") {
        return Ok(PathBuf::from(format!(r"\\{path}")));
    }
    if let Some(path) = rendered.strip_prefix(r"\\?\") {
        return Ok(PathBuf::from(path));
    }
    Ok(canonical)
}

#[cfg(debug_assertions)]
fn apply_test_child_env(command: &mut Command, config: &S2RunConfig) {
    command.envs(
        config
            .test_child_env
            .iter()
            .map(|(key, value)| (key, value)),
    );
}

#[cfg(not(debug_assertions))]
fn apply_test_child_env(_command: &mut Command, _config: &S2RunConfig) {}

fn verify_executable_version(
    invocation: &SafeInvocation,
    global_deadline: Instant,
    config: &S2RunConfig,
) -> Result<()> {
    use std::io::Read;

    remaining_global(global_deadline)?;
    let mut process = invocation.command(VERSION_ARGS)?;
    apply_test_child_env(&mut process, config);
    sanitize_child_environment(&mut process);
    process
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut contained = spawn_contained(process).with_context(|| {
        format!(
            "failed to launch version preflight for {:?}",
            invocation.program
        )
    })?;
    let stdout = contained
        .child
        .stdout
        .take()
        .context("version stdout unavailable")?;
    let stderr = contained
        .child
        .stderr
        .take()
        .context("version stderr unavailable")?;
    let (output_tx, output_rx) = mpsc::sync_channel(2);
    let (done_tx, done_rx) = mpsc::sync_channel(2);
    let mut readers = Vec::new();
    for (label, stream) in [
        ("stdout", Box::new(stdout) as Box<dyn Read + Send>),
        ("stderr", Box::new(stderr) as Box<dyn Read + Send>),
    ] {
        let tx = output_tx.clone();
        let done = done_tx.clone();
        readers.push(thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = stream.take(4097).read_to_end(&mut bytes).map(|_| bytes);
            let _ = tx.send((label, result));
            let _ = done.send(label);
        }));
    }
    drop(output_tx);
    drop(done_tx);
    let deadline = global_deadline.min(Instant::now() + version_preflight_timeout());
    let status_result = loop {
        if let Some(status) = contained
            .child
            .try_wait()
            .context("failed polling version preflight")?
        {
            break Ok(status);
        }
        if Instant::now() >= deadline {
            break Err(anyhow!("version preflight timeout"));
        }
        thread::sleep(Duration::from_millis(5));
    };
    // Always tear down the contained tree before waiting for EOF: a short-lived
    // version process may have spawned descendants that inherited its pipes.
    let cleanup_result = contained
        .terminate_and_wait_bounded()
        .context("version preflight cleanup failed");
    let mut stdout_bytes = None;
    let read_deadline = global_deadline.min(Instant::now() + Duration::from_secs(1));
    for _ in 0..2 {
        let remaining = read_deadline
            .checked_duration_since(Instant::now())
            .context("version output reader timeout")?;
        let (label, bytes) = output_rx
            .recv_timeout(remaining)
            .context("version output reader timeout")?;
        let bytes = bytes.context("version output read failed")?;
        if bytes.len() > 4096 {
            bail!("version output exceeded 4096 bytes");
        }
        if label == "stdout" {
            stdout_bytes = Some(bytes);
        }
    }
    let mut completed = std::collections::HashSet::new();
    let join_deadline = Instant::now() + Duration::from_secs(1);
    while completed.len() < readers.len() {
        let Some(remaining) = join_deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        match done_rx.recv_timeout(remaining) {
            Ok(label) => {
                completed.insert(label);
            }
            Err(_) => break,
        }
    }
    for (index, handle) in readers.into_iter().enumerate() {
        let label = if index == 0 { "stdout" } else { "stderr" };
        if completed.contains(label) {
            let _ = handle.join();
        }
    }
    cleanup_result?;
    let status = status_result?;
    if !status.success() {
        bail!("version preflight exited with status {status}");
    }
    let output = String::from_utf8(stdout_bytes.unwrap_or_default())
        .context("version output was not UTF-8")?;
    if output.trim() != "codex-cli 0.139.0" {
        bail!("version preflight requires exact codex-cli 0.139.0");
    }
    Ok(())
}

fn version_preflight_timeout() -> Duration {
    #[cfg(debug_assertions)]
    if let Some(milliseconds) = std::env::var_os("S2_TEST_VERSION_PREFLIGHT_TIMEOUT_MS")
        .and_then(|value| value.to_str().and_then(|value| value.parse::<u64>().ok()))
        .filter(|milliseconds| *milliseconds <= 30_000)
    {
        return Duration::from_millis(milliseconds);
    }
    VERSION_PREFLIGHT_TIMEOUT
}

fn execute(
    config: &S2RunConfig,
    invocation: &SafeInvocation,
    output_dir: &Path,
    workspace: &Path,
    evidence: &mut S2Evidence,
    thresholds: RunnerThresholds,
    global_deadline: Instant,
    explicit_approval_wrapper: Option<&TrustedApprovalWrapper>,
    marker_helper: &Arc<MarkerHelperInvocation>,
    isolated_codex_home: &Path,
    neutral_root: &Path,
) -> Result<()> {
    remaining_global(global_deadline)?;
    let clock = Arc::new(HostMonotonicClock::new()?);
    let recorder_clock = Arc::clone(&clock);
    let recorder: Arc<Mutex<Recorder>> = Arc::new(Mutex::new(JsonlRecorder::new(
        BufWriter::new(create_capture_file(&output_dir.join(CAPTURE_FILE))?),
        Box::new(move || recorder_clock.now_ns().expect("monotonic clock failed")),
    )));
    let mut app_server = invocation.command(APP_SERVER_ARGS)?;
    apply_test_child_env(&mut app_server, config);
    sanitize_child_environment(&mut app_server);
    app_server.env("CODEX_HOME", isolated_codex_home);
    apply_neutral_home_environment(&mut app_server, neutral_root)?;
    app_server.current_dir(neutral_root);
    let mut session = Session::spawn(
        app_server,
        recorder,
        global_deadline,
        Arc::clone(marker_helper),
    )?;

    let result = (|| {
        let initialized = session.request(
            "initialize",
            json!({"clientInfo":{"name":"codex-s2-runner","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}),
            config.scenario_timeout,
        )?;
        validate_initialize(&initialized)?;
        session.notify("initialized", json!({}))?;
        let effective_config = session.request(
            "config/read",
            json!({"includeLayers":true,"cwd":neutral_root}),
            config.scenario_timeout,
        )?;
        validate_effective_config(&effective_config, isolated_codex_home)?;
        let requirements =
            session.request_without_params("configRequirements/read", config.scenario_timeout)?;
        validate_config_requirements(&requirements)?;
        let account = session.request(
            "account/read",
            json!({"refreshToken":false}),
            config.scenario_timeout,
        )?;
        validate_account(&account)?;
        let limits = session.request(
            "account/rateLimits/read",
            json!({}),
            config.scenario_timeout,
        )?;
        validate_rate_limits(&limits)?;
        if quota_exhausted(limits.get("result").unwrap_or(&Value::Null)) {
            bail!("quota exhausted: account rate limit is reached");
        }

        let mut scenario_b_content = Vec::new();
        let mut interrupt_response_latency_ms = None;
        let mut interrupt_terminal_latency_ms = None;
        let approval_command = approval_command()?;
        #[cfg(windows)]
        let approval_wrapper_shell = match explicit_approval_wrapper {
            Some(wrapper) => Some(wrapper.path.clone()),
            None => auto_trusted_approval_wrapper_shell()?,
        };
        #[cfg(windows)]
        let approval_wrapper = approval_wrapper_shell
            .map(|shell| approval_wrapper_candidate(&shell, &approval_command))
            .transpose()?;
        #[cfg(not(windows))]
        let approval_wrapper: Option<String> = None;
        #[cfg(not(windows))]
        let _ = explicit_approval_wrapper;
        for name in ["A", "B", "C", "D"] {
            let scenario_deadline = Instant::now() + config.scenario_timeout;
            let scenario_workspace = workspace.join(name.to_ascii_lowercase());
            create_scenario_workspace(&scenario_workspace)?;
            let scenario_workspace = scenario_workspace
                .canonicalize()
                .map_err(|_| anyhow!("scenario workspace creation failed"))?;
            if name == "C" {
                ensure_approval_marker_absent_bounded(
                    marker_helper,
                    &scenario_workspace,
                    scenario_deadline,
                    global_deadline,
                )?;
            }
            let approval_policy = if name == "C" { "on-request" } else { "never" };
            let sandbox = if name == "C" {
                "read-only"
            } else {
                "workspace-write"
            };
            let mut thread_params = json!({
                "cwd": scenario_workspace,
                "approvalPolicy": approval_policy,
                "sandbox": sandbox,
                "ephemeral": true
            });
            if let Some(model) = config.model.as_ref() {
                thread_params["model"] = Value::String(model.clone());
            }
            validate_host_managed_surfaces()?;
            let started = session.request(
                "thread/start",
                thread_params,
                remaining_until(scenario_deadline)?,
            )?;
            let thread_id = started
                .pointer("/result/thread/id")
                .and_then(Value::as_str)
                .with_context(|| {
                    format!("scenario {name}: thread/start response missing thread.id")
                })?
                .to_owned();
            let prompt = scenario_prompt(name, &approval_command)?;
            let mut turn_params = json!({
                "threadId":thread_id,
                "cwd":scenario_workspace,
                "input":[{"type":"text","text":prompt}]
            });
            if name == "C" {
                turn_params["approvalPolicy"] = json!("on-request");
                turn_params["sandboxPolicy"] = json!({"type":"readOnly"});
                ensure_approval_marker_absent_bounded(
                    marker_helper,
                    &scenario_workspace,
                    scenario_deadline,
                    global_deadline,
                )?;
            }
            let turn = session.request(
                "turn/start",
                turn_params,
                remaining_until(scenario_deadline)?,
            )?;
            let turn_id = turn
                .pointer("/result/turn/id")
                .and_then(Value::as_str)
                .with_context(|| format!("scenario {name}: turn/start response missing turn.id"))?
                .to_owned();
            let observed = session.drive_scenario(
                name,
                &thread_id,
                &turn_id,
                &prompt,
                &scenario_workspace,
                &approval_command,
                approval_wrapper.as_deref(),
                explicit_approval_wrapper,
                scenario_deadline,
                thresholds,
            )?;
            if name == "C" && observed.evidence.terminal_state == TerminalState::Completed {
                verify_approval_marker_bounded(
                    marker_helper,
                    &scenario_workspace,
                    scenario_deadline,
                    global_deadline,
                )?;
            }
            if name == "B" {
                scenario_b_content.extend(observed.content.iter().cloned());
            }
            if name == "D" {
                interrupt_response_latency_ms = observed.interrupt_response_latency_ms;
                interrupt_terminal_latency_ms = observed.interrupt_terminal_latency_ms;
            }
            evidence.scenarios = evidence
                .scenarios
                .iter()
                .cloned()
                .map(|item| {
                    if item.name == name {
                        observed.evidence.clone()
                    } else {
                        item
                    }
                })
                .collect();
        }
        evidence.performance = Some(performance(&scenario_b_content)?);
        evidence.candidate_percentiles = Some(json!({
            "content_event_samples": scenario_b_content.len(),
            "merge_rate": evidence.performance.as_ref().map(|p| p.merge_output_events as f64 / p.merge_input_events as f64),
            "interrupt_response_latency_ms": interrupt_response_latency_ms,
            "interrupt_terminal_latency_ms": interrupt_terminal_latency_ms
        }));
        Ok(())
    })();
    let cleanup = session.stop();
    result.and(cleanup)
}

struct ObservedScenario {
    evidence: ScenarioEvidence,
    content: Vec<ContentEvent>,
    interrupt_response_latency_ms: Option<f64>,
    interrupt_terminal_latency_ms: Option<f64>,
}

struct ContainedChild {
    child: Child,
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(unix)]
    process_group: Option<i32>,
}

#[cfg(any(unix, test))]
fn terminate_process_group_once(process_group: &mut Option<i32>, terminate: impl FnOnce(i32)) {
    if let Some(process_group) = process_group.take() {
        terminate(process_group);
    }
}

impl ContainedChild {
    fn terminate_tree(&mut self) {
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::System::JobObjects::TerminateJobObject;
            if !self.job.is_null() {
                let _ = TerminateJobObject(self.job, 1);
            }
        }
        #[cfg(unix)]
        terminate_process_group_once(&mut self.process_group, |process_group| unsafe {
            let _ = kill(-process_group, 9);
        });
        let _ = self.child.kill();
    }

    fn terminate_and_wait_bounded(&mut self) -> Result<()> {
        self.terminate_tree();
        let child_result = wait_child_bounded(&mut self.child);
        #[cfg(windows)]
        let job_result = unsafe {
            use windows_sys::Win32::Foundation::CloseHandle;
            use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
            use windows_sys::Win32::System::Threading::WaitForSingleObject;
            if !self.job.is_null() {
                // A job becomes signaled only after every contained process has
                // exited, including descendants that inherited helper pipes.
                let wait = WaitForSingleObject(self.job, 1_000);
                CloseHandle(self.job);
                self.job = std::ptr::null_mut();
                if wait != WAIT_OBJECT_0 {
                    Err(anyhow!("contained process cleanup failed"))
                } else {
                    Ok(())
                }
            } else {
                Ok(())
            }
        };
        child_result?;
        #[cfg(windows)]
        job_result?;
        Ok(())
    }
}

impl Drop for ContainedChild {
    fn drop(&mut self) {
        let _ = self.terminate_and_wait_bounded();
    }
}

struct Session {
    contained: ContainedChild,
    stdin: ChildStdin,
    incoming: mpsc::Receiver<Inbound>,
    recorder: Arc<Mutex<Recorder>>,
    next_id: u64,
    pending: VecDeque<(u64, Value)>,
    global_deadline: Instant,
    marker_helper: Arc<MarkerHelperInvocation>,
    stdout_thread: Option<thread::JoinHandle<()>>,
    stderr_thread: Option<thread::JoinHandle<()>>,
    reader_done: mpsc::Receiver<&'static str>,
    inbound_overflow: Arc<AtomicBool>,
}

fn try_emit(sender: &mpsc::SyncSender<Inbound>, overflow: &AtomicBool, event: Inbound) -> bool {
    match sender.try_send(event) {
        Ok(()) => true,
        Err(mpsc::TrySendError::Full(_)) => {
            overflow.store(true, Ordering::Release);
            true
        }
        Err(mpsc::TrySendError::Disconnected(_)) => false,
    }
}

impl Session {
    fn spawn(
        mut process: Command,
        recorder: Arc<Mutex<Recorder>>,
        global_deadline: Instant,
        marker_helper: Arc<MarkerHelperInvocation>,
    ) -> Result<Self> {
        process
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut contained = spawn_contained(process).context("failed to launch app-server")?;
        let stdin = contained
            .child
            .stdin
            .take()
            .context("app-server stdin unavailable")?;
        let stdout = contained
            .child
            .stdout
            .take()
            .context("app-server stdout unavailable")?;
        let stderr = contained
            .child
            .stderr
            .take()
            .context("app-server stderr unavailable")?;
        let (tx, incoming) = mpsc::sync_channel(INBOUND_CAPACITY);
        let inbound_overflow = Arc::new(AtomicBool::new(false));
        let (reader_done_tx, reader_done) = mpsc::sync_channel(2);
        let stderr_tx = tx.clone();
        let stderr_overflow = Arc::clone(&inbound_overflow);
        let stdout_overflow = Arc::clone(&inbound_overflow);
        let stdout_done = reader_done_tx.clone();
        let stdout_recorder = Arc::clone(&recorder);
        let stdout_thread = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(error) => {
                        try_emit(&tx, &stdout_overflow, Inbound::Malformed(error.to_string()));
                        break;
                    }
                };
                let timestamp_ns = match stdout_recorder.lock() {
                    Ok(mut guard) => {
                        match guard.record_timestamped(CaptureChannel::ServerToClient, &line) {
                            Ok(timestamp) => timestamp,
                            Err(error) => {
                                try_emit(
                                    &tx,
                                    &stdout_overflow,
                                    Inbound::Malformed(format!(
                                        "raw capture write failed: {error}"
                                    )),
                                );
                                break;
                            }
                        }
                    }
                    Err(_) => {
                        try_emit(
                            &tx,
                            &stdout_overflow,
                            Inbound::Malformed("raw capture recorder lock poisoned".to_owned()),
                        );
                        break;
                    }
                };
                match serde_json::from_str::<Value>(&line) {
                    Ok(value) if value.is_object() => {
                        if !try_emit(
                            &tx,
                            &stdout_overflow,
                            Inbound::Frame {
                                timestamp_ns,
                                value,
                            },
                        ) {
                            break;
                        }
                    }
                    Ok(_) => {
                        try_emit(
                            &tx,
                            &stdout_overflow,
                            Inbound::Malformed("server frame was not an object".to_owned()),
                        );
                        break;
                    }
                    Err(error) => {
                        try_emit(
                            &tx,
                            &stdout_overflow,
                            Inbound::Malformed(format!("malformed server JSON: {error}")),
                        );
                        break;
                    }
                }
            }
            try_emit(&tx, &stdout_overflow, Inbound::Closed);
            let _ = stdout_done.send("stdout");
        });
        let stderr_recorder = Arc::clone(&recorder);
        let stderr_done = reader_done_tx;
        let stderr_thread = thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            loop {
                let line = match read_line_checked(&mut reader) {
                    Ok(Some(line)) => line,
                    Ok(None) => break,
                    Err(error) => {
                        try_emit(
                            &stderr_tx,
                            &stderr_overflow,
                            Inbound::Malformed(format!("stderr read failed: {error}")),
                        );
                        break;
                    }
                };
                let result = stderr_recorder
                    .lock()
                    .map_err(|_| anyhow!("raw capture recorder lock poisoned"))
                    .and_then(|mut guard| guard.record(CaptureChannel::Stderr, &line));
                if let Err(error) = result {
                    try_emit(
                        &stderr_tx,
                        &stderr_overflow,
                        Inbound::Malformed(format!("raw stderr capture write failed: {error}")),
                    );
                    break;
                }
            }
            let _ = stderr_done.send("stderr");
        });
        Ok(Self {
            contained,
            stdin,
            incoming,
            recorder,
            next_id: 1,
            pending: VecDeque::new(),
            global_deadline,
            marker_helper,
            stdout_thread: Some(stdout_thread),
            stderr_thread: Some(stderr_thread),
            reader_done,
            inbound_overflow,
        })
    }

    fn send(&mut self, value: &Value) -> Result<u64> {
        let line = serde_json::to_string(value)?;
        let timestamp_ns = self
            .recorder
            .lock()
            .map_err(|_| anyhow!("capture recorder lock poisoned"))?
            .record_timestamped(CaptureChannel::ClientToServer, &line)?;
        writeln!(self.stdin, "{line}").context("failed writing app-server stdin")?;
        self.stdin
            .flush()
            .context("failed flushing app-server stdin")?;
        Ok(timestamp_ns)
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(&json!({"method":method,"params":params}))
            .map(|_| ())
    }

    fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        self.request_inner(method, Some(params), timeout)
    }

    fn request_without_params(&mut self, method: &str, timeout: Duration) -> Result<Value> {
        self.request_inner(method, None, timeout)
    }

    fn request_inner(
        &mut self,
        method: &str,
        params: Option<Value>,
        timeout: Duration,
    ) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let mut request = json!({"id":id,"method":method});
        if let Some(params) = params {
            request["params"] = params;
        }
        self.send(&request)?;
        let deadline = Instant::now() + timeout;
        loop {
            let (timestamp_ns, frame) = self.recv_wire(deadline)?;
            if frame.get("id") == Some(&json!(id)) && frame.get("method").is_none() {
                if let Some(error) = frame.get("error") {
                    bail!("{method} error: {}", sanitized_error(error));
                }
                if frame.get("result").is_none() {
                    bail!("{method} response missing result");
                }
                return Ok(frame);
            }
            if frame.get("id").is_some() && frame.get("method").is_some() {
                if method == "turn/start" {
                    if self.pending.len() >= INBOUND_CAPACITY {
                        bail!("protocol error: pending frame queue overflow");
                    }
                    self.pending.push_back((timestamp_ns, frame));
                    continue;
                }
                self.reject_server_request(&frame)?;
                bail!("unexpected server request while waiting for {method}");
            }
            fail_on_error_notification(&frame)?;
            if method == "turn/start" {
                if self.pending.len() >= INBOUND_CAPACITY {
                    bail!("protocol error: pending frame queue overflow");
                }
                self.pending.push_back((timestamp_ns, frame));
            }
        }
    }

    fn drive_scenario(
        &mut self,
        name: &str,
        thread_id: &str,
        turn_id: &str,
        prompt: &str,
        workspace: &Path,
        approval_command: &str,
        approval_wrapper: Option<&str>,
        explicit_approval_wrapper: Option<&TrustedApprovalWrapper>,
        deadline: Instant,
        thresholds: RunnerThresholds,
    ) -> Result<ObservedScenario> {
        let mut content = Vec::new();
        let mut approval_seen = false;
        let mut interrupt_response_seen = false;
        let mut interrupt_id = None;
        let mut interrupt_request_ns = None;
        let mut interrupt_response_latency_ms = None;
        let mut interrupt_terminal_latency_ms = None;
        let mut pending_terminal = None;
        let mut agent_only_state =
            matches!(name, "A" | "B" | "D").then(|| AgentOnlyState::new(prompt, workspace));
        loop {
            let (timestamp_ns, frame) = self.recv(deadline)?;
            if frame.get("id").is_some() && frame.get("method").is_some() {
                if name != "C" || approval_seen {
                    self.reject_server_request(&frame)?;
                    bail!("scenario {name}: unexpected or duplicate approval request");
                }
                self.approve_exact_command(
                    &frame,
                    thread_id,
                    turn_id,
                    workspace,
                    approval_command,
                    approval_wrapper,
                    explicit_approval_wrapper,
                    deadline,
                )?;
                approval_seen = true;
                continue;
            }
            fail_on_error_notification(&frame)?;
            if let Some(expected) = interrupt_id {
                if frame.get("id") == Some(&json!(expected)) && frame.get("method").is_none() {
                    if frame.get("error").is_some() || frame.get("result").is_none() {
                        bail!("scenario D: malformed interrupt response");
                    }
                    interrupt_response_seen = true;
                    interrupt_response_latency_ms = interrupt_request_ns.map(|request_ns: u64| {
                        timestamp_ns.saturating_sub(request_ns) as f64 / 1_000_000.0
                    });
                    if pending_terminal.is_some() {
                        break;
                    }
                    continue;
                }
            }
            if let Some(state) = agent_only_state.as_mut() {
                state.validate(&frame, thread_id, turn_id)?;
            }
            if let Some(bytes) = agent_delta_bytes(&frame, thread_id, turn_id) {
                content.push(ContentEvent {
                    timestamp_ns,
                    bytes,
                });
                let content_bytes = content.iter().map(|event| event.bytes).sum::<u64>();
                if name == "D"
                    && interrupt_id.is_none()
                    && content.len() >= thresholds.d_min_events
                    && content_bytes >= thresholds.d_min_bytes
                {
                    let id = self.next_id;
                    self.next_id += 1;
                    let request_ns = self.send(&json!({"id":id,"method":"turn/interrupt","params":{"threadId":thread_id,"turnId":turn_id}}))?;
                    interrupt_request_ns = Some(request_ns);
                    interrupt_id = Some(id);
                }
            }
            if let Some(status) = terminal_status(&frame, thread_id, turn_id) {
                let terminal_state = match status {
                    "completed" => TerminalState::Completed,
                    "interrupted" => TerminalState::Interrupted,
                    _ => TerminalState::Failed,
                };
                if name == "D" && interrupt_request_ns.is_some() {
                    interrupt_terminal_latency_ms = interrupt_request_ns.map(|request_ns| {
                        timestamp_ns.saturating_sub(request_ns) as f64 / 1_000_000.0
                    });
                    pending_terminal = Some(terminal_state);
                    if !interrupt_response_seen {
                        continue;
                    }
                    break;
                }
                pending_terminal = Some(terminal_state);
                break;
            }
        }
        let terminal_state = pending_terminal.unwrap_or(TerminalState::Missing);
        if let Some(state) = agent_only_state.as_ref() {
            state.finish(name, &terminal_state)?;
        }
        let bytes: u64 = content.iter().map(|event| event.bytes).sum();
        let span = content
            .first()
            .zip(content.last())
            .map(|(first, last)| {
                Duration::from_nanos(last.timestamp_ns.saturating_sub(first.timestamp_ns))
            })
            .unwrap_or_default();
        let r1_sufficient = match name {
            "A" => {
                content.len() >= thresholds.a_min_events
                    && bytes >= thresholds.a_min_bytes
                    && span >= thresholds.a_min_span
            }
            "B" => content.len() >= thresholds.b_min_events && bytes >= thresholds.b_min_bytes,
            "D" => content.len() >= thresholds.d_min_events && bytes >= thresholds.d_min_bytes,
            _ => false,
        };
        Ok(ObservedScenario {
            evidence: ScenarioEvidence {
                name: name.to_owned(),
                turn_completed: terminal_state == TerminalState::Completed,
                terminal_state,
                first_delta_seen: !content.is_empty(),
                r1_sufficient,
                approval_seen,
                interrupt_response_seen,
            },
            content,
            interrupt_response_latency_ms,
            interrupt_terminal_latency_ms,
        })
    }

    fn approve_exact_command(
        &mut self,
        frame: &Value,
        thread_id: &str,
        turn_id: &str,
        workspace: &Path,
        command: &str,
        wrapper_candidate: Option<&str>,
        explicit_wrapper: Option<&TrustedApprovalWrapper>,
        deadline: Instant,
    ) -> Result<()> {
        let id = frame.get("id").cloned().context("approval missing id")?;
        let method = frame.get("method").and_then(Value::as_str);
        let params = frame.get("params").context("approval missing params")?;
        let cwd = params.get("cwd").and_then(Value::as_str).map(PathBuf::from);
        let workspace_root = workspace.canonicalize().ok();
        let canonical_cwd = cwd.as_deref().and_then(|path| path.canonicalize().ok());
        let raw_command = params.get("command").and_then(Value::as_str);
        let action_consistency = params.get("commandActions").map(|value| {
            value.as_array().is_some_and(|actions| {
                actions.len() == 1
                    && actions[0].get("command").and_then(Value::as_str) == Some(command)
            })
        });
        let direct_command =
            raw_command == Some(command) && action_consistency.is_none_or(|consistent| consistent);
        let exact_wrapped_command = wrapper_candidate.is_some_and(|wrapper| {
            raw_command == Some(wrapper)
                && action_consistency == Some(true)
                && explicit_wrapper.map_or(true, TrustedApprovalWrapper::path_identity_matches)
        });
        let safe = method == Some("item/commandExecution/requestApproval")
            && params.get("threadId").and_then(Value::as_str) == Some(thread_id)
            && params.get("turnId").and_then(Value::as_str) == Some(turn_id)
            && (direct_command || exact_wrapped_command)
            && workspace_root
                .as_ref()
                .is_some_and(|root| canonical_cwd.as_ref().is_some_and(|path| path == root));
        if !safe {
            self.send(&json!({"id":id,"result":{"decision":"cancel"}}))?;
            bail!(
                "unexpected approval rejected: method/command/cwd was outside the exact allowlist"
            );
        }
        if let Err(error) = ensure_approval_marker_absent_bounded(
            &self.marker_helper,
            workspace,
            deadline,
            self.global_deadline,
        ) {
            self.send(&json!({"id":id,"result":{"decision":"cancel"}}))?;
            return Err(error);
        }
        self.send(&json!({"id":id,"result":{"decision":"accept"}}))
            .map(|_| ())
    }

    fn reject_server_request(&mut self, frame: &Value) -> Result<()> {
        if let Some(id) = frame.get("id") {
            self.send(&json!({"id":id,"error":{"code":-32601,"message":"S2 runner rejected unexpected server request"}}))?;
        }
        Ok(())
    }

    fn recv(&mut self, scenario_deadline: Instant) -> Result<(u64, Value)> {
        if let Some(frame) = self.pending.pop_front() {
            return Ok(frame);
        }
        self.recv_wire(scenario_deadline)
    }

    fn recv_wire(&mut self, scenario_deadline: Instant) -> Result<(u64, Value)> {
        if self.inbound_overflow.load(Ordering::Acquire) {
            bail!("protocol error: bounded inbound queue overflow");
        }
        let deadline = scenario_deadline.min(self.global_deadline);
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .context("S2 timeout expired")?;
        match self.incoming.recv_timeout(remaining) {
            Ok(Inbound::Frame {
                timestamp_ns,
                value,
            }) => Ok((timestamp_ns, value)),
            Ok(Inbound::Malformed(error)) => bail!("protocol error: {error}"),
            Ok(Inbound::Closed) => bail!("protocol error: app-server stdout closed"),
            Err(mpsc::RecvTimeoutError::Timeout) => bail!("S2 timeout expired"),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                bail!("protocol error: app-server reader stopped")
            }
        }
    }

    fn stop(&mut self) -> Result<()> {
        let cleanup = self.contained.terminate_and_wait_bounded();
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut done = std::collections::HashSet::new();
        while done.len() < 2 {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            match self.reader_done.recv_timeout(remaining) {
                Ok(label) => {
                    done.insert(label);
                }
                Err(_) => break,
            }
        }
        if done.contains("stdout") {
            if let Some(handle) = self.stdout_thread.take() {
                let _ = handle.join();
            }
        } else {
            self.stdout_thread.take();
        }
        if done.contains("stderr") {
            if let Some(handle) = self.stderr_thread.take() {
                let _ = handle.join();
            }
        } else {
            self.stderr_thread.take();
        }
        cleanup
    }
}

fn read_line_checked(reader: &mut impl BufRead) -> std::io::Result<Option<String>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    while matches!(line.as_bytes().last(), Some(b'\n' | b'\r')) {
        line.pop();
    }
    Ok(Some(line))
}

fn remaining_until(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .context("S2 scenario timeout expired")
}

fn remaining_global(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .context("S2 global timeout expired")
}

fn spawn_contained(mut process: Command) -> Result<ContainedChild> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setpgid is async-signal-safe and called before exec in the child.
        unsafe {
            process.pre_exec(|| {
                if setpgid(0, 0) == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;
        process.creation_flags(CREATE_SUSPENDED);
    }
    let mut child = process.spawn()?;
    #[cfg(windows)]
    let job = match contain_and_resume_windows_child(&child) {
        Ok(job) => job,
        Err(error) => {
            let _ = child.kill();
            let _ = wait_child_bounded(&mut child);
            return Err(error);
        }
    };
    #[cfg(unix)]
    let process_group = Some(child.id() as i32);
    Ok(ContainedChild {
        child,
        #[cfg(windows)]
        job,
        #[cfg(unix)]
        process_group,
    })
}

fn wait_until_reaped_with(
    deadline: Instant,
    mut poll: impl FnMut() -> std::io::Result<bool>,
    mut terminate: impl FnMut() -> std::io::Result<()>,
) -> Result<()> {
    let mut terminate_sent = false;
    loop {
        if matches!(poll(), Ok(true)) {
            return Ok(());
        }
        if !terminate_sent {
            let _ = terminate();
            terminate_sent = true;
        }
        if Instant::now() >= deadline {
            bail!("contained direct child cleanup failed");
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn wait_child_bounded(child: &mut Child) -> Result<()> {
    use std::cell::RefCell;

    let deadline = Instant::now() + Duration::from_secs(1);
    let child = RefCell::new(child);
    wait_until_reaped_with(
        deadline,
        || child.borrow_mut().try_wait().map(|status| status.is_some()),
        || child.borrow_mut().kill(),
    )
}

#[cfg(windows)]
fn contain_and_resume_windows_child(
    child: &Child,
) -> Result<windows_sys::Win32::Foundation::HANDLE> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};
    // SAFETY: null security/name create an unnamed job; child handle is live.
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() || job == INVALID_HANDLE_VALUE {
        bail!(
            "CreateJobObjectW failed: {}",
            std::io::Error::last_os_error()
        );
    }
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            std::mem::size_of_val(&limits) as u32,
        )
    } == 0
    {
        let error = std::io::Error::last_os_error();
        unsafe { CloseHandle(job) };
        bail!("SetInformationJobObject failed: {error}");
    }
    if unsafe { AssignProcessToJobObject(job, child.as_raw_handle()) } == 0 {
        let error = std::io::Error::last_os_error();
        unsafe { CloseHandle(job) };
        bail!("AssignProcessToJobObject failed: {error}");
    }

    // `std::process::Child` does not retain the primary thread handle. Because
    // CREATE_SUSPENDED prevents any child code from running, the snapshot has
    // exactly the suspended primary thread for this PID at this point.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        let error = std::io::Error::last_os_error();
        terminate_and_close_job(job);
        bail!("CreateToolhelp32Snapshot failed: {error}");
    }
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut found = false;
    let mut has_entry = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    while has_entry {
        if entry.th32OwnerProcessID == child.id() {
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if thread.is_null() {
                let error = std::io::Error::last_os_error();
                unsafe {
                    CloseHandle(snapshot);
                }
                terminate_and_close_job(job);
                bail!("OpenThread failed: {error}");
            }
            let resumed = unsafe { ResumeThread(thread) };
            unsafe { CloseHandle(thread) };
            if resumed == u32::MAX {
                let error = std::io::Error::last_os_error();
                unsafe {
                    CloseHandle(snapshot);
                }
                terminate_and_close_job(job);
                bail!("ResumeThread failed: {error}");
            }
            found = true;
            break;
        }
        has_entry = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    if !found {
        terminate_and_close_job(job);
        bail!("suspended child primary thread was not found");
    }
    Ok(job)
}

#[cfg(windows)]
fn terminate_and_close_job(job: windows_sys::Win32::Foundation::HANDLE) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::TerminateJobObject;
    unsafe {
        let _ = TerminateJobObject(job, 1);
        CloseHandle(job);
    }
}

#[cfg(unix)]
unsafe extern "C" {
    fn setpgid(pid: i32, pgid: i32) -> i32;
    fn kill(pid: i32, signal: i32) -> i32;
}

#[cfg(test)]
fn delta_bytes(frame: &Value, thread_id: &str, turn_id: &str) -> Option<u64> {
    let method = frame.get("method").and_then(Value::as_str)?;
    if !matches!(
        method,
        "item/agentMessage/delta" | "item/commandExecution/outputDelta"
    ) || frame.pointer("/params/threadId").and_then(Value::as_str) != Some(thread_id)
        || frame.pointer("/params/turnId").and_then(Value::as_str) != Some(turn_id)
    {
        return None;
    }
    frame
        .pointer("/params/delta")
        .and_then(Value::as_str)
        .filter(|delta| !delta.is_empty())
        .map(|delta| delta.len() as u64)
}

fn agent_delta_bytes(frame: &Value, thread_id: &str, turn_id: &str) -> Option<u64> {
    (frame.get("method").and_then(Value::as_str) == Some("item/agentMessage/delta")
        && frame.pointer("/params/threadId").and_then(Value::as_str) == Some(thread_id)
        && frame.pointer("/params/turnId").and_then(Value::as_str) == Some(turn_id))
    .then(|| {
        frame
            .pointer("/params/delta")
            .and_then(Value::as_str)
            .filter(|delta| !delta.is_empty())
            .map(|delta| delta.len() as u64)
    })
    .flatten()
}

struct AgentOnlyState<'a> {
    prompt: &'a str,
    workspace: &'a Path,
    thread_started: bool,
    turn_started: bool,
    user_raw_seen: bool,
    user_item_id: Option<String>,
    user_completed: bool,
    mcp_startup: HashMap<String, PinnedMcpStartupProgress>,
    items: HashMap<String, AgentItemState>,
    expected_raw_order: VecDeque<String>,
    raw_items: Vec<Value>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AgentItemKind {
    AgentMessage,
    Reasoning,
    Plan,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PinnedMcpStartupProgress {
    Starting,
    Terminal,
}

#[derive(Clone, Copy, Debug)]
struct AgentItemState {
    kind: AgentItemKind,
    phase: Option<PinnedMessagePhase>,
    completed: bool,
    raw_seen: bool,
}

impl<'a> AgentOnlyState<'a> {
    fn new(prompt: &'a str, workspace: &'a Path) -> Self {
        Self {
            prompt,
            workspace,
            thread_started: false,
            turn_started: false,
            user_raw_seen: false,
            user_item_id: None,
            user_completed: false,
            mcp_startup: HashMap::new(),
            items: HashMap::new(),
            expected_raw_order: VecDeque::new(),
            raw_items: Vec::new(),
        }
    }

    fn validate(&mut self, frame: &Value, thread_id: &str, turn_id: &str) -> Result<()> {
        let Some(method) = frame.get("method").and_then(Value::as_str) else {
            return Ok(());
        };
        let params = frame.get("params").and_then(Value::as_object);
        let exact_thread = params
            .and_then(|value| value.get("threadId"))
            .and_then(Value::as_str)
            == Some(thread_id);
        let exact_turn = params
            .and_then(|value| value.get("turnId"))
            .and_then(Value::as_str)
            .or_else(|| frame.pointer("/params/turn/id").and_then(Value::as_str))
            == Some(turn_id);
        match method {
            "thread/started" => {
                if self.thread_started
                    || self.turn_started
                    || self.user_raw_seen
                    || self.user_item_id.is_some()
                    || !valid_thread_started(frame, thread_id, self.workspace)
                {
                    bail!("protocol error: malformed, duplicate, or out-of-order thread/started");
                }
                self.thread_started = true;
            }
            "turn/started" => {
                if !exact_thread || !exact_turn {
                    bail!("protocol error: agent-only notification had mismatched turn identity");
                }
                if !self.thread_started || self.turn_started {
                    bail!("protocol error: duplicate or out-of-order turn/started");
                }
                self.turn_started = true;
            }
            "mcpServer/startupStatus/updated" => {
                let params = frame
                    .get("params")
                    .and_then(Value::as_object)
                    .context("protocol error: malformed MCP startup status notification")?;
                if params.len() != 4
                    || ["threadId", "name", "status", "error"]
                        .iter()
                        .any(|field| !params.contains_key(*field))
                {
                    bail!("protocol error: malformed MCP startup status notification");
                }
                let status = serde_json::from_value::<PinnedMcpStartupStatusParams>(Value::Object(
                    params.clone(),
                ))
                .context("protocol error: malformed MCP startup status notification")?;
                if status.name.is_empty()
                    || match status.status {
                        PinnedMcpStartupStatus::Failed => {
                            status.error.as_deref().is_none_or(str::is_empty)
                        }
                        _ => status.error.is_some(),
                    }
                {
                    bail!("protocol error: malformed MCP startup status notification");
                }
                if !self.thread_started || status.thread_id != thread_id {
                    bail!(
                        "protocol error: MCP startup status had mismatched thread identity or order"
                    );
                }
                match status.status {
                    PinnedMcpStartupStatus::Starting => match self.mcp_startup.entry(status.name) {
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            entry.insert(PinnedMcpStartupProgress::Starting);
                        }
                        std::collections::hash_map::Entry::Occupied(_) => {
                            bail!("protocol error: invalid MCP startup status transition");
                        }
                    },
                    PinnedMcpStartupStatus::Ready
                    | PinnedMcpStartupStatus::Failed
                    | PinnedMcpStartupStatus::Cancelled => {
                        let progress = self
                            .mcp_startup
                            .get_mut(&status.name)
                            .context("protocol error: invalid MCP startup status transition")?;
                        if *progress != PinnedMcpStartupProgress::Starting {
                            bail!("protocol error: invalid MCP startup status transition");
                        }
                        *progress = PinnedMcpStartupProgress::Terminal;
                    }
                }
            }
            "warning" => {
                let warning = serde_json::from_value::<PinnedWarningParams>(
                    frame
                        .get("params")
                        .cloned()
                        .context("protocol error: malformed warning notification")?,
                )
                .context("protocol error: malformed warning notification")?;
                if warning.message.is_empty() {
                    bail!("protocol error: malformed warning notification");
                }
                if !self.thread_started || warning.thread_id != thread_id {
                    bail!("protocol error: warning had mismatched thread identity or order");
                }
            }
            "turn/completed" | "turn/plan/updated" => {
                if !exact_thread || !exact_turn || !self.turn_started || !self.user_completed {
                    bail!("protocol error: mismatched or out-of-order turn notification");
                }
            }
            "thread/tokenUsage/updated" | "turn/moderationMetadata" => {
                if !exact_thread || !exact_turn {
                    bail!("protocol error: benign notification had mismatched turn identity");
                }
            }
            "thread/name/updated" | "thread/status/changed" => {
                if !exact_thread {
                    bail!("protocol error: benign notification had mismatched thread identity");
                }
            }
            "item/agentMessage/delta"
            | "item/plan/delta"
            | "item/reasoning/textDelta"
            | "item/reasoning/summaryTextDelta"
            | "item/reasoning/summaryPartAdded" => {
                if !exact_thread || !exact_turn || !self.turn_started || !self.user_completed {
                    bail!("protocol error: mismatched or out-of-order agent notification");
                }
                let item_id = frame
                    .pointer("/params/itemId")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .context("protocol error: agent delta itemId was missing")?;
                let expected_kind = match method {
                    "item/agentMessage/delta" => AgentItemKind::AgentMessage,
                    "item/plan/delta" => AgentItemKind::Plan,
                    _ => AgentItemKind::Reasoning,
                };
                let structurally_valid = match method {
                    "item/reasoning/summaryPartAdded" => frame
                        .pointer("/params/summaryIndex")
                        .and_then(Value::as_u64)
                        .is_some(),
                    "item/reasoning/textDelta" => {
                        frame.pointer("/params/delta").is_some_and(Value::is_string)
                            && frame
                                .pointer("/params/contentIndex")
                                .and_then(Value::as_u64)
                                .is_some()
                    }
                    "item/reasoning/summaryTextDelta" => {
                        frame.pointer("/params/delta").is_some_and(Value::is_string)
                            && frame
                                .pointer("/params/summaryIndex")
                                .and_then(Value::as_u64)
                                .is_some()
                    }
                    _ => frame.pointer("/params/delta").is_some_and(Value::is_string),
                };
                if !structurally_valid {
                    bail!("protocol error: malformed agent delta notification");
                }
                let item = self
                    .items
                    .get(item_id)
                    .context("protocol error: delta arrived before its item lifecycle started")?;
                if item.kind != expected_kind || item.completed {
                    bail!("protocol error: delta item kind or lifecycle state was invalid");
                }
            }
            "item/started" | "item/completed" => {
                let item_type = frame.pointer("/params/item/type").and_then(Value::as_str);
                if !exact_thread || !exact_turn || !valid_item_timestamp(frame, method) {
                    bail!("protocol error: malformed item lifecycle notification");
                }
                if item_type == Some("userMessage") {
                    let item_id = valid_user_message_item(frame, self.prompt)
                        .context("protocol error: malformed or mismatched user message")?;
                    if method == "item/started" {
                        if !self.thread_started
                            || !self.turn_started
                            || self.user_item_id.is_some()
                            || self.user_completed
                        {
                            bail!("protocol error: duplicate or out-of-order user message start");
                        }
                        self.user_item_id = Some(item_id.to_owned());
                    } else {
                        if self.user_completed || self.user_item_id.as_deref() != Some(item_id) {
                            bail!(
                                "protocol error: mismatched or out-of-order user message completion"
                            );
                        }
                        self.user_completed = true;
                    }
                } else {
                    if !self.user_completed {
                        bail!("protocol error: agent item arrived before user lifecycle completed");
                    }
                    let (item_id, kind, phase) = valid_agent_item(frame).context(
                        "protocol error: tool, unknown, or malformed item is forbidden in agent-only scenario",
                    )?;
                    if method == "item/started" {
                        if self.items.contains_key(item_id) {
                            bail!("protocol error: duplicate agent item start");
                        }
                        self.items.insert(
                            item_id.to_owned(),
                            AgentItemState {
                                kind,
                                phase,
                                completed: false,
                                raw_seen: false,
                            },
                        );
                    } else {
                        let state = self.items.get_mut(item_id).context(
                            "protocol error: agent item completion arrived before start",
                        )?;
                        if state.kind != kind || state.phase != phase || state.completed {
                            bail!("protocol error: mismatched or duplicate agent item completion");
                        }
                        state.completed = true;
                        if matches!(kind, AgentItemKind::AgentMessage | AgentItemKind::Reasoning) {
                            self.expected_raw_order.push_back(item_id.to_owned());
                        }
                    }
                }
            }
            "rawResponseItem/completed" => {
                let raw_item = frame.pointer("/params/item");
                if !exact_thread || !exact_turn || !self.turn_started {
                    bail!("protocol error: raw response item had mismatched identity or order");
                }
                let raw_is_user = raw_item
                    .and_then(Value::as_object)
                    .and_then(|item| item.get("role"))
                    .and_then(Value::as_str)
                    == Some("user");
                if raw_is_user {
                    if self.user_raw_seen || !self.items.is_empty() {
                        bail!("protocol error: duplicate or out-of-order user raw item");
                    }
                    if !valid_user_raw_response_item(raw_item, self.prompt) {
                        bail!("protocol error: malformed or mismatched user raw item");
                    }
                    self.user_raw_seen = true;
                    return Ok(());
                }
                if !self.user_completed {
                    bail!(
                        "protocol error: non-user raw item arrived before user lifecycle completed"
                    );
                }
                let raw_item = raw_item.context("protocol error: raw response item was missing")?;
                if self.raw_items.iter().any(|seen| seen == raw_item) {
                    bail!("protocol error: duplicate raw response item");
                }
                let (kind, phase) = valid_benign_raw_response_item(Some(raw_item))
                    .context("protocol error: malformed or tool raw response item is forbidden")?;
                let expected_id = self
                    .expected_raw_order
                    .front()
                    .context("protocol error: raw response item was out of order or duplicate")?;
                let state = self
                    .items
                    .get_mut(expected_id)
                    .context("protocol error: raw response item was out of order or duplicate")?;
                if state.kind != kind || state.phase != phase || !state.completed || state.raw_seen
                {
                    bail!("protocol error: raw response item was out of order or duplicate");
                }
                state.raw_seen = true;
                self.expected_raw_order.pop_front();
                self.raw_items.push(raw_item.clone());
            }
            "item/commandExecution/outputDelta"
            | "item/commandExecution/terminalInteraction"
            | "item/fileChange/outputDelta"
            | "item/fileChange/patchUpdated"
            | "item/mcpToolCall/progress"
            | "item/autoApprovalReview/started"
            | "item/autoApprovalReview/completed"
            | "hook/started"
            | "turn/diff/updated" => {
                bail!("protocol error: tool activity is forbidden in agent-only scenario");
            }
            _ => bail!("protocol error: unexpected notification in agent-only scenario"),
        }
        Ok(())
    }

    fn finish(&self, scenario: &str, terminal_state: &TerminalState) -> Result<()> {
        if !self.thread_started
            || !self.turn_started
            || self.user_item_id.is_none()
            || !self.user_completed
        {
            bail!("protocol error: required agent-only lifecycle notifications were missing");
        }
        let assistant_seen = self
            .items
            .values()
            .any(|item| item.kind == AgentItemKind::AgentMessage);
        if !assistant_seen {
            bail!("protocol error: assistant agent message lifecycle was missing");
        }
        if matches!(scenario, "A" | "B")
            && (*terminal_state != TerminalState::Completed
                || !self.items.values().all(|item| item.completed)
                || !self
                    .items
                    .values()
                    .any(|item| item.kind == AgentItemKind::AgentMessage && item.completed))
        {
            bail!("protocol error: completed scenario lacked a complete assistant lifecycle");
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PinnedThreadStartedParams {
    thread: PinnedThread,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PinnedThread {
    id: String,
    session_id: String,
    forked_from_id: Option<String>,
    parent_thread_id: Option<String>,
    preview: String,
    ephemeral: bool,
    model_provider: String,
    created_at: i64,
    updated_at: i64,
    status: PinnedThreadStatus,
    path: Option<PathBuf>,
    cwd: PathBuf,
    cli_version: String,
    source: PinnedSessionSource,
    thread_source: Option<PinnedThreadSource>,
    agent_nickname: Option<String>,
    agent_role: Option<String>,
    git_info: Option<PinnedGitInfo>,
    name: Option<String>,
    turns: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PinnedMcpStartupStatusParams {
    thread_id: String,
    name: String,
    status: PinnedMcpStartupStatus,
    error: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PinnedWarningParams {
    thread_id: String,
    message: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
enum PinnedMcpStartupStatus {
    Starting,
    Ready,
    Failed,
    Cancelled,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum PinnedThreadStatus {
    Idle,
}

#[derive(Deserialize)]
enum PinnedSessionSource {
    #[serde(rename = "vscode")]
    VsCode,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum PinnedThreadSource {
    User,
    Subagent,
    MemoryConsolidation,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)]
struct PinnedGitInfo {
    sha: Option<String>,
    branch: Option<String>,
    origin_url: Option<String>,
}

fn valid_thread_started(frame: &Value, thread_id: &str, workspace: &Path) -> bool {
    const FIELDS: [&str; 20] = [
        "id",
        "sessionId",
        "forkedFromId",
        "parentThreadId",
        "preview",
        "ephemeral",
        "modelProvider",
        "createdAt",
        "updatedAt",
        "status",
        "path",
        "cwd",
        "cliVersion",
        "source",
        "threadSource",
        "agentNickname",
        "agentRole",
        "gitInfo",
        "name",
        "turns",
    ];
    let Some(params) = frame.get("params") else {
        return false;
    };
    let Some(object) = params.get("thread").and_then(Value::as_object) else {
        return false;
    };
    if object.len() != FIELDS.len() || FIELDS.iter().any(|field| !object.contains_key(*field)) {
        return false;
    }
    let Ok(params) = serde_json::from_value::<PinnedThreadStartedParams>(params.clone()) else {
        return false;
    };
    let thread = params.thread;
    thread.id == thread_id
        && thread.session_id == thread_id
        && thread.forked_from_id.is_none()
        && thread.parent_thread_id.is_none()
        && thread.preview.is_empty()
        && thread.ephemeral
        && !thread.model_provider.is_empty()
        && thread.created_at >= 0
        && thread.updated_at == thread.created_at
        && matches!(thread.status, PinnedThreadStatus::Idle)
        && thread.path.is_none()
        && pinned_thread_cwd_matches(&thread.cwd, workspace)
        && thread.cli_version == "0.139.0"
        && matches!(thread.source, PinnedSessionSource::VsCode)
        && thread.thread_source.is_none()
        && thread.agent_nickname.is_none()
        && thread.agent_role.is_none()
        && thread.git_info.is_none()
        && thread.name.is_none()
        && thread.turns.is_empty()
}

fn pinned_thread_cwd_matches(observed: &Path, expected: &Path) -> bool {
    #[cfg(windows)]
    {
        let Ok(observed) = normalize_windows_command_path(observed.to_owned()) else {
            return false;
        };
        let Ok(expected) = normalize_windows_command_path(expected.to_owned()) else {
            return false;
        };
        observed.as_os_str() == expected.as_os_str()
    }
    #[cfg(not(windows))]
    {
        observed.as_os_str() == expected.as_os_str()
    }
}

fn valid_item_timestamp(frame: &Value, method: &str) -> bool {
    let field = if method == "item/started" {
        "startedAtMs"
    } else {
        "completedAtMs"
    };
    frame
        .pointer(&format!("/params/{field}"))
        .and_then(Value::as_i64)
        .is_some_and(|timestamp| timestamp >= 0)
}

fn valid_user_message_item<'a>(frame: &'a Value, prompt: &str) -> Result<&'a str> {
    let item = frame
        .pointer("/params/item")
        .and_then(Value::as_object)
        .context("user message item must be an object")?;
    let item_id = item
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .context("user message item id must be nonempty")?;
    if item.len() != 4
        || item.get("type").and_then(Value::as_str) != Some("userMessage")
        || !item.get("clientId").is_some_and(Value::is_null)
    {
        bail!("user message item discriminator was invalid");
    }
    let content = item
        .get("content")
        .and_then(Value::as_array)
        .filter(|content| content.len() == 1)
        .context("user message must contain exactly one input")?;
    let text = content[0]
        .as_object()
        .context("user message input must be an object")?;
    if text.len() != 3
        || text.get("type").and_then(Value::as_str) != Some("text")
        || text.get("text").and_then(Value::as_str) != Some(prompt)
        || !text
            .get("text_elements")
            .is_some_and(|value| value.as_array().is_some_and(Vec::is_empty))
    {
        bail!("user message input did not byte-match the submitted prompt");
    }
    Ok(item_id)
}

fn valid_agent_item(frame: &Value) -> Option<(&str, AgentItemKind, Option<PinnedMessagePhase>)> {
    let item = frame.pointer("/params/item")?.as_object()?;
    let id = item.get("id")?.as_str().filter(|value| !value.is_empty())?;
    let (kind, phase) = match item.get("type")?.as_str()? {
        "agentMessage" => {
            if item.len() != 5 || !item.get("memoryCitation")?.is_null() {
                return None;
            }
            let phase =
                serde_json::from_value::<Option<PinnedMessagePhase>>(item.get("phase")?.clone())
                    .ok()?;
            if phase == Some(PinnedMessagePhase::Commentary) {
                return None;
            }
            item.get("text")?.as_str()?;
            (AgentItemKind::AgentMessage, phase)
        }
        "reasoning" => {
            if item.len() != 4
                || !item
                    .get("summary")?
                    .as_array()?
                    .iter()
                    .all(Value::is_string)
                || !item
                    .get("content")?
                    .as_array()?
                    .iter()
                    .all(Value::is_string)
            {
                return None;
            }
            (AgentItemKind::Reasoning, None)
        }
        "plan" => {
            if item.len() != 3 {
                return None;
            }
            item.get("text")?.as_str()?;
            (AgentItemKind::Plan, None)
        }
        _ => return None,
    };
    Some((id, kind, phase))
}

fn valid_user_raw_response_item(item: Option<&Value>, prompt: &str) -> bool {
    let Some(item) = item else {
        return false;
    };
    serde_json::from_value::<PinnedRawMessage>(item.clone()).is_ok_and(|item| {
        item.kind == "message"
            && item.role == "user"
            && item.phase.is_none()
            && matches!(item.content.as_slice(), [PinnedContent::InputText { text }] if text == prompt)
    })
}

fn valid_benign_raw_response_item(
    item: Option<&Value>,
) -> Option<(AgentItemKind, Option<PinnedMessagePhase>)> {
    let Some(item) = item else {
        return None;
    };
    match item.get("type").and_then(Value::as_str) {
        Some("message") => serde_json::from_value::<PinnedRawMessage>(item.clone())
            .ok()
            .filter(|item| {
                item.kind == "message"
                    && item.role == "assistant"
                    && item.phase != Some(PinnedMessagePhase::Commentary)
                    && !item.content.is_empty()
                    && item
                        .content
                        .iter()
                        .all(|part| matches!(part, PinnedContent::OutputText { text } if !text.is_empty()))
            })
            .map(|item| (AgentItemKind::AgentMessage, item.phase)),
        Some("reasoning") => serde_json::from_value::<PinnedRawReasoning>(item.clone())
            .ok()
            .filter(valid_reasoning_raw)
            .map(|_| (AgentItemKind::Reasoning, None)),
        _ => None,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PinnedRawMessage {
    #[serde(rename = "type")]
    kind: String,
    role: String,
    content: Vec<PinnedContent>,
    phase: Option<PinnedMessagePhase>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum PinnedContent {
    InputText { text: String },
    OutputText { text: String },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum PinnedMessagePhase {
    Commentary,
    FinalAnswer,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PinnedRawReasoning {
    #[serde(rename = "type")]
    kind: String,
    summary: Vec<PinnedReasoningSummary>,
    content: Option<Vec<PinnedReasoningContent>>,
    encrypted_content: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum PinnedReasoningSummary {
    SummaryText { text: String },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum PinnedReasoningContent {
    ReasoningText { text: String },
    Text { text: String },
}

fn valid_reasoning_raw(item: &PinnedRawReasoning) -> bool {
    item.kind == "reasoning"
        && item.summary.iter().all(
            |part| matches!(part, PinnedReasoningSummary::SummaryText { text } if !text.is_empty()),
        )
        && item.content.as_ref().is_none_or(|parts| {
            parts.iter().all(|part| match part {
                PinnedReasoningContent::ReasoningText { text }
                | PinnedReasoningContent::Text { text } => !text.is_empty(),
            })
        })
        && item
            .encrypted_content
            .as_ref()
            .is_none_or(|content| !content.is_empty())
}

fn terminal_status<'a>(frame: &'a Value, thread_id: &str, turn_id: &str) -> Option<&'a str> {
    (frame.get("method").and_then(Value::as_str) == Some("turn/completed")
        && frame.pointer("/params/threadId").and_then(Value::as_str) == Some(thread_id)
        && frame.pointer("/params/turn/id").and_then(Value::as_str) == Some(turn_id))
    .then(|| frame.pointer("/params/turn/status").and_then(Value::as_str))
    .flatten()
}

fn fail_on_error_notification(frame: &Value) -> Result<()> {
    if frame.get("method").and_then(Value::as_str) == Some("error") {
        bail!(
            "server error notification: {}",
            sanitized_error(frame.get("params").unwrap_or(&Value::Null))
        );
    }
    Ok(())
}

fn sanitized_error(value: &Value) -> String {
    value
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/error/message").and_then(Value::as_str))
        .unwrap_or("unspecified server error")
        .chars()
        .take(240)
        .collect()
}

fn validate_initialize(frame: &Value) -> Result<()> {
    let result = frame
        .get("result")
        .and_then(Value::as_object)
        .context("initialize malformed success: result must be an object")?;
    let nonempty_string = |key: &str| {
        result
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
    };
    if !nonempty_string("userAgent")
        || !nonempty_string("platformFamily")
        || !nonempty_string("platformOs")
        || !result
            .get("codexHome")
            .and_then(Value::as_str)
            .is_some_and(|path| Path::new(path).is_absolute())
    {
        bail!("initialize malformed success: missing pinned 0.139 structural fields");
    }
    Ok(())
}

fn validate_account(frame: &Value) -> Result<()> {
    let result = frame
        .get("result")
        .and_then(Value::as_object)
        .context("account/read malformed success: result must be an object")?;
    let requires_auth = result
        .get("requiresOpenaiAuth")
        .and_then(Value::as_bool)
        .context("account/read malformed success: requiresOpenaiAuth must be boolean")?;
    match result.get("account") {
        None | Some(Value::Null) if requires_auth => {
            bail!("authentication required: account/read returned no account")
        }
        None => bail!("account/read malformed success: account field is missing"),
        Some(Value::Null) => Ok(()),
        Some(Value::Object(account)) => match account.get("type").and_then(Value::as_str) {
            Some("apiKey") | Some("amazonBedrock") => Ok(()),
            Some("chatgpt")
                if account.get("email").is_some_and(Value::is_string)
                    && account.get("planType").is_some_and(Value::is_string) =>
            {
                Ok(())
            }
            _ => bail!("account/read malformed success: invalid account structure"),
        },
        Some(_) => bail!("account/read malformed success: account must be object or null"),
    }
}

fn validate_rate_limits(frame: &Value) -> Result<()> {
    let result = frame
        .get("result")
        .and_then(Value::as_object)
        .context("account/rateLimits/read malformed success: result must be an object")?;
    let snapshot = result
        .get("rateLimits")
        .and_then(Value::as_object)
        .context("account/rateLimits/read malformed success: rateLimits must be an object")?;
    for window_name in ["primary", "secondary"] {
        if let Some(value) = snapshot.get(window_name).filter(|value| !value.is_null()) {
            let window = value.as_object().with_context(|| {
                format!(
                    "account/rateLimits/read malformed success: {window_name} must be an object"
                )
            })?;
            if !window.get("usedPercent").is_some_and(Value::is_number) {
                bail!(
                    "account/rateLimits/read malformed success: {window_name} usedPercent must be numeric"
                );
            }
        }
    }
    if snapshot
        .get("rateLimitReachedType")
        .is_some_and(|value| !value.is_null() && !value.is_string())
    {
        bail!(
            "account/rateLimits/read malformed success: rateLimitReachedType must be string or null"
        );
    }
    Ok(())
}

fn validate_effective_config(frame: &Value, isolated_codex_home: &Path) -> Result<()> {
    let result = frame
        .get("result")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("config preflight failed [CFG_RESULT_SHAPE]"))?;
    if result
        .keys()
        .any(|key| !matches!(key.as_str(), "config" | "origins" | "layers"))
    {
        bail!("config preflight failed [CFG_RESULT_SHAPE]");
    }
    let config_value = result
        .get("config")
        .ok_or_else(|| anyhow!("config preflight failed [CFG_RESULT_SHAPE]"))?;
    let config = config_value
        .as_object()
        .ok_or_else(|| anyhow!("config preflight failed [CFG_RESULT_SHAPE]"))?;
    let layers = result
        .get("layers")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("config preflight failed [CFG_RESULT_SHAPE]"))?;
    let origins = result
        .get("origins")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("config preflight failed [CFG_RESULT_SHAPE]"))?;
    let expected_user_config = isolated_codex_home.join("config.toml");
    let expected_system_config = expected_system_config_path()
        .map_err(|_| anyhow!("config preflight failed [CFG_SYSTEM_PATH]"))?;
    let session_name = json!({"type":"sessionFlags"});
    let user_name_matches = |name: &Value| {
        name.as_object().is_some_and(|object| {
            object.len() == 3
                && object.get("type") == Some(&json!("user"))
                && object.get("profile") == Some(&Value::Null)
                && object
                    .get("file")
                    .and_then(Value::as_str)
                    .is_some_and(|file| {
                        pinned_thread_cwd_matches(Path::new(file), &expected_user_config)
                    })
        })
    };
    fn system_name_file(name: &Value) -> Option<&str> {
        let object = name.as_object()?;
        if object.len() != 2 || object.get("type") != Some(&json!("system")) {
            return None;
        }
        object.get("file").and_then(Value::as_str)
    }
    fn layer_object(layer: &Value) -> Option<&serde_json::Map<String, Value>> {
        layer.as_object().filter(|object| {
            object.len() == 3
                && object.contains_key("name")
                && object.get("version").is_some_and(Value::is_string)
                && object.contains_key("config")
        })
    }
    if layers.len() != 3 {
        bail!("config preflight failed [CFG_LAYER_COUNT]");
    }
    let Some(session_layer) = layers.first().and_then(layer_object) else {
        bail!("config preflight failed [CFG_LAYER_SESSION]");
    };
    let Some(user_layer) = layers.get(1).and_then(layer_object) else {
        bail!("config preflight failed [CFG_LAYER_USER]");
    };
    let Some(system_layer) = layers.get(2).and_then(layer_object) else {
        bail!("config preflight failed [CFG_LAYER_SYSTEM]");
    };
    if session_layer.get("name") != Some(&session_name)
        || session_layer.get("config") != Some(&expected_session_layer_config())
    {
        bail!("config preflight failed [CFG_LAYER_SESSION]");
    }
    if !user_layer.get("name").is_some_and(user_name_matches)
        || user_layer.get("config") != Some(&expected_user_layer_config())
    {
        bail!("config preflight failed [CFG_LAYER_USER]");
    }
    let Some(system_file) = system_layer.get("name").and_then(system_name_file) else {
        bail!("config preflight failed [CFG_LAYER_SYSTEM]");
    };
    if system_layer.get("config") != Some(&json!({})) {
        bail!("config preflight failed [CFG_LAYER_SYSTEM]");
    }
    if !pinned_thread_cwd_matches(Path::new(system_file), &expected_system_config) {
        bail!("config preflight failed [CFG_SYSTEM_PATH]");
    }
    let session_version = session_layer.get("version").unwrap();
    let user_version = user_layer.get("version").unwrap();
    let session_origin_matches = |origin: &Value| {
        origin.as_object().is_some_and(|object| {
            object.len() == 2
                && object.get("name") == Some(&session_name)
                && object.get("version") == Some(session_version)
        })
    };
    let user_origin_matches = |origin: &Value| {
        origin.as_object().is_some_and(|object| {
            object.len() == 2
                && object.get("name").is_some_and(user_name_matches)
                && object.get("version") == Some(user_version)
        })
    };
    const SESSION_ORIGINS: &[&str] = &[
        "project_root_markers.0",
        "project_doc_max_bytes",
        "skills.include_instructions",
        "skills.bundled.enabled",
        "analytics.enabled",
        "otel.exporter",
        "otel.trace_exporter",
        "otel.metrics_exporter",
        "features.hooks",
        "features.plugins",
        "features.apps",
        "features.shell_snapshot",
        "features.memories",
    ];
    if origins.len() != SESSION_ORIGINS.len() + 1 {
        bail!("config preflight failed [CFG_ORIGIN_COUNT]");
    }
    if !origins
        .get("cli_auth_credentials_store")
        .is_some_and(user_origin_matches)
    {
        bail!("config preflight failed [CFG_ORIGIN_USER]");
    }
    if SESSION_ORIGINS
        .iter()
        .any(|key| !origins.get(*key).is_some_and(session_origin_matches))
    {
        bail!("config preflight failed [CFG_ORIGIN_SESSION]");
    }
    let exact = |pointer: &str, expected: &Value| config_value.pointer(pointer) == Some(expected);
    if !exact("/cli_auth_credentials_store", &json!("file"))
        || !exact("/analytics/enabled", &json!(false))
        || !exact("/otel/exporter", &json!("none"))
        || !exact("/otel/trace_exporter", &json!("none"))
        || !exact("/otel/metrics_exporter", &json!("none"))
        || !exact("/skills/include_instructions", &json!(false))
        || !exact("/skills/bundled/enabled", &json!(false))
        || !config
            .get("notify")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
        || !exact("/project_doc_max_bytes", &json!(0))
        || !exact("/project_root_markers", &json!([".codex-s2-root"]))
        || config
            .get("mcp_servers")
            .is_some_and(|value| !value.as_object().is_some_and(serde_json::Map::is_empty))
        || config
            .get("model_providers")
            .is_some_and(|value| !value.as_object().is_some_and(serde_json::Map::is_empty))
        || config
            .get("experimental_thread_config_endpoint")
            .is_some_and(|value| !value.is_null())
    {
        bail!("config preflight failed [CFG_EFFECTIVE_FIXED]");
    }
    for feature in ["hooks", "plugins", "apps", "shell_snapshot", "memories"] {
        if config_value.pointer(&format!("/features/{feature}")) != Some(&json!(false)) {
            bail!(
                "config preflight failed [CFG_FEATURE_{}]",
                feature.to_ascii_uppercase()
            );
        }
    }
    Ok(())
}

fn expected_user_layer_config() -> Value {
    json!({
        "cli_auth_credentials_store":"file",
        "analytics":{"enabled":false},
        "otel":{"exporter":"none","trace_exporter":"none","metrics_exporter":"none"},
        "skills":{"include_instructions":false,"bundled":{"enabled":false}}
    })
}

fn expected_session_layer_config() -> Value {
    json!({
        "notify":[],"project_root_markers":[".codex-s2-root"],"project_doc_max_bytes":0,
        "skills":{"include_instructions":false,"bundled":{"enabled":false}},
        "analytics":{"enabled":false},
        "otel":{"exporter":"none","trace_exporter":"none","metrics_exporter":"none"},
        "features":{"hooks":false,"plugins":false,"apps":false,"shell_snapshot":false,"memories":false}
    })
}

fn expected_system_config_path() -> Result<PathBuf> {
    #[cfg(windows)]
    return Ok(known_program_data()?
        .join("OpenAI")
        .join("Codex")
        .join("config.toml"));
    #[cfg(target_os = "linux")]
    return Ok(PathBuf::from("/etc/codex/config.toml"));
    #[cfg(not(any(windows, target_os = "linux")))]
    bail!("system config path unsupported");
}

fn validate_config_requirements(frame: &Value) -> Result<()> {
    let result = frame
        .get("result")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("config requirements preflight failed"))?;
    if result.len() != 1 || result.get("requirements") != Some(&Value::Null) {
        bail!("config requirements preflight failed");
    }
    Ok(())
}

fn quota_exhausted(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(key, value)| {
            (key == "rateLimitReachedType" && !value.is_null())
                || (key == "usedPercent" && value.as_f64().is_some_and(|used| used >= 100.0))
                || quota_exhausted(value)
        }),
        Value::Array(values) => values.iter().any(quota_exhausted),
        _ => false,
    }
}

const AGENT_OUTPUT_SEED: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZABCDEFGHIJKLMNOPQRSTUVWXYZABCDEFGHIJKL";
const A_OUTPUT_LINES: usize = 900;
const B_OUTPUT_LINES: usize = 800;
const D_OUTPUT_LINES: usize = 800;

fn create_scenario_workspace(path: &Path) -> Result<()> {
    std::fs::create_dir(path).map_err(|_| anyhow!("scenario workspace creation failed"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| anyhow!("scenario workspace creation failed"))?;
    }
    Ok(())
}

fn agent_output_lines(name: &str) -> Result<usize> {
    Ok(match name {
        "A" => A_OUTPUT_LINES,
        "B" => B_OUTPUT_LINES,
        "D" => D_OUTPUT_LINES,
        _ => bail!("unknown agent-output scenario {name}"),
    })
}

#[cfg(test)]
fn agent_prompt_target_bytes(name: &str) -> Result<usize> {
    Ok(agent_output_lines(name)? * (6 + 1 + AGENT_OUTPUT_SEED.len() + 1))
}

fn agent_scenario_prompt(name: &str) -> Result<String> {
    let lines = agent_output_lines(name)?;
    let pacing = if name == "A" {
        " Emit steadily for at least 40 seconds; do not finish earlier."
    } else {
        ""
    };
    Ok(format!(
        "S2-{name}: Output exactly {lines} lines in the final response body only. Each line is a six-digit increasing decimal index from 000001 through {lines:06}, one ASCII space, then this fixed 64-character ASCII seed: {AGENT_OUTPUT_SEED}. Use no tools, no commands, no files, no network, no MCP, no web, no search, no code fence, and no commentary. Do not summarize or omit lines.{pacing}"
    ))
}

fn scenario_prompt(name: &str, approval_command: &str) -> Result<String> {
    match name {
        "A" | "B" | "D" => agent_scenario_prompt(name),
        "C" => Ok(format!(
            "S2-C: Execute exactly this command once now. Emit no prose. Use no other command or tool. APPROVAL_COMMAND_JSON:{}",
            serde_json::to_string(approval_command).unwrap()
        )),
        _ => unreachable!(),
    }
}

fn approval_command() -> Result<String> {
    #[cfg(windows)]
    {
        let path = trusted_system_cmd()?;
        let path = path
            .to_str()
            .context("system cmd.exe path was not valid Unicode")?;
        if path.contains('"') {
            bail!("system cmd.exe path contained an unsafe quote");
        }
        return Ok(format!(
            "& \"{path}\" /d /s /c \"<nul set /p ={APPROVAL_MARKER_CONTENT}>{APPROVAL_MARKER_NAME}&exit /b 0\""
        ));
    }
    #[cfg(not(windows))]
    Ok(format!(
        "/bin/sh -c 'printf {APPROVAL_MARKER_CONTENT} > {APPROVAL_MARKER_NAME}'"
    ))
}

fn approval_marker_path(workspace: &Path) -> PathBuf {
    workspace.join(APPROVAL_MARKER_NAME)
}

#[derive(Clone, Copy, Debug)]
enum MarkerHelperOperation {
    Absent,
    Verify,
}

impl MarkerHelperOperation {
    fn stdin_bytes(self) -> &'static [u8] {
        match self {
            Self::Absent => b"absent\n",
            Self::Verify => b"verify\n",
        }
    }

    fn success_bytes(self) -> &'static [u8] {
        match self {
            Self::Absent => b"ABSENT\n",
            Self::Verify => b"VERIFIED\n",
        }
    }
}

pub fn run_marker_helper() -> Result<()> {
    const MAX_OPERATION_BYTES: u64 = 16;
    let mut input = Vec::new();
    std::io::stdin()
        .take(MAX_OPERATION_BYTES + 1)
        .read_to_end(&mut input)
        .map_err(|_| anyhow!("marker helper failed"))?;
    let operation = match input.as_slice() {
        b"absent\n" => MarkerHelperOperation::Absent,
        b"verify\n" => MarkerHelperOperation::Verify,
        _ => bail!("marker helper failed"),
    };
    let workspace = std::env::current_dir().map_err(|_| anyhow!("marker helper failed"))?;
    let marker = approval_marker_path(&workspace);
    match operation {
        MarkerHelperOperation::Absent => ensure_approval_marker_absent_atomically(&marker)?,
        MarkerHelperOperation::Verify => {
            if read_approval_marker_atomically(&marker)? != APPROVAL_MARKER_BYTES {
                bail!("marker helper failed");
            }
        }
    }
    std::io::stdout()
        .write_all(operation.success_bytes())
        .and_then(|_| std::io::stdout().flush())
        .map_err(|_| anyhow!("marker helper failed"))
}

fn read_helper_output_bounded(mut stream: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    stream
        .by_ref()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow!("marker helper output failed"))?;
    if bytes.len() as u64 > limit {
        bail!("marker helper output failed");
    }
    Ok(bytes)
}

fn run_marker_helper_process(
    invocation: &MarkerHelperInvocation,
    workspace: &Path,
    operation: MarkerHelperOperation,
    deadline: Instant,
) -> Result<()> {
    const MAX_HELPER_OUTPUT_BYTES: u64 = 64;
    remaining_until(deadline).map_err(|_| anyhow!("marker helper timeout"))?;
    let mut process = invocation.command()?;
    process
        .current_dir(workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut contained =
        spawn_contained(process).map_err(|_| anyhow!("marker helper launch failed"))?;
    let mut stdin = contained
        .child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("marker helper launch failed"))?;
    let stdout = contained
        .child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("marker helper launch failed"))?;
    let stderr = contained
        .child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("marker helper launch failed"))?;
    stdin
        .write_all(operation.stdin_bytes())
        .and_then(|_| stdin.flush())
        .map_err(|_| anyhow!("marker helper protocol failed"))?;
    drop(stdin);

    let status_result = loop {
        match contained.child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            Ok(None) => break Err(anyhow!("marker helper timeout")),
            Err(_) => break Err(anyhow!("marker helper process failed")),
        }
    };
    // Always close the whole contained tree before consuming output. This also
    // closes inherited pipe handles, so bounded synchronous reads cannot leave
    // detached reader threads behind.
    contained
        .terminate_and_wait_bounded()
        .map_err(|_| anyhow!("marker helper cleanup failed"))?;
    let stdout = read_helper_output_bounded(stdout, MAX_HELPER_OUTPUT_BYTES)?;
    let stderr = read_helper_output_bounded(stderr, MAX_HELPER_OUTPUT_BYTES)?;
    let status = status_result?;
    if !status.success() || !stderr.is_empty() || stdout != operation.success_bytes() {
        bail!("marker helper protocol failed");
    }
    Ok(())
}

fn ensure_approval_marker_absent_bounded(
    invocation: &MarkerHelperInvocation,
    workspace: &Path,
    scenario_deadline: Instant,
    global_deadline: Instant,
) -> Result<()> {
    run_marker_helper_process(
        invocation,
        workspace,
        MarkerHelperOperation::Absent,
        scenario_deadline.min(global_deadline),
    )
}

fn verify_approval_marker_bounded(
    invocation: &MarkerHelperInvocation,
    workspace: &Path,
    scenario_deadline: Instant,
    global_deadline: Instant,
) -> Result<()> {
    run_marker_helper_process(
        invocation,
        workspace,
        MarkerHelperOperation::Verify,
        scenario_deadline.min(global_deadline),
    )
}

#[cfg(debug_assertions)]
pub fn run_marker_helper_process_for_test(
    workspace: &Path,
    program: OsString,
    args: Vec<OsString>,
    operation: &str,
    scenario_timeout: Duration,
    global_timeout: Duration,
) -> Result<()> {
    let operation = match operation {
        "absent" => MarkerHelperOperation::Absent,
        "verify" => MarkerHelperOperation::Verify,
        _ => bail!("marker helper test operation failed"),
    };
    let started = Instant::now();
    run_marker_helper_process(
        &MarkerHelperInvocation {
            program,
            args,
            #[cfg(windows)]
            path: None,
            #[cfg(windows)]
            handle: None,
            #[cfg(windows)]
            identity: None,
        },
        workspace,
        operation,
        (started + scenario_timeout).min(started + global_timeout),
    )
}

#[cfg(windows)]
struct OwnedMarkerHandle(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for OwnedMarkerHandle {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

#[cfg(windows)]
fn open_approval_marker_nofollow(path: &Path) -> std::io::Result<OwnedMarkerHandle> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{GENERIC_READ, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, OPEN_EXISTING,
    };

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    Ok(OwnedMarkerHandle(handle))
}

#[cfg(windows)]
fn ensure_approval_marker_absent_atomically(path: &Path) -> Result<()> {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND};

    match open_approval_marker_nofollow(path) {
        Ok(_) => bail!("scenario C approval marker precondition failed"),
        Err(error)
            if matches!(
                error.raw_os_error(),
                Some(code) if code == ERROR_FILE_NOT_FOUND as i32 || code == ERROR_PATH_NOT_FOUND as i32
            ) =>
        {
            Ok(())
        }
        Err(_) => bail!("scenario C approval marker precondition failed"),
    }
}

#[cfg(unix)]
fn open_approval_marker_nofollow(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
}

#[cfg(unix)]
fn ensure_approval_marker_absent_atomically(path: &Path) -> Result<()> {
    match open_approval_marker_nofollow(path) {
        Ok(_) => bail!("scenario C approval marker precondition failed"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => bail!("scenario C approval marker precondition failed"),
    }
}

#[cfg(windows)]
fn read_approval_marker_atomically(path: &Path) -> Result<Vec<u8>> {
    read_approval_marker_with_post_open(path, || {})
}

#[cfg(windows)]
fn read_approval_marker_with_post_open(path: &Path, post_open: impl FnOnce()) -> Result<Vec<u8>> {
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_TYPE_DISK, GetFileInformationByHandle, GetFileType, ReadFile,
    };

    let handle = open_approval_marker_nofollow(path)
        .map_err(|_| anyhow!("scenario C approval marker verification failed"))?;
    post_open();
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(handle.0, &mut info) } == 0
        || unsafe { GetFileType(handle.0) } != FILE_TYPE_DISK
        || info.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0
    {
        bail!("scenario C approval marker verification failed");
    }
    let size = ((info.nFileSizeHigh as u64) << 32) | info.nFileSizeLow as u64;
    if size > APPROVAL_MARKER_MAX_BYTES || size != APPROVAL_MARKER_BYTES.len() as u64 {
        bail!("scenario C approval marker verification failed");
    }
    let mut bytes = vec![0_u8; (APPROVAL_MARKER_MAX_BYTES + 1) as usize];
    let mut total = 0_usize;
    while total < bytes.len() {
        let mut read = 0_u32;
        if unsafe {
            ReadFile(
                handle.0,
                bytes[total..].as_mut_ptr(),
                (bytes.len() - total) as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        } == 0
        {
            bail!("scenario C approval marker verification failed");
        }
        if read == 0 {
            break;
        }
        total += read as usize;
    }
    bytes.truncate(total);
    Ok(bytes)
}

#[cfg(unix)]
fn read_approval_marker_atomically(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;

    let file = open_approval_marker_nofollow(path)
        .map_err(|_| anyhow!("scenario C approval marker verification failed"))?;
    let metadata = file
        .metadata()
        .map_err(|_| anyhow!("scenario C approval marker verification failed"))?;
    if !metadata.file_type().is_file()
        || metadata.len() > APPROVAL_MARKER_MAX_BYTES
        || metadata.len() != APPROVAL_MARKER_BYTES.len() as u64
    {
        bail!("scenario C approval marker verification failed");
    }
    let mut bytes = Vec::with_capacity(APPROVAL_MARKER_BYTES.len());
    file.take(APPROVAL_MARKER_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow!("scenario C approval marker verification failed"))?;
    Ok(bytes)
}

#[cfg(all(test, windows))]
fn trusted_scenario_shell() -> Result<PathBuf> {
    let system_root = canonical_safe_windows_root(system_directory()?, "system directory")?;
    canonical_safe_windows_executable(
        &system_root.join(r"WindowsPowerShell\v1.0\powershell.exe"),
        "powershell.exe",
        "system Windows PowerShell",
    )
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct WindowsFileIdentity {
    volume_serial: u32,
    file_index: u64,
}

#[cfg(windows)]
struct TrustedApprovalWrapper {
    path: PathBuf,
    handle: windows_sys::Win32::Foundation::HANDLE,
    identity: WindowsFileIdentity,
}

#[cfg(not(windows))]
struct TrustedApprovalWrapper;

#[cfg(windows)]
struct OwnedWrapperHandle(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for OwnedWrapperHandle {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        unsafe { CloseHandle(self.0) };
    }
}

#[cfg(windows)]
impl OwnedWrapperHandle {
    fn into_raw(self) -> windows_sys::Win32::Foundation::HANDLE {
        let handle = self.0;
        std::mem::forget(self);
        handle
    }
}

#[cfg(windows)]
impl Drop for TrustedApprovalWrapper {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        unsafe { CloseHandle(self.handle) };
    }
}

impl TrustedApprovalWrapper {
    #[cfg(windows)]
    fn path_identity_matches(&self) -> bool {
        let Ok(handle) = open_locked_wrapper_handle(&self.path) else {
            return false;
        };
        let Ok(final_path) = final_path_from_wrapper_handle(handle.0) else {
            return false;
        };
        if final_path != self.path {
            return false;
        }
        wrapper_file_identity(handle.0).is_ok_and(|identity| identity == self.identity)
    }

    #[cfg(not(windows))]
    fn path_identity_matches(&self) -> bool {
        false
    }
}

fn validate_explicit_approval_wrapper(
    path: Option<&Path>,
) -> Result<Option<TrustedApprovalWrapper>> {
    let Some(path) = path else {
        return Ok(None);
    };
    #[cfg(windows)]
    {
        if !path.is_absolute() {
            bail!("trusted approval wrapper validation failed");
        }
        return acquire_explicit_approval_wrapper_with_post_open(path, || {})
            .map(Some)
            .map_err(|_| anyhow!("trusted approval wrapper validation failed"));
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        bail!("trusted approval wrapper validation failed");
    }
}

#[cfg(windows)]
fn acquire_explicit_approval_wrapper_with_post_open(
    path: &Path,
    post_open: impl FnOnce(),
) -> Result<TrustedApprovalWrapper> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_TYPE_DISK, GetFileType,
    };

    if !path.is_absolute() {
        bail!("trusted approval wrapper validation failed");
    }
    let handle = open_locked_wrapper_handle(path)?;
    post_open();
    let final_path = final_path_from_wrapper_handle(handle.0)?;
    let info = wrapper_file_information(handle.0)?;
    let safe = final_path.is_absolute()
        && unsafe { GetFileType(handle.0) } == FILE_TYPE_DISK
        && info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
        && final_path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("pwsh.exe"))
        && final_path.to_str().is_some_and(|value| {
            !value.chars().any(|character| {
                matches!(
                    character,
                    '\r' | '\n' | '"' | '&' | '|' | '<' | '>' | '^' | '%' | '!' | '(' | ')' | ';'
                )
            })
        });
    if !safe {
        bail!("trusted approval wrapper validation failed");
    }
    let identity = WindowsFileIdentity {
        volume_serial: info.dwVolumeSerialNumber,
        file_index: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
    };
    Ok(TrustedApprovalWrapper {
        path: final_path,
        handle: handle.into_raw(),
        identity,
    })
}

#[cfg(windows)]
fn open_locked_wrapper_handle(path: &Path) -> Result<OwnedWrapperHandle> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{GENERIC_READ, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, OPEN_EXISTING,
    };

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        bail!("trusted approval wrapper validation failed");
    }
    Ok(OwnedWrapperHandle(handle))
}

#[cfg(windows)]
fn final_path_from_wrapper_handle(
    handle: windows_sys::Win32::Foundation::HANDLE,
) -> Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, VOLUME_NAME_DOS};

    let mut buffer = vec![0_u16; 32_768];
    let length = unsafe {
        GetFinalPathNameByHandleW(
            handle,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            VOLUME_NAME_DOS,
        )
    };
    if length == 0 || length as usize >= buffer.len() {
        bail!("trusted approval wrapper validation failed");
    }
    normalize_windows_command_path(PathBuf::from(std::ffi::OsString::from_wide(
        &buffer[..length as usize],
    )))
}

#[cfg(windows)]
fn wrapper_file_information(
    handle: windows_sys::Win32::Foundation::HANDLE,
) -> Result<windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION> {
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
        bail!("trusted approval wrapper validation failed");
    }
    Ok(info)
}

#[cfg(windows)]
fn wrapper_file_identity(
    handle: windows_sys::Win32::Foundation::HANDLE,
) -> Result<WindowsFileIdentity> {
    let info = wrapper_file_information(handle)?;
    Ok(WindowsFileIdentity {
        volume_serial: info.dwVolumeSerialNumber,
        file_index: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
    })
}

#[cfg(windows)]
fn auto_trusted_approval_wrapper_shell() -> Result<Option<PathBuf>> {
    let candidate = match find_windows_path_command("pwsh.exe") {
        Ok(candidate) => candidate,
        Err(_) => return Ok(None),
    };
    let canonical =
        canonical_safe_windows_executable(&candidate, "pwsh.exe", "app-server PowerShell")?;
    let program_files = canonical_safe_windows_root(known_program_files()?, "Program Files")?;
    let powershell_root = match program_files.join("PowerShell").canonicalize() {
        Ok(root) => normalize_windows_command_path(root)?,
        Err(_) => return Ok(None),
    };
    if !powershell_root.starts_with(&program_files) || !canonical.starts_with(&powershell_root) {
        return Ok(None);
    }
    Ok(Some(canonical))
}

#[cfg(windows)]
fn canonical_safe_windows_root(path: PathBuf, label: &str) -> Result<PathBuf> {
    let canonical = normalize_windows_command_path(
        path.canonicalize()
            .with_context(|| format!("failed to canonicalize {label} {}", path.display()))?,
    )?;
    if !canonical.is_absolute() || !canonical.is_dir() {
        bail!("{label} was not a canonical absolute directory");
    }
    validate_safe_windows_path(&canonical, label)?;
    Ok(canonical)
}

#[cfg(windows)]
fn canonical_safe_windows_executable(
    path: &Path,
    expected_name: &str,
    label: &str,
) -> Result<PathBuf> {
    let canonical = normalize_windows_command_path(
        path.canonicalize()
            .with_context(|| format!("failed to canonicalize {label} {}", path.display()))?,
    )?;
    if !canonical.is_absolute()
        || !canonical.is_file()
        || !canonical
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case(expected_name))
    {
        bail!("{label} was not the expected canonical absolute regular file");
    }
    validate_safe_windows_path(&canonical, label)?;
    Ok(canonical)
}

#[cfg(windows)]
fn validate_safe_windows_path(path: &Path, label: &str) -> Result<()> {
    let rendered = path
        .to_str()
        .with_context(|| format!("{label} path was not valid Unicode"))?;
    if rendered
        .chars()
        .any(|character| matches!(character, '\r' | '\n' | '"'))
    {
        bail!("{label} path contained unsafe characters");
    }
    Ok(())
}

#[cfg(windows)]
fn known_program_files() -> Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_ProgramFiles, SHGetKnownFolderPath};

    let mut path = std::ptr::null_mut();
    let result =
        unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramFiles, 0, std::ptr::null_mut(), &mut path) };
    if result < 0 || path.is_null() {
        unsafe { CoTaskMemFree(path.cast()) };
        bail!("SHGetKnownFolderPath(FOLDERID_ProgramFiles) failed: HRESULT {result:#x}");
    }
    let length = unsafe {
        let mut length = 0;
        while *path.add(length) != 0 {
            length += 1;
        }
        length
    };
    let value = PathBuf::from(OsString::from_wide(unsafe {
        std::slice::from_raw_parts(path, length)
    }));
    unsafe { CoTaskMemFree(path.cast()) };
    Ok(value)
}

#[cfg(windows)]
fn known_program_data() -> Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_ProgramData, SHGetKnownFolderPath};

    let mut path = std::ptr::null_mut();
    let result =
        unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramData, 0, std::ptr::null_mut(), &mut path) };
    if result < 0 || path.is_null() {
        unsafe { CoTaskMemFree(path.cast()) };
        bail!("known folder lookup failed");
    }
    let length = unsafe {
        let mut length = 0;
        while *path.add(length) != 0 {
            length += 1;
        }
        length
    };
    let value = PathBuf::from(OsString::from_wide(unsafe {
        std::slice::from_raw_parts(path, length)
    }));
    unsafe { CoTaskMemFree(path.cast()) };
    Ok(value)
}

#[cfg(windows)]
fn system_directory() -> Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

    let mut buffer = vec![0_u16; 32_768];
    let length = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
    if length == 0 || length as usize >= buffer.len() {
        bail!("GetSystemDirectoryW failed or returned an oversized path");
    }
    Ok(PathBuf::from(OsString::from_wide(
        &buffer[..length as usize],
    )))
}

fn approval_wrapper_candidate(pwsh: &Path, expected: &str) -> Result<String> {
    let pwsh = pwsh
        .to_str()
        .context("trusted pwsh.exe path was not valid Unicode")?;
    if pwsh
        .chars()
        .any(|character| matches!(character, '\r' | '\n' | '"'))
    {
        bail!("trusted pwsh.exe path contained unsafe characters");
    }
    // app-server 0.139 renders the outer executable path with each path separator
    // doubled, while the four wrapper-level quote characters remain literal quotes.
    // The inner command uses its independently observed backslash-then-quote escaping.
    let escaped_pwsh = pwsh.replace('\\', r"\\");
    let escaped = expected.replace('\\', r"\\").replace('"', r#"\""#);
    Ok(format!(r#""{escaped_pwsh}" -Command "{escaped}""#))
}

#[cfg(windows)]
fn trusted_system_cmd() -> Result<PathBuf> {
    let system_root = canonical_safe_windows_root(system_directory()?, "system directory")?;
    let path = canonical_safe_windows_executable(
        &system_root.join("cmd.exe"),
        "cmd.exe",
        "system cmd.exe",
    )?;
    if !path.starts_with(&system_root) {
        bail!("system cmd.exe escaped the protected system directory");
    }
    Ok(path)
}

fn performance(events: &[ContentEvent]) -> Result<PerformanceEvidence> {
    if events.is_empty() {
        bail!("no real content events were observed");
    }
    let mut sizes = events.iter().map(|event| event.bytes).collect::<Vec<_>>();
    sizes.sort_unstable();
    let percentile = |numerator: usize| sizes[((sizes.len() - 1) * numerator) / 100];
    let mut ordered = events.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|event| event.timestamp_ns);
    let mut left = 0;
    let mut window_bytes = 0_u64;
    let mut peak_events = 0_u64;
    let mut peak_bytes = 0_u64;
    for (right, event) in ordered.iter().enumerate() {
        window_bytes += event.bytes;
        while event
            .timestamp_ns
            .saturating_sub(ordered[left].timestamp_ns)
            >= 1_000_000_000
        {
            window_bytes -= ordered[left].bytes;
            left += 1;
        }
        peak_events = peak_events.max((right - left + 1) as u64);
        peak_bytes = peak_bytes.max(window_bytes);
    }
    let mut merge_windows = std::collections::BTreeSet::new();
    let first_timestamp = ordered[0].timestamp_ns;
    for event in ordered {
        merge_windows.insert(event.timestamp_ns.saturating_sub(first_timestamp) / 50_000_000);
    }
    Ok(PerformanceEvidence {
        real_content: true,
        peak_events_per_second: peak_events as f64,
        peak_megabytes_per_second: peak_bytes as f64 / 1_000_000.0,
        event_sizes: EventSizeDistribution {
            samples: sizes.len() as u64,
            min_bytes: sizes[0],
            p50_bytes: percentile(50),
            p95_bytes: percentile(95),
            max_bytes: *sizes.last().unwrap(),
        },
        merge_window_ms: 50,
        merge_input_events: events.len() as u64,
        merge_output_events: merge_windows.len() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::{ContentEvent, performance, read_locked_auth, read_locked_auth_with_post_open};
    use std::io::{self, BufReader, Read};
    use std::path::Path;

    #[test]
    fn marker_helper_invocation_is_not_directly_cloneable() {
        trait AmbiguousIfClone<A> {
            fn assert_not_clone() {}
        }

        impl<T: ?Sized> AmbiguousIfClone<()> for T {}
        impl<T: ?Sized + Clone> AmbiguousIfClone<u8> for T {}

        <super::MarkerHelperInvocation as AmbiguousIfClone<_>>::assert_not_clone();
    }

    #[test]
    fn auth_copy_reads_only_the_original_bounded_nofollow_handle() {
        let root = std::env::temp_dir().join(format!(
            "codex-s2-auth-handle-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let auth = root.join("auth.json");
        let original = br#"{"auth_mode":"chatgpt","tokens":{"access_token":"original"}}"#;
        std::fs::write(&auth, original).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&auth, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let mut replaced = false;
        let observed = read_locked_auth_with_post_open(&auth, || {
            if std::fs::remove_file(&auth).is_ok() {
                replaced = true;
                std::fs::write(&auth, br#"{"auth_mode":"chatgpt","tokens":{}}"#).unwrap();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&auth, std::fs::Permissions::from_mode(0o600))
                        .unwrap();
                }
            }
        })
        .unwrap();
        assert_eq!(observed, original);
        #[cfg(windows)]
        assert!(!replaced, "the open source handle must block replacement");
        #[cfg(unix)]
        assert!(replaced, "the test must exercise same-handle Unix reading");

        let target = root.join("target.json");
        std::fs::write(&target, original).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::{PermissionsExt, symlink};
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
            let link = root.join("link.json");
            symlink(&target, &link).unwrap();
            assert!(read_locked_auth(&link).is_err());
        }
        #[cfg(windows)]
        {
            let link = root.join("link.json");
            if std::os::windows::fs::symlink_file(&target, &link).is_ok() {
                assert!(read_locked_auth(&link).is_err());
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    use serde_json::json;

    #[test]
    fn performance_uses_sliding_one_second_peak() {
        let metrics = performance(&[
            ContentEvent {
                timestamp_ns: 990_000_000,
                bytes: 7,
            },
            ContentEvent {
                timestamp_ns: 1_010_000_000,
                bytes: 11,
            },
        ])
        .unwrap();

        assert_eq!(metrics.peak_events_per_second, 2.0);
        assert_eq!(metrics.peak_megabytes_per_second, 18.0 / 1_000_000.0);
    }

    #[test]
    fn merge_windows_are_relative_to_first_event() {
        let metrics = performance(&[
            ContentEvent {
                timestamp_ns: 49_000_000,
                bytes: 7,
            },
            ContentEvent {
                timestamp_ns: 51_000_000,
                bytes: 11,
            },
        ])
        .unwrap();

        assert_eq!(metrics.merge_input_events, 2);
        assert_eq!(metrics.merge_output_events, 1);
    }

    #[test]
    fn stderr_line_reader_propagates_io_errors() {
        struct BrokenReader;
        impl Read for BrokenReader {
            fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::Other, "injected failure"))
            }
        }

        let error = super::read_line_checked(&mut BufReader::new(BrokenReader)).unwrap_err();
        assert_eq!(error.to_string(), "injected failure");
    }

    #[test]
    fn agent_prompts_are_short_tool_free_and_have_output_headroom() {
        for (name, minimum_bytes) in [("A", 60 * 1024), ("B", 50 * 1024), ("D", 50 * 1024)] {
            let prompt = super::agent_scenario_prompt(name).unwrap();
            assert!(prompt.len() < 1_024);
            assert!(prompt.contains("64-character ASCII seed"));
            assert!(prompt.contains("six-digit"));
            assert!(prompt.contains("final response body"));
            for forbidden in [
                "command",
                "file",
                "network",
                "MCP",
                "web",
                "search",
                "code fence",
                "commentary",
            ] {
                assert!(prompt.contains(&format!("no {forbidden}")));
            }
            assert!(super::agent_prompt_target_bytes(name).unwrap() >= minimum_bytes);
        }
        assert!(
            super::agent_scenario_prompt("A")
                .unwrap()
                .contains("at least 40 seconds")
        );
    }

    #[test]
    fn command_output_delta_is_r1_only_for_the_exact_thread_and_turn() {
        let command_output = json!({
            "method":"item/commandExecution/outputDelta",
            "params":{"threadId":"thread-a","turnId":"turn-a","delta":"real-output"}
        });
        assert_eq!(
            super::delta_bytes(&command_output, "thread-a", "turn-a"),
            Some(11)
        );
        assert_eq!(
            super::delta_bytes(&command_output, "other-thread", "turn-a"),
            None
        );
        assert_eq!(
            super::delta_bytes(&command_output, "thread-a", "other-turn"),
            None
        );
        let unrelated = json!({
            "method":"item/commandExecution/outputDelta",
            "params":{"threadId":"thread-a","turnId":"turn-a","output":"not-a-delta"}
        });
        assert_eq!(super::delta_bytes(&unrelated, "thread-a", "turn-a"), None);
        assert_eq!(
            super::agent_delta_bytes(&command_output, "thread-a", "turn-a"),
            None
        );
    }

    fn pinned_thread_frame(workspace: &Path) -> serde_json::Value {
        json!({"method":"thread/started","params":{"thread":{
            "id":"t","sessionId":"t","forkedFromId":null,"parentThreadId":null,
            "preview":"","ephemeral":true,"modelProvider":"openai","createdAt":1,"updatedAt":1,
            "status":{"type":"idle"},"path":null,"cwd":workspace,"cliVersion":"0.139.0",
            "source":"vscode","threadSource":null,"agentNickname":null,"agentRole":null,
            "gitInfo":null,"name":null,"turns":[]
        }}})
    }

    fn state_after_turn<'a>(prompt: &'a str, workspace: &'a Path) -> super::AgentOnlyState<'a> {
        let mut state = super::AgentOnlyState::new(prompt, workspace);
        state
            .validate(&pinned_thread_frame(workspace), "t", "u")
            .unwrap();
        state
            .validate(
                &json!({"method":"turn/started","params":{"threadId":"t","turn":{"id":"u","status":"inProgress"}}}),
                "t",
                "u",
            )
            .unwrap();
        state
    }

    fn state_after_user<'a>(prompt: &'a str, workspace: &'a Path) -> super::AgentOnlyState<'a> {
        let mut state = state_after_turn(prompt, workspace);
        let user = json!({"type":"userMessage","id":"user","clientId":null,"content":[{"type":"text","text":prompt,"text_elements":[]}]});
        state.validate(&json!({"method":"item/started","params":{"threadId":"t","turnId":"u","startedAtMs":1,"item":user}}), "t", "u").unwrap();
        state.validate(&json!({"method":"item/completed","params":{"threadId":"t","turnId":"u","completedAtMs":2,"item":user}}), "t", "u").unwrap();
        state
    }

    #[test]
    fn pinned_thread_started_requires_every_0139_field_and_scenario_binding() {
        let workspace = std::env::temp_dir().canonicalize().unwrap();
        let frame = pinned_thread_frame(&workspace);
        assert!(super::valid_thread_started(&frame, "t", &workspace));
        let fields = frame["params"]["thread"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for field in fields {
            let mut missing = frame.clone();
            missing["params"]["thread"]
                .as_object_mut()
                .unwrap()
                .remove(&field);
            assert!(
                !super::valid_thread_started(&missing, "t", &workspace),
                "missing {field} was accepted"
            );
        }
        for (field, bad) in [
            ("id", json!("other")),
            ("sessionId", json!("other")),
            ("forkedFromId", json!("fork")),
            ("parentThreadId", json!("parent")),
            ("preview", json!("unexpected")),
            ("ephemeral", json!(false)),
            ("modelProvider", json!("")),
            ("createdAt", json!(-1)),
            ("updatedAt", json!(0)),
            ("status", json!({"type":"active","activeFlags":[]})),
            ("path", json!("somewhere")),
            ("cwd", json!(workspace.join("child"))),
            ("cliVersion", json!("0.140.0")),
            ("source", json!("appServer")),
            ("threadSource", json!("user")),
            ("agentNickname", json!("agent")),
            ("agentRole", json!("worker")),
            (
                "gitInfo",
                json!({"sha":null,"branch":null,"originUrl":null}),
            ),
            ("name", json!("name")),
            ("turns", json!([{}])),
        ] {
            let mut changed = frame.clone();
            changed["params"]["thread"][field] = bad;
            assert!(
                !super::valid_thread_started(&changed, "t", &workspace),
                "wrong {field} was accepted"
            );
        }
        for bad_source in [
            json!("appServer"),
            json!("cli"),
            json!("unknown"),
            json!({"custom":"caller-defined"}),
        ] {
            let mut changed = frame.clone();
            changed["params"]["thread"]["source"] = bad_source.clone();
            assert!(
                !super::valid_thread_started(&changed, "t", &workspace),
                "wrong source {bad_source} was accepted"
            );
        }
        let mut unknown = frame.clone();
        unknown["params"]["thread"]["futureField"] = json!(true);
        assert!(!super::valid_thread_started(&unknown, "t", &workspace));
    }

    #[test]
    #[cfg(windows)]
    fn pinned_thread_cwd_accepts_only_exact_windows_verbatim_equivalence() {
        let expected = std::path::PathBuf::from(r"\\?\C:\codex-s2\workspace");
        let observed = std::path::PathBuf::from(r"C:\codex-s2\workspace");
        let frame = pinned_thread_frame(&observed);
        assert!(super::valid_thread_started(&frame, "t", &expected));

        let unc_expected = std::path::PathBuf::from(r"\\?\UNC\server\share\workspace");
        let unc_observed = std::path::PathBuf::from(r"\\server\share\workspace");
        let frame = pinned_thread_frame(&unc_observed);
        assert!(super::valid_thread_started(&frame, "t", &unc_expected));

        for rejected in [
            r"C:\codex-s2\different",
            r"C:\codex-s2\workspace\child",
            r"C:\codex-s2",
            r"C:\codex-s2\workspace\.",
            r"c:\codex-s2\workspace",
        ] {
            let frame = pinned_thread_frame(Path::new(rejected));
            assert!(
                !super::valid_thread_started(&frame, "t", &expected),
                "non-exact cwd {rejected} was accepted"
            );
        }
    }

    #[test]
    fn thread_lifecycle_rejection_categories_are_stable() {
        let workspace = std::env::temp_dir().canonicalize().unwrap();
        let mut malformed = pinned_thread_frame(&workspace);
        malformed["params"]["thread"]["id"] = json!("other");
        let mut state = super::AgentOnlyState::new("prompt", &workspace);
        let error = state.validate(&malformed, "t", "u").unwrap_err();
        assert_eq!(
            error.to_string(),
            "protocol error: malformed, duplicate, or out-of-order thread/started"
        );

        let mut missing = super::AgentOnlyState::new("prompt", &workspace);
        let error = missing
            .validate(
                &json!({"method":"turn/started","params":{"threadId":"t","turn":{"id":"u","status":"inProgress"}}}),
                "t",
                "u",
            )
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "protocol error: duplicate or out-of-order turn/started"
        );

        let mut duplicate = state_after_turn("prompt", &workspace);
        let error = duplicate
            .validate(&pinned_thread_frame(&workspace), "t", "u")
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "protocol error: malformed, duplicate, or out-of-order thread/started"
        );
    }

    #[test]
    fn mcp_startup_status_is_strictly_typed_and_stateful_for_the_thread() {
        let workspace = std::env::temp_dir().canonicalize().unwrap();
        let mut state = super::AgentOnlyState::new("prompt", &workspace);
        state
            .validate(&pinned_thread_frame(&workspace), "t", "u")
            .unwrap();
        for frame in [
            json!({"method":"mcpServer/startupStatus/updated","params":{"threadId":"t","name":"ready-server","status":"starting","error":null}}),
            json!({"method":"mcpServer/startupStatus/updated","params":{"threadId":"t","name":"ready-server","status":"ready","error":null}}),
            json!({"method":"mcpServer/startupStatus/updated","params":{"threadId":"t","name":"failed-server","status":"starting","error":null}}),
            json!({"method":"mcpServer/startupStatus/updated","params":{"threadId":"t","name":"failed-server","status":"failed","error":"fixture failure"}}),
            json!({"method":"mcpServer/startupStatus/updated","params":{"threadId":"t","name":"still-starting","status":"starting","error":null}}),
        ] {
            state.validate(&frame, "t", "u").unwrap();
        }
        assert!(!state.turn_started);
        state
            .validate(
                &json!({"method":"turn/started","params":{"threadId":"t","turn":{"id":"u","status":"inProgress"}}}),
                "t",
                "u",
            )
            .unwrap();
        let cross_turn_terminal = json!({"method":"mcpServer/startupStatus/updated","params":{"threadId":"t","name":"still-starting","status":"cancelled","error":null}});
        state.validate(&cross_turn_terminal, "t", "u").unwrap();
        let late = json!({"method":"mcpServer/startupStatus/updated","params":{"threadId":"t","name":"late","status":"starting","error":null}});
        state.validate(&late, "t", "u").unwrap();
        let late_terminal = json!({"method":"mcpServer/startupStatus/updated","params":{"threadId":"t","name":"late","status":"failed","error":"late failure"}});
        state.validate(&late_terminal, "t", "u").unwrap();
        assert!(state.turn_started);
        assert!(state.items.is_empty());
        assert!(!state.user_raw_seen);
        assert!(state.user_item_id.is_none());
        let mut before_thread = super::AgentOnlyState::new("prompt", &workspace);
        let error = before_thread.validate(&late, "t", "u").unwrap_err();
        assert_eq!(
            error.to_string(),
            "protocol error: MCP startup status had mismatched thread identity or order"
        );

        let malformed = [
            json!({"threadId":null,"name":"server","status":"starting","error":null}),
            json!({"threadId":"t","name":"","status":"starting","error":null}),
            json!({"threadId":"t","name":"server","status":"future","error":null}),
            json!({"threadId":"t","name":"server","status":"starting"}),
            json!({"threadId":"t","name":"server","status":"starting","error":null,"extra":true}),
            json!({"threadId":"t","name":"server","status":"starting","error":"unexpected"}),
            json!({"threadId":"t","name":"server","status":"ready","error":"unexpected"}),
            json!({"threadId":"t","name":"server","status":"cancelled","error":"unexpected"}),
            json!({"threadId":"t","name":"server","status":"failed","error":null}),
            json!({"threadId":"t","name":"server","status":"failed","error":""}),
        ];
        for params in malformed {
            let mut state = super::AgentOnlyState::new("prompt", &workspace);
            state
                .validate(&pinned_thread_frame(&workspace), "t", "u")
                .unwrap();
            let error = state
                .validate(
                    &json!({"method":"mcpServer/startupStatus/updated","params":params}),
                    "t",
                    "u",
                )
                .unwrap_err();
            assert_eq!(
                error.to_string(),
                "protocol error: malformed MCP startup status notification"
            );
        }

        for params in [
            json!({"threadId":"wrong","name":"server","status":"starting","error":null}),
            json!({"threadId":"t","name":"server","status":"ready","error":null}),
        ] {
            let mut state = super::AgentOnlyState::new("prompt", &workspace);
            state
                .validate(&pinned_thread_frame(&workspace), "t", "u")
                .unwrap();
            let error = state
                .validate(
                    &json!({"method":"mcpServer/startupStatus/updated","params":params}),
                    "t",
                    "u",
                )
                .unwrap_err();
            let expected = if params["threadId"] == "wrong" {
                "protocol error: MCP startup status had mismatched thread identity or order"
            } else {
                "protocol error: invalid MCP startup status transition"
            };
            assert_eq!(error.to_string(), expected);
        }

        for terminal in ["ready", "failed", "cancelled"] {
            let mut state = super::AgentOnlyState::new("prompt", &workspace);
            state
                .validate(&pinned_thread_frame(&workspace), "t", "u")
                .unwrap();
            let starting = json!({"method":"mcpServer/startupStatus/updated","params":{"threadId":"t","name":"server","status":"starting","error":null}});
            state.validate(&starting, "t", "u").unwrap();
            let error = state.validate(&starting, "t", "u").unwrap_err();
            assert_eq!(
                error.to_string(),
                "protocol error: invalid MCP startup status transition"
            );
            let error = if terminal == "failed" { "failure" } else { "" };
            let terminal = json!({"method":"mcpServer/startupStatus/updated","params":{"threadId":"t","name":"server","status":terminal,"error":if error.is_empty() { serde_json::Value::Null } else { json!(error) }}});
            state.validate(&terminal, "t", "u").unwrap();
            let duplicate = state.validate(&terminal, "t", "u").unwrap_err();
            assert_eq!(
                duplicate.to_string(),
                "protocol error: invalid MCP startup status transition"
            );
            let (conflicting_status, conflicting_error) =
                if terminal["params"]["status"] == "failed" {
                    ("ready", serde_json::Value::Null)
                } else {
                    ("failed", json!("conflicting failure"))
                };
            let conflicting = json!({"method":"mcpServer/startupStatus/updated","params":{"threadId":"t","name":"server","status":conflicting_status,"error":conflicting_error}});
            let error = state.validate(&conflicting, "t", "u").unwrap_err();
            assert_eq!(
                error.to_string(),
                "protocol error: invalid MCP startup status transition"
            );
        }
    }

    #[test]
    fn warning_is_strictly_typed_and_thread_scoped() {
        let workspace = std::env::temp_dir().canonicalize().unwrap();
        let warning = |thread_id: serde_json::Value, message: serde_json::Value| json!({"method":"warning","params":{"threadId":thread_id,"message":message}});
        let mut state = super::AgentOnlyState::new("prompt", &workspace);
        let before_thread = state
            .validate(&warning(json!("t"), json!("before")), "t", "u")
            .unwrap_err();
        assert_eq!(
            before_thread.to_string(),
            "protocol error: warning had mismatched thread identity or order"
        );
        state
            .validate(&pinned_thread_frame(&workspace), "t", "u")
            .unwrap();
        state
            .validate(&warning(json!("t"), json!("before turn")), "t", "u")
            .unwrap();
        state
            .validate(
                &json!({"method":"turn/started","params":{"threadId":"t","turn":{"id":"u","status":"inProgress"}}}),
                "t",
                "u",
            )
            .unwrap();
        state
            .validate(&warning(json!("t"), json!("after turn")), "t", "u")
            .unwrap();
        assert!(state.turn_started);
        assert!(state.items.is_empty());
        assert!(state.user_item_id.is_none());

        for frame in [
            warning(serde_json::Value::Null, json!("message")),
            warning(json!("t"), json!("")),
            warning(json!("t"), serde_json::Value::Null),
            json!({"method":"warning","params":{"threadId":"t","message":"message","extra":true}}),
            json!({"method":"warning","params":{"threadId":"t"}}),
        ] {
            let mut state = super::AgentOnlyState::new("prompt", &workspace);
            state
                .validate(&pinned_thread_frame(&workspace), "t", "u")
                .unwrap();
            let error = state.validate(&frame, "t", "u").unwrap_err();
            assert_eq!(
                error.to_string(),
                "protocol error: malformed warning notification"
            );
        }
        let mut state = super::AgentOnlyState::new("prompt", &workspace);
        state
            .validate(&pinned_thread_frame(&workspace), "t", "u")
            .unwrap();
        let wrong_thread = state
            .validate(&warning(json!("wrong"), json!("message")), "t", "u")
            .unwrap_err();
        assert_eq!(
            wrong_thread.to_string(),
            "protocol error: warning had mismatched thread identity or order"
        );
    }

    #[test]
    fn user_raw_is_optional_exact_unique_and_does_not_advance_canonical_lifecycle() {
        let workspace = std::env::temp_dir().canonicalize().unwrap();
        let prompt = "exact prompt";
        for item in [
            json!({"type":"message","role":"user","content":[{"type":"input_text","text":"wrong"}]}),
            json!({"type":"message","role":"user","content":[{"type":"input_text","text":prompt},{"type":"input_text","text":prompt}]}),
            json!({"type":"message","role":"user","content":[{"type":"input_text","text":prompt}],"extra":true}),
        ] {
            let mut state = state_after_turn(prompt, &workspace);
            let error = state.validate(&json!({"method":"rawResponseItem/completed","params":{"threadId":"t","turnId":"u","item":item}}), "t", "u").unwrap_err();
            assert_eq!(
                error.to_string(),
                "protocol error: malformed or mismatched user raw item"
            );
        }
        let mut state = state_after_turn(prompt, &workspace);
        let user_raw = json!({"method":"rawResponseItem/completed","params":{"threadId":"t","turnId":"u","item":{"type":"message","role":"user","content":[{"type":"input_text","text":prompt}]}}});
        state.validate(&user_raw, "t", "u").unwrap();
        let user = json!({"type":"userMessage","id":"user","clientId":null,"content":[{"type":"text","text":prompt,"text_elements":[]}]});
        state.validate(&json!({"method":"item/started","params":{"threadId":"t","turnId":"u","startedAtMs":1,"item":user}}), "t", "u").unwrap();
        state.validate(&json!({"method":"item/completed","params":{"threadId":"t","turnId":"u","completedAtMs":2,"item":user}}), "t", "u").unwrap();

        let mut state = state_after_user(prompt, &workspace);
        state.validate(&user_raw, "t", "u").unwrap();
        let error = state.validate(&user_raw, "t", "u").unwrap_err();
        assert_eq!(
            error.to_string(),
            "protocol error: duplicate or out-of-order user raw item"
        );
    }

    #[test]
    fn agent_item_lifecycle_rejects_wrong_ids_kinds_duplicates_and_raw_order() {
        let workspace = std::env::temp_dir().canonicalize().unwrap();
        let prompt = "exact prompt";
        let assistant = json!({"type":"agentMessage","id":"assistant","text":"","phase":null,"memoryCitation":null});
        let started = json!({"method":"item/started","params":{"threadId":"t","turnId":"u","startedAtMs":3,"item":assistant}});
        let delta = json!({"method":"item/agentMessage/delta","params":{"threadId":"t","turnId":"u","itemId":"assistant","delta":"x"}});
        let completed = json!({"method":"item/completed","params":{"threadId":"t","turnId":"u","completedAtMs":4,"item":{"type":"agentMessage","id":"assistant","text":"x","phase":null,"memoryCitation":null}}});
        let raw = json!({"method":"rawResponseItem/completed","params":{"threadId":"t","turnId":"u","item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"x"}]}}});

        let mut state = state_after_user(prompt, &workspace);
        assert_eq!(
            state.validate(&delta, "t", "u").unwrap_err().to_string(),
            "protocol error: delta arrived before its item lifecycle started"
        );
        assert_eq!(
            state
                .validate(&completed, "t", "u")
                .unwrap_err()
                .to_string(),
            "protocol error: agent item completion arrived before start"
        );
        assert_eq!(
            state.validate(&raw, "t", "u").unwrap_err().to_string(),
            "protocol error: raw response item was out of order or duplicate"
        );

        let mut state = state_after_user(prompt, &workspace);
        state.validate(&started, "t", "u").unwrap();
        assert_eq!(
            state.validate(&started, "t", "u").unwrap_err().to_string(),
            "protocol error: duplicate agent item start"
        );
        assert_eq!(state.validate(&json!({"method":"item/agentMessage/delta","params":{"threadId":"t","turnId":"u","itemId":"other","delta":"x"}}), "t", "u").unwrap_err().to_string(), "protocol error: delta arrived before its item lifecycle started");
        assert_eq!(state.validate(&json!({"method":"item/plan/delta","params":{"threadId":"t","turnId":"u","itemId":"assistant","delta":"x"}}), "t", "u").unwrap_err().to_string(), "protocol error: delta item kind or lifecycle state was invalid");
        state.validate(&delta, "t", "u").unwrap();
        state.validate(&completed, "t", "u").unwrap();
        assert_eq!(
            state.validate(&delta, "t", "u").unwrap_err().to_string(),
            "protocol error: delta item kind or lifecycle state was invalid"
        );
        state.validate(&raw, "t", "u").unwrap();
        assert_eq!(
            state.validate(&raw, "t", "u").unwrap_err().to_string(),
            "protocol error: duplicate raw response item"
        );
        assert_eq!(
            state
                .validate(&completed, "t", "u")
                .unwrap_err()
                .to_string(),
            "protocol error: mismatched or duplicate agent item completion"
        );
    }

    #[test]
    fn completed_agent_only_scenarios_require_canonical_lifecycle_but_not_raw_items() {
        let workspace = std::env::temp_dir().canonicalize().unwrap();
        let prompt = "exact prompt";

        let mut missing_second_agent_raw = state_after_user(prompt, &workspace);
        for (id, text, with_raw) in [("first", "one", true), ("second", "two", false)] {
            let item =
                json!({"type":"agentMessage","id":id,"text":"","phase":null,"memoryCitation":null});
            missing_second_agent_raw.validate(&json!({"method":"item/started","params":{"threadId":"t","turnId":"u","startedAtMs":3,"item":item}}), "t", "u").unwrap();
            let item = json!({"type":"agentMessage","id":id,"text":text,"phase":null,"memoryCitation":null});
            missing_second_agent_raw.validate(&json!({"method":"item/completed","params":{"threadId":"t","turnId":"u","completedAtMs":4,"item":item}}), "t", "u").unwrap();
            if with_raw {
                missing_second_agent_raw.validate(&json!({"method":"rawResponseItem/completed","params":{"threadId":"t","turnId":"u","item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}}}), "t", "u").unwrap();
            }
        }
        missing_second_agent_raw
            .finish("A", &super::TerminalState::Completed)
            .unwrap();

        let mut interrupted = state_after_user(prompt, &workspace);
        let active = json!({"type":"agentMessage","id":"active","text":"","phase":"final_answer","memoryCitation":null});
        interrupted.validate(&json!({"method":"item/started","params":{"threadId":"t","turnId":"u","startedAtMs":7,"item":active}}), "t", "u").unwrap();
        interrupted.validate(&json!({"method":"item/agentMessage/delta","params":{"threadId":"t","turnId":"u","itemId":"active","delta":"partial"}}), "t", "u").unwrap();
        interrupted
            .finish("D", &super::TerminalState::Interrupted)
            .unwrap();

        let mut missing_reasoning_raw = state_after_user(prompt, &workspace);
        let agent = json!({"type":"agentMessage","id":"agent","text":"","phase":null,"memoryCitation":null});
        missing_reasoning_raw.validate(&json!({"method":"item/started","params":{"threadId":"t","turnId":"u","startedAtMs":3,"item":agent}}), "t", "u").unwrap();
        let agent = json!({"type":"agentMessage","id":"agent","text":"answer","phase":null,"memoryCitation":null});
        missing_reasoning_raw.validate(&json!({"method":"item/completed","params":{"threadId":"t","turnId":"u","completedAtMs":4,"item":agent}}), "t", "u").unwrap();
        missing_reasoning_raw.validate(&json!({"method":"rawResponseItem/completed","params":{"threadId":"t","turnId":"u","item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"answer"}]}}}), "t", "u").unwrap();
        let reasoning = json!({"type":"reasoning","id":"reasoning","summary":[],"content":[]});
        missing_reasoning_raw.validate(&json!({"method":"item/started","params":{"threadId":"t","turnId":"u","startedAtMs":5,"item":reasoning}}), "t", "u").unwrap();
        missing_reasoning_raw.validate(&json!({"method":"item/completed","params":{"threadId":"t","turnId":"u","completedAtMs":6,"item":reasoning}}), "t", "u").unwrap();
        missing_reasoning_raw
            .finish("B", &super::TerminalState::Completed)
            .unwrap();
    }

    #[test]
    fn agent_message_phase_is_exact_stable_and_final_only() {
        let workspace = std::env::temp_dir().canonicalize().unwrap();
        let prompt = "exact prompt";
        for phase in [json!(null), json!("final_answer")] {
            let mut state = state_after_user(prompt, &workspace);
            let started = json!({"type":"agentMessage","id":"agent","text":"","phase":phase,"memoryCitation":null});
            state.validate(&json!({"method":"item/started","params":{"threadId":"t","turnId":"u","startedAtMs":3,"item":started}}), "t", "u").unwrap();
            state.validate(&json!({"method":"item/agentMessage/delta","params":{"threadId":"t","turnId":"u","itemId":"agent","delta":"x"}}), "t", "u").unwrap();
            let completed = json!({"type":"agentMessage","id":"agent","text":"x","phase":phase,"memoryCitation":null});
            state.validate(&json!({"method":"item/completed","params":{"threadId":"t","turnId":"u","completedAtMs":4,"item":completed}}), "t", "u").unwrap();
            let mut raw = json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"x"}]});
            if phase == json!("final_answer") {
                raw["phase"] = phase;
            }
            state.validate(&json!({"method":"rawResponseItem/completed","params":{"threadId":"t","turnId":"u","item":raw}}), "t", "u").unwrap();
            state.finish("A", &super::TerminalState::Completed).unwrap();
        }

        for phase in [json!("commentary"), json!("future_phase")] {
            let mut state = state_after_user(prompt, &workspace);
            let started = json!({"type":"agentMessage","id":"agent","text":"","phase":phase,"memoryCitation":null});
            let error = state.validate(&json!({"method":"item/started","params":{"threadId":"t","turnId":"u","startedAtMs":3,"item":started}}), "t", "u").unwrap_err();
            assert_eq!(
                error.to_string(),
                "protocol error: tool, unknown, or malformed item is forbidden in agent-only scenario"
            );
        }

        let mut state = state_after_user(prompt, &workspace);
        let started = json!({"type":"agentMessage","id":"agent","text":"","phase":null,"memoryCitation":null});
        state.validate(&json!({"method":"item/started","params":{"threadId":"t","turnId":"u","startedAtMs":3,"item":started}}), "t", "u").unwrap();
        let completed = json!({"type":"agentMessage","id":"agent","text":"x","phase":"final_answer","memoryCitation":null});
        let error = state.validate(&json!({"method":"item/completed","params":{"threadId":"t","turnId":"u","completedAtMs":4,"item":completed}}), "t", "u").unwrap_err();
        assert_eq!(
            error.to_string(),
            "protocol error: mismatched or duplicate agent item completion"
        );

        let mut state = state_after_user(prompt, &workspace);
        let started = json!({"type":"agentMessage","id":"agent","text":"","phase":"final_answer","memoryCitation":null});
        state.validate(&json!({"method":"item/started","params":{"threadId":"t","turnId":"u","startedAtMs":3,"item":started}}), "t", "u").unwrap();
        let completed = json!({"type":"agentMessage","id":"agent","text":"x","phase":"final_answer","memoryCitation":null});
        state.validate(&json!({"method":"item/completed","params":{"threadId":"t","turnId":"u","completedAtMs":4,"item":completed}}), "t", "u").unwrap();
        let raw_without_phase = json!({"method":"rawResponseItem/completed","params":{"threadId":"t","turnId":"u","item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"x"}]}}});
        let error = state.validate(&raw_without_phase, "t", "u").unwrap_err();
        assert_eq!(
            error.to_string(),
            "protocol error: raw response item was out of order or duplicate"
        );
    }

    #[test]
    fn agent_only_frame_gate_allows_benign_and_rejects_tools() {
        let prompt = "exact prompt";
        let workspace = std::env::temp_dir().canonicalize().unwrap();
        let mut state = super::AgentOnlyState::new(prompt, &workspace);
        let thread = json!({"method":"thread/started","params":{"thread":{
            "id":"t","sessionId":"t","forkedFromId":null,"parentThreadId":null,
            "preview":"","ephemeral":true,"modelProvider":"openai","createdAt":1,"updatedAt":1,
            "status":{"type":"idle"},"path":null,"cwd":workspace,"cliVersion":"0.139.0",
            "source":"vscode","threadSource":null,"agentNickname":null,"agentRole":null,
            "gitInfo":null,"name":null,"turns":[]
        }}});
        state.validate(&thread, "t", "u").unwrap();
        state
            .validate(
                &json!({"method":"turn/started","params":{"threadId":"t","turn":{"id":"u","status":"inProgress"}}}),
                "t",
                "u",
            )
            .unwrap();
        state
            .validate(
                &json!({"method":"rawResponseItem/completed","params":{"threadId":"t","turnId":"u","item":{"type":"message","role":"user","content":[{"type":"input_text","text":prompt}]}}}),
                "t",
                "u",
            )
            .unwrap();
        let user = json!({"type":"userMessage","id":"user","clientId":null,"content":[{"type":"text","text":prompt,"text_elements":[]}]});
        state
            .validate(
                &json!({"method":"item/started","params":{"threadId":"t","turnId":"u","startedAtMs":1,"item":user}}),
                "t",
                "u",
            )
            .unwrap();
        state
            .validate(
                &json!({"method":"item/completed","params":{"threadId":"t","turnId":"u","completedAtMs":2,"item":user}}),
                "t",
                "u",
            )
            .unwrap();
        for method in [
            "item/commandExecution/outputDelta",
            "item/commandExecution/terminalInteraction",
            "item/fileChange/outputDelta",
            "item/fileChange/patchUpdated",
            "item/mcpToolCall/progress",
            "item/autoApprovalReview/started",
            "item/autoApprovalReview/completed",
            "turn/diff/updated",
            "hook/started",
            "item/unknownTool/progress",
        ] {
            let frame = json!({"method":method,"params":{"threadId":"t","turnId":"u","delta":"x"}});
            let error = state.validate(&frame, "t", "u").unwrap_err();
            if method == "hook/started" {
                assert_eq!(
                    error.to_string(),
                    "protocol error: tool activity is forbidden in agent-only scenario"
                );
            }
        }
        let assistant = json!({"type":"agentMessage","id":"assistant","text":"","phase":null,"memoryCitation":null});
        state.validate(&json!({"method":"item/started","params":{"threadId":"t","turnId":"u","startedAtMs":3,"item":assistant}}), "t", "u").unwrap();
        state.validate(&json!({"method":"item/agentMessage/delta","params":{"threadId":"t","turnId":"u","itemId":"assistant","delta":"x"}}), "t", "u").unwrap();
        let assistant = json!({"type":"agentMessage","id":"assistant","text":"x","phase":null,"memoryCitation":null});
        state.validate(&json!({"method":"item/completed","params":{"threadId":"t","turnId":"u","completedAtMs":4,"item":assistant}}), "t", "u").unwrap();
        state.validate(&json!({"method":"rawResponseItem/completed","params":{"threadId":"t","turnId":"u","item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"x"}]}}}), "t", "u").unwrap();
        for item_type in [
            "commandExecution",
            "fileChange",
            "mcpToolCall",
            "dynamicToolCall",
            "webSearch",
            "collabAgentToolCall",
            "imageGeneration",
            "imageView",
            "hookPrompt",
            "contextCompaction",
            "enteredReviewMode",
            "exitedReviewMode",
            "unknownItem",
        ] {
            let frame = json!({"method":"item/started","params":{"threadId":"t","turnId":"u","startedAtMs":3,"item":{"type":item_type}}});
            assert!(state.validate(&frame, "t", "u").is_err(), "{item_type}");
        }
        let plan_item = json!({"type":"plan","id":"plan","text":""});
        state.validate(&json!({"method":"item/started","params":{"threadId":"t","turnId":"u","startedAtMs":5,"item":plan_item}}), "t", "u").unwrap();
        let plan = json!({"method":"item/plan/delta","params":{"threadId":"t","turnId":"u","itemId":"plan","delta":"not R1"}});
        state.validate(&plan, "t", "u").unwrap();
        assert_eq!(super::agent_delta_bytes(&plan, "t", "u"), None);
        let completed_plan = json!({"type":"plan","id":"plan","text":"not R1"});
        state
            .validate(
                &json!({"method":"item/completed","params":{"threadId":"t","turnId":"u","completedAtMs":6,"item":completed_plan}}),
                "t",
                "u",
            )
            .unwrap();
        for frame in [
            json!({"method":"thread/name/updated","params":{"threadId":"t","threadName":"name"}}),
            json!({"method":"thread/status/changed","params":{"threadId":"t","status":{"type":"active"}}}),
            json!({"method":"thread/tokenUsage/updated","params":{"threadId":"t","turnId":"u","tokenUsage":{}}}),
            json!({"method":"turn/moderationMetadata","params":{"threadId":"t","turnId":"u","metadata":{}}}),
        ] {
            state.validate(&frame, "t", "u").unwrap();
            assert_eq!(super::agent_delta_bytes(&frame, "t", "u"), None);
        }
        let aggregate = json!({"method":"item/completed","params":{"threadId":"t","turnId":"u","item":{"type":"agentMessage","text":"not R1"}}});
        assert_eq!(super::agent_delta_bytes(&aggregate, "t", "u"), None);
        state.finish("A", &super::TerminalState::Completed).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn approval_inner_command_executes_only_in_the_exact_outer_powershell_cwd() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "codex-s2-approval-command-{}-{nonce}",
            std::process::id()
        ));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let command = super::approval_command().unwrap();
        let shell = super::trusted_scenario_shell().unwrap();
        let output = std::process::Command::new(shell)
            .args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(&command)
            .current_dir(&workspace)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let marker = workspace.join(super::APPROVAL_MARKER_NAME);
        assert!(marker.is_file());
        assert_eq!(std::fs::read(marker).unwrap(), super::APPROVAL_MARKER_BYTES);
        assert!(!root.join(".codex-s2-approval-marker").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn approval_wrapper_is_byte_exact_and_escapes_backslashes_before_quotes() {
        let expected = r#"& \"C:\Windows\System32\cmd.exe\" /d /s /c \"<nul set /p =S2_APPROVED>.codex-s2-approval-marker&exit /b 0\""#;
        let pwsh = Path::new(r"C:\Program Files\PowerShell\7\pwsh.exe");
        let wrapped = super::approval_wrapper_candidate(pwsh, expected).unwrap();
        let escaped = expected.replace('\\', r"\\").replace('"', r#"\""#);
        let escaped_pwsh = pwsh.to_string_lossy().replace('\\', r"\\");
        assert_eq!(wrapped, format!(r#""{escaped_pwsh}" -Command "{escaped}""#));
        assert!(wrapped.starts_with(r#""C:\\Program Files\\PowerShell"#));
        assert!(!wrapped.starts_with(r#"\""#));
        assert!(!wrapped.ends_with(r#"\""#));
    }

    #[test]
    #[cfg(windows)]
    fn explicit_wrapper_is_locked_before_final_path_resolution() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "codex-s2-atomic-wrapper-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let wrapper_path = root.join("pwsh.exe");
        let displaced_path = root.join("displaced.exe");
        std::fs::write(&wrapper_path, b"trusted").unwrap();

        let wrapper =
            super::acquire_explicit_approval_wrapper_with_post_open(&wrapper_path, || {
                assert!(
                    std::fs::rename(&wrapper_path, &displaced_path).is_err(),
                    "the raw input path must already be locked against replacement"
                );
            })
            .unwrap();

        assert_eq!(
            wrapper.path,
            super::normalize_windows_command_path(wrapper_path.canonicalize().unwrap()).unwrap()
        );
        drop(wrapper);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn marker_reader_locks_the_opened_identity_before_observing_content() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "codex-s2-marker-identity-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let marker = root.join(super::APPROVAL_MARKER_NAME);
        let displaced = root.join("displaced-marker");
        std::fs::write(&marker, super::APPROVAL_MARKER_BYTES).unwrap();

        let bytes = super::read_approval_marker_with_post_open(&marker, || {
            assert!(
                std::fs::rename(&marker, &displaced).is_err(),
                "the opened marker must already exclude delete sharing"
            );
        })
        .unwrap();

        assert_eq!(bytes, super::APPROVAL_MARKER_BYTES);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn marker_helper_image_is_locked_and_revalidated_before_spawn() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "codex-s2-helper-image-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let image = root.join("codex-app-server-capture.exe");
        let displaced = root.join("displaced.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &image).unwrap();

        let mut invocation =
            super::MarkerHelperInvocation::acquire_windows_path_with_post_open(&image, || {
                assert!(
                    std::fs::rename(&image, &displaced).is_err(),
                    "helper image must be locked before its final path is trusted"
                );
            })
            .unwrap();
        assert!(
            std::fs::OpenOptions::new()
                .write(true)
                .open(&image)
                .is_err()
        );
        assert!(invocation.command().is_ok());
        invocation.identity.as_mut().unwrap().file_index ^= 1;
        assert!(invocation.command().is_err());
        drop(invocation);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn child_reap_continues_after_poll_error_until_exit_is_observed() {
        use std::cell::Cell;
        use std::collections::VecDeque;

        let mut polls = VecDeque::from([
            Err(std::io::Error::other("synthetic poll failure")),
            Ok(false),
            Ok(true),
        ]);
        let kills = Cell::new(0);
        super::wait_until_reaped_with(
            std::time::Instant::now() + std::time::Duration::from_secs(1),
            || polls.pop_front().unwrap(),
            || {
                kills.set(kills.get() + 1);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(kills.get(), 1);
        assert!(polls.is_empty());
    }

    #[test]
    fn process_group_termination_takes_the_pgid_only_once() {
        let mut process_group = Some(4242);
        let mut killed = Vec::new();
        super::terminate_process_group_once(&mut process_group, |pgid| killed.push(pgid));
        super::terminate_process_group_once(&mut process_group, |pgid| killed.push(pgid));
        assert_eq!(killed, vec![4242]);
        assert_eq!(process_group, None);
    }

    #[test]
    fn linux_marker_helper_uses_the_running_image_inode_path() {
        assert_eq!(
            super::linux_marker_helper_program(),
            std::path::PathBuf::from("/proc/self/exe")
        );
    }

    #[test]
    #[cfg(windows)]
    fn marker_reader_rejects_a_static_symlink_when_supported() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "codex-s2-marker-symlink-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("target");
        let marker = root.join(super::APPROVAL_MARKER_NAME);
        std::fs::write(&target, super::APPROVAL_MARKER_BYTES).unwrap();
        if std::os::windows::fs::symlink_file(&target, &marker).is_ok() {
            assert!(super::read_approval_marker_atomically(&marker).is_err());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn marker_reader_rejects_symlinks_and_fifos_without_blocking() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::symlink;

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "codex-s2-marker-special-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("target");
        std::fs::write(&target, super::APPROVAL_MARKER_BYTES).unwrap();
        let link = root.join("link");
        symlink(&target, &link).unwrap();
        assert!(super::read_approval_marker_atomically(&link).is_err());

        let fifo = root.join("fifo");
        let fifo_c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
        let started = std::time::Instant::now();
        assert!(super::read_approval_marker_atomically(&fifo).is_err());
        assert!(started.elapsed() < std::time::Duration::from_millis(250));
        std::fs::remove_dir_all(root).unwrap();
    }

    fn valid_config_frame(home: &Path) -> serde_json::Value {
        #[cfg(windows)]
        let system_config = super::known_program_data()
            .unwrap()
            .join("OpenAI")
            .join("Codex")
            .join("config.toml");
        #[cfg(not(windows))]
        let system_config = std::path::PathBuf::from("/etc/codex/config.toml");
        let session_name = json!({"type":"sessionFlags"});
        let user_name = json!({"type":"user","file":home.join("config.toml"),"profile":null});
        let mut origins = serde_json::Map::new();
        origins.insert(
            "cli_auth_credentials_store".into(),
            json!({"name":user_name.clone(),"version":"user-1"}),
        );
        for key in [
            "project_root_markers.0",
            "project_doc_max_bytes",
            "skills.include_instructions",
            "skills.bundled.enabled",
            "analytics.enabled",
            "otel.exporter",
            "otel.trace_exporter",
            "otel.metrics_exporter",
            "features.hooks",
            "features.plugins",
            "features.apps",
            "features.shell_snapshot",
            "features.memories",
        ] {
            origins.insert(
                key.into(),
                json!({"name":session_name.clone(),"version":"session-1"}),
            );
        }
        json!({"result":{
            "config":{
                "cli_auth_credentials_store":"file","notify":[],
                "project_doc_max_bytes":0,"project_root_markers":[".codex-s2-root"],"mcp_servers":{},
                "model_providers":{},"experimental_thread_config_endpoint":null,
                "analytics":{"enabled":false},
                "otel":{"exporter":"none","trace_exporter":"none","metrics_exporter":"none"},
                "skills":{"include_instructions":false,"bundled":{"enabled":false}},
                "features":{"hooks":false,"plugins":false,"apps":false,"shell_snapshot":false,"memories":false}
            },"origins":origins,"layers":[
                {"name":session_name,"version":"session-1","config":{
                    "notify":[],"project_root_markers":[".codex-s2-root"],"project_doc_max_bytes":0,
                    "skills":{"include_instructions":false,"bundled":{"enabled":false}},
                    "analytics":{"enabled":false},"otel":{"exporter":"none","trace_exporter":"none","metrics_exporter":"none"},
                    "features":{"hooks":false,"plugins":false,"apps":false,"shell_snapshot":false,"memories":false}
                }},
                {"name":user_name,"version":"user-1","config":{
                    "cli_auth_credentials_store":"file","analytics":{"enabled":false},
                    "otel":{"exporter":"none","trace_exporter":"none","metrics_exporter":"none"},
                    "skills":{"include_instructions":false,"bundled":{"enabled":false}}
                }},
                {"name":{"type":"system","file":system_config},"version":"system-1","config":{}}
            ]
        }})
    }

    #[test]
    fn config_preflight_rejects_privileged_layers_and_effective_side_effects() {
        let home = std::env::temp_dir();
        super::validate_effective_config(&valid_config_frame(&home), &home).unwrap();
        for source in [
            "system",
            "enterpriseManaged",
            "legacyManaged",
            "mdm",
            "project",
        ] {
            let mut frame = valid_config_frame(&home);
            frame["result"]["layers"] = json!([{"name":{"type":source},"version":"1","config":{}}]);
            assert_eq!(
                super::validate_effective_config(&frame, &home)
                    .unwrap_err()
                    .to_string(),
                "config preflight failed [CFG_LAYER_COUNT]"
            );
        }
        for pointer in [
            "/result/config/mcp_servers",
            "/result/config/model_providers",
        ] {
            let mut frame = valid_config_frame(&home);
            *frame.pointer_mut(pointer).unwrap() = json!({"unsafe":{}});
            assert_eq!(
                super::validate_effective_config(&frame, &home)
                    .unwrap_err()
                    .to_string(),
                "config preflight failed [CFG_EFFECTIVE_FIXED]"
            );
        }
        let mut frame = valid_config_frame(&home);
        frame["result"]["config"]["experimental_thread_config_endpoint"] =
            json!("https://private.invalid");
        assert!(super::validate_effective_config(&frame, &home).is_err());
        #[cfg(windows)]
        {
            let mut frame = valid_config_frame(&home);
            frame["result"]["layers"][2]["name"]["file"] = json!(
                super::known_program_data()
                    .unwrap()
                    .join("OpenAI/Codex/config.toml")
            );
            assert_eq!(
                super::validate_effective_config(&frame, &home)
                    .unwrap_err()
                    .to_string(),
                "config preflight failed [CFG_SYSTEM_PATH]"
            );
        }
        for (mutation, code) in [
            ("order", "CFG_LAYER_USER"),
            ("duplicate", "CFG_LAYER_COUNT"),
            ("system-config", "CFG_LAYER_SYSTEM"),
            ("system-path", "CFG_SYSTEM_PATH"),
            ("user-config", "CFG_LAYER_USER"),
            ("session-config", "CFG_LAYER_SESSION"),
            ("origins", "CFG_ORIGIN_COUNT"),
        ] {
            let mut frame = valid_config_frame(&home);
            match mutation {
                "order" => frame["result"]["layers"].as_array_mut().unwrap().swap(1, 2),
                "duplicate" => {
                    let duplicate = frame["result"]["layers"][0].clone();
                    frame["result"]["layers"]
                        .as_array_mut()
                        .unwrap()
                        .push(duplicate);
                }
                "system-config" => {
                    frame["result"]["layers"][2]["config"] = json!({"model":"unsafe"})
                }
                "system-path" => {
                    frame["result"]["layers"][2]["name"]["file"] = json!(home.join("config.toml"))
                }
                "user-config" => frame["result"]["layers"][1]["config"]["extra"] = json!(true),
                "session-config" => frame["result"]["layers"][0]["config"]["extra"] = json!(true),
                "origins" => {
                    frame["result"]["origins"]["extra"] =
                        frame["result"]["origins"]["project_root_markers.0"].clone()
                }
                _ => unreachable!(),
            }
            assert_eq!(
                super::validate_effective_config(&frame, &home)
                    .unwrap_err()
                    .to_string(),
                format!("config preflight failed [{code}]"),
                "mutation {mutation} returned the wrong predicate code"
            );
        }

        for legacy_key in ["notify", "project_root_markers"] {
            let mut frame = valid_config_frame(&home);
            frame["result"]["origins"][legacy_key] =
                frame["result"]["origins"]["project_root_markers.0"].clone();
            assert_eq!(
                super::validate_effective_config(&frame, &home)
                    .unwrap_err()
                    .to_string(),
                "config preflight failed [CFG_ORIGIN_COUNT]",
                "non-leaf origin {legacy_key} returned the wrong predicate code"
            );
        }

        for (origin, code) in [
            ("cli_auth_credentials_store", "CFG_ORIGIN_USER"),
            ("analytics.enabled", "CFG_ORIGIN_SESSION"),
        ] {
            let mut frame = valid_config_frame(&home);
            frame["result"]["origins"][origin]["version"] = json!("wrong-version");
            assert_eq!(
                super::validate_effective_config(&frame, &home)
                    .unwrap_err()
                    .to_string(),
                format!("config preflight failed [{code}]")
            );
        }

        for feature in ["hooks", "plugins", "apps", "shell_snapshot", "memories"] {
            let mut frame = valid_config_frame(&home);
            frame["result"]["config"]["features"][feature] = json!(true);
            assert_eq!(
                super::validate_effective_config(&frame, &home)
                    .unwrap_err()
                    .to_string(),
                format!(
                    "config preflight failed [CFG_FEATURE_{}]",
                    feature.to_ascii_uppercase()
                )
            );
        }
    }

    #[test]
    fn config_predicate_codes_are_the_only_diagnostic_detail_in_sanitized_artifacts() {
        let sanitized = super::sanitized_error_text(
            r"config preflight failed [CFG_SYSTEM_PATH]: C:\private\account\config.toml",
        );
        assert_eq!(
            sanitized,
            "protocol/scenario precondition failed [CFG_SYSTEM_PATH]"
        );
        assert!(!sanitized.contains("private"));
        assert_eq!(
            super::sanitized_error_text("untrusted server text CFG_PRIVATE_VALUE"),
            "protocol/scenario precondition failed"
        );
    }

    #[test]
    #[cfg(windows)]
    fn neutral_home_rejects_unc_roots_generically() {
        let mut command = std::process::Command::new("cmd.exe");
        assert_eq!(
            super::apply_neutral_home_environment(
                &mut command,
                std::path::Path::new(r"\\server\share\codex-s2")
            )
            .unwrap_err()
            .to_string(),
            "neutral home environment setup failed"
        );
    }

    #[test]
    fn config_requirements_preflight_requires_exactly_null() {
        super::validate_config_requirements(&json!({"result":{"requirements":null}})).unwrap();
        for frame in [
            json!({"result":{}}),
            json!({"result":{"requirements":{}}}),
            json!({"result":{"requirements":null,"extra":true}}),
        ] {
            assert!(super::validate_config_requirements(&frame).is_err());
        }
    }

    #[test]
    fn managed_surface_audit_rejects_any_existing_object() {
        let root = std::env::temp_dir().join(format!(
            "codex-s2-managed-audit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let absent = root.join("absent");
        super::validate_managed_surfaces(std::slice::from_ref(&absent)).unwrap();
        std::fs::create_dir(&root).unwrap();
        std::fs::write(&absent, b"managed").unwrap();
        assert_eq!(
            super::validate_managed_surfaces(&[absent])
                .unwrap_err()
                .to_string(),
            "S2 managed configuration audit failed"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

fn empty_evidence() -> S2Evidence {
    S2Evidence {
        scenarios: ["A", "B", "C", "D"]
            .map(|name| ScenarioEvidence {
                name: name.to_owned(),
                terminal_state: TerminalState::Missing,
                turn_completed: false,
                first_delta_seen: false,
                r1_sufficient: false,
                approval_seen: false,
                interrupt_response_seen: false,
            })
            .to_vec(),
        auth_errors: 0,
        quota_errors: 0,
        protocol_errors: 0,
        performance: None,
        candidate_percentiles: None,
    }
}

fn classify_failure(error: &anyhow::Error, evidence: &mut S2Evidence) {
    let message = format!("{error:#}").to_ascii_lowercase();
    if message.contains("malformed success") {
        evidence.protocol_errors = 1;
    } else if message.contains("auth") || message.contains("unauthorized") {
        evidence.auth_errors = 1;
    } else if message.contains("quota")
        || message.contains("usage limit")
        || message.contains("usagelimit")
        || message.contains("rate limit")
        || message.contains("ratelimit")
    {
        evidence.quota_errors = 1;
    } else {
        evidence.protocol_errors = 1;
    }
}

fn write_artifacts(
    output_dir: &Path,
    evidence: &S2Evidence,
    report: &S2Report,
    error: Option<&anyhow::Error>,
) -> Result<()> {
    std::fs::write(
        output_dir.join(EVIDENCE_FILE),
        format!("{}\n", serde_json::to_string_pretty(evidence)?),
    )?;
    std::fs::write(
        output_dir.join(REPORT_FILE),
        format!("{}\n", serde_json::to_string_pretty(report)?),
    )?;
    let status = if report.valid { "PASS" } else { "INVALID" };
    let detail = match error {
        Some(err) => sanitized_error_text(&format!("{err:#}")),
        None if report.valid => "all F1-F3 gates passed".to_owned(),
        None => report.reasons.join("; "),
    };
    let timing = evidence
        .candidate_percentiles
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|metrics| {
            Some(format!(
                "interrupt_response_latency_ms={:.3} interrupt_terminal_latency_ms={:.3}",
                metrics.get("interrupt_response_latency_ms")?.as_f64()?,
                metrics.get("interrupt_terminal_latency_ms")?.as_f64()?
            ))
        })
        .unwrap_or_else(|| "interrupt_timings=unavailable".to_owned());
    std::fs::write(
        output_dir.join(SUMMARY_FILE),
        format!(
            "S2 {status}\nF1={} F2={} F3={}\n{timing}\n{detail}\n",
            report.f1.passed, report.f2.passed, report.f3.passed,
        ),
    )?;
    Ok(())
}

fn sanitized_error_text(text: &str) -> String {
    if let Some(code) = text
        .split(|character: char| {
            !(character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_')
        })
        .find(|token| {
            matches!(
                *token,
                "CFG_RESULT_SHAPE"
                    | "CFG_LAYER_COUNT"
                    | "CFG_LAYER_SESSION"
                    | "CFG_LAYER_USER"
                    | "CFG_LAYER_SYSTEM"
                    | "CFG_SYSTEM_PATH"
                    | "CFG_ORIGIN_COUNT"
                    | "CFG_ORIGIN_USER"
                    | "CFG_ORIGIN_SESSION"
                    | "CFG_EFFECTIVE_FIXED"
                    | "CFG_FEATURE_HOOKS"
                    | "CFG_FEATURE_PLUGINS"
                    | "CFG_FEATURE_APPS"
                    | "CFG_FEATURE_SHELL_SNAPSHOT"
                    | "CFG_FEATURE_MEMORIES"
            )
        })
    {
        return format!("protocol/scenario precondition failed [{code}]");
    }
    let lower = text.to_ascii_lowercase();
    if lower.contains("usage limit") || lower.contains("quota") || lower.contains("rate limit") {
        "quota/rate-limit precondition failed".to_owned()
    } else if lower.contains("auth") || lower.contains("unauthorized") {
        "authentication precondition failed".to_owned()
    } else if lower.contains("timeout") {
        "bounded scenario/global timeout expired".to_owned()
    } else if lower.contains("approval") {
        "approval safety/protocol precondition failed".to_owned()
    } else {
        "protocol/scenario precondition failed".to_owned()
    }
}

fn default_output_dir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("codex-s2-{}-{nonce}", std::process::id()))
}

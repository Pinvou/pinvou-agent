//! `~/.pinvou3/` directory layout resolution.
//!
//! pinvou3-app never reads `~/.deepseek/` (isolation); every deepseek-tui
//! default that writes to global/cwd fields is mapped into this separate
//! directory tree. See the plan's "directory layout" section for the layout.
//!
//! The `PINVOU3_HOME` environment variable relocates the whole tree (mainly for tests).

use std::path::PathBuf;
use std::sync::OnceLock;

static RUNTIME_RESOURCE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// The user home directory `$HOME`, the root of pinvou3-app's engine workspace.
/// AI access via relative paths lands under the home; access via absolute paths is
/// let through by trust_mode, while sensitive subdirectories are fenced off by the
/// path filter / instructions.
pub fn user_home_dir() -> PathBuf {
    crate::platform::os::user_home_dir()
}

/// The `~/.pinvou3/` root directory.
pub fn pinvou3_home() -> PathBuf {
    if let Ok(custom) = std::env::var("PINVOU3_HOME") {
        return crate::platform::os::platform_compat_path(&custom);
    }
    user_home_dir().join(".pinvou3")
}

/// Folds the storage-root prefix inside arbitrary text (usually an io::Error
/// Display embedding an absolute path such as `create /home/u/.pinvou3/…`)
/// into `~/.pinvou3`, for error text leaving with success payloads: absolute
/// paths must not cross the relay. Non-prefix text is returned as-is.
///
/// Round-12 review: when a fold happens, path separators in the remaining text
/// are normalized to `/` so the outbound form is cross-platform consistent
/// (Windows `join`/`Display` produce `\`, and the folded result used to be
/// `~/.pinvou3\user\...` while consumers assert/pin the forward-slash form).
/// This function's output is display text for phone payloads and logs and is
/// never fed back into filesystem APIs, so separator normalization affects no
/// path resolution; text that was not folded is returned unchanged (matching
/// the existing display_redacted identity pin).
pub fn redact_storage_prefix(text: &str) -> String {
    // Round-14 review NIT: an env-override install may carry a trailing
    // separator (PINVOU3_HOME="/tmp/x/"); a literal replace would swallow the
    // separator too and fold out glued forms like "~/.pinvou3settings.json";
    // strip trailing separators before folding.
    let mut prefix = pinvou3_home().display().to_string();
    while prefix.ends_with('/') || prefix.ends_with('\\') {
        prefix.pop();
    }
    if prefix.is_empty() {
        return text.to_string();
    }
    // Round-28 review MINOR-4: when the needle is a substring of the
    // replacement text (PINVOU3_HOME set to degenerate relative values such as
    // `~`/`3`/`pinv`), the folded output still contains the needle — re-entrant
    // re-folding grows without bound, each read-back heals larger than the last,
    // and the idempotence contract breaks. A real absolute home always contains
    // a second separator or a drive-letter colon and can never be a substring of
    // `~/.pinvou3`, so this shape can only be a degenerate deployment; block it
    // conservatively by returning the text unchanged (it already reads as the
    // `~/.pinvou3` relative form, no fold needed).
    if "~/.pinvou3".contains(prefix.as_str()) {
        return text.to_string();
    }
    // Round-15 review F4: Windows drive-letter/user-segment casing is unstable
    // (`c:\users\…` in error text vs home `C:\Users\…` previously did not fold
    // at all, letting absolute paths cross the relay). Matching is now ASCII
    // case-insensitive; on Linux, case-sensitive filesystems' spelling variants
    // merely get display-level over-folding (safe direction), affecting no path
    // resolution. (Round-18 review residual note: the replacement is
    // ASCII-limited in both directions — when PINVOU3_HOME itself contains
    // non-ASCII letters, byte-level spelling variants of it in error text still
    // do not fold; an extremely rare install shape, handled as a disclosed
    // residual. Round-19 review adds two same-class residuals: the needle is
    // pinvou3_home()'s native Display spelling, so (a) forward-slash absolute
    // paths (`C:/Users/…`, external tool stderr / file URLs, etc.) and (b) the
    // verbatim-UNC normalized spelling of a UNC home (`\\?\UNC\server\share\…`,
    // produced only by canonicalize-like constructions) do not contain the
    // needle verbatim and do not fold; both also leaked under the pre-PR
    // byte-exact replace — not a regression. Verified that no current error
    // constructor produces either spelling; introducing a canonicalize-like
    // construction later must extend the needle in the same change.
    // Round-21 review adds a third same-class residual: when PINVOU3_HOME is a
    // relative value (the tests' `p8`, a bare directory name from a malformed
    // deployment), the needle is that relative string, and any identical
    // substring in separator-free text gets over-folded (display-safe wide
    // direction, same family as the Linux case over-folding above); an empty
    // needle is already blocked above, so no truncation risk.
    // (Round-28 review addendum: the degenerate shape where the needle itself is
    // a substring of the replacement — `~`, `3`, `pinv`, etc., folded output
    // still contains the needle, idempotence breaks — is now blocked by the
    // identity guard below and no longer part of this residual; non-substring
    // relative needles like `p8` remain disclosed under this residual.)
    // Noted here rather than at call sites: redact_storage_prefix folds only
    // the `~/.pinvou3` storage root, not the user `$HOME` itself — the
    // `list_host_files` default `$HOME` enumeration error arm moved to
    // command-layer stable-code folding in round-21 (M3); relying on this
    // primitive to catch `$HOME` usage does not hold, the stable code is
    // required. Round-22 review adds a fourth same-class residual: the needle
    // is read from the current PINVOU3_HOME at call time — old warnings
    // persisted under a previous home (still inside the 20-entry history window
    // after a home migration / data-directory restore) no longer contain the
    // current needle, and read-back folding cannot save them; the trigger
    // surface is narrow (requires a home change plus old-entry survival) and
    // what leaks is a stale old path, handled as a disclosed residual.)
    let folded = replace_ascii_case_insensitive(text, &prefix, "~/.pinvou3");
    if folded == text {
        folded
    } else {
        folded.replace('\\', "/")
    }
}

/// ASCII case-insensitive literal replacement (not a case-folded variant of
/// `replace`, avoiding a regex dependency). Advances character by character,
/// copying UTF-8 multi-byte characters whole.
fn replace_ascii_case_insensitive(text: &str, from: &str, to: &str) -> String {
    let bytes = text.as_bytes();
    let needle = from.as_bytes();
    if needle.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if i + needle.len() <= bytes.len()
            && bytes[i..i + needle.len()].eq_ignore_ascii_case(needle)
        {
            out.push_str(to);
            i += needle.len();
        } else {
            match text[i..].chars().next() {
                Some(ch) => {
                    out.push(ch);
                    i += ch.len_utf8();
                }
                // Unreachable: the loop condition guarantees `text[i..]` is
                // non-empty, so `chars().next()` is always `Some` (the
                // loop advances by whole characters only).
                None => break,
            }
        }
    }
    out
}

pub fn settings_path() -> PathBuf {
    pinvou3_home().join("settings.json")
}

/// `~/.pinvou3/logs/memory-review.log` — conversation memory review diagnostic log.
/// Records only phases, categories, and counts; never conversation text or memory bodies.
pub fn memory_review_log() -> PathBuf {
    pinvou3_home().join("logs").join("memory-review.log")
}

pub fn bundle_root() -> PathBuf {
    pinvou3_home().join("bundle")
}
pub fn bundle_skills_dir() -> PathBuf {
    bundle_root().join("skills")
}
/// `~/.pinvou3/bundles/` — package-directory root aggregated per package
/// (marketplace-unification §4: one package = one directory = one owner).
/// Marketplace skills land in `bundles/<pkg-id>/skills/<name>/`; built-in bundled
/// skills (visual-design etc., non-marketplace packages) stay in `bundle/skills/`.
pub fn bundles_root() -> PathBuf {
    pinvou3_home().join("bundles")
}
pub fn bundle_mcp_json() -> PathBuf {
    bundle_root().join("mcp.json")
}
/// `~/.pinvou3/bundle/mcp-servers/` — pinvou3's built-in MCP server script directory.
pub fn bundle_mcp_servers_dir() -> PathBuf {
    bundle_root().join("mcp-servers")
}

fn managed_connectors_dir() -> PathBuf {
    pinvou3_home().join("connectors")
}
/// Legacy (version-less) CLI layout: `connectors/<platform>/bin/`. From the Phase 2
/// fourth cut, new installs land in `assets/cli/<name>/<version>/`; this entry point
/// remains for existing-data migration and spawn fallback resolution.
pub fn managed_connector_bin_dir() -> Option<PathBuf> {
    managed_connector_bin_dir_for(std::env::consts::OS, std::env::consts::ARCH)
}
fn managed_connector_bin_dir_for(os: &str, arch: &str) -> Option<PathBuf> {
    connector_platform_dir(os, arch)
        .map(|platform| managed_connectors_dir().join(platform).join("bin"))
}

/// `~/.pinvou3/assets/` — versioned external asset store (marketplace-unification
/// §4: packages reference but do not own; the lifecycle is driven by the lock table).
fn assets_root() -> PathBuf {
    pinvou3_home().join("assets")
}

/// `~/.pinvou3/assets/cli/<name>/<version>/` — CLI binaries land at the version pinned
/// by the lock table; upgrading means the new version directory is in place, never an
/// in-place overwrite.
pub fn assets_cli_dir(name: &str, version: &str) -> PathBuf {
    assets_root().join("cli").join(name).join(version)
}

/// `~/.pinvou3/assets/.staging/` — download/unpack staging (absorbing the retired `cache/connectors/`).
pub fn assets_staging_dir() -> PathBuf {
    assets_root().join(".staging")
}

/// Platform directory names supporting on-demand connector CLI download/install.
/// The name selects the compile-time lock file as well as the runtime on-disk
/// location `~/.pinvou3/connectors/<platform>/bin/`.
pub fn connector_platform_dir(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("linux", "aarch64") => Some("linux-arm64"),
        ("linux", "x86_64") => Some("linux-x64"),
        ("macos", "aarch64") => Some("darwin-arm64"),
        ("macos", "x86_64") => Some("darwin-x64"),
        ("windows", "x86_64") => Some("windows-x64"),
        _ => None,
    }
}

/// Tauri's real resource_dir. Written once by setup before any frontend command runs;
/// Linux/macOS connector npm commands use it to locate the bundled Node and npm CLI.
pub fn set_runtime_resource_dir(path: PathBuf) {
    let _ = RUNTIME_RESOURCE_DIR.set(path);
}

fn runtime_resource_dir() -> Option<PathBuf> {
    RUNTIME_RESOURCE_DIR
        .get()
        .cloned()
        .or_else(|| std::env::var_os("PINVOU3_RESOURCE_DIR").map(PathBuf::from))
}

pub fn bundled_connector_node() -> Option<PathBuf> {
    let (path, _) = bundled_connector_runtime_paths_for(
        &runtime_resource_dir()?,
        std::env::consts::OS,
        std::env::consts::ARCH,
    )?;
    path.is_file().then_some(path)
}

pub fn bundled_connector_npm_cli() -> Option<PathBuf> {
    let (_, path) = bundled_connector_runtime_paths_for(
        &runtime_resource_dir()?,
        std::env::consts::OS,
        std::env::consts::ARCH,
    )?;
    path.is_file().then_some(path)
}

fn bundled_connector_runtime_paths_for(
    resource_dir: &std::path::Path,
    os: &str,
    arch: &str,
) -> Option<(PathBuf, PathBuf)> {
    let root = resource_dir.join("runtime/codex-bridge/node");
    let node = match (os, arch) {
        // linux/loongarch64 ships the same unified Node runtime layout as the
        // other Linux arches (packaged by the deb); the connector CLI
        // platform dir (connector_platform_dir) deliberately keeps returning
        // None there because no connector artifacts exist upstream.
        ("linux", "aarch64" | "x86_64" | "loongarch64") => root.join("bin/node"),
        ("macos", "aarch64") => root.join("darwin-arm64/bin/node"),
        ("macos", "x86_64") => root.join("darwin-x64/bin/node"),
        _ => return None,
    };
    let npm = root.join("lib/node_modules/npm/bin/npm-cli.js");
    Some((node, npm))
}
/// Absolute path of the present_artifact MCP server script (mcp.json args point at it).
pub fn bundle_present_artifact_server() -> PathBuf {
    bundle_mcp_servers_dir().join("present_artifact_server.py")
}
/// Local Python MCP bootstrap that adds the tool's isolated dependency directory before running
/// its server script.
pub fn bundle_mcp_python_runner() -> PathBuf {
    bundle_mcp_servers_dir().join("python_dependency_runner.py")
}
/// Absolute path of the bundle version file (`~/.pinvou3/bundle/VERSION`).
pub fn bundle_version_file() -> PathBuf {
    bundle_root().join("VERSION")
}

// --- Browser feature paths (features/browser) ---

/// Coordination, runtime-mapping, and restoration directory for the in-app native browser:
/// `~/.pinvou3/browser/`.
pub fn browser_home() -> PathBuf {
    pinvou3_home().join("browser")
}
/// CDP port coordination file: `{ port, pid, owner: "app"|"mcp", started_at }`.
/// Rust BrowserManager and browser-wrapper.mjs use it to coordinate one instance
/// idempotently.
pub fn browser_cdp_port_json() -> PathBuf {
    browser_home().join("cdp-port.json")
}
/// Dedicated data directory for the native Windows WebView2 surface.
///
/// It persists sign-in state across tasks and application restarts. It is never used for
/// external Chrome and has no screenshot-stream compatibility fallback.
pub fn browser_webview_profile_dir() -> PathBuf {
    browser_home().join("webview2-profile")
}
/// Directory for on-demand startup requests written when an MCP wrapper first needs the
/// browser. Each wrapper uses its own file so concurrent first calls in two conversations
/// cannot overwrite one another.
pub fn browser_host_requests_dir() -> PathBuf {
    browser_home().join("host-requests")
}
/// Authoritative runtime mapping for the native browser. Only the corresponding MCP wrapper
/// uses it to synchronize the host's tabToken-to-automation-target bijection. It must be
/// deleted on application exit, and its target IDs must never be reused across processes.
pub fn browser_workspaces_dir() -> PathBuf {
    browser_home().join("workspaces")
}
pub fn browser_workspace_state_json(session_token: &str) -> PathBuf {
    browser_workspaces_dir().join(format!("{session_token}.json"))
}
/// Restorable page manifest for a conversation browser. The path uses a stable session
/// digest. The content stores only URL order and active index, never the raw session ID,
/// session/tab tokens, target IDs, leases, or cookies. After restart, the host creates new
/// WebView/tab identities and automation targets from this manifest.
pub fn browser_workspace_restore_dir() -> PathBuf {
    browser_home().join("restore")
}
pub fn browser_workspace_restore_json(session_token: &str) -> PathBuf {
    browser_workspace_restore_dir().join(format!("{session_token}.json"))
}
/// Most recent native-host or app-owned CDP connection failure: `{ reason, at }`. The
/// wrapper writes it and Rust may inject an allowlisted model-visible explanation while it
/// remains fresh for 24 hours. It never describes external Chrome state.
pub fn browser_last_error_json() -> PathBuf {
    browser_home().join("last-error.json")
}
/// Work-mode session-specific mcp.json containing global mcp.json plus the browser entry.
/// Browser MCP tools are exposed only to Work-mode assistant Engine sessions. Global
/// mcp.json never registers the browser entry, and external Agents such as Codex ACP do not
/// read this file.
pub fn browser_work_mcp_json() -> PathBuf {
    browser_home().join("mcp.work.json")
}
/// Conversation-scoped Browser MCP configuration. The filename uses only a stable token
/// without path characters and never embeds the raw session ID, preventing invalid path
/// characters and directory traversal.
pub fn browser_session_mcp_dir() -> PathBuf {
    browser_home().join("mcp-sessions")
}
pub fn browser_session_mcp_json(session_id: &str) -> PathBuf {
    browser_session_mcp_dir().join(format!("{}.json", browser_session_token(session_id)))
}

/// Per-aux scoped MCP configuration directory (ADR-0024): holds the
/// token-named session-reader configs that pin an auxiliary engine to its
/// parent task's session id. Sibling layout of the browser `mcp-sessions`
/// directory, but directly under the Pinvou home — it never contains user
/// servers, only app-written single-entry configs.
pub fn aux_mcp_sessions_dir() -> PathBuf {
    pinvou3_home().join("mcp-sessions")
}
/// Conversation-scoped aux MCP config path. Mirrors
/// [`browser_session_mcp_json`]: the filename is the stable session token
/// (the shared fnv1a digest behind `browser_session_token`), so the raw
/// session id never appears in the path — no invalid path characters, no
/// traversal, no id disclosure in directory listings.
pub fn aux_session_mcp_json(session_id: &str) -> PathBuf {
    aux_mcp_sessions_dir().join(format!("{}.json", browser_session_token(session_id)))
}

/// Stable FNV-1a 64-bit digest over raw bytes. Shared building block for
/// stable local identifiers derived from session IDs; callers format the
/// returned value (e.g. `{value:016x}`) for their own file/label needs.
/// `pub` (round-41 review): the CLI's plugin-id fallback derives the same
/// stable stem hash the GUI does, from the shared digest.
pub fn fnv1a64(data: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in data {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Stable FNV-1a token for a session ID. It is used only for local filenames, WebView labels,
/// and initial-page markers, not authentication. Requests retain the complete session_id,
/// which the main application recomputes and validates.
pub fn browser_session_token(session_id: &str) -> String {
    format!("{:016x}", fnv1a64(session_id.as_bytes()))
}
/// Browser MCP wrapper embedded at compile time and extracted to
/// `~/.pinvou3/bundle/mcp-servers/`.
pub fn bundle_browser_wrapper() -> PathBuf {
    bundle_mcp_servers_dir().join("browser-wrapper.mjs")
}
/// Vendored chrome-devtools-mcp entry point distributed through the package resource_dir.
pub fn bundled_chrome_devtools_mcp_bin() -> Option<PathBuf> {
    let res = runtime_resource_dir()?;
    let bin = res.join("runtime/chrome-devtools-mcp/build/src/bin/chrome-devtools-mcp.js");
    bin.is_file().then_some(bin)
}

/// Interpreter command used to launch python MCP servers (present_artifact / pptx etc.).
///
/// - **Windows**: prefer the `PINVOU3_PYTHON` written by the installer, then the
///   bundled `python/pythonw.exe` (no console window, no dependence on the user
///   machine's Python), finally falling back to the real system Python. The Microsoft
///   Store `WindowsApps\python.exe` placeholder is skipped.
/// - **Other platforms** (Linux/macOS): use system `python3` (near-universal on Linux;
///   GUI subprocesses do not pop windows; dependencies are installed by the
///   marketplace's automatic pip installs).
pub fn python_command() -> String {
    crate::platform::os::python_command()
}

/// Locked Python dependencies may only load through the app's bundled interpreter.
/// On Windows `PINVOU3_PYTHON` and the system Python are refused so user site/sitecustomize cannot fill in missing locked dependencies.
pub fn managed_python_command() -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        return crate::platform::os::windows::bundled_python_path()
            .map(|path| path.to_string_lossy().into_owned())
            .ok_or_else(|| {
                "bundled Python runtime is missing; cannot safely load locked MCP dependencies"
                    .to_string()
            });
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(python_command())
    }
}

fn user_root() -> PathBuf {
    pinvou3_home().join("user")
}
pub fn user_instructions() -> PathBuf {
    user_root().join("instructions.md")
}
pub fn user_skills_dir() -> PathBuf {
    user_root().join("skills")
}
/// `~/.pinvou3/user/personas/` — user-created expert persona cards (the card pool).
/// One `<id>.json` per card (PersonaCard serialization). Kept separate from the
/// built-in cards embedded in the bundle; **never overwritten**.
pub fn user_personas_dir() -> PathBuf {
    user_root().join("personas")
}

/// Compatibility field: the Phase B legacy sandbox workspace (no longer used as the
/// engine workspace, but kept as an "AI private sandbox" fallback — some scenarios
/// such as monitor tests still use it).
pub fn workspace_dir() -> PathBuf {
    pinvou3_home().join("workspace")
}
pub fn notes_path() -> PathBuf {
    pinvou3_home().join("notes.md")
}
pub fn memory_path() -> PathBuf {
    pinvou3_home().join("memory.md")
}
pub fn user_memory_dir() -> PathBuf {
    user_root().join("memory")
}
pub fn user_memory_profile() -> PathBuf {
    user_memory_dir().join("profile.json")
}
pub fn user_memory_preferences_dir() -> PathBuf {
    user_memory_dir().join("preferences")
}
pub fn user_memory_work_context_dir() -> PathBuf {
    user_memory_dir().join("work_context")
}
pub fn user_memory_pending() -> PathBuf {
    user_memory_dir().join("_pending.jsonl")
}
pub fn user_memory_never() -> PathBuf {
    user_memory_dir().join("_never.jsonl")
}
pub fn user_memory_recent_work() -> PathBuf {
    user_memory_dir().join("recent_work.jsonl")
}
pub fn user_memory_current_focus() -> PathBuf {
    user_memory_dir().join("current_focus.jsonl")
}
pub fn user_memory_recent_activity() -> PathBuf {
    user_memory_dir().join("recent_activity.jsonl")
}
pub fn user_memory_runtime_dir() -> PathBuf {
    user_memory_dir().join("runtime")
}
pub fn user_memory_snapshot() -> PathBuf {
    user_memory_dir().join("snapshot.md")
}
pub fn user_memory_organize_history() -> PathBuf {
    user_memory_dir().join("organize_history.json")
}
pub fn user_memory_runtime_prompt(session_id: &str) -> PathBuf {
    user_memory_runtime_dir().join(format!("{}.md", sanitize_memory_runtime_id(session_id)))
}
pub fn mcp_config_path() -> PathBuf {
    bundle_mcp_json()
}

/// `~/.pinvou3/sessions/` — root directory where all conversation history is persisted.
pub fn sessions_root() -> PathBuf {
    pinvou3_home().join("sessions")
}

/// Scheduled-run data is separated from ordinary chat history.
fn scheduled_runs_root() -> PathBuf {
    pinvou3_home().join("scheduled-runs")
}

/// `~/.pinvou3/scheduled/` — root directory of scheduled-task workspaces.
/// Each automation owns a separate workspace under `<automation_id>/workspace/`;
/// the automation's multiple run conversations share it, and different automations
/// never share one.
pub fn scheduled_tasks_root() -> PathBuf {
    pinvou3_home().join("scheduled")
}

pub fn scheduled_task_workspace_dir(automation_id: &str) -> PathBuf {
    scheduled_tasks_root().join(automation_id).join("workspace")
}

/// App-owned classification and immutable runtime settings for scheduled sessions.
pub fn scheduled_run_profiles_path() -> PathBuf {
    scheduled_runs_root().join("session-profiles.json")
}

/// `~/.pinvou3/projects/` — on-disk root of the project layer (logical session-archive grouping).
fn projects_root() -> PathBuf {
    pinvou3_home().join("projects")
}

/// The project layer's only persisted file: project definitions and the session
/// membership mapping live in one file, covered by a single atomic write for both
/// views to avoid a half-committed state.
pub fn projects_store_path() -> PathBuf {
    projects_root().join("projects.json")
}

/// App-owned viewed state for independent scheduled-run conversations.
pub fn scheduled_run_read_state_path() -> PathBuf {
    scheduled_runs_root().join("read-state.json")
}

fn sanitize_memory_runtime_id(raw: &str) -> String {
    let sanitized: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

/// `~/.pinvou3/feedback/`.
fn feedback_root() -> PathBuf {
    pinvou3_home().join("feedback")
}

/// `~/.pinvou3/feedback/pending/` — where the headless CLI stages a feedback
/// request while its submission is in flight.
pub fn feedback_pending_dir() -> PathBuf {
    feedback_root().join("pending")
}

/// `~/.pinvou3/feedback/receipts/` — the local receipt the headless CLI
/// writes for each submission.
pub fn feedback_receipts_dir() -> PathBuf {
    feedback_root().join("receipts")
}

/// `~/.pinvou3/sessions/<session_id>/artifacts/` — the AI's default artifact drop directory.
/// The `$PINVOU3_SESSION_ARTIFACTS` environment variable injects this value into engine + LLM.
pub fn session_artifacts_dir(session_id: &str) -> PathBuf {
    sessions_root().join(session_id).join("artifacts")
}

/// `~/.pinvou3/sessions/<session_id>/workspace/` — each session's own working directory.
/// The engine workspace follows the current active session to avoid multi-session
/// shared-file conflicts; on session switch the bridge calls
/// `Op::SyncSession { workspace }` to reset.
pub fn session_workspace_dir(session_id: &str) -> PathBuf {
    sessions_root().join(session_id).join("workspace")
}

/// `~/.pinvou3/sessions/<session_id>/skills/` — the skill set materialized for this
/// session by scope (dual-scope skill governance: see
/// `features/assistant/skill_materialization.rs`). The session-private directory is
/// cleaned up together with the session; no separate cleanup logic needed.
pub fn session_skills_dir(session_id: &str) -> PathBuf {
    sessions_root().join(session_id).join("skills")
}

/// `~/.pinvou3/sessions/<session_id>/persona_events.json` — this session's persona-card
/// equip/unequip event timeline (sidecar). **Deliberately separate from messages**: on
/// cold start the engine replays messages back into the LLM, while card events are pure
/// frontend display and must never enter the LLM context. The frontend re-inserts them
/// by `pos` (the messages count when the event happened) in rerenderFromMessages.
pub fn session_persona_events(session_id: &str) -> PathBuf {
    sessions_root().join(session_id).join("persona_events.json")
}

/// `~/.pinvou3/sessions/<session_id>/pinvou_reviews.json` — this session's Pinvou
/// invocation review timeline (one {pos, review} per entry). Like persona_events,
/// **deliberately separate from messages**: review cards are pure frontend display and
/// must never enter the LLM context (it would pollute the main AI); the frontend
/// re-inserts them by `pos` in rerenderFromMessages. For the Boss to let the main AI
/// see a review, use the "hand off" button to send it as a Boss message.
pub fn session_pinvou_reviews(session_id: &str) -> PathBuf {
    sessions_root().join(session_id).join("pinvou_reviews.json")
}

/// `~/.pinvou3/sessions/<session_id>/pinvou_scene_events.json` — expert-scene display
/// labels for user messages (one `{pos, scene}` per entry). Separate from messages like
/// the persona/review sidecars, keeping pure UI metadata out of the LLM context while
/// letting desktop and WebUI share recovery.
pub fn session_pinvou_scene_events(session_id: &str) -> PathBuf {
    sessions_root()
        .join(session_id)
        .join("pinvou_scene_events.json")
}

/// `~/.pinvou3/sessions/<session_id>/steered_messages.json` — position markers for
/// mid-turn steer messages in the session (one `{pos, text}` per entry). Steer
/// persistence aligns with ordinary admission and contains no `<turn_meta>` block, so
/// the reload projection can no longer infer "non-turn admission" from the envelope;
/// this display-layer fact is therefore persisted explicitly. Separate from messages
/// like the scene sidecar: pure UI metadata stays out of the LLM context; `text` is
/// used for conservative validation after compaction/edit drift — on mismatch, no marker.
pub fn session_steered_messages(session_id: &str) -> PathBuf {
    sessions_root()
        .join(session_id)
        .join("steered_messages.json")
}

/// `~/.pinvou3/sessions/<session_id>/timing_events.jsonl` — end-to-end per-turn timing
/// events (sidecar). Deliberately separate from messages/session schema so context and
/// artifact logic stay unaffected.
pub fn session_timing_events(session_id: &str) -> PathBuf {
    sessions_root().join(session_id).join("timing_events.jsonl")
}

/// `~/.pinvou3/sessions/default/artifacts/` — shared artifact drop point for MCP stdio
/// servers (PPT / official documents etc.). stdio servers cannot reliably know the
/// current GUI session; tool events carrying a `session_id` archive artifacts into the
/// specific session.
pub fn default_session_artifacts_dir() -> PathBuf {
    session_artifacts_dir("default")
}

/// Ensures all directories exist on first startup. Subdirectories such as bundle/skills are ensured again at unpack time.
pub fn ensure_dirs() -> std::io::Result<()> {
    std::fs::create_dir_all(bundle_skills_dir())?;
    std::fs::create_dir_all(user_skills_dir())?;
    std::fs::create_dir_all(user_personas_dir())?;
    std::fs::create_dir_all(workspace_dir())?;
    std::fs::create_dir_all(scheduled_tasks_root())?;
    std::fs::create_dir_all(default_session_artifacts_dir())?;
    // Round-9 review minor: the storage root itself is created implicitly by the
    // create_dir_all calls above, with mode bits left to umask (commonly 0755) —
    // the 0600 secret files inside are separated from other accounts on the same
    // machine by only one directory level. Best-effort tighten an existing root to
    // 0700; failures are tolerated (read-only mounts etc. must not block startup;
    // a failed tightening only reduces hardening depth, it changes no behavior).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let home = pinvou3_home();
        if let Ok(metadata) = std::fs::metadata(&home) {
            if metadata.is_dir() {
                let _ = std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Process-wide env vars are a hard isolation obstacle for tests: cargo
    /// test runs tests in parallel by default, and concurrent modifications of
    /// PINVOU3_HOME would clobber each other's assertions. This is the
    /// **crate-wide single env lock source**: every test that mutates env
    /// vars (bridge/mod.rs EnvGuard/DEEPSEEK_*, feedback, notifications, and
    /// other modules) borrows this lock, so env-writing tests serialize
    /// against one another.
    ///
    /// Note: a single lock source only provides mutual exclusion between
    /// **env-writing tests that hold the lock**; it does **not** mean
    /// `--test-threads=1` can be dropped — env readers that do not take the
    /// lock may still observe another test's temporary value. After mutex
    /// poisoning, the guard obtained via `PoisonError::into_inner()` is
    /// still a locked guard, so mutual exclusion is not bypassed. CI runs
    /// tests single-threaded for now; the real fix is to remove tests'
    /// dependence on process-level env vars, or route all reads and writes
    /// through a single isolation layer.
    pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

    pub(crate) use crate::platform::test_support::EnvRestore as EnvVarGuard;

    #[test]
    fn env_var_guard_restores_preexisting_absent_and_panic_paths() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let host_index = std::env::var_os("GIT_INDEX_FILE");
        let host_objects = std::env::var_os("GIT_OBJECT_DIRECTORY");
        // Outer guard protects the host's true values from this test's own
        // sentinel writes (and restores them even if an assert below panics).
        let host = EnvVarGuard::capture(&["GIT_INDEX_FILE", "GIT_OBJECT_DIRECTORY"]);

        // Pre-set non-empty originals, as a host shell could have them.
        // SAFETY: ENV_LOCK held (first line of this test).
        unsafe { std::env::set_var("GIT_INDEX_FILE", "pinvou-guard-sentinel-index") };
        // SAFETY: same as above.
        unsafe { std::env::set_var("GIT_OBJECT_DIRECTORY", "pinvou-guard-sentinel-objects") };

        // Non-empty restore path: overwrite, then drop the guard.
        {
            let guard = EnvVarGuard::capture(&["GIT_INDEX_FILE"]);
            // SAFETY: ENV_LOCK held.
            unsafe { std::env::set_var("GIT_INDEX_FILE", "polluted") };
            assert_eq!(
                std::env::var_os("GIT_INDEX_FILE").as_deref(),
                Some(std::ffi::OsStr::new("polluted"))
            );
            drop(guard);
        }
        assert_eq!(
            std::env::var_os("GIT_INDEX_FILE").as_deref(),
            Some(std::ffi::OsStr::new("pinvou-guard-sentinel-index")),
            "guard must restore the pre-existing non-empty value"
        );

        // Panic path: Drop runs during unwinding inside catch_unwind.
        let panicked = std::panic::catch_unwind(|| {
            let _guard = EnvVarGuard::capture(&["GIT_OBJECT_DIRECTORY"]);
            // SAFETY: ENV_LOCK held.
            unsafe { std::env::set_var("GIT_OBJECT_DIRECTORY", "panic-pollution") };
            panic!("guard must restore during unwind");
        });
        assert!(panicked.is_err());
        assert_eq!(
            std::env::var_os("GIT_OBJECT_DIRECTORY").as_deref(),
            Some(std::ffi::OsStr::new("pinvou-guard-sentinel-objects")),
            "guard must restore during unwind"
        );

        // Absent restore path: the outer guard removes the sentinels again,
        // back to exactly the host's true state.
        drop(host);
        assert_eq!(std::env::var_os("GIT_INDEX_FILE"), host_index);
        assert_eq!(std::env::var_os("GIT_OBJECT_DIRECTORY"), host_objects);
    }

    /// Produces a monotonically increasing suffix unique **within the process**,
    /// for naming test temp directories / session IDs.
    ///
    /// Pure nanosecond timestamps collide under high concurrency (hot caches plus
    /// thread-scheduling jitter can land two calls on the same nanosecond), producing
    /// duplicate temp directories / session IDs → tests overwrite each other's files.
    /// Adding this atomic counter makes every call unique **within the process**,
    /// eliminating naming collisions between parallel tests.
    ///
    /// Note: the counter is process-level — two concurrent `cargo test` **processes**
    /// (e.g. two local terminals) each count from 0, and a temp-directory name built
    /// only from this suffix (without a pid) still collides across processes. Temp
    /// directory naming should therefore also mix in `std::process::id()` (see
    /// `scheduled/tasks.rs::temp_home`); this suffix alone guarantees uniqueness only
    /// within one process.
    pub(crate) fn unique_suffix() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        COUNTER.fetch_add(1, Ordering::Relaxed)
    }

    /// Plant a FIFO at `path` for the hardening fixtures: a planted FIFO must
    /// be refused by the regular-file-gated reads instead of blocking the
    /// caller (mkfifo is POSIX-only, so callers gate themselves with
    /// `#[cfg(unix)]`).
    #[cfg(test)]
    #[cfg(unix)]
    pub(crate) fn plant_fifo(path: &std::path::Path) {
        let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: mkfifo on a fresh temp path; no other thread touches it.
        let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
        assert_eq!(rc, 0, "fixture: mkfifo must succeed");
    }

    /// Round-9 review minor: even under a permissive umask (0755, backup-restore /
    /// legacy-install shapes) the storage root must be tightened to 0700 by
    /// ensure_dirs — the 0600 secret files inside cannot rely on a single
    /// world-readable directory layer for isolation. Tightening failures are
    /// tolerated, but the success path is pinned here.
    #[test]
    #[cfg(unix)]
    fn ensure_dirs_tightens_the_storage_root_to_0700() {
        use std::os::unix::fs::PermissionsExt as _;
        crate::platform::test_support::with_temp_home("paths-root-mode", || {
            let home = pinvou3_home();
            // Pre-seed the permissive shape: explicit 0755 simulates a legacy install /
            // backup restore (umask varies by environment, cannot rely on it).
            std::fs::create_dir_all(&home).unwrap();
            std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o755)).unwrap();
            ensure_dirs().unwrap();
            let mode = std::fs::metadata(&home).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o700,
                "ensure_dirs must tighten the storage root to 0700"
            );
        });
    }

    #[test]
    fn pinvou3_home_respects_env_override() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", "/tmp/pinvou3-test-override") };
        assert_eq!(
            pinvou3_home(),
            crate::platform::os::platform_compat_path("/tmp/pinvou3-test-override")
        );
        assert_eq!(
            settings_path(),
            crate::platform::os::platform_compat_path("/tmp/pinvou3-test-override")
                .join("settings.json")
        );
        match prev {
            // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn managed_python_rejects_pinvou3_python_override() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let previous = std::env::var_os("PINVOU3_PYTHON");
        let root = std::env::temp_dir().join(format!(
            "pinvou3-managed-python-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let ambient = root.join("python.exe");
        std::fs::write(&ambient, b"ambient").unwrap();
        // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_PYTHON", &ambient) };

        assert_eq!(python_command(), ambient.to_string_lossy());
        assert!(managed_python_command().is_err());

        match previous {
            // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_PYTHON", value) },
            // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_PYTHON") },
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scheduled_paths_are_derived_from_pinvou_home() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let previous = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", "/tmp/pinvou3-scheduled-paths") };

        assert_eq!(scheduled_runs_root(), pinvou3_home().join("scheduled-runs"));
        assert_eq!(scheduled_tasks_root(), pinvou3_home().join("scheduled"));
        assert_eq!(
            scheduled_task_workspace_dir("automation-1"),
            pinvou3_home()
                .join("scheduled")
                .join("automation-1")
                .join("workspace")
        );
        assert_eq!(
            scheduled_run_profiles_path(),
            pinvou3_home()
                .join("scheduled-runs")
                .join("session-profiles.json")
        );
        assert_eq!(
            scheduled_run_read_state_path(),
            pinvou3_home()
                .join("scheduled-runs")
                .join("read-state.json")
        );
        if let Some(value) = previous {
            // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
            unsafe { std::env::set_var("PINVOU3_HOME", value) };
        } else {
            // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
            unsafe { std::env::remove_var("PINVOU3_HOME") };
        }
    }

    /// `user_home_dir` should read $HOME (the root of the pinvou3 engine workspace).
    #[test]
    fn user_home_dir_reads_home_env() {
        assert!(!user_home_dir().as_os_str().is_empty());
    }

    #[test]
    fn connector_bin_dir_covers_all_on_demand_platforms() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", "/tmp/pinvou3-connector-path-test") };
        let root = crate::platform::os::platform_compat_path("/tmp/pinvou3-connector-path-test");
        let expected = |platform: &str| Some(root.join("connectors").join(platform).join("bin"));
        assert_eq!(
            managed_connector_bin_dir_for("linux", "aarch64"),
            expected("linux-arm64")
        );
        assert_eq!(
            managed_connector_bin_dir_for("linux", "x86_64"),
            expected("linux-x64")
        );
        // Connectors have no loongarch64 upstream artifacts: the platform directory stays None (explicit downgrade).
        assert_eq!(managed_connector_bin_dir_for("linux", "loongarch64"), None);
        assert_eq!(
            managed_connector_bin_dir_for("macos", "aarch64"),
            expected("darwin-arm64")
        );
        assert_eq!(
            managed_connector_bin_dir_for("macos", "x86_64"),
            expected("darwin-x64")
        );
        assert_eq!(
            managed_connector_bin_dir_for("windows", "x86_64"),
            expected("windows-x64")
        );
        assert_eq!(managed_connector_bin_dir_for("windows", "aarch64"), None);
        assert_eq!(managed_connector_bin_dir_for("freebsd", "x86_64"), None);
        match prev {
            // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
    }

    #[test]
    fn connector_runtime_paths_cover_packaged_linux_and_macos_layouts() {
        let resource = PathBuf::from("/opt/pinvou/resources");
        let npm = resource.join("runtime/codex-bridge/node/lib/node_modules/npm/bin/npm-cli.js");
        assert_eq!(
            bundled_connector_runtime_paths_for(&resource, "linux", "x86_64"),
            Some((
                resource.join("runtime/codex-bridge/node/bin/node"),
                npm.clone()
            ))
        );
        // The loongarch64 deb ships the same unified Node runtime (bin/node).
        assert_eq!(
            bundled_connector_runtime_paths_for(&resource, "linux", "loongarch64"),
            Some((
                resource.join("runtime/codex-bridge/node/bin/node"),
                npm.clone()
            ))
        );
        assert_eq!(
            bundled_connector_runtime_paths_for(&resource, "macos", "aarch64"),
            Some((
                resource.join("runtime/codex-bridge/node/darwin-arm64/bin/node"),
                npm.clone()
            ))
        );
        assert_eq!(
            bundled_connector_runtime_paths_for(&resource, "macos", "x86_64"),
            Some((
                resource.join("runtime/codex-bridge/node/darwin-x64/bin/node"),
                npm
            ))
        );
        assert_eq!(
            bundled_connector_runtime_paths_for(&resource, "windows", "x86_64"),
            None
        );
    }

    /// Session artifact paths must land under ~/.pinvou3/sessions/<id>/artifacts/.
    #[test]
    fn session_artifacts_layout() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", "/tmp/pinvou3-artifacts-layout-test") };
        let root = crate::platform::os::platform_compat_path("/tmp/pinvou3-artifacts-layout-test");
        assert_eq!(
            session_artifacts_dir("abc123"),
            root.join("sessions").join("abc123").join("artifacts")
        );
        assert_eq!(
            default_session_artifacts_dir(),
            root.join("sessions").join("default").join("artifacts")
        );
        match prev {
            // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
    }

    // Round-7 review: the shared redaction primitive `redact_storage_prefix`
    // previously had only content-level pins from its callers (memory /
    // remote_control), with no direct test of the primitive's own semantic
    // boundaries. The five pins below pin the byte semantics: folding of absolute
    // paths under the root, sibling directories sharing the name prefix must not be
    // rewritten into "inside-root" form, already-folded text is not re-folded, every
    // occurrence in one text folds, and the empty-prefix guard does not panic. The
    // prefix is injected via PINVOU3_HOME (read fresh on every call, no caching);
    // env always goes through the with_temp_home / locked_env scaffolding, and on
    // assertion failure env is restored by Drop so it cannot leak into later tests
    // in the process (Round-9 review minor: straight-line restoration would be
    // skipped on panic paths).

    #[test]
    fn redact_storage_prefix_folds_expanded_absolute_storage_root_prefix() {
        crate::platform::test_support::with_temp_home("paths-redact-root", || {
            let root = pinvou3_home();
            // A fully expanded absolute path under the root must fold to the ~/.pinvou3 prefix.
            let expanded = root.join("user/memory/snapshot.md").display().to_string();
            assert_eq!(
                redact_storage_prefix(&expanded),
                "~/.pinvou3/user/memory/snapshot.md",
                "an expanded absolute path under the storage root must fold"
            );
        });
    }

    /// Round-12 review: separators in the folded remainder are normalized to `/` —
    /// Windows `join`/`Display` produce `\`, and the outbound form must be the
    /// cross-platform-consistent forward-slash form. The input contains backslashes
    /// on POSIX too (display text is never fed back into filesystem APIs), so the
    /// assertion runs on every platform.
    #[test]
    fn redact_storage_prefix_normalizes_separators_in_the_folded_remainder() {
        crate::platform::test_support::with_temp_home("paths-redact-separators", || {
            let root = pinvou3_home();
            let text = format!("{}\\user\\memory\\snapshot.md", root.display());
            assert_eq!(
                redact_storage_prefix(&text),
                "~/.pinvou3/user/memory/snapshot.md",
                "the folded remainder must use forward slashes on every platform"
            );
        });
    }

    /// Round-14 review M1: the `ad0971d2e` parallel wave swallowed this pin's
    /// `#[test]` line while inserting the separators pin, silently detaching the pin
    /// body (zero warnings under `dead_code=allow`). Restored property: all five pins
    /// claimed by this file's block comment must really execute.
    #[test]
    fn redact_storage_prefix_keeps_sibling_out_of_storage_root_form() {
        crate::platform::test_support::with_temp_home("paths-redact-sibling", || {
            // The sibling directory shares the storage root's string prefix (a
            // same-named `.evil`-suffixed neighbor of the temp root; a pure string
            // case, nothing on disk): folding must never rewrite the sibling into
            // the "inside-root" form (~/.pinvou3/…), or it could masquerade as a
            // path inside the storage root and be consumed by the model / relay.
            let root = pinvou3_home();
            let sibling = root.parent().unwrap_or(&root).join(format!(
                "{}.evil",
                root.file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default()
            ));
            assert_eq!(
                redact_storage_prefix(&sibling.display().to_string()),
                "~/.pinvou3.evil",
                "the sibling directory must keep its own name and never take the inside-root form"
            );
        });
    }

    /// Round-14 review NIT: a trailing-separator PINVOU3_HOME (env-override install)
    /// must not swallow the separator into a glued fold; a bare mention of the root
    /// itself must fold too.
    #[test]
    fn redact_storage_prefix_strips_trailing_separator_from_the_home() {
        crate::platform::test_support::with_temp_home("paths-redact-trailing-sep", || {
            let root = pinvou3_home();
            // env rewrite inside panic-safe scaffolding: with_temp_home's Drop
            // restores the value saved on entry, so nothing leaks into later tests.
            // SAFETY: single-threaded test body; with_temp_home's Drop
            // restores the saved value even on assertion failure.
            unsafe {
                std::env::set_var(
                    "PINVOU3_HOME",
                    format!("{}{}", root.display(), std::path::MAIN_SEPARATOR),
                );
            }
            let sep = std::path::MAIN_SEPARATOR;
            let nested = format!(
                "{root}{sep}user{sep}memory{sep}snapshot.md",
                root = root.display(),
                sep = sep
            );
            assert_eq!(
                redact_storage_prefix(&nested),
                "~/.pinvou3/user/memory/snapshot.md",
                "a trailing-separator home must fold without gluing the remainder"
            );
            assert_eq!(
                redact_storage_prefix(&root.display().to_string()),
                "~/.pinvou3",
                "a bare mention of the home root must still fold"
            );
        });
    }

    /// Round-15 review F4: drive-letter/user-segment case drift in Windows error text
    /// previously did not fold at all, letting absolute paths cross the relay; matching
    /// is now ASCII case-insensitive. Semantics preserved: non-prefix text does not
    /// fold; a prefix followed by a non-separator still folds per the existing
    /// convention (over-folding is the display-layer-safe direction, see the sibling pin).
    #[test]
    fn redact_storage_prefix_matches_case_insensitively() {
        // Round-23 review probe confirmation: the home directory was previously
        // derived from temp_dir(), so the case-sensitivity teeth inherited TMPDIR's
        // spelling — Linux CI's /tmp is all lowercase, making lower byte-identical to
        // the needle, and a mutation falling back to byte-exact replace kept every
        // redact pin including this one green (the Windows regression leg's filter
        // excludes this module, so that dimension was never really exercised on any
        // CI leg; the same mutation turns this pin red as soon as TMPDIR contains
        // uppercase — mechanism confirmed). The home is now a fixed mixed-case
        // spelling, set directly under ENV_LOCK; pinvou3_home treats the env value as
        // a pure string with nothing on disk, so the teeth no longer depend on the
        // platform temp directory's spelling.
        let (_lock, _env) = crate::platform::test_support::locked_env(&["PINVOU3_HOME"]);
        let root = std::env::temp_dir().join("PathsRedactCaseHome");
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes
        // serialized in-process (platform::test_support discipline).
        unsafe { std::env::set_var("PINVOU3_HOME", &root) };
        let root = pinvou3_home();
        let lower = format!(
            "{}\\user\\memory\\x.json",
            root.display().to_string().to_ascii_lowercase()
        );
        assert!(
            redact_storage_prefix(&lower).starts_with("~/.pinvou3"),
            "a case-variant absolute path must fold"
        );
        let mixed = format!("{}\\User\\MEMORY\\x.json", root.display());
        assert!(
            redact_storage_prefix(&mixed).starts_with("~/.pinvou3"),
            "the home-root needle matches byte-exactly in this vector; the \
             ASCII case-fold teeth live in the lowercase vector above"
        );
        assert_eq!(
            redact_storage_prefix("no path here"),
            "no path here",
            "non-prefix text stays untouched"
        );
    }
    #[test]
    fn redact_storage_prefix_passes_already_folded_text_through() {
        crate::platform::test_support::with_temp_home("paths-redact-folded", || {
            // Text already in the ~/.pinvou3 folded form must pass through unchanged, without re-folding or rewriting.
            let folded = "~/.pinvou3/user/memory/snapshot.md";
            assert_eq!(
                redact_storage_prefix(folded),
                folded,
                "already-folded text must pass through unchanged"
            );
        });
    }

    #[test]
    fn redact_storage_prefix_folds_every_occurrence_in_one_text() {
        crate::platform::test_support::with_temp_home("paths-redact-multi", || {
            // An io::Error Display may embed several absolute paths (e.g. rename's
            // from/to); every occurrence must fold — folding only the first would leak
            // the second absolute path.
            let root = pinvou3_home();
            let text = format!(
                "{} -> {}",
                root.join("a.md").display(),
                root.join("b/c.md").display()
            );
            assert_eq!(
                redact_storage_prefix(&text),
                "~/.pinvou3/a.md -> ~/.pinvou3/b/c.md",
                "every occurrence of the storage root in one text must fold"
            );
        });
    }

    /// Empty storage-root guard: when PINVOU3_HOME is the empty string,
    /// `pinvou3_home().display()` is empty and `redact_storage_prefix` must take the
    /// `prefix.is_empty()` early return, handing the text back unchanged — a
    /// `replace` over an empty prefix would splice the empty string into every
    /// inter-character gap and destroy the whole text. The prefix is computed inside
    /// the function with no test-visible prefix parameter, so the guard is pinned
    /// through the public API (empty PINVOU3_HOME); with_temp_home only sets
    /// non-empty values, so this test uses locked_env to set the empty string by hand
    /// (env is still restored by Drop).
    #[test]
    fn redact_storage_prefix_empty_prefix_guard_returns_input_unchanged() {
        let (_lock, _env) = crate::platform::test_support::locked_env(&["PINVOU3_HOME"]);
        // SAFETY: ENV_LOCK held via locked_env; env writes serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", "") };
        let text = "/abs/path must stay.md";
        assert_eq!(
            redact_storage_prefix(text),
            text,
            "an empty storage prefix must leave the text untouched"
        );
    }

    /// Round-28 review MINOR-4: when the needle is a substring of the replacement
    /// text (PINVOU3_HOME set to degenerate relative values like `~`), the folded
    /// output still contains the needle — re-entrant re-folding grows without bound,
    /// each read-back heals larger than the last. The guard must return identically:
    /// the single-pass output unchanged and re-folding a no-op. Guard-deletion
    /// mutation: both assertions of both folds go red (`~` survives inside the
    /// replacement text `~/.pinvou3`, growing a segment per fold).
    #[test]
    fn redact_storage_prefix_substring_of_replacement_guard_is_idempotent() {
        let (_lock, _env) = crate::platform::test_support::locked_env(&["PINVOU3_HOME"]);
        // SAFETY: ENV_LOCK held via locked_env; env writes serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", "~") };
        let text = "read /etc/shadow failed under ~/.pinvou3/scheduled/x";
        let once = redact_storage_prefix(text);
        assert_eq!(
            once, text,
            "a degenerate home that is a substring of the replacement \
             must not fold at all (the text already reads as the folded form)"
        );
        let twice = redact_storage_prefix(&once);
        assert_eq!(
            twice, once,
            "re-folding the guard's output must be a no-op (idempotency)"
        );
    }
}

//! Linux virtual display stack (Xvfb + optional EWMH WM + xdotool input +
//! x11rb capture) — the disruption-free computer_use target.
//!
//! One session = one `~/.pinvou3/computer-use/vd/<uniq>/` directory holding a
//! 0600 Xauthority cookie file and a `pidfile.json` written IMMEDIATELY after
//! each Xvfb spawn (before readiness) so a crash anywhere afterwards leaves a
//! sweepable record. Input goes through `xdotool` with `DISPLAY`/`XAUTHORITY`
//! set per call; capture opens a fresh x11rb connection per call and does a
//! full-root `GetImage`.
//!
//! x11rb 0.13 facts (checked against the 0.13.2 registry sources):
//! - `RustConnection::connect(dpy_name)` is NOT usable here (it auto-resolves
//!   auth from the user's `~/.Xauthority`, not our private cookie file).
//! - The explicit-cookie path is
//!   `x11rb::reexports::x11rb_protocol::parse_display::parse_display(Some(":N"))`
//!   → `ParsedDisplay::connect_instruction()` → `DefaultStream::connect(&addr)`
//!   → `RustConnection::connect_to_stream_with_auth_info(stream, screen,
//!   auth_name, auth_data)`. NOTE: the protocol crate is re-exported at
//!   `x11rb::reexports::x11rb_protocol` (the Wave-1 stub doc said
//!   `x11rb::x11rb_protocol`; the actual path carries the `reexports` level).
//! - `auth_name` is `b"MIT-MAGIC-COOKIE-1"`; `auth_data` is the RAW 16 cookie
//!   bytes on the wire.
//! - GetImage is core protocol; RustConnection with `default-features = false`
//!   is the pure-Rust (forbid-unsafe) path — no libxcb anywhere.
//!
//! Cookie/Xauthority format decision (deviation from design §5.3's
//! "data(32 hex bytes)" wording): the Xauthority FILE stores the cookie as the
//! RAW 16 bytes, and the wire payload is those same raw bytes. The X server
//! (`-auth`) byte-compares the wire auth_data against the raw bytes it loaded
//! from the file, and Xlib clients (xdotool, open_app children) read the file
//! via XauReadAuth and send those raw bytes too — hex storage would make the
//! x11rb (raw-wire) connection fail its memcmp while file-reading clients
//! passed, so raw is the only form consistent with every consumer. The hex-32
//! spelling stays the human-readable display form (`xauth list` convention).
//!
//! `-displayfd 3` protocol: Xvfb picks a free display number and prints it as
//! `"%d\n"` on fd 3 (Xserver(1) manual). The parser below accepts both `N\n`
//! and `:N\n` for robustness. Because the number is only known after the
//! spawn, the Xauthority file is written twice: once before the spawn (so the
//! `-auth` file exists at the server's first load — the server matches only
//! name+data, so the placeholder display number in the entry is harmless
//! there) and rewritten with the real number right after it is read, before
//! any client (readiness poll, xdotool, open_app) connects.
//!
//! Socket residue: a SIGKILLed X server cannot run its cleanup, so an
//! abnormally-killed server's `/tmp/.X11-unix/X{n}` survives the process —
//! harmless: `-displayfd` never reports a number the server failed to bind,
//! and the next server's number selection retries past sockets already on
//! disk. The NORMAL teardown avoids this: the held Xvfb/WM handles get a
//! SIGTERM-first wind-down ([`Self::kill_recorded_processes`] →
//! `terminate_then_kill`), and an X server exiting on SIGTERM removes its
//! own socket. Only the crash sweep — which must SIGKILL other-process
//! trees outright — can still strand one.
//!
//! Crash/lifecycle contract (design §5/§9):
//! - Every failure after the Xvfb spawn tears down what was spawned; the
//!   pidfile is only removed after all kills are attempted (a crash
//!   mid-teardown leaves it for [`sweep_orphans`]).
//! - A dead server detected DURING an action answers an explicit `Failed`
//!   ("the next action will attempt one restart"); the NEXT action detects the
//!   death up front and attempts ONE lazy respawn with the stored spawn
//!   parameters. A failed respawn answers `Unavailable` with the reason; the
//!   fallback decision itself stays a creation-time-only concern (§9).

use std::io::{ErrorKind, Read as _};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use std::os::unix::io::AsRawFd as _;
use std::os::unix::process::CommandExt as _;
use wait_timeout::ChildExt as _;
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, ImageOrder};
use x11rb::reexports::x11rb_protocol::parse_display::parse_display;
use x11rb::rust_connection::{DefaultStream, RustConnection};

use super::super::super::backend::ComputerUseBackend;
use super::super::super::types::{
    Capabilities, Capture, ComputerUseError, DisplayTarget, ElementInfo, Key, MouseButton,
    ScrollDirection, UiTreeOptions,
};
use super::super::helpers::{
    DRAG_PRESS_SETTLE_MS, DRAG_STEP_DELAY_MS, DRAG_STEPS, HOLD_CANCEL_POLL_MS, MAX_SCROLL_CLICKS,
    TypeRun, combine_drag_errors, combine_hold_errors, drag_waypoints, normalize_typed_newlines,
    split_type_runs,
};
use super::VirtualAvailability;
use crate::platform::os::{kill_pid_tree, try_make_private_dir};
use crate::platform::paths::pinvou3_home;
use crate::platform::process::{output_with_timeout, std_process_group_leader};

/// Virtual screen geometry (design §5: fixed, model-known coordinates).
const SCREEN_GEOMETRY: &str = "1280x800x24";
const SCREEN_DPI: u32 = 96;
/// Deadline for Xvfb to report its display number on fd 3.
const DISPLAYFD_DEADLINE: Duration = Duration::from_secs(5);
/// Deadline for the x11rb readiness poll after the display number is known.
const READY_TIMEOUT: Duration = Duration::from_secs(5);
/// Bounded wall clock for one xdotool input call (mouse/key/click shapes).
const INPUT_TIMEOUT: Duration = Duration::from_secs(10);
/// Bounded wall clock for one xdotool type chunk (200 chars at 12ms/char
/// ≈ 2.4 s, plus process startup on a loaded machine).
const TYPE_TIMEOUT: Duration = Duration::from_secs(15);
/// The design's type chunk bound (§5): `type` runs are capped at 200 chars so
/// a caller timeout stops zombie injection within one chunk. Independent of
/// the shared 64-char chunk constant because the xdotool call is one bounded
/// subprocess per chunk, not per-event hooks.
const TYPE_CHUNK_CHARS: usize = 200;
/// Inter-key delay for `xdotool type`, in MILLISECONDS (xdotool's `type
/// --delay` takes ms; 12 = 12ms per typed character, so a full 200-char chunk
/// costs ≈2.4s — the budget behind [`TYPE_TIMEOUT`]).
const TYPE_DELAY_MS: &str = "12";
/// Delay between the clicks of one multi-click (`--repeat`).
const MULTI_CLICK_DELAY_MS: &str = "40";
/// X auth protocol served by Xvfb's `-auth` file.
const AUTH_PROTOCOL_NAME: &[u8] = b"MIT-MAGIC-COOKIE-1";
/// X11 `FamilyLocal` (X.h): auth entries for local (unix socket) connections.
const FAMILY_LOCAL: u16 = 256;
/// Session dir layout: `~/.pinvou3/computer-use/vd/<name>/{Xauthority,pidfile.json}`.
const AUTHORITY_FILE_NAME: &str = "Xauthority";
const PIDFILE_NAME: &str = "pidfile.json";
const VD_ROOT_SUBPATH: [&str; 2] = ["computer-use", "vd"];
/// WM candidates, first found wins; absence is fine (noted in capabilities).
const WM_CANDIDATES: [&str; 6] = ["mutter", "openbox", "xfwm4", "marco", "fluxbox", "i3"];
/// The fd Xvfb writes its display number to (`-displayfd 3`).
const DISPLAYFD: i32 = 3;
/// Bounded reap grace after a SIGKILL (same budget as platform::process).
const REAP_GRACE: Duration = Duration::from_secs(2);
/// SIGTERM grace before the teardown falls back to SIGKILL: Xvfb exits on
/// SIGTERM immediately and cleans its `/tmp/.X11-unix` socket on that path;
/// one second is generous for a server that answers TERM by design.
const TEARDOWN_TERM_GRACE: Duration = Duration::from_secs(1);
/// fd 3 read chunk; the display number line is a handful of bytes.
const DISPLAYFD_CHUNK: usize = 16;
/// Cap for error-message subprocess stderr excerpts.
const STDERR_EXCERPT_CHARS: usize = 200;
static SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

// ---------------------------------------------------------------------------
// Availability probe (cheap, no side effects)
// ---------------------------------------------------------------------------

/// Cheap probe: required binaries findable on PATH (pure stat walk, NO shell,
/// NO spawn). Missing → `Unavailable` naming the distro packages. The optional
/// WM is NOT part of availability (absent WM = still available, noted in
/// capabilities).
pub(crate) fn probe() -> VirtualAvailability {
    match missing_required_binaries(find_on_path) {
        Some(detail) => VirtualAvailability::Unavailable { detail },
        None => VirtualAvailability::Available,
    }
}

/// Pure probe core: maps a binary-presence lookup to the verdict detail
/// (`None` = everything present). Kept separate from the PATH walk so the
/// package-name wording is unit-testable without machine state.
fn missing_required_binaries(lookup: impl Fn(&str) -> bool) -> Option<String> {
    let missing: Vec<&str> = ["Xvfb", "xdotool"]
        .into_iter()
        .filter(|binary| !lookup(binary))
        .collect();
    if missing.is_empty() {
        return None;
    }
    Some(format!(
        "the isolated virtual display needs Xvfb and xdotool; missing: {}. \
         Install the distro packages `xvfb` and `xdotool` \
         (e.g. `sudo apt install xvfb xdotool` or the equivalent for your distribution).",
        missing.join(", ")
    ))
}

/// `which`-style PATH probe without spawning anything: the first PATH entry
/// containing an executable regular file named `name` wins. A name with a path
/// separator is never resolved through PATH (mirrors exec semantics).
fn find_on_path(name: &str) -> bool {
    if name.is_empty() || name.contains('/') {
        return false;
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        let Ok(metadata) = std::fs::metadata(&candidate) else {
            continue;
        };
        if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
            return true;
        }
    }
    false
}

/// The same PATH walk returning the RESOLVED path of the first executable
/// match. Spawning goes through this resolved path — never through a bare
/// name — so the app's own PATH additions (connector asset dirs, npm shims,
/// user-local bin dirs) cannot silently substitute our tool binaries: the
/// binary that answered the probe is exactly the binary that gets spawned.
fn find_on_path_full(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains('/') {
        return None;
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        let Ok(metadata) = std::fs::metadata(&candidate) else {
            continue;
        };
        if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
            return Some(candidate);
        }
    }
    None
}

/// First available WM candidate (`None` = run WM-less; noted in capabilities).
fn find_wm() -> Option<&'static str> {
    WM_CANDIDATES.into_iter().find(|wm| find_on_path(wm))
}

// ---------------------------------------------------------------------------
// Xauthority file format
// ---------------------------------------------------------------------------

/// One binary Xauthority entry (the file may hold several; we write exactly
/// one). Format (XauWriteAuth): each field is `u16be length | bytes`, fields
/// in order family, address, number, name, data. `family` = 256
/// (`FamilyLocal`), `address` = hostname ASCII, `number` = display number
/// ASCII, `name` = `MIT-MAGIC-COOKIE-1`, `data` = the RAW 16 cookie bytes
/// (see the module docs for the raw-vs-hex decision).
fn build_xauthority_entry(
    family: u16,
    address: &[u8],
    number: &[u8],
    name: &[u8],
    data: &[u8],
) -> Vec<u8> {
    let mut out =
        Vec::with_capacity(2 + 4 * (2 + address.len() + number.len() + name.len() + data.len()));
    out.extend_from_slice(&family.to_be_bytes());
    for field in [address, number, name, data] {
        out.extend_from_slice(&(field.len() as u16).to_be_bytes());
        out.extend_from_slice(field);
    }
    out
}

/// 16 cookie bytes from /dev/urandom. `fs::read` is forbidden here on purpose:
/// /dev/urandom never reaches EOF, so only a bounded `read_exact` is correct.
fn generate_cookie() -> Result<[u8; 16], ComputerUseError> {
    let mut file = std::fs::File::open("/dev/urandom")
        .map_err(|error| ComputerUseError::failed(format!("cannot open /dev/urandom: {error}")))?;
    let mut cookie = [0u8; 16];
    file.read_exact(&mut cookie)
        .map_err(|error| ComputerUseError::failed(format!("cannot read /dev/urandom: {error}")))?;
    Ok(cookie)
}

/// The machine hostname as the Xauthority `FamilyLocal` address. Falls back to
/// the empty address on failure (the server matches entries by name+data only,
/// so a spawn still works; Xlib clients would then fail to select the entry
/// and the readiness poll turns that into an explicit Unavailable).
fn local_hostname() -> String {
    let mut buffer = vec![0u8; 256];
    // SAFETY: gethostname writes at most buffer.len() bytes into the buffer
    // and NUL-terminates within it; the pointer cast only widens the type.
    let ok =
        unsafe { libc::gethostname(buffer.as_mut_ptr().cast::<libc::c_char>(), buffer.len()) } == 0;
    if !ok {
        return String::new();
    }
    let end = buffer
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}

// ---------------------------------------------------------------------------
// Pidfile
// ---------------------------------------------------------------------------

/// The crash-sweep record, written to `<session dir>/pidfile.json`
/// IMMEDIATELY after each Xvfb spawn (before readiness) and rewritten whenever
/// an open_app child joins. Field names are frozen: the startup sweep reads
/// files written by a previous (possibly older) build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PidFile {
    /// The Xvfb process (group leader).
    xvfb_pid: u32,
    /// The optional window manager (group leader), `null` when WM-less.
    wm_pid: Option<u32>,
    /// The X display string, e.g. `":42"`. `""` in the post-spawn
    /// pre-readiness form (the number is only known after `-displayfd`).
    display: String,
    /// Path of the 0600 Xauthority cookie file in the session dir.
    auth_path: String,
    /// Group-leader pids of every `open_app` application. The WM is NOT in
    /// this list — it is `wm_pid`: Stop must not wind the WM down (it is
    /// part of the display, not an agent-opened app).
    children: Vec<u32>,
    /// Per-child kill identity (the pid-recycling gate): argv[0] file name
    /// and kernel start time at record time. Optional and written only when
    /// non-empty so the frozen six-key shape stays byte-stable for empty
    /// lists; an older build's pidfile (children without detail) cannot be
    /// verified, and the sweep then skips those kills — the safe direction.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    children_detail: Vec<ChildRecord>,
    /// Unix seconds when the stack was spawned.
    started_at: u64,
}

/// One recorded `open_app` child with everything needed to kill it safely
/// after a restart: a pid number can be recycled between the crash and the
/// sweep, but the recycled process can neither reproduce the recorded
/// program name (argv[0] file name) nor the recorded kernel start time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ChildRecord {
    pid: u32,
    /// argv[0] file name at spawn time (basename; the model may pass a path).
    program: String,
    /// /proc/<pid>/stat field 22 (start time in clock ticks after boot),
    /// read right after spawn; 0 = unreadable at record time, which can
    /// never match a real start time (the kill is skipped, never guessed).
    starttime: u64,
}

// ---------------------------------------------------------------------------
// The backend
// ---------------------------------------------------------------------------

/// The virtual X11 stack owned by one backend instance. All OS objects live on
/// the computer_use worker thread (same affinity rule as the other backends).
pub(crate) struct VirtualX11Backend {
    /// Session dir (0700) under `~/.pinvou3/computer-use/vd/`; removed on Drop.
    session_dir: PathBuf,
    /// The RAW 16 auth cookie bytes (wire form; also stored raw in the file).
    cookie: [u8; 16],
    /// Xauthority `FamilyLocal` address (the machine hostname).
    hostname: String,
    /// The 0600 cookie file inside [`Self::session_dir`].
    auth_path: PathBuf,
    /// The display string (`":N"`), known only after the displayfd read.
    display: String,
    /// The numeric display behind [`Self::display`].
    display_number: Option<u16>,
    /// The Xvfb child; held so death is detectable via `try_wait` (reaping
    /// zombies as a side effect) and killable on teardown.
    xvfb: Option<Child>,
    /// The optional WM child (same reasons as [`Self::xvfb`]).
    wm: Option<Child>,
    /// The WM binary chosen at creation (re-spawned by lazy restarts).
    wm_program: Option<&'static str>,
    /// Identity records of `open_app` children (mirrored into the pidfile).
    /// The WM is deliberately absent: `release_os_grant` winds THIS list
    /// down, and the WM belongs to the display (kept alive across Stop;
    /// only the teardown paths kill it, via the held `wm` handle).
    children: Vec<ChildRecord>,
    /// Resolved absolute path of xdotool (probe-what-you-spawn: the binary
    /// that answered the availability probe is exactly the one invoked for
    /// every input action — the app's own PATH additions cannot substitute
    /// it). Resolved once at creation.
    xdotool_path: PathBuf,
    /// Request-scoped cancel flag (see the trait docs).
    cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
}

impl VirtualX11Backend {
    /// Build the whole stack: session dir, cookie, Xauthority, Xvfb, readiness
    /// poll, optional WM. Any failure after the Xvfb spawn tears down what was
    /// spawned; the leftover session dir is removed by the Drop of the
    /// partially built backend.
    fn new() -> Result<Self, ComputerUseError> {
        if let VirtualAvailability::Unavailable { detail } = probe() {
            // The dispatch probe said available but the binaries disappeared:
            // answer with the actionable detail, not a generic failure.
            return Err(ComputerUseError::unavailable(detail));
        }
        let session_dir = fresh_session_dir()?;
        let xdotool_path = find_on_path_full("xdotool").ok_or_else(|| {
            // The dispatch probe verified xdotool minutes earlier; a miss here
            // means it vanished — answer with the actionable install hint.
            ComputerUseError::unavailable(missing_required_binaries(find_on_path).unwrap_or_else(
                || "xdotool is not on PATH; install the `xdotool` package".to_string(),
            ))
        })?;
        let mut backend = Self {
            auth_path: session_dir.join(AUTHORITY_FILE_NAME),
            session_dir,
            cookie: generate_cookie()?,
            hostname: local_hostname(),
            display: String::new(),
            display_number: None,
            xvfb: None,
            wm: None,
            wm_program: find_wm(),
            xdotool_path,
            children: Vec::new(),
            cancel: None,
        };
        backend.spawn_stack()?;
        Ok(backend)
    }

    /// Spawn the display stack into the existing session dir. On any error
    /// everything spawned so far is killed (the session dir itself survives a
    /// respawn-path failure so the next lazy attempt can reuse it; the
    /// creation path cleans it up via Drop).
    fn spawn_stack(&mut self) -> Result<(), ComputerUseError> {
        if let Err(error) = self.spawn_stack_inner() {
            self.kill_recorded_processes();
            return Err(error);
        }
        Ok(())
    }

    fn spawn_stack_inner(&mut self) -> Result<(), ComputerUseError> {
        // 1. Xauthority must exist before the spawn: Xvfb's `-auth` file is
        //    loaded by the server during startup. The display number is not
        //    known yet, so this first write carries a placeholder number
        //    (harmless server-side: the server matches entries by name+data
        //    only); it is rewritten with the real number below, before any
        //    client connects.
        self.write_xauthority("")?;
        // 2. Spawn Xvfb with -displayfd 3 (it picks a free display number and
        //    prints it on our socketpair write end).
        let read_end = self.spawn_xvfb()?;
        // 3. Pidfile IMMEDIATELY after the successful spawn (before the
        //    display number and before readiness): a crash anywhere from here
        //    on leaves a sweepable record (design §5).
        self.write_pidfile();
        // 4. Read ":N" (accepting the bare "N" form) from the displayfd.
        let number = match read_display_number(&read_end) {
            Ok(number) => number,
            Err(error) => {
                drop(read_end);
                return Err(error);
            }
        };
        drop(read_end);
        self.display_number = Some(number);
        self.display = format!(":{number}");
        // 5. Rewrite the Xauthority with the real number (Xlib clients select
        //    entries by (family, address, number)) and refresh the pidfile.
        self.write_xauthority(&number.to_string())?;
        self.write_pidfile();
        // 6. Readiness: poll an x11rb connection until the server answers.
        self.wait_ready()?;
        // 7. Optional WM, first candidate found at creation time. Best-effort:
        //    a WM spawn failure downgrades to the WM-less note, it never
        //    fails a session that already has a working display. On the
        //    respawn path this also kills + reaps the previous WM handle
        //    (inside spawn_wm).
        self.spawn_wm();
        // 7b. Respawn path only: the apps of the DEAD server died with it (X
        //    clients cannot survive their server). Prune the verified-dead
        //    pids BEFORE the pidfile rewrite below, so the crash sweep never
        //    targets a since-recycled pid number (MINOR-2).
        self.prune_dead_children();
        self.write_pidfile();
        Ok(())
    }

    /// Spawn Xvfb (process group leader, null stdio, displayfd 3 = dup2 of a
    /// socketpair write end) and return the read end for the display-number
    /// line.
    fn spawn_xvfb(&mut self) -> Result<UnixStream, ComputerUseError> {
        let (read_end, write_end) = UnixStream::pair().map_err(|error| {
            ComputerUseError::failed(format!(
                "cannot create the Xvfb displayfd socketpair: {error}"
            ))
        })?;
        // Spawn the RESOLVED probe path, never a bare name: the first PATH
        // hit the probe verified is the server that runs (see
        // find_on_path_full). A vanished binary answers explicitly instead
        // of falling through to exec's PATH search.
        let xvfb_path = find_on_path_full("Xvfb").ok_or_else(|| {
            ComputerUseError::unavailable(
                missing_required_binaries(find_on_path).unwrap_or_else(|| {
                    "Xvfb is not on PATH; install the `xvfb` package".to_string()
                }),
            )
        })?;
        let mut command = Command::new(&xvfb_path);
        command
            .args([
                "-displayfd",
                &DISPLAYFD.to_string(),
                "-screen",
                "0",
                SCREEN_GEOMETRY,
                "-dpi",
                &SCREEN_DPI.to_string(),
                "-noreset",
                "-nolisten",
                "tcp",
                "-auth",
            ])
            .arg(&self.auth_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        std_process_group_leader(&mut command);
        let write_fd = write_end.as_raw_fd();
        // SAFETY: runs between fork and exec in the Xvfb child only. dup2
        // copies the socketpair write end onto fd 3; the explicit F_SETFD
        // clear covers the oldfd==newfd case (dup2 keeps CLOEXEC there) and
        // is a no-op otherwise, so fd 3 survives the exec.
        unsafe {
            command.pre_exec(move || {
                if libc::dup2(write_fd, DISPLAYFD) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                let flags = libc::fcntl(DISPLAYFD, libc::F_GETFD);
                if flags != -1 {
                    let _ = libc::fcntl(DISPLAYFD, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
                }
                Ok(())
            });
        }
        let child = command
            .spawn()
            .map_err(|error| ComputerUseError::failed(format!("cannot spawn Xvfb: {error}")))?;
        self.xvfb = Some(child);
        // The child owns fd 3 now; the parent write end closes when this
        // function returns, so the display-number read sees prompt EOF if
        // the server dies before writing.
        Ok(read_end)
    }

    /// Poll an x11rb connection until the server answers or the deadline
    /// passes. A timeout is `Unavailable` so the creation-time caller sees a
    /// fallback-able verdict instead of a generic failure.
    fn wait_ready(&self) -> Result<(), ComputerUseError> {
        let deadline = Instant::now() + READY_TIMEOUT;
        let mut last_error = String::from("no connection attempt finished");
        while Instant::now() < deadline {
            match self.try_connect() {
                Ok(_) => return Ok(()),
                Err(error) => {
                    last_error = error.to_string();
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
        Err(ComputerUseError::unavailable(format!(
            "the isolated virtual display ({SCREEN_GEOMETRY}) did not become ready \
             within {}s: {last_error}",
            READY_TIMEOUT.as_secs()
        )))
    }

    /// Spawn the optional WM into the display (best-effort; a failure is
    /// logged and the session continues WM-less). The candidate name is
    /// resolved through the probe PATH walk first (probe-what-you-spawn).
    fn spawn_wm(&mut self) {
        let Some(program) = self.wm_program else {
            return;
        };
        // A lazy respawn leaves the previous WM handle behind, pointing at
        // the dead server. Kill and reap it BEFORE the overwrite: a bare
        // `self.wm = Some(child)` would drop the old Child un-waited and leak
        // it as a zombie (MINOR-2).
        if let Some(mut old) = self.wm.take() {
            kill_pid_tree(old.id());
            let _ = old.wait_timeout(REAP_GRACE);
        }
        let Some(wm_path) = find_on_path_full(program) else {
            log::warn!(
                "[computer_use] virtual display: window manager '{program}' was found at \
                 probe time but is gone from PATH; continuing without a WM"
            );
            self.wm_program = None;
            return;
        };
        let mut command = Command::new(&wm_path);
        command
            .env("DISPLAY", &self.display)
            .env("XAUTHORITY", &self.auth_path)
            // Keep GTK-based WMs (mutter/marco/xfwm4) on the X11 backend.
            .env("GDK_BACKEND", "x11")
            // Same session-bus discipline as opened apps (apply_open_env): a
            // D-Bus-heavy WM (mutter is the first candidate) must not query
            // or own names on the USER'S real session bus. EWMH window
            // management itself is X11-property based and needs no session
            // bus; WM features that do degrade on a dead address are the
            // disclosed trade (isolation holds either way).
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path={}", self.session_dir.join("bus").display()),
            )
            // Same env discipline as xdotool/opened apps: the WM must not
            // pick up the host's Wayland session.
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("WAYLAND_SOCKET")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        std_process_group_leader(&mut command);
        match command.spawn() {
            Ok(child) => {
                self.wm = Some(child);
                // The WM is recorded ONLY via `wm_pid`/the held handle — never
                // in `children`: the release path winds the children down
                // while the WM must stay alive with the display (trait
                // contract: a re-granted session keeps a managed display).
                log::info!(
                    "[computer_use] virtual display: window manager '{program}' started on {}",
                    self.display
                );
            }
            Err(error) => {
                log::warn!(
                    "[computer_use] virtual display: window manager '{program}' failed to start \
                     ({error}); continuing without a WM"
                );
            }
        }
    }

    /// Record one `open_app` child (pidfile rewrite). The identity captured
    /// here (program basename + kernel start time) is what lets every kill
    /// path verify the pid before firing — a recycled pid number matches
    /// neither and is skipped instead of killed.
    fn record_child(&mut self, program: &str, pid: u32) {
        let record = ChildRecord {
            pid,
            program: basename_of(program).to_string(),
            starttime: process_starttime(pid).unwrap_or(0),
        };
        self.children.push(record);
        self.write_pidfile();
    }

    /// Drop recorded children that can no longer be verified (their /proc
    /// entry is gone or a zombie — dead, merely awaiting its detached reaper;
    /// or the pid number was recycled and no longer matches the recorded
    /// identity — unverifiable, hence unsafely killable). A server respawn
    /// implicitly kills every X client of the dead server; pruning keeps the
    /// pidfile's children list truthful. Called on the respawn path only (at
    /// creation the list is empty).
    fn prune_dead_children(&mut self) {
        let before = self.children.len();
        self.children.retain(child_identity_matches);
        if self.children.len() != before {
            log::info!(
                "[computer_use] virtual display: pruned {} dead/unverifiable child \
                 record(s) after the server restart",
                before - self.children.len()
            );
        }
    }

    /// Serialize the current process state into `<session dir>/pidfile.json`.
    /// Best-effort everywhere: a pidfile write failure must not kill a working
    /// session (the crash-sweep record is lost for THIS session only, and the
    /// warning makes the loss visible). The `xvfb_pid = 0` shape never reaches
    /// disk in practice: every write happens after a successful Xvfb spawn.
    fn write_pidfile(&self) {
        let file = PidFile {
            xvfb_pid: self.xvfb.as_ref().map(Child::id).unwrap_or(0),
            wm_pid: self.wm.as_ref().map(Child::id),
            display: self.display.clone(),
            auth_path: self.auth_path.display().to_string(),
            children: self.children.iter().map(|r| r.pid).collect(),
            children_detail: self.children.clone(),
            started_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|age| age.as_secs())
                .unwrap_or(0),
        };
        let write = || -> Result<(), ComputerUseError> {
            let path = self.session_dir.join(PIDFILE_NAME);
            let json = serde_json::to_string(&file).map_err(|error| {
                ComputerUseError::failed(format!("cannot serialize the pidfile: {error}"))
            })?;
            // Temp + rename within the same directory: a truncate-in-place
            // write that dies midway (crash, ENOSPC) would leave a malformed
            // pidfile the crash sweep cannot parse — exactly the record the
            // file exists to preserve. rename(2) is atomic on one filesystem.
            let tmp = self.session_dir.join(format!("{PIDFILE_NAME}.tmp"));
            std::fs::write(&tmp, json.as_bytes()).map_err(|error| {
                ComputerUseError::failed(format!("cannot write {}: {error}", tmp.display()))
            })?;
            std::fs::rename(&tmp, &path).map_err(|error| {
                let _ = std::fs::remove_file(&tmp);
                ComputerUseError::failed(format!("cannot write {}: {error}", path.display()))
            })
        };
        if let Err(error) = write() {
            log::warn!(
                "[computer_use] virtual display: cannot update the pidfile (the crash \
                 sweep will not know this session): {error}"
            );
        }
    }

    /// Write the 0600 Xauthority file for `number` ("" = placeholder).
    fn write_xauthority(&self, number: &str) -> Result<(), ComputerUseError> {
        let entry = build_xauthority_entry(
            FAMILY_LOCAL,
            self.hostname.as_bytes(),
            number.as_bytes(),
            AUTH_PROTOCOL_NAME,
            &self.cookie,
        );
        std::fs::write(&self.auth_path, entry).map_err(|error| {
            ComputerUseError::failed(format!(
                "cannot write the virtual display Xauthority file: {error}"
            ))
        })?;
        std::fs::set_permissions(&self.auth_path, std::fs::Permissions::from_mode(0o600)).map_err(
            |error| {
                ComputerUseError::failed(format!(
                    "cannot restrict the virtual display Xauthority file: {error}"
                ))
            },
        )
    }

    /// One x11rb connection to the virtual display with our cookie. Fresh
    /// connection per call: nothing to invalidate when the server restarts.
    fn try_connect(&self) -> Result<(RustConnection, usize), ComputerUseError> {
        let parsed = parse_display(Some(&self.display)).map_err(|error| {
            ComputerUseError::failed(format!("cannot parse the virtual display string: {error}"))
        })?;
        let screen = usize::from(parsed.screen);
        let mut last_error = None;
        for address in parsed.connect_instruction() {
            match DefaultStream::connect(&address) {
                Ok((stream, _peer)) => {
                    let connection = RustConnection::connect_to_stream_with_auth_info(
                        stream,
                        screen,
                        AUTH_PROTOCOL_NAME.to_vec(),
                        self.cookie.to_vec(),
                    )
                    .map_err(|error| {
                        ComputerUseError::failed(format!(
                            "the virtual display server rejected the connection: {error}"
                        ))
                    })?;
                    return Ok((connection, screen));
                }
                Err(error) => last_error = Some(error),
            }
        }
        Err(ComputerUseError::failed(format!(
            "cannot reach the virtual display server on {}: {}",
            self.display,
            last_error
                .map(|error| error.to_string())
                .unwrap_or_else(|| "no connect address".to_string())
        )))
    }

    /// Cheap server liveness probe: `try_wait` on the held Xvfb child (reaps
    /// the zombie as a side effect). An errored wait counts as dead.
    fn server_alive(&mut self) -> bool {
        match self.xvfb.as_mut() {
            Some(child) => !matches!(child.try_wait(), Ok(Some(_)) | Err(_)),
            None => false,
        }
    }

    /// Entry check for every action: when the server died since the last
    /// action, attempt ONE lazy respawn with the stored spawn parameters
    /// (§5: at most one attempt per call; a failed respawn answers
    /// `Unavailable` with the reason).
    fn ensure_server(&mut self) -> Result<(), ComputerUseError> {
        if self.server_alive() {
            return Ok(());
        }
        log::warn!(
            "[computer_use] virtual display: the Xvfb server exited; attempting one restart"
        );
        self.spawn_stack().map_err(|error| {
            ComputerUseError::unavailable(format!(
                "the isolated virtual display could not be restarted: {error}; \
                 the virtual target is unavailable for this action — check that the \
                 `xvfb` package works on this machine and retry"
            ))
        })
    }

    /// Wrap an action failure: when the Xvfb process is gone, the failure IS
    /// the server death — answer the explicit `Failed` contract (the next
    /// action performs the one lazy respawn).
    fn classify_failure(&mut self, error: ComputerUseError) -> ComputerUseError {
        if !self.server_alive() {
            return ComputerUseError::failed(format!(
                "{error}; the isolated virtual display server (Xvfb) has exited — \
                 the next action will attempt one automatic restart"
            ));
        }
        error
    }

    /// Kill everything recorded in the backend (children → WM → Xvfb), reap
    /// the held child handles, and clear the records. Child kills go through
    /// the pid-recycling identity gate (a number alone is not an identity);
    /// the WM/Xvfb kills are exempt (their pids were never released for
    /// recycling — they are still our held handles). The WM/Xvfb get a
    /// SIGTERM-first wind-down: an X server that exits on SIGTERM removes its
    /// own `/tmp/.X11-unix/X{n}` socket, which a SIGKILLed server cannot (the
    /// crash sweep — SIGKILL by necessity, other-process context — still
    /// strands that socket; see the module doc). SIGKILL follows only if the
    /// grace expires. All kills go through kill(2) (`kill_pid_tree` —
    /// group-wide, ESRCH-safe) — never an external kill binary (architecture
    /// guard).
    fn kill_recorded_processes(&mut self) {
        let children = std::mem::take(&mut self.children);
        kill_verified_children(&children);
        if let Some(mut wm) = self.wm.take() {
            terminate_then_kill(&mut wm, TEARDOWN_TERM_GRACE);
        }
        if let Some(mut xvfb) = self.xvfb.take() {
            terminate_then_kill(&mut xvfb, TEARDOWN_TERM_GRACE);
        }
        self.display = String::new();
        self.display_number = None;
    }

    /// Run one bounded xdotool call against the virtual display. Never
    /// `.output()` unbounded (design §5); the command is a process group
    /// leader so a timeout kills the whole tree.
    fn run_xdotool(&self, args: &[String], timeout: Duration) -> Result<String, ComputerUseError> {
        // The RESOLVED path stored at creation — the binary that answered the
        // probe is the one that drives input (never a PATH substitution).
        let mut command = Command::new(&self.xdotool_path);
        command.args(args);
        command
            .env("DISPLAY", &self.display)
            .env("XAUTHORITY", &self.auth_path)
            // A host Wayland session must not leak into the virtual target.
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("WAYLAND_SOCKET");
        let display = self.display.clone();
        let output = output_with_timeout(command, timeout).map_err(|error| {
            ComputerUseError::failed(format!("xdotool on {display} failed: {error}"))
        })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr: String = stderr.chars().take(STDERR_EXCERPT_CHARS).collect();
            // The error text lands verbatim in the audit JSONL: a `type`
            // invocation carries the typed text (possibly a secret), so the
            // argv is rendered with that argument redacted.
            let args = render_xdotool_args_for_error(args);
            return Err(ComputerUseError::failed(format!(
                "xdotool {args} on {display} exited with status {}: {stderr}",
                output.status.code().unwrap_or(-1)
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// Render the xdotool argv for an error message. Execution errors land
/// verbatim in the audit JSONL error field, so for `type` invocations the
/// text argument (potentially a typed secret, one call at a time) is replaced
/// by a length-only marker (`<N chars redacted>`); every other command keeps
/// its args visible (coordinates, button numbers, keysyms — nothing
/// sensitive). Pure so the redaction shape is unit-pinned without a display.
fn render_xdotool_args_for_error(args: &[String]) -> String {
    if args.first().map(String::as_str) != Some("type") {
        return args.join(" ");
    }
    let text_chars: usize = match args.iter().position(|arg| arg == "--") {
        Some(index) => args[index + 1..]
            .iter()
            .map(|arg| arg.chars().count())
            .sum(),
        // Defensive: our `type` shape always carries `--`; without it the
        // whole tail after the subcommand is treated as the text.
        None => args[1..].iter().map(|arg| arg.chars().count()).sum(),
    };
    let head_len = args.iter().position(|arg| arg == "--").map_or(1, |i| i + 1);
    let marker = format!("<{text_chars} chars redacted>");
    let mut parts: Vec<&str> = args[..head_len].iter().map(String::as_str).collect();
    parts.push(&marker);
    parts.join(" ")
}

/// The backend dies: kill everything recorded, then remove the session dir.
impl Drop for VirtualX11Backend {
    fn drop(&mut self) {
        // Teardown ordering (design §5): children → WM → Xvfb, and the
        // session dir (which contains the pidfile) is removed only AFTER
        // every kill is attempted — a crash mid-teardown must leave the
        // pidfile behind so sweep_orphans can finish the job.
        self.kill_recorded_processes();
        let _ = std::fs::remove_dir_all(&self.session_dir);
    }
}

// ---------------------------------------------------------------------------
// Pure helpers (unit-pinned below)
// ---------------------------------------------------------------------------

/// Assemble the child environment for `open_app`: pin the virtual display and
/// force every toolkit to its X11 backend, REMOVE the host Wayland session
/// variables so a GUI app cannot escape onto the user's session, and REDIRECT
/// the D-Bus session bus into the session dir. Everything else is inherited.
/// Pure function of (command, display, auth, session_dir) so the exact key
/// set is unit-pinned.
fn apply_open_env(command: &mut Command, display: &str, auth_path: &Path, session_dir: &Path) {
    command.env("DISPLAY", display);
    command.env("XAUTHORITY", auth_path);
    command.env("GDK_BACKEND", "x11");
    command.env("QT_QPA_PLATFORM", "xcb");
    command.env("SDL_VIDEODRIVER", "x11");
    command.env("OZONE_PLATFORM", "x11");
    command.env("ELECTRON_OZONE_PLATFORM_HINT", "x11");
    command.env("LIBGL_ALWAYS_SOFTWARE", "1");
    // Session-bus redirection: without this, children inherit the app host's
    // DBUS_SESSION_BUS_ADDRESS (or resolve it via XDG_RUNTIME_DIR) and the
    // first `notify-send` from an opened app pops a notification on the
    // USER'S real desktop — breaking the isolation promise. The pointed-to
    // socket does not exist at launch, and libdbus/GDBus do NOT autolaunch a
    // private bus for an explicitly-set-but-unusable address (autolaunch
    // applies only when the address is UNKNOWN) — so session-bus-dependent
    // functionality of opened apps fails rather than getting a private bus.
    // That is the deliberate trade: isolation holds either way (nothing
    // reaches the user's real bus). XDG_RUNTIME_DIR is deliberately KEPT
    // (apps need it, e.g. for their own runtime state); the services it
    // hosts that have no env-var redirection (audio via the runtime-dir
    // sockets) are disclosed in the capabilities notes instead.
    command.env(
        "DBUS_SESSION_BUS_ADDRESS",
        format!("unix:path={}", session_dir.join("bus").display()),
    );
    // Wayland escape hatches: removing the two env vars is NOT sufficient by
    // itself — a raw libwayland client with no WAYLAND_DISPLAY falls back to
    // the DEFAULT socket name (`wayland-0`) inside XDG_RUNTIME_DIR, i.e. the
    // user's compositor. The toolkit overrides above (GDK/QT/SDL/OZONE)
    // cover the mainstream stacks; a raw-libwayland app could still land a
    // window on the user's desktop. Disclosed in the capabilities notes.
    command.env_remove("WAYLAND_DISPLAY");
    command.env_remove("WAYLAND_SOCKET");
}

/// `open_app` program validation, mirroring the tool-layer bounds
/// (tool.rs `parse_action` "open"): non-empty and NUL-free (NUL cannot appear
/// in an argv entry on unix; spawn would fail with an obscure libc error).
fn validate_open_program(program: &str) -> Result<(), ComputerUseError> {
    if program.is_empty() {
        return Err(ComputerUseError::failed(
            "open: program must be a non-empty string (a bare name or an absolute path; \
             it is spawned directly, without a shell)",
        ));
    }
    if program.contains('\0') {
        return Err(ComputerUseError::failed(
            "open: program must not contain NUL characters",
        ));
    }
    Ok(())
}

/// `open_app` args validation (same NUL rule; length/count bounds are the
/// tool layer's job — the backend only rejects what cannot spawn).
fn validate_open_args(args: &[String]) -> Result<(), ComputerUseError> {
    if args.iter().any(|arg| arg.contains('\0')) {
        return Err(ComputerUseError::failed(
            "open: args must not contain NUL characters",
        ));
    }
    Ok(())
}

/// The `Key` → xdotool keysym-name table. xdotool resolves each token via
/// `XStringToKeysym`, so this is the X keysym NAME (e.g. `Prior`, not
/// `PageUp`). Exhaustive over [`Key`] with NO wildcard arm: adding a variant
/// must fail to compile here until the mapping is written.
fn keysym_name(key: Key) -> Result<String, ComputerUseError> {
    let name: String = match key {
        Key::Control => "ctrl".to_string(),
        Key::Alt => "alt".to_string(),
        Key::Shift => "shift".to_string(),
        // macOS Cmd / Windows Win / Linux Super (same target as the Wayland
        // portal's 0xffeb Super_L).
        Key::Meta => "super".to_string(),
        Key::Enter => "Return".to_string(),
        Key::Escape => "Escape".to_string(),
        Key::Tab => "Tab".to_string(),
        Key::Space => "space".to_string(),
        Key::Backspace => "BackSpace".to_string(),
        Key::Delete => "Delete".to_string(),
        Key::Insert => "Insert".to_string(),
        Key::Up => "Up".to_string(),
        Key::Down => "Down".to_string(),
        Key::Left => "Left".to_string(),
        Key::Right => "Right".to_string(),
        Key::Home => "Home".to_string(),
        Key::End => "End".to_string(),
        Key::PageUp => "Prior".to_string(),
        Key::PageDown => "Next".to_string(),
        Key::Function(n) => match n {
            1..=12 => format!("F{n}"),
            _ => {
                return Err(ComputerUseError::unsupported(
                    "input",
                    format!("function key F{n} is out of the mappable range F1-F12"),
                ));
            }
        },
        Key::Char(c) => char_keysym_name(c)?,
    };
    Ok(name)
}

/// Keysym names for single-character keys. xdotool resolves letters/digits by
/// their own name; punctuation has dedicated keysym names. Unsupported
/// characters fail CLOSED without echoing the character: execution errors land
/// verbatim in the audit JSONL error field, so a secret typed one call at a
/// time must not leak through the error text (same rule as the Wayland path).
fn char_keysym_name(c: char) -> Result<String, ComputerUseError> {
    // Letters and digits are valid keysym names in their own right.
    if matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9') {
        return Ok(c.to_string());
    }
    let name = match c {
        '+' => "plus",
        '-' => "minus",
        ',' => "comma",
        '.' => "period",
        '/' => "slash",
        '?' => "question",
        '!' => "exclam",
        '@' => "at",
        '#' => "numbersign",
        '$' => "dollar",
        '%' => "percent",
        '^' => "asciicircum",
        '&' => "ampersand",
        '*' => "asterisk",
        '(' => "parenleft",
        ')' => "parenright",
        '[' => "bracketleft",
        ']' => "bracketright",
        '{' => "braceleft",
        '}' => "braceright",
        '<' => "less",
        '>' => "greater",
        '=' => "equal",
        ':' => "colon",
        ';' => "semicolon",
        '\'' => "apostrophe",
        '"' => "quotedbl",
        '`' => "grave",
        '~' => "asciitilde",
        '\\' => "backslash",
        '|' => "bar",
        '_' => "underscore",
        _ => {
            return Err(ComputerUseError::unsupported(
                "input",
                "this key character has no injectable keysym on the virtual display \
                 (printable ASCII only); use the type action for text input",
            ));
        }
    };
    Ok(name.to_string())
}

/// Build the `xdotool key` / `keydown` argument for a chord: modifiers first
/// (stable order), then the rest, joined with `+` (the form `key` interprets
/// as one combo).
fn format_chord(keys: &[Key]) -> Result<String, ComputerUseError> {
    let mut names = Vec::with_capacity(keys.len());
    for key in keys {
        names.push(keysym_name(*key)?);
    }
    let (modifiers, rest): (Vec<String>, Vec<String>) = names
        .into_iter()
        .partition(|name| matches!(name.as_str(), "ctrl" | "alt" | "shift" | "super"));
    let mut ordered = modifiers;
    ordered.extend(rest);
    Ok(ordered.join("+"))
}

/// Fail-closed charset gate for one already-split text chunk of `type_text`:
/// xdotool resolves each typed character via `XStringToKeysym`, which only
/// knows keysym NAMES — non-ASCII characters would be silently dropped
/// ("fake success"), so they are refused up front (same rationale as the
/// Wayland fail-closed path). Only [`TypeRun::Text`] chunks reach here, and
/// only AFTER `split_type_runs`: newlines/tabs are lifted into explicit
/// Return/Tab key clicks by the split and never see this gate.
fn validate_type_text(text: &str) -> Result<(), ComputerUseError> {
    if text.chars().all(|c| matches!(c, ' '..='~')) {
        return Ok(());
    }
    Err(ComputerUseError::unsupported(
        "input",
        "cannot type this text on the virtual display: xdotool silently drops characters \
         outside printable ASCII, so non-ASCII text would be lost; type ASCII text",
    ))
}

/// The `type_text` acceptance path, pure so the gate ORDER is unit-pinned:
/// split FIRST (newlines/tabs become explicit Return/Tab key-click runs),
/// THEN validate only the [`TypeRun::Text`] chunks against the printable-ASCII
/// gate. Validating the whole text before the split rejected every multi-line
/// input up front, leaving the Return/Tab arms of the executor unreachable —
/// the docs and tests claimed the opposite order.
fn plan_type_runs(text: &str) -> Result<Vec<TypeRun>, ComputerUseError> {
    // Fold CRLF/CR to '\n' first (shared helper, same reason as the physical
    // paths): a raw CR would submit twice.
    let text = normalize_typed_newlines(text);
    let runs = split_type_runs(&text, TYPE_CHUNK_CHARS);
    for run in &runs {
        if let TypeRun::Text(chunk) = run {
            validate_type_text(chunk)?;
        }
    }
    Ok(runs)
}

/// Convert one ZPixmap depth-24 GetImage reply (LSBFirst = BGRX bytes per
/// pixel) into RGBA with opaque alpha.
fn bgrx_to_rgba(data: &[u8], width: u32, height: u32) -> Result<Vec<u8>, ComputerUseError> {
    let stride = width as usize * 4;
    let expected = stride.checked_mul(height as usize).ok_or_else(|| {
        ComputerUseError::failed("virtual display capture size overflows the address space")
    })?;
    if data.len() < expected {
        return Err(ComputerUseError::failed(format!(
            "virtual display capture is truncated: got {} bytes, expected {expected}",
            data.len()
        )));
    }
    let mut out = vec![0u8; expected];
    for y in 0..height as usize {
        let src = &data[y * stride..(y + 1) * stride];
        let dst = &mut out[y * stride..(y + 1) * stride];
        for (pixel, chunk) in src.chunks_exact(4).enumerate() {
            dst[pixel * 4] = chunk[2];
            dst[pixel * 4 + 1] = chunk[1];
            dst[pixel * 4 + 2] = chunk[0];
            dst[pixel * 4 + 3] = 255;
        }
    }
    Ok(out)
}

/// Read the display number Xvfb prints on the displayfd (`"%d\n"` per the
/// Xserver manual; the leading `:` form is accepted defensively). Bounded by
/// [`DISPLAYFD_DEADLINE`]: Xvfb writes the number once its sockets are set up.
fn read_display_number(mut stream: &UnixStream) -> Result<u16, ComputerUseError> {
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(|error| {
            ComputerUseError::failed(format!("cannot arm the displayfd read timeout: {error}"))
        })?;
    let deadline = Instant::now() + DISPLAYFD_DEADLINE;
    let mut buffer = Vec::new();
    let mut chunk = [0u8; DISPLAYFD_CHUNK];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => {
                if buffer.contains(&b'\n') {
                    break;
                }
                return Err(ComputerUseError::unavailable(
                    "the Xvfb process closed the display fd before reporting a display \
                     number (it may have failed to start)",
                ));
            }
            Ok(n) => {
                buffer.extend_from_slice(&chunk[..n]);
                if buffer.contains(&b'\n') {
                    break;
                }
            }
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                if Instant::now() >= deadline {
                    return Err(ComputerUseError::unavailable(format!(
                        "the Xvfb process did not report a display number within {}s",
                        DISPLAYFD_DEADLINE.as_secs()
                    )));
                }
            }
            Err(error) => {
                return Err(ComputerUseError::failed(format!(
                    "cannot read the Xvfb display number: {error}"
                )));
            }
        }
    }
    let text = String::from_utf8_lossy(&buffer);
    let number = text.trim().trim_start_matches(':');
    number.parse::<u16>().map_err(|_| {
        ComputerUseError::failed(format!(
            "the Xvfb displayfd output is not a display number: {:?}",
            text.chars().take(STDERR_EXCERPT_CHARS).collect::<String>()
        ))
    })
}

/// Create `~/.pinvou3/computer-use/vd/<uniq>/` with 0700.
fn fresh_session_dir() -> Result<PathBuf, ComputerUseError> {
    let mut root = pinvou3_home();
    for part in VD_ROOT_SUBPATH {
        root.push(part);
    }
    std::fs::create_dir_all(&root).map_err(|error| {
        ComputerUseError::failed(format!("cannot create {}: {error}", root.display()))
    })?;
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|age| age.as_millis())
        .unwrap_or(0);
    for _ in 0..16 {
        let counter = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = root.join(format!("vd-{millis}-{counter:04x}"));
        match std::fs::create_dir(&dir) {
            Ok(()) => {
                try_make_private_dir(&dir).map_err(|error| {
                    ComputerUseError::failed(format!(
                        "cannot restrict the virtual display session dir: {error}"
                    ))
                })?;
                return Ok(dir);
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(ComputerUseError::failed(format!(
                    "cannot create the virtual display session dir: {error}"
                )));
            }
        }
    }
    Err(ComputerUseError::failed(
        "cannot create the virtual display session dir: name collisions persisted \
         (the clock is not advancing?)",
    ))
}

// ---------------------------------------------------------------------------
// Backend trait implementation
// ---------------------------------------------------------------------------

impl ComputerUseBackend for VirtualX11Backend {
    fn capabilities(&self) -> Capabilities {
        let mut notes = Vec::new();
        if self.wm_program.is_none() {
            notes.push(format!(
                "no window manager on the virtual display (candidates: {}): applications \
                 appear without decorations and are not placed or managed",
                WM_CANDIDATES.join(", ")
            ));
        }
        notes.push(
            "ui_tree and element actions are unavailable: the accessibility stack does \
             not target the virtual display; use screenshots and coordinates"
                .to_string(),
        );
        // Isolation disclosures (truthful residuals): Stop/revoke winds down
        // the apps opened on the display (the display itself stays up); the
        // env scrub cannot cover services whose address has no env-var
        // redirection (audio via the user's runtime-dir sockets); the D-Bus
        // session bus is DISABLED for opened apps (an explicitly-set dead
        // address does not autolaunch a private bus — bus-dependent features
        // fail); and a raw-libwayland client could still escape through the
        // default `wayland-0` socket in the kept XDG_RUNTIME_DIR — stated,
        // not hidden.
        notes.push(
            "stopping the session (Stop button, grant revoke, master switch off) winds \
             down the apps opened on the virtual display; apps opened there may still \
             reach shared session services of the user's login session that the \
             environment cannot redirect (e.g. audio via the runtime-dir sockets), \
             their D-Bus session bus is disconnected (bus-dependent features fail \
             rather than getting a private bus), and an app speaking raw Wayland \
             (beyond the redirected toolkits) could still reach the user's \
             compositor through the default runtime-dir socket"
                .to_string(),
        );
        Capabilities {
            screenshot: true,
            input: true,
            ui_tree: false,
            notes: notes.join("; "),
            target: DisplayTarget::Virtual,
            target_note: format!(
                "isolated virtual display (Xvfb {}): the user's desktop is unaffected — \
                 screenshots, input and apps (via open) all stay inside the virtual \
                 screen; stopping the session closes the apps opened on it, and those \
                 apps run with normal user file and network access and may reach \
                 shared session services (e.g. audio) unless their environment is \
                 scrubbed",
                if self.display.is_empty() {
                    "screen"
                } else {
                    &self.display
                }
            ),
        }
    }

    fn capture(&mut self) -> Result<Capture, ComputerUseError> {
        self.ensure_server()?;
        self.capture_inner()
            .map_err(|error| self.classify_failure(error))
    }

    fn cursor_position(&mut self) -> Result<(i32, i32), ComputerUseError> {
        self.ensure_server()?;
        let result = self.cursor_position_inner();
        result.map_err(|error| self.classify_failure(error))
    }

    fn move_to(&mut self, x: i32, y: i32) -> Result<(), ComputerUseError> {
        self.ensure_server()?;
        let result = self
            .run_xdotool(
                &[
                    "mousemove".to_string(),
                    "--sync".to_string(),
                    x.to_string(),
                    y.to_string(),
                ],
                INPUT_TIMEOUT,
            )
            .map(|_| ());
        result.map_err(|error| self.classify_failure(error))
    }

    fn click(&mut self, button: MouseButton, count: u8) -> Result<(), ComputerUseError> {
        self.ensure_server()?;
        // count: 1 = single, 2 = double, 3 = triple (trait contract).
        let repeat = count.max(1).to_string();
        let result = self
            .run_xdotool(
                &[
                    "click".to_string(),
                    "--repeat".to_string(),
                    repeat,
                    "--delay".to_string(),
                    MULTI_CLICK_DELAY_MS.to_string(),
                    xdotool_button(button).to_string(),
                ],
                INPUT_TIMEOUT,
            )
            .map(|_| ());
        result.map_err(|error| self.classify_failure(error))
    }

    fn mouse_down(&mut self, button: MouseButton) -> Result<(), ComputerUseError> {
        self.ensure_server()?;
        let result = self
            .run_xdotool(
                &["mousedown".to_string(), xdotool_button(button).to_string()],
                INPUT_TIMEOUT,
            )
            .map(|_| ());
        result.map_err(|error| self.classify_failure(error))
    }

    fn mouse_up(&mut self, button: MouseButton) -> Result<(), ComputerUseError> {
        self.ensure_server()?;
        let result = self
            .run_xdotool(
                &["mouseup".to_string(), xdotool_button(button).to_string()],
                INPUT_TIMEOUT,
            )
            .map(|_| ());
        result.map_err(|error| self.classify_failure(error))
    }

    fn drag(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<(), ComputerUseError> {
        self.ensure_server()?;
        self.move_to(from.0, from.1)?;
        self.run_xdotool(&["mousedown".to_string(), "1".to_string()], INPUT_TIMEOUT)
            .map(|_| ())?;
        // Let the press land before the interpolated motion starts (shared
        // settle budget, same shape as the other backends).
        std::thread::sleep(Duration::from_millis(DRAG_PRESS_SETTLE_MS));
        // The button must be released at the end no matter what happened
        // midway (same guarantee as the physical backends).
        let cancel = self.cancel.clone();
        let mut result = Ok(());
        for (x, y) in drag_waypoints(from, to, DRAG_STEPS) {
            if cancel
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::SeqCst))
            {
                result = Err(ComputerUseError::unavailable(
                    "drag was cancelled (caller timeout or stop); the button is \
                     released and the pointer stays at the last waypoint",
                ));
                break;
            }
            if let Err(error) = self.run_xdotool(
                &[
                    "mousemove".to_string(),
                    "--sync".to_string(),
                    x.to_string(),
                    y.to_string(),
                ],
                INPUT_TIMEOUT,
            ) {
                result = Err(error);
                break;
            }
            std::thread::sleep(Duration::from_millis(DRAG_STEP_DELAY_MS));
        }
        let release = self
            .run_xdotool(&["mouseup".to_string(), "1".to_string()], INPUT_TIMEOUT)
            .map(|_| ());
        combine_drag_errors(result, release).map_err(|error| self.classify_failure(error))
    }

    fn scroll(&mut self, direction: ScrollDirection, clicks: u32) -> Result<(), ComputerUseError> {
        self.ensure_server()?;
        // amount 0 = no-op (a spin of the wheel by zero detents must not cost
        // a subprocess; same convention as the physical backends).
        if clicks == 0 {
            return Ok(());
        }
        let clicks = clicks.min(MAX_SCROLL_CLICKS);
        let result = self
            .run_xdotool(
                &[
                    "click".to_string(),
                    "--repeat".to_string(),
                    clicks.to_string(),
                    "--delay".to_string(),
                    MULTI_CLICK_DELAY_MS.to_string(),
                    scroll_button(direction).to_string(),
                ],
                INPUT_TIMEOUT,
            )
            .map(|_| ());
        result.map_err(|error| self.classify_failure(error))
    }

    fn type_text(&mut self, text: &str) -> Result<(), ComputerUseError> {
        self.ensure_server()?;
        // Acceptance first (pure, unit-pinned order): split into chunked text
        // runs and explicit Return/Tab key clicks, then gate the text chunks
        // — multi-line text is admitted, non-ASCII text chunks still refused.
        let runs = plan_type_runs(text)?;
        let cancel = self.cancel.clone();
        let mut result = Ok(());
        for run in &runs {
            // Chunked injection: a request the caller already abandoned stops
            // at the next chunk boundary (upper bound of zombie injection =
            // one ≤200-char chunk).
            if cancel
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::SeqCst))
            {
                result = Err(ComputerUseError::unavailable(
                    "type was cancelled (caller timeout or stop); the text typed so \
                     far stays on the virtual display",
                ));
                break;
            }
            let call = match run {
                TypeRun::Text(chunk) => vec![
                    "type".to_string(),
                    "--delay".to_string(),
                    TYPE_DELAY_MS.to_string(),
                    "--".to_string(),
                    chunk.clone(),
                ],
                TypeRun::Return => vec!["key".to_string(), "Return".to_string()],
                TypeRun::Tab => vec!["key".to_string(), "Tab".to_string()],
            };
            if let Err(error) = self.run_xdotool(&call, TYPE_TIMEOUT) {
                result = Err(error);
                break;
            }
        }
        result.map_err(|error| self.classify_failure(error))
    }

    fn key_chord(&mut self, keys: &[Key]) -> Result<(), ComputerUseError> {
        self.ensure_server()?;
        let chord = format_chord(keys)?;
        let result = self
            .run_xdotool(&["key".to_string(), chord], INPUT_TIMEOUT)
            .map(|_| ());
        result.map_err(|error| self.classify_failure(error))
    }

    fn hold_key(&mut self, keys: &[Key], ms: u64) -> Result<(), ComputerUseError> {
        self.ensure_server()?;
        // keydown takes the keysyms as separate argv entries; keyup releases
        // them in reverse (enigo parity: press order = chord order).
        let mut names = Vec::with_capacity(keys.len());
        for key in keys {
            names.push(keysym_name(*key)?);
        }
        let keydown_args: Vec<String> = std::iter::once("keydown".to_string())
            .chain(names.iter().cloned())
            .collect();
        let mut released = names.clone();
        released.reverse();
        let keyup_args: Vec<String> = std::iter::once("keyup".to_string())
            .chain(released)
            .collect();
        self.run_xdotool(&keydown_args, INPUT_TIMEOUT)?;
        // Hold in 100ms cancel-checked slices (shared HOLD_CANCEL_POLL_MS);
        // the keys are ALWAYS released below.
        let cancel = self.cancel.clone();
        let mut cancelled = false;
        let mut remaining = ms;
        while remaining > 0 {
            if cancel
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::SeqCst))
            {
                cancelled = true;
                break;
            }
            let slice = remaining.min(HOLD_CANCEL_POLL_MS);
            std::thread::sleep(Duration::from_millis(slice));
            remaining -= slice;
        }
        let release = self.run_xdotool(&keyup_args, INPUT_TIMEOUT).map(|_| ());
        let hold_result = if cancelled {
            // The base verdict carries only the cancel fact; the combine below
            // appends the truthful release outcome.
            Err(ComputerUseError::unavailable(
                "hold_key was cancelled (caller timeout or stop)",
            ))
        } else {
            Ok(())
        };
        // Merge truthfully (held-key twin of the drag merge): a
        // `Result::and`-style merge kept the FIRST error, so a cancel plus a
        // failed release reported "the keys are released" while the release
        // had actually failed — stranded keys undisclosed. The combine keeps
        // both facts and replaces the all-clear with "the keys may still be
        // held down" when the release failed.
        combine_hold_errors(hold_result, release).map_err(|error| self.classify_failure(error))
    }

    fn ui_tree(&mut self, _opts: &UiTreeOptions) -> Result<String, ComputerUseError> {
        Err(ComputerUseError::unsupported(
            "ui_tree",
            "the isolated virtual display has no accessibility tree (AT-SPI does not \
             target it); use screenshots and coordinates",
        ))
    }

    fn element_at_point(
        &mut self,
        _x: i32,
        _y: i32,
    ) -> Result<Option<ElementInfo>, ComputerUseError> {
        Err(ComputerUseError::unsupported(
            "element_at_point",
            "the isolated virtual display has no accessibility tree (AT-SPI does not \
             target it); use screenshots and coordinates",
        ))
    }

    fn set_cancel_flag(&mut self, flag: Option<Arc<std::sync::atomic::AtomicBool>>) {
        self.cancel = flag;
    }

    /// Triggered by the command layer when the user revokes the grant / hits
    /// Stop / flips the master switch off. The virtual target has no
    /// OS-level authorization grant to revoke (all input stays inside our
    /// private X server behind our private cookie) — what Stop must undo is
    /// the apps the agent OPENED on the display: they are killed tree-wide
    /// here (`kill_pid_tree`), while Xvfb and the WM stay alive so the
    /// backend honors the trait contract (the next action — including a
    /// re-granted session — still works without any respawn). The WM is not
    /// in the recorded children list at all (only `wm_pid`), so a release
    /// cannot touch it; Xvfb/WM themselves are torn down only by Drop
    /// (backend destruction / session end).
    fn release_os_grant(&mut self) -> Result<(), ComputerUseError> {
        let children = std::mem::take(&mut self.children);
        let killed = kill_verified_children(&children);
        // Empty the recorded list in the pidfile: the pids are dead by our
        // own hand, so a crash sweep must never consider them (pid recycling).
        self.write_pidfile();
        log::info!(
            "[computer_use] virtual display: released the virtual grant — {killed} of {} \
             opened app(s) wound down; the display itself stays usable",
            children.len()
        );
        Ok(())
    }

    fn open_app(&mut self, program: &str, args: &[String]) -> Result<u32, ComputerUseError> {
        self.ensure_server()?;
        validate_open_program(program)?;
        validate_open_args(args)?;
        let mut command = Command::new(program);
        command.args(args);
        apply_open_env(
            &mut command,
            &self.display,
            &self.auth_path,
            &self.session_dir,
        );
        std_process_group_leader(&mut command);
        let mut child = command.spawn().map_err(|error| {
            ComputerUseError::failed(format!("open: cannot spawn {program}: {error}"))
        })?;
        let pid = child.id();
        // Detached reaper (the pidfile contract needs the pid, which
        // spawn_detached_and_reap does not return, so this mirrors its
        // reaper-thread pattern instead of calling it): ownership of the
        // Child moves into the thread, so the app is reaped whenever it exits
        // and the worker thread never blocks on it.
        if let Err(error) = std::thread::Builder::new()
            .name("cu-vd-app-reaper".to_string())
            .spawn(move || {
                let _ = child.wait();
            })
        {
            // Thread-spawn failure dropped the closure that owned the Child
            // handle: without this fallback the app pid lingers as an
            // unreaped zombie forever. Kill it and reap synchronously by pid.
            log::warn!(
                "[computer_use] virtual display: cannot spawn the open_app reaper thread \
                 ({error}); killing and reaping the app synchronously"
            );
            kill_pid_tree(pid);
            // SAFETY: waitpid reaps OUR child after the SIGKILL above; the
            // status pointer is null (ignored), no memory is touched.
            unsafe {
                libc::waitpid(pid as i32, std::ptr::null_mut(), 0);
            }
        }
        self.record_child(program, pid);
        log::info!(
            "[computer_use] virtual display: launched {program} on {} (pid {pid})",
            self.display
        );
        Ok(pid)
    }
}

impl VirtualX11Backend {
    /// Full-root GetImage on a fresh connection, BGRX → RGBA.
    fn capture_inner(&self) -> Result<Capture, ComputerUseError> {
        let (connection, screen) = self.try_connect()?;
        let setup = connection.setup();
        let screen_info = setup.roots.get(screen).ok_or_else(|| {
            ComputerUseError::failed(format!(
                "the virtual display server reported no screen {screen}"
            ))
        })?;
        let width = u32::from(screen_info.width_in_pixels);
        let height = u32::from(screen_info.height_in_pixels);
        let root = screen_info.root;
        // The byte-layout below assumes LSBFirst ZPixmap (BGRX per pixel);
        // MSBFirst X servers would produce reversed bytes — refuse instead of
        // returning wrong colors.
        if setup.image_byte_order != ImageOrder::LSB_FIRST {
            return Err(ComputerUseError::unsupported(
                "screenshot",
                "the virtual display server uses a big-endian image byte order; the \
                 capture path only implements little-endian ZPixmap",
            ));
        }
        if screen_info.root_depth != 24 {
            return Err(ComputerUseError::failed(format!(
                "the virtual display root window has depth {} (expected 24)",
                screen_info.root_depth
            )));
        }
        let reply = connection
            .get_image(
                ImageFormat::Z_PIXMAP,
                root,
                0,
                0,
                width as u16,
                height as u16,
                u32::MAX,
            )
            .map_err(|error| {
                ComputerUseError::failed(format!("virtual display GetImage failed: {error}"))
            })?
            .reply()
            .map_err(|error| {
                ComputerUseError::failed(format!("virtual display GetImage failed: {error}"))
            })?;
        let rgba = bgrx_to_rgba(&reply.data, width, height)?;
        Ok(Capture {
            rgba,
            width,
            height,
            origin_x: 0,
            origin_y: 0,
            input_scale_x: 1.0,
            input_scale_y: 1.0,
            input_aligned: true,
        })
    }

    fn cursor_position_inner(&self) -> Result<(i32, i32), ComputerUseError> {
        let output = self.run_xdotool(
            &["getmouselocation".to_string(), "--shell".to_string()],
            INPUT_TIMEOUT,
        )?;
        let mut x = None;
        let mut y = None;
        for line in output.lines() {
            if let Some(value) = line.strip_prefix("X=") {
                x = value.trim().parse::<i32>().ok();
            }
            if let Some(value) = line.strip_prefix("Y=") {
                y = value.trim().parse::<i32>().ok();
            }
        }
        match (x, y) {
            (Some(x), Some(y)) => Ok((x, y)),
            _ => Err(ComputerUseError::failed(
                "xdotool getmouselocation --shell did not report an X=/Y= position",
            )),
        }
    }
}

/// xdotool button numbers: left=1 middle=2 right=3 (X button mapping).
fn xdotool_button(button: MouseButton) -> &'static str {
    match button {
        MouseButton::Left => "1",
        MouseButton::Middle => "2",
        MouseButton::Right => "3",
    }
}

/// Wheel/horizontal scroll buttons: 4/5 = wheel up/down, 6/7 = wheel
/// left/right (classic X wheel button numbering).
fn scroll_button(direction: ScrollDirection) -> &'static str {
    match direction {
        ScrollDirection::Up => "4",
        ScrollDirection::Down => "5",
        ScrollDirection::Left => "6",
        ScrollDirection::Right => "7",
    }
}

// ---------------------------------------------------------------------------
// Creation + crash sweep (dispatch entry points)
// ---------------------------------------------------------------------------

/// Build the virtual backend (called by the shared dispatch after the one-time
/// target decision). Runs on the computer_use worker thread.
pub(crate) fn create_backend() -> Result<Box<dyn ComputerUseBackend>, ComputerUseError> {
    let backend = VirtualX11Backend::new()?;
    Ok(Box::new(backend))
}

/// True while `pid` still names a live process: the /proc entry exists and its
/// state is not `Z` (a zombie is dead, merely awaiting its detached reaper
/// thread). Linux-only file: this module compiles under `cfg(target_os = "linux")`.
fn process_runs(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // /proc/<pid>/stat: the comm field may contain spaces and parens, so
    // anchor on the LAST ')' — the field right after it is the state.
    let state_start = stat.rfind(')').map(|index| index + 2).unwrap_or(0);
    !stat[state_start..].starts_with('Z')
}

/// /proc/<pid>/stat field 22: the process start time in clock ticks after
/// boot. A pid number can be recycled; the start time cannot (it advances
/// monotonically with the kernel's boot time). None when the entry is gone
/// or unparsable.
fn process_starttime(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Fields AFTER comm: state(3) ppid(4) … starttime(22) — index 19 of the
    // whitespace split that starts at the state field.
    stat[stat.rfind(')').map(|index| index + 2).unwrap_or(0)..]
        .split_whitespace()
        .nth(19)
        .and_then(|field| field.parse::<u64>().ok())
}

/// File-name part of a program argument ("xmessage" stays "xmessage",
/// "/usr/bin/xmessage" becomes "xmessage") — the argv[0] identity the kill
/// gate compares. Splitting on '/' only: this module spawns plain binaries.
fn basename_of(program: &str) -> &str {
    program.rsplit('/').next().unwrap_or(program)
}

/// The pid-recycling identity gate for one recorded `open_app` child: the
/// pid must still run and its kernel start time must still match the
/// recorded one. The start time IS the identity — it advances monotonically
/// with boot time, so a recycled pid can never reproduce the recorded value,
/// while a child that exec'd into a different image (a shell's tail-exec, a
/// shebang re-exec, a wrapper script) keeps both pid and start time and must
/// still be wound down when the session ends. That exec shape is why the
/// recorded program name is a DIAGNOSTIC here, not a veto: `bash -c 'app'`
/// ends up running argv[0] `app` under the shell's recorded pid, and skipping
/// its kill because the basename changed would leave the app alive past Stop
/// (the capabilities notes promise wind-down). Killing by number alone could
/// take out an unrelated process — the start-time match is what makes the
/// kill safe (its whole group goes, via `kill_pid_tree`'s group kill).
fn child_identity_matches(record: &ChildRecord) -> bool {
    child_identity_matches_with(record, process_argv0)
}

/// The gate with an injected argv[0] lookup (the sweep's testable shape;
/// the start-time lookup always reads the real /proc).
fn child_identity_matches_with(
    record: &ChildRecord,
    argv0_of: impl Fn(u32) -> Option<String>,
) -> bool {
    if !process_runs(record.pid) {
        return false;
    }
    if process_starttime(record.pid) != Some(record.starttime) {
        return false;
    }
    // Diagnostic only (see the fn doc): a changed basename is logged so the
    // wind-down log stays explainable, never a reason to skip the kill.
    if let Some(argv0) = argv0_of(record.pid) {
        if basename_of(&argv0) != record.program {
            log::debug!(
                "[computer_use] virtual display: recorded child pid {} changed program \
                 ('{}' -> '{argv0}'); killing by its exact start-time identity",
                record.pid,
                record.program
            );
        }
    }
    true
}

/// Kill the recorded `open_app` children whose identity still verifies;
/// skip-and-log the rest (dead or recycled). Shared by the teardown
/// (`kill_recorded_processes`) and the release path so EVERY in-process
/// kill of a recorded child passes the same gate the crash sweep uses.
/// Returns the number of children actually killed (skipped records are
/// not counted, so the caller's wind-down log stays truthful).
fn kill_verified_children(children: &[ChildRecord]) -> usize {
    let mut killed = 0;
    for record in children {
        if child_identity_matches(record) {
            kill_pid_tree(record.pid);
            killed += 1;
        } else if process_runs(record.pid) {
            log::warn!(
                "[computer_use] virtual display: recorded child pid {} (program '{}') \
                 no longer matches its recorded identity (pid recycled?); skipping \
                 the kill",
                record.pid,
                record.program
            );
        } else {
            log::debug!(
                "[computer_use] virtual display: recorded child pid {} (program '{}') \
                 already exited; nothing to wind down",
                record.pid,
                record.program
            );
        }
    }
    killed
}

/// Graceful wind-down for a HELD child handle (the WM / Xvfb at teardown):
/// SIGTERM first — an X server exiting on SIGTERM removes its own
/// `/tmp/.X11-unix/X{n}` socket, which a SIGKILLed server cannot — then
/// SIGKILL via `kill_pid_tree` only if the grace expires. The handle was
/// never released for recycling, so the signal targets our own child.
fn terminate_then_kill(child: &mut Child, grace: Duration) {
    // SAFETY: plain kill(2) with a constant signal on our own recorded
    // child; ESRCH (already gone) is fine to ignore.
    let _ = unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    kill_pid_tree(child.id());
    let _ = child.wait_timeout(REAP_GRACE);
}

/// The argv[0] of a pid — the FIRST NUL-terminated token of
/// `/proc/<pid>/cmdline` — or `None` when the process is gone or /proc is
/// unreadable. The kernel delimits argv with NULs, so a path containing a
/// space (`/opt/My App/bin/app`) stays one token; splitting the joined
/// string on spaces would corrupt exactly those programs.
fn process_argv0(pid: u32) -> Option<String> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let first = raw
        .split(|byte: &u8| *byte == 0)
        .find(|part| !part.is_empty())?;
    Some(String::from_utf8_lossy(first).into_owned())
}

/// Crash-orphan sweep, called ONCE at app setup: every
/// `~/.pinvou3/computer-use/vd/*/pidfile.json` written by a previous (crashed)
/// run gets its recorded trees killed (`kill_pid_tree` — kill(2), ESRCH-safe,
/// never an external kill binary) and its dir removed. Directories without a
/// readable pidfile of ours are NEVER touched (they are not ours), and a
/// malformed pidfile is left in place with a warning instead of guessed at.
pub(crate) fn sweep_orphans() {
    sweep_orphans_with(process_argv0);
}

/// The role side of the sweep's argv[0] gate (pure, unit-pinned): the
/// recorded server pid must still run a program named `Xvfb`; the recorded
/// WM pid must run one of [`WM_CANDIDATES`] (the only programs `spawn_wm`
/// ever records there) — and a WM candidate does NOT satisfy the server
/// role, nor vice versa. The roles are never interchangeable because the
/// recorded pid and its role come from the same pidfile write.
fn role_matches(program: &str, role: &str) -> bool {
    if role == "wm" {
        WM_CANDIDATES.contains(&program)
    } else {
        program == role
    }
}

/// Testable core of [`sweep_orphans`]: the argv[0] lookup is injected so the
/// pid-recycling identity gate is pinable with live fixtures (a real `sleep`
/// child must survive a stale pidfile that claims it as the Xvfb).
fn sweep_orphans_with(argv0_of: impl Fn(u32) -> Option<String>) {
    let mut root = pinvou3_home();
    for part in VD_ROOT_SUBPATH {
        root.push(part);
    }
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let pidfile_path = dir.join(PIDFILE_NAME);
        let Ok(text) = std::fs::read_to_string(&pidfile_path) else {
            // No pidfile of ours inside → not ours; leave it alone.
            continue;
        };
        let Ok(file) = serde_json::from_str::<PidFile>(&text) else {
            log::warn!(
                "[computer_use] virtual display: unreadable pidfile at {}; leaving it alone",
                dir.display()
            );
            continue;
        };
        // Pid-recycling identity gate (MINOR-1): the recorded server/WM pids
        // are killed only when their argv[0] still names the recorded role —
        // "Xvfb" as the file name of the server's argv[0], one of our WM
        // candidates for the WM (the only programs `spawn_wm` ever records
        // there). A mismatch means the pid number was recycled by an
        // unrelated process between the crash and this sweep: skipped and
        // logged, because killing by number alone could take out that
        // unrelated process. Dead pids fail the same gate (no /proc entry)
        // and are skipped too — killing them would be a no-op anyway.
        let matches_role = |pid: u32, role: &str| -> bool {
            let Some(argv0) = argv0_of(pid) else {
                return false;
            };
            role_matches(basename_of(&argv0), role)
        };
        let mut gated: Vec<(u32, &str)> = vec![(file.xvfb_pid, "Xvfb")];
        if let Some(wm_pid) = file.wm_pid {
            gated.push((wm_pid, "wm"));
        }
        for (pid, role) in gated {
            if !matches_role(pid, role) {
                log::warn!(
                    "[computer_use] virtual display: recorded {role} pid {pid} does not \
                     match its role in /proc (pid recycled or already dead?); skipping \
                     the kill",
                );
                continue;
            }
            kill_pid_tree(pid);
        }
        // The open_app children carry per-child identity since the pidfile
        // gained `children_detail` (program basename + kernel start time):
        // the kill fires only on a full match. A pidfile from an older build
        // (children without detail) cannot be verified — those kills are
        // SKIPPED, the safe direction: killing by number alone could take
        // out an unrelated recycled process. Unverifiable dirs still get
        // consumed below; the recorded apps' staleness is otherwise handled
        // at the source by the respawn pruning.
        if file.children_detail.is_empty() {
            if !file.children.is_empty() {
                log::warn!(
                    "[computer_use] virtual display: pidfile lists {} child pid(s) \
                     without kill identity (older build); skipping their kill",
                    file.children.len()
                );
            }
        } else {
            for record in &file.children_detail {
                if child_identity_matches_with(record, &argv0_of) {
                    kill_pid_tree(record.pid);
                } else {
                    log::warn!(
                        "[computer_use] virtual display: recorded child pid {} \
                         (program '{}') does not match its recorded identity (pid \
                         recycled or already dead?); skipping the kill",
                        record.pid,
                        record.program
                    );
                }
            }
        }
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {
                log::info!(
                    "[computer_use] virtual display: swept orphan session dir {}",
                    dir.display()
                );
            }
            Err(error) => {
                log::warn!(
                    "[computer_use] virtual display: cannot remove the swept session dir \
                     {}: {error}",
                    dir.display()
                );
            }
        }
    }
}

#[cfg(test)]
mod test_support {
    use super::*;

    /// Isolates `PINVOU3_HOME` for the duration of a test (the backend reads
    /// it once at creation; the sweep reads it per call). ENV_LOCK is held for
    /// the whole lifetime so in-process env writes stay serialized.
    pub(super) struct IsolatedHome {
        _lock: std::sync::MutexGuard<'static, ()>,
        previous: Option<std::ffi::OsString>,
        pub home: PathBuf,
    }

    impl IsolatedHome {
        pub(super) fn take(tag: &str) -> Self {
            let lock = crate::platform::paths::tests::ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let previous = std::env::var_os("PINVOU3_HOME");
            let home = std::env::temp_dir().join(format!(
                "pinvou3-cu-vd-{tag}-{}-{}",
                std::process::id(),
                crate::platform::paths::tests::unique_suffix()
            ));
            std::fs::create_dir_all(&home).expect("create isolated PINVOU3_HOME");
            // SAFETY: ENV_LOCK is held by this struct; env writes are
            // serialized in-process.
            unsafe { std::env::set_var("PINVOU3_HOME", &home) };
            Self {
                _lock: lock,
                previous,
                home,
            }
        }

        /// The `computer-use/vd` root inside the isolated home.
        pub(super) fn vd_root(&self) -> PathBuf {
            let mut root = self.home.clone();
            for part in VD_ROOT_SUBPATH {
                root.push(part);
            }
            root
        }
    }

    impl Drop for IsolatedHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.home);
            // SAFETY: ENV_LOCK is still held by this struct.
            unsafe {
                match self.previous.take() {
                    Some(value) => std::env::set_var("PINVOU3_HOME", value),
                    None => std::env::remove_var("PINVOU3_HOME"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use test_support::IsolatedHome;

    /// The Xauthority entry is byte-exact: `u16be family | u16be len | bytes`
    /// for each of address/number/name/data after the family word. This pins
    /// the exact file bytes every Xlib client (xdotool, open_app children)
    /// parses.
    #[test]
    fn xauthority_entry_is_byte_exact() {
        let entry =
            build_xauthority_entry(256, b"host7", b"42", b"MIT-MAGIC-COOKIE-1", &[0xab_u8; 16]);
        let mut expected = Vec::new();
        expected.extend_from_slice(&256_u16.to_be_bytes()); // family = FamilyLocal
        expected.extend_from_slice(&5_u16.to_be_bytes()); // address len
        expected.extend_from_slice(b"host7"); // address
        expected.extend_from_slice(&2_u16.to_be_bytes()); // number len
        expected.extend_from_slice(b"42"); // number
        expected.extend_from_slice(&18_u16.to_be_bytes()); // name len
        expected.extend_from_slice(b"MIT-MAGIC-COOKIE-1"); // name
        expected.extend_from_slice(&16_u16.to_be_bytes()); // data len
        expected.extend_from_slice(&[0xab_u8; 16]); // data (raw bytes)
        assert_eq!(entry, expected);
    }

    /// The cookie generator returns exactly 16 bytes and never repeats
    /// (statistically impossible for independent /dev/urandom draws).
    #[test]
    fn cookie_is_16_urandom_bytes() {
        let first = generate_cookie().expect("cookie");
        let second = generate_cookie().expect("cookie");
        assert_eq!(first.len(), 16);
        assert_ne!(first, second, "two urandom draws must differ");
    }

    /// The keysym table covers EVERY `Key` variant (no wildcard arm exists, so
    /// a new variant breaks this test's variant list construction first) and
    /// pins representative values, including the non-obvious ones (xdotool
    /// resolves X keysym NAMES: Prior/Next/BackSpace, super for Meta).
    #[test]
    fn keysym_table_covers_every_key_variant() {
        let mut variants = vec![
            Key::Control,
            Key::Alt,
            Key::Shift,
            Key::Meta,
            Key::Enter,
            Key::Escape,
            Key::Tab,
            Key::Space,
            Key::Backspace,
            Key::Delete,
            Key::Insert,
            Key::Up,
            Key::Down,
            Key::Left,
            Key::Right,
            Key::Home,
            Key::End,
            Key::PageUp,
            Key::PageDown,
            Key::Char('a'),
            Key::Char('Z'),
            Key::Char('0'),
            Key::Char('+'),
            Key::Char('?'),
            Key::Char('_'),
        ];
        variants.extend((1..=12).map(|n| Key::Function(n)));
        for key in &variants {
            let name = keysym_name(*key)
                .unwrap_or_else(|error| panic!("{key:?} must map to a keysym: {error}"));
            assert!(!name.is_empty(), "{key:?} mapped to an empty keysym");
        }
        assert_eq!(keysym_name(Key::Control).as_deref().ok(), Some("ctrl"));
        assert_eq!(keysym_name(Key::Meta).as_deref().ok(), Some("super"));
        assert_eq!(keysym_name(Key::Enter).as_deref().ok(), Some("Return"));
        assert_eq!(
            keysym_name(Key::Backspace).as_deref().ok(),
            Some("BackSpace")
        );
        assert_eq!(keysym_name(Key::PageUp).as_deref().ok(), Some("Prior"));
        assert_eq!(keysym_name(Key::PageDown).as_deref().ok(), Some("Next"));
        assert_eq!(keysym_name(Key::Function(1)).as_deref().ok(), Some("F1"));
        assert_eq!(keysym_name(Key::Function(12)).as_deref().ok(), Some("F12"));
        assert_eq!(
            keysym_name(Key::Char('?')).as_deref().ok(),
            Some("question")
        );
        assert_eq!(
            keysym_name(Key::Char('(')).as_deref().ok(),
            Some("parenleft")
        );
        // Out-of-range function keys and unsupported characters fail closed,
        // and the error text never echoes the character (audit-log rule).
        assert!(keysym_name(Key::Function(13)).is_err());
        let error = keysym_name(Key::Char('é')).unwrap_err().to_string();
        assert!(!error.contains('é'), "error echoed the character: {error}");
    }

    /// Chord formatting puts modifiers first (xdotool holds them around the
    /// rest) and joins with `+` (the single-combo form of `key`).
    #[test]
    fn format_chord_orders_modifiers_first() {
        let chord = format_chord(&[Key::Char('s'), Key::Control]).expect("chord");
        assert_eq!(chord, "ctrl+s");
        // Stable partition: modifiers keep their input order.
        let chord = format_chord(&[Key::Shift, Key::Alt, Key::Function(5)]).expect("chord");
        assert_eq!(chord, "shift+alt+F5");
        let chord = format_chord(&[Key::Enter]).expect("chord");
        assert_eq!(chord, "Return");
    }

    /// The open_app child env pins the EXACT key set: the 8 toolkit/backend
    /// overrides with their exact values, the redirected session-bus address
    /// (a set-but-unusable address: libdbus/GDBus do NOT autolaunch a private
    /// bus for it, so bus-dependent features simply see a disconnected bus
    /// instead of reaching the user's real one), and exactly 2 removals
    /// (the host Wayland session variables); anything else (PATH etc.) is
    /// inherited, so the override map must contain nothing beyond the pinned
    /// keys. XDG_RUNTIME_DIR is deliberately NOT removed (apps need it).
    #[test]
    fn open_env_pins_exact_keys() {
        let mut command = Command::new("true");
        apply_open_env(
            &mut command,
            ":42",
            Path::new("/tmp/vd-auth"),
            Path::new("/tmp/vd-session"),
        );
        let mut set: Vec<(String, String)> = Vec::new();
        let mut removed: Vec<String> = Vec::new();
        for (key, value) in command.get_envs() {
            let key = key.to_string_lossy().into_owned();
            match value {
                Some(value) => set.push((key, value.to_string_lossy().into_owned())),
                None => removed.push(key),
            }
        }
        let expected: Vec<(String, String)> = [
            ("DISPLAY", ":42"),
            ("XAUTHORITY", "/tmp/vd-auth"),
            ("GDK_BACKEND", "x11"),
            ("QT_QPA_PLATFORM", "xcb"),
            ("SDL_VIDEODRIVER", "x11"),
            ("OZONE_PLATFORM", "x11"),
            ("ELECTRON_OZONE_PLATFORM_HINT", "x11"),
            ("LIBGL_ALWAYS_SOFTWARE", "1"),
            ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/tmp/vd-session/bus"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        for pair in &expected {
            assert!(
                set.contains(pair),
                "open env must set {}={}",
                pair.0,
                pair.1
            );
        }
        assert_eq!(
            set.len(),
            expected.len(),
            "open env must not set anything beyond the pinned keys: {set:?}"
        );
        removed.sort();
        assert_eq!(removed, vec!["WAYLAND_DISPLAY", "WAYLAND_SOCKET"]);
    }

    /// Audit-redaction pin (MAJOR-A): a failed `type` invocation reports the
    /// text argument length-only — the typed text itself (a possible secret)
    /// must never appear in the error text, which lands verbatim in the audit
    /// JSONL. Other commands keep their args visible (coordinates, button
    /// numbers, keysyms — nothing sensitive).
    #[test]
    fn xdotool_error_renders_typed_text_redacted() {
        // The exact shape type_text builds: `type --delay 12 -- <chunk>`.
        let args = vec![
            "type".to_string(),
            "--delay".to_string(),
            "12".to_string(),
            "--".to_string(),
            "s3cret-p4ssword".to_string(),
        ];
        let rendered = render_xdotool_args_for_error(&args);
        assert!(
            rendered.starts_with("type --delay 12 -- "),
            "the non-text args must stay visible: {rendered}"
        );
        assert!(
            rendered.contains("<15 chars redacted>"),
            "the redaction marker must carry the length: {rendered}"
        );
        assert!(
            !rendered.contains("s3cret"),
            "the typed text must not reach the error: {rendered}"
        );
        // Non-type commands are NOT redacted.
        let mouse = vec![
            "mousemove".to_string(),
            "--sync".to_string(),
            "10".to_string(),
            "20".to_string(),
        ];
        assert_eq!(
            render_xdotool_args_for_error(&mouse),
            "mousemove --sync 10 20"
        );
        let key = vec!["key".to_string(), "ctrl+s".to_string()];
        assert_eq!(render_xdotool_args_for_error(&key), "key ctrl+s");
    }

    /// Acceptance-order pin (MAJOR-B): the type_text acceptance path splits
    /// FIRST, so multi-line text is admitted (newlines/tabs become explicit
    /// Return/Tab runs — the executor's Return/Tab arms are reachable) while
    /// the printable-ASCII gate still refuses non-ASCII inside a text chunk.
    #[test]
    fn type_acceptance_splits_newlines_before_the_charset_gate() {
        let runs = plan_type_runs("hello\nworld\tend").expect("multi-line text must be accepted");
        assert_eq!(
            runs,
            vec![
                TypeRun::Text("hello".to_string()),
                TypeRun::Return,
                TypeRun::Text("world".to_string()),
                TypeRun::Tab,
                TypeRun::Text("end".to_string()),
            ]
        );
        // A trailing newline (a form-submit style type) is accepted too.
        assert_eq!(
            plan_type_runs("yes\n").expect("trailing newline"),
            vec![TypeRun::Text("yes".to_string()), TypeRun::Return]
        );
        // CRLF folds before the split (one Return, not two).
        assert_eq!(
            plan_type_runs("a\r\nb").expect("crlf"),
            vec![
                TypeRun::Text("a".to_string()),
                TypeRun::Return,
                TypeRun::Text("b".to_string())
            ]
        );
        // The gate still refuses non-ASCII text chunks, without echoing them.
        let error = plan_type_runs("ok\nhéllo").unwrap_err().to_string();
        assert!(!error.contains("héllo"), "error echoed the text: {error}");
        // Empty text plans to no runs at all (a no-op type).
        assert!(plan_type_runs("").expect("empty").is_empty());
    }

    /// open program/args validation mirrors the tool layer: non-empty,
    /// NUL-free.
    #[test]
    fn open_validation_rejects_empty_and_nul() {
        assert!(validate_open_program("xmessage").is_ok());
        assert!(validate_open_program("/usr/bin/xmessage").is_ok());
        assert!(validate_open_program("").is_err());
        assert!(validate_open_program("a\0b").is_err());
        assert!(validate_open_args(&[]).is_ok());
        assert!(validate_open_args(&["ok".to_string()]).is_ok());
        assert!(validate_open_args(&["ok".to_string(), "bad\0".to_string()]).is_err());
    }

    /// The type charset gate accepts printable ASCII (newlines/tabs are lifted
    /// into Return/Tab runs by split_type_runs before the gate) and refuses
    /// anything else WITHOUT echoing the rejected text (audit-log rule).
    #[test]
    fn type_validation_is_printable_ascii_only() {
        assert!(validate_type_text("hello world 123!@#").is_ok());
        assert!(validate_type_text("").is_ok());
        assert!(validate_type_text("héllo").is_err());
        assert!(validate_type_text("中文").is_err());
        let error = validate_type_text("passé").unwrap_err().to_string();
        assert!(!error.contains("passé"), "error echoed the text: {error}");
    }

    /// Type runs are capped at 200 characters per chunk (design §5), with
    /// newlines as separate Return runs between chunks.
    #[test]
    fn type_runs_are_chunked_at_200_chars() {
        let text = "x".repeat(450);
        let runs = split_type_runs(&text, TYPE_CHUNK_CHARS);
        let text_runs: Vec<&String> = runs
            .iter()
            .filter_map(|run| match run {
                TypeRun::Text(chunk) => Some(chunk),
                _ => None,
            })
            .collect();
        assert_eq!(text_runs.len(), 3);
        assert_eq!(text_runs[0].chars().count(), 200);
        assert_eq!(text_runs[1].chars().count(), 200);
        assert_eq!(text_runs[2].chars().count(), 50);
        let mixed = split_type_runs("ab\ncd", TYPE_CHUNK_CHARS);
        assert_eq!(
            mixed,
            vec![
                TypeRun::Text("ab".to_string()),
                TypeRun::Return,
                TypeRun::Text("cd".to_string())
            ]
        );
    }

    /// The pidfile JSON shape is frozen (the startup sweep reads files from
    /// previous builds): exactly the six documented fields.
    #[test]
    fn pidfile_json_shape_is_pinned() {
        let file = PidFile {
            xvfb_pid: 11,
            wm_pid: Some(12),
            display: ":42".to_string(),
            auth_path: "/tmp/x/Xauthority".to_string(),
            children: vec![13, 14],
            children_detail: vec![
                ChildRecord {
                    pid: 13,
                    program: "xmessage".to_string(),
                    starttime: 4_096,
                },
                ChildRecord {
                    pid: 14,
                    program: "xclock".to_string(),
                    starttime: 4_097,
                },
            ],
            started_at: 1_730_000_000,
        };
        let value: serde_json::Value = serde_json::to_value(&file).expect("serialize");
        let mut keys: Vec<&str> = value
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "auth_path",
                "children",
                "children_detail",
                "display",
                "started_at",
                "wm_pid",
                "xvfb_pid"
            ]
        );
        assert_eq!(value["xvfb_pid"], 11);
        assert_eq!(value["wm_pid"], 12);
        assert_eq!(value["display"], ":42");
        assert_eq!(value["children"][1], 14);
        assert_eq!(value["children_detail"][1]["program"], "xclock");
        assert_eq!(value["children_detail"][1]["starttime"], 4_097);
        // Round-trip (the sweep's reader).
        let parsed: PidFile =
            serde_json::from_str(&serde_json::to_string(&file).unwrap()).expect("deserialize");
        assert_eq!(parsed, file);
        // The WM-less shape serializes wm_pid as null.
        let value: serde_json::Value = serde_json::to_value(PidFile {
            wm_pid: None,
            ..file.clone()
        })
        .expect("serialize");
        assert!(value["wm_pid"].is_null());
        // Empty detail disappears (the legacy six-key shape).
        let legacy = PidFile {
            children: vec![],
            children_detail: vec![],
            ..file
        };
        let value: serde_json::Value = serde_json::to_value(&legacy).expect("serialize");
        let keys: Vec<&str> = value
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys.len(), 6, "empty detail must be skipped: {keys:?}");
        assert!(!keys.contains(&"children_detail"));
        // An OLD build's pidfile (no children_detail key) parses: the field
        // defaults to empty, and the sweep will skip those children's kills.
        let legacy_json = r#"{"xvfb_pid":11,"wm_pid":null,"display":"","auth_path":"/x","children":[9],"started_at":1}"#;
        let parsed: PidFile = serde_json::from_str(legacy_json).expect("old pidfile parses");
        assert!(parsed.children_detail.is_empty());
        assert_eq!(parsed.children, vec![9]);
    }

    /// The probe verdict lists every missing binary and names the distro
    /// packages; a green lookup answers Available.
    #[test]
    fn probe_missing_binaries_name_the_packages() {
        let detail = missing_required_binaries(|_| false).expect("both missing");
        assert!(detail.contains("Xvfb") && detail.contains("xdotool"));
        assert!(detail.contains("`xvfb`") && detail.contains("`xdotool`"));
        let detail = missing_required_binaries(|name| name != "Xvfb").expect("one missing");
        assert!(detail.contains("Xvfb") && !detail.contains("missing: xdotool"));
        assert!(missing_required_binaries(|_| true).is_none());
    }

    /// The PATH probe finds a well-known binary and rejects nonsense (no
    /// spawning, no shell involvement).
    #[test]
    fn find_on_path_walks_path_without_spawning() {
        assert!(find_on_path("sh"), "a POSIX sh must be on PATH");
        assert!(!find_on_path(""));
        assert!(!find_on_path("/absolute/path"));
        assert!(!find_on_path("pinvou3-no-such-binary-xyz"));
    }

    /// The BGRX→RGBA conversion is pinned on a 2x1 frame: X ZPixmap depth-24
    /// bytes are B,G,R,x per pixel (LSBFirst).
    #[test]
    fn bgrx_to_rgba_conversion_is_pinned() {
        let bgrx = [
            0x10, 0x20, 0x30, 0x00, // pixel 0: B=10 G=20 R=30
            0xFF, 0x80, 0x01, 0x00, // pixel 1
        ];
        let rgba = bgrx_to_rgba(&bgrx, 2, 1).expect("conversion");
        assert_eq!(rgba, vec![0x30, 0x20, 0x10, 0xFF, 0x01, 0x80, 0xFF, 0xFF]);
        assert!(bgrx_to_rgba(&bgrx[..6], 2, 1).is_err(), "truncated input");
    }

    /// The displayfd parser accepts both wire forms ("N\n" per the Xserver
    /// manual, ":N\n" defensively) and rejects garbage, all without spawning.
    #[test]
    fn displayfd_parser_accepts_both_wire_forms() {
        let (read, mut write) = UnixStream::pair().expect("socketpair");
        write.write_all(b"42\n").expect("write");
        assert_eq!(read_display_number(&read).expect("parse"), 42);

        let (read, mut write) = UnixStream::pair().expect("socketpair");
        write.write_all(b":7\n").expect("write");
        assert_eq!(read_display_number(&read).expect("parse"), 7);

        let (read, mut write) = UnixStream::pair().expect("socketpair");
        write.write_all(b"nan\n").expect("write");
        assert!(read_display_number(&read).is_err());

        // EOF before any data = the server died before reporting.
        let (read, write) = UnixStream::pair().expect("socketpair");
        drop(write);
        let error = read_display_number(&read).unwrap_err();
        assert!(error.to_string().contains("closed the display fd"));
    }

    // ---- crash-orphan sweep (fixture: fake dirs + real child processes) ----

    fn write_pidfile(dir: &Path, file: &PidFile) {
        std::fs::write(
            dir.join(PIDFILE_NAME),
            serde_json::to_string(file).expect("serialize pidfile"),
        )
        .expect("write pidfile");
    }

    /// A pidfile with a huge (nonexistent, out-of-i32-range) pid is swept as a
    /// no-op: the identity gate finds no cmdline (the process is gone) and
    /// skips the kill, and the dir is still removed.
    #[test]
    fn sweep_handles_a_nonexistent_pid_as_noop_and_removes_the_dir() {
        let home = IsolatedHome::take("sweep-noop");
        let session = home.vd_root().join("vd-dead");
        std::fs::create_dir_all(&session).expect("create session dir");
        write_pidfile(
            &session,
            &PidFile {
                xvfb_pid: 4_000_000_000, // > i32::MAX: kill_pid_tree refuses
                wm_pid: None,
                display: ":1".to_string(),
                auth_path: session.join(AUTHORITY_FILE_NAME).display().to_string(),
                children: vec![],
                children_detail: vec![],
                started_at: 0,
            },
        );
        sweep_orphans();
        assert!(!session.exists(), "the swept dir must be removed");
    }

    /// Pid-recycling pin (MINOR-1), live fixture: a stale pidfile claiming a
    /// live `sleep` process as the Xvfb must NOT get it killed — the identity
    /// gate reads /proc/<pid>/cmdline, sees `sleep` instead of the recorded
    /// "Xvfb" role, and skips-and-logs. Without the gate, killing by number
    /// alone could take out the unrelated process that recycled the pid. The
    /// stale session dir itself is still consumed.
    #[test]
    fn sweep_skips_a_recorded_pid_whose_cmdline_does_not_match_the_role() {
        if Command::new("sleep").arg("0").status().is_err() {
            eprintln!("skipping: no `sleep` binary on this platform");
            return;
        }
        let home = IsolatedHome::take("sweep-mismatch");
        let session = home.vd_root().join("vd-recycled");
        std::fs::create_dir_all(&session).expect("create session dir");
        let mut command = Command::new("sleep");
        command.arg("30");
        std_process_group_leader(&mut command);
        let mut child = command.spawn().expect("spawn sleep fixture");
        let pid = child.id();
        write_pidfile(
            &session,
            &PidFile {
                xvfb_pid: pid,
                wm_pid: None,
                display: ":3".to_string(),
                auth_path: session.join(AUTHORITY_FILE_NAME).display().to_string(),
                children: vec![],
                children_detail: vec![],
                started_at: 0,
            },
        );
        sweep_orphans(); // real /proc lookup: sleep ≠ the recorded "Xvfb" role
        // Give an (incorrect) async group kill no chance to hide, then check
        // via the held Child handle (a SIGKILLed child shows up as reaped).
        std::thread::sleep(Duration::from_millis(500));
        assert!(
            matches!(child.try_wait(), Ok(None)),
            "a recorded pid whose cmdline does not name Xvfb must be untouched"
        );
        assert!(!session.exists(), "the stale session dir is still consumed");
        // Clean up the fixture via the sanctioned tree kill.
        kill_pid_tree(pid);
        let _ = child.wait();
    }

    /// The WM arm of the role gate, live fixture: a stale pidfile claiming a
    /// live `sleep` process as the WM (`wm_pid`) must NOT get it killed —
    /// the gate reads /proc/<pid>/cmdline, sees a basename outside
    /// WM_CANDIDATES, and skips-and-logs. The dir is still consumed.
    #[test]
    fn sweep_skips_a_recorded_wm_whose_cmdline_is_not_a_wm() {
        if Command::new("sleep").arg("0").status().is_err() {
            eprintln!("skipping: no `sleep` binary on this platform");
            return;
        }
        let home = IsolatedHome::take("sweep-wm-mismatch");
        let session = home.vd_root().join("vd-wm");
        std::fs::create_dir_all(&session).expect("create session dir");
        let mut command = Command::new("sleep");
        command.arg("30");
        std_process_group_leader(&mut command);
        let mut child = command.spawn().expect("spawn sleep fixture");
        let pid = child.id();
        write_pidfile(
            &session,
            &PidFile {
                xvfb_pid: 4_000_000_005,
                wm_pid: Some(pid),
                display: ":7".to_string(),
                auth_path: session.join(AUTHORITY_FILE_NAME).display().to_string(),
                children: vec![],
                children_detail: vec![],
                started_at: 0,
            },
        );
        sweep_orphans(); // real /proc lookup: sleep is not a WM candidate
        std::thread::sleep(Duration::from_millis(500));
        assert!(
            matches!(child.try_wait(), Ok(None)),
            "a recorded wm pid whose cmdline names no WM candidate must be untouched"
        );
        assert!(!session.exists(), "the stale session dir is still consumed");
        kill_pid_tree(pid);
        let _ = child.wait();
    }

    /// The role-matching table itself: the server role matches only "Xvfb",
    /// the WM role matches exactly the spawn candidates, and neither role
    /// accepts the other side's programs (a real WM is not an Xvfb; Xvfb is
    /// not a WM candidate).
    #[test]
    fn sweep_role_gate_pins_xvfb_and_wm_membership() {
        assert!(role_matches("Xvfb", "Xvfb"), "the server role matches Xvfb");
        assert!(
            !role_matches("sleep", "Xvfb"),
            "an unrelated basename must not satisfy the server role"
        );
        for candidate in WM_CANDIDATES {
            assert!(
                role_matches(candidate, "wm"),
                "every spawn candidate must satisfy the wm role: {candidate}"
            );
        }
        assert!(
            !role_matches("mutter", "Xvfb"),
            "a WM candidate must not satisfy the server role"
        );
        assert!(
            !role_matches("Xvfb", "wm"),
            "the server must not satisfy the wm role"
        );
        assert!(
            !role_matches("not-a-wm", "wm"),
            "an unrelated basename must not satisfy the wm role"
        );
    }

    /// The matching side of the identity gate: with an argv[0] lookup that
    /// REPORTS the recorded role for the fixture pid, the live recorded child
    /// (a `sleep` process as group leader, fixture style of
    /// platform::process tests) must die when the sweep reads its pid from a
    /// pidfile, and its dir must be removed.
    #[test]
    fn sweep_kills_a_live_recorded_child_whose_argv0_matches() {
        if Command::new("sleep").arg("0").status().is_err() {
            eprintln!("skipping: no `sleep` binary on this platform");
            return;
        }
        let home = IsolatedHome::take("sweep-live");
        let session = home.vd_root().join("vd-live");
        std::fs::create_dir_all(&session).expect("create session dir");
        let mut command = Command::new("sleep");
        command.arg("30");
        std_process_group_leader(&mut command);
        let mut child = command.spawn().expect("spawn sleep fixture");
        let pid = child.id();
        write_pidfile(
            &session,
            &PidFile {
                xvfb_pid: pid,
                wm_pid: None,
                display: ":2".to_string(),
                auth_path: session.join(AUTHORITY_FILE_NAME).display().to_string(),
                children: vec![],
                children_detail: vec![],
                started_at: 0,
            },
        );
        // The fake lookup simulates the genuine Xvfb argv[0] for the fixture
        // pid (a real Xvfb cannot be spawned in a unit test); everything else
        // uses the real /proc.
        sweep_orphans_with(|p| {
            if p == pid {
                Some("Xvfb".to_string())
            } else {
                process_argv0(p)
            }
        });
        // Poll for the group kill to take effect (SIGKILL is async). The
        // death is observed via the held Child handle: a SIGKILLed child
        // lingers as an unreaped zombie, where kill(pid, 0) still succeeds.
        let mut gone = false;
        for _ in 0..60 {
            if matches!(child.try_wait(), Ok(Some(_))) {
                gone = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(gone, "the recorded child must be dead after the sweep");
        assert!(!session.exists(), "the swept dir must be removed");
        let _ = child.wait();
    }

    /// A live `sleep` fixture recorded as an open_app child WITH full kill
    /// identity (program "sleep" + its real /proc starttime) must die when
    /// the sweep fires — the matching arm of the children gate, mirroring
    /// the Xvfb-role pins above.
    #[test]
    fn sweep_kills_a_recorded_child_whose_identity_matches() {
        if Command::new("sleep").arg("0").status().is_err() {
            eprintln!("skipping: no `sleep` binary on this platform");
            return;
        }
        let home = IsolatedHome::take("sweep-child-match");
        let session = home.vd_root().join("vd-child");
        std::fs::create_dir_all(&session).expect("create session dir");
        let mut command = Command::new("sleep");
        command.arg("30");
        std_process_group_leader(&mut command);
        let mut child = command.spawn().expect("spawn sleep fixture");
        let pid = child.id();
        let starttime = process_starttime(pid).expect("fixture /proc stat readable");
        write_pidfile(
            &session,
            &PidFile {
                xvfb_pid: 4_000_000_001,
                wm_pid: None,
                display: ":4".to_string(),
                auth_path: session.join(AUTHORITY_FILE_NAME).display().to_string(),
                children: vec![pid],
                children_detail: vec![ChildRecord {
                    pid,
                    program: "sleep".to_string(),
                    starttime,
                }],
                started_at: 0,
            },
        );
        sweep_orphans();
        let mut gone = false;
        for _ in 0..60 {
            if matches!(child.try_wait(), Ok(Some(_))) {
                gone = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(gone, "an identity-verified child must be killed");
        assert!(!session.exists(), "the swept dir must be removed");
        let _ = child.wait();
    }

    /// The exec arm of the children gate: a child that kept its pid and
    /// kernel start time but changed its program image (the `bash -c 'app'`
    /// tail-exec shape — recorded as `xmessage` here, running as something
    /// else) must STILL be killed by the sweep. The start time is the
    /// recycling-proof identity; the program name is a diagnostic only.
    #[test]
    fn sweep_kills_a_child_that_changed_program_but_keeps_its_starttime() {
        if Command::new("sleep").arg("0").status().is_err() {
            eprintln!("skipping: no `sleep` binary on this platform");
            return;
        }
        let home = IsolatedHome::take("sweep-child-exec");
        let session = home.vd_root().join("vd-child2");
        std::fs::create_dir_all(&session).expect("create session dir");
        let mut command = Command::new("sleep");
        command.arg("30");
        std_process_group_leader(&mut command);
        let mut child = command.spawn().expect("spawn sleep fixture");
        let pid = child.id();
        let starttime = process_starttime(pid).expect("fixture /proc stat readable");
        write_pidfile(
            &session,
            &PidFile {
                xvfb_pid: 4_000_000_002,
                wm_pid: None,
                display: ":5".to_string(),
                auth_path: session.join(AUTHORITY_FILE_NAME).display().to_string(),
                children: vec![pid],
                children_detail: vec![ChildRecord {
                    pid,
                    // Deliberately wrong: the exec'd image's argv[0] no longer
                    // matches what was recorded at spawn time.
                    program: "xmessage".to_string(),
                    starttime,
                }],
                started_at: 0,
            },
        );
        sweep_orphans();
        let mut gone = false;
        for _ in 0..60 {
            if matches!(child.try_wait(), Ok(Some(_))) {
                gone = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            gone,
            "a start-time-verified child must be killed even when its program name changed"
        );
        assert!(!session.exists(), "the swept dir must be removed");
        let _ = child.wait();
    }

    /// The recycling arm of the children gate: the same pid recorded with a
    /// start time that NO LONGER matches /proc (the recycled process's kernel
    /// start time can never reproduce the recorded value) must survive the
    /// sweep — killing by number alone could take out an unrelated process.
    #[test]
    fn sweep_skips_a_recorded_child_whose_starttime_does_not_match() {
        if Command::new("sleep").arg("0").status().is_err() {
            eprintln!("skipping: no `sleep` binary on this platform");
            return;
        }
        let home = IsolatedHome::take("sweep-child-mismatch");
        let session = home.vd_root().join("vd-child2b");
        std::fs::create_dir_all(&session).expect("create session dir");
        let mut command = Command::new("sleep");
        command.arg("30");
        std_process_group_leader(&mut command);
        let mut child = command.spawn().expect("spawn sleep fixture");
        let pid = child.id();
        let starttime = process_starttime(pid).expect("fixture /proc stat readable");
        write_pidfile(
            &session,
            &PidFile {
                xvfb_pid: 4_000_000_002,
                wm_pid: None,
                display: ":5".to_string(),
                auth_path: session.join(AUTHORITY_FILE_NAME).display().to_string(),
                children: vec![pid],
                children_detail: vec![ChildRecord {
                    pid,
                    program: "sleep".to_string(),
                    starttime: starttime.wrapping_add(1),
                }],
                started_at: 0,
            },
        );
        sweep_orphans();
        std::thread::sleep(Duration::from_millis(500));
        assert!(
            matches!(child.try_wait(), Ok(None)),
            "a child whose start time no longer matches the record must be untouched"
        );
        assert!(!session.exists(), "the stale session dir is still consumed");
        kill_pid_tree(pid);
        let _ = child.wait();
    }

    /// An old build's pidfile (children listed WITHOUT kill identity) cannot
    /// be verified, so the sweep must SKIP those kills — the safe direction
    /// when a pid number may have been recycled. The dir itself is consumed.
    #[test]
    fn sweep_skips_children_listed_without_identity_detail() {
        if Command::new("sleep").arg("0").status().is_err() {
            eprintln!("skipping: no `sleep` binary on this platform");
            return;
        }
        let home = IsolatedHome::take("sweep-child-nodetail");
        let session = home.vd_root().join("vd-child3");
        std::fs::create_dir_all(&session).expect("create session dir");
        let mut command = Command::new("sleep");
        command.arg("30");
        std_process_group_leader(&mut command);
        let mut child = command.spawn().expect("spawn sleep fixture");
        let pid = child.id();
        write_pidfile(
            &session,
            &PidFile {
                xvfb_pid: 4_000_000_003,
                wm_pid: None,
                display: ":6".to_string(),
                auth_path: session.join(AUTHORITY_FILE_NAME).display().to_string(),
                children: vec![pid],
                children_detail: vec![],
                started_at: 0,
            },
        );
        sweep_orphans();
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            matches!(child.try_wait(), Ok(None)),
            "children without kill identity must not be killed by number alone"
        );
        assert!(!session.exists(), "the stale session dir is still consumed");
        kill_pid_tree(pid);
        let _ = child.wait();
    }

    /// The identity pieces are pure and locally pinable: basename_of strips
    /// paths, and the start-time reader returns the same monotonic value on
    /// repeated reads of a live pid (and None for a nonsense pid).
    #[test]
    fn identity_helpers_pin_basename_and_starttime() {
        assert_eq!(basename_of("xmessage"), "xmessage");
        assert_eq!(basename_of("/usr/bin/xmessage"), "xmessage");
        assert_eq!(basename_of(""), "");
        let mine = process_starttime(std::process::id()).expect("own /proc stat readable");
        assert!(mine > 0, "a real start time is a positive tick count");
        assert_eq!(
            process_starttime(std::process::id()),
            Some(mine),
            "start time is stable across reads"
        );
        assert_eq!(process_starttime(4_000_000_004), None);
    }

    /// The start time is the verdict; the program name is a diagnostic. A
    /// start-time-matching child must be killed regardless of what its
    /// argv[0] says now (the tail-exec/shebang shapes), while a start-time
    /// mismatch is skipped no matter how well the name matches. The pure
    /// `basename_of` space-safety is pinned separately below — it still
    /// backs the diagnostic log and the sweep's Xvfb/WM role gate, which
    /// read argv[0] at the kernel's NUL boundary so `/opt/My App/bin/app`
    /// stays one token.
    #[test]
    fn identity_gate_decides_on_starttime_not_program_name() {
        let mine = std::process::id();
        let record = ChildRecord {
            pid: mine,
            program: "app".to_string(),
            starttime: process_starttime(mine).expect("own /proc stat readable"),
        };
        assert!(
            child_identity_matches_with(&record, |_| { Some("/opt/My App/bin/app".to_string()) }),
            "a start-time match decides the kill; argv[0] is only logged"
        );
        assert!(
            child_identity_matches_with(&record, |_| { Some("/usr/bin/other".to_string()) }),
            "a changed basename must not veto a start-time match (exec'd child)"
        );
        assert!(
            child_identity_matches_with(&record, |_| None),
            "an unreadable argv[0] must not veto a start-time match either"
        );
        let stale = ChildRecord {
            pid: mine,
            program: "app".to_string(),
            starttime: process_starttime(mine)
                .expect("own /proc stat readable")
                .wrapping_add(1),
        };
        assert!(
            !child_identity_matches_with(&stale, |_| { Some("/opt/My App/bin/app".to_string()) }),
            "a start-time mismatch is skipped even with a matching name"
        );
        assert_eq!(
            basename_of("/opt/My App/bin/app"),
            "app",
            "basename_of splits on '/' only, so spaces in the path stay one token"
        );
    }

    /// Directories without OUR pidfile are never touched, and a malformed
    /// pidfile is left in place (never guessed at).
    #[test]
    fn sweep_never_touches_dirs_without_our_pidfile() {
        let home = IsolatedHome::take("sweep-skip");
        let no_pidfile = home.vd_root().join("not-ours");
        std::fs::create_dir_all(&no_pidfile).expect("create dir");
        std::fs::write(no_pidfile.join("unrelated.txt"), b"keep").expect("write marker");
        let malformed = home.vd_root().join("malformed");
        std::fs::create_dir_all(&malformed).expect("create dir");
        std::fs::write(malformed.join(PIDFILE_NAME), b"{not json").expect("write pidfile");
        sweep_orphans();
        assert!(
            no_pidfile.exists(),
            "a dir without our pidfile must survive"
        );
        assert!(
            malformed.exists(),
            "a dir with a malformed pidfile must survive"
        );
    }
}

/// LIVE tests for the full virtual-display lifecycle. These spawn a REAL Xvfb
/// server, drive it with the backend, and verify input EXTERNALLY (a second
/// xdotool run with the same DISPLAY/XAUTHORITY, so the assertions cannot be
/// fooled by anything inside the process). They are `#[ignore]` + env-gated
/// (double opt-in, x11_live_tests conventions) and must run single-threaded:
///
/// ```text
/// cd pinvou3-app/src-tauri
/// PINVOU3_CU_XVFB_LIVE=1 cargo test --lib computer_use -- --ignored --test-threads=1
/// ```
///
/// The tests pass WM-less by design (no window manager is installed on the
/// dev machine), which exercises the optional-WM path. They skip (early
/// return, not fail) when the gate is unset or Xvfb/xdotool are missing.
#[cfg(test)]
mod xvfb_live_tests {
    use super::test_support::IsolatedHome;
    use super::*;

    /// The live gate: `#[ignore]` is only the conventional guard; this env
    /// variable is the explicit opt-in (the tests spawn a real display server
    /// and inject real input into it).
    fn live_enabled() -> bool {
        std::env::var("PINVOU3_CU_XVFB_LIVE").ok().as_deref() == Some("1")
    }

    /// Skip unless the gate is on AND the machine has the required binaries.
    fn live_ready(home: &IsolatedHome) -> bool {
        if !live_enabled() {
            eprintln!("skipping: PINVOU3_CU_XVFB_LIVE is not set to 1");
            return false;
        }
        // The probe reads PATH only; the isolated home affects nothing here.
        let _ = home;
        if let VirtualAvailability::Unavailable { detail } = probe() {
            eprintln!("skipping: the virtual display stack is unavailable: {detail}");
            return false;
        }
        true
    }

    /// EXTERNAL ground truth: run xdotool against the virtual display with
    /// the same DISPLAY/XAUTHORITY the backend published (not through the
    /// backend, so a backend-side bug cannot hide).
    fn external_xdotool(display: &str, auth_path: &Path, args: &[&str]) -> String {
        let output = Command::new("xdotool")
            .env("DISPLAY", display)
            .env("XAUTHORITY", auth_path)
            .args(args)
            .output()
            .expect("spawn the external xdotool");
        assert!(
            output.status.success(),
            "external xdotool {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Pointer position as the X server itself reports it (`--shell` form:
    /// lines `X=…` / `Y=…`).
    fn external_pointer(display: &str, auth_path: &Path) -> (i32, i32) {
        let output = external_xdotool(display, auth_path, &["getmouselocation", "--shell"]);
        let mut x = None;
        let mut y = None;
        for line in output.lines() {
            if let Some(value) = line.strip_prefix("X=") {
                x = value.trim().parse::<i32>().ok();
            }
            if let Some(value) = line.strip_prefix("Y=") {
                y = value.trim().parse::<i32>().ok();
            }
        }
        (
            x.expect("external getmouselocation reported X"),
            y.expect("reported Y"),
        )
    }

    /// True once the pid no longer RUNS anything: either the process entry is
    /// gone (ESRCH) or it is a zombie (state `Z` — a SIGKILLed child we hold
    /// a Child handle for, or an open_app child its detached reaper thread
    /// has not gotten to yet, stays kill(pid, 0)-addressable until reaped).
    /// Delegates to the production check (same semantics the respawn pruning
    /// uses), so the live assertions cannot drift from the real logic.
    fn gone_or_zombie(pid: u32) -> bool {
        !process_runs(pid)
    }

    /// Poll until the recorded pid runs nothing (SIGKILL is async; see
    /// [`gone_or_zombie`]). The pid must be one WE recorded, never our own.
    fn wait_until_gone(pid: u32, what: &str) {
        for _ in 0..60 {
            // SAFETY: kill with sig=0 only probes liveness; no memory touched.
            if unsafe { libc::kill(pid as i32, 0) } != 0 || gone_or_zombie(pid) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .map(|stat| stat.chars().take(120).collect::<String>())
            .unwrap_or_else(|error| format!("unreadable: {error}"));
        panic!("{what} (pid {pid}) is still alive after teardown; /proc stat: {stat}");
    }

    /// Poll a condition for up to ~3 s (60 × 50 ms) — the generic shape the
    /// fixed sleeps used to approximate; a loaded machine gets the same
    /// budget without a hard-coded floor.
    fn wait_until(mut condition: impl FnMut() -> bool, what: &str) -> bool {
        for _ in 0..60 {
            if condition() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        eprintln!("warning: timed out waiting for {what}");
        false
    }

    fn read_pidfile(session_dir: &Path) -> PidFile {
        let text =
            std::fs::read_to_string(session_dir.join(PIDFILE_NAME)).expect("read the pidfile");
        serde_json::from_str(&text).expect("parse the pidfile")
    }

    /// Full cycle: probe → spawn → readiness → open `xmessage ok` →
    /// screenshot (1280x800, non-black) → move + click verified via an
    /// EXTERNAL xdotool against the same display/auth → scroll → type →
    /// chord → hold → drag → teardown asserting process death, session-dir
    /// removal and pidfile removal. Runs on BOTH machine shapes: WM-less
    /// boxes exercise the optional-WM path, WM-equipped boxes additionally
    /// assert the recorded `wm_pid` dies with the session (a path no other
    /// test observes).
    #[test]
    #[ignore]
    fn xvfb_live_full_cycle() {
        let home = IsolatedHome::take("live-full");
        if !live_ready(&home) {
            return;
        }
        let mut backend = VirtualX11Backend::new().expect("virtual backend creation");

        // Capabilities record the virtual target.
        let capabilities = backend.capabilities();
        assert!(capabilities.screenshot && capabilities.input && !capabilities.ui_tree);
        assert_eq!(capabilities.target, DisplayTarget::Virtual);
        assert!(
            capabilities.target_note.contains("Xvfb"),
            "{}",
            capabilities.target_note
        );
        assert!(capabilities.target_note.contains("desktop is unaffected"));

        // Session dir artifacts: pidfile (immediately after spawn), 0600
        // Xauthority, no WM on this machine.
        let session_dir = backend.session_dir.clone();
        assert!(session_dir.is_dir());
        let auth_path = backend.auth_path.clone();
        let auth_mode = std::fs::metadata(&auth_path)
            .expect("auth file exists")
            .permissions()
            .mode();
        assert_eq!(auth_mode & 0o777, 0o600, "the Xauthority file must be 0600");
        let pidfile = read_pidfile(&session_dir);
        assert!(
            pidfile.display.starts_with(':'),
            "pidfile display = {:?}",
            pidfile.display
        );
        assert!(pidfile.xvfb_pid > 0);
        // Portability: WM presence is machine state, not a test given — the
        // WM-dependent EXPECTATION (`wm_pid` must be None) is gated on the
        // flag below, but the app lifecycle, capture, input and teardown
        // assertions run on BOTH machine shapes: the WM teardown path (a
        // recorded `wm_pid` dies on drop) is otherwise asserted nowhere on
        // any machine state, and the external-pointer pins are server state
        // a WM does not touch.
        let has_wm = pidfile.wm_pid.is_some();
        if has_wm {
            eprintln!("a WM candidate IS installed here: running the WM-equipped cycle");
        } else {
            assert_eq!(pidfile.wm_pid, None, "no WM is installed on this machine");
        }
        assert!(pidfile.children.is_empty(), "no app children yet");

        // open an app into the virtual display (xmessage is WM-less-safe).
        // Portability: xmessage may be missing (no `x11-apps` package) —
        // skip the app-dependent assertions rather than failing.
        if !find_on_path("xmessage") {
            eprintln!("skipping: xmessage is not installed on this machine");
            drop(backend);
            return;
        }
        let app_pid = backend
            .open_app("xmessage", &["ok".to_string()])
            .expect("open xmessage");
        assert!(app_pid > 0);
        // Poll for the app window instead of a fixed sleep (a loaded machine
        // maps the window later than 1.2s).
        wait_until(
            || {
                read_pidfile(&session_dir)
                    .children
                    .first()
                    .is_some_and(|&pid| pid == app_pid)
            },
            "open_app to record its pid",
        );
        let pidfile = read_pidfile(&session_dir);
        assert_eq!(
            pidfile.children,
            vec![app_pid],
            "open_app must record its pid"
        );

        // Capture: full screen, correct contract values, xmessage visible.
        let shot = backend.capture().expect("capture");
        assert_eq!(shot.width, 1280);
        assert_eq!(shot.height, 800);
        assert_eq!(shot.rgba.len(), (1280 * 800 * 4) as usize);
        assert_eq!(shot.origin_x, 0);
        assert_eq!(shot.origin_y, 0);
        assert_eq!(shot.input_scale_x, 1.0);
        assert_eq!(shot.input_scale_y, 1.0);
        assert!(shot.input_aligned);
        // The xmessage window is white: some RED byte must be bright (RGBA
        // layout, so step_by(4) walks the R bytes). Poll with FRESH captures
        // (the window maps on the server's own schedule).
        assert!(
            wait_until(
                || {
                    backend
                        .capture()
                        .map(|frame| frame.rgba.iter().step_by(4).any(|&red| red > 16))
                        .unwrap_or(false)
                },
                "the capture to show the xmessage window",
            ),
            "the capture must not be an all-black frame with xmessage open"
        );

        // Input, verified against the X server itself (external xdotool).
        backend.move_to(100, 200).expect("move_to");
        assert_eq!(external_pointer(&pidfile.display, &auth_path), (100, 200));
        backend.move_to(640, 400).expect("move_to center");
        backend.click(MouseButton::Left, 1).expect("click");
        // click must not move the pointer.
        assert_eq!(external_pointer(&pidfile.display, &auth_path), (640, 400));
        // The backend's own cursor report agrees with the server's.
        assert_eq!(
            backend.cursor_position().expect("cursor_position"),
            (640, 400)
        );

        backend.scroll(ScrollDirection::Down, 2).expect("scroll");
        backend.type_text("hello pinvou").expect("type_text");
        backend
            .key_chord(&[Key::Control, Key::Char('a')])
            .expect("key_chord");
        backend
            .hold_key(&[Key::Shift, Key::Char('x')], 50)
            .expect("hold_key");
        backend.drag((100, 100), (300, 300)).expect("drag");

        // Teardown: drop → children/WM/Xvfb killed, session dir + pidfile
        // removed, no process residue. The WM (when this machine has one)
        // must die with the session too — recorded `wm_pid`, SIGTERM-first
        // wind-down.
        let xvfb_pid = pidfile.xvfb_pid;
        let wm_pid = pidfile.wm_pid;
        drop(backend);
        wait_until_gone(xvfb_pid, "the Xvfb server");
        if let Some(wm) = wm_pid {
            wait_until_gone(wm, "the window manager");
        }
        wait_until_gone(app_pid, "the opened xmessage");
        assert!(
            !session_dir.exists(),
            "the session dir must be removed on drop"
        );
        assert!(
            !session_dir.join(PIDFILE_NAME).exists(),
            "no pidfile residue"
        );
        assert!(!auth_path.exists(), "the cookie file must be gone");
    }

    /// Respawn-after-server-kill: kill the Xvfb under the backend; the NEXT
    /// backend call performs the ONE lazy respawn and the capture recovers on
    /// a fresh server (new pid, refreshed pidfile).
    #[test]
    #[ignore]
    fn xvfb_live_respawn_after_server_kill() {
        let home = IsolatedHome::take("live-respawn");
        if !live_ready(&home) {
            return;
        }
        let mut backend = VirtualX11Backend::new().expect("virtual backend creation");
        let first = backend.capture().expect("initial capture");
        assert_eq!((first.width, first.height), (1280, 800));

        let session_dir = backend.session_dir.clone();
        let pidfile = read_pidfile(&session_dir);
        let old_pid = pidfile.xvfb_pid;

        // Kill the server out from under the backend (direct kill(2) — the
        // sanctioned syscall path, never an external kill binary).
        // SAFETY: direct kill(2) on a pid WE spawned; no memory touched.
        assert_eq!(unsafe { libc::kill(old_pid as i32, libc::SIGKILL) }, 0);
        wait_until_gone(old_pid, "the killed Xvfb (pre-respawn)");

        // The next call attempts one lazy respawn and must recover.
        let second = backend
            .capture()
            .expect("capture must recover via the one lazy respawn after a server kill");
        assert_eq!((second.width, second.height), (1280, 800));
        let pidfile = read_pidfile(&session_dir);
        assert_ne!(
            pidfile.xvfb_pid, old_pid,
            "a respawned server must have a fresh pid"
        );
        assert!(
            pidfile.display.starts_with(':'),
            "the respawned pidfile must carry a display: {:?}",
            pidfile.display
        );

        // Teardown of the respawned stack is equally complete.
        let new_pid = pidfile.xvfb_pid;
        drop(backend);
        wait_until_gone(new_pid, "the respawned Xvfb");
        assert!(!session_dir.exists());
    }

    /// release_os_grant pin (MAJOR-D, live): Stop/revoke/master-off on the
    /// virtual target kills the RECORDED children (the apps the agent opened)
    /// while Xvfb — and the WM when present — stay alive, empties the pidfile
    /// children list, and the backend keeps working (the trait contract: the
    /// next action, including a re-granted session, must still succeed).
    #[test]
    #[ignore]
    fn xvfb_live_release_os_grant_kills_apps_and_keeps_the_display() {
        let home = IsolatedHome::take("live-release");
        if !live_ready(&home) {
            return;
        }
        let mut backend = VirtualX11Backend::new().expect("virtual backend creation");
        let session_dir = backend.session_dir.clone();
        // Portability: xmessage may be missing (no `x11-apps` package) —
        // skip the app-dependent assertions rather than failing (the module
        // contract: skip, early return, not fail).
        if !find_on_path("xmessage") {
            eprintln!("skipping: xmessage is not installed on this machine");
            drop(backend);
            return;
        }
        let app_pid = backend
            .open_app("xmessage", &["released".to_string()])
            .expect("open xmessage");
        // Poll for the recording like the full-cycle test (a loaded machine
        // maps the window later than any fixed sleep).
        wait_until(
            || {
                read_pidfile(&session_dir)
                    .children
                    .first()
                    .is_some_and(|&pid| pid == app_pid)
            },
            "open_app to record its pid",
        );
        let pidfile = read_pidfile(&session_dir);
        assert_eq!(
            pidfile.children,
            vec![app_pid],
            "open_app must record its pid"
        );
        let xvfb_pid = pidfile.xvfb_pid;
        let wm_pid = pidfile.wm_pid;

        backend.release_os_grant().expect("release_os_grant");

        wait_until_gone(app_pid, "the opened app after release_os_grant");
        let pidfile = read_pidfile(&session_dir);
        assert!(
            pidfile.children.is_empty(),
            "the released apps must leave the pidfile children list: {:?}",
            pidfile.children
        );
        assert!(
            process_runs(xvfb_pid),
            "Xvfb must stay alive across release_os_grant"
        );
        // The WM belongs to the display, not the released grant: on a machine
        // with a WM it must survive the release exactly like the server.
        if let Some(wm_pid) = wm_pid {
            assert!(
                process_runs(wm_pid),
                "the WM must stay alive across release_os_grant"
            );
        }
        // The display still answers: subsequent actions work (trait contract —
        // the backend must remain usable after the release, e.g. for a
        // re-granted session).
        backend
            .move_to(40, 60)
            .expect("action after release_os_grant");
        assert_eq!(backend.cursor_position().expect("cursor"), (40, 60));

        // Final teardown still removes everything (Xvfb was never released).
        drop(backend);
        wait_until_gone(xvfb_pid, "the Xvfb after the final drop");
        assert!(!session_dir.exists(), "the session dir must be removed");
    }

    /// Open-env pin (MAJOR-E, live): an opened child's ACTUAL environment
    /// (read from /proc/<pid>/environ after exec) carries the redirected
    /// session-bus address pointing inside the session dir — the exact
    /// key/value — so session-bus traffic (notify-send & co) cannot reach the
    /// user's real bus. XDG_RUNTIME_DIR stays present (deliberately kept).
    #[test]
    #[ignore]
    fn xvfb_live_open_env_carries_the_redirected_session_bus() {
        let home = IsolatedHome::take("live-dbus");
        if !live_ready(&home) {
            return;
        }
        let mut backend = VirtualX11Backend::new().expect("virtual backend creation");
        let session_dir = backend.session_dir.clone();
        // Portability: xmessage may be missing (no `x11-apps` package) —
        // skip the app-dependent assertions rather than failing (the module
        // contract: skip, early return, not fail).
        if !find_on_path("xmessage") {
            eprintln!("skipping: xmessage is not installed on this machine");
            drop(backend);
            return;
        }
        let app_pid = backend
            .open_app("xmessage", &["bus".to_string()])
            .expect("open xmessage");
        let expected = format!(
            "DBUS_SESSION_BUS_ADDRESS=unix:path={}",
            session_dir.join("bus").display()
        );
        // Retry briefly: /proc/<pid>/environ only shows the post-exec env, and
        // the exec can lag the spawn by a scheduler tick. Read as BYTES (the
        // entries are NUL-separated, not line-oriented).
        let mut seen = None;
        for _ in 0..30 {
            if let Ok(raw) = std::fs::read(format!("/proc/{app_pid}/environ")) {
                if raw
                    .split(|byte: &u8| *byte == 0)
                    .any(|entry| entry == expected.as_bytes())
                {
                    seen = Some(raw);
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let environ = seen.expect("the child must inherit the redirected session bus");
        assert!(
            environ
                .split(|byte: &u8| *byte == 0)
                .any(|entry| entry.starts_with(b"XDG_RUNTIME_DIR=")),
            "XDG_RUNTIME_DIR must stay (apps need it): {:?}",
            String::from_utf8_lossy(&environ)
        );
        backend.release_os_grant().expect("cleanup release");
        wait_until_gone(app_pid, "the opened app (cleanup)");
    }
}

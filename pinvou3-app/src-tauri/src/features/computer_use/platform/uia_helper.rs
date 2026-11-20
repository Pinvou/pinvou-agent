//! The hidden-desktop UIA helper process — parent-side client and the
//! helper's own loop.
//!
//! Cross-desktop UI Automation cannot run from the backend's worker thread:
//! the UIA core and the COM apartment it lives in bind to the calling
//! process's desktop context, and this process's default desktop is the
//! USER'S real one. So the queries run in an attached HELPER PROCESS — this
//! same executable re-spawned with [`uia_helper_protocol::HELPER_ARG`] and
//! `lpDesktop` pointing at the hidden desktop (via the same contained-launch
//! path as every other session child, so the helper is also inside the
//! kill-on-close job). The helper initializes a UIA client ON the hidden
//! desktop and answers NDJSON requests over inherited stdin/stdout.
//!
//! Desktop scoping is explicit at every op: the helper does its own
//! desktop-relative window walk (`GetTopWindow`/`WindowFromPoint` are
//! desktop-relative to the calling thread, and the helper's threads are on
//! the hidden desktop) and hands UIA concrete HWNDs via
//! `element_from_handle`. It deliberately NEVER calls UIA's
//! `get_focused_element` or `element_from_point` — those resolve against the
//! INPUT desktop (the user's real one) and would silently return the USER'S
//! focused element. `focus` instead reads the foreground window's thread
//! focus via `GetGUIThreadInfo` (desktop-relative) and maps that HWND.
//!
//! Lifecycle: the parent lazily spawns the helper on the first UIA query
//! (with a respawn cooldown after any TRANSPORT failure), degrades to the
//! phase-2 window-class walk whenever the helper is unavailable, and tears
//! the client down on drop: the write pipe closes (stdin EOF is the helper's
//! clean-exit signal) and the retained spawn handle is terminated — a helper
//! wedged inside one uninterruptible cross-process UIA call never reaches
//! its read loop again, so EOF alone can strand it (with this reader
//! thread) until the session-end job; terminating on client drop bounds
//! every drop path to zero residue (the kill-on-close job remains the
//! session-end backstop). The helper is session INFRASTRUCTURE (like the
//! shell): it is not in the wind-down list, so Stop/revoke leaves it alive
//! for a re-granted session. A healthy helper's SOFT verdict (an honest
//! "cannot serve" — e.g. nothing holds focus) is the helper answering
//! normally: it surfaces as the query's error but NEVER tears the client
//! down ([`UiaHelperOutcome`]). The NDJSON readers bound ONE line at
//! [`MAX_LINE_BYTES`] in both directions (a fresh limit per read, so the
//! bound does not accrue across a session's traffic) — a longer or
//! delimiter-less chunk is framing corruption (transport death), not an
//! unbounded allocation. Lines cross the pipe WITH std's delimiter and are
//! decoded through [`decode_line`] before any comparison or parse — the
//! ready handshake's bare-constant comparison depends on that strip (r6).

use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::io::FromRawHandle;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::TerminateProcess;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, PROCESS_PER_MONITOR_DPI_AWARE,
    SetProcessDpiAwareness, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GUITHREADINFO, GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId,
};

use super::super::types::{ComputerUseError, ElementInfo, UiTreeOptions};
use super::helpers::{MAX_TREE_DEPTH, MAX_TREE_NODES, append_tree_truncation_footers, tree_budget};
use super::uia_helper_protocol::{Op, READY_LINE, Request, Response, WireElement, decode_line};
use super::virtual_display::windows_virtual::{
    TREE_TOP_LEVEL_CAP, deepest_window_at, desktop_target_path, is_shell_window_class,
    launch_suspended_in_job, walk_visible_toplevels, window_class_name,
};
use super::windows::{
    TreeWriter, build_tree_cache, element_info_from_cache, map_uia_err, write_tree_node,
};
use uiautomation::UIAutomation;

/// How long the parent waits for the helper's ready line (a fresh process
/// plus `UIAutomation::new` COM initialization — sub-second in practice,
/// with a wide margin for a cold, busy machine).
const HELPER_READY_TIMEOUT_MS: u64 = 10_000;
/// Per-request tree budget. The tree request's real bound is the node
/// budget scaled by the per-node COM fetch cost (see the timeout
/// composition in [`UiaHelperClient::tree`]); a flat timeout would strand
/// escalated-budget queries in a timeout/cooldown loop where only the
/// degraded answer ever lands. Point/focus are single-element reads.
const HELPER_TREE_TIMEOUT_BASE_MS: u64 = 20_000;
/// Per-node allowance on top of the base tree timeout (a bounded
/// cross-process UIA fetch is 5-20 ms on a busy desktop).
const HELPER_TREE_TIMEOUT_PER_NODE_MS: u64 = 20;
const HELPER_ELEMENT_TIMEOUT_MS: u64 = 5_000;
/// The between-node cancellation checkpoint of the tree walk, not wired over
/// the pipe: the node budget bounds the helper's work and the parent's
/// scaled timeout bounds the wait (see `helper_tree` below and the module
/// doc's residual note).
const HELPER_TREE_WALK_CANCEL: Option<&std::sync::atomic::AtomicBool> = None;
/// Framing containment on the NDJSON pipe, BOTH directions: one line is
/// bounded by [`MAX_LINE_BYTES`]. The honest bound math: a worst-case tree
/// row is ~0.4 KiB (depth indent + index + quoted 80-CHAR name — up to ~240
/// UTF-8 bytes for CJK — plus rect and flags), so the 2000-node ceiling
/// produces at most ~0.8 MiB and 1 MiB leaves ~20% headroom. A longer (or
/// delimiter-less) line means the framing is corrupt or hostile: the reader
/// stops and the client is torn down as a transport failure instead of
/// streaming an unbounded allocation into memory. The limit re-arms PER
/// LINE (a fresh `Take` per read, r6): `io::Take` bounds the TOTAL bytes
/// across the reader it wraps, so a reader-lifetime `take` would kill a
/// healthy helper once ~1 MiB of responses had crossed the pipe — several
/// full-budget trees into a session.
const MAX_LINE_BYTES: u64 = 1 << 20;

// ── helper-side loop (runs in the re-spawned process on the hidden desktop) ──

/// Match the parent's DPI awareness (Per-Monitor-V2, set by tao at
/// EventLoop creation — a step this process never reaches, since it
/// dispatches before any app init). Without this the helper's coordinate
/// space is DPI-virtualized while the parent's is physical: element_at_point
/// points and tree rects would diverge from click coordinates on any
/// display scaled ≠ 100% (the r4 review's coordinate-desync finding).
/// PMv2 (Win10 1703+, the Tauri floor) with tao's PMv1 fallback via
/// shcore; on a system old enough to fail both, the process stays unaware
/// and the real-machine retest list covers scaled displays.
fn become_dpi_aware_for_helper() {
    // SAFETY: process-wide awareness setting before any coordinate query in
    // this process; no handles or windows are involved.
    if unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }.is_ok()
    {
        return;
    }
    // SAFETY: see the fn contract (PMv1 is tao's documented fallback).
    let _ = unsafe { SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE) };
}

/// The helper's main: called from `run()` before any app initialization when
/// argv carries [`uia_helper_protocol::HELPER_ARG`]. Reads request lines from
/// stdin until EOF (the parent closed the write end), answering one response
/// line per request. Process exits 0 on EOF; the session job is the backstop
/// for anything harsher.
pub(crate) fn run_helper_loop() {
    // The coordinate contract must match the parent's BEFORE any query runs.
    become_dpi_aware_for_helper();
    // COM + UIA client init happens on the helper's main thread, which the
    // OS placed on the hidden desktop via lpDesktop — every desktop-relative
    // query below resolves there. `new` (NOT `new_direct`) is required here:
    // this fresh process has never initialized COM, and `new_direct` only
    // CoCreateInstance's — it would fail CO_E_NOTINITIALIZED (the crate
    // documents `new` as the initializing entry; the in-process physical
    // stack relies on the same contract).
    let uia = match UIAutomation::new() {
        Ok(uia) => uia,
        Err(error) => {
            // The parent's ready wait fails and it degrades; still emit a
            // structured line so logs carry the reason.
            emit_line(&format!(
                "{}",
                serde_json::json!({"ready": false, "error": error.to_string()})
            ));
            return;
        }
    };
    // Signal readiness BEFORE the first request can arrive (the parent waits
    // for this line with a deadline).
    emit_line(READY_LINE);

    let stdin = std::io::stdin();
    let mut raw = BufReader::new(stdin.lock());
    let mut stdout = std::io::stdout();
    loop {
        let mut raw_line = String::new();
        // Same per-line containment as the parent's reader (r6): a fresh
        // `Take` per read bounds ONE line — a reader-lifetime limit would
        // stop serving after ~1 MiB of well-framed requests — and a chunk
        // without its delimiter (over-cap or a dying parent) is framing
        // corruption: stop serving rather than grow the allocation.
        match (&mut raw).take(MAX_LINE_BYTES + 1).read_line(&mut raw_line) {
            Ok(0) => break, // stdin EOF: the parent dropped the write end
            Ok(_) => {
                let Some(line) = decode_line(&raw_line) else {
                    break; // delimiter-less chunk: corrupt framing
                };
                // A blank line is skipped without consuming an id.
                if line.trim().is_empty() {
                    continue;
                }
                let response = match serde_json::from_str::<Request>(&line) {
                    Ok(request) => handle_request(&uia, &request),
                    Err(error) => Response::error(0, format!("unparsable request: {error}")),
                };
                let payload = match serde_json::to_string(&response) {
                    Ok(payload) => payload,
                    // A response that cannot serialize is a bug, not churn:
                    // fail the loop rather than answer nothing (the parent
                    // times out and degrades either way).
                    Err(_) => break,
                };
                if emit_line_to(&mut stdout, &payload).is_err() {
                    break; // parent gone
                }
            }
            Err(_) => break, // stdin gone
        }
    }
    // stdin EOF: the parent dropped the write end (client dropped). Exit
    // cleanly; the job would kill us at session end regardless.
}

fn handle_request(uia: &UIAutomation, request: &Request) -> Response {
    match &request.op {
        Op::Tree {
            max_depth,
            max_nodes,
        } => helper_tree(uia, *max_depth, *max_nodes)
            .map(|tree| Response::tree(request.id, tree))
            .unwrap_or_else(|error| Response::error(request.id, error.to_string())),
        Op::AtPoint { x, y } => helper_at_point(uia, *x, *y)
            .map(|element| Response::element(request.id, element))
            .unwrap_or_else(|error| Response::error(request.id, error.to_string())),
        Op::Focus => helper_focus(uia)
            .map(|element| Response::element(request.id, element))
            .unwrap_or_else(|error| Response::error(request.id, error.to_string())),
    }
}

/// The UIA tree over the hidden desktop's visible top-level windows (shell
/// excluded, the same cap and exclusion the fallback walk applies — the two
/// answers must describe the same desktop). Physical-stack serialization
/// ([`write_tree_node`]) so the model reads one format across all three
/// stacks. Residual (accepted): the request is atomic over the pipe, so the
/// between-node cancellation checkpoint is not wired here (None) — the node
/// budget bounds the helper's work and the parent's timeout bounds the wait.
fn helper_tree(
    uia: &UIAutomation,
    max_depth: u32,
    max_nodes: u32,
) -> Result<String, ComputerUseError> {
    // Defense-in-depth re-clamp (r5): the parent clamps before sending, but
    // this helper must not trust the wire — a future second client of the
    // protocol would otherwise bypass the shared ceilings. Same constants as
    // every other stack's budget.
    let (max_depth, max_nodes) = (max_depth.min(MAX_TREE_DEPTH), max_nodes.min(MAX_TREE_NODES));
    // SAFETY: desktop-relative Z-order enumeration on the helper's own
    // (hidden) desktop. Shell windows are excluded BEFORE they can consume
    // a top-level cap slot (the same predicate the fallback walk applies —
    // the two answers must describe the same desktop).
    let (roots, top_cap_bound) = unsafe {
        // SAFETY (closure body): the class-name query on a handle from the
        // enumeration above runs inside the walk's unsafe block — the
        // syntactic nesting covers it, so no inner `unsafe` is needed.
        walk_visible_toplevels(TREE_TOP_LEVEL_CAP, &|hwnd| {
            is_shell_window_class(&window_class_name(hwnd))
        })
    };
    let cache = build_tree_cache(uia)?;
    let mut writer = TreeWriter {
        next_index: 0,
        remaining: max_nodes,
        truncated: false,
        truncated_by_depth: false,
    };
    let mut out = String::new();
    for root in roots {
        // A root that died between the Z-order enumeration and the handle
        // bind is branch-end churn (the physical writer's rule): skip it
        // instead of failing the whole query — the walk fans in over up to
        // the whole top-level cap, so windows closing mid-enumeration would
        // otherwise make ui_tree flaky at exactly the moments windows close
        // (r4). A walk failure deeper in the tree still propagates.
        let Ok(element) = uia.element_from_handle(root.into()) else {
            continue;
        };
        write_tree_node(
            &element,
            &cache,
            0,
            max_depth,
            &mut writer,
            &mut out,
            HELPER_TREE_WALK_CANCEL,
        )?;
    }
    append_tree_truncation_footers(
        writer.truncated,
        writer.truncated_by_depth,
        top_cap_bound,
        writer.next_index,
        &mut out,
    );
    Ok(out)
}

/// The element at a screen point: the parent's own hit-test semantics
/// ([`deepest_window_at`], desktop-relative) scope the window, UIA only
/// names it — deliberately NOT `element_from_point`, which would resolve
/// against the input desktop.
fn helper_at_point(
    uia: &UIAutomation,
    x: i32,
    y: i32,
) -> Result<Option<WireElement>, ComputerUseError> {
    // SAFETY: desktop-relative window query on the helper's own desktop.
    let Some((target, _, _)) = (unsafe { deepest_window_at(x, y) }) else {
        return Ok(None); // a confirmable "nothing there", not an error
    };
    let element = uia
        .element_from_handle(target.into())
        .map_err(|e| map_uia_err("element_at_point", e))?;
    // SAFETY: cache request and element live for the call.
    let cache = build_tree_cache(uia)?;
    let info = element_info_from_handle_cache(&element, &cache, "element_at_point")?;
    Ok(Some(info))
}

/// The focused control of the hidden desktop's foreground window: the
/// foreground + thread-focus pair is desktop-relative, and the HWND maps to
/// one UIA element — no `get_focused_element` (input-desktop poison).
fn helper_focus(uia: &UIAutomation) -> Result<Option<WireElement>, ComputerUseError> {
    // SAFETY: desktop-relative focus/foreground queries on the helper's own
    // desktop.
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.is_invalid() || is_shell_window_class(&window_class_name(foreground)) {
            // The blank-start shell or a desktop without foreground: the
            // parent MUST treat this as uncertain (it translates to Err and
            // masks the confirm preview) — never as a confirmed "nothing".
            return Err(ComputerUseError::unavailable(
                "no app window on the hidden desktop holds keyboard focus",
            ));
        }
        let thread = GetWindowThreadProcessId(foreground, None);
        if thread == 0 {
            return Err(ComputerUseError::unavailable(
                "the foreground window's thread could not be identified",
            ));
        }
        let mut info = GUITHREADINFO::default();
        info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
        if !GetGUIThreadInfo(thread, &mut info).is_ok() || info.hwndFocus.is_invalid() {
            return Err(ComputerUseError::unavailable(
                "the focused control cannot be identified (no focused control on \
                 the foreground window's thread)",
            ));
        }
        let element = uia
            .element_from_handle(info.hwndFocus.into())
            .map_err(|e| map_uia_err("focused_element", e))?;
        let cache = build_tree_cache(uia)?;
        let wire = element_info_from_handle_cache(&element, &cache, "focused_element")?;
        Ok(Some(wire))
    }
}

/// ElementInfo fields as the wire shape — computed HERE where the raw cached
/// name still exists (screening + binding), transported display-truncated.
fn element_info_from_handle_cache(
    element: &uiautomation::UIElement,
    cache: &uiautomation::core::UICacheRequest,
    context: &str,
) -> Result<WireElement, ComputerUseError> {
    let filled = element
        .build_updated_cache(cache)
        .map_err(|e| map_uia_err(&format!("{context} cache"), e))?;
    let info = element_info_from_cache(context, &filled)?;
    Ok(WireElement {
        role: info.role,
        name: info.name,
        x: info.x,
        y: info.y,
        width: info.width,
        height: info.height,
        secure: info.secure,
        screening_hit: info.name_screening_hit,
        binding: info.raw_binding,
    })
}

fn emit_line(line: &str) {
    let mut stdout = std::io::stdout();
    let _ = emit_line_to(&mut stdout, line);
}

fn emit_line_to(stdout: &mut impl Write, line: &str) -> std::io::Result<()> {
    stdout.write_all(line.as_bytes())?;
    stdout.write_all(b"\n")?;
    stdout.flush()
}

// ── parent-side client (runs on the backend's attached worker) ──────────────

/// Transport-shaped failure: the helper could not be talked to at all. The
/// caller drops the client and degrades; a SOFT verdict (`ok: false`) is
/// NOT this — that is the helper answering normally, carried as
/// [`UiaHelperOutcome::Soft`] so the type system keeps the two apart.
#[derive(Debug)]
pub(crate) enum HelperTransportError {
    Spawn(ComputerUseError),
    Timeout,
    BrokenPipe,
    Unparsable(String),
}

/// One op's outcome from a HEALTHY helper. `Ok` carries the answer; `Soft`
/// carries the helper's honest "cannot answer" verdict (e.g. nothing on the
/// hidden desktop holds focus — the DEFAULT state of a fresh desktop, not a
/// fault). Only [`HelperTransportError`] kills the client: a soft verdict
/// that dropped the helper would respawn a perfectly good process on every
/// cooldown lapse (and the focus verdict rides the keystroke path).
#[derive(Debug)]
pub(crate) enum UiaHelperOutcome<T> {
    Ok(T),
    Soft(String),
}

pub(crate) struct UiaHelperClient {
    /// The write end, kept in an Option so Drop can close it (stdin EOF is
    /// the helper's clean-exit signal); a File itself cannot be moved out of
    /// `&mut self`.
    stdin: Option<std::fs::File>,
    reader: Option<Receiver<String>>,
    reader_thread: Option<JoinHandle<()>>,
    next_id: u64,
    /// The spawn process handle, RETAINED so Drop can terminate a helper a
    /// pipe EOF cannot reach (one wedged in a cross-process UIA COM call
    /// never returns to its read loop — the handle is the only mid-session
    /// kill channel; the session-end job remains the backstop for the
    /// backend's own drop).
    process: HANDLE,
}

impl UiaHelperClient {
    /// Spawn this executable as the helper on `desktop_name` (the session
    /// desktop), inside `job`. The child's stdin/stdout are the pipe ends;
    /// the parent closes ITS copies of the child ends right after spawn so
    /// EOF reaches the helper when the client drops.
    pub(crate) fn spawn(desktop_name: &str, job: HANDLE) -> Result<Self, HelperTransportError> {
        let exe = std::env::current_exe().map_err(|e| {
            HelperTransportError::Spawn(ComputerUseError::failed(format!(
                "cannot resolve this executable for the UIA helper: {e}"
            )))
        })?;
        let command_line = format!(
            "\"{}\" {}",
            exe.display(),
            super::uia_helper_protocol::HELPER_ARG
        );
        let exe_wide = to_wide(&exe.display().to_string());
        let desktop_wide = to_wide(&desktop_target_path(desktop_name));
        // Pipe 1: the child reads its stdin end (inheritable); pipe 2: the
        // child writes its stdout end (inheritable). The parent keeps the
        // opposite ends — converted to owned Files IMMEDIATELY, and each
        // child end is registered in the guard AS ITS PIPE IS CREATED, so
        // ANY failure after the first pipe (including the second pipe's own
        // creation failing) closes exactly the ends made so far — the r4
        // leak window between the two pipe creations is gone (a raw HANDLE
        // in a bare local would leak on that path).
        let mut child_ends = ChildEndsGuard::default();
        let (child_stdin_read, parent_stdin_write) =
            unsafe { make_inheritable_pipe(true) }.map_err(HelperTransportError::Spawn)?;
        let stdin = handle_to_file(parent_stdin_write);
        child_ends.push(child_stdin_read);
        let (parent_stdout_read, child_stdout_write) =
            unsafe { make_inheritable_pipe(false) }.map_err(HelperTransportError::Spawn)?;
        let read_file = handle_to_file(parent_stdout_read);
        child_ends.push(child_stdout_write);
        // The child ends stay open across the launch: the guard closes them
        // when the launch FAILS (all four pipe ends close on exactly the
        // paths the launcher anticipates — packaged/job-refused spawns,
        // CreateProcessW failures), and is dropped explicitly after a
        // successful launch so EOF reaches the helper when the client drops
        // (the helper sees EOF only when every write-end duplicate is gone).
        // SAFETY: contained suspended launch on the hidden desktop with the
        // std-handle triple (stderr is left null: a Rust panic message in
        // the helper would otherwise ride the NDJSON stream and corrupt the
        // framing — losing the panic text is the better trade, and the
        // parent logs the degrade reason it does see).
        let (_pid, process) = unsafe {
            launch_suspended_in_job(
                desktop_wide,
                Some(&exe_wide),
                &command_line,
                job,
                Some([child_stdin_read, child_stdout_write, HANDLE::default()]),
            )
        }
        .map_err(HelperTransportError::Spawn)?;
        // The spawn handle is RETAINED (r5): Drop terminates the helper
        // deterministically, so a helper wedged inside one uninterruptible
        // cross-process UIA call cannot outlive the client (with this
        // reader thread) until the session-end job.
        drop(child_ends);

        let (tx, rx) = channel::<String>();
        // The client is assembled BEFORE the reader thread spawns (r6):
        // the thread spawn is the one failure path left after a successful
        // launch, and constructing first means the `?` below drops the
        // client — whose Drop terminates and closes the retained spawn
        // handle — instead of leaking one kernel handle per occurrence
        // (the raw-HANDLE-in-a-bare-local class the r4 guard closed for
        // the pipe ends).
        let mut client = Self {
            stdin: Some(stdin),
            reader: Some(rx),
            reader_thread: None,
            next_id: 1,
            process,
        };
        let reader_thread = std::thread::Builder::new()
            .name("cu-uia-helper-reader".to_string())
            .spawn(move || {
                // Per-line framing containment (r6): `io::Take` bounds the
                // TOTAL bytes across the reader it wraps, so a
                // reader-lifetime `take` (the r5 shape) would kill a
                // healthy helper once ~1 MiB of responses had crossed the
                // pipe — the fresh limit PER read bounds ONE line. A chunk
                // that arrives without its newline is framing corruption —
                // stop instead of streaming the rest of an unbounded line
                // into memory, and never hand a delimiter-less chunk to a
                // caller (see `decode_line`).
                let mut raw = BufReader::new(read_file);
                loop {
                    let mut line = String::new();
                    match (&mut raw).take(MAX_LINE_BYTES + 1).read_line(&mut line) {
                        Ok(0) => break, // EOF: the helper exited
                        Ok(_) => {
                            let Some(line) = decode_line(&line) else {
                                break; // delimiter-less chunk: corrupt framing
                            };
                            if tx.send(line).is_err() {
                                break; // client dropped
                            }
                        }
                        Err(_) => break, // pipe broken
                    }
                }
                // Dropping `tx` lets recv fail fast for any waiting caller.
            })
            .map_err(|e| {
                HelperTransportError::Spawn(ComputerUseError::failed(format!(
                    "cannot spawn the UIA helper reader thread: {e}"
                )))
            })?;
        client.reader_thread = Some(reader_thread);
        client.wait_ready()?;
        Ok(client)
    }

    /// Bounded wait for the ready line (a stray early response line would
    /// be protocol corruption — treated as failure). Both arms terminate:
    /// the ready line ends the wait, anything else is untrustworthy
    /// framing — there is no third case to loop on. `take_line` yields
    /// DELIMITER-STRIPPED lines (`decode_line` in the reader thread), so
    /// the bare-constant comparison is exact — the r6 handshake failure
    /// compared a newline-inclusive line against this constant and failed
    /// forever; the framing contract is pinned Linux-side in
    /// `line_decoding_strips_the_delimiter_and_rejects_unframed_chunks`.
    fn wait_ready(&mut self) -> Result<(), HelperTransportError> {
        let remaining = Duration::from_millis(HELPER_READY_TIMEOUT_MS);
        match self.take_line(remaining)? {
            line if line == READY_LINE => Ok(()),
            other => Err(HelperTransportError::Unparsable(format!(
                "expected the ready line, got: {other}"
            ))),
        }
    }

    /// One line from the reader thread, delimiter already stripped
    /// (`decode_line`). The channel carries decoded lines so every
    /// consumer (this ready compare, `exchange`'s parse and id check)
    /// sees the same clean shape.
    fn take_line(&mut self, timeout: Duration) -> Result<String, HelperTransportError> {
        let rx = self
            .reader
            .as_mut()
            .ok_or(HelperTransportError::BrokenPipe)?;
        match rx.recv_timeout(timeout) {
            Ok(line) => Ok(line),
            Err(RecvTimeoutError::Timeout) => Err(HelperTransportError::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(HelperTransportError::BrokenPipe),
        }
    }

    /// One serialized request/response exchange. Transport failures carry
    /// the helper-death semantics (caller drops the client); a well-framed
    /// `ok: false` is returned as-is for the caller to translate.
    fn exchange(&mut self, op: Op, timeout: Duration) -> Result<Response, HelperTransportError> {
        let id = self.next_id;
        self.next_id += 1;
        let line = serde_json::to_string(&Request { id, op })
            .map_err(|e| HelperTransportError::Unparsable(format!("request serialize: {e}")))?;
        // Writes are small (< pipe buffer) and the helper reads eagerly; a
        // blocking write failing IS the broken-pipe signal.
        let stdin = self
            .stdin
            .as_mut()
            .ok_or(HelperTransportError::BrokenPipe)?;
        stdin
            .write_all(line.as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .and_then(|_| stdin.flush())
            .map_err(|_| HelperTransportError::BrokenPipe)?;
        let line = self.take_line(timeout)?;
        let response: Response = serde_json::from_str(&line)
            .map_err(|e| HelperTransportError::Unparsable(format!("response parse: {e}")))?;
        if response.id != id {
            // Framing corruption: never answer a query with another query's
            // response.
            return Err(HelperTransportError::Unparsable(format!(
                "response id {} does not match request id {id}",
                response.id
            )));
        }
        Ok(response)
    }

    /// The helper's UIA tree. The wait budget scales with the node budget
    /// (each node is a bounded cross-process COM fetch — 5-20 ms on a busy
    /// desktop — so a max-budget walk can legitimately run tens of
    /// seconds; the base only has to cover spawn-adjacent latency).
    pub(crate) fn tree(
        &mut self,
        opts: &UiTreeOptions,
    ) -> Result<UiaHelperOutcome<String>, HelperTransportError> {
        let (max_depth, max_nodes) = tree_budget(opts);
        let timeout = Duration::from_millis(
            HELPER_TREE_TIMEOUT_BASE_MS + u64::from(max_nodes) * HELPER_TREE_TIMEOUT_PER_NODE_MS,
        );
        match self.exchange(
            Op::Tree {
                max_depth,
                max_nodes,
            },
            timeout,
        ) {
            Ok(Response {
                ok: true,
                tree: Some(tree),
                ..
            }) => Ok(UiaHelperOutcome::Ok(tree)),
            Ok(Response {
                ok: false,
                error: Some(error),
                ..
            }) => Ok(UiaHelperOutcome::Soft(error)),
            // A malformed frame is TRANSPORT corruption: the helper we
            // cannot trust framing from is a helper we cannot query.
            Ok(_) => Err(HelperTransportError::Unparsable(
                "the tree response carried no tree and no error".to_string(),
            )),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn at_point(
        &mut self,
        x: i32,
        y: i32,
    ) -> Result<UiaHelperOutcome<Option<ElementInfo>>, HelperTransportError> {
        match self.exchange(
            Op::AtPoint { x, y },
            Duration::from_millis(HELPER_ELEMENT_TIMEOUT_MS),
        ) {
            Ok(Response {
                ok: true, element, ..
            }) => Ok(UiaHelperOutcome::Ok(element.map(wire_to_info))),
            Ok(Response {
                ok: false,
                error: Some(error),
                ..
            }) => Ok(UiaHelperOutcome::Soft(error)),
            Ok(_) => Err(HelperTransportError::Unparsable(
                "the point response carried no element and no error".to_string(),
            )),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn focus(
        &mut self,
    ) -> Result<UiaHelperOutcome<Option<ElementInfo>>, HelperTransportError> {
        match self.exchange(Op::Focus, Duration::from_millis(HELPER_ELEMENT_TIMEOUT_MS)) {
            // Focus NEVER answers Ok(None): an unidentified focus must
            // surface as the query's error (the tool layer masks the
            // confirm preview on it), because a confirmed "nothing holds
            // focus" would clear the uncertain-focus mask and let an
            // unreadable target type unmasked. `element: None` is
            // meaningful for at_point only — for focus it is a malformed
            // frame (the helper answers focus queries with Some or an
            // error, so a Some(ok) frame without an element is a helper
            // regression, treated as transport corruption like the tree
            // op's "no tree and no error" arm).
            Ok(Response {
                ok: true,
                element: Some(element),
                ..
            }) => Ok(UiaHelperOutcome::Ok(Some(wire_to_info(element)))),
            Ok(Response {
                ok: true,
                element: None,
                ..
            }) => Err(HelperTransportError::Unparsable(
                "the focus response carried no element despite ok — focus \
                 uncertainty must surface as a soft/error verdict, never a \
                 confirmed none"
                    .to_string(),
            )),
            Ok(Response {
                ok: false,
                error: Some(error),
                ..
            }) => Ok(UiaHelperOutcome::Soft(error)),
            Ok(_) => Err(HelperTransportError::Unparsable(
                "the focus response carried no element and no error".to_string(),
            )),
            Err(error) => Err(error),
        }
    }
}

impl Drop for UiaHelperClient {
    fn drop(&mut self) {
        // Close the write end FIRST: stdin EOF is the helper's clean-exit
        // signal for the healthy case. Then terminate deterministically
        // (r5): a helper wedged inside one uninterruptible cross-process
        // UIA COM call never returns to its read loop, so EOF alone would
        // strand it — with this reader thread — until the session-end job,
        // one process per transport event. Terminating on drop leaves zero
        // residue on every drop path (transport death, respawn after
        // cooldown, and the backend's session-end drop — the job would end
        // it at teardown anyway). The reader thread detaches and exits on
        // the pipe death the terminate causes.
        drop(self.stdin.take());
        self.reader = None;
        if !self.process.is_invalid() {
            // SAFETY: terminate + close of our own retained spawn handle
            // (PROCESS_TERMINATE right is part of CreateProcessW's return).
            let _ = unsafe { TerminateProcess(self.process, 1) };
            let _ = unsafe { CloseHandle(self.process) };
        }
    }
}

fn wire_to_info(wire: WireElement) -> ElementInfo {
    ElementInfo {
        role: wire.role,
        name_screening_hit: wire.screening_hit,
        raw_binding: wire.binding,
        name: wire.name,
        x: wire.x,
        y: wire.y,
        width: wire.width,
        height: wire.height,
        secure: wire.secure,
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// One anonymous pipe whose READ end (distinguished by `read_end`) is
/// inheritable. SAFETY: pipe creation + flag set on handles this function
/// created; the caller wires them into the child's std handles.
unsafe fn make_inheritable_pipe(read_end: bool) -> Result<(HANDLE, HANDLE), ComputerUseError> {
    let mut read: HANDLE = HANDLE::default();
    let mut write: HANDLE = HANDLE::default();
    // SAFETY: out-pointers are valid locals; default security; default size.
    unsafe { CreatePipe(&mut read, &mut write, None, 0) }.map_err(|error| {
        ComputerUseError::failed(format!("cannot create the UIA helper pipe: {error}"))
    })?;
    let inherit = if read_end { read } else { write };
    // SAFETY: flag set on our own newly created handle.
    unsafe { SetHandleInformation(inherit, HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT) }.map_err(
        |error| {
            // SAFETY: close of our own pipe ends on the failure path (an
            // unsafe fn body still requires the explicit block under the
            // rust-2024 compatibility gate).
            let _ = unsafe { CloseHandle(read) };
            let _ = unsafe { CloseHandle(write) };
            ComputerUseError::failed(format!(
                "cannot mark the UIA helper pipe end inheritable: {error}"
            ))
        },
    )?;
    Ok((read, write))
}

fn handle_to_file(handle: HANDLE) -> std::fs::File {
    // SAFETY: `handle` is an owned pipe end created by make_inheritable_pipe;
    // ownership moves into the File exclusively (the raw value is not used
    // again on this side).
    unsafe { std::fs::File::from_raw_handle(handle.0 as _) }
}

/// The parent's duplicates of the child pipe ends, closed on drop. Ends are
/// pushed as each pipe is created (r4: closing the window where a second
/// pipe's creation failure would leak the first pipe's ends), so any
/// failure after the first pipe closes exactly the ends made so far. After
/// a SUCCESSFUL launch the caller drops the guard deliberately: closing the
/// parent's duplicates is what lets stdin EOF reach the helper when the
/// client drops.
#[derive(Default)]
struct ChildEndsGuard(Vec<HANDLE>);

impl ChildEndsGuard {
    fn push(&mut self, handle: HANDLE) {
        self.0.push(handle);
    }
}

impl Drop for ChildEndsGuard {
    fn drop(&mut self) {
        for handle in self.0.drain(..) {
            // SAFETY: close of our own pipe-end duplicates.
            let _ = unsafe { CloseHandle(handle) };
        }
    }
}

#[cfg(test)]
mod tests {
    /// The helper's coordinate contract rides on matching the parent's DPI
    /// awareness BEFORE any query runs: a revert that drops the call makes
    /// element_at_point points and tree rects DPI-virtualized (unaware
    /// process) while the parent's click coordinates stay physical —
    /// silently wrong answers on every display scaled ≠ 100% (r4 M2).
    /// Source-counted: no CI seam can run a hidden desktop, and DPI state
    /// has no observable test seam; the Windows CI leg type-checks the call
    /// and the real-machine retest list covers scaled displays.
    #[test]
    fn helper_dpi_awareness_stays_wired() {
        let src = include_str!("uia_helper.rs");
        let production = src.split("mod tests {").next().unwrap_or("");
        assert_eq!(
            production.matches("become_dpi_aware_for_helper()").count(),
            2,
            "definition + the first call in the loop: deleting the call \
             desyncs the helper's coordinate space from the parent's on \
             scaled displays"
        );
        // The spawn's std-handle triple keeps stderr NULL (r5 pin of the
        // r3 NDJSON-corruption fix): a revert that passes the parent's
        // real stdout/stderr lets panic or tracing text ride the NDJSON
        // stream and corrupt the framing — every other gate stays green.
        assert!(
            production.contains("Some([child_stdin_read, child_stdout_write, HANDLE::default()])"),
            "the helper spawn must pass the null-stderr std-handle triple"
        );
        // The wire→ElementInfo mapping must NOT be free to swap its bools
        // (r5): `secure` and `name_screening_hit` are both bools, a swap
        // compiles and passes every other gate, and the T3 confirm preview
        // would then screen on the password flag while treating a screened
        // name as secure.
        assert_eq!(
            production
                .matches("name_screening_hit: wire.screening_hit")
                .count(),
            1,
            "the wire→ElementInfo mapping must carry the screening verdict \
             from the wire's screening field exactly once"
        );
        assert_eq!(
            production.matches("secure: wire.secure").count(),
            1,
            "the wire→ElementInfo mapping must carry the secure flag from \
             the wire's secure field exactly once"
        );
        // The client retains the spawn handle and terminates it on drop
        // (r5): a helper wedged in a cross-process UIA COM call never sees
        // EOF — without the retained handle it (and this reader thread)
        // lingers until the session-end job, one per transport event.
        assert_eq!(
            production
                .matches("TerminateProcess(self.process, 1)")
                .count(),
            1,
            "the client Drop must terminate the retained spawn handle: EOF \
             alone cannot reach a helper wedged inside a UIA COM call"
        );
        // The NDJSON line cap travels PER LINE (r6): `io::Take` bounds the
        // TOTAL bytes across the reader it wraps, so the r5 reader-lifetime
        // `take` killed a healthy helper once ~1 MiB of responses had
        // crossed the pipe (a handful of full-budget trees into a session)
        // while every gate stayed green. Both directions re-arm the limit
        // per read — the parent's response reader AND the helper's request
        // loop — and route every line through `decode_line`, which strips
        // the delimiter std delivers (the r6 ready-handshake BLOCKER was a
        // newline-inclusive line compared against the bare READY_LINE
        // constant; the framing behavior itself is pinned Linux-side in the
        // protocol file's decode test).
        assert_eq!(
            production
                .matches("(&mut raw).take(MAX_LINE_BYTES + 1)")
                .count(),
            2,
            "parent reader + helper request loop: each read re-arms the \
             per-line bound; collapsing to a reader-lifetime take (the r5 \
             shape) changes the token or the count and fails here"
        );
        assert_eq!(
            production.matches("decode_line(&").count(),
            2,
            "both loops must decode the std-delivered line before sending \
             it on — the bare-constant ready comparison and every JSON \
             parse depend on the delimiter strip"
        );
        // The client is assembled before the reader thread spawns (r6): a
        // reader-thread spawn failure then drops the client, whose Drop
        // terminates the retained spawn handle — the construct-after shape
        // leaked one kernel handle per failed spawn.
        assert_eq!(
            production
                .matches("client.reader_thread = Some(reader_thread);")
                .count(),
            1,
            "client-before-reader-thread ordering: the spawn-failure path \
             must run the client's Drop (terminate + close the retained \
             handle)"
        );
        // The helper's tree walk shares the fallback's shell exclusion
        // (r6 pin): the exclusion predicate itself is pinned in
        // windows_virtual.rs, but the HELPER's use of it — the primary
        // structured-query path — had no pin, so deleting it would
        // green-light shell windows crowding every helper tree while all
        // gates stayed green.
        assert_eq!(
            production
                .matches("is_shell_window_class(&window_class_name(hwnd))")
                .count(),
            1,
            "the helper's top-level walk must apply the shared \
             shell-exclusion predicate exactly once (the fallback walk's \
             use is pinned in windows_virtual.rs; the two answers must \
             describe the same desktop)"
        );
        // The tree wait must keep scaling with the node budget (r6 pin):
        // each node is a bounded cross-process COM fetch, so a flat
        // timeout desyncs the parent's wait from the walk budget and
        // strands escalated-budget queries in timeout/cooldown loops.
        assert_eq!(
            production
                .matches(
                    "HELPER_TREE_TIMEOUT_BASE_MS + u64::from(max_nodes) * \
                     HELPER_TREE_TIMEOUT_PER_NODE_MS"
                )
                .count(),
            1,
            "the tree wait budget must scale per node (the formula token \
             is single-line, CRLF-safe)"
        );
        // The helper serializes through the shared footer builder (r6
        // pin): a revert to inline literals would fork the payload format
        // across stacks (the builder's bytes are golden-pinned in
        // helpers.rs; windows.rs's routing is pinned in its own file).
        assert_eq!(
            production
                .matches("append_tree_truncation_footers(")
                .count(),
            1,
            "the helper tree must emit its truncation footers through the \
             shared builder — one format across the three stacks"
        );
    }
}

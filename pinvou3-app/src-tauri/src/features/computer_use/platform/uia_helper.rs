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
//! (with a respawn cooldown after any transport failure), degrades to the
//! phase-2 window-class walk whenever the helper is unavailable, and closes
//! the write pipe on drop — the helper exits on stdin EOF, with the
//! kill-on-close job as the backstop. The helper is session INFRASTRUCTURE
//! (like the shell): it is not in the wind-down list, so Stop/revoke leaves
//! it alive for a re-granted session.

// The helper's tree footer serializes with `writeln!` into a `String` —
// the trait import the physical stack's writer carries (this file compiles
// only on Windows, where no Linux gate would catch the missing import).
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::io::FromRawHandle;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, HWND, SetHandleInformation,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::STARTF_USESTDHANDLES;
use windows::Win32::UI::WindowsAndMessaging::{
    GUITHREADINFO, GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId,
};

use super::super::super::types::{ComputerUseError, ElementInfo, UiTreeOptions};
use super::uia_helper_protocol::{Op, READY_LINE, Request, Response, WireElement};
use super::virtual_display::windows_virtual::{
    TREE_TOP_LEVEL_CAP, deepest_window_at, is_shell_window_class, launch_suspended_in_job,
    walk_visible_toplevels, window_class_name,
};
use super::windows::{
    TreeWriter, build_tree_cache, element_info_from_cache, map_uia_err, write_tree_node,
};
use uiautomation::UIAutomation;

/// How long the parent waits for the helper's ready line (a fresh process
/// plus `UIAutomation::new_direct` COM init — sub-second in practice, with
/// a wide margin for a cold, busy machine).
const HELPER_READY_TIMEOUT_MS: u64 = 10_000;
/// Per-request budget. The tree request's real bound is the node budget
/// (each node is a bounded COM fetch inside the helper); this timeout only
/// catches a wedged helper. Point/focus are single-element reads.
const HELPER_TREE_TIMEOUT_MS: u64 = 20_000;
const HELPER_ELEMENT_TIMEOUT_MS: u64 = 5_000;
/// Minimum spacing between spawn ATTEMPTS after a failure: a broken
/// environment (helper exe missing, pipe creation refused) must not turn
/// every query into a fresh process storm.
const HELPER_RESPAWN_COOLDOWN_MS: u64 = 5_000;
/// The helper's argv is built from `current_exe` only (no user input), and
/// the session job (kill-on-close) bounds its lifetime.
const HELPER_TREE_WALK_CANCEL: Option<&std::sync::atomic::AtomicBool> = None;

// ── helper-side loop (runs in the re-spawned process on the hidden desktop) ──

/// The helper's main: called from `run()` before any app initialization when
/// argv carries [`uia_helper_protocol::HELPER_ARG`]. Reads request lines from
/// stdin until EOF (the parent closed the write end), answering one response
/// line per request. Process exits 0 on EOF; the session job is the backstop
/// for anything harsher.
pub(crate) fn run_helper_loop() {
    // COM + UIA client init happens on the helper's main thread, which the
    // OS placed on the hidden desktop via lpDesktop — every desktop-relative
    // query below resolves there.
    let uia = match UIAutomation::new_direct() {
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
    let mut requests = BufReader::new(stdin.lock()).lines();
    let mut stdout = std::io::stdout();
    while let Some(Ok(line)) = requests.next() {
        // EOF simply ends this loop (the parent closed the write end); a
        // blank line is skipped without consuming an id.
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => handle_request(&uia, &request),
            Err(error) => Response::error(0, format!("unparsable request: {error}")),
        };
        let payload = match serde_json::to_string(&response) {
            Ok(payload) => payload,
            // A response that cannot serialize is a bug, not churn: fail the
            // loop rather than answer nothing (the parent times out and
            // degrades either way).
            Err(_) => break,
        };
        if emit_line_to(&mut stdout, &payload).is_err() {
            break; // parent gone
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
    // SAFETY: desktop-relative Z-order enumeration on the helper's own
    // (hidden) desktop.
    let roots: Vec<HWND> = unsafe { walk_visible_toplevels(TREE_TOP_LEVEL_CAP) }
        .into_iter()
        // SAFETY: class-name query on a handle from the enumeration above,
        // on the helper's own (hidden) desktop.
        .filter(|root| !is_shell_window_class(&unsafe { window_class_name(*root) }))
        .collect();
    let cache = build_tree_cache(uia)?;
    let mut writer = TreeWriter {
        next_index: 0,
        remaining: max_nodes,
        truncated: false,
        truncated_by_depth: false,
    };
    let mut out = String::new();
    for root in roots {
        let element = uia
            .element_from_handle(root.into())
            .map_err(|e| map_uia_err("ui_tree root", e))?;
        // SAFETY: cache owns the interface for the duration of the walk.
        unsafe {
            write_tree_node(
                &element,
                &cache,
                0,
                max_depth,
                &mut writer,
                &mut out,
                HELPER_TREE_WALK_CANCEL,
            )?
        };
    }
    if writer.truncated {
        let _ = writeln!(
            out,
            "... (truncated at {} nodes; pass a larger max_nodes to see more)",
            writer.next_index
        );
    }
    if writer.truncated_by_depth {
        let _ = writeln!(
            out,
            "... (truncated by depth; pass a larger max_depth to see more)"
        );
    }
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
    let info = element_info_from_handle_cache(uia, &element, &cache, "element_at_point")?;
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
        let wire = element_info_from_handle_cache(uia, &element, &cache, "focused_element")?;
        Ok(Some(wire))
    }
}

/// ElementInfo fields as the wire shape — computed HERE where the raw cached
/// name still exists (screening + binding), transported display-truncated.
fn element_info_from_handle_cache(
    uia: &UIAutomation,
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
/// caller drops the client and degrades; a SOFT error (`ok: false`) is NOT
/// this — that is the helper answering normally.
#[derive(Debug)]
pub(crate) enum HelperTransportError {
    Spawn(ComputerUseError),
    Timeout,
    BrokenPipe,
    Unparsable(String),
}

pub(crate) struct UiaHelperClient {
    /// The write end, kept in an Option so Drop can close it (stdin EOF is
    /// the helper's clean-exit signal); a File itself cannot be moved out of
    /// `&mut self`.
    stdin: Option<std::fs::File>,
    reader: Option<Receiver<String>>,
    reader_thread: Option<JoinHandle<()>>,
    next_id: u64,
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
        let cmdline_wide = to_wide(&command_line);
        let desktop_wide = to_wide(&format!("WinSta0\\{desktop_name}"));
        // SAFETY: pipe creation with default security; the inherit flags are
        // set on exactly the child ends below.
        // Pipe 1: the child reads its stdin end (inheritable); pipe 2: the
        // child writes its stdout end (inheritable). The parent keeps the
        // opposite ends.
        let (child_stdin_read, parent_stdin_write) =
            unsafe { make_inheritable_pipe(true) }.map_err(HelperTransportError::Spawn)?;
        let (parent_stdout_read, child_stdout_write) =
            unsafe { make_inheritable_pipe(false) }.map_err(HelperTransportError::Spawn)?;
        // SAFETY: contained suspended launch on the hidden desktop with the
        // std-handle triple; the launcher closes/frees nothing of ours.
        let (_pid, _process) = unsafe {
            launch_suspended_in_job(
                desktop_wide,
                Some(&exe_wide),
                &command_line,
                job,
                Some([child_stdin_read, child_stdout_write, child_stdout_write]),
            )
        }
        .map_err(|e| HelperTransportError::Spawn(e))?;
        // Our copies of the CHILD's ends must close now: the helper sees EOF
        // only when every write-end duplicate is gone. From here the parent
        // holds `parent_stdin_write` (write) and `parent_stdout_read` (read).
        drop(child_ends_guard(child_stdin_read, child_stdout_write));

        let stdin = handle_to_file(parent_stdin_write);
        let read_file = handle_to_file(parent_stdout_read);
        let (tx, rx) = channel::<String>();
        let reader_thread = std::thread::Builder::new()
            .name("cu-uia-helper-reader".to_string())
            .spawn(move || {
                let reader = BufReader::new(read_file);
                for line in reader.lines() {
                    match line {
                        Ok(line) => {
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
        let mut client = Self {
            stdin: Some(stdin),
            reader: Some(rx),
            reader_thread: Some(reader_thread),
            next_id: 1,
        };
        client.wait_ready()?;
        Ok(client)
    }

    /// Bounded wait for the ready line (a stray early response line would
    /// be protocol corruption — treated as failure).
    fn wait_ready(&mut self) -> Result<(), HelperTransportError> {
        let deadline = Instant::now() + Duration::from_millis(HELPER_READY_TIMEOUT_MS);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(HelperTransportError::Timeout);
            }
            match self.take_line(remaining)? {
                line if line == READY_LINE => return Ok(()),
                // Anything else before ready is corruption: the helper we
                // cannot trust framing from is a helper we cannot query.
                other => {
                    return Err(HelperTransportError::Unparsable(format!(
                        "expected the ready line, got: {other}"
                    )));
                }
            }
        }
    }

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

    pub(crate) fn tree(&mut self, opts: &UiTreeOptions) -> Result<String, ComputerUseError> {
        let (max_depth, max_nodes) = tree_budget(opts);
        match self.exchange(
            Op::Tree {
                max_depth,
                max_nodes,
            },
            Duration::from_millis(HELPER_TREE_TIMEOUT_MS),
        ) {
            Ok(Response {
                ok: true,
                tree: Some(tree),
                ..
            }) => Ok(tree),
            Ok(Response {
                ok: false,
                error: Some(error),
                ..
            }) => Err(ComputerUseError::unavailable(format!(
                "the UIA helper could not serve the tree: {error}"
            ))),
            Ok(_) => Err(ComputerUseError::failed(
                "the UIA helper answered the tree request with a malformed response",
            )),
            Err(HelperTransportError::Spawn(error)) => Err(error),
            Err(HelperTransportError::Timeout) => Err(ComputerUseError::unavailable(
                "the UIA helper did not answer the tree request in time",
            )),
            Err(HelperTransportError::BrokenPipe) => Err(ComputerUseError::unavailable(
                "the UIA helper's pipe closed before the tree request was served",
            )),
            Err(HelperTransportError::Unparsable(detail)) => Err(ComputerUseError::failed(
                format!("the UIA helper's answer could not be trusted: {detail}"),
            )),
        }
    }

    pub(crate) fn at_point(
        &mut self,
        x: i32,
        y: i32,
    ) -> Result<Option<ElementInfo>, ComputerUseError> {
        match self.exchange(
            Op::AtPoint { x, y },
            Duration::from_millis(HELPER_ELEMENT_TIMEOUT_MS),
        ) {
            Ok(Response {
                ok: true, element, ..
            }) => Ok(element.map(wire_to_info)),
            Ok(Response {
                ok: false,
                error: Some(error),
                ..
            }) => Err(ComputerUseError::unavailable(format!(
                "the UIA helper could not serve element_at_point: {error}"
            ))),
            Ok(_) => Err(ComputerUseError::failed(
                "the UIA helper answered the point request with a malformed response",
            )),
            Err(HelperTransportError::Spawn(error)) => Err(error),
            Err(HelperTransportError::Timeout) => Err(ComputerUseError::unavailable(
                "the UIA helper did not answer the point request in time",
            )),
            Err(HelperTransportError::BrokenPipe) => Err(ComputerUseError::unavailable(
                "the UIA helper's pipe closed before the point request was served",
            )),
            Err(HelperTransportError::Unparsable(detail)) => Err(ComputerUseError::failed(
                format!("the UIA helper's answer could not be trusted: {detail}"),
            )),
        }
    }

    pub(crate) fn focus(&mut self) -> Result<Option<ElementInfo>, ComputerUseError> {
        match self.exchange(Op::Focus, Duration::from_millis(HELPER_ELEMENT_TIMEOUT_MS)) {
            Ok(Response {
                ok: true, element, ..
            }) => Ok(element.map(wire_to_info)),
            Ok(Response {
                ok: false,
                error: Some(error),
                ..
            }) => Err(ComputerUseError::unavailable(error)),
            Ok(_) => Err(ComputerUseError::failed(
                "the UIA helper answered the focus request with a malformed response",
            )),
            Err(HelperTransportError::Spawn(error)) => Err(error),
            Err(HelperTransportError::Timeout) => Err(ComputerUseError::unavailable(
                "the UIA helper did not answer the focus request in time",
            )),
            Err(HelperTransportError::BrokenPipe) => Err(ComputerUseError::unavailable(
                "the UIA helper's pipe closed before the focus request was served",
            )),
            Err(HelperTransportError::Unparsable(detail)) => Err(ComputerUseError::failed(
                format!("the UIA helper's answer could not be trusted: {detail}"),
            )),
        }
    }
}

impl Drop for UiaHelperClient {
    fn drop(&mut self) {
        // Close the write end FIRST: stdin EOF is the helper's clean-exit
        // signal. The reader thread detaches (it exits on the broken pipe
        // the same way the capture thread does) — bounded cost, no join.
        self.reader = None;
        drop(self.stdin.take());
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

/// The shared budget rule (kept here so the parent translates options once;
/// the helper trusts the values it receives).
fn tree_budget(opts: &UiTreeOptions) -> (u32, u32) {
    (
        opts.max_depth
            .unwrap_or(DEFAULT_TREE_MAX_DEPTH)
            .min(MAX_TREE_DEPTH),
        opts.max_nodes
            .unwrap_or(DEFAULT_TREE_MAX_NODES)
            .min(MAX_TREE_NODES),
    )
}

use super::helpers::{
    DEFAULT_TREE_MAX_DEPTH, DEFAULT_TREE_MAX_NODES, MAX_TREE_DEPTH, MAX_TREE_NODES,
};

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
    let (inherit, keep) = if read_end {
        (read, write)
    } else {
        (write, read)
    };
    // SAFETY: flag set on our own newly created handle.
    unsafe { SetHandleInformation(inherit, HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT) }.map_err(
        |error| {
            let _ = CloseHandle(read);
            let _ = CloseHandle(write);
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

/// Mark BOTH child ends inheritable and return a guard whose drop closes the
/// parent's duplicates of them after CreateProcessW ran.
fn child_ends_guard(child_stdin_read: HANDLE, child_stdout_write: HANDLE) -> impl Drop {
    struct Ends([HANDLE; 2]);
    impl Drop for Ends {
        fn drop(&mut self) {
            for handle in self.0 {
                // SAFETY: close of our own pipe-end duplicates.
                let _ = unsafe { CloseHandle(handle) };
            }
        }
    }
    Ends([child_stdin_read, child_stdout_write])
}

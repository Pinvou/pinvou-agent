use super::*;
use crate::features::computer_use::backend::ComputerUseBackend;
use crate::features::computer_use::guard::{
    PHYSICAL_INPUT_LOCK_TIMEOUT, set_physical_input_lock_timeout_for_tests,
};
use crate::features::computer_use::types::{Capabilities, Capture, ElementInfo, Key};
use std::sync::Mutex as StdMutex;

/// A chord summary containing character keys shows only named keys plus a
/// character count in the confirm dialog/event stream — `key "shift+h"` under
/// a password focus no longer sends the typed characters into the dialog and
/// remote event stream (the existing semantics of the Type action). The
/// summary is also bound into the approval token (both mint and spend call
/// [`action_summary`]), so it must be a pure function of the action.
#[test]
fn chord_summaries_mask_typed_characters() {
    let summary_of = |input: serde_json::Value| {
        let parsed = parse_action(&input).expect("chord parses");
        action_summary(&parsed.action)
    };
    // Pure named-key chords keep the readable original text.
    assert_eq!(
        summary_of(json!({"action": "key", "text": "ctrl+Delete"})),
        "key ctrl+Delete"
    );
    assert_eq!(
        summary_of(json!({"action": "key", "text": "Return"})),
        "key Return"
    );
    // Chords with character keys render only modifiers/named keys + a
    // character count.
    assert_eq!(
        summary_of(json!({"action": "key", "text": "ctrl+s"})),
        "key ctrl + 1 character"
    );
    assert_eq!(
        summary_of(json!({"action": "key", "text": "shift+h"})),
        "key shift + 1 character"
    );
    assert_eq!(
        summary_of(json!({"action": "key", "text": "esc+h+u+n"})),
        "key esc + 3 characters"
    );
    assert_eq!(
        summary_of(json!({"action": "key", "text": "p+a+Return"})),
        "key enter + 2 characters"
    );
    // hold_key follows the same rule, keeping the held duration.
    assert_eq!(
        summary_of(json!({"action": "hold_key", "text": "ctrl+s", "ms": 500})),
        "hold ctrl + 1 character for 500ms"
    );
    assert_eq!(
        summary_of(json!({"action": "hold_key", "text": "Return", "ms": 500})),
        "hold Return for 500ms"
    );
}

// ---------------------------------------------------------------------------
// Test doubles
// ---------------------------------------------------------------------------

struct MockState {
    /// Hit-testing decides by element bounds (real a11y semantics):
    /// element_at_point returns an element only when the query point falls
    /// inside its rectangle.
    element: Option<ElementInfo>,
    /// Secondary element list (tried in order when the primary misses):
    /// drives multi-element scenarios such as "start point has no on-screen
    /// element while the drop point hits one" (the mock's primary is a
    /// single element).
    background: Vec<ElementInfo>,
    /// When set, element_at_point returns Err (injected a11y failure).
    element_error: bool,
    /// The focused element (focused_element's return value; None = definitively
    /// no focus).
    focused: Option<ElementInfo>,
    /// When set, focused_element returns Err (injected failure of the focus
    /// query / unsupported platform).
    focused_error: bool,
    /// The monitor origin captured in the screenshot (input coordinate space;
    /// default (0,0), i.e. the identity mapping).
    capture_origin: (i32, i32),
    /// The captured size (device physical pixels; default 16x16).
    capture_size: (u32, u32),
    /// The captured device→input scale (default (1.0, 1.0); set 0.5 for the
    /// Retina scenario).
    input_scale: (f64, f64),
    /// cursor_position's return value (**input coordinate space** — points on
    /// macOS, physical pixels on Windows/X11; configurable to drive screening
    /// location and cursor-move scenarios). Default (7,9) (historical
    /// behavior).
    cursor: (i32, i32),
    /// When set, cursor_position returns Err (cursor unknown, e.g. before the
    /// first Wayland move).
    cursor_error: bool,
    /// When set, the input capability bit is false (input capable by default).
    no_input_cap: bool,
    /// When set, the screenshot capability bit is false (capture capable by
    /// default).
    screenshot_cap: bool,
    /// When set, the ui_tree capability bit is false (a11y tree capable by
    /// default).
    ui_tree_cap: bool,
    /// When set, the screening queries (element_at_point/focused_element/
    /// cursor_position) revoke this fixture session's grant ("s-test") before
    /// answering — drives the last-moment re-check pin (a revoke landing
    /// mid-screening must abort before injection).
    revoke_grant_via: Option<Arc<ComputerUseShared>>,
    moved_to: Vec<(i32, i32)>,
    clicked: Vec<(MouseButton, u8)>,
    /// All five injection surfaces are recorded — the "NOT executed"
    /// assertions for down/up/drag/scroll/hold_key verify the execution
    /// surface (the regression pin was soft without them).
    downed: Vec<MouseButton>,
    upped: Vec<MouseButton>,
    drags: Vec<((i32, i32), (i32, i32))>,
    scrolled: Vec<(ScrollDirection, u32)>,
    held: Vec<(Vec<Key>, u64)>,
    typed: Vec<String>,
    chords: Vec<Vec<Key>>,
    /// Number of times release_os_grant was called: revoke/stop must
    /// trigger the backend to close its persistent OS-level grant.
    released: u64,
    /// When set, drag returns Err (injected drag backend failure, verifying
    /// the button-release safety net after a failure).
    drag_error: bool,
    /// When set, capture returns Err (injected capture backend failure; pins
    /// that a failed screenshot action must propagate instead of degrading to
    /// a warning).
    capture_error: bool,
    /// When non-empty, type_text returns this error text (a pin for audit
    /// redaction on the execution-error path: the error goes into the audit,
    /// the typed content must still never appear).
    type_error: Option<String>,
}

impl Default for MockState {
    fn default() -> Self {
        Self {
            element: None,
            background: Vec::new(),
            element_error: false,
            focused: None,
            focused_error: false,
            capture_origin: (0, 0),
            capture_size: (16, 16),
            input_scale: (1.0, 1.0),
            cursor: (7, 9),
            cursor_error: false,
            no_input_cap: false,
            screenshot_cap: true,
            ui_tree_cap: true,
            revoke_grant_via: None,
            moved_to: Vec::new(),
            clicked: Vec::new(),
            downed: Vec::new(),
            upped: Vec::new(),
            drags: Vec::new(),
            scrolled: Vec::new(),
            held: Vec::new(),
            typed: Vec::new(),
            chords: Vec::new(),
            released: 0,
            drag_error: false,
            capture_error: false,
            type_error: None,
        }
    }
}

impl MockState {
    fn with_cursor(cursor: (i32, i32)) -> Self {
        Self {
            cursor,
            ..Self::default()
        }
    }
}

struct MockBackend {
    state: Arc<Mutex<MockState>>,
}

impl MockBackend {
    /// Revokes the fixture session's grant before answering a screening
    /// query when the mock state asks for it (the last-moment re-check pin).
    fn maybe_revoke_grant(&self) {
        let shared = self.state.lock().revoke_grant_via.clone();
        if let Some(shared) = shared {
            shared.revoke_session("s-test");
        }
    }
}

impl ComputerUseBackend for MockBackend {
    fn capabilities(&self) -> Capabilities {
        let state = self.state.lock();
        Capabilities {
            screenshot: state.screenshot_cap,
            input: !state.no_input_cap,
            ui_tree: state.ui_tree_cap,
            notes: "mock".to_string(),
        }
    }

    fn capture(&mut self) -> Result<Capture, ComputerUseError> {
        let state = self.state.lock();
        if state.capture_error {
            return Err(ComputerUseError::unavailable("mock: capture denied"));
        }
        let (width, height) = state.capture_size;
        let mut rgba = vec![0u8; width as usize * height as usize * 4];
        for (i, byte) in rgba.iter_mut().enumerate() {
            *byte = (i % 253) as u8;
        }
        Ok(Capture {
            rgba,
            width,
            height,
            origin_x: state.capture_origin.0,
            origin_y: state.capture_origin.1,
            input_scale_x: state.input_scale.0,
            input_scale_y: state.input_scale.1,
        })
    }

    fn cursor_position(&mut self) -> Result<(i32, i32), ComputerUseError> {
        self.maybe_revoke_grant();
        let state = self.state.lock();
        if state.cursor_error {
            return Err(ComputerUseError::unsupported(
                "cursor_position",
                "mock: cursor position unknown",
            ));
        }
        Ok(state.cursor)
    }

    fn move_to(&mut self, x: i32, y: i32) -> Result<(), ComputerUseError> {
        self.state.lock().moved_to.push((x, y));
        Ok(())
    }

    fn click(&mut self, button: MouseButton, count: u8) -> Result<(), ComputerUseError> {
        self.state.lock().clicked.push((button, count));
        Ok(())
    }

    fn mouse_down(&mut self, button: MouseButton) -> Result<(), ComputerUseError> {
        self.state.lock().downed.push(button);
        Ok(())
    }

    fn mouse_up(&mut self, button: MouseButton) -> Result<(), ComputerUseError> {
        self.state.lock().upped.push(button);
        Ok(())
    }

    fn drag(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<(), ComputerUseError> {
        let mut state = self.state.lock();
        if state.drag_error {
            return Err(ComputerUseError::unavailable("mock: drag backend failed"));
        }
        state.drags.push((from, to));
        Ok(())
    }

    fn scroll(&mut self, direction: ScrollDirection, clicks: u32) -> Result<(), ComputerUseError> {
        self.state.lock().scrolled.push((direction, clicks));
        Ok(())
    }

    fn type_text(&mut self, text: &str) -> Result<(), ComputerUseError> {
        let mut state = self.state.lock();
        if let Some(error) = &state.type_error {
            return Err(ComputerUseError::unavailable(error.clone()));
        }
        state.typed.push(text.to_string());
        Ok(())
    }

    fn key_chord(&mut self, keys: &[Key]) -> Result<(), ComputerUseError> {
        self.state.lock().chords.push(keys.to_vec());
        Ok(())
    }

    fn hold_key(&mut self, keys: &[Key], ms: u64) -> Result<(), ComputerUseError> {
        self.state.lock().held.push((keys.to_vec(), ms));
        Ok(())
    }

    fn ui_tree(&mut self, _opts: &UiTreeOptions) -> Result<String, ComputerUseError> {
        Ok("window \"Mock\"\n  button \"OK\"".to_string())
    }

    fn element_at_point(
        &mut self,
        x: i32,
        y: i32,
    ) -> Result<Option<ElementInfo>, ComputerUseError> {
        self.maybe_revoke_grant();
        let state = self.state.lock();
        if state.element_error {
            return Err(ComputerUseError::unavailable("mock: a11y backend failed"));
        }
        let hit = |element: &ElementInfo| {
            x >= element.x
                && x < element.x + element.width
                && y >= element.y
                && y < element.y + element.height
        };
        if let Some(element) = &state.element {
            if hit(element) {
                return Ok(Some(element.clone()));
            }
        }
        Ok(state.background.iter().find(|e| hit(e)).cloned())
    }

    fn focused_element(&mut self) -> Result<Option<ElementInfo>, ComputerUseError> {
        self.maybe_revoke_grant();
        let state = self.state.lock();
        if state.focused_error {
            return Err(ComputerUseError::unsupported(
                "focused_element",
                "mock: focus query unavailable",
            ));
        }
        Ok(state.focused.clone())
    }

    fn release_os_grant(&mut self) -> Result<(), ComputerUseError> {
        self.state.lock().released += 1;
        Ok(())
    }
}

struct RecordingSink(Arc<StdMutex<Vec<(String, Value)>>>);

impl ComputerUseEventSink for RecordingSink {
    fn emit(&self, event: &str, payload: Value) {
        if let Ok(mut events) = self.0.lock() {
            events.push((event.to_string(), payload));
        }
    }
}

struct TestFixture {
    tool: ComputerUseTool,
    shared: Arc<ComputerUseShared>,
    mock: Arc<Mutex<MockState>>,
    events: Arc<StdMutex<Vec<(String, Value)>>>,
    workspace: PathBuf,
    /// The temp root PINVOU3_HOME points at (the audit JSONL lands in
    /// `<home>/computer-use/`). Deleted automatically when the fixture drops.
    home: PathBuf,
    _home_dir: tempfile::TempDir,
    _env_guard: std::sync::MutexGuard<'static, ()>,
}

struct EnvRestore(Option<std::ffi::OsString>);

impl Drop for EnvRestore {
    fn drop(&mut self) {
        match self.0.take() {
            // SAFETY: the test holds platform::paths::tests::ENV_LOCK, so env
            // writes are serialized in-process.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: same as above.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
    }
}

fn fixture() -> (TestFixture, EnvRestore) {
    let env_lock = crate::platform::paths::tests::ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let previous = std::env::var_os("PINVOU3_HOME");
    let home_dir = tempfile::Builder::new()
        .prefix("pinvou3-cu-home-")
        .tempdir()
        .expect("temp home dir");
    let home = home_dir.path().to_path_buf();
    // SAFETY: holding platform::paths::tests::ENV_LOCK, so env writes are
    // serialized in-process.
    unsafe { std::env::set_var("PINVOU3_HOME", &home) };

    let workspace = home.join("sessions").join("s-test").join("workspace");
    let _ = std::fs::create_dir_all(&workspace);

    let shared = Arc::new(ComputerUseShared::new());
    shared.set_enabled(true);
    let mock = Arc::new(Mutex::new(MockState::default()));
    let events = Arc::new(StdMutex::new(Vec::new()));

    let mock_for_factory = Arc::clone(&mock);
    let backend = BackendHandle::lazy(move || {
        Ok(Box::new(MockBackend {
            state: mock_for_factory,
        }) as Box<dyn ComputerUseBackend>)
    });
    let tool = ComputerUseTool::with_parts(
        "s-test".to_string(),
        Arc::clone(&shared),
        backend,
        Arc::new(RecordingSink(Arc::clone(&events))),
    );
    (
        TestFixture {
            tool,
            shared,
            mock,
            events,
            workspace,
            home,
            _home_dir: home_dir,
            _env_guard: env_lock,
        },
        EnvRestore(previous),
    )
}

fn context(workspace: &Path) -> ToolContext {
    ToolContext::new(workspace)
}

/// The fixture session's audit JSONL path, resolved through `AuditLog` so the
/// filename format stays owned by audit.rs (the fixture pins PINVOU3_HOME).
fn session_audit_path() -> PathBuf {
    AuditLog::for_session("s-test")
        .expect("audit log for session")
        .path()
        .to_path_buf()
}

/// Reads the fixture session's audit JSONL into typed records (one record
/// per non-empty line). AuditRecord has no Deserialize derive, so the fields
/// are mapped from the parsed JSON explicitly.
fn read_audit_records(_fixture: &TestFixture) -> Vec<AuditRecord> {
    let raw = std::fs::read_to_string(session_audit_path()).expect("audit jsonl exists");
    raw.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let value: Value = serde_json::from_str(line).expect("valid jsonl line");
            let mut record = AuditRecord::new(
                value["session_id"].as_str().unwrap_or_default(),
                value["action"].as_str().unwrap_or_default(),
                value["consent"].as_str().unwrap_or_default(),
            );
            record.target = value["target"].as_str().map(str::to_string);
            record.result = value["result"].as_str().map(str::to_string);
            record.error = value["error"].as_str().map(str::to_string);
            record.duration_ms = value["duration_ms"].as_u64();
            record.screenshot_sha256 = value["screenshot_sha256"].as_str().map(str::to_string);
            record.screenshot_path = value["screenshot_path"].as_str().map(str::to_string);
            record
        })
        .collect()
}

/// A benign element outside the denylist (T3 screening Clear): the place
/// element covers the screening point so the pass path reaches execution.
/// Unnamed targets screen Clear (Ok(None) is not a red flag);
/// a11y query *errors* also screen Clear (best-effort fail-open), but a
/// positive denylist/secure match still confirms.
fn benign_element(x: i32, y: i32, width: i32, height: i32) -> ElementInfo {
    ElementInfo {
        role: "AXGroup".to_string(),
        name: "Workspace".to_string(),
        x,
        y,
        width,
        height,
        secure: false,
    }
}

// ---------------------------------------------------------------------------
// Parameter validation
// ---------------------------------------------------------------------------

#[test]
fn rejects_coordinate_for_key() {
    let err = parse_action(&json!({"action": "key", "text": "ctrl+s", "x": 1, "y": 2}));
    let text = err.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(
        text.contains("coordinate is not accepted for key"),
        "{text}"
    );
}

#[test]
fn rejects_bad_param_combinations() {
    let cases: &[(&str, Value)] = &[
        ("type", json!({"action": "type"})),
        (
            "left_click",
            json!({"action": "left_click", "x": -1, "y": 5}),
        ),
        ("left_click", json!({"action": "left_click", "x": 5})),
        (
            "scroll",
            json!({"action": "scroll", "direction": "north", "amount": 1}),
        ),
        ("scroll", json!({"action": "scroll", "direction": "down"})),
        // Scroll amount has a lower bound of 1; 0
        // clicks are rejected explicitly.
        (
            "scroll",
            json!({"action": "scroll", "direction": "down", "amount": 0}),
        ),
        ("wait", json!({"action": "wait"})),
        // ms upper bound: wait rejects anything above MAX_WAIT_MS.
        ("wait", json!({"action": "wait", "ms": 30001})),
        (
            "left_click_drag",
            json!({"action": "left_click_drag", "x": 1, "y": 2}),
        ),
        ("key", json!({"action": "key", "text": "ctrl+shift"})),
        ("key", json!({"action": "key", "text": "ctrl+nosuchkey"})),
        // Chord token cap and control-character
        // rejection.
        ("key", json!({"action": "key", "text": "a+b+c+d+e"})),
        (
            "key",
            json!({"action": "key", "text": "ctrl+alt+shift+meta+c"}),
        ),
        ("key", json!({"action": "key", "text": "\u{1}"})),
        ("screenshot", json!({"action": "screenshot", "text": "x"})),
        (
            "hold_key",
            json!({"action": "hold_key", "text": "a", "ms": 99999}),
        ),
        ("mouse_move", json!({"action": "mouse_move", "x": 1})),
        ("bogus", json!({"action": "bogus"})),
    ];
    for (name, input) in cases {
        assert!(parse_action(input).is_err(), "{name} should reject {input}");
    }
}

/// Type text is capped at 10_000 characters; NUL is
/// rejected.
#[test]
fn type_rejects_oversized_text_and_nul() {
    let ok = "a".repeat(MAX_TYPE_TEXT_CHARS);
    assert!(parse_action(&json!({"action": "type", "text": ok})).is_ok());
    let too_long = "a".repeat(MAX_TYPE_TEXT_CHARS + 1);
    let err = parse_action(&json!({"action": "type", "text": too_long}))
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(err.contains("too long"), "{err}");
    assert!(err.contains("10000"), "{err}");
    let err = parse_action(&json!({"action": "type", "text": "bad\0nul"}))
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(err.contains("NUL"), "{err}");
}

#[test]
fn accepts_valid_param_combinations() {
    for input in [
        json!({"action": "screenshot"}),
        json!({"action": "left_click", "x": 10, "y": 20}),
        json!({"action": "left_click"}),
        json!({"action": "double_click", "x": 0, "y": 0}),
        json!({"action": "scroll", "direction": "down", "amount": 3}),
        json!({"action": "scroll", "direction": "up", "amount": 1, "x": 5, "y": 6}),
        json!({"action": "left_click_drag", "start_x": 1, "start_y": 2, "x": 3, "y": 4}),
        json!({"action": "key", "text": "ctrl+s"}),
        json!({"action": "hold_key", "text": "ctrl+a", "ms": 100}),
        json!({"action": "wait", "ms": 500}),
        json!({"action": "ui_tree", "max_depth": 3, "max_nodes": 100}),
        json!({"action": "element_at_point", "x": 1, "y": 2}),
        json!({"action": "left_click", "x": 1, "y": 2, "confirm_id": "cu-x"}),
    ] {
        assert!(parse_action(&input).is_ok(), "should accept {input}");
    }
}

// hold_key's "shift" alone is a modifier key — a chord must contain a
// non-modifier key.
#[test]
fn hold_key_rejects_modifier_only_chord() {
    assert!(parse_action(&json!({"action": "hold_key", "text": "shift", "ms": 100})).is_err());
}

/// A failed `key` parse must never echo the
/// model's text into the audit log — the model can carry a sensitive string
/// in the text field to probe (`{"action":"key","text":"hunter2"}` used to
/// write that string into the JSONL error field via the error message). The
/// error string only describes the chord shape; the model already knows its
/// own input, so no information is lost.
#[tokio::test]
async fn parse_failure_audit_record_does_not_echo_the_chord_text() {
    let (fixture, _restore) = fixture();
    // Two shape-only rejection paths: "hunter2" is a single unknown token;
    // "h+u+n+t+e+r" exceeds the chord token limit. Neither error may echo
    // the model's text.
    for chord in ["hunter2", "h+u+n+t+e+r"] {
        let result = fixture
            .tool
            .execute(
                json!({"action": "key", "text": chord}),
                &context(&fixture.workspace),
            )
            .await;
        let error = match result {
            Ok(r) => panic!("the secret chord must not parse: {r:?}"),
            Err(e) => e.to_string(),
        };
        // The model still gets a shape-only explanation.
        assert!(
            error.contains("key chord"),
            "expected a shape-only chord error: {error}"
        );
        assert!(!error.contains(chord), "error echoed the chord: {error}");
    }

    let raw = std::fs::read_to_string(session_audit_path()).expect("audit jsonl exists");
    assert!(
        !raw.contains("hunter2") && !raw.contains("h+u+n+t+e+r"),
        "the secret substring must appear nowhere in the audit log: {raw}"
    );
    let records = read_audit_records(&fixture);
    assert_eq!(records.len(), 2, "one record per call: {records:?}");
    for record in &records {
        assert_eq!(record.action, "unparseable", "{records:?}");
        assert_eq!(record.result.as_deref(), Some("rejected"), "{records:?}");
        assert!(
            record
                .error
                .as_deref()
                .unwrap_or_default()
                .starts_with("parse failed: Failed to validate input: key chord"),
            "shape-only parse error expected: {records:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Consent gating
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disabled_returns_clear_error() {
    let (fixture, _restore) = fixture();
    fixture.shared.set_enabled(false);
    let result = fixture
        .tool
        .execute(
            json!({"action": "screenshot"}),
            &context(&fixture.workspace),
        )
        .await;
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        text.contains("computer use is disabled in settings"),
        "{text}"
    );
}

#[tokio::test]
async fn enabling_toggle_serves_existing_tool_instance_without_rebuild() {
    // Engines spawned while the settings toggle is
    // off hold a ComputerUseTool instance (tool_factory construction is
    // unconditional; visibility comes from the disallow list). Flipping the
    // toggle on — everything computer_use_set_enabled does on the tool
    // side — must make THAT already-constructed instance serviceable, so a
    // live session does not have to wait for an engine rebuild. This pins
    // the tool half of that contract: the guard reads the live flag and
    // never a snapshot taken at construction time.
    let (fixture, _restore) = fixture();
    fixture.shared.set_enabled(false);
    let rejected = fixture
        .tool
        .execute(
            json!({"action": "screenshot"}),
            &context(&fixture.workspace),
        )
        .await;
    let text = rejected.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        text.contains("computer use is disabled in settings"),
        "{text}"
    );

    fixture.shared.set_enabled(true);
    let served = fixture
        .tool
        .execute(
            json!({"action": "screenshot"}),
            &context(&fixture.workspace),
        )
        .await;
    let served = match served {
        Ok(r) => r,
        Err(e) => panic!("execute failed after enabling: {e}"),
    };
    assert!(served.success, "{}", served.content);
    assert!(served.content.contains("16x16 px"), "{}", served.content);
}

#[tokio::test]
async fn input_without_grant_emits_event_and_errors() {
    let (fixture, _restore) = fixture();
    let result = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 1, "y": 2}),
            &context(&fixture.workspace),
        )
        .await;
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("has not granted control"), "{text}");
    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    assert!(
        events.iter().any(
            |(name, payload)| name == EVENT_GRANT_REQUIRED && payload["session_id"] == "s-test"
        ),
        "expected grant_required event, got {events:?}"
    );
    // Without a grant the backend must never be touched.
    assert!(fixture.mock.lock().clicked.is_empty());
    // Rejected calls must leave a trace: an audit record with
    // result:"rejected" and consent:"rejected:grant-required".
    let records = read_audit_records(&fixture);
    let rejected: Vec<&AuditRecord> = records
        .iter()
        .filter(|r| r.result.as_deref() == Some("rejected") && r.action == "left_click")
        .collect();
    assert_eq!(
        rejected.len(),
        1,
        "the grant-rejected click must leave exactly one audit record: {records:?}"
    );
    assert_eq!(rejected[0].consent, "rejected:grant-required");
}

#[tokio::test]
async fn granted_click_executes_and_attaches_screenshot() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // A benign element covers the (5,6) target (screens Clear).
    fixture.mock.lock().element = Some(benign_element(0, 0, 16, 16));
    let result = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 5, "y": 6}),
            &context(&fixture.workspace),
        )
        .await;
    let result = match result {
        Ok(r) => r,
        Err(e) => panic!("execute failed: {e}"),
    };
    assert!(result.success, "{}", result.content);
    // Before the click, moved to the mapped input coordinates (16x16, no
    // scaling, input_scale=1 → identity).
    assert_eq!(fixture.mock.lock().moved_to.last().copied(), Some((5, 6)));
    assert_eq!(
        fixture.mock.lock().clicked.last().copied(),
        Some((MouseButton::Left, 1))
    );
    // metadata.images carries the absolute path; the text carries the
    // attachments relative path with the image_analyze fallback hint.
    let images = result
        .metadata
        .as_ref()
        .and_then(|m| m.get("images"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(images.len(), 1, "metadata: {:?}", result.metadata);
    let path = images[0].as_str().unwrap_or_default().to_string();
    assert!(path.ends_with(".png"), "{path}");
    assert!(Path::new(&path).is_file(), "{path} should exist");
    // The screenshot file must be 0600: previously only
    // asserted in the ignored live suite; in CI, nothing would catch
    // capture_and_store degrading to a plain write).
    crate::platform::filesystem::assert_private_file_mode(Path::new(&path));
    assert!(
        result.content.contains("attachments/computer_use/"),
        "{}",
        result.content
    );
    assert!(
        result.content.contains("image_analyze"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn out_of_bounds_coordinates_clamp_with_warning() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // A benign element covers the clamped (15,15) point (screens Clear).
    fixture.mock.lock().element = Some(benign_element(0, 0, 16, 16));
    let result = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 9999, "y": 50}),
            &context(&fixture.workspace),
        )
        .await;
    let result = match result {
        Ok(r) => r,
        Err(e) => panic!("execute failed: {e}"),
    };
    assert!(result.success, "{}", result.content);
    assert!(result.content.contains("clamped"), "{}", result.content);
    // 16x16 screenshot → clamped to (15, 15).
    assert_eq!(fixture.mock.lock().moved_to.last().copied(), Some((15, 15)));
}

#[tokio::test]
async fn first_coordinate_action_auto_captures() {
    let (fixture, _restore) = fixture();
    // scroll is an Input-class action: it needs a session
    // grant.
    fixture.shared.grant_session("s-test");
    // A benign element covers the (3,4) screening point (screens Clear).
    fixture.mock.lock().element = Some(benign_element(0, 0, 16, 16));
    let result = fixture
        .tool
        .execute(
            json!({"action": "scroll", "direction": "down", "amount": 2, "x": 3, "y": 4}),
            &context(&fixture.workspace),
        )
        .await;
    let result = match result {
        Ok(r) => r,
        Err(e) => panic!("execute failed: {e}"),
    };
    assert!(result.success, "{}", result.content);
    assert!(
        result.content.contains("captured automatically"),
        "{}",
        result.content
    );
    assert!(
        result
            .metadata
            .as_ref()
            .and_then(|m| m.get("images"))
            .is_some()
    );
}

// ---------------------------------------------------------------------------
// T3 consequential actions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn t3_denylist_blocks_click_until_user_confirms() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    fixture.mock.lock().element = Some(ElementInfo {
        role: "button".to_string(),
        name: "Buy now".to_string(),
        x: 0,
        y: 0,
        width: 10,
        height: 10,
        secure: false,
    });
    let result = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 5, "y": 5}),
            &context(&fixture.workspace),
        )
        .await;
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("NOT executed"), "{text}");
    assert!(text.contains("confirm_id"), "{text}");
    assert!(fixture.mock.lock().clicked.is_empty(), "must not click");

    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    let confirm = events
        .iter()
        .find(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
        .map(|(_, payload)| payload.clone());
    let payload = match confirm {
        Some(p) => p,
        None => panic!("expected confirm_required event, got {events:?}"),
    };
    assert_eq!(payload["element"], "Buy now (button)");
    let confirm_id = payload["confirm_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // A forged/replayed confirm_id is always rejected.
    let forged = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 5, "y": 5, "confirm_id": "cu-forged"}),
            &context(&fixture.workspace),
        )
        .await;
    let forged_text = forged.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        forged_text.contains("invalid, expired, or was already used"),
        "{forged_text}"
    );

    // After the user confirms (a future computer_use_confirm command mints
    // the token), the retry succeeds.
    fixture.shared.mint_confirmation(&confirm_id);
    let confirmed = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 5, "y": 5, "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    let confirmed = match confirmed {
        Ok(r) => r,
        Err(e) => panic!("execute failed: {e}"),
    };
    assert!(confirmed.success, "{}", confirmed.content);
    assert_eq!(fixture.mock.lock().clicked.len(), 1);
}

#[tokio::test]
async fn secure_field_blocks_typing() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // Keyboard screening checks the **focused element**: focus on a password
    // field blocks regardless of the cursor position.
    fixture.mock.lock().focused = Some(ElementInfo {
        role: "AXSecureTextField".to_string(),
        name: "Password".to_string(),
        x: 0,
        y: 0,
        width: 10,
        height: 10,
        secure: true,
    });
    let result = fixture
        .tool
        .execute(
            json!({"action": "type", "text": "hunter2"}),
            &context(&fixture.workspace),
        )
        .await;
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("NOT executed"), "{text}");
    assert!(text.contains("password/secure field"), "{text}");
    assert!(fixture.mock.lock().typed.is_empty(), "must not type");
}

// ---------------------------------------------------------------------------
// Observation class
// ---------------------------------------------------------------------------

#[tokio::test]
async fn screenshot_always_attaches_and_reports_geometry() {
    let (fixture, _restore) = fixture();
    let result = fixture
        .tool
        .execute(
            json!({"action": "screenshot"}),
            &context(&fixture.workspace),
        )
        .await;
    let result = match result {
        Ok(r) => r,
        Err(e) => panic!("execute failed: {e}"),
    };
    assert!(result.success, "{}", result.content);
    assert!(result.content.contains("16x16 px"), "{}", result.content);
    assert!(result.content.contains("scale 1.000"), "{}", result.content);
    assert!(
        result
            .metadata
            .as_ref()
            .and_then(|m| m.get("images"))
            .is_some()
    );
}

#[tokio::test]
async fn cursor_position_reports_screenshot_space_after_capture() {
    let (fixture, _restore) = fixture();
    let _ = fixture
        .tool
        .execute(
            json!({"action": "screenshot"}),
            &context(&fixture.workspace),
        )
        .await;
    let result = fixture
        .tool
        .execute(
            json!({"action": "cursor_position"}),
            &context(&fixture.workspace),
        )
        .await;
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("(7, 9) in screenshot space"), "{text}");
}

// ---------------------------------------------------------------------------
// Layered screening / screening-unavailable passes /
// token binding / capabilities first
// ---------------------------------------------------------------------------

/// P0 regression: mouse_down/mouse_up were once absent from the T3 list,
/// allowing a zero-confirmation click to be decomposed.
#[tokio::test]
async fn mouse_down_up_composition_is_t3_screened() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // The denylist control sits at the cursor (7,9).
    fixture.mock.lock().element = Some(ElementInfo {
        role: "button".to_string(),
        name: "Delete forever".to_string(),
        x: 0,
        y: 0,
        width: 10,
        height: 10,
        secure: false,
    });
    for action in ["left_mouse_down", "left_mouse_up"] {
        let result = fixture
            .tool
            .execute(json!({"action": action}), &context(&fixture.workspace))
            .await;
        let text = result.ok().map(|r| r.content).unwrap_or_default();
        assert!(text.contains("NOT executed"), "{action}: {text}");
        assert!(text.contains("confirm_id"), "{action}: {text}");
    }
    assert!(
        !fixture
            .events
            .lock()
            .map(|e| e.clone())
            .unwrap_or_default()
            .is_empty(),
        "confirm_required must have been emitted"
    );
    // Execution-surface assertions: when
    // blocked, down/up must not actually be dispatched (the mock records
    // all five injection surfaces).
    let mock = fixture.mock.lock();
    assert!(
        mock.downed.is_empty(),
        "down must not execute: {:?}",
        mock.downed
    );
    assert!(
        mock.upped.is_empty(),
        "up must not execute: {:?}",
        mock.upped
    );
}

/// Screening being unavailable does not block execution (mainstream
/// position: screening is best-effort category detection, and no product asks
/// for a confirmation over a screening infrastructure failure): a target
/// with an a11y query failure executes as usual without a confirm event; a
/// positive denylist hit still blocks (see the other T3 tests).
#[tokio::test]
async fn a11y_query_error_executes_without_confirmation() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    fixture.mock.lock().element_error = true;
    let result = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 5, "y": 5}),
            &context(&fixture.workspace),
        )
        .await;
    let result = match result {
        Ok(r) => r,
        Err(e) => panic!("execute failed: {e}"),
    };
    assert!(result.success, "{}", result.content);
    assert!(
        !result.content.contains("NOT executed"),
        "an unreadable target must not be gated: {}",
        result.content
    );
    assert_eq!(fixture.mock.lock().clicked.len(), 1, "the click executed");
    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    assert!(
        !events
            .iter()
            .any(|(name, _)| name == EVENT_CONFIRM_REQUIRED),
        "no confirmation may be requested while screening is unavailable: {events:?}"
    );
}

/// Keyboard input lands on the **focused
/// element**, not at the cursor — when focus is on a password field and the
/// cursor is elsewhere, the old type implementation screened by cursor (found
/// no element) and let the injection through.
#[tokio::test]
async fn focus_on_password_with_cursor_elsewhere_requires_confirmation() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // Focus is on a password field; no element at all at the cursor (7,9)
    // (element only proves the cursor point is blank).
    fixture.mock.lock().focused = Some(ElementInfo {
        role: "AXSecureTextField".to_string(),
        name: "Password".to_string(),
        x: 100,
        y: 100,
        width: 5,
        height: 5,
        secure: true,
    });
    let result = fixture
        .tool
        .execute(
            json!({"action": "type", "text": "hunter2"}),
            &context(&fixture.workspace),
        )
        .await;
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("NOT executed"), "{text}");
    assert!(text.contains("password/secure field"), "{text}");
    assert!(fixture.mock.lock().typed.is_empty(), "must not type");

    // key chords are screened by focus too: a consequential control with
    // focus requires confirmation.
    fixture.mock.lock().focused = Some(ElementInfo {
        role: "button".to_string(),
        name: "Send payment".to_string(),
        x: 100,
        y: 100,
        width: 5,
        height: 5,
        secure: false,
    });
    let result = fixture
        .tool
        .execute(
            json!({"action": "key", "text": "ctrl+s"}),
            &context(&fixture.workspace),
        )
        .await;
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("NOT executed"), "{text}");
    assert!(text.contains("Send payment"), "{text}");
    assert!(fixture.mock.lock().chords.is_empty(), "must not press");
}

/// Only a **positive** hit on the password/secure role confirms: an
/// unreadable focus (query failure) is not a signal — type executes as usual
/// without a confirm event; a focus positively hitting the password role
/// still blocks (two contrasting halves in one test, pin (c)).
#[tokio::test]
async fn unreadable_type_focus_executes_and_password_focus_confirms() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // Focus query fails: does not block execution, no confirm event.
    fixture.mock.lock().focused_error = true;
    let result = fixture
        .tool
        .execute(
            json!({"action": "type", "text": "hello"}),
            &context(&fixture.workspace),
        )
        .await;
    let result = match result {
        Ok(r) => r,
        Err(e) => panic!("execute failed: {e}"),
    };
    assert!(result.success, "{}", result.content);
    assert_eq!(
        fixture.mock.lock().typed.last().map(String::as_str),
        Some("hello"),
        "an unreadable focus must not block typing"
    );
    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    assert!(
        !events
            .iter()
            .any(|(name, _)| name == EVENT_CONFIRM_REQUIRED),
        "no confirmation may be requested while the focus is unreadable: {events:?}"
    );

    // Contrast: focus positively hits the password role → blocked with a
    // confirmation.
    fixture.mock.lock().focused_error = false;
    fixture.mock.lock().focused = Some(ElementInfo {
        role: "AXSecureTextField".to_string(),
        name: "Password".to_string(),
        x: 0,
        y: 0,
        width: 10,
        height: 10,
        secure: true,
    });
    let blocked = fixture
        .tool
        .execute(
            json!({"action": "type", "text": "hunter2"}),
            &context(&fixture.workspace),
        )
        .await;
    let text = blocked.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("NOT executed"), "{text}");
    assert!(text.contains("password/secure field"), "{text}");
    assert_eq!(
        fixture.mock.lock().typed.len(),
        1,
        "only the first type ran"
    );
}

/// Definitively no focused element = nowhere to type; type passes (Ok(None),
/// like Err, is not a confirmation signal; only a positive hit on the
/// password/denylist role blocks — see the other T3 tests).
#[tokio::test]
async fn no_focused_element_allows_typing() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    fixture.mock.lock().focused = None;
    let result = fixture
        .tool
        .execute(
            json!({"action": "type", "text": "hello"}),
            &context(&fixture.workspace),
        )
        .await;
    let result = match result {
        Ok(r) => r,
        Err(e) => panic!("execute failed: {e}"),
    };
    assert!(result.success, "{}", result.content);
    assert_eq!(
        fixture.mock.lock().typed.last().map(String::as_str),
        Some("hello")
    );
}

/// Both the drag start and the drop point must be screened (pin (f)): a
/// drop point hitting the denylist (benign start) and a start hitting the
/// denylist (benign drop) both require confirmation.
#[tokio::test]
async fn drag_drop_target_is_screened() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // A benign element covers the (1,1) start point (the start must also be
    // screened and read Clear); the (12,12) drop point hits the denylist.
    fixture.mock.lock().background = vec![benign_element(0, 0, 3, 3)];
    fixture.mock.lock().element = Some(ElementInfo {
        role: "button".to_string(),
        name: "Delete".to_string(),
        x: 10,
        y: 10,
        width: 5,
        height: 5,
        secure: false,
    });
    let result = fixture
        .tool
        .execute(
            json!({"action": "left_click_drag", "start_x": 1, "start_y": 1, "x": 12, "y": 12}),
            &context(&fixture.workspace),
        )
        .await;
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("NOT executed"), "{text}");
    assert!(text.contains("Delete"), "{text}");
    // When blocked, the drag must not
    // actually be dispatched.
    assert!(
        fixture.mock.lock().drags.is_empty(),
        "drag must not execute"
    );

    // Reverse: the start hits the denylist (benign drop) — blocked as well.
    fixture.mock.lock().background = vec![ElementInfo {
        role: "button".to_string(),
        name: "Delete".to_string(),
        x: 0,
        y: 0,
        width: 3,
        height: 3,
        secure: false,
    }];
    fixture.mock.lock().element = Some(benign_element(10, 10, 5, 5));
    let result = fixture
        .tool
        .execute(
            json!({"action": "left_click_drag", "start_x": 1, "start_y": 1, "x": 12, "y": 12}),
            &context(&fixture.workspace),
        )
        .await;
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("NOT executed"), "{text}");
    assert!(text.contains("Delete"), "{text}");
    assert!(
        fixture.mock.lock().drags.is_empty(),
        "drag must not execute"
    );
}

/// Denial closes the dialog and consumes the pending confirmation — and
/// records NO server-side state (mainstream: decline is model-visible
/// context only). Retrying the denied id fails (the token is spent/unknown,
/// nothing is injected); an identical retry of the SAME action goes through
/// the normal flow again — it emits a fresh confirm_required event and
/// mints a NEW confirm_id.
#[tokio::test]
async fn denied_action_retry_mints_a_fresh_confirmation() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    fixture.mock.lock().element = Some(ElementInfo {
        role: "button".to_string(),
        name: "Buy now".to_string(),
        x: 0,
        y: 0,
        width: 10,
        height: 10,
        secure: false,
    });
    let first = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 5, "y": 5}),
            &context(&fixture.workspace),
        )
        .await;
    assert!(
        first
            .ok()
            .map(|r| r.content)
            .unwrap_or_default()
            .contains("NOT executed")
    );
    let denied_id = latest_confirm_id(&fixture.events);
    assert!(
        fixture.shared.deny_confirmation(&denied_id),
        "deny must consume the pending"
    );

    // Retrying the denied id: the token is simply invalid and nothing is
    // injected.
    let retry = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 5, "y": 5, "confirm_id": denied_id}),
            &context(&fixture.workspace),
        )
        .await;
    let text = retry.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        text.contains("invalid, expired, or was already used"),
        "{text}"
    );
    assert!(fixture.mock.lock().clicked.is_empty());
    assert!(
        !fixture.shared.mint_confirmation(&denied_id),
        "a denied id must not mint"
    );

    // An identical retry of the SAME action is NOT refused server-side: it
    // re-raises the blocking dialog (a fresh confirm_required event, one
    // more than before) — no denial memory.
    let events_before = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    let again = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 5, "y": 5}),
            &context(&fixture.workspace),
        )
        .await;
    let again_text = again.ok().map(|r| r.content).unwrap_or_default();
    assert!(again_text.contains("NOT executed"), "{again_text}");
    assert!(
        !again_text.contains("denied"),
        "a retry after denial must not be refused: {again_text}"
    );
    let events_after = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    assert_eq!(
        events_after
            .iter()
            .filter(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
            .count(),
        events_before
            .iter()
            .filter(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
            .count()
            + 1,
        "an identical retry must emit a fresh confirm_required event"
    );
    assert!(fixture.mock.lock().clicked.is_empty());
    let fresh_id = latest_confirm_id(&fixture.events);
    assert_ne!(
        fresh_id, denied_id,
        "a retry after denial must mint a new confirm_id"
    );
    assert!(
        fixture.shared.pending_confirmation(&fresh_id).is_some(),
        "the fresh pending must be live and confirmable"
    );
}

/// Tokens are bound to the action: a confirm_id minted for action A cannot
/// be spent on action B (tool-layer integration).
#[tokio::test]
async fn confirm_token_is_bound_to_the_action() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    fixture.mock.lock().element = Some(ElementInfo {
        role: "button".to_string(),
        name: "Buy now".to_string(),
        x: 0,
        y: 0,
        width: 10,
        height: 10,
        secure: false,
    });
    let _ = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 5, "y": 5}),
            &context(&fixture.workspace),
        )
        .await;
    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    let confirm_id = events
        .iter()
        .rev()
        .find(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
        .map(|(_, p)| p["confirm_id"].as_str().unwrap_or_default().to_string())
        .expect("confirm event");
    fixture.shared.mint_confirmation(&confirm_id);
    // Replaying a "type" with the click's token — must be rejected.
    let replay = fixture
        .tool
        .execute(
            json!({"action": "type", "text": "hi", "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    let text = replay.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        text.contains("invalid, expired, or was already used"),
        "{text}"
    );
    assert!(fixture.mock.lock().typed.is_empty());
}

/// mouse_move/scroll are real pointer input and must hold
/// a session grant.
#[tokio::test]
async fn mouse_move_requires_session_grant() {
    let (fixture, _restore) = fixture();
    let result = fixture
        .tool
        .execute(
            json!({"action": "mouse_move", "x": 3, "y": 4}),
            &context(&fixture.workspace),
        )
        .await;
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("has not granted control"), "{text}");
    assert!(fixture.mock.lock().moved_to.is_empty(), "must not move");
}

/// Capabilities first: a platform without input capability is rejected before
/// the grant gate — no grant prompt, no budget spent.
#[tokio::test]
async fn unsupported_input_platform_is_rejected_before_grant_prompt() {
    let (fixture, _restore) = fixture();
    fixture.mock.lock().no_input_cap = true;
    let result = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 1, "y": 2}),
            &context(&fixture.workspace),
        )
        .await;
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("input injection is unsupported"), "{text}");
    assert!(
        !text.contains("has not granted control"),
        "must not ask for a grant on an incapable platform: {text}"
    );
    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    assert!(
        !events.iter().any(|(name, _)| name == EVENT_GRANT_REQUIRED),
        "no grant prompt expected, got {events:?}"
    );
    // Capability rejections must leave a trace.
    let records = read_audit_records(&fixture);
    let rejected: Vec<&AuditRecord> = records
        .iter()
        .filter(|r| r.result.as_deref() == Some("rejected") && r.action == "left_click")
        .collect();
    assert_eq!(
        rejected.len(),
        1,
        "the capability-rejected click must leave exactly one audit record: {records:?}"
    );
    assert_eq!(rejected[0].consent, "rejected:capability:input-unsupported");
}

/// The schema enum, the unknown-action error text, and parse_action's
/// dispatch must all agree.
#[tokio::test]
async fn schema_actions_match_parser() {
    let (fixture, _restore) = fixture();
    let schema = fixture.tool.input_schema();
    let enum_values = schema["properties"]["action"]["enum"]
        .as_array()
        .expect("enum");
    let listed: Vec<&str> = enum_values
        .iter()
        .map(|v| v.as_str().expect("string enum entry"))
        .collect();
    assert_eq!(
        listed, SUPPORTED_ACTIONS,
        "schema enum must match SUPPORTED_ACTIONS"
    );
    for action in SUPPORTED_ACTIONS {
        assert!(
            parse_action(&minimal_input(action)).is_ok(),
            "{action} should parse with minimal valid args"
        );
    }
    let err = parse_action(&json!({"action": "bogus"}));
    let text = err.err().map(|e| e.to_string()).unwrap_or_default();
    for action in SUPPORTED_ACTIONS {
        assert!(text.contains(action), "error must list {action}: {text}");
    }
}

/// The minimal valid arguments for each action (used by the parity test).
fn minimal_input(action: &str) -> Value {
    match action {
        "wait" => json!({"action": "wait", "ms": 1}),
        "element_at_point" => json!({"action": "element_at_point", "x": 1, "y": 2}),
        "mouse_move" => json!({"action": "mouse_move", "x": 1, "y": 2}),
        "scroll" => json!({"action": "scroll", "direction": "down", "amount": 1}),
        "left_click_drag" => {
            json!({"action": "left_click_drag", "start_x": 1, "start_y": 2, "x": 3, "y": 4})
        }
        "type" => json!({"action": "type", "text": "a"}),
        "key" => json!({"action": "key", "text": "a"}),
        "hold_key" => json!({"action": "hold_key", "text": "a", "ms": 10}),
        other => json!({"action": other}),
    }
}

// ---------------------------------------------------------------------------
// Confirmation summaries: a plain parameter summary (type N characters); the
// full text goes out via type_preview_full
// ---------------------------------------------------------------------------

/// The Type summary is exactly `type N characters`: no raw text, no
/// fingerprint suffix, no inline preview. For a non-secure target the
/// eligible full text rides `type_preview_full` instead; control characters
/// are the frontend's concern, not the summary's.
#[tokio::test]
async fn type_summary_is_a_plain_character_count() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    fixture.mock.lock().focused = Some(ElementInfo {
        // Non-secure denylisted focused element: the action is blocked (T3)
        // but the typing target is NOT a password/secure field.
        role: "AXButton".to_string(),
        name: "Send message".to_string(),
        x: 0,
        y: 0,
        width: 10,
        height: 10,
        secure: false,
    });
    // 20 chars, including a control char
    // the old in-summary preview had to escape.
    let text = "hello\u{7}world123456789".to_string();
    let result = fixture
        .tool
        .execute(
            json!({"action": "type", "text": text}),
            &context(&fixture.workspace),
        )
        .await;
    assert!(
        result
            .ok()
            .map(|r| r.content)
            .unwrap_or_default()
            .contains("NOT executed")
    );
    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    let payload = events
        .iter()
        .rev()
        .find(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
        .map(|(_, p)| p.clone())
        .expect("confirm event carries the action summary");
    // Canonical action name + the English parameter summary under `summary`.
    assert_eq!(payload["action"], "type", "{payload}");
    let summary = payload["summary"].as_str().unwrap_or_default();

    // Exactly the plain character count — no `[fp …]` suffix, nothing else.
    assert_eq!(summary, "type 20 characters", "{summary}");
    // The raw text never rides the dialog summary.
    assert!(!summary.contains("hello"), "{summary}");
    assert!(!summary.contains('\u{7}'), "{summary:?}");
    // Structured typing fields disclose the count and the full preview.
    assert_eq!(payload["text_length"], 20, "{payload}");
    assert_eq!(payload["text_preview"], text, "{payload}");
    assert_eq!(payload["text_preview_truncated"], false, "{payload}");
    // The full text rides type_preview_full for this non-secure target.
    assert_eq!(
        payload["type_preview_full"].as_str(),
        Some(text.as_str()),
        "the expander payload carries the full text: {payload}"
    );
    // Non-execution pin: see the preview tests below.
    assert!(
        fixture.mock.lock().typed.is_empty(),
        "the blocked type must not reach the injection surface"
    );
}

/// For a secure (password) typing target the summary is the masked
/// character count: no plaintext and no full-text preview is ever emitted
/// for it.
#[tokio::test]
async fn type_summary_masks_preview_for_secure_targets() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    fixture.mock.lock().focused = Some(ElementInfo {
        role: "AXSecureTextField".to_string(),
        name: "Password".to_string(),
        x: 0,
        y: 0,
        width: 10,
        height: 10,
        secure: true,
    });
    let text = "hunter2secret!".to_string();
    let result = fixture
        .tool
        .execute(
            json!({"action": "type", "text": text}),
            &context(&fixture.workspace),
        )
        .await;
    assert!(
        result
            .ok()
            .map(|r| r.content)
            .unwrap_or_default()
            .contains("NOT executed")
    );
    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    let payload = events
        .iter()
        .rev()
        .find(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
        .map(|(_, p)| p.clone())
        .expect("confirm event carries the action summary");
    let summary = payload["summary"].as_str().unwrap_or_default();

    // Plaintext must not appear in the summary; the summary is only a
    // character count.
    assert_eq!(summary, "type 14 characters", "{summary}");
    assert!(!summary.contains("hunter2"), "{summary}");
    // Masked target: the count may ride (length only), but no preview of any
    // form and no truncation flag.
    assert_eq!(payload["text_length"], 14, "{payload}");
    assert!(
        payload.get("text_preview").is_none(),
        "secure targets must not carry a preview: {payload}"
    );
    assert_eq!(payload["text_preview_truncated"], false, "{payload}");
    // No full-text preview and no plaintext anywhere in the payload.
    assert!(
        payload.get("type_preview_full").is_none(),
        "secure targets must not carry the full preview: {payload}"
    );
    assert!(
        !serde_json::to_string(&payload)
            .unwrap_or_default()
            .contains("hunter2"),
        "password text must not leak anywhere in the payload: {payload}"
    );
}

// ---------------------------------------------------------------------------
// Audit redaction (single-character chords log only a key count; multi-key
// shortcut chords keep the plaintext)
// ---------------------------------------------------------------------------

/// Single-character chords are audited redacted as `pressed 1 key` — never
/// the character, regardless of the modifier: a password
/// spelled one `alt+x` call at a time would otherwise land in the log as
/// plaintext `keys: alt+x` records; classification is by content, not token
/// position, so duplicate-modifier chords like `shift+shift+h` — which types
/// a capital H — are covered too). Multi-key shortcut chords keep the
/// readable `keys: <chord>` target, and no crypto fields exist in the JSONL.
#[tokio::test]
async fn single_char_key_chord_is_audited_as_typed_text() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    for chord in [
        "p",
        "shift+h",
        "h+shift",
        "shift+shift+h",
        "shift+h+shift",
        "h+shift+shift",
        "ctrl+s",
    ] {
        let result = fixture
            .tool
            .execute(
                json!({"action": "key", "text": chord}),
                &context(&fixture.workspace),
            )
            .await;
        let result = match result {
            Ok(r) => r,
            Err(e) => panic!("execute failed for {chord}: {e}"),
        };
        assert!(result.success, "{chord}: {}", result.content);
    }
    // A typing-form hold_key records the held duration on the redacted
    // target (`pressed 1 key held for Nms`).
    let held = fixture
        .tool
        .execute(
            json!({"action": "hold_key", "text": "p", "ms": 10}),
            &context(&fixture.workspace),
        )
        .await;
    let held = match held {
        Ok(r) => r,
        Err(e) => panic!("hold_key execute failed: {e}"),
    };
    assert!(held.success, "{}", held.content);

    let raw = std::fs::read_to_string(session_audit_path()).expect("audit jsonl exists");
    let records = read_audit_records(&fixture);
    let keys: Vec<&AuditRecord> = records.iter().filter(|r| r.action == "key").collect();
    assert_eq!(keys.len(), 7, "seven key calls: {records:?}");
    let holds: Vec<&AuditRecord> = records.iter().filter(|r| r.action == "hold_key").collect();
    assert_eq!(holds.len(), 1, "one hold_key call: {records:?}");

    let typed: Vec<&&AuditRecord> = keys
        .iter()
        .filter(|r| r.target.as_deref() == Some("pressed 1 key"))
        .collect();
    assert_eq!(
        typed.len(),
        7,
        "every single-character chord (any modifier) is audited count-only: {records:?}"
    );
    // No single-character chord keeps a plaintext target — ctrl+s included.
    assert!(
        keys.iter().all(|r| !r
            .target
            .as_deref()
            .unwrap_or_default()
            .starts_with("keys: ")),
        "single-character chords must never log their character: {records:?}"
    );
    // The held typing-form chord carries the held-ms suffix, no character.
    assert_eq!(
        holds[0].target.as_deref(),
        Some("pressed 1 key held for 10ms"),
        "hold_key appends the held duration to the redacted target"
    );
    // No crypto fields exist at all: the audit is a plain log.
    for absent in ["salt", "text_hmac", "text_len", "phase"] {
        assert!(!raw.contains(absent), "{absent} in {raw}");
    }
    // Plaintext of typed-text chords (incl. any shift+char permutation)
    // must never appear in keys: form.
    assert!(!raw.contains("keys: p"));
    assert!(!raw.contains("keys: shift+h"));
    assert!(!raw.contains("keys: h+shift"));
    assert!(!raw.contains("keys: shift+shift+h"));
    assert!(!raw.contains("keys: shift+h+shift"));
    assert!(!raw.contains("keys: h+shift+shift"));
}

/// A chord with character keys
/// (≤4 tokens) spells out text in ≤4-character chunks — the audit logs only
/// the key count (`pressed 4 keys`), never the plaintext. Mixed chords
/// (`p+a+Return`, `esc+h+u+n`) inject their character keys too; the old
/// implementation logged the whole thing as plaintext `keys: <chord>` as long
/// as a named key was mixed in. Pure named-key chords (`Return`) inject no
/// characters and keep the readable `keys: <chord>`.
#[tokio::test]
async fn multi_char_letter_chords_are_audited_as_counts_only() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    for chord in ["p+a+s+s", "h+u+n", "p+a+Return", "esc+h+u+n"] {
        let result = fixture
            .tool
            .execute(
                json!({"action": "key", "text": chord}),
                &context(&fixture.workspace),
            )
            .await;
        let result = match result {
            Ok(r) => r,
            Err(e) => panic!("execute failed for {chord}: {e}"),
        };
        assert!(result.success, "{chord}: {}", result.content);
    }
    // A named-key chord is a shortcut/editing action, not spelled text: it
    // keeps the readable plaintext target.
    let named = fixture
        .tool
        .execute(
            json!({"action": "key", "text": "Return"}),
            &context(&fixture.workspace),
        )
        .await;
    let named = match named {
        Ok(r) => r,
        Err(e) => panic!("execute failed for Return: {e}"),
    };
    assert!(named.success, "{}", named.content);

    let raw = std::fs::read_to_string(session_audit_path()).expect("audit jsonl exists");
    assert!(
        !raw.contains("p+a+s+s")
            && !raw.contains("h+u+n")
            && !raw.contains("p+a+Return")
            && !raw.contains("esc+h+u+n"),
        "letter chunks (mixed with named keys or not) must never reach the log as plaintext: {raw}"
    );
    let records = read_audit_records(&fixture);
    let targets: Vec<&str> = records
        .iter()
        .filter(|r| r.action == "key")
        .filter_map(|r| r.target.as_deref())
        .collect();
    assert_eq!(
        targets,
        vec![
            "pressed 4 keys",
            "pressed 3 keys",
            "pressed 3 keys",
            "pressed 4 keys",
            "keys: Return",
        ],
        "any chord carrying a character key logs counts, named keys stay readable: {records:?}"
    );
}

// ---------------------------------------------------------------------------
// Audit fail-open (informational log) / mixed-DPI out-of-bounds guard
// ---------------------------------------------------------------------------

/// The audit is an informational, fail-open log: when the audit directory
/// cannot be created (PINVOU3_HOME points at a plain file), the append
/// fails, the caller warns via eprintln, and the action still executes —
/// for every action class.
#[tokio::test]
// ENV_LOCK must be held across the awaits: run() resolves PINVOU3_HOME from
// the process-level env on a spawn_blocking thread, so both the env and the
// lock must live until the test ends (same convention as fixture()).
#[allow(clippy::await_holding_lock)]
async fn audit_unavailable_fails_open_and_actions_still_execute() {
    let _env_lock = crate::platform::paths::tests::ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let previous = std::env::var_os("PINVOU3_HOME");
    let blocker_dir = tempfile::Builder::new()
        .prefix("pinvou3-cu-blocker-")
        .tempdir()
        .expect("temp blocker dir");
    let blocker = blocker_dir.path().join("not-a-directory");
    std::fs::write(&blocker, b"not a directory").expect("write blocker file");
    // SAFETY: holding platform::paths::tests::ENV_LOCK, so env writes are
    // serialized in-process.
    unsafe { std::env::set_var("PINVOU3_HOME", &blocker) };
    // Restore via the fixture's EnvRestore guard: the guard runs even when
    // an assertion (or the awaits) panic, so the overridden env cannot leak
    // into other tests. Declared after env_lock, so the env is restored
    // while the lock is still held.
    let _env_restore = EnvRestore(previous);

    // The workspace is decoupled from the audit directory: it lives in its
    // own temp directory.
    let workspace_dir = tempfile::Builder::new()
        .prefix("pinvou3-cu-nows-")
        .tempdir()
        .expect("temp workspace dir");
    let workspace = workspace_dir.path().to_path_buf();
    let _ = std::fs::create_dir_all(&workspace);

    let shared = Arc::new(ComputerUseShared::new());
    shared.set_enabled(true);
    shared.grant_session("s-test");
    let mock = Arc::new(Mutex::new(MockState::default()));
    let events = Arc::new(StdMutex::new(Vec::new()));
    let mock_for_factory = Arc::clone(&mock);
    let backend = BackendHandle::lazy(move || {
        Ok(Box::new(MockBackend {
            state: mock_for_factory,
        }) as Box<dyn ComputerUseBackend>)
    });
    let tool = ComputerUseTool::with_parts(
        "s-test".to_string(),
        Arc::clone(&shared),
        backend,
        Arc::new(RecordingSink(Arc::clone(&events))),
    );

    // Input class: audit unavailability only degrades to eprintln; the
    // action executes as usual.
    let typed = tool
        .execute(
            json!({"action": "type", "text": "hello"}),
            &context(&workspace),
        )
        .await;
    let typed = match typed {
        Ok(r) => r,
        Err(e) => panic!("type failed: {e}"),
    };
    assert!(typed.success, "{}", typed.content);
    assert_eq!(
        mock.lock().typed.last().map(String::as_str),
        Some("hello"),
        "the action must execute although the audit append failed"
    );

    // Observe class: likewise executes as usual.
    let shot = tool
        .execute(json!({"action": "screenshot"}), &context(&workspace))
        .await;
    let shot = match shot {
        Ok(r) => r,
        Err(e) => panic!("screenshot failed: {e}"),
    };
    assert!(shot.success, "{}", shot.content);
}

/// Mixed-DPI guard: when the cursor is outside the captured monitor,
/// cursor_position does no conversion — it reports the raw input coordinates
/// with warning text attached.
#[tokio::test]
async fn cursor_outside_captured_monitor_reports_input_position_with_warning() {
    let (fixture, _restore) = fixture();
    // The captured monitor spans (-1000,-1000)..(0,0); the cursor (7,9) is on
    // another screen.
    fixture.mock.lock().capture_origin = (-1000, -1000);
    let _ = fixture
        .tool
        .execute(
            json!({"action": "screenshot"}),
            &context(&fixture.workspace),
        )
        .await;
    let result = fixture
        .tool
        .execute(
            json!({"action": "cursor_position"}),
            &context(&fixture.workspace),
        )
        .await;
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        text.contains("cursor is at (7, 9) in global input coordinates"),
        "{text}"
    );
    assert!(text.contains("outside the captured monitor"), "{text}");
    assert!(text.contains("warning: cursor is outside"), "{text}");
    assert!(!text.contains("in screenshot space"), "{text}");
}

/// Mixed DPI: when the cursor is outside the captured monitor there is no
/// target to check (never screen with cross-screen junk coordinates) —
/// screening being unavailable does not block execution nor ask for
/// confirmation.
#[tokio::test]
async fn mouse_down_outside_captured_monitor_executes_without_confirmation() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    fixture.mock.lock().capture_origin = (-1000, -1000);
    let result = fixture
        .tool
        .execute(
            json!({"action": "left_mouse_down"}),
            &context(&fixture.workspace),
        )
        .await;
    let result = match result {
        Ok(r) => r,
        Err(e) => panic!("execute failed: {e}"),
    };
    assert!(result.success, "{}", result.content);
    assert!(
        !result.content.contains("NOT executed"),
        "an unlocatable cursor must not be gated: {}",
        result.content
    );
    assert_eq!(fixture.mock.lock().downed.len(), 1, "the down executed");
    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    assert!(
        !events
            .iter()
            .any(|(name, _)| name == EVENT_CONFIRM_REQUIRED),
        "no confirmation may be requested while the target cannot be located: {events:?}"
    );
}

/// Retina-style mixed-DPI regression: capture 200x200 device pixels
/// with input_scale 0.5 (i.e. a 100x100-point monitor). cursor_position
/// reports input coordinates directly:
/// - Cursor at input (60,40) (inside the capture) → the coordinate-less down
///   screens at (60,40), hits a denylist control, and is blocked;
/// - Cursor at input (150,40) (outside the input rect [0,100)) → no target
///   to check; the action executes as usual with zero confirm events;
///   cursor_position reports the raw input coordinates with a warning;
/// - Cursor back at (60,40) → cursor_position reports the exact screenshot
///   coordinates (120, 80) (input_to_shot's ×2 inverse conversion).
#[tokio::test]
async fn retina_input_space_cursor_screens_inside_and_reports_exact_coords() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    {
        let mut mock = fixture.mock.lock();
        mock.capture_size = (200, 200);
        mock.input_scale = (0.5, 0.5);
    }
    // Establish the scale map (shot 200x200; input coordinates = screenshot
    // coordinates × 0.5).
    let _ = fixture
        .tool
        .execute(
            json!({"action": "screenshot"}),
            &context(&fixture.workspace),
        )
        .await;

    // A consequential control sits at the cursor (60,40): the coordinate-less
    // down must screen at the input point (60,40).
    fixture.mock.lock().cursor = (60, 40);
    fixture.mock.lock().element = Some(ElementInfo {
        role: "AXButton".to_string(),
        name: "Buy now".to_string(),
        x: 50,
        y: 30,
        width: 20,
        height: 20,
        secure: false,
    });
    let blocked = fixture
        .tool
        .execute(
            json!({"action": "left_mouse_down"}),
            &context(&fixture.workspace),
        )
        .await;
    let blocked_text = blocked.ok().map(|r| r.content).unwrap_or_default();
    assert!(blocked_text.contains("NOT executed"), "{blocked_text}");
    assert!(
        fixture.mock.lock().downed.is_empty(),
        "a blocked down must not inject"
    );

    // The cursor (150,40) is outside the captured monitor's input rect: no
    // screening, executes, zero confirmations.
    fixture.mock.lock().cursor = (150, 40);
    let executed = fixture
        .tool
        .execute(
            json!({"action": "left_mouse_down"}),
            &context(&fixture.workspace),
        )
        .await;
    let executed = match executed {
        Ok(r) => r,
        Err(e) => panic!("execute failed: {e}"),
    };
    assert!(executed.success, "{}", executed.content);
    assert_eq!(fixture.mock.lock().downed.len(), 1, "the down executed");
    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    assert_eq!(
        events
            .iter()
            .filter(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
            .count(),
        1,
        "only the blocked down may raise a confirmation: {events:?}"
    );

    // With the cursor outside the rect, cursor_position reports input
    // coordinates + a warning, not screenshot coordinates.
    let outside = fixture
        .tool
        .execute(
            json!({"action": "cursor_position"}),
            &context(&fixture.workspace),
        )
        .await;
    let outside_text = outside.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        outside_text.contains("cursor is at (150, 40) in global input coordinates"),
        "{outside_text}"
    );
    assert!(
        outside_text.contains("outside the captured monitor"),
        "{outside_text}"
    );
    assert!(
        !outside_text.contains("in screenshot space"),
        "{outside_text}"
    );

    // Cursor back at (60,40): converted exactly to the screenshot coordinates
    // (120, 80).
    fixture.mock.lock().cursor = (60, 40);
    let inside = fixture
        .tool
        .execute(
            json!({"action": "cursor_position"}),
            &context(&fixture.workspace),
        )
        .await;
    let inside_text = inside.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        inside_text.contains("cursor is at (120, 80) in screenshot space"),
        "{inside_text}"
    );
}

// ---------------------------------------------------------------------------
// Chords never get a form-based confirmation /
// mouse_move·scroll are not screened
// ---------------------------------------------------------------------------

/// Chords never require a confirmation (pin (d)): chord editing is
/// reversible and no mainstream product gates by chord shape. No matter the
/// modifier combination (cmd+delete, ctrl+Enter, shift+delete…), key executes
/// directly; denylist/password screening of the focused element still applies
/// (see the other T3 tests).
#[tokio::test]
async fn key_chords_never_require_confirmation() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // Focus is an ordinary text area (denylist verdict Clear) — the old
    // position would have blocked it by chord semantics.
    fixture.mock.lock().focused = Some(ElementInfo {
        role: "AXTextArea".to_string(),
        name: String::new(),
        x: 0,
        y: 0,
        width: 10,
        height: 10,
        secure: false,
    });
    for chord in [
        "cmd+delete",
        "ctrl+backspace",
        "meta+Enter",
        "ctrl+Enter",
        // The old shift-involved verdict is gone too: shift+delete executes
        // directly.
        "shift+delete",
        "shift+Backspace",
        // Ordinary keys (the main text-editing path) execute directly as
        // before.
        "Return",
        "Delete",
        "Backspace",
        "shift+Return",
    ] {
        let result = fixture
            .tool
            .execute(
                json!({"action": "key", "text": chord}),
                &context(&fixture.workspace),
            )
            .await;
        let result = match result {
            Ok(r) => r,
            Err(e) => panic!("execute failed for {chord}: {e}"),
        };
        assert!(result.success, "{chord}: {}", result.content);
        assert!(
            !result.content.contains("NOT executed"),
            "{chord} must not require confirmation: {}",
            result.content
        );
    }
    assert_eq!(
        fixture.mock.lock().chords.len(),
        10,
        "every chord must reach the injection"
    );
    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    assert!(
        !events
            .iter()
            .any(|(name, _)| name == EVENT_CONFIRM_REQUIRED),
        "no chord may raise a confirmation: {events:?}"
    );
}

/// mouse_move and scroll never require a confirmation (pin (e)): hover and
/// scroll have no action consequences, consistent with mainstream products —
/// they execute even when the landing point/target sits on a denylist
/// control; the same applies to a move while the left button is held
/// (held-move re-screening was removed; drag screening is fixed at the Drag
/// action's start + end points).
#[tokio::test]
async fn mouse_move_and_scroll_never_require_confirmation() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // The denylist control "Pay" covers the cursor (7,9) and the whole
    // screenshot: scroll / mouse_move execute as usual.
    fixture.mock.lock().element = Some(ElementInfo {
        role: "AXButton".to_string(),
        name: "Pay".to_string(),
        x: 0,
        y: 0,
        width: 16,
        height: 16,
        secure: false,
    });
    for input in [
        json!({"action": "scroll", "direction": "down", "amount": 2}),
        json!({"action": "scroll", "direction": "down", "amount": 1, "x": 3, "y": 4}),
    ] {
        let result = fixture
            .tool
            .execute(input, &context(&fixture.workspace))
            .await;
        let result = match result {
            Ok(r) => r,
            Err(e) => panic!("execute failed: {e}"),
        };
        assert!(result.success, "{}", result.content);
        assert!(
            !result.content.contains("NOT executed"),
            "scroll must not require confirmation: {}",
            result.content
        );
    }
    assert_eq!(
        fixture.mock.lock().scrolled.len(),
        2,
        "both scrolls reached the injection"
    );
    let result = fixture
        .tool
        .execute(
            json!({"action": "mouse_move", "x": 3, "y": 4}),
            &context(&fixture.workspace),
        )
        .await;
    let result = match result {
        Ok(r) => r,
        Err(e) => panic!("execute failed: {e}"),
    };
    assert!(result.success, "{}", result.content);
    assert!(
        !result.content.contains("NOT executed"),
        "mouse_move must not require confirmation: {}",
        result.content
    );
    let moves_before_held = fixture.mock.lock().moved_to.len();

    // A move while the left button is held (effectively a drag) likewise only
    // executes — no re-screening. The down needs the cursor point to screen
    // Clear: place a benign element first, then swap in a denylist control.
    fixture.mock.lock().element = Some(benign_element(0, 0, 16, 16));
    let down = fixture
        .tool
        .execute(
            json!({"action": "left_mouse_down"}),
            &context(&fixture.workspace),
        )
        .await;
    let down_text = down.ok().map(|r| r.content).unwrap_or_default();
    assert!(down_text.contains("mouse button is down"), "{down_text}");
    fixture.mock.lock().element = Some(ElementInfo {
        role: "AXButton".to_string(),
        name: "Trash".to_string(),
        x: 10,
        y: 10,
        width: 5,
        height: 5,
        secure: false,
    });
    let held_move = fixture
        .tool
        .execute(
            json!({"action": "mouse_move", "x": 12, "y": 12}),
            &context(&fixture.workspace),
        )
        .await;
    let held_text = held_move.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        held_text.contains("mouse moved to"),
        "a held move must execute without confirmation: {held_text}"
    );
    // up: the cursor (7,9) is not inside the Trash rect → Clear → executes.
    let up = fixture
        .tool
        .execute(
            json!({"action": "left_mouse_up"}),
            &context(&fixture.workspace),
        )
        .await;
    let up_text = up.ok().map(|r| r.content).unwrap_or_default();
    assert!(up_text.contains("mouse button is up"), "{up_text}");
    assert_eq!(
        fixture.mock.lock().moved_to.len(),
        moves_before_held + 1,
        "exactly the held move injected past the first hover"
    );

    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    assert!(
        !events
            .iter()
            .any(|(name, _)| name == EVENT_CONFIRM_REQUIRED),
        "no scroll/move may raise a confirmation: {events:?}"
    );
}

/// The denylist narrowed to consequence categories (pin (a)): generic
/// affirmatives (OK/Continue/Run) get no confirmation and execute directly;
/// consequence-category words (Pay/Delete/Submit/Accept) still block and ask
/// for confirmation.
#[tokio::test]
async fn trimmed_denylist_affirmatives_execute_and_consequences_confirm() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");

    // Generic affirmatives: execute directly.
    for name in ["OK", "Continue", "Run"] {
        fixture.mock.lock().element = Some(ElementInfo {
            role: "AXButton".to_string(),
            name: name.to_string(),
            x: 0,
            y: 0,
            width: 16,
            height: 16,
            secure: false,
        });
        let result = fixture
            .tool
            .execute(
                json!({"action": "left_click", "x": 5, "y": 5}),
                &context(&fixture.workspace),
            )
            .await;
        let result = match result {
            Ok(r) => r,
            Err(e) => panic!("execute failed for {name}: {e}"),
        };
        assert!(result.success, "{name}: {}", result.content);
        assert!(
            !result.content.contains("NOT executed"),
            "\"{name}\" must not require confirmation: {}",
            result.content
        );
    }
    assert_eq!(
        fixture.mock.lock().clicked.len(),
        3,
        "all affirmative-label clicks executed"
    );

    // Consequence-category words: block + confirm event.
    for name in ["Pay", "Delete", "Submit", "Accept"] {
        fixture.mock.lock().element = Some(ElementInfo {
            role: "AXButton".to_string(),
            name: name.to_string(),
            x: 0,
            y: 0,
            width: 16,
            height: 16,
            secure: false,
        });
        let result = fixture
            .tool
            .execute(
                json!({"action": "left_click", "x": 5, "y": 5}),
                &context(&fixture.workspace),
            )
            .await;
        let text = result.ok().map(|r| r.content).unwrap_or_default();
        assert!(text.contains("NOT executed"), "{name}: {text}");
        assert!(text.contains("confirm_id"), "{name}: {text}");
    }
    assert_eq!(
        fixture.mock.lock().clicked.len(),
        3,
        "no consequence-labeled click may execute"
    );
}

// ---------------------------------------------------------------------------
// Revoke/stop must terminate the persistent OS-level
// grant (registry → release_os_grant)
// ---------------------------------------------------------------------------

/// Revoke triggers an emergency release via the registry (a detached thread,
/// waited on by polling): the physical left button is released first, then
/// the backend's persistent OS-level grant is closed (worker channel
/// serialization guarantees this order). The registration is kept (cleanup is
/// idempotent; a session granted again must still be reachable by a later
/// global stop); a live tool unregisters via Drop's
/// `release_and_unregister`. The Wayland portal session thereby ends with the
/// user's "stop control" instead of living until process exit.
#[tokio::test]
async fn emergency_release_keeps_registration_releases_button_then_os_grant() {
    let (fixture, _restore) = fixture();
    assert!(
        fixture.shared.backends.contains("s-test"),
        "tool construction must register its backend handle"
    );
    fixture.shared.backends.emergency_release("s-test");
    assert!(
        fixture.shared.backends.contains("s-test"),
        "emergency_release must keep the registration (only tool Drop unregisters)"
    );
    // Phase one: the physical left button release reaches the backend first.
    let mut upped = false;
    for _ in 0..300 {
        if !fixture.mock.lock().upped.is_empty() {
            upped = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(upped, "emergency_mouse_up must reach the backend");
    // Phase two: the OS-level grant close arrives afterwards (channel
    // serialization: released can only be incremented after the mouse_up
    // request has finished executing).
    let mut released = false;
    for _ in 0..300 {
        if fixture.mock.lock().released > 0 {
            released = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(released, "release_os_grant must reach the backend");
    let mock = fixture.mock.lock();
    // All three buttons released one by one (the documented contract of
    // emergency_mouse_up, see backend.rs).
    assert_eq!(
        mock.upped,
        vec![MouseButton::Left, MouseButton::Right, MouseButton::Middle]
    );
}

/// Tool drop goes through the emergency release: the physical left button
/// and the OS-level grant are cleaned up together (a model that pressed the
/// left button and died must not leave the user's machine in a held-drag
/// state), and the registration is removed.
#[tokio::test]
async fn dropping_the_tool_releases_the_button_and_os_grant() {
    let (fixture, _restore) = fixture();
    assert!(fixture.shared.backends.contains("s-test"));
    drop(fixture.tool);
    let mut done = false;
    for _ in 0..300 {
        let mock = fixture.mock.lock();
        if mock.released > 0 && !mock.upped.is_empty() {
            done = true;
            break;
        }
        drop(mock);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(done, "tool drop must release button + OS grant");
    assert!(
        !fixture.shared.backends.contains("s-test"),
        "tool drop must unregister its backend handle"
    );
}

/// Tool drop unregisters: otherwise the registry handle would pin the worker
/// thread (and its portal session) until process exit.
#[tokio::test]
async fn dropping_the_tool_unregisters_its_backend_handle() {
    let (fixture, _restore) = fixture();
    assert!(fixture.shared.backends.contains("s-test"));
    drop(fixture.tool);
    assert!(
        !fixture.shared.backends.contains("s-test"),
        "tool drop must unregister its backend handle"
    );
}

/// Session end = the engine reclaiming the tool
/// (Drop). Drop must revoke this session's grant and wipe its consent
/// artifacts (pending confirmations, minted tokens) — consent state must not
/// outlive the tool holding it; other sessions' grants and artifacts are
/// unaffected.
#[tokio::test]
async fn dropping_the_tool_revokes_the_grant_and_consent_artifacts() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    fixture.shared.grant_session("s-other");
    let summary = "left click x1 at Some((5, 5))";
    let own_token = fixture
        .shared
        .new_pending_confirmation("s-test", summary, "Buy now", 0);
    assert!(fixture.shared.mint_confirmation(&own_token));
    let other_token = fixture
        .shared
        .new_pending_confirmation("s-other", summary, "Buy now", 0);
    assert!(fixture.shared.mint_confirmation(&other_token));

    drop(fixture.tool);

    // This session: the grant and all consent artifacts are wiped.
    assert!(!fixture.shared.has_active_grant("s-test"));
    assert_eq!(
        fixture.shared.begin_input_action("s-test"),
        Err(GuardRejection::GrantRequired)
    );
    assert_eq!(
        fixture
            .shared
            .take_confirmation(&own_token, "s-test", summary, 0),
        ConfirmationCheck::Unknown,
        "tool drop must wipe the session's minted approval tokens"
    );
    // Other sessions: grants and artifacts kept intact.
    assert!(fixture.shared.has_active_grant("s-other"));
    assert_eq!(
        fixture
            .shared
            .take_confirmation(&other_token, "s-other", summary, 0),
        ConfirmationCheck::Granted,
        "tool drop must not touch other sessions' consent artifacts"
    );
}

// ---------------------------------------------------------------------------
// Regression: element-free targets screen Clear / approved tokens spend
// directly / stable audit error code
// ---------------------------------------------------------------------------

/// Latest confirm_id from the most recent confirm_required event.
fn latest_confirm_id(events: &Arc<StdMutex<Vec<(String, Value)>>>) -> String {
    events
        .lock()
        .map(|e| e.clone())
        .unwrap_or_default()
        .iter()
        .rev()
        .find(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
        .map(|(_, p)| p["confirm_id"].as_str().unwrap_or_default().to_string())
        .expect("confirm event")
}

/// Regression: a target point with no a11y element screens Clear — unnamed
/// targets are everywhere on real desktops (canvas, hover targets, custom
/// widgets) and the denylist is name-based, so an absent name is not a red
/// flag. The click executes without any confirmation; a query *error* also
/// screens Clear (see `a11y_query_error_executes_without_confirmation`).
#[tokio::test]
async fn click_without_a11y_element_executes_without_confirmation() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // No element at all at the (5,5) target.
    let result = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 5, "y": 5}),
            &context(&fixture.workspace),
        )
        .await;
    let result = match result {
        Ok(r) => r,
        Err(e) => panic!("execute failed: {e}"),
    };
    assert!(result.success, "{}", result.content);
    assert!(
        !result.content.contains("NOT executed"),
        "an unnamed target must not be gated: {}",
        result.content
    );
    assert_eq!(fixture.mock.lock().clicked.len(), 1, "the click executed");
    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    assert!(
        !events
            .iter()
            .any(|(name, _)| name == EVENT_CONFIRM_REQUIRED),
        "no confirmation may be requested for an unnamed target: {events:?}"
    );
}

/// A granted token spends directly: no spend-time re-screen and no
/// "world changed" re-request arm (the mainstream model — the API
/// confirmation is one per-action id the client acknowledges). Even when
/// the target now reads differently than at mint time, the approval
/// executes; the token is single-use afterwards.
#[tokio::test]
async fn approved_token_executes_without_rescreening_when_the_world_changed() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    fixture.mock.lock().element = Some(ElementInfo {
        role: "AXButton".to_string(),
        name: "Buy now".to_string(),
        x: 0,
        y: 0,
        width: 10,
        height: 10,
        secure: false,
    });
    let result = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 5, "y": 5}),
            &context(&fixture.workspace),
        )
        .await;
    assert!(
        result
            .ok()
            .map(|r| r.content)
            .unwrap_or_default()
            .contains("NOT executed")
    );
    let confirm_id = latest_confirm_id(&fixture.events);
    assert!(fixture.shared.mint_confirmation(&confirm_id));

    // The world changed: the target point now holds a benign element
    // (Clear). The approval still executes — spending does not re-screen.
    fixture.mock.lock().element = Some(benign_element(0, 0, 16, 16));
    let replay = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 5, "y": 5, "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    let text = replay.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        !text.contains("NOT executed"),
        "a granted token must proceed to execution: {text}"
    );
    assert_eq!(fixture.mock.lock().clicked.len(), 1);
    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    assert_eq!(
        events
            .iter()
            .filter(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
            .count(),
        1,
        "spending must not re-request confirmation: {events:?}"
    );

    // Single-use: a further replay with the spent token is rejected.
    let again = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 5, "y": 5, "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    let text = again.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        text.contains("invalid, expired, or was already used"),
        "{text}"
    );
    assert_eq!(fixture.mock.lock().clicked.len(), 1, "nothing more ran");
}

/// Regression: the T3 "confirmation required" error is audited with only
/// the stable code in the error field — the element label (on-screen
/// content) must not reach the audit record; the model still receives the
/// full message.
#[tokio::test]
async fn t3_confirmation_error_is_audited_as_stable_code() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    fixture.mock.lock().element = Some(ElementInfo {
        role: "AXButton".to_string(),
        name: "Buy now".to_string(),
        x: 0,
        y: 0,
        width: 10,
        height: 10,
        secure: false,
    });
    let result = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 5, "y": 5}),
            &context(&fixture.workspace),
        )
        .await;
    // The model receives the full message (element label included).
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("Buy now"), "{text}");
    assert!(text.contains("NOT executed"), "{text}");

    let raw = std::fs::read_to_string(session_audit_path()).expect("audit jsonl exists");
    let records = read_audit_records(&fixture);
    assert_eq!(records.len(), 1, "one record per call: {records:?}");
    assert_eq!(records[0].result.as_deref(), Some("error"));
    assert_eq!(records[0].action, "left_click");
    assert_eq!(records[0].consent, "input:session-grant");
    assert_eq!(
        records[0].error.as_deref(),
        Some(T3_CONFIRM_REQUIRED_ERROR),
        "audit error field must be the stable code"
    );
    // The element label must not appear anywhere in the audit record.
    assert!(!raw.contains("Buy now"), "element label leaked to audit");
}

/// Audit integration: after the full approval flow of "mint a
/// pending → the user approves (mint) → execute with the confirmation (with
/// a follow-up screenshot)", the confirmed record in the session JSONL
/// satisfies the redaction contract: consent contains "t3-confirmed", target
/// is the count/coords form (coordinate parameters only, no element label
/// text), and the screenshot record carries the sha256 field.
#[tokio::test]
async fn approved_click_audit_record_meets_the_redaction_contract() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    fixture.mock.lock().element = Some(ElementInfo {
        role: "AXButton".to_string(),
        name: "Buy now".to_string(),
        x: 0,
        y: 0,
        width: 16,
        height: 16,
        secure: false,
    });

    // Mint: the blocked click raises one pending.
    let blocked = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 5, "y": 5}),
            &context(&fixture.workspace),
        )
        .await;
    assert!(
        blocked
            .ok()
            .map(|r| r.content)
            .unwrap_or_default()
            .contains("NOT executed")
    );
    let confirm_id = latest_confirm_id(&fixture.events);
    assert!(fixture.shared.mint_confirmation(&confirm_id));

    // Spend: the approved click executes and attaches a fresh screenshot.
    let approved = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 5, "y": 5, "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    let approved = match approved {
        Ok(r) => r,
        Err(e) => panic!("execute failed: {e}"),
    };
    assert!(approved.success, "{}", approved.content);

    let log = AuditLog::for_session("s-test").expect("audit log for session");
    let raw = std::fs::read_to_string(log.path()).expect("audit jsonl exists");
    let records = read_audit_records(&fixture);
    let confirmed: Vec<&AuditRecord> = records
        .iter()
        .filter(|r| r.consent.contains("t3-confirmed"))
        .collect();
    assert_eq!(
        confirmed.len(),
        1,
        "exactly one t3-confirmed record: {records:?}"
    );
    let record = confirmed[0];
    // The target is the count/coords form: coordinates only, never the
    // screened element's label text.
    assert_eq!(
        record.target.as_deref(),
        Some("left click x1 at Some((5, 5))"),
        "target must be the parameter summary: {record:?}"
    );
    assert_eq!(record.result.as_deref(), Some("ok"), "{record:?}");
    // The attached screenshot rides as sha256 + path, never pixels.
    let sha = record.screenshot_sha256.as_deref().unwrap_or_default();
    assert_eq!(sha.len(), 64, "sha256 field must be present: {record:?}");
    assert!(record.screenshot_path.is_some(), "{record:?}");
    // The element label (on-screen content) never reached the log at all.
    assert!(!raw.contains("Buy now"), "element label leaked: {raw}");
}

// ---------------------------------------------------------------------------
// Regression: failed drag releases a held left button / full type preview
// in the confirm event
// ---------------------------------------------------------------------------

/// Regression: a failed drag must not leave the physical left button stuck
/// held when this tool pressed it earlier — the tool issues a best-effort
/// release, clears the held state, and tells the model to verify.
#[tokio::test]
async fn failed_drag_releases_a_held_left_button() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // A benign element covers the cursor (7,9): the down executes and the
    // tool records the held state.
    fixture.mock.lock().element = Some(benign_element(0, 0, 16, 16));
    let down = fixture
        .tool
        .execute(
            json!({"action": "left_mouse_down"}),
            &context(&fixture.workspace),
        )
        .await;
    let down_text = down.ok().map(|r| r.content).unwrap_or_default();
    assert!(down_text.contains("mouse button is down"), "{down_text}");

    // The drag now fails at the backend.
    fixture.mock.lock().drag_error = true;
    let result = fixture
        .tool
        .execute(
            json!({"action": "left_click_drag", "start_x": 1, "start_y": 1, "x": 5, "y": 5}),
            &context(&fixture.workspace),
        )
        .await;
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("computer use action failed"), "{text}");
    assert!(text.contains("drag backend failed"), "{text}");
    assert!(
        text.contains("best-effort mouse-button release"),
        "the model must be told about the safety-net release: {text}"
    );
    assert_eq!(
        fixture.mock.lock().upped,
        vec![MouseButton::Left],
        "exactly one best-effort left-button release"
    );

    // The held state must be cleared: a second failing drag must NOT issue
    // another best-effort release (the safety net only releases a button the
    // tool itself pressed, and that state was consumed by the first release).
    fixture.mock.lock().element = None;
    let again = fixture
        .tool
        .execute(
            json!({"action": "left_click_drag", "start_x": 1, "start_y": 1, "x": 5, "y": 5}),
            &context(&fixture.workspace),
        )
        .await;
    let again_text = again.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        again_text.contains("drag backend failed"),
        "the second drag must fail at the backend again: {again_text}"
    );
    assert_eq!(
        fixture.mock.lock().upped,
        vec![MouseButton::Left],
        "exactly one best-effort release: the held state was cleared by the first"
    );
}

/// The confirm event carries the full typed text (`type_preview_full`) for
/// long, non-secure type actions so the frontend can offer a "show full
/// text" expander instead of a blind 12-char preview.
#[tokio::test]
async fn confirm_event_carries_full_type_preview_for_long_non_secure_text() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // Non-secure denylisted focused element: the action is blocked (T3) but
    // the typing target is NOT a password/secure field.
    fixture.mock.lock().focused = Some(ElementInfo {
        role: "AXButton".to_string(),
        name: "Send message".to_string(),
        x: 0,
        y: 0,
        width: 10,
        height: 10,
        secure: false,
    });
    let text = "this paragraph is long enough to be truncated in the summary".to_string();
    let result = fixture
        .tool
        .execute(
            json!({"action": "type", "text": text}),
            &context(&fixture.workspace),
        )
        .await;
    assert!(
        result
            .ok()
            .map(|r| r.content)
            .unwrap_or_default()
            .contains("NOT executed")
    );
    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    let payload = events
        .iter()
        .rev()
        .find(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
        .map(|(_, p)| p.clone())
        .expect("confirm event");
    assert_eq!(
        payload["type_preview_full"].as_str(),
        Some(text.as_str()),
        "full text must ride the confirm event: {payload}"
    );
    // The dialog summary stays a plain character count (the expander is the
    // full view), with the structured typing fields alongside it.
    let summary = payload["summary"].as_str().unwrap_or_default();
    assert_eq!(
        summary,
        format!("type {} characters", text.chars().count()),
        "summary must be the parameter summary: {payload}"
    );
    assert_eq!(payload["text_length"], text.chars().count(), "{payload}");
    assert_eq!(payload["text_preview"], text, "{payload}");
    assert_eq!(payload["text_preview_truncated"], false, "{payload}");
    // Blocked-type non-execution is pinned here too: the
    // event assertions alone would stay green if the blocked path began
    // injecting before raising the confirmation.
    assert!(
        fixture.mock.lock().typed.is_empty(),
        "the blocked type must not reach the injection surface"
    );
}

/// Short type texts DO ride `type_preview_full`: the
/// dialog would otherwise show only "type 2 characters" — approving a
/// length with zero visible content is not informed consent. The old
/// 12-char lower bound was stale logic from the removed inline preview.
#[tokio::test]
async fn confirm_event_carries_full_type_preview_even_for_short_text() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    fixture.mock.lock().focused = Some(ElementInfo {
        role: "AXButton".to_string(),
        name: "Send message".to_string(),
        x: 0,
        y: 0,
        width: 10,
        height: 10,
        secure: false,
    });
    let result = fixture
        .tool
        .execute(
            json!({"action": "type", "text": "hi"}),
            &context(&fixture.workspace),
        )
        .await;
    assert!(
        result
            .ok()
            .map(|r| r.content)
            .unwrap_or_default()
            .contains("NOT executed")
    );
    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    let payload = events
        .iter()
        .rev()
        .find(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
        .map(|(_, p)| p.clone())
        .expect("confirm event");
    assert_eq!(
        payload.get("type_preview_full").and_then(|v| v.as_str()),
        Some("hi"),
        "short non-secure text must carry the full preview: {payload}"
    );
}

// ---------------------------------------------------------------------------
// Regressions: cursor moving/unreadable does not block an approved token /
// token binding at summary granularity / injection-surface execution
// assertions
// ---------------------------------------------------------------------------

/// A cursor-acting action's approval token is bound to the session and the
/// action summary only — the pointer moving after approval does NOT
/// invalidate it (mainstream model: an approval is a per-action id; there
/// is no cursor-origin binding).
#[tokio::test]
async fn approved_cursor_action_spends_even_after_the_pointer_moved() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // The cursor defaults to (7,9), covered by a consequential target → the
    // down is blocked and a pending minted.
    fixture.mock.lock().element = Some(ElementInfo {
        role: "AXButton".to_string(),
        name: "Buy now".to_string(),
        x: 0,
        y: 0,
        width: 16,
        height: 16,
        secure: false,
    });
    let blocked = fixture
        .tool
        .execute(
            json!({"action": "left_mouse_down"}),
            &context(&fixture.workspace),
        )
        .await;
    assert!(
        blocked
            .ok()
            .map(|r| r.content)
            .unwrap_or_default()
            .contains("NOT executed")
    );
    let confirm_id = latest_confirm_id(&fixture.events);
    assert!(fixture.shared.mint_confirmation(&confirm_id));

    // Replay after the cursor moved: the token is bound only to the session
    // and the summary; the injection executes as usual.
    fixture.mock.lock().cursor = (3, 4);
    let spend = fixture
        .tool
        .execute(
            json!({"action": "left_mouse_down", "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    let spend = match spend {
        Ok(r) => r,
        Err(e) => panic!("spend execute failed: {e}"),
    };
    assert!(spend.success, "{}", spend.content);
    assert_eq!(
        fixture.mock.lock().downed.len(),
        1,
        "cursor movement after approval must not block the approved action"
    );
    // Single-use: the token has been spent.
    let again = fixture
        .tool
        .execute(
            json!({"action": "left_mouse_down", "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    let text = again.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        text.contains("invalid, expired, or was already used"),
        "{text}"
    );
    assert_eq!(fixture.mock.lock().downed.len(), 1, "nothing more ran");
}

/// The token binds the action summary AND the full action content: another
/// text with the same N characters produces the same summary
/// `type 21 characters` but a different content hash, so the swap is rejected
/// (a summary-only binding let the approved preview and the
/// executed content diverge). The user-approved original still spends the
/// token; single-use semantics unchanged.
#[tokio::test]
async fn same_summary_type_text_swap_is_rejected() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    fixture.mock.lock().focused = Some(ElementInfo {
        role: "AXSecureTextField".to_string(),
        name: "Password".to_string(),
        x: 0,
        y: 0,
        width: 10,
        height: 10,
        secure: true,
    });
    let approved_text = "correct-horse-battery"; // 21 chars
    let blocked = fixture
        .tool
        .execute(
            json!({"action": "type", "text": approved_text}),
            &context(&fixture.workspace),
        )
        .await;
    assert!(
        blocked
            .ok()
            .map(|r| r.content)
            .unwrap_or_default()
            .contains("NOT executed")
    );
    let confirm_id = latest_confirm_id(&fixture.events);
    assert!(fixture.shared.mint_confirmation(&confirm_id));

    // Same length, different content: same summary (type 21 characters) but
    // a different action binding → the token is NOT spent and nothing
    // injects (the mismatch keeps the token: the user-approved original can
    // still go through).
    let swapped = "XXXXXXXXXXXXXXXXXXXXX"; // 21 chars
    let replay = fixture
        .tool
        .execute(
            json!({"action": "type", "text": swapped, "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    let text = replay.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        text.contains("invalid, expired, or was already used"),
        "a same-summary different-content swap must not spend the token: {text}"
    );
    assert!(
        fixture.mock.lock().typed.is_empty(),
        "nothing may inject on a content swap"
    );

    // The user-approved original still spends the token.
    let spend = fixture
        .tool
        .execute(
            json!({"action": "type", "text": approved_text, "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    let spend = match spend {
        Ok(r) => r,
        Err(e) => panic!("approved replay execute failed: {e}"),
    };
    assert!(spend.success, "{}", spend.content);
    assert_eq!(
        fixture.mock.lock().typed.last().map(String::as_str),
        Some(approved_text),
        "the exact approved content executes"
    );

    // Single-use: the token has been spent; the original text's replay is
    // rejected.
    let again = fixture
        .tool
        .execute(
            json!({"action": "type", "text": approved_text, "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    let text = again.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        text.contains("invalid, expired, or was already used"),
        "{text}"
    );
    assert_eq!(
        fixture.mock.lock().typed.len(),
        1,
        "only the approved replay injects"
    );
}

/// An unreadable cursor position at spend time (the normal state before the
/// first Wayland move) is irrelevant to the token: the token binds only the
/// session and the summary, with no cursor comparison — an unreadable cursor
/// does not block executing an approved action.
#[tokio::test]
async fn unreadable_cursor_at_spend_does_not_block_a_granted_token() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    fixture.mock.lock().element = Some(ElementInfo {
        role: "AXButton".to_string(),
        name: "Buy now".to_string(),
        x: 0,
        y: 0,
        width: 16,
        height: 16,
        secure: false,
    });
    let blocked = fixture
        .tool
        .execute(
            json!({"action": "left_mouse_down"}),
            &context(&fixture.workspace),
        )
        .await;
    assert!(
        blocked
            .ok()
            .map(|r| r.content)
            .unwrap_or_default()
            .contains("NOT executed")
    );
    let confirm_id = latest_confirm_id(&fixture.events);
    assert!(fixture.shared.mint_confirmation(&confirm_id));

    fixture.mock.lock().cursor_error = true;
    let spend = fixture
        .tool
        .execute(
            json!({"action": "left_mouse_down", "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    let spend = match spend {
        Ok(r) => r,
        Err(e) => panic!("spend execute failed: {e}"),
    };
    assert!(spend.success, "{}", spend.content);
    assert_eq!(
        fixture.mock.lock().downed.len(),
        1,
        "an unreadable cursor must not block a granted token"
    );
}

/// The happy paths of drag / scroll / hold_key never
/// asserted "it really executed" — the mock had recording fields but zero
/// assertions, so a refactor silently dropping calls would stay green.
#[tokio::test]
async fn drags_scrolls_and_holds_execute_on_granted_actions() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // No element covers the target → screening Clear; all three actions
    // execute directly.
    let drag = fixture
        .tool
        .execute(
            json!({"action": "left_click_drag", "start_x": 2, "start_y": 3, "x": 8, "y": 9}),
            &context(&fixture.workspace),
        )
        .await;
    let drag = match drag {
        Ok(r) => r,
        Err(e) => panic!("drag execute failed: {e}"),
    };
    assert!(drag.success, "{}", drag.content);
    assert_eq!(
        fixture.mock.lock().drags.len(),
        1,
        "drag must actually reach the backend injection"
    );

    let scroll = fixture
        .tool
        .execute(
            json!({"action": "scroll", "direction": "down", "amount": 3}),
            &context(&fixture.workspace),
        )
        .await;
    let scroll = match scroll {
        Ok(r) => r,
        Err(e) => panic!("scroll execute failed: {e}"),
    };
    assert!(scroll.success, "{}", scroll.content);
    assert_eq!(
        fixture.mock.lock().scrolled.len(),
        1,
        "scroll must actually reach the backend injection"
    );

    let held = fixture
        .tool
        .execute(
            json!({"action": "hold_key", "text": "shift+p", "ms": 10}),
            &context(&fixture.workspace),
        )
        .await;
    let held = match held {
        Ok(r) => r,
        Err(e) => panic!("hold_key execute failed: {e}"),
    };
    assert!(held.success, "{}", held.content);
    assert_eq!(
        fixture.mock.lock().held.len(),
        1,
        "hold_key must actually reach the backend injection"
    );
}

/// The "typed text never reaches
/// the audit" contract of type's success path previously had only
/// construction-side unit tests; no test actually executed a successful type
/// and checked the real JSONL file — a refactor writing text into the target
/// would still be green. Pinned: the file carries only the count
/// ("typed 5 characters"); the original bytes never appear.
#[tokio::test]
async fn type_success_audit_record_carries_counts_never_text() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    fixture.mock.lock().focused = None;
    let result = fixture
        .tool
        .execute(
            json!({"action": "type", "text": "hello"}),
            &context(&fixture.workspace),
        )
        .await;
    let result = match result {
        Ok(r) => r,
        Err(e) => panic!("execute failed: {e}"),
    };
    assert!(result.success, "{}", result.content);

    let raw = std::fs::read_to_string(session_audit_path()).expect("audit jsonl exists");
    assert!(
        !raw.contains("hello"),
        "typed text must never reach the audit log: {raw}"
    );
    assert!(
        raw.contains("typed 5 characters"),
        "the audit target must carry the redacted count: {raw}"
    );
}

/// stop_all's consent-state wipe
/// previously had only grant assertions — the clearing of pendings and
/// minted tokens had no direct pin. After an emergency stop: pendings are
/// gone and unspent tokens are uniformly Unknown.
#[test]
fn stop_all_wipes_pending_confirmations_and_approved_tokens() {
    let shared = ComputerUseShared::new();
    shared.set_enabled(true);
    shared.grant_session("s1");
    // Two sessions: a second pending for the same session would replace the
    // first per "newest wins"; only across sessions can one pending and one
    // minted token coexist.
    shared.grant_session("s2");
    let pending_id = shared.new_pending_confirmation("s1", "left click", "Buy now", 0);
    let token_id = shared.new_pending_confirmation("s2", "type 3 characters", "secret-field", 0);
    assert!(shared.pending_confirmation(&pending_id).is_some());
    assert!(shared.mint_confirmation(&token_id));

    shared.stop_all();

    assert!(
        shared.pending_confirmation(&pending_id).is_none(),
        "stop must clear pending confirmations"
    );
    assert_eq!(
        shared.take_confirmation(&token_id, "s2", "type 3 characters", 0),
        ConfirmationCheck::Unknown,
        "stop must wipe minted approval tokens"
    );
}

/// key/hold_key's text length cap: a legal chord is constrained
/// to ≤4 key names after parsing, but the raw text rode verbatim into the
/// confirm event/result payload, and whitespace alone could smuggle in a
/// string of any size. >128 characters is
/// rejected; the error text does not echo it.
#[tokio::test]
async fn key_chord_text_length_is_capped() {
    let (fixture, _restore) = fixture();
    // 129 'a's (a shape with separators would exceed the cap too — here the
    // oversized raw text is constructed directly).
    let oversized = "a".repeat(MAX_KEY_CHORD_TEXT_CHARS + 1);
    let result = fixture
        .tool
        .execute(
            json!({"action": "key", "text": oversized}),
            &context(&fixture.workspace),
        )
        .await;
    let error = match result {
        Ok(r) => panic!("an oversized chord text must not parse: {r:?}"),
        Err(e) => e.to_string(),
    };
    assert!(
        error.contains("too long for key"),
        "expected the length-cap error: {error}"
    );
    // hold_key follows the same rule.
    let result = fixture
        .tool
        .execute(
            json!({"action": "hold_key", "text": "a".repeat(MAX_KEY_CHORD_TEXT_CHARS + 1), "ms": 10}),
            &context(&fixture.workspace),
        )
        .await;
    assert!(result.is_err(), "an oversized hold_key text must not parse");
}

/// The bounds returned by
/// element_at_point must be in **screenshot pixel space** — a11y element
/// rectangles are input/screen coordinates; when the screenshot's long edge
/// is downsampled, the two coordinate sets drift apart by the scale factor,
/// and a click using bounds as screenshot coordinates would silently land
/// offset. In the Retina scenario (shot 200x200, input = shot × 0.5): the
/// input rect (50,30,20x20) must be reported as the screenshot rect
/// (100,60,40x40).
#[tokio::test]
async fn element_at_point_reports_bounds_in_screenshot_space() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    {
        let mut mock = fixture.mock.lock();
        mock.capture_size = (200, 200);
        mock.input_scale = (0.5, 0.5);
        mock.element = Some(ElementInfo {
            role: "AXButton".to_string(),
            name: "Buy now".to_string(),
            x: 50,
            y: 30,
            width: 20,
            height: 20,
            secure: false,
        });
    }
    // Establish the scale map.
    let _ = fixture
        .tool
        .execute(
            json!({"action": "screenshot"}),
            &context(&fixture.workspace),
        )
        .await;

    // The query point (120, 80) in screenshot pixel space → input (60, 40)
    // → hits the element.
    let result = fixture
        .tool
        .execute(
            json!({"action": "element_at_point", "x": 120, "y": 80}),
            &context(&fixture.workspace),
        )
        .await;
    let result = match result {
        Ok(r) => r,
        Err(e) => panic!("execute failed: {e}"),
    };
    assert!(result.success, "{}", result.content);
    assert!(
        result
            .content
            .contains("bounds=(100, 60, 40x40) in screenshot space"),
        "bounds must be converted to screenshot space: {}",
        result.content
    );
    assert!(
        !result.content.contains("(50, 30, 20x20)"),
        "raw input-space bounds must not leak into the output: {}",
        result.content
    );
}

// ---------------------------------------------------------------------------
// Screenshot failure, execution-error redaction, and preview-cap pins
// ---------------------------------------------------------------------------

/// The screenshot action's capture IS the action — a
/// capture failure must propagate as a failed result (success=false), not
/// degrade to success+warning with an ok audit record (the old behavior made
/// the "capture unavailable" platform state invisible to both the model and
/// the audit).
#[tokio::test]
async fn screenshot_failure_fails_the_action_instead_of_warning() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    fixture.mock.lock().capture_error = true;
    let result = fixture
        .tool
        .execute(
            json!({"action": "screenshot"}),
            &context(&fixture.workspace),
        )
        .await;
    let result = match result {
        Ok(r) => r,
        Err(e) => panic!("execute failed: {e}"),
    };
    assert!(
        !result.success,
        "a failed capture must fail the screenshot action: {}",
        result.content
    );
    assert!(
        result.content.contains("capture denied"),
        "the capture error must reach the model: {}",
        result.content
    );
    let raw = std::fs::read_to_string(session_audit_path()).expect("audit jsonl exists");
    assert!(
        raw.contains("\"result\":\"error\""),
        "the audit record must carry the failure, not ok: {raw}"
    );
    assert!(
        !raw.contains("\"result\":\"ok\""),
        "no ok record may exist for the failed screenshot: {raw}"
    );
}

/// Audit redaction on the execution-error path was
/// previously verified only on the construction side — no test actually
/// executed a failing type and checked the file bytes. Pinned: the error
/// message goes into the audit (the backend error constructors guarantee it
/// carries no input content), but the typed text never appears.
#[tokio::test]
async fn type_execution_error_audits_the_error_never_the_text() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    fixture.mock.lock().type_error = Some("mock: type injection failed".to_string());
    let result = fixture
        .tool
        .execute(
            json!({"action": "type", "text": "hunter2"}),
            &context(&fixture.workspace),
        )
        .await;
    let result = match result {
        Ok(r) => r,
        Err(e) => panic!("execute failed: {e}"),
    };
    assert!(!result.success, "{}", result.content);

    let raw = std::fs::read_to_string(session_audit_path()).expect("audit jsonl exists");
    assert!(
        !raw.contains("hunter2"),
        "typed text must never reach the audit log, even on execution errors: {raw}"
    );
    assert!(
        raw.contains("mock: type injection failed"),
        "the execution error itself is audited: {raw}"
    );
}

/// type_preview_full's 4096 truncation: a non-secure
/// type of 4097 characters must not carry the full-text
/// preview (the dialog falls back to the character-count summary).
#[tokio::test]
async fn confirm_event_drops_full_preview_above_4096_chars() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    fixture.mock.lock().focused = Some(ElementInfo {
        role: "AXButton".to_string(),
        name: "Send message".to_string(),
        x: 0,
        y: 0,
        width: 10,
        height: 10,
        secure: false,
    });
    let text = "x".repeat(4097);
    let result = fixture
        .tool
        .execute(
            json!({"action": "type", "text": text}),
            &context(&fixture.workspace),
        )
        .await;
    assert!(
        result
            .ok()
            .map(|r| r.content)
            .unwrap_or_default()
            .contains("NOT executed")
    );
    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    let payload = events
        .iter()
        .rev()
        .find(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
        .map(|(_, p)| p.clone())
        .expect("confirm event");
    assert!(
        payload["type_preview_full"].is_null(),
        "preview must be dropped above the 4096 cap: {}",
        payload["type_preview_full"]
    );
}

/// Character-key chords are typing too — on a non-secure
/// target the `key "h+a+c+k"` confirm event must carry the character-sequence
/// preview (the same transparency as type, the same 4096 cap), otherwise the
/// user is blind-signing chunk by chunk. Secure targets still get a count
/// only.
#[tokio::test]
async fn char_carrying_chord_confirm_carries_preview_on_non_secure_target() {
    let (fixture_plain, _restore_plain) = fixture();
    fixture_plain.shared.grant_session("s-test");
    fixture_plain.mock.lock().focused = Some(ElementInfo {
        role: "AXButton".to_string(),
        name: "Send message".to_string(),
        x: 0,
        y: 0,
        width: 10,
        height: 10,
        secure: false,
    });
    let result = fixture_plain
        .tool
        .execute(
            json!({"action": "key", "text": "h+a+c+k"}),
            &context(&fixture_plain.workspace),
        )
        .await;
    assert!(
        result
            .ok()
            .map(|r| r.content)
            .unwrap_or_default()
            .contains("NOT executed")
    );
    let events = fixture_plain
        .events
        .lock()
        .map(|e| e.clone())
        .unwrap_or_default();
    let payload = events
        .iter()
        .rev()
        .find(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
        .map(|(_, p)| p.clone())
        .expect("confirm event");
    assert_eq!(
        payload["type_preview_full"].as_str(),
        Some("hack"),
        "the typed characters must ride the confirm event: {payload}"
    );
}

/// The secure-target side of the same contract: chord typing on a password
/// field gets a count only; the preview never appears (the masking contract
/// is unchanged). A separate test: calling `fixture()` twice inside the same
/// fn would self-deadlock on ENV_LOCK (std Mutex is not reentrant; the first
/// phase's guard is still alive).
#[tokio::test]
async fn char_carrying_chord_confirm_stays_masked_on_secure_target() {
    let (fixture_secure, _restore_secure) = fixture();
    fixture_secure.shared.grant_session("s-test");
    fixture_secure.mock.lock().focused = Some(ElementInfo {
        role: "AXTextField".to_string(),
        name: "Password".to_string(),
        x: 0,
        y: 0,
        width: 10,
        height: 10,
        secure: true,
    });
    let result = fixture_secure
        .tool
        .execute(
            json!({"action": "key", "text": "h+a+c+k"}),
            &context(&fixture_secure.workspace),
        )
        .await;
    assert!(
        result
            .ok()
            .map(|r| r.content)
            .unwrap_or_default()
            .contains("NOT executed")
    );
    let events = fixture_secure
        .events
        .lock()
        .map(|e| e.clone())
        .unwrap_or_default();
    let payload = events
        .iter()
        .rev()
        .find(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
        .map(|(_, p)| p.clone())
        .expect("confirm event");
    assert!(
        payload["type_preview_full"].is_null(),
        "secure targets never carry the preview: {payload}"
    );
}

/// The cross-session physical input lock: while session A holds the lock,
/// session B's input action is rejected with InputBusy and never injects.
/// The rejection path itself waits out the bounded lock-acquisition timeout,
/// so the test temporarily shrinks it to ~50ms; the production value is
/// restored on drop (panic-safe, so the short timeout cannot leak into other
/// tests).
struct RestoreLockTimeout;
impl Drop for RestoreLockTimeout {
    fn drop(&mut self) {
        set_physical_input_lock_timeout_for_tests(PHYSICAL_INPUT_LOCK_TIMEOUT);
    }
}

#[tokio::test]
async fn held_input_lock_rejects_other_sessions_to_inject() {
    let (fixture, _restore) = fixture();
    set_physical_input_lock_timeout_for_tests(std::time::Duration::from_millis(50));
    let _timeout_reset = RestoreLockTimeout;
    fixture.shared.grant_session("s-test");
    let mock2 = Arc::new(Mutex::new(MockState::default()));
    let events2 = Arc::new(StdMutex::new(Vec::new()));
    let mock2_for_factory = Arc::clone(&mock2);
    let backend2 = BackendHandle::lazy(move || {
        Ok(Box::new(MockBackend {
            state: mock2_for_factory,
        }) as Box<dyn ComputerUseBackend>)
    });
    let tool2 = ComputerUseTool::with_parts(
        "s-other".to_string(),
        Arc::clone(&fixture.shared),
        backend2,
        Arc::new(RecordingSink(Arc::clone(&events2))),
    );
    fixture.shared.grant_session("s-other");

    // Session 1 holds the lock (simulating an in-flight injection action).
    let guard = fixture
        .shared
        .lock_physical_input()
        .expect("lock is free at test start");
    let result = tool2
        .execute(
            json!({"action": "left_click", "x": 1, "y": 2}),
            &context(&fixture.workspace),
        )
        .await;
    drop(guard);
    let result = match result {
        Ok(r) => r,
        Err(e) => panic!("execute failed: {e}"),
    };
    assert!(
        !result.success,
        "the blocked session must not report success: {}",
        result.content
    );
    assert!(
        result
            .content
            .contains("another session is performing a physical input action"),
        "the rejection must be the explicit InputBusy message: {}",
        result.content
    );
    assert!(
        mock2.lock().clicked.is_empty(),
        "the blocked session must never reach the injection surface"
    );
}

/// When the same-session factory re-enters, a late stale
/// tool's Drop previously revoked the session's grant unconditionally — the
/// registry-side identity check is extended to the consent revocation. When
/// the new tool has registered and then the old tool Drops, the new tool's
/// grant and dialogs must be kept intact.
#[test]
fn stale_tool_drop_keeps_a_same_session_successors_grant() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");

    // The successor tool: registered for the same session (with_parts'
    // internal insert replaces the old entry).
    let mock2 = Arc::new(Mutex::new(MockState::default()));
    let events2 = Arc::new(StdMutex::new(Vec::new()));
    let mock2_for_factory = Arc::clone(&mock2);
    let backend2 = BackendHandle::lazy(move || {
        Ok(Box::new(MockBackend {
            state: mock2_for_factory,
        }) as Box<dyn ComputerUseBackend>)
    });
    let tool2 = ComputerUseTool::with_parts(
        "s-test".to_string(),
        Arc::clone(&fixture.shared),
        backend2,
        Arc::new(RecordingSink(Arc::clone(&events2))),
    );

    // The old tool now Drops: it must not revoke s-test's grant (the
    // successor is already in the registry).
    drop(fixture.tool);

    assert!(
        fixture.shared.has_active_grant("s-test"),
        "the successor's grant must survive the stale tool's Drop"
    );
    // The successor tool can still actually inject (the grant is available
    // and the backend registration is intact).
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let result = rt
        .block_on(tool2.execute(
            json!({"action": "left_click", "x": 3, "y": 4}),
            &context(&fixture.workspace),
        ))
        .expect("execute on the successor tool");
    assert!(result.success, "{}", result.content);
    assert_eq!(
        mock2.lock().clicked.len(),
        1,
        "the successor must still reach the injection surface"
    );
}

// ---------------------------------------------------------------------------
// Execution-surface pins: click-variant button/count mapping, capability bits,
// no-screenshot branches, wait bounds, ui_tree pass-through, last-moment
// consent re-check
// ---------------------------------------------------------------------------

/// Execution-level pin for the click-variant mapping: right_click injects
/// (Right, 1), double_click (Left, 2), triple_click (Left, 3) — the mock
/// records the injected button/count, so a mapping regression cannot stay
/// green behind a generic "clicked" assertion.
#[tokio::test]
async fn click_variants_map_to_the_right_button_and_count() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // No element covers the targets (screening Clear); the auto-captured
    // 16x16 screenshot maps (5, 6) identically to input coordinates.
    let cases: &[(&str, MouseButton, u8)] = &[
        ("right_click", MouseButton::Right, 1),
        ("double_click", MouseButton::Left, 2),
        ("triple_click", MouseButton::Left, 3),
    ];
    for (action, button, count) in cases {
        let result = fixture
            .tool
            .execute(
                json!({"action": action, "x": 5, "y": 6}),
                &context(&fixture.workspace),
            )
            .await;
        let result = match result {
            Ok(r) => r,
            Err(e) => panic!("execute failed for {action}: {e}"),
        };
        assert!(result.success, "{action}: {}", result.content);
        let mock = fixture.mock.lock();
        assert_eq!(
            mock.moved_to.last().copied(),
            Some((5, 6)),
            "{action} must move to the target first"
        );
        assert_eq!(
            mock.clicked.last().copied(),
            Some((*button, *count)),
            "{action} must inject the mapped button/count"
        );
    }
}

/// Capabilities first for the a11y-observation actions: without the ui_tree
/// capability bit, ui_tree and element_at_point are rejected with the
/// explicit "accessibility tree is unsupported" error before any a11y query.
#[tokio::test]
async fn unsupported_ui_tree_capability_rejects_ui_tree_and_element_at_point() {
    let (fixture, _restore) = fixture();
    fixture.mock.lock().ui_tree_cap = false;
    for input in [
        json!({"action": "ui_tree"}),
        json!({"action": "element_at_point", "x": 1, "y": 2}),
    ] {
        let result = fixture
            .tool
            .execute(input, &context(&fixture.workspace))
            .await;
        let text = result.ok().map(|r| r.content).unwrap_or_default();
        assert!(
            text.contains("the accessibility tree is unsupported"),
            "{text}"
        );
    }
}

/// Without the screenshot capability bit, the screenshot action is rejected
/// with the explicit "screen capture is unsupported" error instead of running
/// all the way to a degraded success with no image.
#[tokio::test]
async fn unsupported_screenshot_capability_rejects_screenshot() {
    let (fixture, _restore) = fixture();
    fixture.mock.lock().screenshot_cap = false;
    let result = fixture
        .tool
        .execute(
            json!({"action": "screenshot"}),
            &context(&fixture.workspace),
        )
        .await;
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("screen capture is unsupported"), "{text}");
}

/// cursor_position before any screenshot: no ScaleMap exists, so the raw
/// input coordinates are reported with the explicit "no screenshot has been
/// taken this session" explanation.
#[tokio::test]
async fn cursor_position_without_screenshot_reports_input_coordinates() {
    let (fixture, _restore) = fixture();
    let result = fixture
        .tool
        .execute(
            json!({"action": "cursor_position"}),
            &context(&fixture.workspace),
        )
        .await;
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        text.contains("cursor is at (7, 9) in global input coordinates"),
        "{text}"
    );
    assert!(
        text.contains("no screenshot has been taken this session"),
        "{text}"
    );
}

/// The coordinate-carrying actions' "no screenshot yet" execution branches:
/// when run() has no ScaleMap for them (the defensive `None` arm of
/// execute_action), each action fails with its own "take one before …"
/// message instead of injecting against a missing coordinate space.
#[test]
fn coordinate_actions_without_a_map_fail_with_take_one_first() {
    let shared = Arc::new(ComputerUseShared::new());
    let mock = Arc::new(Mutex::new(MockState::default()));
    let backend = BackendHandle::lazy({
        let mock = Arc::clone(&mock);
        move || Ok(Box::new(MockBackend { state: mock }) as Box<dyn ComputerUseBackend>)
    });
    let parts = Parts {
        session_id: "s-test".to_string(),
        shared,
        backend,
        events: Arc::new(RecordingSink(Arc::new(StdMutex::new(Vec::new())))),
        state: Arc::new(Mutex::new(ToolState::default())),
    };
    let cases: &[(&str, Value, &str)] = &[
        (
            "element_at_point",
            json!({"action": "element_at_point", "x": 1, "y": 2}),
            "take one before element_at_point",
        ),
        (
            "mouse_move",
            json!({"action": "mouse_move", "x": 1, "y": 2}),
            "take one before mouse_move",
        ),
        (
            "scroll with coordinates",
            json!({"action": "scroll", "direction": "down", "amount": 1, "x": 1, "y": 2}),
            "take one before scroll with coordinates",
        ),
        (
            "click with coordinates",
            json!({"action": "left_click", "x": 1, "y": 2}),
            "take one before clicking with coordinates",
        ),
        (
            "left_click_drag",
            json!({"action": "left_click_drag", "start_x": 1, "start_y": 2, "x": 3, "y": 4}),
            "take one before dragging",
        ),
    ];
    for (name, input, expected) in cases {
        let parsed = parse_action(input).expect("parses");
        let mut warnings = Vec::new();
        let error = execute_action(&parts, &parsed.action, None, &mut warnings)
            .expect_err("a missing scale map must fail the action");
        assert!(
            error.to_string().contains(expected),
            "{name}: expected `{expected}` in {error}"
        );
    }
    // Nothing may have been injected on any of the failed branches.
    let mock = mock.lock();
    assert!(mock.moved_to.is_empty(), "no move may be injected");
    assert!(mock.clicked.is_empty(), "no click may be injected");
    assert!(mock.scrolled.is_empty(), "no scroll may be injected");
    assert!(mock.drags.is_empty(), "no drag may be injected");
}

/// wait executes through the tool (an Observe-class action: no grant needed),
/// sleeps the requested duration and attaches a fresh screenshot.
#[tokio::test]
async fn wait_executes_and_attaches_a_screenshot() {
    let (fixture, _restore) = fixture();
    let result = fixture
        .tool
        .execute(
            json!({"action": "wait", "ms": 50}),
            &context(&fixture.workspace),
        )
        .await;
    let result = match result {
        Ok(r) => r,
        Err(e) => panic!("execute failed: {e}"),
    };
    assert!(result.success, "{}", result.content);
    assert!(
        result.content.contains("waited 50 ms"),
        "{}",
        result.content
    );
    assert!(
        result
            .metadata
            .as_ref()
            .and_then(|m| m.get("images"))
            .is_some(),
        "wait attaches a follow-up screenshot: {:?}",
        result.metadata
    );
}

/// The ms upper bound is inclusive: exactly MAX_WAIT_MS parses, one
/// millisecond over is rejected with the explicit bound and the offending
/// value in the message.
#[test]
fn wait_ms_boundary_is_inclusive_at_the_cap() {
    let err = parse_action(&json!({"action": "wait", "ms": MAX_WAIT_MS + 1}))
        .expect_err("above the cap must reject")
        .to_string();
    assert!(err.contains("ms must be <= 30000"), "{err}");
    assert!(err.contains("30001"), "{err}");
    assert!(
        parse_action(&json!({"action": "wait", "ms": MAX_WAIT_MS})).is_ok(),
        "exactly the cap is accepted"
    );
}

/// ui_tree's backend output passes through to the model verbatim (the mock
/// tree text), plus the coordinate-space disclaimer that keeps the model from
/// reading a11y rectangles as screenshot-space coordinates.
#[tokio::test]
async fn ui_tree_output_passes_through_to_the_model() {
    let (fixture, _restore) = fixture();
    let result = fixture
        .tool
        .execute(
            json!({"action": "ui_tree", "max_depth": 3, "max_nodes": 100}),
            &context(&fixture.workspace),
        )
        .await;
    let result = match result {
        Ok(r) => r,
        Err(e) => panic!("execute failed: {e}"),
    };
    assert!(result.success, "{}", result.content);
    assert!(
        result.content.contains("window \"Mock\"\n  button \"OK\""),
        "the backend tree must reach the model verbatim: {}",
        result.content
    );
    assert!(
        result
            .content
            .contains("bounds above are global input/screen coordinates"),
        "{}",
        result.content
    );
}

/// The last-moment re-check before injection: a revoke landing after the
/// consent gate (here inside the screening query, i.e. after the automatic
/// screenshot) must abort the action with the grant error instead of
/// injecting.
#[tokio::test]
async fn revoke_during_screening_aborts_before_injection() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    {
        let mut mock = fixture.mock.lock();
        mock.revoke_grant_via = Some(Arc::clone(&fixture.shared));
    }
    // A coordinate click with no prior screenshot: run() auto-captures, the
    // consent gate passes, then the screening query revokes the grant — the
    // re-check before the capability check/injection must catch it.
    let result = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 5, "y": 6}),
            &context(&fixture.workspace),
        )
        .await;
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        text.contains("has not granted control"),
        "the revoke must surface as the grant error: {text}"
    );
    assert!(
        fixture.mock.lock().clicked.is_empty(),
        "the revoked action must never reach the injection surface"
    );
}

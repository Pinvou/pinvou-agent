use super::*;
use crate::features::computer_use::backend::ComputerUseBackend;
use crate::features::computer_use::types::{Capabilities, Capture, ElementInfo, Key};
use std::sync::Mutex as StdMutex;

// ---------------------------------------------------------------------------
// 测试替身
// ---------------------------------------------------------------------------

struct MockState {
    /// 命中测试按元素 bounds 判定（真实 a11y 语义）：element_at_point 只在
    /// 查询点落入元素矩形内时返回它。
    element: Option<ElementInfo>,
    /// Secondary element list (tried in order when the primary misses):
    /// drives multi-element scenarios such as "start point has no on-screen
    /// element while the drop point hits one" (the mock's primary is a
    /// single element).
    background: Vec<ElementInfo>,
    /// 置位时 element_at_point 返回 Err（a11y 故障注入）。
    element_error: bool,
    /// 焦点元素（focused_element 的返回值；None = 明确无焦点）。
    focused: Option<ElementInfo>,
    /// 置位时 focused_element 返回 Err（焦点查询失败/平台不支持的故障注入）。
    focused_error: bool,
    /// 截图捕获的显示器原点（输入坐标空间；默认 (0,0) 即恒等映射）。
    capture_origin: (i32, i32),
    /// 截图捕获的尺寸（设备物理像素；默认 16x16）。
    capture_size: (u32, u32),
    /// 捕获的 device→input 倍率（默认 (1.0, 1.0)；Retina 场景设 0.5）。
    input_scale: (f64, f64),
    /// cursor_position 的返回值（**输入坐标空间**——macOS 为点、Windows/X11
    /// 为物理像素；可配置以驱动筛查定位与光标移动场景）。默认 (7,9)（历史
    /// 行为）。
    cursor: (i32, i32),
    /// 置位时 cursor_position 返回 Err（光标未知，如 Wayland 首次 move 前）。
    cursor_error: bool,
    /// 置位时 input 能力位为 false（默认具备输入能力）。
    no_input_cap: bool,
    moved_to: Vec<(i32, i32)>,
    clicked: Vec<(MouseButton, u8)>,
    /// 评审修复（第三轮）：五个注入面全部记录——down/up/drag/scroll/hold_key
    /// 的「NOT executed」断言此前验不了执行面（回归钉子是软的）。
    downed: Vec<MouseButton>,
    upped: Vec<MouseButton>,
    drags: Vec<((i32, i32), (i32, i32))>,
    scrolled: Vec<(ScrollDirection, u32)>,
    held: Vec<(Vec<Key>, u64)>,
    typed: Vec<String>,
    chords: Vec<Vec<Key>>,
    /// release_os_grant 被调用次数（评审修复回归：revoke/stop 必须触发
    /// 后端关闭持久 OS 级授权）。
    released: u64,
    /// 置位时 drag 返回 Err（拖拽后端故障注入，验证失败后的按钮释放兜底）。
    drag_error: bool,
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

impl ComputerUseBackend for MockBackend {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            screenshot: true,
            input: !self.state.lock().no_input_cap,
            ui_tree: true,
            notes: "mock".to_string(),
        }
    }

    fn capture(&mut self) -> Result<Capture, ComputerUseError> {
        let state = self.state.lock();
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
        self.state.lock().typed.push(text.to_string());
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
    /// PINVOU3_HOME 指向的临时根（审计 JSONL 落在 `<home>/computer-use/`）。
    home: PathBuf,
    _env_guard: std::sync::MutexGuard<'static, ()>,
}

struct EnvRestore(Option<std::ffi::OsString>);

impl Drop for EnvRestore {
    fn drop(&mut self) {
        match self.0.take() {
            // SAFETY: 测试持有 platform::paths::tests::ENV_LOCK，进程内 env 写串行。
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: 同上。
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
    }
}

fn fixture() -> (TestFixture, EnvRestore) {
    let env_lock = crate::platform::paths::tests::ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let previous = std::env::var_os("PINVOU3_HOME");
    let home = std::env::temp_dir().join(format!(
        "pinvou3-cu-home-{}-{}",
        std::process::id(),
        crate::platform::paths::tests::unique_suffix()
    ));
    // SAFETY: 持有 platform::paths::tests::ENV_LOCK，进程内 env 写串行。
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
            _env_guard: env_lock,
        },
        EnvRestore(previous),
    )
}

fn context(workspace: &Path) -> ToolContext {
    ToolContext::new(workspace)
}

/// 名单之外的无害元素（T3 筛查 Clear）：place 元素盖住筛查点，让放行路径
/// 走到执行。Unnamed targets screen Clear (Ok(None) is not a red flag);
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
// 参数校验
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
        // 评审修复回归：scroll amount 下限 1，0 格显式拒绝。
        (
            "scroll",
            json!({"action": "scroll", "direction": "down", "amount": 0}),
        ),
        ("wait", json!({"action": "wait"})),
        (
            "left_click_drag",
            json!({"action": "left_click_drag", "x": 1, "y": 2}),
        ),
        ("key", json!({"action": "key", "text": "ctrl+shift"})),
        ("key", json!({"action": "key", "text": "ctrl+nosuchkey"})),
        // 评审修复回归：和弦 token 上限与控制字符拒绝。
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

/// 评审修复回归：type 文本上限 10_000 字符、拒绝 NUL。
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

// hold_key 的 "shift" 单独是修饰键——chord 必须含非修饰键。
#[test]
fn hold_key_rejects_modifier_only_chord() {
    assert!(parse_action(&json!({"action": "hold_key", "text": "shift", "ms": 100})).is_err());
}

/// 评审修复回归（M1）：`key` 的解析失败绝不把模型文本回显进审计日志——
/// 模型可以用 text 字段携带敏感串探测（`{"action":"key","text":"hunter2"}`
/// 曾把该串经错误信息写进 JSONL 的 error 字段）。错误串只描述和弦形状，
/// 模型本来就知道自己的输入，不损失任何信息。
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

    let audit_path = fixture.home.join("computer-use").join("audit-s-test.jsonl");
    let raw = std::fs::read_to_string(&audit_path).expect("audit jsonl exists");
    assert!(
        !raw.contains("hunter2") && !raw.contains("h+u+n+t+e+r"),
        "the secret substring must appear nowhere in the audit log: {raw}"
    );
    let records: Vec<serde_json::Value> = raw
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).expect("valid jsonl line"))
        .collect();
    assert_eq!(records.len(), 2, "one record per call: {records:?}");
    for record in &records {
        assert_eq!(record["action"], "unparseable", "{records:?}");
        assert_eq!(record["result"], "rejected", "{records:?}");
        assert!(
            record["error"]
                .as_str()
                .unwrap_or_default()
                .starts_with("parse failed: Failed to validate input: key chord"),
            "shape-only parse error expected: {records:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 同意门控
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
    // 未授权时绝不能触碰后端。
    assert!(fixture.mock.lock().clicked.is_empty());
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
    // 点击前移到了映射后的输入坐标（16x16 无缩放，input_scale=1 → 恒等）。
    assert_eq!(fixture.mock.lock().moved_to.last().copied(), Some((5, 6)));
    assert_eq!(
        fixture.mock.lock().clicked.last().copied(),
        Some((MouseButton::Left, 1))
    );
    // metadata.images 带绝对路径；文本带 attachments 相对路径回退指引。
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
    // 16x16 截图 → 钳到 (15, 15)。
    assert_eq!(fixture.mock.lock().moved_to.last().copied(), Some((15, 15)));
}

#[tokio::test]
async fn first_coordinate_action_auto_captures() {
    let (fixture, _restore) = fixture();
    // scroll 是 Input 类动作（评审修正）：需要会话授权。
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
// T3 后果性动作
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

    // 伪造/重复使用 confirm_id 一律拒绝。
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

    // 用户确认（未来的 computer_use_confirm 命令铸造令牌）后重试成功。
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
    // 键盘筛查查**焦点元素**：焦点在密码框即拦截，与光标位置无关。
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
// 观察类
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
// 评审修复回归:层级筛查 / 筛查不可用放行 / 令牌绑定 / 能力先行
// ---------------------------------------------------------------------------

/// P0 回归:mouse_down/mouse_up 曾不在 T3 清单里,可拆解出零确认点击。
#[tokio::test]
async fn mouse_down_up_composition_is_t3_screened() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // 光标 (7,9) 处是 denylist 控件。
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
    // 第三轮评审修复回归：注入面执行断言——拦截时 down/up 不得真的下发
    // （mock 现在记录全部五个注入面）。
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

/// 筛查不可用不阻断执行（主流口径：筛查是尽力而为的类别检测，没有产品
/// 为筛查基础设施故障单独索要确认）：a11y 查询故障的目标照常执行、不发
/// 确认事件；正面命中名单仍会拦截（见其余 T3 测试）。
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

/// 评审修复回归（最重）：键盘输入落在**焦点元素**而非光标处——焦点在密码
/// 框、光标在空白处时，type 旧实现按光标筛查（查不到元素）直接放行注入。
#[tokio::test]
async fn focus_on_password_with_cursor_elsewhere_requires_confirmation() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // 焦点在密码框；光标 (7,9) 处无任何元素（element 只用于证明光标处空白）。
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

    // key 和弦同样按焦点筛查：焦点在后果性控件上时要求确认。
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

/// 只有**正面**命中密码/安全角色才确认：焦点读不出（查询故障）不构成
/// 信号，type 照常执行、不发确认事件；焦点正面命中密码角色时仍拦截
/// （同一测试两段对照，pin (c)）。
#[tokio::test]
async fn unreadable_type_focus_executes_and_password_focus_confirms() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // 焦点查询失败：不阻断执行、不发确认事件。
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

    // 对照：焦点正面命中密码角色 → 拦截确认。
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

/// 明确无焦点元素 = 无处键入，type 放行（Ok(None) 与 Err 一样不构成确认
/// 信号；只有正面命中密码/名单角色才拦截，见其余 T3 测试）。
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

/// drag 的起点与落点都必须被筛查（pin (f)）：终点命中 denylist（起点良性）
/// 与起点命中 denylist（终点良性）两个方向都要求确认。
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
    // 第三轮评审修复回归：拦截时拖拽不得真的下发。
    assert!(
        fixture.mock.lock().drags.is_empty(),
        "drag must not execute"
    );

    // 反向：起点命中 denylist（终点良性）同样拦截。
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

/// 令牌绑定动作:为 A 动作铸造的 confirm_id 不能给 B 动作用(工具层集成)。
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
    // 拿「点击」的令牌去重放「type」——必须被拒。
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

/// 评审修正:mouse_move/scroll 是真实指针输入,必须持会话授权。
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

/// 能力先行:无输入能力的平台在授权门控之前就被拒绝,不弹授权、不耗预算。
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
}

/// schema enum、未知动作错误文案与 parse_action 分发三者一致。
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

/// 每个动作的最小合法参数（parity 测试用）。
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
// 确认摘要：纯参数摘要（type N characters），全文经 type_preview_full 下发
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
    let summary = payload["action"].as_str().unwrap_or_default();

    // Exactly the plain character count — no `[fp …]` suffix, nothing else.
    assert_eq!(summary, "type 20 characters", "{summary}");
    // The raw text never rides the dialog summary.
    assert!(!summary.contains("hello"), "{summary}");
    assert!(!summary.contains('\u{7}'), "{summary:?}");
    // The full text rides type_preview_full for this non-secure target.
    assert_eq!(
        payload["type_preview_full"].as_str(),
        Some(text.as_str()),
        "the expander payload carries the full text: {payload}"
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
    let summary = payload["action"].as_str().unwrap_or_default();

    // 明文不得出现在摘要；摘要只有字符数。
    assert_eq!(summary, "type 14 characters", "{summary}");
    assert!(!summary.contains("hunter2"), "{summary}");
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
// 审计脱敏（单字符和弦只记键数；多键快捷键和弦保留明文）
// ---------------------------------------------------------------------------

/// Single-character chords are audited redacted as `pressed 1 key` — never
/// the character, regardless of the modifier (round-6 review: a password
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

    let audit_path = fixture.home.join("computer-use").join("audit-s-test.jsonl");
    let raw = std::fs::read_to_string(&audit_path).expect("audit jsonl exists");
    let records: Vec<serde_json::Value> = raw
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).expect("valid jsonl line"))
        .collect();
    let keys: Vec<&serde_json::Value> = records.iter().filter(|r| r["action"] == "key").collect();
    assert_eq!(keys.len(), 7, "seven key calls: {records:?}");
    let holds: Vec<&serde_json::Value> = records
        .iter()
        .filter(|r| r["action"] == "hold_key")
        .collect();
    assert_eq!(holds.len(), 1, "one hold_key call: {records:?}");

    let typed: Vec<&&serde_json::Value> = keys
        .iter()
        .filter(|r| r["target"] == "pressed 1 key")
        .collect();
    assert_eq!(
        typed.len(),
        7,
        "every single-character chord (any modifier) is audited count-only: {records:?}"
    );
    // No single-character chord keeps a plaintext target — ctrl+s included.
    assert!(
        keys.iter().all(|r| !r["target"]
            .as_str()
            .unwrap_or_default()
            .starts_with("keys: ")),
        "single-character chords must never log their character: {records:?}"
    );
    // The held typing-form chord carries the held-ms suffix, no character.
    assert_eq!(
        holds[0]["target"], "pressed 1 key held for 10ms",
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

/// 评审修复回归（M2）：多字符字母和弦（≤4 token，如 `p+a+s+s`）是把文本
/// 按 ≤4 字符块拼写——审计只记非修饰键数（`pressed 4 keys`），绝不记明文
/// （旧实现只对单字符和弦脱敏，`p+a+s+s` 以 `keys: p+a+s+s` 明文落盘）。
/// 含命名键的快捷键（`Return`）不是拼写文本，保持可读的 `keys: <chord>`。
#[tokio::test]
async fn multi_char_letter_chords_are_audited_as_counts_only() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    for chord in ["p+a+s+s", "h+u+n"] {
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

    let audit_path = fixture.home.join("computer-use").join("audit-s-test.jsonl");
    let raw = std::fs::read_to_string(&audit_path).expect("audit jsonl exists");
    assert!(
        !raw.contains("p+a+s+s") && !raw.contains("h+u+n"),
        "letter-chunk chords must never reach the log as plaintext: {raw}"
    );
    let records: Vec<serde_json::Value> = raw
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).expect("valid jsonl line"))
        .collect();
    let targets: Vec<&str> = records
        .iter()
        .filter(|r| r["action"] == "key")
        .filter_map(|r| r["target"].as_str())
        .collect();
    assert_eq!(
        targets,
        vec!["pressed 4 keys", "pressed 3 keys", "keys: Return"],
        "letter chunks log counts, named keys stay readable: {records:?}"
    );
}

// ---------------------------------------------------------------------------
// 审计 fail-open（信息性日志）/ 混合 DPI 出界防护
// ---------------------------------------------------------------------------

/// The audit is an informational, fail-open log: when the audit directory
/// cannot be created (PINVOU3_HOME points at a plain file), the append
/// fails, the caller warns via eprintln, and the action still executes —
/// for every action class.
#[tokio::test]
// ENV_LOCK 必须横跨 await 持有：run() 在 spawn_blocking 线程上按进程级 env
// 解析 PINVOU3_HOME，env 与锁都要活到测试结束（与 fixture() 同一约定）。
#[allow(clippy::await_holding_lock)]
async fn audit_unavailable_fails_open_and_actions_still_execute() {
    let _env_lock = crate::platform::paths::tests::ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let previous = std::env::var_os("PINVOU3_HOME");
    let blocker = std::env::temp_dir().join(format!(
        "pinvou3-cu-blocker-{}-{}",
        std::process::id(),
        crate::platform::paths::tests::unique_suffix()
    ));
    std::fs::write(&blocker, b"not a directory").expect("write blocker file");
    // SAFETY: 持有 platform::paths::tests::ENV_LOCK，进程内 env 写串行。
    unsafe { std::env::set_var("PINVOU3_HOME", &blocker) };
    // Restore via the fixture's EnvRestore guard: the guard runs even when
    // an assertion (or the awaits) panic, so the overridden env cannot leak
    // into other tests. Declared after env_lock, so the env is restored
    // while the lock is still held.
    let _env_restore = EnvRestore(previous);

    // workspace 与审计目录解耦：放在独立临时目录。
    let workspace = std::env::temp_dir().join(format!(
        "pinvou3-cu-nows-{}-{}",
        std::process::id(),
        crate::platform::paths::tests::unique_suffix()
    ));
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

    // Input 类：审计不可用只降级为 eprintln，动作照常执行。
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

    // Observe 类：同样照常。
    let shot = tool
        .execute(json!({"action": "screenshot"}), &context(&workspace))
        .await;
    let shot = match shot {
        Ok(r) => r,
        Err(e) => panic!("screenshot failed: {e}"),
    };
    assert!(shot.success, "{}", shot.content);

    let _ = std::fs::remove_dir_all(&workspace);
    let _ = std::fs::remove_file(&blocker);
}

/// 混合 DPI 防护：光标在截图显示器之外时，cursor_position 不做换算，
/// 回报原始输入坐标并附警告文本。
#[tokio::test]
async fn cursor_outside_captured_monitor_reports_input_position_with_warning() {
    let (fixture, _restore) = fixture();
    // 截图显示器在 (-1000,-1000)..(0,0)；光标 (7,9) 在另一块屏上。
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

/// 混合 DPI：光标在截图显示器之外时无目标可查（绝不拿跨屏垃圾坐标筛查）
/// ——筛查不可用不阻断执行，也不索要确认。
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

/// Retina 式混合 DPI 回归（M3）：捕获 200x200 设备像素、input_scale 0.5
/// （即 100x100 点的显示器）。cursor_position 直接回报输入坐标：
/// - 光标在输入 (60,40)（截图内）→ 无坐标 down 在 (60,40) 处筛查，命中
///   名单控件被拦；
/// - 光标在输入 (150,40)（输入矩形 [0,100) 之外）→ 无目标可查，动作照常
///   执行、零确认事件；cursor_position 回报原始输入坐标并附警告；
/// - 光标回到 (60,40) → cursor_position 报告精确的截图坐标 (120, 80)
///   （input_to_shot 的 ×2 逆换算）。
#[tokio::test]
async fn retina_input_space_cursor_screens_inside_and_reports_exact_coords() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    {
        let mut mock = fixture.mock.lock();
        mock.capture_size = (200, 200);
        mock.input_scale = (0.5, 0.5);
    }
    // 建立映射表（shot 200x200，输入坐标 = 截图坐标 × 0.5）。
    let _ = fixture
        .tool
        .execute(
            json!({"action": "screenshot"}),
            &context(&fixture.workspace),
        )
        .await;

    // 光标 (60,40) 处是后果性控件：无坐标 down 必须在输入点 (60,40) 筛查。
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

    // 光标 (150,40) 在捕获显示器的输入矩形之外：不筛查、执行、零确认。
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

    // 光标在矩形之外时 cursor_position 回报输入坐标 + 警告，不给截图坐标。
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

    // 光标回到 (60,40)：精确换算到截图坐标 (120, 80)。
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
// 第三轮评审回归（口径更新后）：和弦不设形态确认 / mouse_move·scroll 不筛查
// ---------------------------------------------------------------------------

/// 和弦永不设确认（pin (d)）：和弦编辑可逆，没有主流产品按和弦形态设门。
/// 无论修饰键组合如何（cmd+delete、ctrl+Enter、shift+delete……），key 都
/// 直接执行；焦点元素的名单/密码筛查仍然生效（见其余 T3 测试）。
#[tokio::test]
async fn key_chords_never_require_confirmation() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // 焦点是普通文本域（名单判定 Clear）——旧口径会按和弦语义拦截。
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
        // Shift 参与过的判定也已移除：shift+delete 直接执行。
        "shift+delete",
        "shift+Backspace",
        // 普通键（文本编辑主路径）照旧直接执行。
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

/// mouse_move 与 scroll 永不设确认（pin (e)）：悬停/滚动无动作后果，主流
/// 一致——即使落点/目标压在名单控件上也照常执行；按住左键期间的 move 亦然
/// （held-move 复筛已移除，drag 的筛查固定在 Drag 动作的起点+终点）。
#[tokio::test]
async fn mouse_move_and_scroll_never_require_confirmation() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // 名单控件 "Pay" 盖住光标 (7,9) 与整个截图：scroll / mouse_move 照常执行。
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

    // 按住左键期间的 move（实质拖拽）同样只走执行——不复筛。down 需要光标点
    // 筛查 Clear：先放良性元素，再换成名单控件。
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
    // up：光标 (7,9) 不在 Trash 矩形内 → Clear → 执行。
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

/// 名单收敛为后果类别（pin (a)）：泛化肯定词（OK/Continue/Run）不设确认、
/// 直接执行；后果类别词（Pay/Delete/Submit/Accept）仍拦截并索要确认。
#[tokio::test]
async fn trimmed_denylist_affirmatives_execute_and_consequences_confirm() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");

    // 泛化肯定词：直接执行。
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

    // 后果类别词：拦截 + 确认事件。
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
// 评审修复回归:撤销/停止必须终止持久 OS 级授权(登记表 → release_os_grant)
// ---------------------------------------------------------------------------

/// revoke 经登记表触发 emergency release（detached 线程，轮询等待）：
/// 先释放物理左键、再关闭后端持久 OS 级授权（worker 通道串行化保证此
/// 顺序）。登记保留（清理幂等；被再次授权的会话必须仍可被后续全局停止
/// 触达）；活着的工具经 Drop 的 `release_and_unregister` 注销。Wayland
/// portal 会话由此随用户"停止控制"终止，而不是活到进程退出。
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
    // 阶段一：物理左键释放先到达后端。
    let mut upped = false;
    for _ in 0..300 {
        if !fixture.mock.lock().upped.is_empty() {
            upped = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(upped, "emergency_mouse_up must reach the backend");
    // 阶段二：OS 级授权关闭在其后到达（通道串行化：released 只能在
    // mouse_up 请求执行完毕后置数）。
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
    assert_eq!(mock.upped, vec![MouseButton::Left]);
}

/// 工具析构经 emergency release：物理左键与 OS 级授权一并清理（模型按下
/// 左键后死掉不得把用户机器留在按住拖拽状态），并注销登记。
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

/// 工具析构注销登记:否则登记表里的句柄把 worker 线程(及其上的 portal
/// 会话)吊到进程退出。
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

/// 评审修复回归（M4）：会话结束 = 引擎回收工具（Drop）。Drop 必须先吊销
/// 本会话的授权并清空其同意工件（待决确认、已铸令牌）——同意状态不得比
/// 持有它的工具活得更久；其他会话的授权与工件不受影响。
#[tokio::test]
async fn dropping_the_tool_revokes_the_grant_and_consent_artifacts() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    fixture.shared.grant_session("s-other");
    let summary = "left click x1 at Some((5, 5))";
    let own_token = fixture
        .shared
        .new_pending_confirmation("s-test", summary, "Buy now");
    assert!(fixture.shared.mint_confirmation(&own_token));
    let other_token = fixture
        .shared
        .new_pending_confirmation("s-other", summary, "Buy now");
    assert!(fixture.shared.mint_confirmation(&other_token));

    drop(fixture.tool);

    // 本会话：授权与同意工件全部清除。
    assert!(!fixture.shared.has_active_grant("s-test"));
    assert_eq!(
        fixture.shared.begin_input_action("s-test"),
        Err(GuardRejection::GrantRequired)
    );
    assert_eq!(
        fixture
            .shared
            .take_confirmation(&own_token, "s-test", summary),
        ConfirmationCheck::Unknown,
        "tool drop must wipe the session's minted approval tokens"
    );
    // 其他会话：授权与工件原样保留。
    assert!(fixture.shared.has_active_grant("s-other"));
    assert_eq!(
        fixture
            .shared
            .take_confirmation(&other_token, "s-other", summary),
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

    let audit_path = fixture.home.join("computer-use").join("audit-s-test.jsonl");
    let raw = std::fs::read_to_string(&audit_path).expect("audit jsonl exists");
    let records: Vec<serde_json::Value> = raw
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("valid jsonl line"))
        .collect();
    assert_eq!(records.len(), 1, "one record per call: {records:?}");
    assert_eq!(records[0]["result"], "error");
    assert_eq!(records[0]["action"], "left_click");
    assert_eq!(records[0]["consent"], "input:session-grant");
    assert_eq!(
        records[0]["error"], T3_CONFIRM_REQUIRED_ERROR,
        "audit error field must be the stable code"
    );
    // The element label must not appear anywhere in the audit record.
    assert!(!raw.contains("Buy now"), "element label leaked to audit");
}

/// 审计集成（m4 round-6）：一次「铸造 pending → 用户批准铸币 → 带确认执行
/// （附补拍截图）」的完整批准流之后，会话 JSONL 的已确认记录满足脱敏契约：
/// consent 含 "t3-confirmed"、target 是 count/coords 形态（只有坐标参数，
/// 无元素标签文本）、截图记录带 sha256 字段。
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
    let records: Vec<serde_json::Value> = raw
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).expect("valid jsonl line"))
        .collect();
    let confirmed: Vec<&serde_json::Value> = records
        .iter()
        .filter(|r| {
            r["consent"]
                .as_str()
                .unwrap_or_default()
                .contains("t3-confirmed")
        })
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
        record["target"], "left click x1 at Some((5, 5))",
        "target must be the parameter summary: {record}"
    );
    assert_eq!(record["result"], "ok", "{record}");
    // The attached screenshot rides as sha256 + path, never pixels.
    let sha = record["screenshot_sha256"].as_str().unwrap_or_default();
    assert_eq!(sha.len(), 64, "sha256 field must be present: {record}");
    assert!(record["screenshot_path"].is_string(), "{record}");
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
    // full view).
    let summary = payload["action"].as_str().unwrap_or_default();
    assert_eq!(
        summary,
        format!("type {} characters", text.chars().count()),
        "summary must be the parameter summary: {payload}"
    );
}

/// Short type texts DO ride `type_preview_full` (round-6 review): the
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
// 回归：光标移动/不可读不阻碍已批令牌 / 摘要粒度的令牌绑定 / 注入面执行断言
// ---------------------------------------------------------------------------

/// A cursor-acting action's approval token is bound to the session and the
/// action summary only — the pointer moving after approval does NOT
/// invalidate it (mainstream model: an approval is a per-action id; there
/// is no cursor-origin binding).
#[tokio::test]
async fn approved_cursor_action_spends_even_after_the_pointer_moved() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // 光标默认 (7,9)，其上覆盖一个后果性目标 → down 被拦、铸造 pending。
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

    // 光标移走后重放：令牌只绑会话与摘要，注入照常执行。
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
    // 单次有效：令牌已被消费。
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

/// 令牌只绑**动作摘要**（主流模型）：同为 N 字符的另一段文本产生相同摘要
/// `type 21 characters`——令牌会花在它上面（内容级指纹绑定已按主流口径
/// 移除；摘要即用户批准的粒度）。单次有效语义不变。
#[tokio::test]
async fn same_summary_type_text_spends_the_token() {
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

    // 同长不同文：摘要相同（type 21 characters）→ 令牌消费，注入执行。
    let swapped = "XXXXXXXXXXXXXXXXXXXXX"; // 21 chars
    let replay = fixture
        .tool
        .execute(
            json!({"action": "type", "text": swapped, "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    let replay = match replay {
        Ok(r) => r,
        Err(e) => panic!("replay execute failed: {e}"),
    };
    assert!(replay.success, "{}", replay.content);
    assert_eq!(
        fixture.mock.lock().typed.last().map(String::as_str),
        Some(swapped),
        "the token binds the summary, so a same-summary text spends it"
    );

    // 单次有效：令牌已被消费，原文重放被拒。
    let spend = fixture
        .tool
        .execute(
            json!({"action": "type", "text": approved_text, "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    let text = spend.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        text.contains("invalid, expired, or was already used"),
        "{text}"
    );
    assert_eq!(
        fixture.mock.lock().typed.len(),
        1,
        "only the first replay injects"
    );
}

/// 消费时光标位置读不出（Wayland 首次 move 前的常态）与令牌无关：令牌只绑
/// 会话与摘要，没有光标比对——光标不可读不阻碍已批准动作的执行。
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

/// Round-6 评审缺口：drag / scroll / hold_key 的 happy-path 此前从不断言
/// 「真的执行了」——mock 有记录字段但零断言，静默丢调用的重构照样全绿。
#[tokio::test]
async fn drags_scrolls_and_holds_execute_on_granted_actions() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // 无元素覆盖 → 筛查 Clear，三个动作直接执行。
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

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
    /// cursor_position 的返回值（设备像素；评审修复：可配置以驱动「光标
    /// 移动后令牌失配」的场景）。默认 (7,9)（历史行为）。
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
        let mut rgba = vec![0u8; 16 * 16 * 4];
        for (i, byte) in rgba.iter_mut().enumerate() {
            *byte = (i % 253) as u8;
        }
        Ok(Capture {
            rgba,
            width: 16,
            height: 16,
            origin_x: state.capture_origin.0,
            origin_y: state.capture_origin.1,
            input_scale_x: 1.0,
            input_scale_y: 1.0,
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
/// only an a11y query *error* fails closed.
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
// 评审修复回归:层级绕过 / fail-open / 令牌绑定 / 能力先行
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

/// 筛查故障必须失败关闭:无法证明目标无害 = 要求确认,绝不放行。
#[tokio::test]
async fn unscreenable_a11y_failure_requires_confirmation() {
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
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("NOT executed"), "{text}");
    assert!(text.contains("unverifiable target"), "{text}");
    assert!(fixture.mock.lock().clicked.is_empty());
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

/// 评审修复回归：焦点查询失败时键盘动作必须失败关闭（Unscreenable → 确认）。
#[tokio::test]
async fn focus_query_failure_fails_closed_for_typing() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    fixture.mock.lock().focused_error = true;
    let result = fixture
        .tool
        .execute(
            json!({"action": "type", "text": "hello"}),
            &context(&fixture.workspace),
        )
        .await;
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("NOT executed"), "{text}");
    assert!(text.contains("unverifiable target"), "{text}");
    assert!(fixture.mock.lock().typed.is_empty());
}

/// 明确无焦点元素 = 无处键入，type 放行（Ok(None) ≠ Err，不误伤）。
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

/// drag 的落点(而不只是光标)必须被筛查:起点无元素、终点命中 denylist。
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
}

/// Denial closes the dialog AND suppresses the identical retry (round-6
/// review: without suppression, the model's retry re-opens the blocking
/// modal on every attempt — consent fatigue with approval as the only
/// in-app exit). The denied id itself reads as invalid/spent on retry and
/// mints nothing; the SAME action is refused server-side with NO new
/// confirm event; a DIFFERENT action still goes through the normal flow.
#[tokio::test]
async fn denied_action_retry_is_suppressed_without_a_new_dialog() {
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

    // A fresh attempt of the SAME action is refused server-side: the error
    // names the denial, no new confirm event is emitted (the modal must not
    // re-open), and nothing is injected.
    let events_before = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    let again = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 5, "y": 5}),
            &context(&fixture.workspace),
        )
        .await;
    let again_text = again.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        again_text.contains("denied this exact action"),
        "{again_text}"
    );
    assert!(again_text.contains("t3-denied-recently"), "{again_text}");
    let events_after = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    assert_eq!(
        events_before
            .iter()
            .filter(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
            .count(),
        events_after
            .iter()
            .filter(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
            .count(),
        "a suppressed retry must not emit a new confirm_required event"
    );
    assert!(fixture.mock.lock().clicked.is_empty());

    // A DIFFERENT action (different summary) still goes through the normal
    // confirmation flow with a NEW confirm_id — suppression is per-summary,
    // not a blanket gag.
    let other = fixture
        .tool
        .execute(
            json!({"action": "right_click", "x": 5, "y": 5}),
            &context(&fixture.workspace),
        )
        .await;
    assert!(
        other
            .ok()
            .map(|r| r.content)
            .unwrap_or_default()
            .contains("NOT executed")
    );
    let fresh_id = latest_confirm_id(&fixture.events);
    assert_ne!(fresh_id, denied_id, "a new request must mint a new id");
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

/// The Type summary is the character count plus a truncated SHA-256 content
/// fingerprint (`type N characters [fp …]`): no raw text, but a minted
/// approval can no longer be replayed on a different same-length text
/// (round-6 review). For a non-secure target the eligible full text rides
/// `type_preview_full` instead; control characters are the frontend's
/// concern, not the summary's.
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

    assert_eq!(
        summary,
        format!(
            "type 20 characters [fp {:016x}]",
            content_fingerprint(&text)
        ),
        "{summary}"
    );
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
/// character count: no plaintext, no inline preview, no fingerprint — and
/// no full-text preview is ever emitted for it.
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

    // 明文不得出现在摘要；摘要只有字符数 + 指纹。
    assert_eq!(
        summary,
        format!(
            "type 14 characters [fp {:016x}]",
            content_fingerprint(&text)
        ),
        "{summary}"
    );
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
    let env_lock = crate::platform::paths::tests::ENV_LOCK
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

    // SAFETY: 持有 ENV_LOCK（上面同一把锁未释放）。
    unsafe {
        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
    }
    drop(env_lock);
    let _ = std::fs::remove_dir_all(&workspace);
    let _ = std::fs::remove_file(&blocker);
}

/// 混合 DPI 防护：光标在截图显示器之外时，cursor_position 不硬换算，
/// 回报设备坐标并附警告文本。
#[tokio::test]
async fn cursor_outside_captured_monitor_reports_device_position_with_warning() {
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
    assert!(text.contains("device position (7, 9)"), "{text}");
    assert!(text.contains("outside the captured monitor"), "{text}");
    assert!(text.contains("warning: cursor is outside"), "{text}");
    assert!(!text.contains("in screenshot space"), "{text}");
}

/// 混合 DPI 防护：光标在截图显示器之外时 mouse_down 的筛查无法定位目标
/// ——失败关闭（Unscreenable → 要求确认），绝不拿跨屏垃圾坐标筛查。
#[tokio::test]
async fn mouse_down_outside_captured_monitor_fails_closed() {
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
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("NOT executed"), "{text}");
    assert!(text.contains("outside the captured monitor"), "{text}");
    assert!(text.contains("confirm_id"), "{text}");
    // 第三轮评审修复回归：拦截时按下不得真的下发。
    assert!(
        fixture.mock.lock().downed.is_empty(),
        "down must not execute"
    );
}

// ---------------------------------------------------------------------------
// 第三轮评审修复回归：和弦语义筛查 / 按住拖拽筛查
// ---------------------------------------------------------------------------

/// 破坏性和弦（修饰键 + Delete/Backspace、修饰键 + Enter）无论焦点元素
/// 名单判定如何都要求确认：焦点元素的 name 看不出按键会触发的后果
/// （聊天框 cmd+Enter 发送、Finder cmd+delete 删文件）。
#[tokio::test]
async fn destructive_key_chords_require_confirmation() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // 焦点是普通文本域（名单判定 Clear）——和弦语义仍须拦截。
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
        // 评审修复回归：Shift 参与 Delete/Backspace 判定——shift+delete 是
        // 绕过回收站的永久删除。
        "shift+delete",
        "shift+Backspace",
    ] {
        let result = fixture
            .tool
            .execute(
                json!({"action": "key", "text": chord}),
                &context(&fixture.workspace),
            )
            .await;
        let text = result.ok().map(|r| r.content).unwrap_or_default();
        assert!(text.contains("NOT executed"), "{chord}: {text}");
        assert!(text.contains("confirm_id"), "{chord}: {text}");
    }
    assert!(fixture.mock.lock().chords.is_empty(), "no chord may inject");
    // 普通键不受影响：无修饰键的 Enter/Delete 直接放行（文本编辑主路径）；
    // shift+Enter 是换行等键入形态，不升级为强制确认（Shift 不参与 Enter 判定）。
    for chord in ["Return", "Delete", "Backspace", "shift+Return"] {
        let result = fixture
            .tool
            .execute(
                json!({"action": "key", "text": chord}),
                &context(&fixture.workspace),
            )
            .await;
        let text = result.ok().map(|r| r.content).unwrap_or_default();
        assert!(!text.contains("NOT executed"), "{chord}: {text}");
    }
    assert_eq!(fixture.mock.lock().chords.len(), 4);
}

/// 按住左键期间 mouse_move 实质是拖拽：落点必须过 T3 筛查（拆解
/// down→move→up 不得零确认把目标拖进后果性位置）。
#[tokio::test]
async fn mouse_move_while_button_held_is_screened() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // A benign element covers the cursor (7,9): the down screening point
    // passes and the held state is set.
    fixture.mock.lock().element = Some(benign_element(0, 0, 16, 16));
    let result = fixture
        .tool
        .execute(
            json!({"action": "left_mouse_down"}),
            &context(&fixture.workspace),
        )
        .await;
    assert!(
        result
            .ok()
            .map(|r| r.content)
            .unwrap_or_default()
            .contains("mouse button is down")
    );
    // 目标 (12,12) 处是回收站：按住期间的 move 必须被拦截。
    fixture.mock.lock().element = Some(ElementInfo {
        role: "AXButton".to_string(),
        name: "Trash".to_string(),
        x: 10,
        y: 10,
        width: 5,
        height: 5,
        secure: false,
    });
    let result = fixture
        .tool
        .execute(
            json!({"action": "mouse_move", "x": 12, "y": 12}),
            &context(&fixture.workspace),
        )
        .await;
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("NOT executed"), "{text}");
    assert!(text.contains("confirm_id"), "{text}");
    // Same move with the button released passes as before (release first:
    // up at (7,9) has a benign element, so the screening reads Clear).
    fixture.mock.lock().element = Some(benign_element(0, 0, 16, 16));
    let result = fixture
        .tool
        .execute(
            json!({"action": "left_mouse_up"}),
            &context(&fixture.workspace),
        )
        .await;
    assert!(
        result
            .ok()
            .map(|r| r.content)
            .unwrap_or_default()
            .contains("mouse button is up")
    );
    let moved: Vec<_> = fixture.mock.lock().moved_to.clone();
    let result = fixture
        .tool
        .execute(
            json!({"action": "mouse_move", "x": 12, "y": 12}),
            &context(&fixture.workspace),
        )
        .await;
    let moved_text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        moved_text.contains("mouse moved to"),
        "unheld move must execute: {moved_text}"
    );
    assert_eq!(
        fixture.mock.lock().moved_to.len(),
        moved.len() + 1,
        "exactly one more move after release"
    );
}

/// 评审修复回归：mouse_move 的确认摘要必须绑定坐标。旧的兜底摘要只有动作
/// 名——弹窗盲批 "mouse_move"，且批准后带任意坐标重试都能重建相同摘要。
#[tokio::test]
async fn held_move_confirmation_binds_coordinates() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // A benign element covers the cursor (7,9): the down screening point
    // passes.
    fixture.mock.lock().element = Some(benign_element(0, 0, 16, 16));
    let result = fixture
        .tool
        .execute(
            json!({"action": "left_mouse_down"}),
            &context(&fixture.workspace),
        )
        .await;
    assert!(
        result
            .ok()
            .map(|r| r.content)
            .unwrap_or_default()
            .contains("mouse button is down")
    );
    // (12,12) 是回收站：按住 move 被拦，确认事件必须带目的地坐标。
    fixture.mock.lock().element = Some(ElementInfo {
        role: "AXButton".to_string(),
        name: "Trash".to_string(),
        x: 10,
        y: 10,
        width: 5,
        height: 5,
        secure: false,
    });
    let result = fixture
        .tool
        .execute(
            json!({"action": "mouse_move", "x": 12, "y": 12}),
            &context(&fixture.workspace),
        )
        .await;
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("NOT executed"), "{text}");
    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    let confirmed = events
        .iter()
        .rev()
        .find(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
        .expect("confirm event");
    let confirm_id = confirmed.1["confirm_id"].as_str().unwrap_or_default();
    let summary = confirmed.1["action"].as_str().unwrap_or_default();
    assert!(
        summary.contains("(12, 12)"),
        "summary must bind the destination: {summary}"
    );
    fixture.shared.mint_confirmation(confirm_id);
    // 拿 (12,12) 的令牌去 move (2,2)——摘要失配必须被拒。
    let replay = fixture
        .tool
        .execute(
            json!({"action": "mouse_move", "x": 2, "y": 2, "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    let text = replay.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        text.contains("invalid, expired, or was already used"),
        "{text}"
    );
    assert!(
        fixture.mock.lock().moved_to.is_empty(),
        "no move may inject"
    );
}

/// 不可筛 → 用户批准 → 重试执行 的既有放行语义不得被重筛误伤：
/// Unscreenable 在批准语境下是"用户已在知情下批准"，不二次索要确认。
#[tokio::test]
async fn approved_unscreenable_action_executes_after_confirmation() {
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
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("NOT executed"), "{text}");
    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    let confirm_id = events
        .iter()
        .rev()
        .find(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
        .map(|(_, p)| p["confirm_id"].as_str().unwrap_or_default().to_string())
        .expect("confirm event");
    fixture.shared.mint_confirmation(&confirm_id);
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
        "user-approved unverifiable action must execute: {text}"
    );
    assert_eq!(fixture.mock.lock().clicked.len(), 1);
}

// ---------------------------------------------------------------------------
// 评审修复回归:撤销/停止必须终止持久 OS 级授权(登记表 → release_os_grant)
// ---------------------------------------------------------------------------

/// revoke 经登记表触发 emergency release(detached 线程,轮询等待):
/// 先释放物理左键、再关闭后端持久 OS 级授权(worker 通道串行化保证此
/// 顺序),并同步注销句柄。Wayland portal 会话由此随用户"停止控制"终止,
/// 而不是活到进程退出。
#[tokio::test]
async fn emergency_release_unregisters_releases_button_then_os_grant() {
    let (fixture, _restore) = fixture();
    assert!(
        fixture.shared.backends.contains("s-test"),
        "tool construction must register its backend handle"
    );
    fixture.shared.backends.emergency_release("s-test");
    assert!(
        !fixture.shared.backends.contains("s-test"),
        "emergency_release must unregister the handle"
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
/// flag. The click executes without any confirmation; only a query *error*
/// fails closed (see `unscreenable_a11y_failure_requires_confirmation`).
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

    // The held state must be cleared: a held move would be T3-screened (and
    // blocked at the Trash), while a released move executes.
    fixture.mock.lock().element = Some(ElementInfo {
        role: "AXButton".to_string(),
        name: "Trash".to_string(),
        x: 10,
        y: 10,
        width: 5,
        height: 5,
        secure: false,
    });
    let result = fixture
        .tool
        .execute(
            json!({"action": "mouse_move", "x": 12, "y": 12}),
            &context(&fixture.workspace),
        )
        .await;
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        !text.contains("NOT executed"),
        "held state must be cleared after the safety-net release: {text}"
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
    // The dialog summary stays a character count plus content fingerprint
    // (the expander is the full view).
    let summary = payload["action"].as_str().unwrap_or_default();
    assert_eq!(
        summary,
        format!(
            "type {} characters [fp {:016x}]",
            text.chars().count(),
            content_fingerprint(&text)
        ),
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
// Round-6 回归：光标原点绑定 / 同长文本重放 / 光标未知消费 / 注入面执行断言
// ---------------------------------------------------------------------------

/// approve-then-move：光标类动作（left_mouse_down）铸造时记录光标原点，
/// 光标移走后令牌被拒（Stale，令牌保留），移回原位才能消费执行。
#[tokio::test]
async fn approve_then_move_breaks_cursor_bound_tokens() {
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

    // 光标移走后重放：Stale——批准与目标脱钩的入口被封死。
    fixture.mock.lock().cursor = (3, 4);
    let replay = fixture
        .tool
        .execute(
            json!({"action": "left_mouse_down", "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    let text = replay.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        text.contains("the pointer has moved since the user approved"),
        "{text}"
    );
    assert!(
        fixture.mock.lock().downed.is_empty(),
        "a moved-pointer replay must not inject"
    );

    // 光标回到批准时的原位：消费成功，注入执行。
    fixture.mock.lock().cursor = (7, 9);
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
        "the approved action executes at the approved origin"
    );
}

/// 同长文本重放：type 令牌绑定内容指纹，同为 N 字符的另一段文本花不掉
/// 批准（round-6 评审：无指纹时一次批准可花在任意同长文本上）。
#[tokio::test]
async fn same_length_type_replay_breaks_the_token() {
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

    // 同长不同文：摘要（指纹）失配 → 拒绝且不注入。
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
        "{text}"
    );
    assert!(
        fixture.mock.lock().typed.is_empty(),
        "a same-length text swap must not inject"
    );

    // 原文重放：执行。
    let spend = fixture
        .tool
        .execute(
            json!({"action": "type", "text": approved_text, "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    let spend = match spend {
        Ok(r) => r,
        Err(e) => panic!("spend execute failed: {e}"),
    };
    assert!(spend.success, "{}", spend.content);
    assert_eq!(
        fixture.mock.lock().typed.last().map(String::as_str),
        Some(approved_text)
    );
}

/// 消费时光标位置读不出（Wayland 首次 move 前的常态）：光标类令牌无法
/// 验证原点 → fail-closed 拒绝，不注入。
#[tokio::test]
async fn cursor_unknown_at_spend_fails_closed() {
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
    let text = spend.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        text.contains("the pointer has moved since the user approved"),
        "an unreadable cursor cannot verify the origin and must be refused: {text}"
    );
    assert!(fixture.mock.lock().downed.is_empty());
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

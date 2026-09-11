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
/// 走到执行（评审修复后「目标点无 a11y 元素」会失败关闭要求确认）。
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
    // A benign element covers the (5,6) target: an element-free point now
    // fails closed and demands confirmation.
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
    // A benign element covers the clamped (15,15) point: an element-free
    // point now fails closed and demands confirmation.
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
    // A benign element covers the (3,4) screening point: an element-free
    // point now fails closed and demands confirmation.
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

/// 用户拒绝后的 confirm_id 重试必须得到明确「已被拒绝」。
#[tokio::test]
async fn denied_confirmation_reports_denial_on_retry() {
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
    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    let confirm_id = events
        .iter()
        .rev()
        .find(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
        .map(|(_, p)| p["confirm_id"].as_str().unwrap_or_default().to_string())
        .expect("confirm event");
    assert!(
        first
            .ok()
            .map(|r| r.content)
            .unwrap_or_default()
            .contains("NOT executed")
    );
    fixture.shared.deny_confirmation(&confirm_id);
    let retry = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 5, "y": 5, "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    let text = retry.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("user denied"), "{text}");
    assert!(fixture.mock.lock().clicked.is_empty());
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
// 评审修复回归:确认摘要含预览+指纹 / 令牌绑定内容而非长度
// ---------------------------------------------------------------------------

/// 评审修复回归：同长度不同文本不能换用同一确认令牌（旧摘要只有
/// `type N chars`，令牌盲绑字符数）。
#[tokio::test]
async fn type_confirm_token_is_bound_to_text_not_length() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // 焦点在密码框：两次同长度（8 字符）的 type 都会被拦。
    fixture.mock.lock().focused = Some(ElementInfo {
        role: "AXSecureTextField".to_string(),
        name: "Password".to_string(),
        x: 0,
        y: 0,
        width: 10,
        height: 10,
        secure: true,
    });
    let first = " hunter2".to_string();
    let second = "wrongpwd".to_string();
    assert_eq!(
        first.chars().count(),
        second.chars().count(),
        "预置条件：同长度"
    );
    let result = fixture
        .tool
        .execute(
            json!({"action": "type", "text": first}),
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
    let confirm_id = events
        .iter()
        .rev()
        .find(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
        .map(|(_, p)| p["confirm_id"].as_str().unwrap_or_default().to_string())
        .expect("confirm event");
    fixture.shared.mint_confirmation(&confirm_id);

    // 拿「 hunter2」的令牌去键入「wrongpwd」（同长度）——必须被拒。
    let replay = fixture
        .tool
        .execute(
            json!({"action": "type", "text": second, "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    let text = replay.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        text.contains("invalid, expired, or was already used"),
        "same-length text must not consume the token: {text}"
    );
    assert!(fixture.mock.lock().typed.is_empty());
}

/// 评审修复回归：确认摘要在「N chars」之外必须含安全预览（控制字符转义、
/// 超长省略）与全文指纹，向用户展示即将键入的内容（知情批准）。
/// 第三轮评审修正拆成两个用例：普通目标展示预览；密码字段目标掩码预览
/// （明文预览会把密码前缀送进确认弹窗/事件流）。
#[tokio::test]
async fn type_summary_shows_preview_and_fingerprint() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    fixture.mock.lock().focused = Some(ElementInfo {
        // 非 secure 但命中 T3 名单的焦点元素：走确认流程且预览不掩码。
        role: "AXButton".to_string(),
        name: "Send message".to_string(),
        x: 0,
        y: 0,
        width: 10,
        height: 10,
        secure: false,
    });
    // 20 个字符（> 预览上限 12），第 6 个字符是控制字符 BEL。
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
    let summary = events
        .iter()
        .rev()
        .find(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
        .map(|(_, p)| p["action"].as_str().unwrap_or_default().to_string());
    let summary = summary.expect("confirm event carries the action summary");

    assert!(summary.starts_with("type 20 chars:"), "{summary}");
    // 预览 = 前 12 字符（"hello<BEL>world123"）+ 省略号；控制字符转义为 \uXXXX。
    assert!(
        summary.contains(r"\u0007"),
        "control char must be escaped: {summary}"
    );
    assert!(
        !summary.contains('\u{7}'),
        "raw control char must not appear: {summary:?}"
    );
    assert!(
        summary.contains('…'),
        "truncated preview must end with ellipsis: {summary}"
    );
    assert!(summary.contains("\"hello"), "{summary}");
    // 截断之后的内容不进摘要。
    assert!(!summary.contains("456789"), "{summary}");
    // 指纹 = 全文 SHA-256 前 16 hex（64 位：32 位对持壳模型的暴力碰撞不够，
    // 第三轮评审发现）。
    let expected_hash = super::super::audit::sha256_hex(text.as_bytes());
    let expected_fingerprint = &expected_hash[..16];
    assert!(
        summary.contains(&format!("[{expected_fingerprint}]")),
        "fingerprint {expected_fingerprint} must be in summary: {summary}"
    );
    // 不同文本 → 不同指纹（同长度令牌不可换用的机制保证）。
    let other = "hello\u{7}world87654321".to_string();
    let other_hash = super::super::audit::sha256_hex(other.as_bytes());
    let other_fingerprint = &other_hash[..16];
    assert_ne!(expected_fingerprint, other_fingerprint);
}

/// 第三轮评审修复回归：type 的目标是密码/安全字段时，确认摘要**掩码**
/// 预览——「向密码框键入」必然走确认流程，明文预览等于把密码前缀送进
/// 确认弹窗与事件流。指纹与长度仍然展示（知情+完整性）。
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
    let summary = events
        .iter()
        .rev()
        .find(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
        .map(|(_, p)| p["action"].as_str().unwrap_or_default().to_string());
    let summary = summary.expect("confirm event carries the action summary");

    // 明文不得出现在摘要；长度+指纹保留。
    assert!(!summary.contains("hunter2"), "{summary}");
    assert!(summary.contains("type 14 chars:"), "{summary}");
    assert!(
        summary.contains("<masked: the typing target is a password/secure field>"),
        "masked preview marker must be present: {summary}"
    );
    let expected_hash = super::super::audit::sha256_hex(text.as_bytes());
    assert!(
        summary.contains(&format!("[{}]", &expected_hash[..16])),
        "fingerprint must still be shown: {summary}"
    );
}

// ---------------------------------------------------------------------------
// 评审修复回归:审计内容（单字符 key 走 HMAC；含修饰键的和弦保留明文）
// ---------------------------------------------------------------------------

/// 评审修复回归：`key "p"` 逐字符泄漏密码——单 Char 无修饰键的和弦按
/// 键入文本审计（长度 + salt || text 的 HMAC），明文不进 JSONL；
/// `ctrl+s` 等快捷键和弦保留明文。
/// Duplicate-modifier permutations (shift+shift+h etc.) are classified by
/// content regardless of token position and are typing forms all the same.
#[tokio::test]
async fn single_char_key_chord_is_audited_as_typed_text() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // 评审修复回归：shift+字符（两种顺序，解析保留模型给定顺序）是大写/
    // 符号的键入形态，与裸单字符同策略——只记 HMAC，明文不得进 JSONL
    // （`key "shift+H"` 逐字符可拼出密码）。
    // Duplicate-modifier chords (regression fix: `shift+shift+h` slipped
    // past the positional pattern while still typing a capital H;
    // `h+shift+shift` types a lowercase h) are audited as typed text too.
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

    let audit_path = fixture.home.join("computer-use").join("audit-s-test.jsonl");
    let raw = std::fs::read_to_string(&audit_path).expect("audit jsonl exists");
    let records: Vec<serde_json::Value> = raw
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).expect("valid jsonl line"))
        .collect();
    let begins: Vec<&serde_json::Value> = records
        .iter()
        .filter(|r| r["action"] == "key" && r["phase"] == "begin")
        .collect();
    assert_eq!(begins.len(), 7, "seven key calls: {records:?}");

    let typed: Vec<&serde_json::Value> = begins
        .iter()
        .copied()
        .filter(|r| r["target"] == "keyboard focus")
        .collect();
    assert_eq!(
        typed.len(),
        6,
        "typed-text chords (incl. duplicate-modifier ones) audited as typed text"
    );
    // Lengths = the chord text lengths.
    let mut lens: Vec<u64> = typed
        .iter()
        .map(|r| r["text_len"].as_u64().unwrap_or_default())
        .collect();
    lens.sort();
    assert_eq!(
        lens,
        vec![1, 7, 7, 13, 13, 13],
        "typed-text lengths: {typed:?}"
    );
    for record in &typed {
        // 与 type 同策略：盐 + HMAC 齐备。
        assert_eq!(record["salt"].as_str().unwrap_or_default().len(), 32);
        assert_eq!(
            record["text_hmac_sha256"]
                .as_str()
                .unwrap_or_default()
                .len(),
            64
        );
    }
    // 修饰键和弦保留明文（快捷键无字典风险）。
    let chorded = begins
        .iter()
        .copied()
        .find(|r| r["target"] == "keys: ctrl+s")
        .expect("modifier chord keeps plaintext target");
    assert!(chorded["text_len"].is_null());
    assert!(chorded["text_hmac_sha256"].is_null());
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
// 评审修复回归:审计 fail-closed / 混合 DPI 出界防护
// ---------------------------------------------------------------------------

/// 审计目录不可用时：Input 类拒绝执行；Observe 类降级可用（只读无注入
/// 后果）。PINVOU3_HOME 指向一个普通文件即可让审计目录创建失败。
#[tokio::test]
// ENV_LOCK 必须横跨 await 持有：run() 在 spawn_blocking 线程上按进程级 env
// 解析 PINVOU3_HOME，env 与锁都要活到测试结束（与 fixture() 同一约定）。
#[allow(clippy::await_holding_lock)]
async fn input_action_fails_closed_when_audit_dir_unavailable() {
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

    // workspace 与审计目录解耦：放在独立临时目录，观察类动作仍可执行。
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

    // Input 类：审计不可用 → 拒绝执行。
    let typed = tool
        .execute(
            json!({"action": "type", "text": "hello"}),
            &context(&workspace),
        )
        .await;
    let text = typed.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("audit log is unavailable"), "{text}");
    assert!(text.contains("refusing"), "{text}");
    assert!(mock.lock().typed.is_empty(), "must not type without audit");

    // Observe 类：维持 eprintln 降级，动作照常。
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
// 第三轮评审修复回归：和弦语义筛查 / 按住拖拽筛查 / 无坐标令牌的光标绑定
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
    // passes and the held state is set (an element-free point now fails
    // closed).
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
    // passes (an element-free point now fails closed).
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

/// 评审修复回归：批准后重试必须重筛。批准到重试之间隔着任意模型调用
/// （可重新截图/焦点已移动），摘要绑定的静态参数挡不住"目标处的东西
/// 变了"；重筛确定性命中后果性名单时作废本次批准、要求重新确认。
#[tokio::test]
async fn approved_retry_is_rescreened_against_the_current_world() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // 铸造时刻 a11y 故障：不可筛 → 走人工确认。
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
    // 世界变了：目标处现在是回收站。批准不得静默生效——重筛命中名单，
    // 令牌作废（单次有效），要求重新确认。
    fixture.mock.lock().element_error = false;
    fixture.mock.lock().element = Some(ElementInfo {
        role: "AXButton".to_string(),
        name: "Trash".to_string(),
        x: 0,
        y: 0,
        width: 10,
        height: 10,
        secure: false,
    });
    let replay = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 5, "y": 5, "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    let text = replay.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("NOT executed"), "{text}");
    assert!(text.contains("consequential"), "{text}");
    assert!(fixture.mock.lock().clicked.is_empty(), "must not click");
    // 新的确认请求已发出（用户要再批一次）。
    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    assert_eq!(
        events
            .iter()
            .filter(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
            .count(),
        2,
        "a fresh confirmation must be requested: {events:?}"
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

/// 无坐标 click 的令牌绑定含光标位置：铸造后把光标移到别的目标再花令牌，
/// 摘要失配必须被拒（用户批准的落点与实际执行落点不同）。
#[tokio::test]
async fn token_for_coordinateless_click_breaks_when_cursor_moves() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // 光标 (7,9) 处是 denylist 控件 → 拦截并铸造（摘要含 cursor Some((7,9))）。
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
            json!({"action": "left_click"}),
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
    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    let confirm_id = events
        .iter()
        .rev()
        .find(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
        .map(|(_, p)| p["confirm_id"].as_str().unwrap_or_default().to_string())
        .expect("confirm event");
    assert!(fixture.shared.mint_confirmation(&confirm_id));

    // 用户批准后模型把光标移走再花令牌：摘要失配 → 拒绝且不执行。
    fixture.mock.lock().cursor = (12, 12);
    let retry = fixture
        .tool
        .execute(
            json!({"action": "left_click", "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    let text = retry.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("confirm_id is invalid"), "{text}");
    assert!(fixture.mock.lock().clicked.is_empty(), "click must not run");

    // 光标回到批准时的位置：同一令牌照常消费执行。
    fixture.mock.lock().cursor = (7, 9);
    let retry = fixture
        .tool
        .execute(
            json!({"action": "left_click", "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    assert!(
        retry
            .ok()
            .map(|r| r.content)
            .unwrap_or_default()
            .contains("click executed"),
        "same cursor position must spend the token"
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
// Regression: element-free targets fail closed / approvals bind target
// geometry / stable audit error code
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

/// Regression: a target point with no a11y element = unscreenable (no label
/// to check, no geometry to bind) — fail closed with a confirmation instead
/// of silently passing; after the user approves (verified_target=false mint)
/// the retry executes.
#[tokio::test]
async fn click_without_a11y_element_requires_confirmation() {
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
    let text = result.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("NOT executed"), "{text}");
    assert!(
        text.contains("no accessibility element exists at the target point"),
        "{text}"
    );
    assert!(fixture.mock.lock().clicked.is_empty(), "must not click");

    let confirm_id = latest_confirm_id(&fixture.events);
    assert!(fixture.shared.mint_confirmation(&confirm_id));
    let retry = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 5, "y": 5, "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    let text = retry.ok().map(|r| r.content).unwrap_or_default();
    assert!(
        !text.contains("NOT executed"),
        "user-approved unverifiable target must execute: {text}"
    );
    assert_eq!(fixture.mock.lock().clicked.len(), 1);
}

/// Regression: an approval token minted from a concrete hit
/// (verified_target=true) must not take effect silently when, at spend time,
/// the target reads Clear (element gone / replaced) — fail closed and
/// re-request confirmation (the token is single-use, already consumed).
#[tokio::test]
async fn approved_blocked_target_fails_closed_when_target_reads_clear_at_spend() {
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

    // The world changed: the target point now holds a benign element (Clear).
    fixture.mock.lock().element = Some(benign_element(0, 0, 16, 16));
    let replay = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 5, "y": 5, "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    let text = replay.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("NOT executed"), "{text}");
    assert!(
        text.contains("no longer reads as itself"),
        "fail-closed re-request expected: {text}"
    );
    assert!(fixture.mock.lock().clicked.is_empty(), "must not click");
    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    assert_eq!(
        events
            .iter()
            .filter(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
            .count(),
        2,
        "a fresh confirmation must be requested: {events:?}"
    );
}

/// Regression: an approval minted from a concrete hit must also fail closed
/// when the screening goes blind at spend time (Unscreenable) — "the
/// approved target no longer reads as itself" no longer passes.
#[tokio::test]
async fn approved_blocked_target_fails_closed_when_unscreenable_at_spend() {
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

    fixture.mock.lock().element_error = true;
    let replay = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 5, "y": 5, "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    let text = replay.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("NOT executed"), "{text}");
    assert!(
        text.contains("no longer reads as itself"),
        "fail-closed re-request expected: {text}"
    );
    assert!(fixture.mock.lock().clicked.is_empty(), "must not click");
    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    assert_eq!(
        events
            .iter()
            .filter(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
            .count(),
        2,
        "a fresh confirmation must be requested: {events:?}"
    );
}

/// A verified_target=false mint (the user approved in an unscreenable
/// context) still passes when the target reads Clear at spend time — the
/// existing semantics must not be broken by the geometry binding.
#[tokio::test]
async fn unverified_approval_passes_when_target_reads_clear_at_spend() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // At mint time the a11y query fails: unscreenable -> manual confirmation.
    fixture.mock.lock().element_error = true;
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

    // At spend time the target point reads as a benign element (Clear).
    fixture.mock.lock().element_error = false;
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
        "unverified approval must pass on Clear: {text}"
    );
    assert_eq!(fixture.mock.lock().clicked.len(), 1);
}

/// Regression: the approval token binds the element geometry — the same
/// label moving beyond the tolerance (center drift >12px) counts as a
/// different target: the approval is invalidated and confirmation is
/// re-requested.
#[tokio::test]
async fn approval_invalidated_when_element_moves_beyond_tolerance() {
    let (fixture, _restore) = fixture();
    fixture.shared.grant_session("s-test");
    // At mint time: "Buy now" spans (0,0,100,100), center (50,50).
    fixture.mock.lock().element = Some(ElementInfo {
        role: "AXButton".to_string(),
        name: "Buy now".to_string(),
        x: 0,
        y: 0,
        width: 100,
        height: 100,
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

    // At spend time: the same-named element shrank to (5,5,2,2), center
    // (6,6) — label matches but the geometry drifts far beyond 12px.
    fixture.mock.lock().element = Some(ElementInfo {
        role: "AXButton".to_string(),
        name: "Buy now".to_string(),
        x: 5,
        y: 5,
        width: 2,
        height: 2,
        secure: false,
    });
    let replay = fixture
        .tool
        .execute(
            json!({"action": "left_click", "x": 5, "y": 5, "confirm_id": confirm_id}),
            &context(&fixture.workspace),
        )
        .await;
    let text = replay.ok().map(|r| r.content).unwrap_or_default();
    assert!(text.contains("NOT executed"), "{text}");
    assert!(fixture.mock.lock().clicked.is_empty(), "must not click");
    let events = fixture.events.lock().map(|e| e.clone()).unwrap_or_default();
    assert_eq!(
        events
            .iter()
            .filter(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
            .count(),
        2,
        "a fresh confirmation must be requested: {events:?}"
    );
}

/// A small shift within the geometry tolerance (≤12px) is not punished:
/// label match + rect match -> the token spends.
#[tokio::test]
async fn approval_survives_small_element_drift() {
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

    // The same-named element moved by 2px: center (5,5) -> (7,7), within
    // the tolerance.
    fixture.mock.lock().element = Some(ElementInfo {
        role: "AXButton".to_string(),
        name: "Buy now".to_string(),
        x: 2,
        y: 2,
        width: 10,
        height: 10,
        secure: false,
    });
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
        "label + rect match must spend the token: {text}"
    );
    assert_eq!(fixture.mock.lock().clicked.len(), 1);
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
    let end_records: Vec<serde_json::Value> = raw
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("valid jsonl line"))
        .filter(|r| r["phase"] == "end")
        .collect();
    assert_eq!(end_records.len(), 1, "one finished call: {end_records:?}");
    assert_eq!(end_records[0]["result"], "error");
    assert_eq!(
        end_records[0]["error"], T3_CONFIRM_REQUIRED_ERROR,
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
    // The dialog summary stays truncated (the expander is the full view).
    assert!(
        payload["action"].as_str().unwrap_or_default().contains('…'),
        "summary preview must stay truncated: {payload}"
    );
}

/// Short type texts must not get `type_preview_full`: the 12-char summary
/// preview already shows the whole text, so the expander would be noise.
#[tokio::test]
async fn confirm_event_omits_full_type_preview_for_short_text() {
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
    assert!(
        payload.get("type_preview_full").is_none(),
        "short text must not carry the full preview: {payload}"
    );
}

/// Secure/masked typing targets must NEVER get `type_preview_full`: the
/// masked summary exists precisely so password text is not revealed in the
/// confirm dialog or event stream.
#[tokio::test]
async fn confirm_event_never_carries_full_type_preview_for_secure_targets() {
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
    let text = "hunter2super-secret-password-value".to_string();
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

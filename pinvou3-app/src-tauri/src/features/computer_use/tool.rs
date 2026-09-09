//! `computer_use` 工具：单一 ToolSpec，`action` 字段区分动作
//! （Anthropic computer_20250124 动作集 + a11y 扩展 ui_tree / element_at_point）。
//!
//! 关键不变量：
//! - 模型坐标永远是「截图空间」（最近一次返回 PNG 的像素，原点左上）。
//! - 每次截图生成新 ScaleMap 存入会话状态；带坐标的动作没有 ScaleMap 时先自动截图。
//! - 输入类与 scroll 动作执行后等待 [`POST_ACTION_SETTLE_MS`] 再补拍截图附上，
//!   模型始终看到最新状态。
//! - 同意门控（[`ComputerUseShared`]）在每次注入前检查；T3 后果性动作一律拦截。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter as _};

use deepseek_tui::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
};

use super::audit::{self, AuditLog, AuditRecord};
use super::backend::BackendHandle;
use super::guard::{ComputerUseShared, GuardRejection, is_secure_role, matches_t3_denylist};
use super::platform;
use super::scaling::{self, ScaleMap, ScaledScreenshot};
use super::types::{
    ActionClass, ComputerUseAction, ComputerUseError, EVENT_CONFIRM_REQUIRED, EVENT_GRANT_REQUIRED,
    MouseButton, ScrollDirection, TOOL_NAME, UiTreeOptions, parse_key_chord,
};

/// 输入/scroll 动作执行后的界面稳定等待（唯一的 settle 常数定义点）。
pub const POST_ACTION_SETTLE_MS: u64 = 350;
/// `wait` 上限 30 秒。
pub const MAX_WAIT_MS: u64 = 30_000;
/// `hold_key` 上限 30 秒。
pub const MAX_HOLD_KEY_MS: u64 = 30_000;
/// 单次 scroll 格数上限。
pub const MAX_SCROLL_AMOUNT: u32 = 100;
/// `ui_tree` 参数上限。
pub const MAX_UI_TREE_DEPTH: u32 = 64;
pub const MAX_UI_TREE_NODES: u32 = 10_000;

/// 截图存放的子目录（相对 workspace）：engine 的 image_analyze 回退按
/// workspace 相对路径解析，`attachments/` 是它的既定根。
const ATTACHMENTS_DIR: &str = "attachments/computer_use";

/// Tauri 事件出口（测试注入记录器替代）。
pub trait ComputerUseEventSink: Send + Sync {
    fn emit(&self, event: &str, payload: Value);
}

pub struct TauriEventSink {
    app: AppHandle,
}

impl TauriEventSink {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl ComputerUseEventSink for TauriEventSink {
    fn emit(&self, event: &str, payload: Value) {
        // Tauri UI 发射 + 可选远控传输转发。
        let _ = self.app.emit(event, payload.clone());
        crate::platform::app_events::forward_app_event(&self.app, event, payload);
    }
}

#[derive(Default)]
struct ToolState {
    last_map: Option<ScaleMap>,
    shot_seq: u64,
}

/// 可克隆的执行部件（execute 是 async，后端调用全同步，整体移入
/// `spawn_blocking`；测试用 `with_parts` 注入 mock 后端与事件记录器）。
#[derive(Clone)]
struct Parts {
    session_id: String,
    shared: Arc<ComputerUseShared>,
    backend: BackendHandle,
    events: Arc<dyn ComputerUseEventSink>,
    state: Arc<Mutex<ToolState>>,
}

pub struct ComputerUseTool {
    parts: Parts,
}

impl ComputerUseTool {
    /// 生产构造：后端懒启动（首个请求才 spawn worker 线程）。
    pub fn new(app: AppHandle, session_id: String, shared: Arc<ComputerUseShared>) -> Self {
        Self {
            parts: Parts {
                session_id,
                shared,
                backend: BackendHandle::lazy(platform::create_backend),
                events: Arc::new(TauriEventSink::new(app)),
                state: Arc::new(Mutex::new(ToolState::default())),
            },
        }
    }

    /// 测试构造：注入 mock 后端与事件出口。
    #[cfg(test)]
    pub(crate) fn with_parts(
        session_id: String,
        shared: Arc<ComputerUseShared>,
        backend: BackendHandle,
        events: Arc<dyn ComputerUseEventSink>,
    ) -> Self {
        Self {
            parts: Parts {
                session_id,
                shared,
                backend,
                events,
                state: Arc::new(Mutex::new(ToolState::default())),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// 输入校验（Anthropic 风格：非法参数组合给描述性错误，模型据此自我修复）
// ---------------------------------------------------------------------------

struct ParsedCall {
    action: ComputerUseAction,
    confirm_id: Option<String>,
}

fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::invalid_input(message.into())
}

fn field_type_error(field: &str, expected: &str) -> ToolError {
    invalid(format!("{field} must be {expected}"))
}

fn opt_u64(input: &Value, field: &str, max: u64) -> Result<Option<u64>, ToolError> {
    match input.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let number = value
                .as_u64()
                .ok_or_else(|| field_type_error(field, "a non-negative integer"))?;
            if number > max {
                return Err(invalid(format!("{field} must be <= {max}; got {number}")));
            }
            Ok(Some(number))
        }
    }
}

fn opt_i64(input: &Value, field: &str) -> Result<Option<i64>, ToolError> {
    match input.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .map(Some)
            .ok_or_else(|| field_type_error(field, "an integer")),
    }
}

/// x/y 成对出现、非负（截图空间坐标）。
fn opt_coord(input: &Value) -> Result<Option<(i64, i64)>, ToolError> {
    let x = opt_i64(input, "x")?;
    let y = opt_i64(input, "y")?;
    match (x, y) {
        (None, None) => Ok(None),
        (Some(x), Some(y)) => {
            if x < 0 || y < 0 {
                return Err(invalid(format!(
                    "coordinates must be non-negative; got ({x}, {y})"
                )));
            }
            Ok(Some((x, y)))
        }
        _ => Err(invalid(
            "x and y must be provided together (or neither)".to_string(),
        )),
    }
}

fn req_text(input: &Value, action: &str) -> Result<String, ToolError> {
    match input.get("text") {
        Some(Value::String(text)) if !text.is_empty() => Ok(text.clone()),
        Some(Value::String(_)) => Err(invalid(format!(
            "text must be a non-empty string for {action}"
        ))),
        None | Some(Value::Null) => Err(invalid(format!("text is required for {action}"))),
        Some(_) => Err(field_type_error("text", "a string")),
    }
}

/// 拒绝该动作不接受的字段（`action`/`confirm_id` 全局通用，不计）。
fn reject_unexpected(input: &Value, action: &str, allowed: &[&str]) -> Result<(), ToolError> {
    let Some(object) = input.as_object() else {
        return Err(invalid("input must be a JSON object"));
    };
    for key in object.keys() {
        if key == "action" || key == "confirm_id" || allowed.contains(&key.as_str()) {
            continue;
        }
        if object.get(key).is_some_and(Value::is_null) {
            continue;
        }
        let label = match key.as_str() {
            "x" | "y" => "coordinate",
            other => other,
        };
        return Err(invalid(format!("{label} is not accepted for {action}")));
    }
    Ok(())
}

fn parse_action(input: &Value) -> Result<ParsedCall, ToolError> {
    let action_name = input
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::missing_field("action"))?;
    let confirm_id = match input.get("confirm_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) => Some(id.clone()),
        Some(_) => return Err(field_type_error("confirm_id", "a string")),
    };

    let coord = opt_coord(input)?;
    let ms = opt_u64(input, "ms", MAX_WAIT_MS.max(MAX_HOLD_KEY_MS))?;

    let action = match action_name {
        "screenshot" => {
            reject_unexpected(input, action_name, &[])?;
            ComputerUseAction::Screenshot
        }
        "cursor_position" => {
            reject_unexpected(input, action_name, &[])?;
            ComputerUseAction::CursorPosition
        }
        "wait" => {
            reject_unexpected(input, action_name, &["ms"])?;
            let ms = ms.ok_or_else(|| invalid("ms is required for wait"))?;
            if ms > MAX_WAIT_MS {
                return Err(invalid(format!("ms must be <= {MAX_WAIT_MS} for wait")));
            }
            ComputerUseAction::Wait { ms }
        }
        "ui_tree" => {
            reject_unexpected(input, action_name, &["max_depth", "max_nodes"])?;
            let max_depth = opt_u64(input, "max_depth", u64::from(MAX_UI_TREE_DEPTH))?;
            let max_nodes = opt_u64(input, "max_nodes", u64::from(MAX_UI_TREE_NODES))?;
            ComputerUseAction::UiTree {
                opts: UiTreeOptions {
                    max_depth: max_depth.map(|v| v as u32),
                    max_nodes: max_nodes.map(|v| v as u32),
                },
            }
        }
        "element_at_point" => {
            reject_unexpected(input, action_name, &["x", "y"])?;
            let (x, y) =
                coord.ok_or_else(|| invalid("x and y are required for element_at_point"))?;
            ComputerUseAction::ElementAtPoint { x, y }
        }
        "mouse_move" => {
            reject_unexpected(input, action_name, &["x", "y"])?;
            let (x, y) = coord.ok_or_else(|| invalid("x and y are required for mouse_move"))?;
            ComputerUseAction::MouseMove { x, y }
        }
        "scroll" => {
            reject_unexpected(input, action_name, &["direction", "amount", "x", "y"])?;
            let direction = input
                .get("direction")
                .and_then(Value::as_str)
                .and_then(ScrollDirection::parse)
                .ok_or_else(|| {
                    invalid("direction is required for scroll and must be up|down|left|right")
                })?;
            let amount = match input.get("amount") {
                None | Some(Value::Null) => {
                    return Err(invalid("amount is required for scroll"));
                }
                Some(value) => value
                    .as_u64()
                    .ok_or_else(|| field_type_error("amount", "a non-negative integer"))?,
            };
            if amount > u64::from(MAX_SCROLL_AMOUNT) {
                return Err(invalid(format!(
                    "amount must be <= {MAX_SCROLL_AMOUNT}; got {amount}"
                )));
            }
            ComputerUseAction::Scroll {
                direction,
                amount: amount as u32,
                at: coord,
            }
        }
        "left_click" | "right_click" | "middle_click" | "double_click" | "triple_click" => {
            reject_unexpected(input, action_name, &["x", "y"])?;
            let (button, count) = match action_name {
                "left_click" => (MouseButton::Left, 1),
                "right_click" => (MouseButton::Right, 1),
                "middle_click" => (MouseButton::Middle, 1),
                "double_click" => (MouseButton::Left, 2),
                _ => (MouseButton::Left, 3),
            };
            ComputerUseAction::Click {
                button,
                count,
                at: coord,
            }
        }
        "left_mouse_down" => {
            reject_unexpected(input, action_name, &[])?;
            ComputerUseAction::MouseDown {
                button: MouseButton::Left,
            }
        }
        "left_mouse_up" => {
            reject_unexpected(input, action_name, &[])?;
            ComputerUseAction::MouseUp {
                button: MouseButton::Left,
            }
        }
        "left_click_drag" => {
            reject_unexpected(input, action_name, &["start_x", "start_y", "x", "y"])?;
            let start_x = opt_i64(input, "start_x")?
                .ok_or_else(|| invalid("start_x is required for left_click_drag"))?;
            let start_y = opt_i64(input, "start_y")?
                .ok_or_else(|| invalid("start_y is required for left_click_drag"))?;
            let (x, y) =
                coord.ok_or_else(|| invalid("x and y are required for left_click_drag"))?;
            if start_x < 0 || start_y < 0 {
                return Err(invalid("coordinates must be non-negative"));
            }
            ComputerUseAction::Drag {
                start: (start_x, start_y),
                end: (x, y),
            }
        }
        "type" => {
            reject_unexpected(input, action_name, &["text"])?;
            ComputerUseAction::Type {
                text: req_text(input, action_name)?,
            }
        }
        "key" => {
            reject_unexpected(input, action_name, &["text"])?;
            let text = req_text(input, action_name)?;
            let keys = parse_key_chord(&text).map_err(invalid)?;
            ComputerUseAction::KeyChord { keys, chord: text }
        }
        "hold_key" => {
            reject_unexpected(input, action_name, &["text", "ms"])?;
            let text = req_text(input, action_name)?;
            let keys = parse_key_chord(&text).map_err(invalid)?;
            let ms = ms.ok_or_else(|| invalid("ms is required for hold_key"))?;
            if ms == 0 || ms > MAX_HOLD_KEY_MS {
                return Err(invalid(format!(
                    "ms must be in 1..={MAX_HOLD_KEY_MS} for hold_key"
                )));
            }
            ComputerUseAction::HoldKey {
                keys,
                chord: text,
                ms,
            }
        }
        other => {
            return Err(invalid(format!(
                "unknown action '{other}'; supported: screenshot, cursor_position, wait, ui_tree, element_at_point, mouse_move, scroll, left_click, right_click, middle_click, double_click, triple_click, left_mouse_down, left_mouse_up, left_click_drag, type, key, hold_key"
            )));
        }
    };
    Ok(ParsedCall { action, confirm_id })
}

// ---------------------------------------------------------------------------
// 执行
// ---------------------------------------------------------------------------

/// 截图产物（已落盘 PNG + 映射表）。
struct ShotOutcome {
    abs_path: PathBuf,
    rel_path: String,
    png: Vec<u8>,
    map: ScaleMap,
}

fn resolve_workspace(context: &ToolContext, session_id: &str) -> PathBuf {
    if context.workspace.as_os_str().is_empty() {
        crate::platform::paths::session_workspace_dir(session_id)
    } else {
        context.workspace.clone()
    }
}

fn capture_and_store(parts: &Parts, workspace: &Path) -> Result<ShotOutcome, ComputerUseError> {
    let capture = parts.backend.capture()?;
    let scaled: ScaledScreenshot = scaling::downscale_and_encode(&capture)?;
    let dir = workspace.join(ATTACHMENTS_DIR);
    std::fs::create_dir_all(&dir).map_err(|error| {
        ComputerUseError::failed(format!("cannot create {ATTACHMENTS_DIR}: {error}"))
    })?;
    let mut state = parts.state.lock();
    state.shot_seq += 1;
    let seq = state.shot_seq;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let file_name = format!("{stamp}-{seq:04}.png");
    let abs_path = dir.join(&file_name);
    std::fs::write(&abs_path, &scaled.png)
        .map_err(|error| ComputerUseError::failed(format!("cannot write screenshot: {error}")))?;
    let map = scaled.map;
    state.last_map = Some(map.clone());
    drop(state);
    Ok(ShotOutcome {
        abs_path,
        rel_path: format!("{ATTACHMENTS_DIR}/{file_name}"),
        png: scaled.png,
        map,
    })
}

fn shot_result_text(shot: &ShotOutcome) -> String {
    format!(
        "screenshot saved: {}\nimage: {}x{} px (scale {:.3} of device {}x{}, monitor origin ({}, {}))\n\
         All coordinates you pass to computer_use are in this image's pixel space, origin top-left.\n\
         If the image is not attached to this result, call image_analyze with image_path=\"{}\" to view it.",
        shot.abs_path.display(),
        shot.map.shot_w,
        shot.map.shot_h,
        shot.map.factor(),
        shot.map.dev_w,
        shot.map.dev_h,
        shot.map.origin_x,
        shot.map.origin_y,
        shot.rel_path,
    )
}

fn with_image_metadata(result: ToolResult, shot: &ShotOutcome) -> ToolResult {
    result.with_metadata(json!({
        "images": [shot.abs_path.to_string_lossy()]
    }))
}

fn backend_error_text(error: &ComputerUseError) -> String {
    format!("computer use action failed: {error}")
}

/// 解析带坐标动作的目标：钳制到截图范围（越界给警告不失败）→ 输入坐标。
fn resolve_targets(
    map: &ScaleMap,
    coords: &[(i64, i64)],
    warnings: &mut Vec<String>,
) -> Vec<(i32, i32)> {
    coords
        .iter()
        .map(|(x, y)| {
            let (cx, cy, clamped) = map.clamp_shot(*x, *y);
            if clamped {
                warnings.push(format!(
                    "coordinate ({x}, {y}) is outside the screenshot ({}x{}); clamped to ({cx}, {cy})",
                    map.shot_w, map.shot_h
                ));
            }
            map.shot_to_input(cx, cy)
        })
        .collect()
}

/// T3 后果性检测：点击/键盘动作前查目标（或光标）处的 a11y 元素，
/// 命中名单或密码字段即拦截。
struct T3Hit {
    element_label: String,
    reason: &'static str,
}

fn t3_check(parts: &Parts, action: &ComputerUseAction, map: Option<&ScaleMap>) -> Option<T3Hit> {
    let target_input: Option<(i32, i32)> = match action {
        ComputerUseAction::Click {
            at: Some((x, y)), ..
        }
        | ComputerUseAction::Scroll {
            at: Some((x, y)), ..
        } => map.map(|m| {
            let (cx, cy, _) = m.clamp_shot(*x, *y);
            m.shot_to_input(cx, cy)
        }),
        ComputerUseAction::ElementAtPoint { .. } | ComputerUseAction::MouseMove { .. } => None,
        _ => {
            // 无坐标输入动作：用当前光标位置。
            match parts.backend.cursor_position() {
                Ok((dx, dy)) => Some(match map {
                    Some(m) => m.device_to_input(dx, dy),
                    None => (dx, dy),
                }),
                Err(_) => None,
            }
        }
    };
    let (x, y) = target_input?;
    let element = parts.backend.element_at_point(x, y).ok().flatten()?;
    if element.secure || is_secure_role(&element.role) {
        return Some(T3Hit {
            element_label: format!("{} ({})", element.name, element.role),
            reason: "a password/secure field",
        });
    }
    if matches_t3_denylist(&element.name) || matches_t3_denylist(&element.role) {
        return Some(T3Hit {
            element_label: format!("{} ({})", element.name, element.role),
            reason: "a consequential control (purchase/payment/send/delete/transfer/submit)",
        });
    }
    None
}

fn requires_t3_check(action: &ComputerUseAction) -> bool {
    matches!(
        action,
        ComputerUseAction::Click { .. }
            | ComputerUseAction::Drag { .. }
            | ComputerUseAction::Type { .. }
            | ComputerUseAction::KeyChord { .. }
            | ComputerUseAction::HoldKey { .. }
    )
}

/// 动作是否附截图：截图/wait 总是；输入类与 scroll 执行后补拍。
fn attaches_screenshot(action: &ComputerUseAction) -> bool {
    matches!(
        action,
        ComputerUseAction::Screenshot | ComputerUseAction::Wait { .. }
    ) || action.class() == ActionClass::Input
        || matches!(action, ComputerUseAction::Scroll { .. })
}

fn consent_label(action: &ComputerUseAction, confirmed: bool) -> String {
    match action.class() {
        ActionClass::Observe => "observe".to_string(),
        ActionClass::Passive => "passive".to_string(),
        ActionClass::Input => {
            if confirmed {
                "input:session-grant+t3-confirmed".to_string()
            } else {
                "input:session-grant".to_string()
            }
        }
    }
}

fn run(parts: Parts, parsed: ParsedCall, workspace: PathBuf) -> ToolResult {
    let started = Instant::now();
    let action = parsed.action;
    let call_id = audit::new_call_id();
    let audit_log = AuditLog::for_session(&parts.session_id)
        .map_err(|error| {
            eprintln!("[computer_use] audit log unavailable: {error}");
            error
        })
        .ok();

    let mut record = AuditRecord::begin(
        call_id,
        parts.session_id.clone(),
        action.name(),
        action.class().as_str(),
        consent_label(&action, parsed.confirm_id.is_some()),
    );
    match &action {
        ComputerUseAction::Type { text } => {
            record.with_typed_text(text).with_target("keyboard focus");
        }
        ComputerUseAction::KeyChord { chord, .. } | ComputerUseAction::HoldKey { chord, .. } => {
            record.with_target(&format!("keys: {chord}"));
        }
        ComputerUseAction::Click { at, button, count } => {
            record.with_target(&format!("{} click x{count} at {at:?}", button.as_str()));
        }
        ComputerUseAction::Drag { start, end } => {
            record.with_target(&format!("drag {start:?} -> {end:?}"));
        }
        ComputerUseAction::MouseMove { x, y } | ComputerUseAction::ElementAtPoint { x, y } => {
            record.with_target(&format!("({x}, {y})"));
        }
        _ => {}
    }
    if let Some(log) = &audit_log {
        if let Err(error) = log.append(&record) {
            eprintln!("[computer_use] audit begin append failed: {error}");
        }
    }

    let mut warnings: Vec<String> = Vec::new();
    let mut shot: Option<ShotOutcome> = None;
    let mut confirmed_t3 = false;

    let body: Result<String, String> = (|| {
        // 带坐标动作没有 ScaleMap 时先自动截图（会话首次）。
        if action.needs_scale_map() && parts.state.lock().last_map.is_none() {
            let auto = capture_and_store(&parts, &workspace).map_err(|e| backend_error_text(&e))?;
            warnings.push(
                "no screenshot had been taken this session; one was captured automatically and is attached"
                    .to_string(),
            );
            shot = Some(auto);
        }

        // T3 确认令牌：模型回传 confirm_id 且状态里有批准令牌才放行。
        if requires_t3_check(&action) {
            let map = parts.state.lock().last_map.clone();
            let bypass = match &parsed.confirm_id {
                Some(id) => parts.shared.take_confirmation(id),
                None => false,
            };
            if parsed.confirm_id.is_some() && !bypass {
                return Err(
                    "the confirm_id is invalid or was already used. Ask the user to confirm again."
                        .to_string(),
                );
            }
            if !bypass {
                if let Some(hit) = t3_check(&parts, &action, map.as_ref()) {
                    let summary = action_summary(&action);
                    let confirm_id = parts.shared.new_pending_confirmation(
                        &parts.session_id,
                        summary.clone(),
                        hit.element_label.clone(),
                    );
                    parts.events.emit(
                        EVENT_CONFIRM_REQUIRED,
                        json!({
                            "session_id": parts.session_id,
                            "action": summary,
                            "element": hit.element_label,
                            "confirm_id": confirm_id,
                        }),
                    );
                    return Err(format!(
                        "this action targets {}: \"{}\". It was NOT executed. Ask the user to confirm in the app; then retry the same action with confirm_id=\"{confirm_id}\".",
                        hit.reason, hit.element_label
                    ));
                }
            } else {
                confirmed_t3 = true;
            }
        }

        // 停止旗标：注入前最后一刻检查。
        if action.class() == ActionClass::Input && parts.shared.is_stopped() {
            return Err(GuardRejection::Stopped.message());
        }

        // 能力检查。
        let capabilities = parts
            .backend
            .capabilities()
            .map_err(|e| backend_error_text(&e))?;
        match action.class() {
            ActionClass::Input if !capabilities.input => {
                return Err(format!(
                    "input injection is unsupported on this platform/session ({})",
                    capabilities.notes
                ));
            }
            ActionClass::Observe
                if matches!(
                    action,
                    ComputerUseAction::UiTree { .. } | ComputerUseAction::ElementAtPoint { .. }
                ) && !capabilities.ui_tree =>
            {
                return Err(format!(
                    "the accessibility tree is unsupported on this platform/session ({})",
                    capabilities.notes
                ));
            }
            _ => {}
        }

        // 执行动作。
        let map = parts.state.lock().last_map.clone();
        let mut outcome = execute_action(&parts, &action, map.as_ref(), &mut warnings)
            .map_err(|e| backend_error_text(&e))?;

        // 补拍截图。
        if attaches_screenshot(&action) {
            match &action {
                ComputerUseAction::Screenshot => {}
                ComputerUseAction::Wait { ms } => {
                    std::thread::sleep(std::time::Duration::from_millis(*ms));
                }
                _ => std::thread::sleep(std::time::Duration::from_millis(POST_ACTION_SETTLE_MS)),
            }
            match capture_and_store(&parts, &workspace) {
                Ok(fresh) => {
                    if !outcome.is_empty() {
                        outcome.push('\n');
                    }
                    outcome.push_str(&shot_result_text(&fresh));
                    shot = Some(fresh);
                }
                Err(error) => warnings.push(format!("post-action capture failed: {error}")),
            }
        }

        for warning in &warnings {
            outcome.push_str(&format!("\nwarning: {warning}"));
        }
        Ok(outcome)
    })();

    let duration_ms = started.elapsed().as_millis() as u64;
    if let Some(shot) = &shot {
        record.with_screenshot(&shot.png, &shot.abs_path);
    }

    match body {
        Ok(text) => {
            let mut result = ToolResult::success(text);
            if let Some(shot) = &shot {
                result = with_image_metadata(result, shot);
            }
            record.consent = consent_label(&action, confirmed_t3);
            let record = record.finish("ok", None, duration_ms);
            if let Some(log) = &audit_log {
                if let Err(error) = log.append(&record) {
                    eprintln!("[computer_use] audit end append failed: {error}");
                }
            }
            result
        }
        Err(message) => {
            let record = record.finish("error", Some(message.clone()), duration_ms);
            if let Some(log) = &audit_log {
                if let Err(error) = log.append(&record) {
                    eprintln!("[computer_use] audit end append failed: {error}");
                }
            }
            ToolResult::error(message)
        }
    }
}

fn action_summary(action: &ComputerUseAction) -> String {
    match action {
        ComputerUseAction::Click { button, count, at } => {
            format!("{} click x{count} at {at:?}", button.as_str())
        }
        ComputerUseAction::Drag { start, end } => format!("drag {start:?} -> {end:?}"),
        ComputerUseAction::Type { text } => format!("type {} chars", text.chars().count()),
        ComputerUseAction::KeyChord { chord, .. } => format!("key {chord}"),
        ComputerUseAction::HoldKey { chord, ms, .. } => format!("hold {chord} for {ms}ms"),
        other => other.name().to_string(),
    }
}

fn execute_action(
    parts: &Parts,
    action: &ComputerUseAction,
    map: Option<&ScaleMap>,
    warnings: &mut Vec<String>,
) -> Result<String, ComputerUseError> {
    let backend = &parts.backend;
    match action {
        ComputerUseAction::Screenshot => Ok(String::new()),
        ComputerUseAction::CursorPosition => {
            let (dx, dy) = backend.cursor_position()?;
            match map {
                Some(m) => {
                    let (sx, sy) = m.device_to_shot(dx, dy);
                    Ok(format!("cursor is at ({sx}, {sy}) in screenshot space"))
                }
                None => Ok(format!(
                    "cursor is at device position ({dx}, {dy}); no screenshot has been taken this session, so screenshot-space coordinates are unavailable"
                )),
            }
        }
        ComputerUseAction::Wait { ms } => Ok(format!("waited {ms} ms")),
        ComputerUseAction::UiTree { opts } => backend.ui_tree(*opts),
        ComputerUseAction::ElementAtPoint { x, y } => {
            let Some(m) = map else {
                return Err(ComputerUseError::failed(
                    "no screenshot has been taken this session; take one before element_at_point",
                ));
            };
            let targets = resolve_targets(m, &[(*x, *y)], warnings);
            let Some(&(ix, iy)) = targets.first() else {
                return Err(ComputerUseError::failed("missing element target"));
            };
            match backend.element_at_point(ix, iy)? {
                Some(element) => Ok(format!(
                    "element at ({x}, {y}): role=\"{}\" name=\"{}\" bounds=({}, {}, {}x{}) secure={}",
                    element.role,
                    element.name,
                    element.x,
                    element.y,
                    element.width,
                    element.height,
                    element.secure
                )),
                None => Ok(format!("no accessibility element found at ({x}, {y})")),
            }
        }
        ComputerUseAction::MouseMove { x, y } => {
            let Some(m) = map else {
                return Err(ComputerUseError::failed(
                    "no screenshot has been taken this session; take one before mouse_move",
                ));
            };
            let targets = resolve_targets(m, &[(*x, *y)], warnings);
            let Some(&(ix, iy)) = targets.first() else {
                return Err(ComputerUseError::failed("missing move target"));
            };
            backend.move_to(ix, iy)?;
            Ok(format!("mouse moved to ({x}, {y}) in screenshot space"))
        }
        ComputerUseAction::Scroll {
            direction,
            amount,
            at,
        } => {
            if let Some((x, y)) = at {
                let Some(m) = map else {
                    return Err(ComputerUseError::failed(
                        "no screenshot has been taken this session; take one before scroll with coordinates",
                    ));
                };
                let targets = resolve_targets(m, &[(*x, *y)], warnings);
                let Some(&(ix, iy)) = targets.first() else {
                    return Err(ComputerUseError::failed("missing scroll target"));
                };
                backend.move_to(ix, iy)?;
            }
            backend.scroll(*direction, *amount)?;
            Ok(format!(
                "scrolled {} {} click(s)",
                direction.as_str(),
                amount
            ))
        }
        ComputerUseAction::Click { button, count, at } => {
            if let Some((x, y)) = at {
                let Some(m) = map else {
                    return Err(ComputerUseError::failed(
                        "no screenshot has been taken this session; take one before clicking with coordinates",
                    ));
                };
                let targets = resolve_targets(m, &[(*x, *y)], warnings);
                let Some(&(ix, iy)) = targets.first() else {
                    return Err(ComputerUseError::failed("missing click target"));
                };
                backend.move_to(ix, iy)?;
            }
            backend.click(*button, *count)?;
            let verb = match count {
                2 => "double-",
                3 => "triple-",
                _ => "",
            };
            Ok(format!("{} {verb}click executed", button.as_str()))
        }
        ComputerUseAction::MouseDown { button } => {
            backend.mouse_down(*button)?;
            Ok(format!("{} mouse button is down", button.as_str()))
        }
        ComputerUseAction::MouseUp { button } => {
            backend.mouse_up(*button)?;
            Ok(format!("{} mouse button is up", button.as_str()))
        }
        ComputerUseAction::Drag { start, end } => {
            let Some(m) = map else {
                return Err(ComputerUseError::failed(
                    "no screenshot has been taken this session; take one before dragging",
                ));
            };
            let targets = resolve_targets(m, &[*start, *end], warnings);
            let (Some(&from), Some(&to)) = (targets.first(), targets.get(1)) else {
                return Err(ComputerUseError::failed("missing drag targets"));
            };
            backend.drag(from, to)?;
            Ok(format!("dragged from {start:?} to {end:?}"))
        }
        ComputerUseAction::Type { text } => {
            backend.type_text(text)?;
            Ok(format!("typed {} chars", text.chars().count()))
        }
        ComputerUseAction::KeyChord { keys, chord } => {
            backend.key_chord(keys)?;
            Ok(format!("pressed {chord}"))
        }
        ComputerUseAction::HoldKey { keys, chord, ms } => {
            backend.hold_key(keys, *ms)?;
            Ok(format!("held {chord} for {ms} ms"))
        }
    }
}

// ---------------------------------------------------------------------------
// ToolSpec
// ---------------------------------------------------------------------------

#[async_trait]
impl ToolSpec for ComputerUseTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn description(&self) -> &str {
        "Control the user's desktop: take screenshots, move/click the mouse, type text, \
         press key chords, scroll, and inspect the accessibility tree. \
         Coordinates are ALWAYS in the pixel space of the last screenshot this tool returned \
         (origin top-left); take a screenshot first and reuse its coordinate space. \
         Input actions (clicks, keys, typing) require the user's session grant; consequential \
         actions (purchase/payment/send/delete/submit controls, password fields) require explicit \
         user confirmation via confirm_id. After every action a fresh screenshot is attached; \
         if it is not visible, call image_analyze with the returned attachments path."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "screenshot", "cursor_position", "wait", "ui_tree", "element_at_point",
                        "mouse_move", "scroll",
                        "left_click", "right_click", "middle_click", "double_click", "triple_click",
                        "left_mouse_down", "left_mouse_up", "left_click_drag",
                        "type", "key", "hold_key"
                    ],
                    "description": "The computer action to perform"
                },
                "x": { "type": "integer", "minimum": 0, "description": "X coordinate in the last screenshot's pixel space" },
                "y": { "type": "integer", "minimum": 0, "description": "Y coordinate in the last screenshot's pixel space" },
                "start_x": { "type": "integer", "minimum": 0, "description": "Drag start X (left_click_drag only)" },
                "start_y": { "type": "integer", "minimum": 0, "description": "Drag start Y (left_click_drag only)" },
                "text": { "type": "string", "description": "Text to type (type) or xdotool-style key chord like \"ctrl+s\", \"Return\", \"alt+Tab\" (key, hold_key)" },
                "ms": { "type": "integer", "minimum": 0, "description": "Duration in milliseconds (wait, hold_key)" },
                "direction": { "type": "string", "enum": ["up", "down", "left", "right"], "description": "Scroll direction (scroll)" },
                "amount": { "type": "integer", "minimum": 0, "description": "Scroll wheel clicks (scroll)" },
                "max_depth": { "type": "integer", "minimum": 1, "description": "Max accessibility tree depth (ui_tree)" },
                "max_nodes": { "type": "integer", "minimum": 1, "description": "Max accessibility tree nodes (ui_tree)" },
                "confirm_id": { "type": "string", "description": "Single-use user-confirmation token for a blocked consequential action" }
            },
            "required": ["action"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::RequiresApproval]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        // 引擎当前忽略该元数据、同意门控在工具内部执行；标 Required 供未来引擎
        // 支持时组合。
        ApprovalRequirement::Required
    }

    fn supports_parallel(&self) -> bool {
        // 鼠标/键盘是全局独占资源：绝不与其他工具并行。
        false
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let parsed = parse_action(&input)?;

        // 同意门控（同步快速路径，拒绝时发事件）。
        let gate = match parsed.action.class() {
            ActionClass::Observe | ActionClass::Passive => self.parts.shared.check_readonly(),
            ActionClass::Input => self.parts.shared.begin_input_action(&self.parts.session_id),
        };
        if let Err(rejection) = gate {
            if rejection == GuardRejection::GrantRequired {
                self.parts.events.emit(
                    EVENT_GRANT_REQUIRED,
                    json!({ "session_id": self.parts.session_id }),
                );
            }
            // 审计被拒调用（best-effort）。
            let mut record = AuditRecord::begin(
                audit::new_call_id(),
                self.parts.session_id.clone(),
                parsed.action.name(),
                parsed.action.class().as_str(),
                format!("rejected:{}", rejection_name(rejection)),
            );
            if let Ok(log) = AuditLog::for_session(&self.parts.session_id) {
                let _ = log.append(&record);
                record = record.finish("rejected", Some(rejection.message()), 0);
                let _ = log.append(&record);
            }
            return Ok(ToolResult::error(rejection.message()));
        }

        let workspace = resolve_workspace(context, &self.parts.session_id);
        let parts = self.parts.clone();
        tauri::async_runtime::spawn_blocking(move || run(parts, parsed, workspace))
            .await
            .map_err(|error| {
                ToolError::execution_failed(format!("computer use worker join failed: {error}"))
            })
    }
}

fn rejection_name(rejection: GuardRejection) -> &'static str {
    match rejection {
        GuardRejection::Disabled => "disabled",
        GuardRejection::Stopped => "stopped",
        GuardRejection::GrantRequired => "grant-required",
        GuardRejection::RateLimited => "rate-limited",
        GuardRejection::BudgetExhausted => "budget-exhausted",
    }
}

#[cfg(test)]
mod tests;

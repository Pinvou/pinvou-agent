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
use super::guard::{
    ComputerUseShared, ConfirmationCheck, GuardRejection, is_secure_role, matches_t3_denylist,
};
use super::platform;
use super::scaling::{self, ScaleMap, ScaledScreenshot};
use super::types::{
    ActionClass, ComputerUseAction, ComputerUseError, EVENT_CONFIRM_REQUIRED, EVENT_GRANT_REQUIRED,
    ElementInfo, Key, MouseButton, ScrollDirection, TOOL_NAME, UiTreeOptions, parse_key_chord,
};

/// 输入/scroll 动作执行后的界面稳定等待（唯一的 settle 常数定义点）。
pub const POST_ACTION_SETTLE_MS: u64 = 350;
/// `wait` 上限 30 秒。
pub const MAX_WAIT_MS: u64 = 30_000;
/// `hold_key` 上限 30 秒。
pub const MAX_HOLD_KEY_MS: u64 = 30_000;
/// 单次 scroll 格数上限。
pub const MAX_SCROLL_AMOUNT: u32 = 100;
/// `type` 文本长度上限（字符数）。超限显式拒绝——失控的超长注入既会拖死
/// 输入循环，也会让确认摘要失去可读性。
pub const MAX_TYPE_TEXT_CHARS: usize = 10_000;
/// `ui_tree` 参数上限。
pub const MAX_UI_TREE_DEPTH: u32 = 64;
pub const MAX_UI_TREE_NODES: u32 = 10_000;

/// 截图存放的子目录（相对 workspace）：engine 的 image_analyze 回退按
/// workspace 相对路径解析，`attachments/` 是它的既定根。
const ATTACHMENTS_DIR: &str = "attachments/computer_use";

/// 工具 schema 暴露的动作全集。单一来源：schema 的 enum、未知动作的错误
/// 文案与 `parse_action` 的分发必须一致——parity 测试（tool/tests.rs）钉住
/// 三者，防止新增动作时只改一处。
pub const SUPPORTED_ACTIONS: &[&str] = &[
    "screenshot",
    "cursor_position",
    "wait",
    "ui_tree",
    "element_at_point",
    "mouse_move",
    "scroll",
    "left_click",
    "right_click",
    "middle_click",
    "double_click",
    "triple_click",
    "left_mouse_down",
    "left_mouse_up",
    "left_click_drag",
    "type",
    "key",
    "hold_key",
];

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
    /// 左键是否处于本工具按下的状态(down 成功置位、up/drag 成功清除)。
    /// 按住期间 mouse_move 实质是拖拽,落点必须过 T3 筛查(评审发现:
    /// down(无害)→move(不筛查)→up 的拆解可把文件拖进回收站)。
    mouse_buttons_held: bool,
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
        let backend = BackendHandle::lazy(platform::create_backend);
        // 登记句柄：revoke/stop/总开关关闭时命令层经 shared.backends 触发
        // 后端关闭持久 OS 级授权（Wayland portal 会话）；Drop 注销。
        shared.backends.insert(&session_id, backend.clone());
        Self {
            parts: Parts {
                session_id,
                shared,
                backend,
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
        shared.backends.insert(&session_id, backend.clone());
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

impl Drop for ComputerUseTool {
    fn drop(&mut self) {
        // 注销登记：否则登记表里的句柄会把 worker 线程（及其上的 portal
        // 会话）吊到进程退出。
        self.parts.shared.backends.remove(&self.parts.session_id);
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

/// `Option<u64>` 参数收窄到 `Option<u32>`：`opt_u64` 的 max 已把取值钉在
/// u32 范围内，这里仍用 `try_from` 显式拒绝而不是 `as u32` 静默截断
/// （评审发现：截断会让 max_nodes/max_depth 参数静默变成别的值）。
fn opt_u32_bounded(input: &Value, field: &str, max: u32) -> Result<Option<u32>, ToolError> {
    match opt_u64(input, field, u64::from(max))? {
        None => Ok(None),
        Some(value) => u32::try_from(value)
            .map(Some)
            .map_err(|_| invalid(format!("{field} out of range; got {value}"))),
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

/// 拒绝该动作不接受的字段（`action`/`confirm_id` 全局通用，不计）。显式
/// `null` 同样拒绝——schema 未定义的字段即使值为 null 也是模型幻觉/协议
/// 漂移的信号，静默放行会让 schema 漂移不可观测（评审发现）。
fn reject_unexpected(input: &Value, action: &str, allowed: &[&str]) -> Result<(), ToolError> {
    let Some(object) = input.as_object() else {
        return Err(invalid("input must be a JSON object"));
    };
    for key in object.keys() {
        if key == "action" || key == "confirm_id" || allowed.contains(&key.as_str()) {
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
            let max_depth = opt_u32_bounded(input, "max_depth", MAX_UI_TREE_DEPTH)?;
            let max_nodes = opt_u32_bounded(input, "max_nodes", MAX_UI_TREE_NODES)?;
            ComputerUseAction::UiTree {
                opts: UiTreeOptions {
                    max_depth,
                    max_nodes,
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
            // 下限 1：滚动 0 格是模型错误，显式拒绝而不是静默 no-op。
            if amount == 0 {
                return Err(invalid("amount must be >= 1 for scroll"));
            }
            if amount > u64::from(MAX_SCROLL_AMOUNT) {
                return Err(invalid(format!(
                    "amount must be <= {MAX_SCROLL_AMOUNT}; got {amount}"
                )));
            }
            ComputerUseAction::Scroll {
                direction,
                amount: u32::try_from(amount)
                    .map_err(|_| invalid("amount out of range for scroll"))?,
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
            let text = req_text(input, action_name)?;
            // NUL 无法有意义地键入，且会污染下游的长度/审计统计：显式拒绝。
            if text.contains('\0') {
                return Err(invalid("text must not contain NUL characters for type"));
            }
            let count = text.chars().count();
            if count > MAX_TYPE_TEXT_CHARS {
                return Err(invalid(format!(
                    "text is too long for type: {count} chars (max {MAX_TYPE_TEXT_CHARS}); \
                     split it into smaller chunks"
                )));
            }
            ComputerUseAction::Type { text }
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
                "unknown action '{other}'; supported: {}",
                SUPPORTED_ACTIONS.join(", ")
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
    // 私有文件基座落盘（评审发现：此前 `std::fs::write` 按 umask 默认权限
    // 落盘，屏幕内容可能含密码，而审计记录本身是 0600——隐私口径自相
    // 矛盾）。0700 目录 + 0600 文件（Windows 为 profile ACL 语义），
    // 前端 `openArtifactExternal` 同用户读取不受影响。
    let directory =
        crate::platform::filesystem::open_private_file_directory(&dir).map_err(|error| {
            ComputerUseError::failed(format!("cannot create {ATTACHMENTS_DIR}: {error}"))
        })?;
    let mut state = parts.state.lock();
    state.shot_seq += 1;
    let seq = state.shot_seq;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let file_name = format!("{stamp}-{seq:04}.png");
    let abs_path = dir.join(&file_name);
    directory
        .atomic_write_private_file(std::ffi::OsStr::new(&file_name), &scaled.png)
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

/// T3 筛查结论。`Unscreenable` 表示无法证明目标无害（a11y 查询故障、光标
/// 位置未知、缺截图映射等）——按**失败关闭**处理：与命中名单同样要求显式
/// 用户确认。旧实现把后端错误吞成「无元素」直接放行（评审发现），安全
/// 筛查绝不能 fail-open。
enum T3Screening {
    Clear,
    Blocked(T3Hit),
    Unscreenable(String),
}

struct T3Hit {
    element_label: String,
    reason: &'static str,
}

/// 对一个 a11y 元素做名单/密码字段判定。坐标筛查（screen_point）与键盘焦点
/// 筛查共用同一套判定，两条路径的安全标准必须一致。
fn screen_element(element: &ElementInfo) -> T3Screening {
    if element.secure || is_secure_role(&element.role) {
        return T3Screening::Blocked(T3Hit {
            element_label: format!("{} ({})", element.name, element.role),
            reason: "a password/secure field",
        });
    }
    if matches_t3_denylist(&element.name) || matches_t3_denylist(&element.role) {
        return T3Screening::Blocked(T3Hit {
            element_label: format!("{} ({})", element.name, element.role),
            reason: "a consequential control (purchase/payment/send/delete/transfer/submit)",
        });
    }
    T3Screening::Clear
}

/// 对一个输入坐标处的 a11y 元素做名单/密码字段筛查。
fn screen_point(parts: &Parts, x: i32, y: i32) -> T3Screening {
    let element = match parts.backend.element_at_point(x, y) {
        Ok(element) => element,
        Err(error) => {
            return T3Screening::Unscreenable(format!(
                "accessibility screening failed at ({x}, {y}): {error}"
            ));
        }
    };
    match element {
        Some(element) => screen_element(&element),
        None => T3Screening::Clear,
    }
}

/// 键盘和弦是否具有后果性语义：Delete/Backspace 与修饰键组合（cmd+delete
/// 删除文件、ctrl+w 丢失未保存工作）、Enter 与修饰键组合（cmd/ctrl+Enter
/// 发送/提交）。评审发现：焦点元素的 name 看不出按键会触发的后果——焦点
/// 在聊天框（name 为空）时 `key "cmd+Enter"` 直接发送消息，焦点元素筛查
/// 完全拦不住。普通 Enter/Delete（文本编辑最常用）不受影响。
/// Shift 只参与 Delete/Backspace 的判定：shift+delete/backspace 是绕过
/// 回收站永久删除级别的语义（评审发现：修饰集漏 Shift）；shift+Enter/
/// shift+字母 仍是换行、大写等键入形态，不升级为强制确认。
fn chord_is_consequential(keys: &[Key]) -> bool {
    let has_modifier = keys
        .iter()
        .any(|k| matches!(k, Key::Control | Key::Alt | Key::Meta));
    let has_shift = keys.iter().any(|k| matches!(k, Key::Shift));
    let destructive = keys
        .iter()
        .any(|k| matches!(k, Key::Delete | Key::Backspace));
    let enter = keys.iter().any(|k| matches!(k, Key::Enter));
    (destructive && (has_modifier || has_shift)) || (enter && has_modifier)
}

/// Type 动作的目标是否密码/安全字段（确认摘要据此掩码预览）。以焦点元素为
/// 准；焦点读不出时无法证明不是密码框，保守掩码（评审发现：确认摘要把键入
/// 文本前 12 字符明文送进确认弹窗/事件流，而「向密码框键入」恰是必然被拦
/// 走确认流程的场景）。
fn type_target_is_secure(parts: &Parts) -> bool {
    match parts.backend.focused_element() {
        Ok(Some(element)) => element.secure || is_secure_role(&element.role),
        // Ok(None)=无处键入（掩码与否无意义）；Err=查询失败，保守掩码。
        _ => true,
    }
}

/// T3 后果性筛查：Input 类动作执行前查目标处的 a11y 元素。
///
/// 筛查点选择：
/// - 带坐标的点击/滚动查目标点；`left_click_drag` 查**起点与落点**两个点
///   （拖进回收站/Delete 区是典型后果性动作，只查光标会漏掉终点——评审发现）。
/// - 键盘类动作（type/key/hold_key）查**焦点元素**：键盘输入落在焦点上而不
///   是光标处——焦点在密码框、光标在别处时按光标筛查会漏判放行（评审发现，
///   最重级别）。`focused_element` 返回 Ok(None)=明确无焦点=无处键入，放行；
///   Err=查询失败/平台不支持 → `Unscreenable` 失败关闭。后端不支持时默认
///   实现返回 Err，同样失败关闭。此外，破坏性组合键（见
///   [`chord_is_consequential`]）无论焦点为何都要求确认。
/// - mouse_move 在左键**未**按下时只悬停、不产生后果，明确不筛查（否则合法
///   hover 全被拦）；按住期间移动实质是拖拽，落点照常筛查（评审发现：
///   down(无害)→move(不筛查)→up 的拆解可零确认完成后果性拖拽）。
/// - 其余输入动作（mouse down/up、无坐标点击/滚动）作用于光标处，查当前
///   光标；光标必须落在截图显示器范围内（混合 DPI 防护：光标在另一块屏上
///   时换算结果是垃圾坐标，按 Unscreenable 失败关闭）。
///
/// 映射缺失（无截图）或光标未知时不再用设备像素硬猜坐标（评审发现：缩放
/// 屏上会查错位置静默放行），一律 `Unscreenable` 失败关闭。
fn t3_screening(parts: &Parts, action: &ComputerUseAction, map: Option<&ScaleMap>) -> T3Screening {
    const NO_MAP: &str = "no screenshot mapping is available to resolve the target point";
    match action {
        ComputerUseAction::ElementAtPoint { .. } => return T3Screening::Clear,
        ComputerUseAction::MouseMove { x, y } => {
            if !parts.state.lock().mouse_buttons_held {
                return T3Screening::Clear;
            }
            // 按住期间移动=拖拽：筛查落点。
            let Some(m) = map else {
                return T3Screening::Unscreenable(NO_MAP.to_string());
            };
            let (cx, cy, _) = m.clamp_shot(*x, *y);
            let (ix, iy) = m.shot_to_input(cx, cy);
            return screen_point(parts, ix, iy);
        }
        // 键盘类动作：筛查焦点元素（键盘输入的真正落点）+ 和弦语义。
        ComputerUseAction::KeyChord { keys, chord }
        | ComputerUseAction::HoldKey { keys, chord, .. }
            if chord_is_consequential(keys) =>
        {
            return T3Screening::Blocked(T3Hit {
                element_label: format!("key chord \"{chord}\""),
                reason: "a consequential key chord (destructive delete/send semantics on the focused control)",
            });
        }
        // 键盘类动作：筛查焦点元素（键盘输入的真正落点）。破坏性和弦已在
        // 上面的守卫臂拦截；余下的按焦点元素筛查。
        ComputerUseAction::Type { .. }
        | ComputerUseAction::KeyChord { .. }
        | ComputerUseAction::HoldKey { .. } => {
            return match parts.backend.focused_element() {
                Ok(Some(element)) => screen_element(&element),
                Ok(None) => T3Screening::Clear,
                Err(error) => T3Screening::Unscreenable(format!(
                    "the keyboard-focused element cannot be determined, so the typing target \
                     cannot be screened: {error}"
                )),
            };
        }
        _ => {}
    }
    let points: Vec<(i32, i32)> = match action {
        ComputerUseAction::Click {
            at: Some((x, y)), ..
        }
        | ComputerUseAction::Scroll {
            at: Some((x, y)), ..
        } => {
            let Some(m) = map else {
                return T3Screening::Unscreenable(NO_MAP.to_string());
            };
            let (cx, cy, _) = m.clamp_shot(*x, *y);
            vec![m.shot_to_input(cx, cy)]
        }
        ComputerUseAction::Drag { start, end } => {
            let Some(m) = map else {
                return T3Screening::Unscreenable(NO_MAP.to_string());
            };
            let (sx, sy, _) = m.clamp_shot(start.0, start.1);
            let (ex, ey, _) = m.clamp_shot(end.0, end.1);
            vec![m.shot_to_input(sx, sy), m.shot_to_input(ex, ey)]
        }
        _ => match parts.backend.cursor_position() {
            Ok((dx, dy)) => {
                let Some(m) = map else {
                    return T3Screening::Unscreenable(
                        "cursor position cannot be mapped into the input space without a \
                         screenshot"
                            .to_string(),
                    );
                };
                // 混合 DPI 防护：光标不在截图显示器上时 device_to_input 的
                // 结果对本次动作没有意义（如 mouse_down 在副屏、映射是主屏）。
                if !m.contains_device_point(dx, dy) {
                    return T3Screening::Unscreenable(format!(
                        "cursor at device ({dx}, {dy}) is outside the captured monitor \
                         (origin ({}, {}), {}x{}), so the target cannot be screened",
                        m.origin_x, m.origin_y, m.dev_w, m.dev_h
                    ));
                }
                vec![m.device_to_input(dx, dy)]
            }
            Err(error) => {
                return T3Screening::Unscreenable(format!(
                    "cursor position is unknown, so the target cannot be screened: {error}"
                ));
            }
        },
    };
    for (x, y) in points {
        match screen_point(parts, x, y) {
            T3Screening::Clear => {}
            other => return other,
        }
    }
    T3Screening::Clear
}

/// T3 筛查覆盖全部 Input 类动作——层级表以 [`ActionClass::class`] 为单一
/// 来源（评审发现：旧版手抄清单漏掉 MouseDown/Up，点击可被拆解绕过）。
fn requires_t3_check(action: &ComputerUseAction) -> bool {
    action.class() == ActionClass::Input
}

/// 铸造待确认请求、发事件并给模型返回「未执行、去要确认」错误。
fn request_confirmation(
    parts: &Parts,
    summary: &str,
    element_label: &str,
    reason_phrase: &str,
) -> String {
    // 确认队列满（大量未决确认）时拒绝并给出可操作的错误——评审发现：旧的
    // 「逐出最旧 pending」会把用户正在等待的确认弹窗挤掉。
    let Some(confirm_id) = parts.shared.new_pending_confirmation(
        &parts.session_id,
        summary.to_string(),
        element_label.to_string(),
    ) else {
        return "the confirmation queue is full (too many unresolved confirmations). \
                Do not spam further blocked actions; wait for the user to respond to the \
                pending prompts first."
            .to_string();
    };
    parts.events.emit(
        EVENT_CONFIRM_REQUIRED,
        json!({
            "session_id": parts.session_id,
            "action": summary,
            "element": element_label,
            "confirm_id": confirm_id,
        }),
    );
    format!(
        "this action targets {reason_phrase}: \"{element_label}\". It was NOT executed. \
         Ask the user to confirm in the app; then retry the same action with \
         confirm_id=\"{confirm_id}\"."
    )
}

/// 动作是否附截图（单一事实来源在 [`ComputerUseAction::attaches_screenshot`]）。
fn attaches_screenshot(action: &ComputerUseAction) -> bool {
    action.attaches_screenshot()
}

fn consent_label(action: &ComputerUseAction, confirmed: bool) -> String {
    match action.class() {
        ActionClass::Observe => "observe".to_string(),
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
    // 审计 fail-closed（评审发现：Input 类动作曾可静默无审计执行）。审计
    // 目录不可用时：Input 类拒绝执行；Observe 类维持 eprintln 降级（只读
    // 观察没有注入后果，可用性优先）。
    let audit_log = match AuditLog::for_session(&parts.session_id) {
        Ok(log) => Some(log),
        Err(error) => {
            eprintln!("[computer_use] audit log unavailable: {error}");
            if action.class() == ActionClass::Input {
                return ToolResult::error(format!(
                    "refusing to execute an input action because the audit log is \
                     unavailable: {error}"
                ));
            }
            None
        }
    };

    // begin 记录的 consent 标签保持中性：此刻令牌尚未验证（评审发现：带
    // confirm_id 的调用曾被预标成 t3-confirmed，伪造 id 会留下失实审计）。
    let mut record = AuditRecord::begin(
        call_id,
        parts.session_id.clone(),
        action.name(),
        action.class().as_str(),
        consent_label(&action, false),
    );
    match &action {
        ComputerUseAction::Type { text } => {
            record.with_typed_text(text).with_target("keyboard focus");
        }
        // 评审发现：`key "p"` 逐字符调用可把密码明文写进审计。仅由单个
        // Char 组成（无修饰键）的和弦实质是键入文本，与 type 同策略——
        // 只记长度 + salt || text 的 HMAC；含修饰键的和弦是快捷键，无
        // 字典风险，保留明文（用户可读性优先）。例外：shift+字符 是
        // 大写字母/符号的键入形态而非快捷键（评审发现：`key "shift+H"`
        // 逐字符键入可绕开 HMAC 策略拼出明文），同样只记 HMAC；解析保留
        // 模型给定的顺序，两种顺序都要覆盖。
        ComputerUseAction::KeyChord { keys, chord }
        | ComputerUseAction::HoldKey { keys, chord, .. }
            if matches!(
                keys.as_slice(),
                [Key::Char(_)] | [Key::Shift, Key::Char(_)] | [Key::Char(_), Key::Shift]
            ) =>
        {
            record.with_typed_text(chord).with_target("keyboard focus");
        }
        ComputerUseAction::KeyChord { chord, .. } => {
            record.with_target(&format!("keys: {chord}"));
        }
        // hold_key 记录按住时长（评审发现：审计此前答不了"按了多久"）。
        ComputerUseAction::HoldKey { chord, ms, .. } => {
            record.with_target(&format!("keys: {chord} held for {ms}ms"));
        }
        ComputerUseAction::Click { at, button, count } => {
            record.with_target(&format!("{} click x{count} at {at:?}", button.as_str()));
        }
        // 滚动/按下/释放此前不记参数（评审发现：审计答不了"滚了哪个方向
        // 几格"）。
        ComputerUseAction::Scroll {
            direction,
            amount,
            at,
        } => {
            record.with_target(&format!(
                "scroll {} x{amount} at {at:?}",
                direction.as_str()
            ));
        }
        ComputerUseAction::MouseDown { button } => {
            record.with_target(&format!("{} mouse button down", button.as_str()));
        }
        ComputerUseAction::MouseUp { button } => {
            record.with_target(&format!("{} mouse button up", button.as_str()));
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
            if action.class() == ActionClass::Input {
                return ToolResult::error(format!(
                    "refusing to execute an input action because its audit record \
                     could not be written: {error}"
                ));
            }
        }
    }

    let mut warnings: Vec<String> = Vec::new();
    let mut shot: Option<ShotOutcome> = None;
    let mut confirmed_t3 = false;

    let body: Result<String, String> = (|| {
        // 物理输入是全局独占资源：Input 类动作从筛查到注入全程持有进程级
        // 互斥，两个并发会话不能交替打字/点击（评审发现）。获取改为有界等待：
        // 锁被其他会话持有时超时显式报错（InputBusy），而不是无限挂等把其他
        // 会话静默卡死（评审发现）。
        let _input_guard = (action.class() == ActionClass::Input)
            .then(|| parts.shared.lock_physical_input())
            .transpose()
            .map_err(|rejection| rejection.message())?;

        // 带坐标动作没有 ScaleMap 时先自动截图（会话首次）；T3 动作同样需要
        // 映射——筛查点的坐标换算和光标映射都依赖它。
        if (action.needs_scale_map() || requires_t3_check(&action))
            && parts.state.lock().last_map.is_none()
        {
            let auto = capture_and_store(&parts, &workspace).map_err(|e| backend_error_text(&e))?;
            warnings.push(
                "no screenshot had been taken this session; one was captured automatically and is attached"
                    .to_string(),
            );
            shot = Some(auto);
        }

        // T3 确认令牌：模型回传 confirm_id 且状态里有与之匹配（同会话、同
        // 动作摘要）的批准令牌才放行。
        if requires_t3_check(&action) {
            // 摘要绑定输入：
            // - cursor：无坐标 click/down/up/scroll 的落点是执行时刻的光标，
            //   静态参数绑不住——把铸造/消费时刻的光标位置写进摘要，模型把
            //   光标移到别的目标上再花令牌时摘要失配被拒（评审发现：用户
            //   批准的落点与实际执行的落点可能不同）。
            // - secure_type_target：Type 的目标是密码框时掩码预览（铸造与
            //   消费都按当时焦点重算，焦点不变即确定一致）。
            let cursor = parts.backend.cursor_position().ok();
            let secure_type_target =
                matches!(&action, ComputerUseAction::Type { .. }) && type_target_is_secure(&parts);
            let summary = action_summary(&action, cursor, secure_type_target);
            let bypass = match &parsed.confirm_id {
                Some(id) => Some(
                    parts
                        .shared
                        .take_confirmation(id, &parts.session_id, &summary),
                ),
                None => None,
            };
            match bypass {
                Some(ConfirmationCheck::Granted {
                    approved_element_label,
                }) => {
                    // 已获批准不等于可以免检执行：批准到重试之间隔着任意
                    // 模型调用（可能已重新截图、焦点已移动），摘要绑定的
                    // 静态参数挡不住"目标处的东西变了"（评审发现：批准会
                    // 被静默改指向）。重筛一次：
                    // - Clear / 不可筛 → 放行（后者用户本就是在知情下批准）；
                    // - 命中后果性名单 → 按用户批准时看到的元素标签绑定：
                    //   眼前的目标还是批准的那个（标签一致）→ 放行；换了
                    //   内容（标签不一致）→ 作废本次批准、要求重新确认
                    //   （令牌单次有效，已消费）。
                    let map = parts.state.lock().last_map.clone();
                    match t3_screening(&parts, &action, map.as_ref()) {
                        T3Screening::Clear => confirmed_t3 = true,
                        T3Screening::Unscreenable(_) => confirmed_t3 = true,
                        T3Screening::Blocked(hit) => {
                            if hit.element_label == approved_element_label {
                                confirmed_t3 = true;
                            } else {
                                return Err(request_confirmation(
                                    &parts,
                                    &summary,
                                    &hit.element_label,
                                    &hit.reason,
                                ));
                            }
                        }
                    }
                }
                Some(ConfirmationCheck::Denied) => {
                    return Err(
                        "the user denied this action. Do not retry it; ask the user how to \
                         proceed."
                            .to_string(),
                    );
                }
                Some(ConfirmationCheck::Unknown) => {
                    return Err(
                        "the confirm_id is invalid, expired, or was already used. Ask the \
                         user to confirm again."
                            .to_string(),
                    );
                }
                None => {
                    let map = parts.state.lock().last_map.clone();
                    match t3_screening(&parts, &action, map.as_ref()) {
                        T3Screening::Clear => {}
                        T3Screening::Blocked(hit) => {
                            return Err(request_confirmation(
                                &parts,
                                &summary,
                                &hit.element_label,
                                hit.reason,
                            ));
                        }
                        T3Screening::Unscreenable(reason) => {
                            // 失败关闭：无法证明目标无害时同样要求用户确认。
                            return Err(request_confirmation(
                                &parts,
                                &summary,
                                &reason,
                                "an unverifiable target (screening unavailable)",
                            ));
                        }
                    }
                }
            }
        }

        // 注入前最后一刻只读复检（停止旗标 + 授权仍有效）：gate 之后可能隔
        // 着自动截图等耗时步骤，期间用户可能 revoke/stop（评审发现）。
        if action.class() == ActionClass::Input {
            if let Err(rejection) = parts.shared.verify_input_action(&parts.session_id) {
                return Err(rejection.message());
            }
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

    // 审计 end 记录失败时动作已执行、无法撤销，但绝不能静默吞掉（评审发现
    // 的 `let _ = append` 路径）：Input 类把失败显式带回给模型与用户。
    let audit_end_failure = |log: &AuditLog, record: &AuditRecord| {
        log.append(record).err().map(|error| {
            eprintln!("[computer_use] audit end append failed: {error}");
            format!("the audit record for this action could not be written: {error}")
        })
    };

    match body {
        Ok(text) => {
            let mut result = ToolResult::success(text);
            if let Some(shot) = &shot {
                result = with_image_metadata(result, shot);
            }
            record.consent = consent_label(&action, confirmed_t3);
            let record = record.finish("ok", None, duration_ms);
            if let Some(log) = &audit_log {
                if let Some(audit_error) = audit_end_failure(log, &record) {
                    if action.class() == ActionClass::Input {
                        return ToolResult::error(format!(
                            "the action was executed, but {audit_error}; \
                             treat this action as unverified"
                        ));
                    }
                }
            }
            result
        }
        Err(message) => {
            let record = record.finish("error", Some(message.clone()), duration_ms);
            if let Some(log) = &audit_log {
                if let Some(audit_error) = audit_end_failure(log, &record) {
                    if action.class() == ActionClass::Input {
                        return ToolResult::error(format!(
                            "{message}; additionally, {audit_error}"
                        ));
                    }
                }
            }
            ToolResult::error(message)
        }
    }
}

/// Type 摘要预览的最大字符数。
const TYPE_PREVIEW_CHARS: usize = 12;

/// Type 摘要的安全预览：最多 [`TYPE_PREVIEW_CHARS`] 个字符，控制字符转义为
/// `\uXXXX`（预览会进确认对话框/事件与模型结果，不可夹带不可见载荷），
/// 超长截断加省略号。
fn type_preview(text: &str) -> String {
    let mut preview = String::new();
    let mut chars = text.chars();
    for _ in 0..TYPE_PREVIEW_CHARS {
        match chars.next() {
            None => return preview,
            Some(c) if c.is_control() => preview.push_str(&format!("\\u{:04x}", c as u32)),
            Some(c) => preview.push(c),
        }
    }
    if chars.next().is_some() {
        preview.push('…');
    }
    preview
}

/// 键入文本的全文指纹：SHA-256 前 16 个 hex（64 位）。确认令牌与摘要绑定，
/// 文本一变指纹即变——同长度的另一段文本不能换用同一令牌。评审发现：8 个
/// hex（32 位）对有代码执行能力的对抗模型可离线暴力碰撞（同长度+同前缀+
/// 同指纹的恶意文本分钟级可得），摘要又是字符串全等比较，故加倍到 64 位
/// （2^64 次SHA-256 在会话时间尺度内不可行）。
fn text_fingerprint(text: &str) -> String {
    audit::sha256_hex(text.as_bytes())[..16].to_string()
}

/// Type 动作摘要：字符数 + 预览 + 指纹。摘要会展示给用户（知情批准）并
/// 参与 guard 令牌绑定——旧摘要只有 `type N chars`，用户盲批且同长度不同
/// 文本可换用同一令牌（评审发现）。`secure_target` 为真时预览掩码：向密码
/// 框键入恰是必然走确认流程的场景，明文预览会把密码前 12 字符送进确认
/// 弹窗/事件流（评审发现）。
fn typed_text_summary(verb: &str, text: &str, secure_target: bool) -> String {
    let preview = if secure_target {
        "<masked: the typing target is a password/secure field>".to_string()
    } else {
        type_preview(text)
    };
    format!(
        "{verb} {} chars: \"{}\" [{}]",
        text.chars().count(),
        preview,
        text_fingerprint(text)
    )
}

/// 无坐标点击/down/up 的落点绑定后缀：这类动作的落点 = 执行时刻的光标，
/// 摘要绑不住静态参数，把**铸造时的光标位置**写进摘要——消费时按当时光标
/// 重建摘要，光标被移到别的目标上时摘要失配、令牌被拒（评审发现：用户批准
/// 的落点与实际执行的落点可能不同）。None=光标读不出（保守绑定，消费时
/// 除非同样读不出否则不放行）。
fn cursor_binding_suffix(cursor: Option<(i32, i32)>) -> String {
    format!(" at cursor {cursor:?}")
}

fn action_summary(
    action: &ComputerUseAction,
    cursor: Option<(i32, i32)>,
    secure_type_target: bool,
) -> String {
    match action {
        ComputerUseAction::Click { button, count, at } => {
            let mut summary = format!("{} click x{count} at {at:?}", button.as_str());
            if at.is_none() {
                summary.push_str(&cursor_binding_suffix(cursor));
            }
            summary
        }
        ComputerUseAction::Drag { start, end } => format!("drag {start:?} -> {end:?}"),
        ComputerUseAction::Type { text } => typed_text_summary("type", text, secure_type_target),
        ComputerUseAction::KeyChord { chord, .. } => format!("key {chord}"),
        ComputerUseAction::HoldKey { chord, ms, .. } => format!("hold {chord} for {ms}ms"),
        ComputerUseAction::Scroll {
            direction,
            amount,
            at,
        } => {
            let mut summary = format!("scroll {} x{amount} at {at:?}", direction.as_str());
            if at.is_none() {
                summary.push_str(&cursor_binding_suffix(cursor));
            }
            summary
        }
        ComputerUseAction::MouseDown { button } => {
            format!(
                "{} mouse down{}",
                button.as_str(),
                cursor_binding_suffix(cursor)
            )
        }
        ComputerUseAction::MouseUp { button } => {
            format!(
                "{} mouse up{}",
                button.as_str(),
                cursor_binding_suffix(cursor)
            )
        }
        // mouse_move 带静态坐标，必须绑定进摘要（评审发现：兜底分支只留
        // 动作名，确认弹窗盲批 "mouse_move"，批准后带任意坐标重试即可
        // 重建相同摘要）。
        ComputerUseAction::MouseMove { x, y } => format!("mouse_move to ({x}, {y})"),
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
                    // 混合 DPI 防护：光标不在截图显示器上时不硬换算（换算结果
                    // 是跨屏垃圾坐标），回报设备坐标并附警告（评审发现）。
                    if !m.contains_device_point(dx, dy) {
                        warnings.push(format!(
                            "cursor is outside the captured monitor (origin ({}, {}), {}x{}); \
                             screenshot-space coordinates are unavailable for it this turn",
                            m.origin_x, m.origin_y, m.dev_w, m.dev_h
                        ));
                        return Ok(format!(
                            "cursor is at device position ({dx}, {dy}), which is outside the \
                             captured monitor"
                        ));
                    }
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
            // 按住状态供 mouse_move 的拖拽筛查使用（失败路径不置位：按下
            // 失败=物理上没有按住）。
            if *button == MouseButton::Left {
                parts.state.lock().mouse_buttons_held = true;
            }
            Ok(format!("{} mouse button is down", button.as_str()))
        }
        ComputerUseAction::MouseUp { button } => {
            backend.mouse_up(*button)?;
            if *button == MouseButton::Left {
                parts.state.lock().mouse_buttons_held = false;
            }
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
            // 拖拽内部完成按下+释放：无论此前状态如何，左键已不在按下态。
            parts.state.lock().mouse_buttons_held = false;
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
         Every action that touches the mouse or keyboard (mouse_move, scroll, clicks, keys, \
         typing) requires the user's session grant; consequential actions (purchase/payment/ \
         send/delete/submit controls, password fields) additionally require explicit user \
         confirmation via confirm_id. After actions that change the screen a fresh screenshot \
         is attached; if it is not visible, call image_analyze with the returned attachments \
         path."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": SUPPORTED_ACTIONS,
                    "description": "The computer action to perform"
                },
                "x": { "type": "integer", "minimum": 0, "description": "X coordinate in the last screenshot's pixel space" },
                "y": { "type": "integer", "minimum": 0, "description": "Y coordinate in the last screenshot's pixel space" },
                "start_x": { "type": "integer", "minimum": 0, "description": "Drag start X (left_click_drag only)" },
                "start_y": { "type": "integer", "minimum": 0, "description": "Drag start Y (left_click_drag only)" },
                "text": { "type": "string", "description": "Text to type (type) or xdotool-style key chord like \"ctrl+s\", \"Return\", \"alt+Tab\" (key, hold_key)" },
                "ms": { "type": "integer", "minimum": 0, "description": "Duration in milliseconds (wait, hold_key)" },
                "direction": { "type": "string", "enum": ["up", "down", "left", "right"], "description": "Scroll direction (scroll)" },
                "amount": { "type": "integer", "minimum": 1, "description": "Scroll wheel clicks, 1-100 (scroll)" },
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

        // 能力先行（评审发现：先弹授权再报不支持，会诱导用户为一个永远无法
        // 使用的平台授权，还白白消耗一次授权预算）。
        if parsed.action.class() == ActionClass::Input {
            let backend = self.parts.backend.clone();
            let capabilities = tauri::async_runtime::spawn_blocking(move || backend.capabilities())
                .await
                .map_err(|error| {
                    ToolError::execution_failed(format!("computer use worker join failed: {error}"))
                })?
                .map_err(|error| ToolError::execution_failed(backend_error_text(&error)))?;
            if !capabilities.input {
                // 审计被拒调用（评审发现：能力拒绝此前无任何痕迹，与 gate
                // 拒绝的既有审计口径不一致）。
                audit_rejected_call(
                    &self.parts,
                    &parsed.action,
                    "capability:input-unsupported",
                    &format!(
                        "input injection is unsupported on this platform/session ({})",
                        capabilities.notes
                    ),
                );
                return Ok(ToolResult::error(format!(
                    "input injection is unsupported on this platform/session ({})",
                    capabilities.notes
                )));
            }
        }

        // 同意门控（同步快速路径，拒绝时发事件）。观察类动作同样过限速记账
        // （评审发现：screenshot/ui_tree/cursor 此前完全不限速）。
        let gate = match parsed.action.class() {
            ActionClass::Observe => self
                .parts
                .shared
                .begin_observe_action(&self.parts.session_id),
            ActionClass::Input => self.parts.shared.begin_input_action(&self.parts.session_id),
        };
        if let Err(rejection) = gate {
            if rejection == GuardRejection::GrantRequired {
                self.parts.events.emit(
                    EVENT_GRANT_REQUIRED,
                    json!({ "session_id": self.parts.session_id }),
                );
            }
            // 审计被拒调用（非输入类允许降级为 eprintln；动作未执行，无注入
            // 后果，但绝不静默吞错——评审发现）。
            audit_rejected_call(
                &self.parts,
                &parsed.action,
                rejection_name(rejection),
                &rejection.message(),
            );
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
        GuardRejection::ObserveRateLimited => "observe-rate-limited",
        GuardRejection::BudgetExhausted => "budget-exhausted",
        GuardRejection::InputBusy => "input-busy",
    }
}

/// 被拒调用（gate 拒绝 / 能力拒绝）的审计：动作未执行、无注入后果，但
/// 绝不静默吞错（非输入类允许降级为 eprintln）。
fn audit_rejected_call(parts: &Parts, action: &ComputerUseAction, reason: &str, message: &str) {
    let mut record = AuditRecord::begin(
        audit::new_call_id(),
        parts.session_id.clone(),
        action.name(),
        action.class().as_str(),
        format!("rejected:{reason}"),
    );
    match AuditLog::for_session(&parts.session_id) {
        Ok(log) => {
            if let Err(error) = log.append(&record) {
                eprintln!("[computer_use] rejected-call audit append failed: {error}");
            }
            record = record.finish("rejected", Some(message.to_string()), 0);
            if let Err(error) = log.append(&record) {
                eprintln!("[computer_use] rejected-call audit append failed: {error}");
            }
        }
        Err(error) => {
            eprintln!("[computer_use] audit log unavailable for rejected call: {error}");
        }
    }
}

#[cfg(test)]
mod tests;

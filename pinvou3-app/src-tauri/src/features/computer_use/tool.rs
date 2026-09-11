//! `computer_use` 工具：单一 ToolSpec，`action` 字段区分动作
//! （Anthropic computer_20250124 动作集 + a11y 扩展 ui_tree / element_at_point）。
//!
//! 关键不变量：
//! - 模型坐标永远是「截图空间」（最近一次返回 PNG 的像素，原点左上）。
//! - 每次截图生成新 ScaleMap 存入会话状态；带坐标的动作没有 ScaleMap 时先自动截图。
//! - 输入类与 scroll 动作执行后等待 [`POST_ACTION_SETTLE_MS`] 再补拍截图附上，
//!   模型始终看到最新状态。
//! - 同意门控（[`ComputerUseShared`]）在每次注入前检查；命中后果性名单或
//!   密码字段的目标必须经用户确认（单次令牌，绑定动作摘要）。筛查是尽力
//!   而为的类别检测：筛查不可用不阻断执行，键入内容永不筛查。

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

use super::audit::{AuditLog, AuditRecord};
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

/// Stable audit code for the T3 "confirmation required" error. The full
/// error message carries the element label (on-screen content may be
/// sensitive), so the audit record's error field holds only this code; the
/// model still receives the full message verbatim.
const T3_CONFIRM_REQUIRED_ERROR: &str = "t3-confirmation-required";

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
    /// 本工具是否物理按下了左键（down 成功置位、up/drag 成功清除）。唯一
    /// 用途是 `left_click_drag` 失败后的兜底松键：只释放**本工具自己按下
    /// 的**键，绝不松用户物理按住的键。不再作为筛查输入（按住期间的
    /// mouse_move 与其他 mouse_move 一样不设确认，主流无 held-move 复筛）。
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
        // Emergency release (it also unregisters): a model that pressed the
        // left button and was dropped mid-drag must not leave the user's
        // machine in button-held drag state, and the persistent OS-level
        // grant (Wayland portal session) must close with the tool. The
        // detached thread keeps its own handle clone, so the worker lives
        // until the cleanup finishes and only then is torn down.
        self.parts
            .shared
            .backends
            .emergency_release(&self.parts.session_id);
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

/// T3 筛查结论。筛查是尽力而为的类别检测：**无法**筛查（a11y 查询故障、
/// 光标未知、缺截图映射、光标在截图显示器之外等）一律按 Clear 放行——
/// 没有主流产品为筛查基础设施故障单独索要确认，fail-closed 只会在
/// AT-SPI 不可用、多屏等场景制造确认风暴；只有**正面命中**名单/密码
/// 字段才要求确认。
enum T3Screening {
    Clear,
    Blocked(T3Hit),
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
            reason: "a consequential control (financial/send/delete/submit/consent)",
        });
    }
    T3Screening::Clear
}

/// 对一个输入坐标处的 a11y 元素做名单/密码字段筛查。
fn screen_point(parts: &Parts, x: i32, y: i32) -> T3Screening {
    let element = match parts.backend.element_at_point(x, y) {
        Ok(element) => element,
        // 筛查不可用（a11y 查询故障）≠ 命中名单：筛查是尽力而为的类别检测，
        // 没有主流产品为筛查基础设施故障单独索要确认（AT-SPI 不可用、多屏
        // 等场景下 fail-closed 只会制造确认风暴）。放行执行。
        Err(_) => return T3Screening::Clear,
    };
    match element {
        Some(element) => screen_element(&element),
        // No element at the target point = nothing to check (Clear): unnamed
        // targets are everywhere on real desktops (canvas, hover targets,
        // custom widgets) and the denylist is name-based, so an absent name
        // is not a red flag.
        None => T3Screening::Clear,
    }
}

/// Whether a key chord is effectively typed text: exactly one character key
/// with any modifiers. Classification must be content-based, not
/// token-position based — `shift+shift+h` parses to
/// `[Shift, Shift, Char('h')]`, which positional patterns like
/// `[Char] | [Shift, Char] | [Char, Shift]` miss while the injection still
/// types a capital H; `h+shift+shift` types a lowercase h and is a typing
/// form all the same. Modifier choice does not make a character chord a
/// shortcut: a password spelled one `alt+x` call at a time would otherwise
/// land in the audit log as plaintext `keys: alt+x` records (round-6
/// review), so every single-character chord logs the count only.
fn is_typed_text_chord(keys: &[Key]) -> bool {
    let mut non_modifiers = keys.iter().filter(|k| !k.is_modifier());
    let (Some(only), None) = (non_modifiers.next(), non_modifiers.next()) else {
        return false;
    };
    matches!(only, Key::Char(_))
}

/// Type 动作的目标是否密码/安全字段（确认摘要据此掩码预览）。以焦点元素为
/// 准；只有**正面**命中密码/安全角色才算密码框——焦点读不出（Ok(None)=
/// 无处键入、Err=查询失败）不构成信号，按非密码处理（筛查不可用既不阻断
/// 执行，也不改变摘要形态；命中密码角色仍会走确认，见 `t3_screening`）。
fn type_target_is_secure(parts: &Parts) -> bool {
    match parts.backend.focused_element() {
        Ok(Some(element)) => element.secure || is_secure_role(&element.role),
        _ => false,
    }
}

/// T3 后果性筛查：只对**激活类** Input 动作执行（见 [`requires_t3_check`]）。
///
/// 筛查点选择：
/// - 带坐标的点击查目标点；`left_click_drag` 查**起点与落点**两个点（拖进
///   回收站/Delete 区是典型后果性动作，只查光标会漏掉终点）。
/// - 键盘类动作（type/key/hold_key）查**焦点元素**：键盘输入落在焦点上而
///   不是光标处——焦点在密码框、光标在别处时按光标筛查会漏判放行（评审
///   发现，最重级别）。`focused_element` 返回 Ok(None)=明确无焦点=无处键入，
///   放行；Err=查询失败/平台不支持，同样放行（筛查不可用不阻断执行）。
///   组合键不再设破坏性确认——和弦编辑可逆，没有主流产品按和弦形态设门；
///   焦点元素的名单/密码筛查照常生效。
/// - mouse down/up、无坐标点击作用于光标处，查当前光标；光标必须落在截图
///   显示器范围内才换算（混合 DPI：光标在另一块屏上时换算结果是垃圾坐标，
///   查了只会筛错点）；范围外或光标未知时无目标可查，放行。
/// - mouse_move 与 scroll 根本不进入本函数（悬停/滚动无动作后果，主流对
///   hover/scroll 一律不设门）。
///
/// 所有「筛查不可用」路径一律 Clear（best-effort 类别检测：正面命中才确认，
/// 筛查基础设施故障不制造确认风暴）。
fn t3_screening(parts: &Parts, action: &ComputerUseAction, map: Option<&ScaleMap>) -> T3Screening {
    // 键盘类动作：筛查焦点元素（键盘输入的真正落点）。
    if matches!(
        action,
        ComputerUseAction::Type { .. }
            | ComputerUseAction::KeyChord { .. }
            | ComputerUseAction::HoldKey { .. }
    ) {
        return match parts.backend.focused_element() {
            Ok(Some(element)) => screen_element(&element),
            Ok(None) | Err(_) => T3Screening::Clear,
        };
    }
    let points: Vec<(i32, i32)> = match action {
        ComputerUseAction::Click {
            at: Some((x, y)), ..
        } => {
            let Some(m) = map else {
                return T3Screening::Clear;
            };
            let (cx, cy, _) = m.clamp_shot(*x, *y);
            vec![m.shot_to_input(cx, cy)]
        }
        ComputerUseAction::Drag { start, end } => {
            let Some(m) = map else {
                return T3Screening::Clear;
            };
            let (sx, sy, _) = m.clamp_shot(start.0, start.1);
            let (ex, ey, _) = m.clamp_shot(end.0, end.1);
            vec![m.shot_to_input(sx, sy), m.shot_to_input(ex, ey)]
        }
        // mouse down/up 与无坐标点击作用于当前光标处。
        _ => match parts.backend.cursor_position() {
            Ok((dx, dy)) => {
                let Some(m) = map else {
                    return T3Screening::Clear;
                };
                // Mixed-DPI guard: device-space rects of different monitors can
                // overlap when their scales differ, so containment must be
                // checked in the input space — otherwise the cursor on one
                // screen can pass the wrong monitor's map and device_to_input
                // yields coordinates far from the real cursor (screening the
                // wrong point while the injection lands on the real one).
                let Some((ix, iy)) = m.device_to_input_checked(dx, dy) else {
                    return T3Screening::Clear;
                };
                vec![(ix, iy)]
            }
            Err(_) => return T3Screening::Clear,
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

/// T3 筛查只覆盖**激活类**动作：点击/按下/释放/拖拽/键盘——能触发控件或
/// 产生输入后果的动作。mouse_move 与 scroll 仍是 Input 类（需要会话授权、
/// 照常审计），但悬停与滚动不产生动作后果，不设筛查确认——主流产品对
/// hover/scroll 一律不设门。
fn requires_t3_check(action: &ComputerUseAction) -> bool {
    matches!(
        action,
        ComputerUseAction::Click { .. }
            | ComputerUseAction::MouseDown { .. }
            | ComputerUseAction::MouseUp { .. }
            | ComputerUseAction::Drag { .. }
            | ComputerUseAction::Type { .. }
            | ComputerUseAction::KeyChord { .. }
            | ComputerUseAction::HoldKey { .. }
    )
}

/// Whether the full typed text may ride the confirm event
/// (`type_preview_full`), or `None` when it must not. Included ONLY when:
/// - the action is `Type` (other actions have no typed text),
/// - the target is NOT secure/masked for this mint — the masked summary
///   exists precisely so password text is never revealed in the confirm
///   dialog or event stream,
/// - the text is at most 4096 chars — the cap bounds the payload size and
///   thus the abuse surface that reaches the dialog. Short texts ride along
///   too: the dialog otherwise shows only "type N characters", and approving
///   a length with zero visible content is not informed consent (round-6
///   review; the old 12-char lower bound was stale logic from the removed
///   inline preview).
fn full_type_preview(action: &ComputerUseAction, secure_type_target: bool) -> Option<String> {
    let ComputerUseAction::Type { text } = action else {
        return None;
    };
    if secure_type_target {
        return None;
    }
    let count = text.chars().count();
    (count <= 4096).then(|| text.clone())
}

/// 铸造待确认请求、发事件并给模型返回「未执行、去要确认」错误。每个会话
/// 至多一个 pending：新的请求直接替换旧请求（最新胜出，与普通对话框一致）。
fn request_confirmation(
    parts: &Parts,
    summary: &str,
    element_label: &str,
    reason_phrase: &str,
    type_preview_full: Option<String>,
) -> String {
    let confirm_id = parts.shared.new_pending_confirmation(
        &parts.session_id,
        summary.to_string(),
        element_label.to_string(),
    );
    let mut payload = json!({
        "session_id": parts.session_id,
        "action": summary,
        "element": element_label,
        "confirm_id": confirm_id,
    });
    // Optional full typed text for the frontend's "show full text" expander
    // (absent = old dialog behavior). See `full_type_preview` for when it
    // may exist (never for secure/masked targets).
    if let Some(full) = type_preview_full {
        payload["type_preview_full"] = Value::String(full);
    }
    parts.events.emit(EVENT_CONFIRM_REQUIRED, payload);
    // The prefix is [`T3_CONFIRM_REQUIRED_ERROR`]: the error message carries
    // the element label, so the audit record's error field holds only the
    // stable code (see the tail of `run`); the model still receives the full
    // message.
    format!(
        "{T3_CONFIRM_REQUIRED_ERROR}: this action targets {reason_phrase}: \
         \"{element_label}\". It was NOT executed. \
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
            // The masked-target decision only shapes the dialog payload
            // (whether the full typed text may ride the confirm event).
            let secure_type_target =
                matches!(&action, ComputerUseAction::Type { .. }) && type_target_is_secure(&parts);
            let summary = action_summary(&action);
            // Optional full text for the confirm event's expander, fixed at
            // mint time (never recomputed at spend time).
            let type_preview_full = full_type_preview(&action, secure_type_target);
            match &parsed.confirm_id {
                Some(id) => match parts
                    .shared
                    .take_confirmation(id, &parts.session_id, &summary)
                {
                    // A granted token proceeds directly to execution — no
                    // re-screen, no re-request arms (mainstream model: the
                    // API confirmation is one per-action id the client
                    // acknowledges; there is no crypto and no re-verification).
                    ConfirmationCheck::Granted => {
                        confirmed_t3 = true;
                    }
                    ConfirmationCheck::Unknown => {
                        return Err(
                            "the confirm_id is invalid, expired, or was already used. Ask the \
                             user to confirm again."
                                .to_string(),
                        );
                    }
                },
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
                                type_preview_full.clone(),
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
    // Single informational audit record per call. Redaction lives here: typed
    // text is never logged (length only), typing-form chords log a key count,
    // screenshots log SHA-256 + path. Fail-open: a write failure warns and
    // NEVER blocks the action, for every action class.
    let mut record = AuditRecord::new(
        &parts.session_id,
        action.name(),
        consent_label(&action, confirmed_t3),
    );
    match &action {
        // Privacy: never log typed text — length only.
        ComputerUseAction::Type { text } => {
            record.with_target(&format!("typed {} characters", text.chars().count()));
        }
        // Typing-form chords (single char + shifts, any order) could spell
        // out a password one call at a time, so only the key count is
        // logged — never the character; chords with real modifier keys are
        // shortcuts with no dictionary risk and keep plaintext (readability
        // for the user wins).
        ComputerUseAction::KeyChord { keys, .. } | ComputerUseAction::HoldKey { keys, .. }
            if is_typed_text_chord(keys) =>
        {
            record.with_target(&format!("pressed 1 key{}", held_key_suffix(&action)));
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
    if let Some(shot) = &shot {
        record.with_screenshot(&shot.png, &shot.abs_path);
    }
    let record = match &body {
        Ok(_) => record.finish("ok", None, duration_ms),
        // The full T3 "confirmation required" message carries the element
        // label, so the audit error field holds only the stable code; every
        // other error path keeps the full message (the model always receives
        // the full message).
        Err(message) => {
            let audit_error = if message.starts_with(T3_CONFIRM_REQUIRED_ERROR) {
                Some(T3_CONFIRM_REQUIRED_ERROR.to_string())
            } else {
                Some(message.clone())
            };
            record.finish("error", audit_error, duration_ms)
        }
    };
    if let Err(error) = AuditLog::for_session(&parts.session_id).and_then(|log| log.append(&record))
    {
        eprintln!("[computer_use] audit append failed: {error}");
    }

    match body {
        Ok(text) => {
            let mut result = ToolResult::success(text);
            if let Some(shot) = &shot {
                result = with_image_metadata(result, shot);
            }
            result
        }
        Err(message) => ToolResult::error(message),
    }
}

/// hold_key 的审计目标后缀（按住时长）；非 hold 动作为空串。
fn held_key_suffix(action: &ComputerUseAction) -> String {
    match action {
        ComputerUseAction::HoldKey { ms, .. } => format!(" held for {ms}ms"),
        _ => String::new(),
    }
}

/// 动作摘要：纯人类可读的参数摘要（`left click x1 at Some((5, 6))`、
/// `type 3 characters`……）。摘要展示给用户（知情批准）并绑定进批准令牌；
/// 键入内容永不出现（Type 只记字符数，无明文）。
fn action_summary(action: &ComputerUseAction) -> String {
    match action {
        ComputerUseAction::Click { button, count, at } => {
            format!("{} click x{count} at {at:?}", button.as_str())
        }
        ComputerUseAction::Drag { start, end } => format!("drag {start:?} -> {end:?}"),
        ComputerUseAction::Type { text } => {
            format!("type {} characters", text.chars().count())
        }
        ComputerUseAction::KeyChord { chord, .. } => format!("key {chord}"),
        ComputerUseAction::HoldKey { chord, ms, .. } => format!("hold {chord} for {ms}ms"),
        ComputerUseAction::Scroll {
            direction,
            amount,
            at,
        } => format!("scroll {} x{amount} at {at:?}", direction.as_str()),
        ComputerUseAction::MouseDown { button } => format!("{} mouse down", button.as_str()),
        ComputerUseAction::MouseUp { button } => format!("{} mouse up", button.as_str()),
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
                    // Mixed-DPI guard: device-space containment is unsound when
                    // monitor scales differ (overlapping device rects); gate on
                    // the input-space check and otherwise report raw device
                    // coordinates with a warning instead of cross-screen junk.
                    if m.device_to_input_checked(dx, dy).is_none() {
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
            // 按住状态只服务于拖拽失败后的兜底松键（失败路径不置位：按下
            // 失败=物理上没有按住）；筛查不再使用它。
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
            if let Err(error) = backend.drag(from, to) {
                // Tool-layer safety net: a failed drag must not leave the
                // physical left button stuck held when this tool pressed it
                // earlier. Best-effort release (platform backends may clean
                // up on their own; its error is ignored — if it also fails,
                // revoke/stop/tool-drop cleanup retries via the control
                // lane). The note rides on the error message because the
                // warnings vector is dropped on the error path in `run`.
                let mut note = String::new();
                if parts.state.lock().mouse_buttons_held
                    && backend.mouse_up(MouseButton::Left).is_ok()
                {
                    parts.state.lock().mouse_buttons_held = false;
                    note.push_str(
                        "\nwarning: the drag failed; a best-effort mouse-button release was \
                         issued — verify no button is stuck held",
                    );
                }
                return Err(ComputerUseError::failed(format!("{error}{note}")));
            }
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
         typing) requires a per-session grant from the user; the grant stays valid until \
         revoked. Before an activation action runs (clicks, drags, mouse down/up, typing, \
         keys), its target is screened against a consequential-control denylist \
         (financial/send/delete/submit/consent categories) and password fields; a hit \
         pauses the action until the user confirms it via confirm_id. Screening is \
         best-effort category detection over the accessibility tree: when the target \
         cannot be read or screening is unavailable, the action still executes without \
         confirmation. The CONTENT you type is never screened, and key chords are not \
         screened for destructiveness. Observation (screenshots, ui_tree) reads on-screen \
         and focused-window content while the feature is enabled, which may include \
         private information. After actions that change the screen a fresh screenshot is \
         attached; if it is not visible, call image_analyze with the returned attachments \
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
        // 解析失败同样留痕（round-6 评审：parse 阶段的拒绝此前零审计，模型
        // 可以无痕迹地探测动作面）。只记截断后的动作名与错误，绝不记录
        // 调用参数全文（可能含键入文本）。
        let parsed = match parse_action(&input) {
            Ok(parsed) => parsed,
            Err(error) => {
                let action_name = input
                    .get("action")
                    .and_then(Value::as_str)
                    .map(|name| name.chars().take(80).collect::<String>())
                    .unwrap_or_else(|| "missing".to_string());
                let error_text = error.to_string();
                let record = AuditRecord::new(
                    &self.parts.session_id,
                    "unparseable",
                    format!("action:{action_name}"),
                )
                .finish(
                    "rejected",
                    Some(format!(
                        "parse failed: {}",
                        error_text.chars().take(160).collect::<String>()
                    )),
                    0,
                );
                if let Ok(log) = AuditLog::for_session(&self.parts.session_id) {
                    if let Err(audit_error) = log.append(&record) {
                        eprintln!("[computer_use] audit append failed: {audit_error}");
                    }
                }
                return Err(error);
            }
        };

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

        // 同意门控（同步快速路径，拒绝时发事件）。观察类动作只需开关开启且
        // 未停止（check_readonly）。
        let gate = match parsed.action.class() {
            ActionClass::Observe => self.parts.shared.check_readonly(),
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
        GuardRejection::InputBusy => "input-busy",
    }
}

/// 被拒调用（gate 拒绝 / 能力拒绝）的审计：动作未执行、无注入后果，但
/// 绝不静默吞错（fail-open：append 失败只 eprintln）。
fn audit_rejected_call(parts: &Parts, action: &ComputerUseAction, reason: &str, message: &str) {
    let record = AuditRecord::new(
        &parts.session_id,
        action.name(),
        format!("rejected:{reason}"),
    )
    .finish("rejected", Some(message.to_string()), 0);
    match AuditLog::for_session(&parts.session_id) {
        Ok(log) => {
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

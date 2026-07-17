//! 桌宠窗口:透明 / 无边框 / 置顶 / 不进任务栏的常驻小窗,加载独立 `pet.html` 入口。
//!
//! 与 detach.rs 的撕离窗口平行——撕离是"通用面板搬家"(带边框、可缩放、落位最大化),
//! 桌宠语义完全相反(固定小窗 + 透明 + 置顶),故独立成模块,不复用 detached kind。
//! 动画状态由前端 pet 窗口自己监听全局 `chat:*` 事件驱动,Rust 侧只管窗口生命周期。
//! 注意:pet 窗口的 JS 端 IPC 权限在 capabilities/default.json 的 windows 里登记,
//! 漏掉会导致 listen/startDragging 全部静默被拒(宠物不动、拖不了)。

use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, sync::Mutex};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

pub const PET_LABEL: &str = "pet";
pub const PET_MENU_LABEL: &str = "pet-menu";

const PET_FRAME_W: f64 = 192.0;
const PET_FRAME_H: f64 = 208.0;
const PET_HORIZONTAL_PADDING: f64 = 48.0;
const PET_VERTICAL_PADDING: f64 = 16.0;
const PET_COMPACT_MIN_H: f64 = 165.0;
const PET_ACTIVITY_WINDOW_W: f64 = 350.0;
const PET_ACTIVITY_DEFAULT_H: f64 = 112.0;
const PET_ACTIVITY_MIN_H: f64 = 48.0;
const PET_ACTIVITY_MAX_H: f64 = 260.0;
const PET_ACTIVITY_GAP_H: f64 = 12.0;
/// 人物可见区距窗口底边的距离,与 pet.css(.pet-root padding-bottom)及
/// JS 拖拽物理(pet-interaction PET_BOTTOM_PADDING)三方必须一致。
const PET_CHARACTER_BOTTOM: f64 = 8.0;
const MIN_SCALE: f64 = 0.5;
const MAX_SCALE: f64 = 1.2;
const PET_MENU_WIDTH: f64 = 58.0;
const PET_MENU_HEIGHT: f64 = 28.0;
const PET_MENU_GAP: f64 = 3.0;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PetNavigationRequest {
    pub session_id: Option<String>,
    pub scheduled_run: Option<PetScheduledRunNavigation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PetScheduledRunNavigation {
    pub automation_id: String,
    pub run_id: String,
    pub session_id: String,
    pub task_name: String,
    pub ended_at: String,
}

impl PetScheduledRunNavigation {
    fn validated(self) -> Result<Self, String> {
        let automation_id = self.automation_id.trim();
        let run_id = self.run_id.trim();
        let session_id = self.session_id.trim();
        let task_name = self.task_name.trim();
        let ended_at = self.ended_at.trim();
        if automation_id.is_empty()
            || run_id.is_empty()
            || session_id.is_empty()
            || task_name.is_empty()
            || ended_at.is_empty()
        {
            return Err("scheduled pet navigation is incomplete".into());
        }
        Ok(Self {
            automation_id: automation_id.to_string(),
            run_id: run_id.to_string(),
            session_id: session_id.to_string(),
            task_name: task_name.to_string(),
            ended_at: ended_at.to_string(),
        })
    }
}

#[derive(Default)]
pub struct PetNavigationState {
    pending: Mutex<Option<PetNavigationRequest>>,
}

impl PetNavigationState {
    fn replace(&self, request: PetNavigationRequest) -> Result<(), String> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| "pet navigation state lock poisoned".to_string())?;
        *pending = Some(request);
        Ok(())
    }

    fn take(&self) -> Result<Option<PetNavigationRequest>, String> {
        self.pending
            .lock()
            .map_err(|_| "pet navigation state lock poisoned".to_string())
            .map(|mut pending| pending.take())
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PetReplyRequest {
    pub request_id: String,
    pub session_id: String,
    pub text: String,
}

impl PetReplyRequest {
    fn validated(request_id: &str, session_id: &str, text: &str) -> Result<Self, String> {
        let request_id = request_id.trim();
        let session_id = session_id.trim();
        let text = text.trim();
        if request_id.is_empty() {
            return Err("pet reply request id is empty".into());
        }
        if session_id.is_empty() {
            return Err("pet reply session id is empty".into());
        }
        if text.is_empty() {
            return Err("pet reply text is empty".into());
        }
        Ok(Self {
            request_id: request_id.to_string(),
            session_id: session_id.to_string(),
            text: text.to_string(),
        })
    }
}

#[derive(Default)]
pub struct PetReplyState {
    pending: Mutex<VecDeque<PetReplyRequest>>,
}

impl PetReplyState {
    fn push(&self, request: PetReplyRequest) -> Result<(), String> {
        self.pending
            .lock()
            .map_err(|_| "pet reply state lock poisoned".to_string())?
            .push_back(request);
        Ok(())
    }

    fn take(&self) -> Result<Option<PetReplyRequest>, String> {
        self.pending
            .lock()
            .map_err(|_| "pet reply state lock poisoned".to_string())
            .map(|mut pending| pending.pop_front())
    }
}

fn pet_window_logical_size(
    scale: f64,
    activity_visible: bool,
    activity_height: Option<f64>,
) -> (f64, f64) {
    let scale = clamp_scale(scale);
    let compact_width = PET_FRAME_W * scale + PET_HORIZONTAL_PADDING;
    if activity_visible {
        let content_height = activity_content_height(activity_height);
        (
            compact_width.max(PET_ACTIVITY_WINDOW_W),
            PET_FRAME_H * scale + content_height + PET_ACTIVITY_GAP_H,
        )
    } else {
        (
            compact_width,
            (PET_FRAME_H * scale + PET_VERTICAL_PADDING).max(PET_COMPACT_MIN_H),
        )
    }
}

fn activity_content_height(activity_height: Option<f64>) -> f64 {
    let measured = activity_height.unwrap_or(PET_ACTIVITY_DEFAULT_H);
    if measured.is_finite() {
        measured.clamp(PET_ACTIVITY_MIN_H, PET_ACTIVITY_MAX_H)
    } else {
        PET_ACTIVITY_DEFAULT_H
    }
}

fn character_local_top_left(
    scale: f64,
    activity_visible: bool,
    activity_height: Option<f64>,
    alignment: &str,
) -> (f64, f64) {
    let scale = clamp_scale(scale);
    let (window_width, window_height) =
        pet_window_logical_size(scale, activity_visible, activity_height);
    let character_width = PET_FRAME_W * scale;
    let character_height = PET_FRAME_H * scale;
    let x = if alignment == "left" {
        PET_HORIZONTAL_PADDING / 2.0
    } else {
        window_width - PET_HORIZONTAL_PADDING / 2.0 - character_width
    };
    // 人物贴底(与 pet.css 的 flex-end + padding-bottom: 8px 严格一致):
    // 纵向位置只由窗口高度决定,活动卡的出现/消失不再牵动人物。
    let y = window_height - PET_CHARACTER_BOTTOM - character_height;
    (x, y)
}

fn clamp_scale_to_character_work_area(
    scale: f64,
    anchor: (f64, f64),
    scale_factor: f64,
    work_area: Option<(i32, i32, u32, u32)>,
) -> f64 {
    let scale = clamp_scale(scale);
    let Some((left, top, width, height)) = work_area else {
        return scale;
    };
    if !scale_factor.is_finite()
        || scale_factor <= 0.0
        || !anchor.0.is_finite()
        || !anchor.1.is_finite()
    {
        return scale;
    }
    let right = left as f64 + width as f64;
    let bottom = top as f64 + height as f64;
    let max_horizontal = (right - anchor.0) / (PET_FRAME_W * scale_factor);
    let max_vertical = (bottom - anchor.1) / (PET_FRAME_H * scale_factor);
    let maximum = max_horizontal.min(max_vertical);
    if maximum.is_finite() {
        scale.min(maximum.max(MIN_SCALE))
    } else {
        scale
    }
}

/// `~/.pinvou3/pet_window.json` —— 桌宠窗口位置(全局物理像素)+ 缩放。
/// 见 prefs::PetPrefs 注释:刻意不进 settings.json,避免前端整份回写覆盖。
fn state_path() -> std::path::PathBuf {
    crate::bridge::paths::pinvou3_home().join("pet_window.json")
}

fn default_scale() -> f64 {
    MIN_SCALE
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PetWindowState {
    pub x: Option<i32>,
    pub y: Option<i32>,
    #[serde(default = "default_scale")]
    pub scale: f64,
    pub activity_visible: bool,
}

impl Default for PetWindowState {
    fn default() -> Self {
        Self {
            x: None,
            y: None,
            scale: MIN_SCALE,
            activity_visible: false,
        }
    }
}

pub fn clamp_scale(s: f64) -> f64 {
    if !s.is_finite() {
        return 1.0;
    }
    s.clamp(MIN_SCALE, MAX_SCALE)
}

fn load_state() -> PetWindowState {
    let st: PetWindowState = std::fs::read_to_string(state_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    PetWindowState {
        scale: clamp_scale(st.scale),
        ..st
    }
}

fn save_state_to(path: &std::path::Path, st: &PetWindowState) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create pet state directory {}: {error}", parent.display()))?;
    }
    let json = serde_json::to_string(st)
        .map_err(|error| format!("serialize pet window state: {error}"))?;
    std::fs::write(path, json)
        .map_err(|error| format!("write pet window state {}: {error}", path.display()))
}

fn save_state(st: PetWindowState) -> Result<(), String> {
    let path = state_path();
    save_state_to(&path, &st).map_err(|error| {
        eprintln!("[pinvou3-app] {error}");
        error
    })
}

/// 点 (cx,cy) 是否落在任一显示器矩形内。恢复保存位置前用窗口中心点判定——
/// 显示器可能被拔掉/换分辨率,落在"不存在的屏"上的宠物等于消失。
pub fn point_on_any_monitor(cx: i32, cy: i32, monitors: &[(i32, i32, u32, u32)]) -> bool {
    monitors
        .iter()
        .any(|&(x, y, w, h)| crate::detach::point_in_rect(cx, cy, x, y, w as i32, h as i32))
}

/// 建/显示桌宠窗口。已存在只 show(设置开关反复切换不重建 WebView)。
pub fn create_or_show(app: &AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(PET_LABEL) {
        show_and_keep_above(&win)?;
        return Ok(());
    }
    let state = load_state();
    let scale = state.scale;
    let initial_size = pet_window_logical_size(scale, state.activity_visible, None);
    let win = WebviewWindowBuilder::new(app, PET_LABEL, WebviewUrl::App("pet.html".into()))
        .title("PINVOU 桌伴公仔")
        .inner_size(initial_size.0, initial_size.1)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .build()
        .map_err(|e| format!("build pet window: {e}"))?;

    position_window(&win);
    // Linux/X11 下 builder 在窗口 map 之前写入的 always_on_top 可能丢失，
    // 必须在窗口已显示后再向窗口管理器重申一次。
    keep_above(&win)?;
    Ok(())
}

fn keep_above(win: &tauri::WebviewWindow) -> Result<(), String> {
    win.set_always_on_top(true)
        .map_err(|error| format!("keep pet window always on top: {error}"))
}

fn show_and_keep_above(win: &tauri::WebviewWindow) -> Result<(), String> {
    win.show()
        .map_err(|error| format!("show pet window: {error}"))?;
    keep_above(win)
}

/// 恢复保存位置(中心点仍在某显示器内才信),否则落到主屏右下角。
fn position_window(win: &tauri::WebviewWindow) {
    let monitors: Vec<(i32, i32, u32, u32)> = win
        .available_monitors()
        .map(|ms| {
            ms.iter()
                .map(|m| {
                    let p = m.position();
                    let s = m.size();
                    (p.x, p.y, s.width, s.height)
                })
                .collect()
        })
        .unwrap_or_default();

    let st = load_state();
    if let (Some(x), Some(y)) = (st.x, st.y) {
        let fallback = pet_window_logical_size(st.scale, st.activity_visible, None);
        let (w, h) = win
            .outer_size()
            .map(|s| (s.width as i32, s.height as i32))
            .unwrap_or((fallback.0 as i32, fallback.1 as i32));
        if point_on_any_monitor(x + w / 2, y + h / 2, &monitors) {
            let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
            return;
        }
    }
    // 默认落点:主屏右下,留边距 + 任务栏冗余。
    if let Ok(Some(m)) = win.primary_monitor() {
        let p = m.position();
        let s = m.size();
        let monitor_scale = m.scale_factor();
        let logical = pet_window_logical_size(st.scale, st.activity_visible, None);
        let w = (logical.0 * monitor_scale) as i32;
        let h = (logical.1 * monitor_scale) as i32;
        let x = p.x + s.width as i32 - w - (24.0 * monitor_scale) as i32;
        let y = p.y + s.height as i32 - h - (96.0 * monitor_scale) as i32;
        let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
    }
}

/// 启动时按 settings.json 决定是否拉起桌宠(setup 钩子里调)。
pub fn spawn_if_enabled(app: &AppHandle) {
    if crate::bridge::prefs::UserPrefs::load().pet.enabled {
        if let Err(e) = create_or_show(app) {
            eprintln!("[pinvou3-app] pet window create failed: {e}");
        }
    }
}

/// 主窗口销毁时把桌宠一并带走——否则只剩宠物窗口时 app 不退出。
pub fn close_with_main(app: &AppHandle) {
    if let Some(pet) = app.get_webview_window(PET_LABEL) {
        let _ = pet.close();
    }
    if let Some(menu) = app.get_webview_window(PET_MENU_LABEL) {
        let _ = menu.close();
    }
}

/// 开关桌宠:持久化 settings.json + 窗口即时显隐 + 广播给主窗口同步其 settings
/// 副本(否则主窗口下次整份保存会用旧值把开关翻回去)。
/// 设置页开关和宠物右键"隐藏"都走这一个命令,单一路径。
#[tauri::command]
pub async fn set_pet_enabled(enabled: bool, app: AppHandle) -> Result<(), String> {
    let window_existed = app.get_webview_window(PET_LABEL).is_some();
    if enabled {
        create_or_show(&app)?;
    }
    let mut prefs = crate::bridge::prefs::UserPrefs::load();
    let was_enabled = prefs.pet.enabled;
    prefs.pet.enabled = enabled;
    if let Err(error) = prefs.save() {
        if enabled && !was_enabled {
            if let Some(win) = app.get_webview_window(PET_LABEL) {
                if window_existed {
                    let _ = win.hide();
                } else {
                    let _ = win.close();
                }
            }
        }
        return Err(format!("save pet.enabled failed: {error:?}"));
    }
    if !enabled {
        if let Some(win) = app.get_webview_window(PET_LABEL) {
            let _ = win.hide();
        }
        if let Some(menu) = app.get_webview_window(PET_MENU_LABEL) {
            let _ = menu.hide();
        }
    }
    let _ = app.emit(
        "pet:enabled_changed",
        serde_json::json!({ "enabled": enabled }),
    );
    Ok(())
}

fn position_pet_menu_at_cursor(
    pet: &tauri::WebviewWindow,
    menu: &tauri::WebviewWindow,
    anchor: (f64, f64),
) {
    let (Ok(pet_position), Ok(menu_size)) = (pet.outer_position(), menu.outer_size()) else {
        return;
    };
    let scale_factor = pet.scale_factor().unwrap_or(1.0);
    let cursor_x = pet_position.x + (anchor.0 * scale_factor).round() as i32;
    let cursor_y = pet_position.y + (anchor.1 * scale_factor).round() as i32;
    let gap = (PET_MENU_GAP * scale_factor).round() as i32;
    let work_area = pet.current_monitor().ok().flatten().map(|monitor| {
        let area = monitor.work_area();
        (
            area.position.x,
            area.position.y,
            area.size.width,
            area.size.height,
        )
    });
    let mut x = cursor_x + gap;
    let mut y = cursor_y + gap;
    if let Some((left, top, width, height)) = work_area {
        let right = left + width as i32;
        let bottom = top + height as i32;
        if x + menu_size.width as i32 > right {
            x = cursor_x - menu_size.width as i32 - gap;
        }
        if y + menu_size.height as i32 > bottom {
            y = cursor_y - menu_size.height as i32 - gap;
        }
        x = x.clamp(left, (right - menu_size.width as i32).max(left));
        y = y.clamp(top, (bottom - menu_size.height as i32).max(top));
    }
    let _ = menu.set_position(tauri::PhysicalPosition::new(x, y));
}

/// 使用独立的紧凑弹层复刻 Codex 菜单视觉，同时避开系统菜单不可调的最小宽度。
#[tauri::command]
pub async fn show_pet_context_menu(
    anchor_x: f64,
    anchor_y: f64,
    app: AppHandle,
) -> Result<(), String> {
    let pet = app
        .get_webview_window(PET_LABEL)
        .ok_or_else(|| "pet window not found".to_string())?;
    let menu = if let Some(menu) = app.get_webview_window(PET_MENU_LABEL) {
        menu
    } else {
        WebviewWindowBuilder::new(
            &app,
            PET_MENU_LABEL,
            WebviewUrl::App("pet-menu.html".into()),
        )
        .title("桌伴公仔菜单")
        .inner_size(PET_MENU_WIDTH, PET_MENU_HEIGHT)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .build()
        .map_err(|error| format!("build pet context menu window failed: {error}"))?
    };
    let _ = menu.set_size(tauri::LogicalSize::new(PET_MENU_WIDTH, PET_MENU_HEIGHT));
    position_pet_menu_at_cursor(&pet, &menu, (anchor_x, anchor_y));
    let _ = menu.eval("document.documentElement.classList.remove('pet-menu-hidden')");
    menu.show()
        .map_err(|error| format!("show pet context menu failed: {error}"))?;
    menu.set_focus()
        .map_err(|error| format!("focus pet context menu failed: {error}"))
}

#[tauri::command]
pub async fn hide_pet_context_menu(app: AppHandle) -> Result<(), String> {
    if let Some(menu) = app.get_webview_window(PET_MENU_LABEL) {
        menu.hide()
            .map_err(|error| format!("hide pet context menu failed: {error}"))?;
    }
    Ok(())
}

/// 前端初始化取缩放。
#[tauri::command]
pub async fn get_pet_scale() -> Result<f64, String> {
    Ok(load_state().scale)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScaleAnchor {
    BottomCenter,
    BottomLeft,
    BottomRight,
    TopLeft,
}

fn resized_position(
    position: (i32, i32),
    old_size: (u32, u32),
    new_size: (u32, u32),
    anchor: ScaleAnchor,
    work_area: Option<(i32, i32, u32, u32)>,
) -> (i32, i32) {
    let (mut x, mut y) = position;
    match anchor {
        ScaleAnchor::BottomCenter => {
            x += (old_size.0 as i32 - new_size.0 as i32) / 2;
            y += old_size.1 as i32 - new_size.1 as i32;
        }
        ScaleAnchor::BottomLeft => {
            y += old_size.1 as i32 - new_size.1 as i32;
        }
        ScaleAnchor::BottomRight => {
            x += old_size.0 as i32 - new_size.0 as i32;
            y += old_size.1 as i32 - new_size.1 as i32;
        }
        ScaleAnchor::TopLeft => {}
    }
    if let Some((left, top, width, height)) = work_area {
        let right = left + width as i32;
        let bottom = top + height as i32;
        let max_x = (right - new_size.0 as i32).max(left);
        let max_y = (bottom - new_size.1 as i32).max(top);
        x = x.clamp(left, max_x);
        y = y.clamp(top, max_y);
    }
    (x, y)
}

fn edge_anchor(
    position: (i32, i32),
    size: (u32, u32),
    work_area: Option<(i32, i32, u32, u32)>,
) -> ScaleAnchor {
    let Some((left, _, width, _)) = work_area else {
        return ScaleAnchor::BottomCenter;
    };
    let window_center = position.0 as i64 + size.0 as i64 / 2;
    let monitor_center = left as i64 + width as i64 / 2;
    if window_center <= monitor_center {
        ScaleAnchor::BottomLeft
    } else {
        ScaleAnchor::BottomRight
    }
}

fn window_edge_anchor(win: &tauri::WebviewWindow) -> ScaleAnchor {
    let position = win.outer_position().ok();
    let size = win.outer_size().ok();
    let work_area = win.current_monitor().ok().flatten().map(|monitor| {
        let area = monitor.work_area();
        (
            area.position.x,
            area.position.y,
            area.size.width,
            area.size.height,
        )
    });
    match (position, size) {
        (Some(position), Some(size)) => edge_anchor(
            (position.x, position.y),
            (size.width, size.height),
            work_area,
        ),
        _ => ScaleAnchor::BottomCenter,
    }
}

fn resize_pet_window(win: &tauri::WebviewWindow, logical_size: (f64, f64), anchor: ScaleAnchor) {
    let before = win.outer_size().ok().zip(win.outer_position().ok());
    let work_area = win.current_monitor().ok().flatten().map(|monitor| {
        let area = monitor.work_area();
        (
            area.position.x,
            area.position.y,
            area.size.width,
            area.size.height,
        )
    });
    let sf = win.scale_factor().unwrap_or(1.0);
    let (nw, nh) = (
        (logical_size.0 * sf).round() as u32,
        (logical_size.1 * sf).round() as u32,
    );
    let _ = win.set_size(tauri::PhysicalSize::new(nw, nh));
    if let Some((size, pos)) = before {
        let (nx, ny) = resized_position(
            (pos.x, pos.y),
            (size.width, size.height),
            (nw, nh),
            anchor,
            work_area,
        );
        let _ = win.set_position(tauri::PhysicalPosition::new(nx, ny));
    }
}

/// 人物锚点靠近工作区左/上边时,窗口左上角会被算成负坐标(活动卡展开且人物
/// 右对齐时人物局部横坐标可达 230px),活动卡整块跑到屏幕外。clamp_scale_to_
/// character_work_area 只按右/下距离限制缩放,管不到这一侧,所以位置这里再钳一次。
fn character_anchor_position(
    position: (i32, i32),
    size: (u32, u32),
    work_area: Option<(i32, i32, u32, u32)>,
) -> (i32, i32) {
    let (mut x, mut y) = position;
    if let Some((left, top, width, height)) = work_area {
        let right = left + width as i32;
        let bottom = top + height as i32;
        let max_x = (right - size.0 as i32).max(left);
        let max_y = (bottom - size.1 as i32).max(top);
        x = x.clamp(left, max_x);
        y = y.clamp(top, max_y);
    }
    (x, y)
}

fn resize_pet_window_at_character_anchor(
    win: &tauri::WebviewWindow,
    logical_size: (f64, f64),
    scale: f64,
    activity_visible: bool,
    activity_height: Option<f64>,
    alignment: &str,
    screen_anchor: (f64, f64),
) {
    let sf = win.scale_factor().unwrap_or(1.0);
    let (local_x, local_y) =
        character_local_top_left(scale, activity_visible, activity_height, alignment);
    let width = (logical_size.0 * sf).round() as u32;
    let height = (logical_size.1 * sf).round() as u32;
    let work_area = win.current_monitor().ok().flatten().map(|monitor| {
        let area = monitor.work_area();
        (
            area.position.x,
            area.position.y,
            area.size.width,
            area.size.height,
        )
    });
    let (x, y) = character_anchor_position(
        (
            (screen_anchor.0 - local_x * sf).round() as i32,
            (screen_anchor.1 - local_y * sf).round() as i32,
        ),
        (width, height),
        work_area,
    );
    let _ = win.set_size(tauri::PhysicalSize::new(width, height));
    let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
}

/// 缩放桌宠:右下角拉伸保持人物可见区域左上角不动;右键菜单缩放保持底边中点不动。
/// 两种路径都会钳制在当前显示器工作区内。返回 clamp 后的实际值。
#[tauri::command]
pub async fn set_pet_scale(
    scale: f64,
    anchor: Option<String>,
    alignment: Option<String>,
    anchor_x: Option<f64>,
    anchor_y: Option<f64>,
    activity_visible: Option<bool>,
    activity_height: Option<f64>,
    persist: Option<bool>,
    app: AppHandle,
) -> Result<f64, String> {
    let win = app.get_webview_window(PET_LABEL);
    let character_anchor = match (anchor.as_deref(), anchor_x, anchor_y) {
        (Some("character_top_left"), Some(x), Some(y)) if x.is_finite() && y.is_finite() => {
            Some((x, y))
        }
        _ => None,
    };
    let mut scale = clamp_scale(scale);
    if let (Some(win), Some(character_anchor)) = (win.as_ref(), character_anchor) {
        let scale_factor = win.scale_factor().unwrap_or(1.0);
        let work_area = win.current_monitor().ok().flatten().map(|monitor| {
            let area = monitor.work_area();
            (
                area.position.x,
                area.position.y,
                area.size.width,
                area.size.height,
            )
        });
        scale =
            clamp_scale_to_character_work_area(scale, character_anchor, scale_factor, work_area);
    }
    let mut st = load_state();
    st.scale = scale;
    let activity_visible = activity_visible.unwrap_or(st.activity_visible);
    st.activity_visible = activity_visible;
    if persist.unwrap_or(true) {
        save_state(st)?;
    }
    if let Some(win) = win {
        let logical_size = pet_window_logical_size(scale, activity_visible, activity_height);
        if let Some(character_anchor) = character_anchor {
            resize_pet_window_at_character_anchor(
                &win,
                logical_size,
                scale,
                activity_visible,
                activity_height,
                alignment.as_deref().unwrap_or("right"),
                character_anchor,
            );
        } else {
            let scale_anchor = if anchor.as_deref() == Some("top_left") {
                ScaleAnchor::TopLeft
            } else if activity_visible {
                window_edge_anchor(&win)
            } else {
                ScaleAnchor::BottomCenter
            };
            resize_pet_window(&win, logical_size, scale_anchor);
        }
    }
    Ok(scale)
}

#[tauri::command]
pub async fn set_pet_activity_visible(
    visible: bool,
    activity_height: Option<f64>,
    alignment: Option<String>,
    app: AppHandle,
) -> Result<(), String> {
    let mut st = load_state();
    // 高度流式上报会频繁进入本命令:可见性没变时跳过写盘,
    // 别让每次窗口微调都附带一次同步磁盘 IO。
    if st.activity_visible != visible {
        st.activity_visible = visible;
        save_state(st)?;
    }
    if let Some(win) = app.get_webview_window(PET_LABEL) {
        let logical_size = pet_window_logical_size(st.scale, visible, activity_height);
        // 人物在窗口内贴底、贴对齐侧(CSS 与 character_local_top_left 一致),
        // 所以只要缩放时保持"底边 + 人物贴边侧"两条边不动,人物在屏幕上就
        // 纹丝不动——卡片卸载的中间帧也稳定,因为人物位置不依赖卡片存在。
        // 贴边方向必须用前端的实际对齐值:按窗口中心猜测在屏幕中部会猜反,
        // 这正是收起时人物瞬移的原始根因。
        let anchor = match alignment.as_deref() {
            Some("left") => ScaleAnchor::BottomLeft,
            Some(_) => ScaleAnchor::BottomRight,
            None => window_edge_anchor(&win),
        };
        resize_pet_window(&win, logical_size, anchor);
    }
    Ok(())
}

/// 桌宠窗口拖动落定后保存位置(前端 onMoved 防抖后调,全局物理像素)。
#[tauri::command]
pub async fn save_pet_position(x: i32, y: i32) -> Result<(), String> {
    let mut st = load_state(); // 保留 scale
    st.x = Some(x);
    st.y = Some(y);
    save_state(st)
}

/// 点击宠物时唤醒主窗口；点击活动时额外把目标 session 路由给主窗口。
/// 会话切换仍由现有 TauriBridge/Session 实现，这里只负责原生窗口与导航消息。
#[tauri::command]
pub async fn open_main_from_pet(
    session_id: Option<String>,
    scheduled_run: Option<PetScheduledRunNavigation>,
    navigation: State<'_, PetNavigationState>,
    app: AppHandle,
) -> Result<(), String> {
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())?;
    main.show()
        .map_err(|error| format!("show main window failed: {error}"))?;
    main.unminimize()
        .map_err(|error| format!("unminimize main window failed: {error}"))?;
    let target = session_id.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    });
    let scheduled_run = scheduled_run
        .map(PetScheduledRunNavigation::validated)
        .transpose()?;
    navigation.replace(PetNavigationRequest {
        session_id: target,
        scheduled_run,
    })?;
    main.set_focus()
        .map_err(|error| format!("focus main window failed: {error}"))?;
    app.emit_to("main", "pet:navigation_pending", ())
        .map_err(|error| format!("emit pet navigation wakeup failed: {error}"))?;
    Ok(())
}

#[tauri::command]
pub async fn take_pet_navigation(
    navigation: State<'_, PetNavigationState>,
) -> Result<Option<PetNavigationRequest>, String> {
    navigation.take()
}

#[tauri::command]
pub async fn queue_pet_reply(
    request_id: String,
    session_id: String,
    text: String,
    replies: State<'_, PetReplyState>,
    app: AppHandle,
) -> Result<(), String> {
    replies.push(PetReplyRequest::validated(&request_id, &session_id, &text)?)?;
    // 入队已经成功；唤醒失败时主窗口仍会在 effect 启动后主动消费。
    // 这里不能返回可重试错误，否则相同回复可能重复入队。
    let _ = app.emit_to("main", "pet:reply_pending", ());
    Ok(())
}

#[tauri::command]
pub async fn take_pet_reply(
    replies: State<'_, PetReplyState>,
) -> Result<Option<PetReplyRequest>, String> {
    replies.take()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_on_any_monitor_basic() {
        let monitors = [(0, 0, 1920, 1080), (1920, 0, 2560, 1440)];
        assert!(point_on_any_monitor(100, 100, &monitors));
        assert!(point_on_any_monitor(3000, 500, &monitors)); // 第二屏
        assert!(!point_on_any_monitor(-50, 100, &monitors)); // 屏外左
        assert!(!point_on_any_monitor(100, 2000, &monitors)); // 屏外下
        assert!(!point_on_any_monitor(0, 0, &[])); // 拿不到显示器 → 不信任保存位置
    }

    #[test]
    fn pet_state_serde_roundtrip_and_legacy() {
        let st = PetWindowState {
            x: Some(-120),
            y: Some(3456),
            scale: 1.3,
            activity_visible: true,
        };
        let s = serde_json::to_string(&st).unwrap();
        let back: PetWindowState = serde_json::from_str(&s).unwrap();
        assert_eq!(st, back);
        // 旧版文件只有 x/y(无 scale)→ 回当前默认(最小尺寸),同 default_scale()
        let legacy: PetWindowState = serde_json::from_str(r#"{"x":10,"y":20}"#).unwrap();
        assert_eq!(legacy.scale, MIN_SCALE);
        assert_eq!(legacy.x, Some(10));
        assert!(!legacy.activity_visible);
        // 空文件/缺字段 → 全默认
        let empty: PetWindowState = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, PetWindowState::default());
    }

    #[test]
    fn pet_navigation_request_is_taken_exactly_once() {
        let state = PetNavigationState::default();
        let request = PetNavigationRequest {
            session_id: Some("session-1".to_string()),
            scheduled_run: None,
        };
        state.replace(request.clone()).unwrap();
        assert_eq!(state.take().unwrap(), Some(request));
        assert_eq!(state.take().unwrap(), None);
    }

    #[test]
    fn scheduled_pet_navigation_is_trimmed_and_validated() {
        let request = PetScheduledRunNavigation {
            automation_id: " task-1 ".into(),
            run_id: " run-1 ".into(),
            session_id: " sched-1 ".into(),
            task_name: " 新闻速览 ".into(),
            ended_at: " 2026-07-15T10:42:00+08:00 ".into(),
        }
        .validated()
        .unwrap();
        assert_eq!(request.automation_id, "task-1");
        assert_eq!(request.session_id, "sched-1");
        assert!(PetScheduledRunNavigation {
            automation_id: String::new(),
            run_id: "run-1".into(),
            session_id: "sched-1".into(),
            task_name: "新闻速览".into(),
            ended_at: "2026-07-15T10:42:00+08:00".into(),
        }
        .validated()
        .is_err());
    }

    #[test]
    fn pet_reply_queue_is_fifo_and_validated() {
        assert!(PetReplyRequest::validated("", "s", "hello").is_err());
        assert!(PetReplyRequest::validated("r", "", "hello").is_err());
        assert!(PetReplyRequest::validated("r", "s", "   ").is_err());

        let state = PetReplyState::default();
        let first = PetReplyRequest::validated("r1", "s1", " first ").unwrap();
        let second = PetReplyRequest::validated("r2", "s2", "second").unwrap();
        state.push(first.clone()).unwrap();
        state.push(second.clone()).unwrap();
        assert_eq!(state.take().unwrap(), Some(first));
        assert_eq!(state.take().unwrap(), Some(second));
        assert_eq!(state.take().unwrap(), None);
    }

    #[test]
    fn scale_clamped() {
        assert_eq!(clamp_scale(0.1), 0.5);
        assert_eq!(clamp_scale(9.0), 1.2);
        assert_eq!(clamp_scale(1.2), 1.2);
        assert_eq!(clamp_scale(f64::NAN), 1.0);
    }

    #[test]
    fn first_launch_defaults_to_minimum_scale() {
        let state = PetWindowState::default();
        assert_eq!(state.scale, MIN_SCALE);

        let deserialized: PetWindowState = serde_json::from_str("{}").unwrap();
        assert_eq!(deserialized.scale, MIN_SCALE);
        assert_eq!(
            pet_window_logical_size(state.scale, false, None),
            (144.0, 165.0)
        );
    }

    #[test]
    fn pet_window_size_keeps_activity_cards_readable_at_every_pet_scale() {
        assert_eq!(pet_window_logical_size(0.5, false, None), (144.0, 165.0));
        assert_eq!(pet_window_logical_size(0.5, true, None), (350.0, 228.0));
        assert_eq!(pet_window_logical_size(1.0, true, None), (350.0, 332.0));
        assert_eq!(pet_window_logical_size(1.2, true, None), (350.0, 373.6));
    }

    #[test]
    fn character_top_left_tracks_the_real_flex_layout() {
        // 贴底布局:y = 窗高 - 8 - 人物高,与卡片存在与否无关。
        assert_eq!(
            character_local_top_left(0.5, false, None, "right"),
            (24.0, 53.0)
        );
        assert_eq!(
            character_local_top_left(1.0, false, None, "left"),
            (24.0, 8.0)
        );
        assert_eq!(
            character_local_top_left(0.5, true, Some(112.0), "right"),
            (230.0, 116.0)
        );
        assert_eq!(
            character_local_top_left(0.5, true, Some(112.0), "left"),
            (24.0, 116.0)
        );
    }

    #[test]
    fn character_anchor_caps_scale_instead_of_moving_the_anchor() {
        let clamped =
            clamp_scale_to_character_work_area(1.2, (100.0, 100.0), 1.0, Some((0, 0, 300, 300)));
        assert!((clamped - (200.0 / PET_FRAME_H)).abs() < 1e-9);
        assert_eq!(
            clamp_scale_to_character_work_area(1.2, (100.0, 100.0), 1.0, None),
            1.2
        );
    }

    #[test]
    fn character_anchor_position_keeps_window_inside_work_area() {
        // 人物锚点靠屏幕左上角:窗口左上角本会被算成负坐标,活动卡跑到屏幕外。
        assert_eq!(
            character_anchor_position((-130, -40), (350, 332), Some((0, 0, 1920, 1080))),
            (0, 0)
        );
        // 靠右下角:同样要收回工作区内。
        assert_eq!(
            character_anchor_position((1800, 900), (350, 332), Some((0, 0, 1920, 1040))),
            (1570, 708)
        );
        // 工作区带偏移(副屏 / 任务栏在左侧)时按该工作区钳制,不是按 (0,0)。
        assert_eq!(
            character_anchor_position((1900, -20), (350, 332), Some((1920, 0, 1920, 1080))),
            (1920, 0)
        );
        // 已经在工作区内的位置不动。
        assert_eq!(
            character_anchor_position((400, 300), (350, 332), Some((0, 0, 1920, 1080))),
            (400, 300)
        );
        // 窗口比工作区还大时,左/上优先,不产生反向越界。
        assert_eq!(
            character_anchor_position((-50, -50), (400, 400), Some((0, 0, 300, 300))),
            (0, 0)
        );
        // 拿不到工作区就别瞎猜。
        assert_eq!(
            character_anchor_position((-130, -40), (350, 332), None),
            (-130, -40)
        );
    }

    #[test]
    fn pet_window_size_uses_measured_activity_height() {
        assert_eq!(pet_window_logical_size(1.0, false, None), (240.0, 224.0));
        assert_eq!(
            pet_window_logical_size(1.0, true, Some(64.0)),
            (350.0, 284.0)
        );
        assert_eq!(
            pet_window_logical_size(1.0, true, Some(180.0)),
            (350.0, 400.0)
        );
        assert_eq!(
            pet_window_logical_size(1.0, true, Some(999.0)),
            (350.0, 480.0)
        );
    }

    #[test]
    fn resize_position_preserves_selected_anchor_and_work_area() {
        assert_eq!(
            resized_position(
                (100, 200),
                (240, 330),
                (480, 660),
                ScaleAnchor::TopLeft,
                Some((0, 0, 1920, 1080)),
            ),
            (100, 200)
        );
        assert_eq!(
            resized_position(
                (1700, 800),
                (240, 330),
                (480, 660),
                ScaleAnchor::TopLeft,
                Some((0, 0, 1920, 1080)),
            ),
            (1440, 420)
        );
        assert_eq!(
            resized_position(
                (100, 200),
                (240, 330),
                (480, 660),
                ScaleAnchor::BottomCenter,
                None,
            ),
            (-20, -130)
        );
        assert_eq!(
            resized_position(
                (100, 200),
                (144, 165),
                (350, 228),
                ScaleAnchor::BottomLeft,
                None,
            ),
            (100, 137)
        );
        assert_eq!(
            resized_position(
                (1000, 200),
                (144, 165),
                (350, 228),
                ScaleAnchor::BottomRight,
                None,
            ),
            (794, 137)
        );
    }

    #[test]
    fn activity_window_anchor_tracks_the_screen_half() {
        let work_area = Some((100, 0, 1200, 800));
        assert_eq!(
            edge_anchor((120, 20), (144, 165), work_area),
            ScaleAnchor::BottomLeft
        );
        assert_eq!(
            edge_anchor((1000, 20), (144, 165), work_area),
            ScaleAnchor::BottomRight
        );
    }

    /// 位置文件路径必须落在 ~/.pinvou3/ 下(跟随 PINVOU3_HOME 重定位)。
    #[test]
    fn state_path_under_pinvou3_home() {
        let _g = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", "/tmp/pinvou3-pet-path-test");
        assert_eq!(
            state_path(),
            crate::bridge::paths::pinvou3_home().join("pet_window.json")
        );
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
    }

    #[test]
    fn pet_window_state_write_reports_filesystem_failures() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("pinvou3-pet-state-{}-{unique}", std::process::id()));
        let state = PetWindowState {
            x: Some(12),
            y: Some(34),
            scale: 1.2,
            activity_visible: true,
        };
        let path = root.join("nested").join("pet_window.json");
        save_state_to(&path, &state).unwrap();
        let saved: PetWindowState =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved, state);

        let blocked_parent = root.join("not-a-directory");
        std::fs::write(&blocked_parent, "file").unwrap();
        let error = save_state_to(&blocked_parent.join("pet_window.json"), &state)
            .expect_err("a file cannot be used as the state directory");
        assert!(error.contains("create pet state directory"));
        let _ = std::fs::remove_dir_all(root);
    }
}

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

// 纯几何函数、常量与 PetVerticalAlignment/ScaleAnchor 类型抽离到 geometry 子模块;
// 这里复用其计算并保持本文件的 pub 面不变。
use super::geometry::{
    character_anchor_position, character_local_top_left, clamp_scale_to_character_work_area,
    default_scale, legacy_frame_position_to_client, pet_window_effective_size,
    scale_resize_required,
};
pub use super::geometry::{clamp_scale, point_on_any_monitor, PetVerticalAlignment, PET_LABEL};
pub(crate) use super::geometry::{edge_anchor, resized_position, ScaleAnchor};

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

/// `~/.pinvou3/pet_window.json` —— 桌宠 client 原点(全局物理像素)+ 缩放 + 竖向靠边。
/// 见 prefs::PetPrefs 注释:刻意不进 settings.json,与其他设置域保持隔离。
fn state_path() -> std::path::PathBuf {
    crate::platform::paths::pinvou3_home().join("pet_window.json")
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PetPositionSpace {
    Client,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PetWindowState {
    pub x: Option<i32>,
    pub y: Option<i32>,
    /// `None` 表示旧版本保存的 WM frame 原点；新版本一律保存 WebView client 原点。
    pub position_space: Option<PetPositionSpace>,
    #[serde(default = "default_scale")]
    pub scale: f64,
    pub activity_visible: bool,
    pub vertical_alignment: PetVerticalAlignment,
}

impl Default for PetWindowState {
    fn default() -> Self {
        Self {
            x: None,
            y: None,
            position_space: None,
            scale: default_scale(),
            activity_visible: false,
            vertical_alignment: PetVerticalAlignment::Bottom,
        }
    }
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

/// 建/显示桌宠窗口。已存在只 show(设置开关反复切换不重建 WebView)。
pub fn create_or_show(app: &AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(PET_LABEL) {
        show_and_keep_above(&win)?;
        return Ok(());
    }
    let state = load_state();
    let scale = state.scale;
    let initial_size = pet_window_effective_size(scale, state.activity_visible, None);
    let pet_url = format!(
        "pet.html?verticalAlignment={}",
        state.vertical_alignment.as_str()
    );
    let win = WebviewWindowBuilder::new(app, PET_LABEL, WebviewUrl::App(pet_url.into()))
        .title("PINVOU 桌伴公仔")
        .inner_size(initial_size.0, initial_size.1)
        // GTK 下无显式 min hint 的窗口会被钳到 ~200x200 最小尺寸(GB10 实测,
        // 菜单窗口同病):紧凑桌伴请求 144x165 实得 200x200,定位数学随之失准。
        // 96x120 覆盖 MIN_SCALE 下的最小合法尺寸,放开 GTK 的钳制。
        .min_inner_size(96.0, 120.0)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        // TAO/GTK 默认会先 map 窗口、再应用 decorations(false)。Mutter 因此可能
        // 给无边框桌伴缓存一段标题栏 frame，形成约 37px 的顶部透明空气墙。
        // 隐藏创建让无装饰属性在首次 map 前完整生效，再由下方统一定位和显示。
        .visible(false)
        .build()
        .map_err(|e| format!("build pet window: {e}"))?;

    position_window(&win);
    // Linux/X11 下 builder 在窗口 map 之前写入的 always_on_top 可能丢失，
    // 必须在窗口已显示后再向窗口管理器重申一次。
    show_and_keep_above(&win)?;
    // macOS:pet 策略必须在 show_and_keep_above 之后执行——后者调 set_always_on_top(true),
    // tao 0.35.2 在 macOS 上实现为 setLevel(NSFloatingWindowLevel=3),会覆盖策略设的
    // NSStatusWindowLevel(=25)。最后执行确保策略的更高 level 不被覆盖(桌宠浮在全屏 App 上)。
    super::platform::apply_pet_window_policy(&win);
    Ok(())
}

/// 重申置顶是对 builder 声明的增强：部分窗口管理器不支持运行时修改时，
/// 不能因此把已经成功创建/显示的桌伴当作失败。
fn keep_above(win: &tauri::WebviewWindow) {
    if let Err(error) = win.set_always_on_top(true) {
        eprintln!("[pinvou3-app] keep pet window always on top failed: {error}");
    }
}

fn show_and_keep_above(win: &tauri::WebviewWindow) -> Result<(), String> {
    win.show()
        .map_err(|error| format!("show pet window: {error}"))?;
    keep_above(win);
    Ok(())
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

    let mut st = load_state();
    if let (Some(saved_x), Some(saved_y)) = (st.x, st.y) {
        let client_position = match st.position_space {
            Some(PetPositionSpace::Client) => Some(((saved_x, saved_y), false)),
            None => {
                // 旧版直接保存 Linux/TAO onMoved 的 WM frame 原点，但 GTK 的
                // set_position 实际按 client 原点移动。只有观测到可信 inset 才
                // 迁移并落 marker；观测异常时宁可使用默认落点，也不污染旧状态。
                match (win.inner_position(), win.outer_position()) {
                    (Ok(inner), Ok(outer)) => legacy_frame_position_to_client(
                        (saved_x, saved_y),
                        (inner.x, inner.y),
                        (outer.x, outer.y),
                    )
                    .map(|position| (position, true)),
                    _ => None,
                }
            }
        };
        if let Some(((x, y), migrated)) = client_position {
            let fallback = pet_window_effective_size(st.scale, st.activity_visible, None);
            let (w, h) = win
                .inner_size()
                .map(|s| (s.width as i32, s.height as i32))
                .unwrap_or_else(|_| {
                    let sf = win.scale_factor().unwrap_or(1.0);
                    (
                        (fallback.0 * sf).round() as i32,
                        (fallback.1 * sf).round() as i32,
                    )
                });
            if point_on_any_monitor(x + w / 2, y + h / 2, &monitors) {
                let positioned = win.set_position(tauri::PhysicalPosition::new(x, y)).is_ok();
                if migrated && positioned {
                    st.x = Some(x);
                    st.y = Some(y);
                    st.position_space = Some(PetPositionSpace::Client);
                    let _ = save_state(st);
                }
                return;
            }
        }
    }
    // 默认落点:主屏右下,留边距 + 任务栏冗余。
    if let Ok(Some(m)) = win.primary_monitor() {
        let p = m.position();
        let s = m.size();
        let monitor_scale = m.scale_factor();
        let logical = pet_window_effective_size(st.scale, st.activity_visible, None);
        let w = (logical.0 * monitor_scale) as i32;
        let h = (logical.1 * monitor_scale) as i32;
        let x = p.x + s.width as i32 - w - (24.0 * monitor_scale) as i32;
        let y = p.y + s.height as i32 - h - (96.0 * monitor_scale) as i32;
        let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
    }
}

/// 启动时按 settings.json 决定是否拉起桌宠(setup 钩子里调)。
pub fn spawn_if_enabled(app: &AppHandle) {
    if crate::platform::prefs::UserPrefs::load().pet.enabled {
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
}

/// 开关桌宠:持久化 settings.json + 窗口即时显隐 + 广播给主窗口同步其 settings
/// 副本，让设置界面与专用命令写入的权威状态保持一致。
/// 设置页开关和宠物右键"隐藏"都走这一个命令,单一路径。
pub async fn set_pet_enabled(enabled: bool, app: AppHandle) -> Result<(), String> {
    let window_existed = app.get_webview_window(PET_LABEL).is_some();
    if enabled {
        create_or_show(&app)?;
    }
    let mut was_enabled = false;
    if let Err(error) = crate::platform::prefs::UserPrefs::update_transaction(|prefs| {
        was_enabled = prefs.pet.enabled;
        prefs.pet.enabled = enabled;
        Ok(())
    }) {
        if enabled && !was_enabled {
            if let Some(win) = app.get_webview_window(PET_LABEL) {
                if window_existed {
                    let _ = win.hide();
                } else {
                    let _ = win.close();
                }
            }
        }
        return Err(format!("save pet.enabled failed: {error:#}"));
    }
    if !enabled {
        if let Some(win) = app.get_webview_window(PET_LABEL) {
            let _ = win.hide();
        }
    }
    let _ = app.emit(
        "pet:enabled_changed",
        serde_json::json!({ "enabled": enabled }),
    );
    Ok(())
}

/// 前端初始化取缩放。
pub async fn get_pet_scale() -> Result<f64, String> {
    Ok(load_state().scale)
}

fn window_edge_anchor(
    win: &tauri::WebviewWindow,
    vertical_alignment: PetVerticalAlignment,
) -> ScaleAnchor {
    let position = win.inner_position().ok();
    let size = win.inner_size().ok();
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
            vertical_alignment,
        ),
        _ => match vertical_alignment {
            PetVerticalAlignment::Top => ScaleAnchor::TopCenter,
            PetVerticalAlignment::Bottom => ScaleAnchor::BottomCenter,
        },
    }
}

fn resize_pet_window(win: &tauri::WebviewWindow, logical_size: (f64, f64), anchor: ScaleAnchor) {
    // 平台差异(是否原子提交、坐标系)收敛在 platform::resize_pet_window:macOS 用
    // 单次 setFrame:display: 把尺寸与原点一并提交,消除气泡展开时人物向左上方
    // 闪现一帧的中间合成帧;其他平台退化为既有的 set_size → set_position 两步。
    super::platform::resize_pet_window(win, logical_size, anchor);
}

fn resize_pet_window_at_character_anchor(
    win: &tauri::WebviewWindow,
    logical_size: (f64, f64),
    scale: f64,
    activity_visible: bool,
    activity_height: Option<f64>,
    alignment: &str,
    vertical_alignment: PetVerticalAlignment,
    screen_anchor: (f64, f64),
) {
    let sf = win.scale_factor().unwrap_or(1.0);
    let (local_x, local_y) = character_local_top_left(
        scale,
        activity_visible,
        activity_height,
        alignment,
        vertical_alignment,
    );
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
    // 定位一律用请求值:请求值已经过 pet_window_effective_size 与 GTK 真实
    // 钳制对齐;X11 resize 异步生效,此刻回读多为旧值,不能参与定位数学。
    let _ = win.set_size(tauri::PhysicalSize::new(width, height));
    if let Ok(size) = win.inner_size() {
        if (size.width, size.height) != (width, height) {
            eprintln!(
                "[pet resize] requested {width}x{height} readback {}x{} (async, stale ok)",
                size.width, size.height
            );
        }
    }
    let (x, y) = character_anchor_position(
        (
            (screen_anchor.0 - local_x * sf).round() as i32,
            (screen_anchor.1 - local_y * sf).round() as i32,
        ),
        (width, height),
        work_area,
    );
    let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
}

/// 缩放桌宠:右下角拉伸保持人物可见区域左上角不动;右键菜单缩放保持底边中点不动。
/// 两种路径都会钳制在当前显示器工作区内。返回 clamp 后的实际值。
pub async fn set_pet_scale(
    scale: f64,
    anchor: Option<String>,
    alignment: Option<String>,
    vertical_alignment: Option<String>,
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
    let resize_required = scale_resize_required(st.scale, scale, anchor.is_some());
    st.scale = scale;
    let activity_visible = activity_visible.unwrap_or(st.activity_visible);
    let vertical_alignment = vertical_alignment
        .as_deref()
        .map(PetVerticalAlignment::from_str)
        .unwrap_or(st.vertical_alignment);
    st.activity_visible = activity_visible;
    st.vertical_alignment = vertical_alignment;
    if persist.unwrap_or(true) {
        save_state(st)?;
    }
    // 启动时活动可见性和缩放状态会由两个 React effect 紧邻上报。X11 resize
    // 异步生效，如果缩放值根本没变却再次按旧尺寸改位置，两次锚定会竞态，
    // 让右侧公仔每次启动随机横移半个宽差。活动显隐由专用命令负责；这里
    // 只有真实缩放变化（或显式锚点）才修改原生窗口几何。
    if let Some(win) = win.filter(|_| resize_required) {
        let logical_size = pet_window_effective_size(scale, activity_visible, activity_height);
        if let Some(character_anchor) = character_anchor {
            resize_pet_window_at_character_anchor(
                &win,
                logical_size,
                scale,
                activity_visible,
                activity_height,
                alignment.as_deref().unwrap_or("right"),
                vertical_alignment,
                character_anchor,
            );
        } else {
            let scale_anchor = if anchor.as_deref() == Some("top_left") {
                ScaleAnchor::TopLeft
            } else if activity_visible {
                window_edge_anchor(&win, vertical_alignment)
            } else if vertical_alignment == PetVerticalAlignment::Top {
                ScaleAnchor::TopCenter
            } else {
                ScaleAnchor::BottomCenter
            };
            resize_pet_window(&win, logical_size, scale_anchor);
        }
    }
    Ok(scale)
}
pub async fn set_pet_activity_visible(
    visible: bool,
    activity_height: Option<f64>,
    alignment: Option<String>,
    vertical_alignment: Option<String>,
    app: AppHandle,
) -> Result<(), String> {
    let mut st = load_state();
    let vertical_alignment = vertical_alignment
        .as_deref()
        .map(PetVerticalAlignment::from_str)
        .unwrap_or(st.vertical_alignment);
    // 高度流式上报会频繁进入本命令:可见性没变时跳过写盘,
    // 别让每次窗口微调都附带一次同步磁盘 IO。
    if st.activity_visible != visible || st.vertical_alignment != vertical_alignment {
        st.activity_visible = visible;
        st.vertical_alignment = vertical_alignment;
        save_state(st)?;
    }
    if let Some(win) = app.get_webview_window(PET_LABEL) {
        let logical_size = pet_window_effective_size(st.scale, visible, activity_height);
        // 人物在窗口内贴当前竖向边与横向对齐侧(CSS 与
        // character_local_top_left 一致)。活动卡显隐改变窗口尺寸时，
        // 同时保住这两条边，人物在屏幕上就不会跳。
        // 贴边方向必须用前端的实际对齐值:按窗口中心猜测在屏幕中部会猜反,
        // 这正是收起时人物瞬移的原始根因。
        let anchor = match (alignment.as_deref(), vertical_alignment) {
            (Some("left"), PetVerticalAlignment::Top) => ScaleAnchor::TopLeft,
            (Some(_), PetVerticalAlignment::Top) => ScaleAnchor::TopRight,
            (Some("left"), PetVerticalAlignment::Bottom) => ScaleAnchor::BottomLeft,
            (Some(_), PetVerticalAlignment::Bottom) => ScaleAnchor::BottomRight,
            (None, vertical_alignment) => window_edge_anchor(&win, vertical_alignment),
        };
        resize_pet_window(&win, logical_size, anchor);
    }
    Ok(())
}

/// 桌宠窗口拖动落定后保存 client 原点(前端 onMoved 防抖后重新读取,全局物理像素)。
pub async fn save_pet_position(
    x: i32,
    y: i32,
    vertical_alignment: Option<String>,
) -> Result<(), String> {
    let mut st = load_state(); // 保留 scale
    st.x = Some(x);
    st.y = Some(y);
    st.position_space = Some(PetPositionSpace::Client);
    if let Some(vertical_alignment) = vertical_alignment.as_deref() {
        st.vertical_alignment = PetVerticalAlignment::from_str(vertical_alignment);
    }
    save_state(st)
}
pub async fn save_pet_vertical_alignment(alignment: String) -> Result<(), String> {
    let mut st = load_state();
    let alignment = PetVerticalAlignment::from_str(&alignment);
    if st.vertical_alignment != alignment {
        st.vertical_alignment = alignment;
        save_state(st)?;
    }
    Ok(())
}

/// 点击宠物时唤醒主窗口；点击活动时额外把目标 session 路由给主窗口。
/// 会话切换仍由现有 TauriBridge/Session 实现，这里只负责原生窗口与导航消息。
pub async fn open_main_from_pet(
    session_id: Option<String>,
    scheduled_run: Option<PetScheduledRunNavigation>,
    navigation: State<'_, PetNavigationState>,
    app: AppHandle,
) -> Result<(), String> {
    // 诊断走终端:此命令的失败只会回到 pet 窗口的 JS console,无 inspector 时
    // 不可见——X11 上 show/set_focus 被 WM 拒绝时用户只看到"点了没反应"。
    eprintln!("[pet nav] open_main_from_pet invoked");
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())
        .map_err(|error| {
            eprintln!("[pet nav] {error}");
            error
        })?;
    // GB10 实测:主窗口最小化时 unminimize 返回 Ok 但 mutter 拒绝 deiconify
    // (焦点抢占保护),窗口召不回来。withdraw+remap 等价于新窗口映射,WM 必须
    // 显示——仅在确实最小化时走这条路,避免可见窗口无谓闪一下。
    let was_minimized = main.is_minimized().unwrap_or(false);
    eprintln!("[pet nav] main minimized={was_minimized}");
    if was_minimized {
        let _ = main.hide();
    }
    main.show().map_err(|error| {
        let msg = format!("show main window failed: {error}");
        eprintln!("[pet nav] {msg}");
        msg
    })?;
    main.unminimize().map_err(|error| {
        let msg = format!("unminimize main window failed: {error}");
        eprintln!("[pet nav] {msg}");
        msg
    })?;
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
    // X11 焦点抢占保护:实测(GB10) show/unminimize/set_focus 全部返回 Ok,
    // 但 WM 拒绝把主窗口提到前台,只打 demand-attention——用户看到"点了没反应"。
    // raise 不受焦点保护限制:瞬时置顶把窗口强制提前。取消置顶不能紧跟
    // set_focus 同步执行;X11/Mutter 上主窗口可能尚未完成激活,立刻撤销会让
    // 激活点击落到下层窗口(常见表现:下层窗口收到点击后 Pinvou 又最小化)。
    super::platform::prepare_main_focus_raise(&app);
    let _ = main.set_always_on_top(true);
    let focus_result = main.set_focus().map_err(|error| {
        let msg = format!("focus main window failed: {error}");
        eprintln!("[pet nav] {msg}");
        msg
    });
    super::platform::finish_main_focus_raise(&main);
    focus_result?;
    app.emit_to("main", "pet:navigation_pending", ())
        .map_err(|error| format!("emit pet navigation wakeup failed: {error}"))?;
    eprintln!("[pet nav] open_main_from_pet ok");
    Ok(())
}
pub async fn take_pet_navigation(
    navigation: State<'_, PetNavigationState>,
) -> Result<Option<PetNavigationRequest>, String> {
    navigation.take()
}
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
pub async fn take_pet_reply(
    replies: State<'_, PetReplyState>,
) -> Result<Option<PetReplyRequest>, String> {
    replies.take()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pet_state_serde_roundtrip_and_legacy() {
        let st = PetWindowState {
            x: Some(-120),
            y: Some(3456),
            position_space: Some(PetPositionSpace::Client),
            scale: 1.3,
            activity_visible: true,
            vertical_alignment: PetVerticalAlignment::Top,
        };
        let s = serde_json::to_string(&st).unwrap();
        let back: PetWindowState = serde_json::from_str(&s).unwrap();
        assert_eq!(st, back);
        // 旧版文件只有 x/y(无 scale)→ 回当前默认(最小尺寸),同 default_scale()
        let legacy: PetWindowState = serde_json::from_str(r#"{"x":10,"y":20}"#).unwrap();
        assert_eq!(legacy.scale, default_scale());
        assert_eq!(legacy.x, Some(10));
        assert_eq!(legacy.position_space, None);
        assert!(!legacy.activity_visible);
        assert_eq!(legacy.vertical_alignment, PetVerticalAlignment::Bottom);
        // 空文件/缺字段 → 全默认；「默认 scale 即最小档」的等价断言在
        // first_launch_defaults_to_minimum_scale(经 geometry 最小窗口尺寸钉住)。
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
    fn first_launch_defaults_to_minimum_scale() {
        let state = PetWindowState::default();
        assert_eq!(state.scale, default_scale());

        let deserialized: PetWindowState = serde_json::from_str("{}").unwrap();
        assert_eq!(deserialized.scale, default_scale());
        // 最小尺寸下的窗口大小由 geometry 子模块覆盖测试(见
        // pet_window_size_keeps_activity_cards_readable_at_every_pet_scale)。
        assert_eq!(
            super::super::geometry::pet_window_logical_size(state.scale, false, None),
            (144.0, 165.0)
        );
    }

    /// 位置文件路径必须落在 ~/.pinvou3/ 下(跟随 PINVOU3_HOME 重定位)。
    #[test]
    fn state_path_under_pinvou3_home() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", "/tmp/pinvou3-pet-path-test");
        assert_eq!(
            state_path(),
            crate::platform::paths::pinvou3_home().join("pet_window.json")
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
            position_space: Some(PetPositionSpace::Client),
            scale: 1.2,
            activity_visible: true,
            vertical_alignment: PetVerticalAlignment::Top,
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

//! 撕离窗口（tear-off）：把某个左侧菜单项弹成独立 WebviewWindow。
//! 模式照搬 commands::open_artifact_window（label 去重 + 聚焦已存在窗口）。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

/// 同一时刻只允许一个撕离拖拽,防止多个跟随循环并存。
static DRAG_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 撕离窗口 label。Tauri label 仅允许 a-zA-Z0-9-_，故 id 用 16 位 hex 哈希而非原样拼接，
/// 避免 id 里的非法字符 / 冲突。同一 (kind,id) → 同一 label，用于去重 + 聚焦。
pub fn detached_label(kind: &str, id: Option<&str>) -> String {
    let mut h = DefaultHasher::new();
    id.unwrap_or("").hash(&mut h);
    format!("detached-{kind}-{:016x}", h.finish())
}

/// kind → 窗口标题。未知 kind 退化为通用标题。
pub fn view_title(kind: &str) -> &'static str {
    match kind {
        "session" => "对话",
        "persona" => "专家",
        "workflow" => "工作流",
        "monitor" => "系统监控",
        "toolstore" => "工具商店",
        "cardpool" => "专家卡牌池",
        "localenv" => "本地环境",
        _ => "PINVOU",
    }
}

/// 点 (px,py) 是否落在矩形 [x, x+w) × [y, y+h) 内(物理像素，全局虚拟桌面坐标)。
/// 撕离落位判定用:松手点在主窗口外接矩形外 → 建窗;在内 → 取消。
pub fn point_in_rect(px: i32, py: i32, x: i32, y: i32, w: i32, h: i32) -> bool {
    px >= x && px < x + w && py >= y && py < y + h
}

/// 极简 URL 编码：只转义 query 里会出问题的字符，足够 kind/id 用。
fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// 建/聚焦撕离窗口的核心。已存在同 (kind,id) 窗口则只聚焦。
/// pos=Some 时建好后把窗口左上角移到全局物理坐标(拖拽松手落位,跨屏)。
/// 撕离窗口加载同一个 index.html，带 ?detached=1&kind=&id=，前端据此只渲染该面板。
pub fn create_detached_at(
    app: &AppHandle,
    kind: &str,
    id: Option<&str>,
    pos: Option<(i32, i32)>,
) -> Result<(), String> {
    let label = detached_label(kind, id);
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.set_focus();
        return Ok(());
    }

    // UI schema 版本戳与主窗口一致，避免撕离窗口命中跨版本旧 HTML。
    // id 做 URL 编码，空 id 省略。
    let mut query = format!(
        "ui={}&detached=1&kind={}",
        crate::platform::ui_cache::UI_CACHE_SCHEMA,
        urlencode(kind)
    );
    if let Some(i) = id {
        query.push_str(&format!("&id={}", urlencode(i)));
    }
    let url = WebviewUrl::App(format!("index.html?{query}").into());

    let win = WebviewWindowBuilder::new(app, &label, url)
        .title(view_title(kind))
        .inner_size(900.0, 720.0)
        .resizable(true)
        .decorations(true)
        .build()
        .map_err(|e| format!("build detached window: {e}"))?;

    // 用 PhysicalPosition 落位:device_query 给的是全局物理像素,绕开 logical/scale 换算。
    // 落位即"全屏":先放到目标屏,再 maximize → 填满该显示器(保留标题栏可关)。
    if let Some((x, y)) = pos {
        let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
        let _ = win.maximize();
    }
    Ok(())
}

/// 建/聚焦某菜单项的撕离窗口(按钮触发,默认位置)。
pub async fn open_detached_window(
    kind: String,
    id: Option<String>,
    app: AppHandle,
) -> Result<(), String> {
    create_detached_at(&app, &kind, id.as_deref(), None)
}

/// 主窗口外接矩形是否包含全局点 (px,py)。拿不到主窗口几何 → 视为不包含(倾向于建窗)。
fn main_window_contains(app: &AppHandle, px: i32, py: i32) -> bool {
    if let Some(w) = app.get_webview_window("main") {
        if let (Ok(pos), Ok(size)) = (w.outer_position(), w.outer_size()) {
            return point_in_rect(px, py, pos.x, pos.y, size.width as i32, size.height as i32);
        }
    }
    false
}

/// 撕离拖拽起手:原生层只负责"读全局光标+左键、判松手落点"。视觉跟随由前端 DOM avatar 完成
/// (在主窗内丝滑跟手,WM 无关、无文字选中)。本函数松手时按全局落点决定建窗(主窗外那一屏
/// 最大化)或取消,并广播 detach:drag-ended 让前端收起 avatar。
pub async fn begin_detach_drag(
    kind: String,
    id: Option<String>,
    app: AppHandle,
) -> Result<(), String> {
    if DRAG_ACTIVE.swap(true, Ordering::SeqCst) {
        return Ok(()); // 已有拖拽进行中,忽略重复起手
    }

    // device_query 硬件状态轮询(独立 OS 线程);窗口操作 marshal 回主线程。
    std::thread::spawn(move || {
        use device_query::{DeviceQuery, DeviceState};
        let dev = DeviceState::new();
        let mut was_down = false;
        let mut idle_ticks = 0u32;
        loop {
            let m = dev.get_mouse();
            let (mx, my) = m.coords;
            let down = *m.button_pressed.get(1).unwrap_or(&false);

            if down {
                was_down = true;
            }
            if was_down && !down {
                // 松手:落点在主窗外那一屏 → 最大化建窗;在内 → 取消。
                let a2 = app.clone();
                let kind2 = kind.clone();
                let id2 = id.clone();
                let _ = app.run_on_main_thread(move || {
                    if !main_window_contains(&a2, mx, my) {
                        let _ = create_detached_at(&a2, &kind2, id2.as_deref(), Some((mx, my)));
                    }
                });
                break;
            }
            if !was_down {
                idle_ticks += 1;
                if idle_ticks > 250 {
                    break; // ~3s 没等到按下(异常起手)→ 放弃
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(12));
        }
        // 拖拽结束(落位/取消/超时任一)→ 广播,让前端收起 avatar。
        let _ = app.emit("detach:drag-ended", ());
        DRAG_ACTIVE.store(false, Ordering::SeqCst);
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_is_sanitized_and_stable() {
        let a = detached_label("session", Some("s-../etc/passwd 你好"));
        assert!(a.starts_with("detached-session-"));
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
        // 同输入稳定（去重/聚焦依赖此性质）
        assert_eq!(a, detached_label("session", Some("s-../etc/passwd 你好")));
    }

    #[test]
    fn label_differs_by_id_and_kind() {
        assert_ne!(
            detached_label("session", Some("a")),
            detached_label("session", Some("b"))
        );
        assert_ne!(
            detached_label("session", Some("a")),
            detached_label("workflow", Some("a"))
        );
        assert_ne!(
            detached_label("monitor", None),
            detached_label("toolstore", None)
        );
    }

    #[test]
    fn view_title_known_and_fallback() {
        assert_eq!(view_title("workflow"), "工作流");
        assert_eq!(view_title("???"), "PINVOU");
    }

    #[test]
    fn urlencode_escapes_unsafe() {
        assert_eq!(urlencode("a-b_1.~"), "a-b_1.~");
        assert_eq!(urlencode("a b&c=d"), "a%20b%26c%3Dd");
    }

    #[test]
    fn point_in_rect_basic() {
        assert!(point_in_rect(10, 10, 0, 0, 100, 100));
        assert!(!point_in_rect(100, 10, 0, 0, 100, 100)); // 右边界开区间
        assert!(!point_in_rect(-1, 10, 0, 0, 100, 100));
        assert!(point_in_rect(2000, 50, 1920, 0, 1920, 1080)); // 第二屏
    }

    #[test]
    fn detached_url_contract_uses_ui_cache_schema() {
        let query = format!(
            "ui={}&detached=1&kind={}",
            crate::platform::ui_cache::UI_CACHE_SCHEMA,
            urlencode("workflow")
        );
        assert_eq!(query, "ui=vite-react-1&detached=1&kind=workflow");
    }
}

//! 撕离窗口（tear-off）：把某个左侧菜单项弹成独立 WebviewWindow。
//! 模式照搬 commands::open_artifact_window（label 去重 + 聚焦已存在窗口）。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

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

/// 建/聚焦某菜单项的撕离窗口。已存在同 (kind,id) 窗口则只聚焦。
/// 撕离窗口加载同一个 index.html，带 ?detached=1&kind=&id=，前端据此只渲染该面板。
#[tauri::command]
pub async fn open_detached_window(
    kind: String,
    id: Option<String>,
    app: AppHandle,
) -> Result<(), String> {
    let label = detached_label(&kind, id.as_deref());
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.set_focus();
        return Ok(());
    }

    // index.html?detached=1&kind=<kind>&id=<id>。id 做 URL 编码，空 id 省略。
    let mut query = format!("detached=1&kind={}", urlencode(&kind));
    if let Some(ref i) = id {
        query.push_str(&format!("&id={}", urlencode(i)));
    }
    let url = WebviewUrl::App(format!("index.html?{query}").into());

    WebviewWindowBuilder::new(&app, &label, url)
        .title(view_title(&kind))
        .inner_size(900.0, 720.0)
        .resizable(true)
        .decorations(true)
        .build()
        .map_err(|e| format!("build detached window: {e}"))?;
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
}

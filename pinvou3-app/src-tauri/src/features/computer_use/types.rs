//! Computer Use 核心类型：动作枚举、错误、后端结果结构、按键和弦解析。
//!
//! 坐标契约（全模块唯一事实来源）：模型看到的坐标永远是「截图空间」——
//! 工具最近一次返回的 PNG 的像素空间，原点在左上。截图空间 → 设备物理像素 →
//! 输入注入坐标的换算集中在 [`crate::features::computer_use::scaling::ScaleMap`]，
//! 本文件只定义承载这些空间的结构。

use serde::Serialize;

/// 工具名（单一 ToolSpec，`action` 字段区分动作）。
pub const TOOL_NAME: &str = "computer_use";
/// 输入类动作缺少会话授权时发出的 Tauri 事件。
pub const EVENT_GRANT_REQUIRED: &str = "computer_use:grant_required";
/// T3 后果性动作被拦截、等待用户确认时发出的 Tauri 事件。
pub const EVENT_CONFIRM_REQUIRED: &str = "computer_use:confirm_required";

/// 动作同意层级（consent tier）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionClass {
    /// 只读观察：screenshot / cursor_position / wait / ui_tree / element_at_point。
    Observe,
    /// 不改变焦点/状态的指针动作：mouse_move / scroll。
    Passive,
    /// 真实输入注入：点击、拖拽、键盘、文本。
    Input,
}

impl ActionClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::Passive => "passive",
            Self::Input => "input",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

impl MouseButton {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Middle => "middle",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollDirection {
    Up,
    Down,
    Left,
    Right,
}

impl ScrollDirection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Left => "left",
            Self::Right => "right",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            _ => None,
        }
    }
}

/// 归一化后的按键身份。`key` / `hold_key` 的 xdotool 风格和弦（"ctrl+s"、
/// "Return"、"alt+Tab"）解析成一组 `Key`，后端按下全部再逆序释放。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Key {
    Control,
    Alt,
    Shift,
    /// macOS Cmd / Windows Win / Linux Super。
    Meta,
    Enter,
    Escape,
    Tab,
    Space,
    Backspace,
    Delete,
    Insert,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// F1..=F12。
    Function(u8),
    /// 单字符键（字母、数字、标点）。
    Char(char),
}

impl Key {
    /// 规范化显示名（用于审计与错误信息）。
    pub fn canonical_name(self) -> String {
        match self {
            Self::Control => "ctrl".to_string(),
            Self::Alt => "alt".to_string(),
            Self::Shift => "shift".to_string(),
            Self::Meta => "meta".to_string(),
            Self::Enter => "enter".to_string(),
            Self::Escape => "esc".to_string(),
            Self::Tab => "tab".to_string(),
            Self::Space => "space".to_string(),
            Self::Backspace => "backspace".to_string(),
            Self::Delete => "delete".to_string(),
            Self::Insert => "insert".to_string(),
            Self::Up => "up".to_string(),
            Self::Down => "down".to_string(),
            Self::Left => "left".to_string(),
            Self::Right => "right".to_string(),
            Self::Home => "home".to_string(),
            Self::End => "end".to_string(),
            Self::PageUp => "pageup".to_string(),
            Self::PageDown => "pagedown".to_string(),
            Self::Function(n) => format!("f{n}"),
            Self::Char(c) => c.to_string(),
        }
    }

    fn is_modifier(self) -> bool {
        matches!(self, Self::Control | Self::Alt | Self::Shift | Self::Meta)
    }
}

fn parse_key_token(token: &str) -> Option<Key> {
    let lower = token.to_ascii_lowercase();
    let named = match lower.as_str() {
        "ctrl" | "control" => Key::Control,
        "alt" | "option" => Key::Alt,
        "shift" => Key::Shift,
        "cmd" | "command" | "meta" | "super" | "win" | "windows" => Key::Meta,
        "enter" | "return" => Key::Enter,
        "esc" | "escape" => Key::Escape,
        "tab" => Key::Tab,
        "space" => Key::Space,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "insert" | "ins" => Key::Insert,
        "up" | "arrowup" => Key::Up,
        "down" | "arrowdown" => Key::Down,
        "left" | "arrowleft" => Key::Left,
        "right" | "arrowright" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "pgup" => Key::PageUp,
        "pagedown" | "pgdn" => Key::PageDown,
        "plus" => return Some(Key::Char('+')),
        "minus" => return Some(Key::Char('-')),
        "comma" => return Some(Key::Char(',')),
        "period" => return Some(Key::Char('.')),
        _ => {
            if let Some(digits) = lower.strip_prefix('f') {
                if !digits.is_empty() {
                    if let Ok(n) = digits.parse::<u8>() {
                        if (1..=12).contains(&n) {
                            return Some(Key::Function(n));
                        }
                    }
                    return None;
                }
            }
            // 单字符键：归一到小写（大小写不敏感；要按大写 "S" 用 shift+s）。
            let mut chars = lower.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => return Some(Key::Char(c)),
                _ => return None,
            }
        }
    };
    Some(named)
}

/// 解析 xdotool 风格和弦："ctrl+s"、"Return"、"alt+Tab"、"shift+f5"。
/// 大小写不敏感；同义词表见 `parse_key_token`。
pub fn parse_key_chord(text: &str) -> Result<Vec<Key>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("key chord text must not be empty".to_string());
    }
    let mut keys = Vec::new();
    for token in trimmed.split('+') {
        let token = token.trim();
        if token.is_empty() {
            return Err(format!(
                "invalid key chord '{text}': empty key between '+' separators"
            ));
        }
        let key = parse_key_token(token)
            .ok_or_else(|| format!("invalid key chord '{text}': unknown key '{token}'"))?;
        keys.push(key);
    }
    if keys.iter().all(|key| key.is_modifier()) {
        return Err(format!(
            "invalid key chord '{text}': a chord needs at least one non-modifier key"
        ));
    }
    Ok(keys)
}

/// 后端能力声明。stub 后端全部 false；真实后端按平台实际支持填写。
#[derive(Debug, Clone)]
pub struct Capabilities {
    pub screenshot: bool,
    pub input: bool,
    pub ui_tree: bool,
    /// 自由文本说明（如 "input unavailable: no portal authorization"）。
    pub notes: String,
}

/// 一次屏幕捕获的原始结果（设备物理像素）。
///
/// - `rgba`：宽度×高度×4 字节的 RGBA 像素。
/// - `origin_x/origin_y`：显示器原点，单位是**输入坐标空间**
///   （Windows/X11 为物理像素；macOS 为 CGEvent 点）。
/// - `input_scale_x/input_scale_y`：设备物理像素 → 输入坐标的倍率。
///   Windows/X11 为 1.0；macOS 为 1/backing_scale_factor（Retina 2x 时为 0.5）。
#[derive(Debug, Clone)]
pub struct Capture {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub origin_x: i32,
    pub origin_y: i32,
    pub input_scale_x: f64,
    pub input_scale_y: f64,
}

impl Capture {
    /// 设备物理像素 → 输入坐标倍率（每平台规则编码于此）。
    pub fn input_scale(&self) -> (f64, f64) {
        (self.input_scale_x, self.input_scale_y)
    }
}

/// `element_at_point` 命中的 UI 元素（坐标为输入坐标空间）。
#[derive(Debug, Clone, Serialize)]
pub struct ElementInfo {
    pub role: String,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// 密码框/安全文本字段（T3 强制确认信号之一）。
    pub secure: bool,
}

/// `ui_tree` 抓取选项。`None` 由后端给默认值。
#[derive(Debug, Clone, Copy, Default)]
pub struct UiTreeOptions {
    pub max_depth: Option<u32>,
    pub max_nodes: Option<u32>,
}

/// 后端错误。平台缺少能力时必须显式 `Unsupported`，不得静默降级。
#[derive(Debug, Clone)]
pub enum ComputerUseError {
    /// 该平台/会话不支持此能力（如 portal 未提供 RemoteDesktop、未实现的后端）。
    Unsupported {
        capability: &'static str,
        detail: String,
    },
    /// 能力原则上支持但当前不可用（权限未授予、显示器句柄失效等）。
    Unavailable { detail: String },
    /// 执行失败。
    Failed { detail: String },
}

impl ComputerUseError {
    pub fn unsupported(capability: &'static str, detail: impl Into<String>) -> Self {
        Self::Unsupported {
            capability,
            detail: detail.into(),
        }
    }

    pub fn unavailable(detail: impl Into<String>) -> Self {
        Self::Unavailable {
            detail: detail.into(),
        }
    }

    pub fn failed(detail: impl Into<String>) -> Self {
        Self::Failed {
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for ComputerUseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported { capability, detail } => {
                write!(f, "unsupported: {capability}: {detail}")
            }
            Self::Unavailable { detail } => write!(f, "unavailable: {detail}"),
            Self::Failed { detail } => write!(f, "failed: {detail}"),
        }
    }
}

impl std::error::Error for ComputerUseError {}

/// 解析校验后的工具调用。
#[derive(Debug, Clone)]
pub enum ComputerUseAction {
    Screenshot,
    CursorPosition,
    Wait {
        ms: u64,
    },
    UiTree {
        opts: UiTreeOptions,
    },
    ElementAtPoint {
        x: i64,
        y: i64,
    },
    MouseMove {
        x: i64,
        y: i64,
    },
    Scroll {
        direction: ScrollDirection,
        amount: u32,
        at: Option<(i64, i64)>,
    },
    /// count: 1=单击 2=双击 3=三击。
    Click {
        button: MouseButton,
        count: u8,
        at: Option<(i64, i64)>,
    },
    MouseDown {
        button: MouseButton,
    },
    MouseUp {
        button: MouseButton,
    },
    Drag {
        start: (i64, i64),
        end: (i64, i64),
    },
    Type {
        text: String,
    },
    KeyChord {
        keys: Vec<Key>,
        chord: String,
    },
    HoldKey {
        keys: Vec<Key>,
        chord: String,
        ms: u64,
    },
}

impl ComputerUseAction {
    pub fn class(&self) -> ActionClass {
        match self {
            Self::Screenshot
            | Self::CursorPosition
            | Self::Wait { .. }
            | Self::UiTree { .. }
            | Self::ElementAtPoint { .. } => ActionClass::Observe,
            Self::MouseMove { .. } | Self::Scroll { .. } => ActionClass::Passive,
            Self::Click { .. }
            | Self::MouseDown { .. }
            | Self::MouseUp { .. }
            | Self::Drag { .. }
            | Self::Type { .. }
            | Self::KeyChord { .. }
            | Self::HoldKey { .. } => ActionClass::Input,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Screenshot => "screenshot",
            Self::CursorPosition => "cursor_position",
            Self::Wait { .. } => "wait",
            Self::UiTree { .. } => "ui_tree",
            Self::ElementAtPoint { .. } => "element_at_point",
            Self::MouseMove { .. } => "mouse_move",
            Self::Scroll { .. } => "scroll",
            Self::Click {
                button: MouseButton::Left,
                count: 1,
                ..
            } => "left_click",
            Self::Click {
                button: MouseButton::Right,
                ..
            } => "right_click",
            Self::Click {
                button: MouseButton::Middle,
                ..
            } => "middle_click",
            Self::Click { count: 2, .. } => "double_click",
            Self::Click { count: 3, .. } => "triple_click",
            Self::Click { .. } => "left_click",
            Self::MouseDown { .. } => "left_mouse_down",
            Self::MouseUp { .. } => "left_mouse_up",
            Self::Drag { .. } => "left_click_drag",
            Self::Type { .. } => "type",
            Self::KeyChord { .. } => "key",
            Self::HoldKey { .. } => "hold_key",
        }
    }

    /// 动作是否携带截图空间坐标（需要 ScaleMap 才能执行）。
    pub fn needs_scale_map(&self) -> bool {
        match self {
            Self::ElementAtPoint { .. } | Self::MouseMove { .. } | Self::Drag { .. } => true,
            Self::Scroll { at, .. } | Self::Click { at, .. } => at.is_some(),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_chord_parses_synonyms_case_insensitively() {
        assert_eq!(
            parse_key_chord("ctrl+s").as_deref(),
            Ok(&[Key::Control, Key::Char('s')][..])
        );
        assert_eq!(
            parse_key_chord("CTRL+S").as_deref(),
            Ok(&[Key::Control, Key::Char('s')][..])
        );
        assert_eq!(
            parse_key_chord("Control+Shift+F5").as_deref(),
            Ok(&[Key::Control, Key::Shift, Key::Function(5)][..])
        );
        assert_eq!(parse_key_chord("Return").as_deref(), Ok(&[Key::Enter][..]));
        assert_eq!(parse_key_chord("ENTER").as_deref(), Ok(&[Key::Enter][..]));
        assert_eq!(parse_key_chord("esc").as_deref(), Ok(&[Key::Escape][..]));
        assert_eq!(
            parse_key_chord("alt+Tab").as_deref(),
            Ok(&[Key::Alt, Key::Tab][..])
        );
        for meta in ["cmd", "command", "meta", "super", "win"] {
            assert_eq!(
                parse_key_chord(&format!("{meta}+c")).as_deref(),
                Ok(&[Key::Meta, Key::Char('c')][..]),
                "{meta} should map to Meta"
            );
        }
        assert_eq!(parse_key_chord("arrowup").as_deref(), Ok(&[Key::Up][..]));
        assert_eq!(parse_key_chord("pgdn").as_deref(), Ok(&[Key::PageDown][..]));
        assert_eq!(
            parse_key_chord("ctrl+plus").as_deref(),
            Ok(&[Key::Control, Key::Char('+')][..])
        );
    }

    #[test]
    fn key_chord_rejects_invalid_input() {
        assert!(parse_key_chord("").is_err());
        assert!(parse_key_chord("   ").is_err());
        assert!(parse_key_chord("ctrl+").is_err());
        assert!(parse_key_chord("+s").is_err());
        assert!(parse_key_chord("ctrl+nosuchkey").is_err());
        assert!(parse_key_chord("f13").is_err());
        assert!(parse_key_chord("f0").is_err());
        // 纯修饰键不构成一次按键。
        assert!(parse_key_chord("ctrl+shift").is_err());
        // 多字符且不在同义词表。
        assert!(parse_key_chord("ctrl+ab").is_err());
    }

    #[test]
    fn action_classification_matches_consent_tiers() {
        assert_eq!(ComputerUseAction::Screenshot.class(), ActionClass::Observe);
        assert_eq!(
            ComputerUseAction::Wait { ms: 100 }.class(),
            ActionClass::Observe
        );
        assert_eq!(
            ComputerUseAction::MouseMove { x: 1, y: 2 }.class(),
            ActionClass::Passive
        );
        assert_eq!(
            ComputerUseAction::Scroll {
                direction: ScrollDirection::Down,
                amount: 3,
                at: None,
            }
            .class(),
            ActionClass::Passive
        );
        assert_eq!(
            ComputerUseAction::Click {
                button: MouseButton::Left,
                count: 1,
                at: None,
            }
            .class(),
            ActionClass::Input
        );
        assert_eq!(
            ComputerUseAction::Type { text: "x".into() }.class(),
            ActionClass::Input
        );
    }
}

//! Computer Use core types: action enums, errors, backend result structs, key chord parsing.
//!
//! Coordinate contract (the single source of truth for the whole module): the coordinates the
//! model sees are always "screenshot space" — the pixel space of the PNG the tool most
//! recently returned, origin at the top-left. The conversion screenshot space → device
//! physical pixels → input injection coordinates is centralized in
//! [`crate::features::computer_use::scaling::ScaleMap`]; this file only defines the structs
//! carrying those spaces.

use serde::Serialize;

/// Tool name (a single ToolSpec; the `action` field distinguishes actions).
pub const TOOL_NAME: &str = "computer_use";
/// Tauri event emitted when an input action lacks session authorization.
pub const EVENT_GRANT_REQUIRED: &str = "computer_use:grant_required";
/// Tauri event emitted when a T3 consequential action is intercepted, awaiting user confirmation.
pub const EVENT_CONFIRM_REQUIRED: &str = "computer_use:confirm_required";

/// Action consent tier.
///
/// Single source of truth: `class()` decides the gating strength, and every
/// derived check defers to it — `requires_t3_check` (tool.rs) is "Input
/// class minus the hover/scroll no-gate set", so the tier table and the
/// screening gate cannot drift apart when a new action variant is added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionClass {
    /// Read-only observation: screenshot / cursor_position / wait / ui_tree / element_at_point.
    Observe,
    /// Real input injection: mouse move/wheel/click/press-release/drag/keyboard/text — all
    /// require session authorization. mouse_move and scroll likewise actually touch the
    /// user's pointer device, gated at the same tier as clicks (review correction: the
    /// former Passive tier allowed moving the user's cursor/scrolling the wheel without
    /// authorization).
    Input,
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
        // Case-insensitive (review finding: the key/chord parser is
        // case-insensitive; the scroll parser silently wasn't, so `"Up"`
        // errored for no reason).
        match text.to_ascii_lowercase().as_str() {
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            _ => None,
        }
    }
}

/// The normalized key identity. The xdotool-style chords of `key` / `hold_key` ("ctrl+s",
/// "Return", "alt+Tab") parse into a set of `Key`s; the backend presses all of them then
/// releases in reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Key {
    Control,
    Alt,
    Shift,
    /// macOS Cmd / Windows Win / Linux Super.
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
    /// F1..=F12.
    Function(u8),
    /// Single-character key (letters, digits, punctuation).
    Char(char),
}

impl Key {
    /// Canonical display name (used for audit and error messages).
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

    pub(crate) fn is_modifier(self) -> bool {
        matches!(self, Self::Control | Self::Alt | Self::Shift | Self::Meta)
    }
}

/// Token cap for a single chord. Legitimate shortcuts have at most 4 keys
/// (ctrl+shift+alt+f1); longer sequences are almost certainly the model typing text
/// character by character via the key action (review finding: with no cap, calls like
/// `key "p"` could bypass type's screening and audit policy).
pub const MAX_KEY_CHORD_TOKENS: usize = 4;

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
            // Single-character key: normalize to lowercase (case-insensitive; to press an
            // uppercase "S" use shift+s). Control characters are not pressable key positions
            // and are refused (review finding: injecting invisible characters like NUL/ESC
            // through the key action is both meaningless and hard to audit).
            let mut chars = lower.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if !c.is_control() => return Some(Key::Char(c)),
                _ => return None,
            }
        }
    };
    Some(named)
}

/// Parse an xdotool-style chord: "ctrl+s", "Return", "alt+Tab", "shift+f5".
/// Case-insensitive; see `parse_key_token` for the synonym table. The token count is capped
/// at [`MAX_KEY_CHORD_TOKENS`].
///
/// Parse errors describe the chord's SHAPE only and never echo the input
/// text: the error string is written into the audit log's parse-failure
/// record, and a chord can carry secret content (a model probing
/// `{"action":"key","text":"hunter2"}` must not write that string into the
/// JSONL). The model knows its own input, so nothing is lost.
pub fn parse_key_chord(text: &str) -> Result<Vec<Key>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("key chord text must not be empty".to_string());
    }
    let mut keys = Vec::new();
    for token in trimmed.split('+') {
        if keys.len() >= MAX_KEY_CHORD_TOKENS {
            return Err(format!(
                "key chord has more than {MAX_KEY_CHORD_TOKENS} keys; \
                 use the type action for text input"
            ));
        }
        let token = token.trim();
        if token.is_empty() {
            return Err("key chord has an empty segment between '+' separators".to_string());
        }
        let key = parse_key_token(token).ok_or_else(|| {
            "key chord contains an unknown key name \
             (tokens must be modifiers, key names, or f1..f12)"
                .to_string()
        })?;
        keys.push(key);
    }
    if keys.iter().all(|key| key.is_modifier()) {
        return Err("key chord needs at least one non-modifier key".to_string());
    }
    Ok(keys)
}

/// Backend capability declaration. The stub backend reports all false; real backends fill in
/// what the platform actually supports.
#[derive(Debug, Clone)]
pub struct Capabilities {
    pub screenshot: bool,
    pub input: bool,
    pub ui_tree: bool,
    /// Free-text explanation (e.g. "input unavailable: no portal authorization").
    pub notes: String,
}

/// The raw result of one screen capture (device physical pixels).
///
/// - `rgba`: width×height×4 bytes of RGBA pixels. Invariant:
///   `rgba.len() == width*height*4`; the tool layer's single capture
///   consumption point (tool.rs `capture_and_store`) enforces it with a
///   `debug_assert!` plus an explicit error, so a malformed backend buffer
///   fails loudly instead of panicking downstream in the scaling layer.
/// - `origin_x/origin_y`: the monitor origin, in **input coordinate space**
///   (Windows/X11 physical pixels; macOS CGEvent points).
/// - `input_scale_x/input_scale_y`: the device-physical-pixel → input-coordinate scale.
///   Windows/X11 is 1.0; macOS is 1/backing_scale_factor (0.5 on a 2x Retina).
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
    /// Device physical pixels → input coordinate scale (per-platform rule encoded here).
    pub fn input_scale(&self) -> (f64, f64) {
        (self.input_scale_x, self.input_scale_y)
    }
}

/// The UI element hit by `element_at_point` (coordinates in input coordinate space).
#[derive(Debug, Clone, Serialize)]
pub struct ElementInfo {
    pub role: String,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// Password/secure text field (one of the T3 forced-confirmation signals).
    pub secure: bool,
}

/// `ui_tree` fetch options. `None` lets the backend apply its defaults.
#[derive(Debug, Clone, Copy, Default)]
pub struct UiTreeOptions {
    pub max_depth: Option<u32>,
    pub max_nodes: Option<u32>,
}

/// Backend error. When the platform lacks a capability it must be an explicit
/// `Unsupported`, never a silent degradation.
#[derive(Debug, Clone)]
pub enum ComputerUseError {
    /// This platform/session does not support the capability (e.g. the portal does not offer
    /// RemoteDesktop, an unimplemented backend).
    Unsupported {
        capability: &'static str,
        detail: String,
    },
    /// The capability is supported in principle but currently unavailable (permission not
    /// granted, a stale monitor handle, etc.).
    Unavailable { detail: String },
    /// Execution failed.
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

    /// Keep the error kind, rewrite the detail: drag-teardown error merging
    /// must not re-wrap an `Unavailable` as `Failed` — category-based
    /// upstream handling (e.g. "run as administrator") depends on the
    /// classification surviving, not just the textual clue.
    pub fn same_kind(&self, detail: impl Into<String>) -> Self {
        match self {
            Self::Unsupported { capability, .. } => Self::Unsupported {
                capability,
                detail: detail.into(),
            },
            Self::Unavailable { .. } => Self::unavailable(detail),
            Self::Failed { .. } => Self::failed(detail),
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

/// A parsed, validated tool call.
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
    /// count: 1 = single click, 2 = double click, 3 = triple click.
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
            Self::MouseMove { .. }
            | Self::Scroll { .. }
            | Self::Click { .. }
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

    /// Whether the action carries screenshot-space coordinates (needs a ScaleMap to execute).
    pub fn needs_scale_map(&self) -> bool {
        match self {
            Self::ElementAtPoint { .. } | Self::MouseMove { .. } | Self::Drag { .. } => true,
            Self::Scroll { at, .. } | Self::Click { at, .. } => at.is_some(),
            _ => false,
        }
    }

    /// Whether a follow-up screenshot is attached after the action. Single source of truth:
    /// actions that actually change screen content (keyboard, click, drag, wheel) and wait
    /// all attach a follow-up; mouse_move only moves the pointer and changes no content, so
    /// a follow-up would only burn tokens and is not attached.
    pub fn attaches_screenshot(&self) -> bool {
        match self {
            Self::Screenshot | Self::Wait { .. } | Self::Scroll { .. } => true,
            Self::Click { .. }
            | Self::MouseDown { .. }
            | Self::MouseUp { .. }
            | Self::Drag { .. }
            | Self::Type { .. }
            | Self::KeyChord { .. }
            | Self::HoldKey { .. } => true,
            Self::CursorPosition | Self::UiTree { .. } | Self::ElementAtPoint { .. } => false,
            // mouse_move deliberately does not attach a screenshot.
            Self::MouseMove { .. } => false,
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
        // Modifiers alone do not constitute a key press.
        assert!(parse_key_chord("ctrl+shift").is_err());
        // Multi-character and not in the synonym table.
        assert!(parse_key_chord("ctrl+ab").is_err());
    }

    /// Review-fix regression: a parse-failure error string describes only the chord's
    /// **shape** and never echoes the input text — the error string is written into the
    /// audit log's parse-failure record, and the model could carry sensitive content in the
    /// text field to probe (M1).
    #[test]
    fn key_chord_errors_do_not_echo_the_input_text() {
        for input in ["hunter2", "ctrl+secret", "a+b+c+d+e", "ctrl+", "ctrl+shift"] {
            let error = parse_key_chord(input).expect_err("chord must be rejected");
            assert!(!error.contains(input), "error echoed the input: {error}");
        }
    }

    /// Review-fix regression: the chord token cap is ≤4 and control characters are not valid
    /// key positions. (Longer sequences should take the type action instead, which accepts
    /// focus screening and confirmation.)
    #[test]
    fn key_chord_rejects_overlong_chords_and_control_characters() {
        assert!(parse_key_chord("ctrl+alt+shift+meta+c").is_err());
        assert!(parse_key_chord("a+b+c+d+e").is_err());
        // Within the cap, still accepted.
        assert!(parse_key_chord("ctrl+alt+shift+f1").is_ok());
        assert!(parse_key_chord("ctrl+a").is_ok());
        // Control characters (NUL, ESC, BEL, DEL, ...) are rejected outright.
        for control in ['\0', '\u{1}', '\u{7}', '\u{1b}', '\u{7f}'] {
            let chord = control.to_string();
            assert!(
                parse_key_chord(&chord).is_err(),
                "control char U+{:04X} must be rejected",
                control as u32
            );
            assert!(
                parse_key_chord(&format!("ctrl+{control}")).is_err(),
                "control char U+{:04X} must be rejected inside a chord",
                control as u32
            );
        }
    }

    #[test]
    fn action_classification_matches_consent_tiers() {
        assert_eq!(ComputerUseAction::Screenshot.class(), ActionClass::Observe);
        assert_eq!(
            ComputerUseAction::Wait { ms: 100 }.class(),
            ActionClass::Observe
        );
        // mouse_move/scroll really touch the pointer device, gated at the same tier as
        // clicks (review correction).
        assert_eq!(
            ComputerUseAction::MouseMove { x: 1, y: 2 }.class(),
            ActionClass::Input
        );
        assert_eq!(
            ComputerUseAction::Scroll {
                direction: ScrollDirection::Down,
                amount: 3,
                at: None,
            }
            .class(),
            ActionClass::Input
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
            ComputerUseAction::MouseDown {
                button: MouseButton::Left
            }
            .class(),
            ActionClass::Input
        );
        assert_eq!(
            ComputerUseAction::Type { text: "x".into() }.class(),
            ActionClass::Input
        );
        // Follow-up contract: actions that actually change screen content and wait attach a
        // follow-up screenshot; mouse_move does not.
        assert!(
            ComputerUseAction::Scroll {
                direction: ScrollDirection::Down,
                amount: 1,
                at: None,
            }
            .attaches_screenshot()
        );
        assert!(ComputerUseAction::Type { text: "x".into() }.attaches_screenshot());
        assert!(
            !ComputerUseAction::MouseMove { x: 1, y: 2 }.attaches_screenshot(),
            "mouse_move must not attach a screenshot"
        );
        assert!(
            !ComputerUseAction::UiTree {
                opts: UiTreeOptions::default()
            }
            .attaches_screenshot()
        );
    }
}

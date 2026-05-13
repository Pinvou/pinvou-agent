//! GUI 可调的用户偏好 + 开发者后门高级字段。
//!
//! 序列化到 `~/.pinvou3/settings.json`。前 3 个字段（theme / color_scheme / language）
//! 暴露在 Settings 面板里；`advanced` 是不进 UI 的开发者后门——可通过手改
//! `settings.json` 或对应的 `PINVOU3_*` 环境变量调整。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    Genesis,
    LiquidLight,
    LiquidDark,
}
impl Default for Theme {
    fn default() -> Self {
        Theme::Genesis
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ColorScheme {
    Light,
    Dark,
    System,
}
impl Default for ColorScheme {
    fn default() -> Self {
        ColorScheme::System
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Language {
    #[serde(rename = "zh-Hans")]
    ZhHans,
    #[serde(rename = "en")]
    En,
}
impl Default for Language {
    fn default() -> Self {
        Language::ZhHans
    }
}
impl Language {
    pub fn locale_tag(self) -> &'static str {
        match self {
            Language::ZhHans => "zh-Hans",
            Language::En => "en",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelPreset {
    LocalVllm,
}
impl Default for ModelPreset {
    fn default() -> Self {
        ModelPreset::LocalVllm
    }
}

/// 开发者后门字段。GUI 永远不暴露这些，靠手改 settings.json 或 env 调。
/// `None` 走 bridge 里的默认值；env 优先级高于 settings.json。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AdvancedPrefs {
    pub allow_shell: Option<bool>,
    pub model_preset: Option<ModelPreset>,
    pub max_output_tokens: Option<u32>,
    pub max_subagents: Option<usize>,
    pub max_steps: Option<u32>,
}

/// 用户偏好。`settings.json` 顶层结构。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UserPrefs {
    pub theme: Theme,
    pub color_scheme: ColorScheme,
    pub language: Language,
    pub advanced: AdvancedPrefs,
}

impl UserPrefs {
    /// 从 `~/.pinvou3/settings.json` 读。文件不存在或 JSON 解析失败时返回默认。
    pub fn load() -> Self {
        let path = super::paths::settings_path();
        match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                eprintln!(
                    "[pinvou3-app] settings.json parse failed ({e}), using defaults"
                );
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = super::paths::settings_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let s = serde_json::to_string_pretty(self).expect("UserPrefs serialize");
        std::fs::write(path, s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_roundtrip() {
        let prefs = UserPrefs {
            theme: Theme::LiquidDark,
            color_scheme: ColorScheme::Dark,
            language: Language::En,
            advanced: AdvancedPrefs {
                allow_shell: Some(false),
                max_output_tokens: Some(8192),
                max_subagents: Some(2),
                ..Default::default()
            },
        };
        let json = serde_json::to_string(&prefs).unwrap();
        let parsed: UserPrefs = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.theme, Theme::LiquidDark);
        assert_eq!(parsed.color_scheme, ColorScheme::Dark);
        assert_eq!(parsed.language, Language::En);
        assert_eq!(parsed.advanced.allow_shell, Some(false));
        assert_eq!(parsed.advanced.max_output_tokens, Some(8192));
    }

    #[test]
    fn prefs_partial_json_fills_defaults() {
        let json = r#"{"theme":"genesis"}"#;
        let prefs: UserPrefs = serde_json::from_str(json).unwrap();
        assert_eq!(prefs.theme, Theme::Genesis);
        assert_eq!(prefs.color_scheme, ColorScheme::System);
        assert_eq!(prefs.language, Language::ZhHans);
        assert!(prefs.advanced.allow_shell.is_none());
    }

    #[test]
    fn language_serializes_as_bcp47_tag() {
        assert_eq!(
            serde_json::to_string(&Language::ZhHans).unwrap(),
            r#""zh-Hans""#
        );
        assert_eq!(serde_json::to_string(&Language::En).unwrap(), r#""en""#);
    }

    #[test]
    fn locale_tag_helper() {
        assert_eq!(Language::ZhHans.locale_tag(), "zh-Hans");
        assert_eq!(Language::En.locale_tag(), "en");
    }
}

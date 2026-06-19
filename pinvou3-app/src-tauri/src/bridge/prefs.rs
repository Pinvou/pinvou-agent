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
    /// 日语。底座 prompts.rs 的 translation_target_language_for_tag 已认识 "ja"，
    /// LLM 回复语言链路零改动。
    #[serde(rename = "ja")]
    Ja,
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
            Language::Ja => "ja",
        }
    }

    /// present_artifact 的 `title` 该用什么语言(instructions.md 的 {{PINVOU3_TITLE_LANG}})。
    /// 原文写死"中文 title",英文 UI 下模型走到调 present_artifact 就生成中文标题、并把后续
    /// 描述/总结也带回中文(tool-call 现场的具体指令压过通用语言规则)→ 改成跟 locale。
    pub fn title_language_name(self) -> &'static str {
        match self {
            Language::ZhHans => "简体中文",
            Language::En => "English",
            Language::Ja => "日本語",
        }
    }

    /// pinvou3 补丁:底座 `locale_reinforcement_preamble` 对 `en` 返回 `None`
    /// (英文是模型默认语言,底座认为无需强化)。但 pinvou3 的 system prompt 主体
    /// (instructions.md)整份是中文,会把模型的回复语言拽回中文 —— 故英文 UI 下
    /// 仍中文回复。zh-Hans / ja 已由底座 bookend(见 `bridge::bundle` 的
    /// `set_locale_preamble_*_override`)覆盖,这里只补底座留空的 locale,返回
    /// `None` 的不再重复注入。文案采 mirror 语义,与 zh-Hans preamble 对称。
    pub fn extra_language_directive(self) -> Option<&'static str> {
        match self {
            Language::En => Some(
                "## Language\n\n\
                 Respond in English by default, and mirror the language of the \
                 user's latest message. Keep code, file paths, tool names \
                 (e.g. `read_file`, `exec_shell`), environment variables, \
                 command-line flags, and URLs verbatim — only natural-language \
                 prose follows the language rule.",
            ),
            // 底座已注入对应 bookend,避免重复。
            Language::ZhHans | Language::Ja => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelPreset {
    /// 默认本地 vLLM：qwen36_35b_256k @ 127.0.0.1:8000/v1
    LocalVllm,
    /// DeepSeek 官方 API
    Deepseek,
    /// Kimi (Moonshot)
    Kimi,
    /// OpenAI 兼容 API（OpenAI 官方 / 自托管 / 代理 / 其他 OpenAI 兼容厂商）
    OpenaiCompatible,
    /// 通义千问 (Qwen)
    Qwen,
    /// 豆包 (火山方舟)
    Doubao,
    /// MiniMax
    Minimax,
    /// 智谱 GLM
    Glm,
    /// 小米 MiMo
    Mimo,
}
impl Default for ModelPreset {
    fn default() -> Self {
        ModelPreset::LocalVllm
    }
}

/// Search 后端选择。
/// - `Bing`(默认): HTML scrape,无需 key,但对中文长复合查询相关性差。
///   DDG 在 GFW + 代理 datacenter IP 段下基本恒返 anomaly-modal,
///   所以底座 fork patch #42 已把默认翻成 Bing,这里前端默认对齐。
/// - `Metaso` / `Bocha` / `Baidu`: 国内 AI 搜索 API,中文场景相关性远好于 Bing scrape。
///   Metaso 留空 key 走底座内置共享 key(~100 次/天);Bocha/Baidu 必须填 key。
/// - `Tavily`: 海外 agent 搜索 API(<https://app.tavily.com/> 拿 `tvly-` key,API 实际打
///   `api.tavily.com`)。结果是干净抽取的 content 而非 HTML scrape,质量好;但要稳定外网 +
///   自带额度,key 必填(留空底座直接报 "requires API key")。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchProvider {
    Bing,
    Metaso,
    Bocha,
    Baidu,
    Tavily,
}
impl Default for SearchProvider {
    fn default() -> Self {
        SearchProvider::Bing
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SearchPrefs {
    pub provider: SearchProvider,
    /// 当 `provider = Metaso` 时:None 走底座内置共享 key。
    /// 当 `provider = Bocha`/`Baidu` 时:None 会让 web_search 直接报错(必填)。
    pub api_key: Option<String>,
}

impl SearchPrefs {
    /// 传给底座前归一化 key。
    ///
    /// 空字符串如果透传成 `Some("")`,部分搜索 API 会返回 HTTP 200 + 业务错误体,
    /// 底座旧版本可能误解析成 `No results found`。这里统一把空白 key 当未配置。
    pub fn normalized_api_key(&self) -> Option<String> {
        self.api_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(ToString::to_string)
    }

    pub fn normalize(&mut self) {
        self.api_key = self.normalized_api_key();
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
    /// 自定义模型 ID（CustomLocal / Remote* 生效）
    pub custom_model_name: Option<String>,
    /// 自定义 API base URL（CustomLocal / Remote* 生效）
    pub custom_base_url: Option<String>,
    /// 自定义 API key（CustomLocal / Remote* 生效）
    pub custom_api_key: Option<String>,
}

/// 用户偏好。`settings.json` 顶层结构。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UserPrefs {
    pub theme: Theme,
    pub color_scheme: ColorScheme,
    pub language: Language,
    pub search: SearchPrefs,
    pub advanced: AdvancedPrefs,
}

impl UserPrefs {
    /// 从 `~/.pinvou3/settings.json` 读。文件不存在或 JSON 解析失败时返回默认。
    pub fn load() -> Self {
        let path = super::paths::settings_path();
        match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                eprintln!("[pinvou3-app] settings.json parse failed ({e}), using defaults");
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
        let mut normalized = self.clone();
        normalized.search.normalize();
        let s = serde_json::to_string_pretty(&normalized).expect("UserPrefs serialize");
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
            search: SearchPrefs::default(),
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
        assert_eq!(serde_json::to_string(&Language::Ja).unwrap(), r#""ja""#);
    }

    #[test]
    fn locale_tag_helper() {
        assert_eq!(Language::ZhHans.locale_tag(), "zh-Hans");
        assert_eq!(Language::En.locale_tag(), "en");
        assert_eq!(Language::Ja.locale_tag(), "ja");
    }

    #[test]
    fn language_ja_roundtrip() {
        let json = r#"{"theme":"genesis","language":"ja"}"#;
        let prefs: UserPrefs = serde_json::from_str(json).unwrap();
        assert_eq!(prefs.language, Language::Ja);
    }

    #[test]
    fn search_prefs_default_is_bing_no_key() {
        let p = SearchPrefs::default();
        assert_eq!(p.provider, SearchProvider::Bing);
        assert!(p.api_key.is_none());
    }

    #[test]
    fn search_prefs_roundtrip_with_metaso_key() {
        let prefs = UserPrefs {
            search: SearchPrefs {
                provider: SearchProvider::Metaso,
                api_key: Some("mk-user-own-key".to_string()),
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&prefs).unwrap();
        let parsed: UserPrefs = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.search.provider, SearchProvider::Metaso);
        assert_eq!(parsed.search.api_key.as_deref(), Some("mk-user-own-key"));
    }

    #[test]
    fn search_prefs_normalized_api_key_treats_blank_as_none() {
        for raw in [None, Some("".to_string()), Some("   \n\t ".to_string())] {
            let prefs = SearchPrefs {
                provider: SearchProvider::Metaso,
                api_key: raw,
            };
            assert!(prefs.normalized_api_key().is_none());
        }

        let prefs = SearchPrefs {
            provider: SearchProvider::Metaso,
            api_key: Some("  mk-user-key  ".to_string()),
        };
        assert_eq!(prefs.normalized_api_key().as_deref(), Some("mk-user-key"));
    }

    #[test]
    fn prefs_save_normalizes_blank_search_api_key_on_disk() {
        let old_home = std::env::var_os("PINVOU3_HOME");
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-prefs-save-normalize-{}",
            std::process::id()
        ));
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let prefs = UserPrefs {
            search: SearchPrefs {
                provider: SearchProvider::Metaso,
                api_key: Some(" \n\t ".to_string()),
            },
            ..Default::default()
        };
        prefs.save().expect("prefs should save");

        let saved = std::fs::read_to_string(super::super::paths::settings_path())
            .expect("settings should exist");
        let parsed: UserPrefs = serde_json::from_str(&saved).expect("settings should parse");
        assert_eq!(parsed.search.provider, SearchProvider::Metaso);
        assert!(parsed.search.api_key.is_none());

        let _ = std::fs::remove_dir_all(&tmp);
        match old_home {
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
    }

    #[test]
    fn search_prefs_partial_json_fills_defaults() {
        // 老的 settings.json 没 search 字段 → 默认 Bing/None,不破坏向前兼容。
        let json = r#"{"theme":"genesis","language":"zh-Hans"}"#;
        let prefs: UserPrefs = serde_json::from_str(json).unwrap();
        assert_eq!(prefs.search.provider, SearchProvider::Bing);
        assert!(prefs.search.api_key.is_none());
    }
}

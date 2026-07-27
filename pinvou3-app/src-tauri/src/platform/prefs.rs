//! GUI 可调的用户偏好 + 开发者后门高级字段。
//!
//! 序列化到 `~/.pinvou3/settings.json`。前 3 个字段（theme / color_scheme / language）
//! 暴露在 Settings 面板里；`advanced` 是不进 UI 的开发者后门——可通过手改
//! `settings.json` 或对应的 `PINVOU3_*` 环境变量调整。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::platform::credential_store::{
    CredentialEditAction, CredentialMigrationResult, CredentialReference, CredentialState,
    CredentialStore, SystemCredentialStore,
};

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

    pub fn supports_memory(self) -> bool {
        matches!(self, Language::ZhHans)
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

    /// macOS Speech 框架（`SFSpeechRecognizer`）识别用的 locale 标识（BCP 47）。
    ///
    /// **UI 语言 = 语音识别语言**：保持一致,避免「中文 UI 但用系统默认 locale
    /// （可能是 en-US）识别」→ 中文语音被当英文解析、产出无意义英文字母的错配。
    /// 选 `zh-CN` 而非 `zh-Hans-CN`：Speech 框架对 `zh-CN` 的识别模型质量与
    /// on-device 支持最好。映射可单测,无设备依赖。
    pub fn speech_recognition_locale(self) -> &'static str {
        match self {
            Language::ZhHans => "zh-CN",
            Language::En => "en-US",
            Language::Ja => "ja-JP",
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

pub const MODEL_PROVIDER_KIND_CODING_PLAN: &str = "coding_plan";
pub const MODEL_PROVIDER_KIND_OFFICIAL_API: &str = "official_api";
pub const MODEL_PROVIDER_KIND_CUSTOM: &str = "custom";

fn trim_url_tail(value: &str) -> String {
    value.trim().trim_end_matches('/').to_string()
}

fn strip_chat_completions_suffix(value: &str) -> String {
    let trimmed = trim_url_tail(value);
    let lower = trimmed.to_ascii_lowercase();
    if lower.ends_with("/chat/completions") {
        trimmed[..trimmed.len() - "/chat/completions".len()].to_string()
    } else {
        trimmed
    }
}

fn identify_coding_plan_endpoint(base_url: &str) -> Option<(&'static str, &'static str)> {
    let base = strip_chat_completions_suffix(base_url);
    let normalized = base.to_ascii_lowercase();
    match normalized.as_str() {
        "https://open.bigmodel.cn/api/coding/paas/v4" => {
            Some(("glm", "https://open.bigmodel.cn/api/coding/paas/v4"))
        }
        "https://api.kimi.com/coding/v1" => Some(("kimi", "https://api.kimi.com/coding/v1")),
        "https://api.lkeap.cloud.tencent.com/coding/v3" => {
            Some(("tencent", "https://api.lkeap.cloud.tencent.com/coding/v3"))
        }
        "https://api.lkeap.cloud.tencent.com/plan/v3" => {
            Some(("tencent", "https://api.lkeap.cloud.tencent.com/plan/v3"))
        }
        _ => None,
    }
}

impl Default for ModelPreset {
    /// 平台感知默认预设:macOS/Windows 无本地 vLLM 支持(相关后端命令已 cfg 掉),
    /// 默认到 DeepSeek 远程 API,否则新用户首启即落在 127.0.0.1:8000 永远连不上。
    /// Linux 保持 LocalVllm(麒麟环境默认有本地大模型)。
    fn default() -> Self {
        #[cfg(target_os = "linux")]
        {
            ModelPreset::LocalVllm
        }
        #[cfg(not(target_os = "linux"))]
        {
            ModelPreset::Deepseek
        }
    }
}
impl ModelPreset {
    /// 与前端 preset key、settings.json 序列化值一致的稳定串(snake_case)。
    pub fn as_str(&self) -> &'static str {
        match self {
            ModelPreset::LocalVllm => "local_vllm",
            ModelPreset::Deepseek => "deepseek",
            ModelPreset::Kimi => "kimi",
            ModelPreset::OpenaiCompatible => "openai_compatible",
            ModelPreset::Qwen => "qwen",
            ModelPreset::Doubao => "doubao",
            ModelPreset::Minimax => "minimax",
            ModelPreset::Glm => "glm",
            ModelPreset::Mimo => "mimo",
        }
    }
    /// 各预设默认 base_url(与 bridge `default_base_url_for_preset` 对齐;迁移/添加模型模板兜底)。
    pub fn default_base_url(&self) -> &'static str {
        match self {
            ModelPreset::LocalVllm => "http://127.0.0.1:8000/v1",
            ModelPreset::Deepseek => "https://api.deepseek.com",
            ModelPreset::Kimi => "https://api.moonshot.cn/v1",
            ModelPreset::OpenaiCompatible => "https://api.openai.com/v1",
            ModelPreset::Qwen => "https://dashscope.aliyuncs.com/compatible-mode/v1",
            ModelPreset::Doubao => "https://ark.cn-beijing.volces.com/api/v3",
            ModelPreset::Minimax => "https://api.minimax.chat/v1",
            ModelPreset::Glm => "https://open.bigmodel.cn/api/paas/v4",
            ModelPreset::Mimo => "https://api.xiaomimimo.com/v1",
        }
    }
    /// 各预设默认模型名(与 bridge `default_model_for_preset` 对齐)。
    /// LocalVllm 的 `qwen36_35b_256k` 后缀语义见 bridge `LOCAL_VLLM_MODEL`。
    pub fn default_model(&self) -> &'static str {
        match self {
            ModelPreset::LocalVllm => "qwen36_35b_256k",
            ModelPreset::Deepseek => "deepseek-v4-pro",
            ModelPreset::Kimi => "kimi-k3",
            ModelPreset::OpenaiCompatible => "gpt-5.6-terra",
            ModelPreset::Qwen => "qwen3.7-plus",
            ModelPreset::Doubao => "doubao-seed-evolving",
            ModelPreset::Minimax => "MiniMax-M3",
            ModelPreset::Glm => "glm-5.2",
            ModelPreset::Mimo => "mimo-v2.5-pro",
        }
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
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
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

impl SearchProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            SearchProvider::Bing => "bing",
            SearchProvider::Metaso => "metaso",
            SearchProvider::Bocha => "bocha",
            SearchProvider::Baidu => "baidu",
            SearchProvider::Tavily => "tavily",
        }
    }

    pub fn supports_api_key(self) -> bool {
        !matches!(self, SearchProvider::Bing)
    }

    pub fn env_key_names(self) -> &'static [&'static str] {
        match self {
            SearchProvider::Metaso => &["METASO_API_KEY"],
            SearchProvider::Baidu => &["BAIDU_SEARCH_API_KEY"],
            SearchProvider::Bing | SearchProvider::Bocha | SearchProvider::Tavily => &[],
        }
    }

    pub fn credential_reference(self) -> CredentialReference {
        CredentialReference::for_search_provider(self.as_str())
    }
}

fn default_enabled_search_providers() -> Vec<SearchProvider> {
    vec![SearchProvider::Bing]
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SearchCredential {
    #[serde(default, skip_serializing)]
    pub api_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<CredentialReference>,
    #[serde(default)]
    pub credential_state: CredentialState,
    #[serde(default)]
    pub has_secret: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_action: Option<CredentialEditAction>,
}

impl SearchCredential {
    pub fn clear_plaintext_key(&mut self) {
        self.api_key.clear();
        self.credential_action = None;
    }

    pub fn mark_configured(&mut self, reference: CredentialReference) {
        self.credential_ref = Some(reference);
        self.credential_state = CredentialState::Configured;
        self.has_secret = true;
        self.clear_plaintext_key();
    }

    pub fn mark_missing(&mut self) {
        self.credential_ref = None;
        self.credential_state = CredentialState::Missing;
        self.has_secret = false;
        self.clear_plaintext_key();
    }

    pub fn mark_unavailable(&mut self) {
        self.credential_state = CredentialState::Unavailable;
        self.has_secret = self.credential_ref.is_some();
        self.clear_plaintext_key();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SearchPrefs {
    pub provider: SearchProvider,
    #[serde(default = "default_enabled_search_providers")]
    pub enabled_providers: Vec<SearchProvider>,
    /// 当 `provider = Metaso` 时:None 走底座内置共享 key。
    /// 当 `provider = Bocha`/`Baidu` 时:None 会让 web_search 直接报错(必填)。
    #[serde(default, skip_serializing)]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub credentials: BTreeMap<SearchProvider, SearchCredential>,
}

impl Default for SearchPrefs {
    fn default() -> Self {
        Self {
            provider: SearchProvider::Bing,
            enabled_providers: default_enabled_search_providers(),
            api_key: None,
            credentials: BTreeMap::new(),
        }
    }
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
        self.api_key = None;
        if self.enabled_providers.is_empty() {
            self.enabled_providers.push(SearchProvider::Bing);
        }
        if !self.enabled_providers.contains(&SearchProvider::Bing) {
            self.enabled_providers.insert(0, SearchProvider::Bing);
        }
        if !self.enabled_providers.contains(&self.provider) {
            self.enabled_providers.push(self.provider);
        }
        self.enabled_providers.sort();
        self.enabled_providers.dedup();
        self.credentials
            .retain(|_, credential| credential.has_secret || credential.credential_ref.is_some());
        for credential in self.credentials.values_mut() {
            credential.clear_plaintext_key();
        }
    }
}

/// 一条用户保存的模型配置:GUI「模型列表」的一项,也是热切换的最小单位。
/// `id` 稳定(前端生成),被 `active_model_id` / session `model_id` 引用。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedModel {
    pub id: String,
    /// 用户起的显示名("本地 Qwen"/"DeepSeek 线上")。
    pub name: String,
    /// 决定 provider 路由 + 模板,复用现有 9 预设枚举。
    pub preset: ModelPreset,
    /// 该具体部署允许的 context window；与发给服务端的 `model` wire name 解耦。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u32>,
    /// Pinvou 对该 route 声明的单轮 output 上限；最终仍受进程级请求上限约束。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    pub model: String,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_mode: Option<String>,
    #[serde(default, skip_serializing)]
    pub api_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<CredentialReference>,
    #[serde(default)]
    pub credential_state: CredentialState,
    #[serde(default)]
    pub has_secret: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_action: Option<CredentialEditAction>,
}

impl SavedModel {
    fn normalize_route_limits(&mut self) {
        self.context_window_tokens = self.context_window_tokens.filter(|tokens| *tokens > 0);
        self.max_output_tokens = self.max_output_tokens.filter(|tokens| *tokens > 0);
        if self.preset == ModelPreset::LocalVllm {
            if self.context_window_tokens.is_none() && self.model == "qwen36_35b_256k" {
                self.context_window_tokens = Some(262_144);
            }
            if self.max_output_tokens.is_none() {
                self.max_output_tokens = Some(24_576);
            }
        }
    }

    fn normalize_provider_metadata(&mut self) {
        self.base_url = strip_chat_completions_suffix(&self.base_url);
        if let Some((vendor, canonical_base_url)) = identify_coding_plan_endpoint(&self.base_url) {
            self.provider_kind = Some(MODEL_PROVIDER_KIND_CODING_PLAN.to_string());
            self.vendor = Some(vendor.to_string());
            self.base_url = canonical_base_url.to_string();
            if self.endpoint_mode.as_deref() == Some("full_chat_completions") {
                self.endpoint_mode = None;
            }
            return;
        }
        if self.provider_kind.as_deref() == Some(MODEL_PROVIDER_KIND_CODING_PLAN) {
            self.provider_kind = None;
        }
        if self.provider_kind.is_none() {
            self.provider_kind = Some(
                if self.preset == ModelPreset::OpenaiCompatible {
                    MODEL_PROVIDER_KIND_CUSTOM
                } else {
                    MODEL_PROVIDER_KIND_OFFICIAL_API
                }
                .to_string(),
            );
        }
        if self
            .vendor
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            self.vendor = None;
        }
        if self
            .endpoint_mode
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            self.endpoint_mode = None;
        }
    }

    pub fn credential_reference(&self) -> CredentialReference {
        self.credential_ref
            .clone()
            .unwrap_or_else(|| CredentialReference::for_model(&self.id))
    }

    pub fn clear_plaintext_key(&mut self) {
        self.api_key.clear();
        self.credential_action = None;
    }

    pub fn mark_configured(&mut self, reference: CredentialReference) {
        self.credential_ref = Some(reference);
        self.credential_state = CredentialState::Configured;
        self.has_secret = true;
        self.clear_plaintext_key();
    }

    pub fn mark_missing(&mut self) {
        self.credential_ref = None;
        self.credential_state = CredentialState::Missing;
        self.has_secret = false;
        self.clear_plaintext_key();
    }

    pub fn mark_unavailable(&mut self) {
        self.credential_state = CredentialState::Unavailable;
        self.has_secret = self.credential_ref.is_some();
        self.clear_plaintext_key();
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
    #[serde(default, skip_serializing)]
    pub custom_api_key: Option<String>,
    /// 「添加模型」方案:已保存模型列表(GUI 增删改)。空 = 触发迁移兜底
    /// (见 `UserPrefs::migrate_models`),把旧 model_preset+custom_* 合成一条。
    #[serde(default)]
    pub saved_models: Vec<SavedModel>,
    /// 全局默认/当前激活模型 id(新建会话继承它)。None = 回退列表首条。
    #[serde(default)]
    pub active_model_id: Option<String>,
    /// MegaCube(GB10) 本地大模型一键引导是否成功跑过一次。
    /// 置真后首屏引导框永不再弹(见 `local_vllm_setup::detect`)。引导失败/被跳过不置真。
    #[serde(default)]
    pub local_vllm_bootstrapped: bool,
    /// 用户点「不再提醒 → 确认」婉拒预装本地大模型:置真后开机引导框不再自动弹。
    /// 与 bootstrapped 区别:婉拒是"我先不要",仍可在设置→模型管理「检测本机 vLLM」里手动启用。
    #[serde(default)]
    pub local_vllm_setup_declined: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationPrefs {
    pub enabled: bool,
    pub task_completed: bool,
}

impl Default for NotificationPrefs {
    fn default() -> Self {
        // Linux desktop notification portals vary widely by distro/session.
        // Keep task completion notifications opt-in there, while preserving
        // the previous default on Windows/macOS.
        let enabled = !cfg!(target_os = "linux");
        Self {
            enabled,
            task_completed: enabled,
        }
    }
}

/// 桌宠偏好。只存开关——窗口位置在 `~/.pinvou3/pet_window.json`(pet_window.rs 私有
/// 管理)。位置刻意不进 settings.json:settings 由前端整份回写(update_settings),
/// 旧副本会把拖动后的实时位置覆盖回去。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PetPrefs {
    pub enabled: bool,
}

/// 侧栏任务列表偏好。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct SidebarPrefs {
    /// 任务列表按日期分组折叠(今天默认展开);false = 平铺列表。
    pub date_grouping: bool,
}

impl Default for SidebarPrefs {
    fn default() -> Self {
        Self { date_grouping: true }
    }
}

/// 用户偏好。`settings.json` 顶层结构。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UserPrefs {
    pub theme: Theme,
    pub color_scheme: ColorScheme,
    pub language: Language,
    pub memory_enabled: bool,
    pub search: SearchPrefs,
    pub notifications: NotificationPrefs,
    pub pet: PetPrefs,
    pub sidebar: SidebarPrefs,
    pub advanced: AdvancedPrefs,
}

impl UserPrefs {
    /// 从 `~/.pinvou3/settings.json` 读。文件不存在或 JSON 解析失败时返回默认。
    pub fn load() -> Self {
        let path = super::paths::settings_path();
        let mut prefs = match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                eprintln!("[pinvou3-app] settings.json parse failed ({e}), using defaults");
                Self::default()
            }),
            Err(_) => Self::default(),
        };
        prefs.migrate_models();
        prefs.normalize_saved_model_metadata();
        let migration = prefs.migrate_plaintext_api_keys_with_store(&SystemCredentialStore::new());
        let memory_policy_changed = prefs.enforce_memory_locale_policy();
        if migration.settings_sanitized || memory_policy_changed {
            if let Err(e) = prefs.save() {
                eprintln!("[pinvou3-app] settings normalization save failed: {e:?}");
            }
        }
        prefs.sanitize_plaintext_api_keys();
        prefs
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = super::paths::settings_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut normalized = self.clone();
        normalized.search.normalize();
        normalized.enforce_memory_locale_policy();
        normalized.normalize_saved_model_metadata();
        for model in &mut normalized.advanced.saved_models {
            model.normalize_route_limits();
        }
        normalized.sanitize_plaintext_api_keys();
        let s = serde_json::to_string_pretty(&normalized).expect("UserPrefs serialize");
        std::fs::write(path, s)
    }

    pub fn normalize_saved_model_metadata(&mut self) {
        for model in &mut self.advanced.saved_models {
            model.normalize_provider_metadata();
            model.normalize_route_limits();
        }
    }

    fn enforce_memory_locale_policy(&mut self) -> bool {
        if !self.language.supports_memory() && self.memory_enabled {
            self.memory_enabled = false;
            true
        } else {
            false
        }
    }

    /// 迁移:旧版只有 `model_preset`+`custom_*` 单组配置 → 合成一条 `SavedModel`
    /// 进列表并设为 active。幂等(仅当 `saved_models` 为空,多次 load 安全)。
    /// 全新用户(default prefs)也走这里,得到一条默认 LocalVllm 模型。
    /// `pub(crate)`:bridge 测试模拟 `load()` 的迁移路径(custom_* → active model)。
    pub(crate) fn migrate_models(&mut self) {
        if !self.advanced.saved_models.is_empty() {
            for model in &mut self.advanced.saved_models {
                model.normalize_provider_metadata();
                model.normalize_route_limits();
            }
            return;
        }
        let preset = self.advanced.model_preset.unwrap_or_default();
        let model = self
            .advanced
            .custom_model_name
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| preset.default_model().to_string());
        let base_url = self
            .advanced
            .custom_base_url
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| preset.default_base_url().to_string());
        let api_key = self.advanced.custom_api_key.clone().unwrap_or_default();
        let id = "default".to_string();
        self.advanced.saved_models.push(SavedModel {
            id: id.clone(),
            name: model.clone(),
            preset,
            context_window_tokens: None,
            max_output_tokens: None,
            model,
            base_url,
            provider_kind: None,
            vendor: None,
            endpoint_mode: None,
            api_key,
            credential_ref: None,
            credential_state: CredentialState::Missing,
            has_secret: false,
            credential_action: None,
        });
        self.advanced.saved_models[0].normalize_route_limits();
        self.advanced.saved_models[0].normalize_provider_metadata();
        self.advanced.custom_api_key = None;
        if self.advanced.active_model_id.is_none() {
            self.advanced.active_model_id = Some(id);
        }
    }

    pub fn migrate_plaintext_api_keys_with_store<S: CredentialStore>(
        &mut self,
        store: &S,
    ) -> CredentialMigrationResult {
        let mut result = CredentialMigrationResult::default();

        for model in &mut self.advanced.saved_models {
            let key = model.api_key.trim().to_string();
            if key.is_empty() {
                // 明文 key 为空(keep_existing 场景):不盲改 credential_state。
                // 凭据是否真的存在由后续 refresh_credential_states_with_store 真实
                // 回读存储判定——此前这里"只看 credential_ref 存在就标 Configured"
                // 会导致存储里是空值却显示"已配置"(假阳性 → 401)。
                if model.credential_ref.is_none() {
                    result.skipped_count += 1;
                }
                model.clear_plaintext_key();
                continue;
            }

            let reference = model.credential_reference();
            match store.set(&reference, &key) {
                Ok(()) => {
                    model.mark_configured(reference);
                    result.migrated_count += 1;
                    result.settings_sanitized = true;
                }
                Err(err) => {
                    eprintln!(
                        "[pinvou3-app] credential migration failed for model {}: {}",
                        model.id,
                        err.user_message()
                    );
                    model.credential_state = CredentialState::Unavailable;
                    model.has_secret = false;
                    result.failed_model_ids.push(model.id.clone());
                }
            }
        }

        if let Some(key) = self
            .advanced
            .custom_api_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(ToString::to_string)
        {
            let model_index = self
                .advanced
                .active_model_id
                .as_deref()
                .and_then(|id| self.advanced.saved_models.iter().position(|m| m.id == id))
                .or_else(|| (!self.advanced.saved_models.is_empty()).then_some(0));
            if let Some(index) = model_index {
                let model = &mut self.advanced.saved_models[index];
                let reference = model.credential_reference();
                match store.set(&reference, &key) {
                    Ok(()) => {
                        model.mark_configured(reference);
                        result.migrated_count += 1;
                        result.settings_sanitized = true;
                        self.advanced.custom_api_key = None;
                    }
                    Err(err) => {
                        eprintln!(
                            "[pinvou3-app] custom_api_key migration failed for model {}: {}",
                            model.id,
                            err.user_message()
                        );
                        model.credential_state = CredentialState::Unavailable;
                        result.failed_model_ids.push(model.id.clone());
                    }
                }
            }
        } else {
            self.advanced.custom_api_key = None;
        }

        if let Some(key) = self.search.normalized_api_key() {
            if self.search.provider.supports_api_key() {
                let credential = self
                    .search
                    .credentials
                    .entry(self.search.provider)
                    .or_default();
                credential.api_key = key;
                credential.credential_action = Some(CredentialEditAction::Replace);
            }
            self.search.api_key = None;
            result.settings_sanitized = true;
        }

        for (provider, credential) in &mut self.search.credentials {
            let action = credential.credential_action.unwrap_or_else(|| {
                if credential.api_key.trim().is_empty() {
                    CredentialEditAction::KeepExisting
                } else {
                    CredentialEditAction::Replace
                }
            });
            match action {
                CredentialEditAction::KeepExisting => {
                    // keep_existing:不盲改 credential_state,留给 refresh 真实回读判定。
                    credential.clear_plaintext_key();
                }
                CredentialEditAction::Replace => {
                    let key = credential.api_key.trim().to_string();
                    if key.is_empty() {
                        credential.mark_missing();
                        result.settings_sanitized = true;
                    } else {
                        let reference = provider.credential_reference();
                        match store.set(&reference, &key) {
                            Ok(()) => {
                                credential.mark_configured(reference);
                                result.migrated_count += 1;
                                result.settings_sanitized = true;
                            }
                            Err(err) => {
                                eprintln!(
                                    "[pinvou3-app] search credential migration failed for {}: {}",
                                    provider.as_str(),
                                    err.user_message()
                                );
                                credential.mark_unavailable();
                                result
                                    .failed_search_providers
                                    .push(provider.as_str().to_string());
                            }
                        }
                    }
                }
                CredentialEditAction::Delete => {
                    if let Some(reference) = credential.credential_ref.clone().or_else(|| {
                        provider
                            .supports_api_key()
                            .then(|| provider.credential_reference())
                    }) {
                        if let Err(err) = store.delete(&reference) {
                            eprintln!(
                                "[pinvou3-app] search credential delete failed for {}: {}",
                                provider.as_str(),
                                err.user_message()
                            );
                            credential.mark_unavailable();
                            result
                                .failed_search_providers
                                .push(provider.as_str().to_string());
                            continue;
                        }
                    }
                    credential.mark_missing();
                    result.settings_sanitized = true;
                }
            }
        }

        result
    }

    pub fn sanitize_plaintext_api_keys(&mut self) {
        // 只清空内存里的明文 key 字段。credential_state / has_secret 的权威判定
        // 交给 `refresh_credential_states_with_store`(它真实回读存储校验非空)。
        // 此前这里会"只看 credential_ref 存在就把 Missing 盲改回 Configured",
        // 导致 refresh 刚校准出的 Missing 被覆盖 → 假阳性(Keychain 存空值却显示
        // "已配置") → 云端调用拿空 key → 401。
        self.search.api_key = None;
        for credential in self.search.credentials.values_mut() {
            credential.clear_plaintext_key();
        }
        self.advanced.custom_api_key = None;
        for model in &mut self.advanced.saved_models {
            model.clear_plaintext_key();
        }
    }

    pub fn refresh_credential_states_with_store<S: CredentialStore>(&mut self, store: &S) {
        let env_override = std::env::var("DEEPSEEK_API_KEY")
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false);
        for model in &mut self.advanced.saved_models {
            if env_override {
                model.credential_state = CredentialState::EnvOverride;
                model.has_secret = model.credential_ref.is_some();
                model.clear_plaintext_key();
                continue;
            }
            let Some(reference) = model.credential_ref.clone() else {
                model.mark_missing();
                continue;
            };
            match store.get(&reference) {
                Ok(Some(value)) if !value.trim().is_empty() => model.mark_configured(reference),
                Ok(_) => model.mark_missing(),
                Err(_) => model.mark_unavailable(),
            }
        }

        for (provider, credential) in &mut self.search.credentials {
            if provider.env_key_names().iter().any(|name| {
                std::env::var(name)
                    .map(|v| !v.trim().is_empty())
                    .unwrap_or(false)
            }) {
                credential.credential_state = CredentialState::EnvOverride;
                credential.has_secret = credential.credential_ref.is_some();
                credential.clear_plaintext_key();
                continue;
            }
            let Some(reference) = credential.credential_ref.clone() else {
                credential.mark_missing();
                continue;
            };
            match store.get(&reference) {
                Ok(Some(value)) if !value.trim().is_empty() => {
                    credential.mark_configured(reference)
                }
                Ok(_) => credential.mark_missing(),
                Err(_) => credential.mark_unavailable(),
            }
        }
    }

    /// 当前全局激活模型:`active_model_id` 指向的那条,失效则回退列表首条。
    /// load 后 `saved_models` 必非空(migrate 保证),故正常返回 Some。
    pub fn active_model(&self) -> Option<&SavedModel> {
        if let Some(id) = &self.advanced.active_model_id {
            if let Some(m) = self.advanced.saved_models.iter().find(|m| &m.id == id) {
                return Some(m);
            }
        }
        self.advanced.saved_models.first()
    }

    /// 按 id 查模型(session per-model 解析用)。
    pub fn model_by_id(&self, id: &str) -> Option<&SavedModel> {
        self.advanced.saved_models.iter().find(|m| m.id == id)
    }

    /// 增或改(按 id)一条模型。
    pub fn upsert_model(&mut self, mut m: SavedModel) {
        m.normalize_provider_metadata();
        m.normalize_route_limits();
        if let Some(existing) = self.advanced.saved_models.iter_mut().find(|x| x.id == m.id) {
            *existing = m;
        } else {
            self.advanced.saved_models.push(m);
        }
    }

    /// 删一条模型;若删的是当前 active,回退到列表首条。
    pub fn remove_model(&mut self, id: &str) {
        self.advanced.saved_models.retain(|m| m.id != id);
        if self.advanced.active_model_id.as_deref() == Some(id) {
            self.advanced.active_model_id =
                self.advanced.saved_models.first().map(|m| m.id.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::credential_store::MemoryCredentialStore;
    use crate::platform::paths::tests::ENV_LOCK;

    #[test]
    fn migrate_creates_default_model_for_fresh_prefs() {
        let mut prefs = UserPrefs::default();
        prefs.migrate_models();
        assert_eq!(prefs.advanced.saved_models.len(), 1);
        let m = &prefs.advanced.saved_models[0];
        // 默认预设平台感知(Linux→LocalVllm,macOS/Windows→Deepseek),见 ModelPreset::default()。
        // 各平台默认模型/上下文随之不同,这里按平台分别断言。
        let expected_preset = ModelPreset::default();
        assert_eq!(m.preset, expected_preset);
        assert_eq!(m.model, expected_preset.default_model());
        assert_eq!(prefs.advanced.active_model_id.as_deref(), Some("default"));
        assert_eq!(prefs.active_model().map(|m| m.id.as_str()), Some("default"));
    }

    #[test]
    fn migrate_is_idempotent_and_preserves_custom() {
        let mut prefs = UserPrefs::default();
        prefs.advanced.model_preset = Some(ModelPreset::Deepseek);
        prefs.advanced.custom_model_name = Some("deepseek-v4-flash".into());
        prefs.advanced.custom_api_key = Some("sk-x".into());
        prefs.migrate_models();
        let snapshot = prefs.advanced.saved_models.clone();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].preset, ModelPreset::Deepseek);
        assert_eq!(snapshot[0].model, "deepseek-v4-flash");
        assert_eq!(snapshot[0].base_url, "https://api.deepseek.com");
        assert_eq!(snapshot[0].api_key, "sk-x");
        // 再次迁移幂等
        prefs.migrate_models();
        assert_eq!(prefs.advanced.saved_models, snapshot);
    }

    #[test]
    fn coding_plan_endpoint_alias_is_normalized_with_metadata() {
        let mut prefs = UserPrefs::default();
        prefs.migrate_models();
        prefs.upsert_model(SavedModel {
            id: "glm-coding".into(),
            name: "GLM-5-Turbo".into(),
            preset: ModelPreset::OpenaiCompatible,
            context_window_tokens: None,
            max_output_tokens: None,
            model: "glm-5-turbo".into(),
            base_url: "https://open.bigmodel.cn/api/coding/paas/v4/chat/completions/".into(),
            provider_kind: None,
            vendor: None,
            endpoint_mode: Some("full_chat_completions".into()),
            api_key: String::new(),
            credential_ref: None,
            credential_state: CredentialState::Missing,
            has_secret: false,
            credential_action: None,
        });

        let model = prefs.model_by_id("glm-coding").expect("coding model");
        assert_eq!(
            model.base_url,
            "https://open.bigmodel.cn/api/coding/paas/v4"
        );
        assert_eq!(
            model.provider_kind.as_deref(),
            Some(MODEL_PROVIDER_KIND_CODING_PLAN)
        );
        assert_eq!(model.vendor.as_deref(), Some("glm"));
        assert!(model.endpoint_mode.is_none());
    }

    #[test]
    fn normal_glm_api_is_not_migrated_to_coding_plan() {
        let mut prefs = UserPrefs::default();
        prefs.migrate_models();
        prefs.upsert_model(SavedModel {
            id: "glm-api".into(),
            name: "GLM API".into(),
            preset: ModelPreset::Glm,
            context_window_tokens: None,
            max_output_tokens: None,
            model: "glm-5.2".into(),
            base_url: "https://open.bigmodel.cn/api/paas/v4".into(),
            provider_kind: None,
            vendor: None,
            endpoint_mode: None,
            api_key: String::new(),
            credential_ref: None,
            credential_state: CredentialState::Missing,
            has_secret: false,
            credential_action: None,
        });

        let model = prefs.model_by_id("glm-api").expect("glm model");
        assert_eq!(model.base_url, "https://open.bigmodel.cn/api/paas/v4");
        assert_eq!(
            model.provider_kind.as_deref(),
            Some(MODEL_PROVIDER_KIND_OFFICIAL_API)
        );
        assert!(model.vendor.is_none());
    }

    #[test]
    fn remove_active_model_falls_back_to_first() {
        let mut prefs = UserPrefs::default();
        prefs.migrate_models();
        prefs.upsert_model(SavedModel {
            id: "m2".into(),
            name: "Kimi".into(),
            preset: ModelPreset::Kimi,
            context_window_tokens: None,
            max_output_tokens: None,
            model: "kimi-k2.6".into(),
            base_url: "https://api.moonshot.cn/v1".into(),
            provider_kind: None,
            vendor: None,
            endpoint_mode: None,
            api_key: String::new(),
            credential_ref: None,
            credential_state: CredentialState::Missing,
            has_secret: false,
            credential_action: None,
        });
        prefs.advanced.active_model_id = Some("m2".into());
        prefs.remove_model("m2");
        assert_eq!(prefs.advanced.active_model_id.as_deref(), Some("default"));
        assert!(prefs.model_by_id("m2").is_none());
    }

    #[test]
    fn saved_model_api_key_is_not_serialized() {
        let mut prefs = UserPrefs::default();
        prefs.migrate_models();
        prefs.advanced.saved_models[0].api_key = "sk-test-secret-1234567890".into();
        prefs.advanced.custom_api_key = Some("sk-legacy-secret-1234567890".into());

        let json = serde_json::to_string(&prefs).unwrap();

        assert!(!json.contains("sk-test-secret"));
        assert!(!json.contains("sk-legacy-secret"));
        assert!(!json.contains("custom_api_key"));
    }

    #[test]
    fn migrate_saved_model_plaintext_key_to_reference_with_memory_store() {
        let store = MemoryCredentialStore::default();
        let mut prefs = UserPrefs::default();
        prefs.migrate_models();
        prefs.advanced.saved_models[0].api_key = "sk-model-secret-1234567890".into();

        let result = prefs.migrate_plaintext_api_keys_with_store(&store);

        assert_eq!(result.migrated_count, 1);
        assert!(result.settings_sanitized);
        let model = &prefs.advanced.saved_models[0];
        let reference = model.credential_ref.clone().expect("credential reference");
        assert_eq!(model.credential_state, CredentialState::Configured);
        assert!(model.has_secret);
        assert!(model.api_key.is_empty());
        assert_eq!(
            store.get(&reference).unwrap().as_deref(),
            Some("sk-model-secret-1234567890")
        );
    }

    #[test]
    fn migrate_custom_api_key_to_active_model_with_memory_store() {
        let store = MemoryCredentialStore::default();
        let mut prefs = UserPrefs::default();
        prefs.advanced.model_preset = Some(ModelPreset::Deepseek);
        prefs.advanced.custom_api_key = Some("sk-custom-secret-1234567890".into());
        prefs.migrate_models();

        let result = prefs.migrate_plaintext_api_keys_with_store(&store);

        assert_eq!(result.migrated_count, 1);
        assert!(result.settings_sanitized);
        assert!(prefs.advanced.custom_api_key.is_none());
        let model = prefs.active_model().expect("active model");
        let reference = model.credential_ref.clone().expect("credential reference");
        assert_eq!(model.credential_state, CredentialState::Configured);
        assert_eq!(
            store.get(&reference).unwrap().as_deref(),
            Some("sk-custom-secret-1234567890")
        );
    }

    #[test]
    fn credential_migration_is_idempotent() {
        let store = MemoryCredentialStore::default();
        let mut prefs = UserPrefs::default();
        prefs.migrate_models();
        prefs.advanced.saved_models[0].api_key = "sk-once-secret-1234567890".into();

        let first = prefs.migrate_plaintext_api_keys_with_store(&store);
        let second = prefs.migrate_plaintext_api_keys_with_store(&store);

        assert_eq!(first.migrated_count, 1);
        assert_eq!(second.migrated_count, 0);
        assert_eq!(second.failed_model_ids.len(), 0);
        assert!(!second.settings_sanitized);
        let model = &prefs.advanced.saved_models[0];
        assert!(model.api_key.is_empty());
        assert_eq!(model.credential_state, CredentialState::Configured);
    }

    #[test]
    fn prefs_roundtrip() {
        let prefs = UserPrefs {
            theme: Theme::LiquidDark,
            color_scheme: ColorScheme::Dark,
            language: Language::En,
            memory_enabled: false,
            search: SearchPrefs::default(),
            notifications: NotificationPrefs::default(),
            pet: PetPrefs::default(),
            sidebar: SidebarPrefs::default(),
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
        #[cfg(target_os = "linux")]
        {
            assert!(!prefs.notifications.enabled);
            assert!(!prefs.notifications.task_completed);
        }
        #[cfg(not(target_os = "linux"))]
        {
            assert!(prefs.notifications.enabled);
            assert!(prefs.notifications.task_completed);
        }
        assert!(prefs.advanced.allow_shell.is_none());
    }

    #[test]
    fn notification_prefs_default_matches_platform() {
        let prefs = UserPrefs::default();
        assert_eq!(prefs.notifications.enabled, !cfg!(target_os = "linux"));
        assert_eq!(
            prefs.notifications.task_completed,
            !cfg!(target_os = "linux")
        );

        let json = r#"{"notifications":{"task_completed":false}}"#;
        let parsed: UserPrefs = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.notifications.enabled, !cfg!(target_os = "linux"));
        assert!(!parsed.notifications.task_completed);
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
    fn speech_recognition_locale_maps_to_speech_framework_ids() {
        // macOS Speech 框架的 locale 必须是它支持的 BCP 47 标识;
        // 与 UI 语言保持一致,避免「中文 UI 但英文识别」错配。
        assert_eq!(Language::ZhHans.speech_recognition_locale(), "zh-CN");
        assert_eq!(Language::En.speech_recognition_locale(), "en-US");
        assert_eq!(Language::Ja.speech_recognition_locale(), "ja-JP");
    }

    #[test]
    fn memory_is_only_available_for_zh_hans() {
        assert!(Language::ZhHans.supports_memory());
        assert!(!Language::En.supports_memory());
        assert!(!Language::Ja.supports_memory());

        let mut english = UserPrefs {
            language: Language::En,
            memory_enabled: true,
            ..Default::default()
        };
        assert!(english.enforce_memory_locale_policy());
        assert!(!english.memory_enabled);

        let mut japanese = UserPrefs {
            language: Language::Ja,
            memory_enabled: true,
            ..Default::default()
        };
        assert!(japanese.enforce_memory_locale_policy());
        assert!(!japanese.memory_enabled);

        let mut chinese = UserPrefs {
            language: Language::ZhHans,
            memory_enabled: true,
            ..Default::default()
        };
        assert!(!chinese.enforce_memory_locale_policy());
        assert!(chinese.memory_enabled);
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
                ..Default::default()
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&prefs).unwrap();
        let parsed: UserPrefs = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.search.provider, SearchProvider::Metaso);
        assert!(parsed.search.api_key.is_none());
        assert!(!json.contains("mk-user-own-key"));
    }

    #[test]
    fn search_prefs_normalized_api_key_treats_blank_as_none() {
        for raw in [None, Some("".to_string()), Some("   \n\t ".to_string())] {
            let prefs = SearchPrefs {
                provider: SearchProvider::Metaso,
                api_key: raw,
                ..Default::default()
            };
            assert!(prefs.normalized_api_key().is_none());
        }

        let prefs = SearchPrefs {
            provider: SearchProvider::Metaso,
            api_key: Some("  mk-user-key  ".to_string()),
            ..Default::default()
        };
        assert_eq!(prefs.normalized_api_key().as_deref(), Some("mk-user-key"));
    }

    #[test]
    fn prefs_save_normalizes_blank_search_api_key_on_disk() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
                ..Default::default()
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
    fn migrate_search_plaintext_key_to_provider_credential() {
        let store = MemoryCredentialStore::default();
        let mut prefs = UserPrefs {
            search: SearchPrefs {
                provider: SearchProvider::Metaso,
                api_key: Some("mk-search-secret-1234567890".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let result = prefs.migrate_plaintext_api_keys_with_store(&store);

        assert_eq!(result.migrated_count, 1);
        assert!(result.settings_sanitized);
        assert!(prefs.search.api_key.is_none());
        let credential = prefs
            .search
            .credentials
            .get(&SearchProvider::Metaso)
            .expect("metaso credential");
        let reference = credential
            .credential_ref
            .clone()
            .expect("credential reference");
        assert_eq!(credential.credential_state, CredentialState::Configured);
        assert!(credential.has_secret);
        assert!(credential.api_key.is_empty());
        assert_eq!(
            store.get(&reference).unwrap().as_deref(),
            Some("mk-search-secret-1234567890")
        );
    }

    #[test]
    fn search_prefs_partial_json_fills_defaults() {
        // 老的 settings.json 没 search 字段 → 默认 Bing/None,不破坏向前兼容。
        let json = r#"{"theme":"genesis","language":"zh-Hans"}"#;
        let prefs: UserPrefs = serde_json::from_str(json).unwrap();
        assert_eq!(prefs.search.provider, SearchProvider::Bing);
        assert!(prefs.search.api_key.is_none());
    }

    /// 回归:credential_ref 存在但存储里是空值时,credential_state 必须是 Missing,
    /// 不能被假阳性地标成 Configured(根因:macOS Keychain 存空值 + 旧 sanitize 盲置)。
    #[test]
    fn refresh_does_not_mark_configured_when_store_value_is_empty() {
        use crate::platform::credential_store::{CredentialReference, MemoryCredentialStore};

        // 存储里有 credential_ref 指向的条目,但值为空字符串。
        let store = MemoryCredentialStore::default();
        let reference = CredentialReference::for_model("default");
        store.set(&reference, "").unwrap(); // 空值!

        let mut prefs = UserPrefs::default();
        prefs.migrate_models();
        // 模拟"之前保存过 key"的状态:有 ref + state 曾被标 Configured。
        let model = &mut prefs.advanced.saved_models[0];
        model.credential_ref = Some(reference);
        model.credential_state = CredentialState::Configured;
        model.has_secret = true;

        // refresh 真实回读 → 发现空值 → 标 Missing。
        prefs.refresh_credential_states_with_store(&store);
        // sanitize 不应再把 Missing 盲改回 Configured(这是之前的 bug)。
        prefs.sanitize_plaintext_api_keys();

        let model = &prefs.advanced.saved_models[0];
        assert_eq!(
            model.credential_state,
            CredentialState::Missing,
            "空值存储不应被标为 Configured(假阳性会导致云端调用拿空 key → 401)"
        );
        assert!(!model.has_secret);
    }
}

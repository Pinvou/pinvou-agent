//! GUI 可调的用户偏好 + 开发者后门高级字段。
//!
//! 序列化到 `~/.pinvou3/settings.json`。前 3 个字段（theme / color_scheme / language）
//! 暴露在 Settings 面板里；`advanced` 是不进 UI 的开发者后门——可通过手改
//! `settings.json` 或对应的 `PINVOU3_*` 环境变量调整。

use serde::{Deserialize, Serialize};
use std::sync::{Mutex, MutexGuard};

use crate::core::mode_state::SerializableMode;
use crate::platform::credential_store::{
    CredentialEditAction, CredentialMigrationResult, CredentialReference, CredentialState,
    CredentialStore, SystemCredentialStore,
};

mod search;
pub use search::{SearchCredential, SearchPrefs, SearchProvider};

/// `settings.json` 的进程内统一读写锁。
///
/// Tauri 命令会在不同异步任务中并发执行；如果各自执行 `load -> 修改 -> save`，
/// 后完成的旧快照会覆盖先完成的新值。所有偏好读写和字段级事务都必须经过此锁。
static USER_PREFS_LOCK: Mutex<()> = Mutex::new(());

fn lock_user_prefs() -> MutexGuard<'static, ()> {
    USER_PREFS_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum Theme {
    #[default]
    Genesis,
    LiquidLight,
    LiquidDark,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum ColorScheme {
    Light,
    Dark,
    #[default]
    System,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum Language {
    #[serde(rename = "zh-Hans")]
    #[default]
    ZhHans,
    #[serde(rename = "en")]
    En,
    /// 日语。底座 prompts.rs 的 translation_target_language_for_tag 已认识 "ja"，
    /// LLM 回复语言链路零改动。
    #[serde(rename = "ja")]
    Ja,
}
impl Language {
    fn from_system_locale(locale: Option<&str>) -> Self {
        let Some(locale) = locale.map(str::trim).filter(|locale| !locale.is_empty()) else {
            return Language::En;
        };
        let primary = locale
            .split(|ch: char| matches!(ch, '-' | '_' | '.' | '@' | ':'))
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        match primary.as_str() {
            "zh" => Language::ZhHans,
            "ja" => Language::Ja,
            "en" => Language::En,
            // 品悟当前只提供中、英、日；其它系统语言使用英文，而不是误显示中文。
            _ => Language::En,
        }
    }

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
                 (e.g. `File`, `Bash`), environment variables, \
                 command-line flags, and URLs verbatim — only natural-language \
                 prose follows the language rule.",
            ),
            // 底座已注入对应 bookend,避免重复。
            Language::ZhHans | Language::Ja => None,
        }
    }
}

pub(crate) mod model;
use model::{
    identify_coding_plan_endpoint, migrated_minimax_base_url, strip_chat_completions_suffix,
};
pub use model::{
    ModelPreset, MODEL_PROVIDER_KIND_CODING_PLAN, MODEL_PROVIDER_KIND_CUSTOM,
    MODEL_PROVIDER_KIND_OFFICIAL_API,
};

/// 用户对某条 [`SavedModel`] 图片输入能力的显式覆盖(模型设置页「图片输入能力」,
/// 设计 §6.3/§7.3)。`Auto` = 走能力解析链(模型目录→内置已验证表→Unknown);
/// `Enabled`/`Disabled` 直接钉死,供本地自定义模型人工确认用。
///
/// 反序列化手写兜底:未知档位值落 `Auto` 而非报错。没有这一层,单个未知值
/// (未来版本新增枚举后降级运行/手工编辑)会让整份 `UserPrefs` 反序列化失败,
/// `load` 整体回退默认值,此后任意一次设置写入都会把用户的全部模型条目与
/// 凭据引用不可逆覆盖。落 `Auto` 后写回即规范化为 `"auto"`。不能改用
/// `#[serde(other)]`:它要求挂在最后一个变体上,而"未知=Disabled"语义危险,
/// 新增兜底变体又会破坏穷举 match 且无法序列化。
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ImageCapabilityOverride {
    /// 自动探测(默认;旧 settings.json 无该字段反序列化即落这里,无感迁移)。
    /// 保存时触发连接 + 识图探测,按结果回填 `Pinvou`/`Enabled`/`Disabled`。
    #[default]
    Auto,
    /// pinvou 决策:按内置已验证能力表判断,不探测(即原「自动判断」语义)。
    Pinvou,
    /// 探测/用户确认该模型支持图片输入(「能」)。
    Enabled,
    /// 探测/用户确认该模型不支持图片输入(「不能」)。
    Disabled,
}

impl<'de> Deserialize<'de> for ImageCapabilityOverride {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = ImageCapabilityOverride;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("image capability override (auto/pinvou/enabled/disabled)")
            }
            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "auto" => Ok(ImageCapabilityOverride::Auto),
                    "pinvou" => Ok(ImageCapabilityOverride::Pinvou),
                    "enabled" => Ok(ImageCapabilityOverride::Enabled),
                    "disabled" => Ok(ImageCapabilityOverride::Disabled),
                    // 未知值兜底:见枚举头注释。走 Auto 而非 Disabled——
                    // "判不出"绝不能冒充"确认不支持"。
                    _ => Ok(ImageCapabilityOverride::Auto),
                }
            }
        }
        deserializer.deserialize_str(Visitor)
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
    /// 用户选择的思考深度档位（透传底座 reasoning_effort：off/low/medium/high/max）。
    /// None = 未显式设置，走 provider 默认（vllm→off 防 SSE timeout，其余→high）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    pub model: String,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_mode: Option<String>,
    /// 图片输入能力覆盖(设计 §6.3):Auto 走能力解析链;Enabled/Disabled 强制。
    #[serde(default)]
    pub image_capability_override: ImageCapabilityOverride,
    /// 视觉兜底模型引用(设计 §9.3):指向另一条 SavedModel 的 `id`,复用其
    /// endpoint 与 `credential_ref`,不保存第二份明文密钥。None = 未配置。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision_model_id: Option<String>,
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
        // reasoning_effort 归一为底座 `ReasoningEffort::parse_strict` 认识的规范档位
        // （off/low/medium/high/auto/max）。别名（disabled/minimum/light/ultra 等）
        // 规范化为对应档位，避免底座 wire 层 `apply_reasoning_effort` 只认规范档位
        // + 少数别名，把 `minimum`/`light`/`ultra`/`maximum` 等静默丢弃；非法值置
        // None 走 provider 默认，避免被底座 `from_setting` 静默回退成 Max。
        if let Some(effort) = self.reasoning_effort.as_deref() {
            self.reasoning_effort = match effort.trim().to_ascii_lowercase().as_str() {
                "off" | "disabled" | "none" | "false" => Some("off".to_string()),
                "low" | "minimum" | "minimal" | "light" => Some("low".to_string()),
                "medium" | "mid" => Some("medium".to_string()),
                "high" => Some("high".to_string()),
                "auto" | "automatic" => Some("auto".to_string()),
                "max" | "maximum" | "xhigh" | "ultra" | "ultracode" => Some("max".to_string()),
                _ => None,
            };
        }
    }

    fn normalize_provider_metadata(&mut self) {
        self.base_url = strip_chat_completions_suffix(&self.base_url);
        // MiniMax 旧域名 api.minimax.chat 已废弃,官方国内端点为 api.minimaxi.com;
        // 存量配置在 load 时一次性改写,避免继续打已下线域名。
        if let Some(migrated) = migrated_minimax_base_url(&self.base_url) {
            self.base_url = migrated;
        }
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
/// 管理)。位置刻意不进 settings.json；开关由字段级事务写入，窗口状态不参与通用设置保存。
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
        Self {
            date_grouping: true,
        }
    }
}

/// 品悟原生 code 会话权限模式的全局记忆。产品语义（已拍板）：
/// - 从未用过 code 模式时，新建 code 会话默认 Plan（只读）；
/// - 新建 code 会话的默认 mode = code lane 的全局 last_mode；
/// - last_mode 只由「code 页草稿态显式切换」写入（已生成会话的切换只写
///   会话自己的记录，不再渗全局——三分 lane 语义复审拍板）；
/// - 首次切 yolo 弹一次性确认卡，确认后全局记住、之后切换不再弹。
///
/// 不进设置 UI；写入走字段级事务（同 PetPrefs），per-session mode 另存
/// `sessions/_session_mode_states.json`（见 `features::sessions`）。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CodePermissionPrefs {
    /// code lane 的全局默认 mode（草稿态显式切换时写入）。None = 从未使用过
    /// code 模式。
    pub last_mode: Option<SerializableMode>,
    /// yolo 一次性确认（"全自动读写项目目录、可执行 shell、无逐步审批"）标志。
    pub yolo_confirmed: bool,
}

/// 工作（work）/ 设计（design）两个 plain lane 的全局默认 mode。
/// 与 code lane（`code_permission.last_mode`）并列——三个工作区 lane 各有
/// 独立全局默认；只由对应 lane 草稿态的显式切换写入，已生成会话的切换不碰。
/// None = 该 lane 从未显式选过 → 缺省 Yolo（与 plain 历史默认一致）。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModeDefaultPrefs {
    pub work: Option<SerializableMode>,
    pub design: Option<SerializableMode>,
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
    pub code_permission: CodePermissionPrefs,
    pub mode_defaults: ModeDefaultPrefs,
    pub advanced: AdvancedPrefs,
}

struct ParsedSettings {
    prefs: UserPrefs,
    allow_normalization_persist: bool,
}

fn should_persist_normalization(allow_persist: bool, requested: bool, changed: bool) -> bool {
    requested && changed && allow_persist
}

impl UserPrefs {
    /// 从 `~/.pinvou3/settings.json` 读。没有有效语言配置时跟随当前系统语言。
    pub fn load() -> Self {
        let _guard = lock_user_prefs();
        Self::load_unlocked(true)
    }

    fn defaults_for_system_locale(locale: Option<&str>) -> Self {
        Self {
            language: Language::from_system_locale(locale),
            ..Self::default()
        }
    }

    fn parse_settings_with_state(raw: Option<&str>, system_locale: Option<&str>) -> ParsedSettings {
        let Some(raw) = raw else {
            return ParsedSettings {
                prefs: Self::defaults_for_system_locale(system_locale),
                allow_normalization_persist: false,
            };
        };
        let value: serde_json::Value = match serde_json::from_str(raw) {
            Ok(value) => value,
            Err(error) => {
                eprintln!(
                    "[pinvou3-app] settings.json parse failed ({error}), using system-language defaults"
                );
                return ParsedSettings {
                    prefs: Self::defaults_for_system_locale(system_locale),
                    allow_normalization_persist: false,
                };
            }
        };
        let has_language = value
            .as_object()
            .is_some_and(|settings| settings.contains_key("language"));
        let mut prefs: Self = match serde_json::from_value(value) {
            Ok(prefs) => prefs,
            Err(error) => {
                eprintln!(
                    "[pinvou3-app] settings.json parse failed ({error}), using system-language defaults"
                );
                return ParsedSettings {
                    prefs: Self::defaults_for_system_locale(system_locale),
                    allow_normalization_persist: false,
                };
            }
        };
        if !has_language {
            prefs.language = Language::from_system_locale(system_locale);
        }
        ParsedSettings {
            prefs,
            allow_normalization_persist: true,
        }
    }

    #[cfg(test)]
    fn parse_settings(raw: Option<&str>, system_locale: Option<&str>) -> Self {
        Self::parse_settings_with_state(raw, system_locale).prefs
    }

    fn load_unlocked(persist_normalized: bool) -> Self {
        let path = super::paths::settings_path();
        let raw = std::fs::read_to_string(&path).ok();
        let system_locale = crate::platform::os::current_system_locale();
        let mut parsed = Self::parse_settings_with_state(raw.as_deref(), system_locale.as_deref());
        let allow_normalization_persist = parsed.allow_normalization_persist;
        let prefs = &mut parsed.prefs;
        // 必须在 migrate_models/normalize 改写前记录；否则只能修正本次运行的内存值，
        // save gate 看不到变化，旧域名会永久留在 settings.json 中、每次启动重复迁移。
        let minimax_endpoint_changed = prefs
            .advanced
            .saved_models
            .iter()
            .any(|model| migrated_minimax_base_url(&model.base_url).is_some())
            || prefs
                .advanced
                .custom_base_url
                .as_deref()
                .is_some_and(|url| migrated_minimax_base_url(url).is_some());
        prefs.migrate_models();
        prefs.normalize_saved_model_metadata();
        let migration = prefs.migrate_plaintext_api_keys_with_store(&SystemCredentialStore::new());
        let memory_policy_changed = prefs.enforce_memory_locale_policy();
        let normalization_changed =
            minimax_endpoint_changed || migration.settings_sanitized || memory_policy_changed;
        if should_persist_normalization(
            allow_normalization_persist,
            persist_normalized,
            normalization_changed,
        ) {
            if let Err(e) = prefs.save_unlocked() {
                eprintln!("[pinvou3-app] settings normalization save failed: {e:#}");
            }
        }
        prefs.sanitize_plaintext_api_keys();
        parsed.prefs
    }

    pub fn save(&self) -> std::io::Result<()> {
        let _guard = lock_user_prefs();
        self.save_unlocked()
    }

    fn save_unlocked(&self) -> std::io::Result<()> {
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
        // 原子写：直接 std::fs::write 在进程中断时可能留下截断文件，而 load 对
        // 损坏的 settings.json 是回退默认值——code_permission.last_mode 等持久化
        // 偏好会整体丢失，表现为「重启后设置回到默认」。tmp + rename 保证目标
        // 文件永远完整。
        crate::platform::filesystem::atomic_write(&path, s.as_bytes())
    }

    /// 在同一临界区内读取磁盘最新偏好、修改指定字段并写回。
    ///
    /// 闭包必须只修改自己负责的设置域，避免把调用方持有的整份旧快照写回。
    pub fn update_transaction<F>(mutate: F) -> Result<Self, String>
    where
        F: FnOnce(&mut Self) -> Result<(), String>,
    {
        let _guard = lock_user_prefs();
        let mut prefs = Self::load_unlocked(false);
        mutate(&mut prefs)?;
        prefs
            .save_unlocked()
            .map_err(|error| format!("save settings failed: {error:#}"))?;
        // 返回再次从磁盘解析的规范化结果，保证桥接层内存状态与实际持久化内容一致。
        Ok(Self::load_unlocked(false))
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
            reasoning_effort: None,
            model,
            base_url,
            provider_kind: None,
            vendor: None,
            endpoint_mode: None,
            image_capability_override: ImageCapabilityOverride::default(),
            vision_model_id: None,
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

    /// reasoning_effort 归一为底座 `ReasoningEffort::parse_strict` 认识的规范档位：
    /// 非法值置 None（避免被底座静默回退成 Max），合法别名规范化为对应档位
    /// （对齐 `as_setting()`，避免 wire 层 `apply_reasoning_effort` 静默丢弃）。
    #[test]
    fn normalize_reasoning_effort_canonicalizes_aliases_and_rejects_unknown() {
        let base = SavedModel {
            id: "m1".into(),
            name: "m1".into(),
            preset: ModelPreset::OpenaiCompatible,
            context_window_tokens: None,
            max_output_tokens: None,
            reasoning_effort: None,
            model: "m1".into(),
            base_url: "https://example.invalid/v1".into(),
            provider_kind: None,
            vendor: None,
            endpoint_mode: None,
            image_capability_override: Default::default(),
            vision_model_id: None,
            api_key: String::new(),
            credential_ref: None,
            credential_state: CredentialState::Missing,
            has_secret: false,
            credential_action: None,
        };
        let mut invalid = base.clone();
        invalid.reasoning_effort = Some("turbo".into());
        invalid.normalize_route_limits();
        assert_eq!(
            invalid.reasoning_effort, None,
            "非法档位应置 None 而非交给底座静默回退 Max"
        );

        for (alias, canonical) in [
            ("off", "off"),
            ("disabled", "off"),
            ("none", "off"),
            ("false", "off"),
            ("low", "low"),
            ("minimum", "low"),
            ("minimal", "low"),
            ("light", "low"),
            ("medium", "medium"),
            ("mid", "medium"),
            ("high", "high"),
            ("auto", "auto"),
            ("automatic", "auto"),
            ("max", "max"),
            ("maximum", "max"),
            ("xhigh", "max"),
            ("ultra", "max"),
            ("ultracode", "max"),
        ] {
            let mut m = base.clone();
            m.reasoning_effort = Some(alias.into());
            m.normalize_route_limits();
            assert_eq!(
                m.reasoning_effort.as_deref(),
                Some(canonical),
                "别名 {alias} 应规范化为 {canonical}"
            );
        }
    }

    /// 旧版 settings.json（无 reasoning_effort 字段）必须反序列化成功且字段为 None。
    #[test]
    fn saved_model_missing_reasoning_effort_field_defaults_to_none() {
        let json = r#"{"id":"m1","name":"m1","preset":"openai_compatible","model":"gpt-5.4-mini","base_url":"https://api.openai.com/v1"}"#;
        let model: SavedModel = serde_json::from_str(json).expect("旧数据必须能反序列化");
        assert_eq!(model.reasoning_effort, None);
    }

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
            reasoning_effort: None,
            model: "glm-5-turbo".into(),
            base_url: "https://open.bigmodel.cn/api/coding/paas/v4/chat/completions/".into(),
            provider_kind: None,
            vendor: None,
            endpoint_mode: Some("full_chat_completions".into()),
            image_capability_override: ImageCapabilityOverride::default(),
            vision_model_id: None,
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
            reasoning_effort: None,
            model: "glm-5.2".into(),
            base_url: "https://open.bigmodel.cn/api/paas/v4".into(),
            provider_kind: None,
            vendor: None,
            endpoint_mode: None,
            image_capability_override: ImageCapabilityOverride::default(),
            vision_model_id: None,
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
    fn legacy_minimax_chat_domain_is_rewritten() {
        assert_eq!(
            migrated_minimax_base_url("https://api.minimax.chat").as_deref(),
            Some("https://api.minimaxi.com")
        );
        assert_eq!(
            migrated_minimax_base_url("https://API.MINIMAX.CHAT/v1").as_deref(),
            Some("https://api.minimaxi.com/v1")
        );

        let mut prefs = UserPrefs::default();
        prefs.migrate_models();
        prefs.upsert_model(SavedModel {
            id: "minimax-api".into(),
            name: "MiniMax".into(),
            preset: ModelPreset::Minimax,
            context_window_tokens: None,
            max_output_tokens: None,
            reasoning_effort: None,
            model: "MiniMax-M3".into(),
            base_url: "https://api.minimax.chat/v1".into(),
            provider_kind: None,
            vendor: None,
            endpoint_mode: None,
            image_capability_override: ImageCapabilityOverride::default(),
            vision_model_id: None,
            api_key: String::new(),
            credential_ref: None,
            credential_state: CredentialState::Missing,
            has_secret: false,
            credential_action: None,
        });

        let model = prefs.model_by_id("minimax-api").expect("minimax model");
        assert_eq!(model.base_url, "https://api.minimaxi.com/v1");
    }

    #[test]
    fn load_persists_legacy_minimax_domain_migration() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let old_home = std::env::var_os("PINVOU3_HOME");
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-prefs-minimax-domain-migration-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("create temporary prefs home");
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let mut prefs = UserPrefs::default();
        prefs.advanced.saved_models.push(SavedModel {
            id: "minimax-api".into(),
            name: "MiniMax".into(),
            preset: ModelPreset::Minimax,
            context_window_tokens: None,
            max_output_tokens: None,
            reasoning_effort: None,
            model: "MiniMax-M3".into(),
            base_url: "https://api.minimax.chat".into(),
            provider_kind: Some(MODEL_PROVIDER_KIND_OFFICIAL_API.into()),
            vendor: None,
            endpoint_mode: None,
            image_capability_override: ImageCapabilityOverride::default(),
            vision_model_id: None,
            api_key: String::new(),
            credential_ref: None,
            credential_state: CredentialState::Missing,
            has_secret: false,
            credential_action: None,
        });
        prefs.advanced.active_model_id = Some("minimax-api".into());
        let path = super::super::paths::settings_path();
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&prefs).expect("serialize legacy prefs"),
        )
        .expect("write legacy prefs");

        let loaded = UserPrefs::load();
        assert_eq!(
            loaded.active_model().map(|model| model.base_url.as_str()),
            Some("https://api.minimaxi.com")
        );
        let persisted = std::fs::read_to_string(&path).expect("read migrated prefs");
        assert!(!persisted.contains("api.minimax.chat"));
        assert!(persisted.contains("api.minimaxi.com"));

        let _ = std::fs::remove_dir_all(&tmp);
        match old_home {
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
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
            reasoning_effort: None,
            model: "kimi-k2.6".into(),
            base_url: "https://api.moonshot.cn/v1".into(),
            provider_kind: None,
            vendor: None,
            endpoint_mode: None,
            image_capability_override: ImageCapabilityOverride::default(),
            vision_model_id: None,
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
    fn saved_model_image_fields_default_for_legacy_json() {
        // 旧 settings.json 没有 image_capability_override / vision_model_id:
        // serde default 保证无感迁移 → Auto / None(设计 §6.3,阶段 C)。
        let legacy = r#"{
            "id": "m1",
            "name": "DeepSeek 线上",
            "preset": "deepseek",
            "model": "deepseek-v4-pro",
            "base_url": "https://api.deepseek.com"
        }"#;
        let model: SavedModel = serde_json::from_str(legacy).expect("legacy SavedModel json");
        assert_eq!(
            model.image_capability_override,
            ImageCapabilityOverride::Auto
        );
        assert!(model.vision_model_id.is_none());

        // 显式值能序列化往返;Auto/None 时字段不写入(保持 settings.json 干净)。
        let mut overridden = model.clone();
        overridden.image_capability_override = ImageCapabilityOverride::Enabled;
        overridden.vision_model_id = Some("vision-1".into());
        let json = serde_json::to_string(&overridden).unwrap();
        let back: SavedModel = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.image_capability_override,
            ImageCapabilityOverride::Enabled
        );
        assert_eq!(back.vision_model_id.as_deref(), Some("vision-1"));

        let json = serde_json::to_string(&model).unwrap();
        assert!(!json.contains("vision_model_id"));
    }

    #[test]
    fn saved_model_image_capability_unknown_value_falls_back_to_auto() {
        // 未知档位值(未来版本新增枚举后降级运行/手工编辑)必须落 Auto,
        // 而不是让整份 UserPrefs 反序列化失败——后者会让 `load` 整体回退
        // 默认值,此后任意一次设置写入把用户的全部模型条目与凭据引用覆盖。
        let future = r#"{
            "id": "m1",
            "name": "DeepSeek 线上",
            "preset": "deepseek",
            "model": "deepseek-v4-pro",
            "base_url": "https://api.deepseek.com",
            "image_capability_override": "agentic_probe_v2"
        }"#;
        let model: SavedModel = serde_json::from_str(future).expect("unknown override value");
        assert_eq!(
            model.image_capability_override,
            ImageCapabilityOverride::Auto
        );
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
            code_permission: CodePermissionPrefs::default(),
            mode_defaults: ModeDefaultPrefs::default(),
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
    fn system_locale_maps_to_supported_language() {
        assert_eq!(
            Language::from_system_locale(Some("zh_CN.UTF-8")),
            Language::ZhHans
        );
        assert_eq!(
            Language::from_system_locale(Some("zh-Hant-TW")),
            Language::ZhHans
        );
        assert_eq!(Language::from_system_locale(Some("ja-JP")), Language::Ja);
        assert_eq!(Language::from_system_locale(Some("en-US")), Language::En);
        assert_eq!(Language::from_system_locale(Some("fr-FR")), Language::En);
        assert_eq!(Language::from_system_locale(None), Language::En);
    }

    #[test]
    fn missing_settings_uses_system_language() {
        let prefs = UserPrefs::parse_settings(None, Some("ja-JP"));
        assert_eq!(prefs.language, Language::Ja);
    }

    #[test]
    fn invalid_settings_uses_system_language() {
        let prefs = UserPrefs::parse_settings(Some("{broken"), Some("en-US"));
        assert_eq!(prefs.language, Language::En);
    }

    #[test]
    fn invalid_settings_never_allow_normalization_persist() {
        for (raw, locale, expected_language) in [
            ("{broken", "en-US", Language::En),
            (r#"{"language":42}"#, "ja-JP", Language::Ja),
        ] {
            let mut parsed = UserPrefs::parse_settings_with_state(Some(raw), Some(locale));
            assert_eq!(parsed.prefs.language, expected_language);
            assert!(!parsed.allow_normalization_persist);
            parsed.prefs.memory_enabled = true;
            let normalization_changed = parsed.prefs.enforce_memory_locale_policy();
            assert!(normalization_changed);
            assert!(!should_persist_normalization(
                parsed.allow_normalization_persist,
                true,
                normalization_changed,
            ));
        }
    }

    #[test]
    fn missing_settings_do_not_allow_normalization_persist() {
        let parsed = UserPrefs::parse_settings_with_state(None, Some("en-US"));
        assert!(!parsed.allow_normalization_persist);
        assert!(!should_persist_normalization(
            parsed.allow_normalization_persist,
            true,
            true,
        ));
    }

    #[test]
    fn valid_settings_allow_existing_normalization_persist() {
        let mut parsed = UserPrefs::parse_settings_with_state(
            Some(r#"{"language":"en","memory_enabled":true}"#),
            Some("ja-JP"),
        );
        assert!(parsed.allow_normalization_persist);
        let normalization_changed = parsed.prefs.enforce_memory_locale_policy();
        assert!(normalization_changed);
        assert!(should_persist_normalization(
            parsed.allow_normalization_persist,
            true,
            normalization_changed,
        ));
    }

    #[test]
    fn settings_without_language_uses_system_language() {
        let prefs = UserPrefs::parse_settings(Some(r#"{"theme":"genesis"}"#), Some("ja-JP"));
        assert_eq!(prefs.theme, Theme::Genesis);
        assert_eq!(prefs.language, Language::Ja);
    }

    #[test]
    fn explicit_language_overrides_system_language() {
        let prefs = UserPrefs::parse_settings(Some(r#"{"language":"zh-Hans"}"#), Some("en-US"));
        assert_eq!(prefs.language, Language::ZhHans);
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
    fn code_permission_defaults_and_legacy_json_compat() {
        // 默认：从未用过 code 模式（last_mode=None → 新建 code 会话默认 Plan）、未确认 yolo。
        let prefs = UserPrefs::default();
        assert!(prefs.code_permission.last_mode.is_none());
        assert!(!prefs.code_permission.yolo_confirmed);

        // 旧 settings.json 没有 code_permission 字段 → serde default 兼容读出。
        let legacy: UserPrefs = serde_json::from_str(r#"{"theme":"genesis"}"#).unwrap();
        assert!(legacy.code_permission.last_mode.is_none());
        assert!(!legacy.code_permission.yolo_confirmed);

        // 写过的值能回读（mode 与 get_mode_state 协议同用 snake_case）。
        let json = r#"{"code_permission":{"last_mode":"yolo","yolo_confirmed":true}}"#;
        let parsed: UserPrefs = serde_json::from_str(json).unwrap();
        assert_eq!(
            parsed.code_permission.last_mode,
            Some(SerializableMode::Yolo)
        );
        assert!(parsed.code_permission.yolo_confirmed);
        let serialized = serde_json::to_string(&parsed.code_permission).unwrap();
        assert!(serialized.contains("\"last_mode\":\"yolo\""));
        assert!(serialized.contains("\"yolo_confirmed\":true"));
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
    fn concurrent_model_and_search_transactions_preserve_both_changes() {
        use std::sync::{Arc, Barrier};

        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let old_home = std::env::var_os("PINVOU3_HOME");
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-prefs-concurrent-update-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let mut initial = UserPrefs::default();
        initial.migrate_models();
        initial.save().expect("initial settings should save");

        let barrier = Arc::new(Barrier::new(3));
        let model_barrier = Arc::clone(&barrier);
        let model_thread = std::thread::spawn(move || {
            model_barrier.wait();
            UserPrefs::update_transaction(|prefs| {
                let mut model = prefs.advanced.saved_models[0].clone();
                model.id = "concurrent-model".to_string();
                model.name = "Concurrent model".to_string();
                prefs.upsert_model(model);
                Ok(())
            })
            .expect("model transaction should save");
        });

        let search_barrier = Arc::clone(&barrier);
        let search_thread = std::thread::spawn(move || {
            search_barrier.wait();
            UserPrefs::update_transaction(|prefs| {
                prefs.search.provider = SearchProvider::Tavily;
                prefs.search.enabled_providers = vec![SearchProvider::Bing, SearchProvider::Tavily];
                Ok(())
            })
            .expect("search transaction should save");
        });

        barrier.wait();
        model_thread.join().expect("model thread should finish");
        search_thread.join().expect("search thread should finish");

        let saved = UserPrefs::load();
        assert!(saved.model_by_id("concurrent-model").is_some());
        assert_eq!(saved.search.provider, SearchProvider::Tavily);
        assert!(saved
            .search
            .enabled_providers
            .contains(&SearchProvider::Tavily));

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

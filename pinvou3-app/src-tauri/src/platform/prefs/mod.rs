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

/// The environment variable that overrides every saved model's API key:
/// while it is set to a non-empty value,
/// `refresh_credential_states_with_store` classifies all models as
/// [`CredentialState::EnvOverride`]. Defined once so the check and every
/// surface that names the variable (e.g. the CLI's
/// `models show --reveal-key` placeholder) cannot drift apart.
pub const MODEL_API_KEY_ENV_OVERRIDE: &str = "DEEPSEEK_API_KEY";

/// 密封的凭据状态机字段访问:`SavedModel` 与 `SearchCredential` 的凭据字段
/// 同构(`api_key` / `credential_ref` / `credential_state` / `has_secret` /
/// `credential_action`),字段访问由各结构体提供,状态迁移逻辑见
/// [`CredentialStateOps`]——只保留一份实现。私有 `Sealed` 限定实现者,
/// prefs 模块树之外无法进入该状态机。
pub trait CredentialStateAccess: sealed::Sealed {
    fn api_key_mut(&mut self) -> &mut String;
    fn credential_ref_mut(&mut self) -> &mut Option<CredentialReference>;
    fn credential_state_mut(&mut self) -> &mut CredentialState;
    fn has_secret_mut(&mut self) -> &mut bool;
    fn credential_action_mut(&mut self) -> &mut Option<CredentialEditAction>;
}

/// 凭据状态迁移操作,对所有 [`CredentialStateAccess`] 实现统一提供
/// (原 `SavedModel` / `SearchCredential` 各一份的同构方法收编于此)。
pub trait CredentialStateOps: CredentialStateAccess {
    fn clear_plaintext_key(&mut self) {
        self.api_key_mut().clear();
        *self.credential_action_mut() = None;
    }

    fn mark_configured(&mut self, reference: CredentialReference) {
        *self.credential_ref_mut() = Some(reference);
        *self.credential_state_mut() = CredentialState::Configured;
        *self.has_secret_mut() = true;
        self.clear_plaintext_key();
    }

    fn mark_missing(&mut self) {
        *self.credential_ref_mut() = None;
        *self.credential_state_mut() = CredentialState::Missing;
        *self.has_secret_mut() = false;
        self.clear_plaintext_key();
    }

    fn mark_unavailable(&mut self) {
        *self.credential_state_mut() = CredentialState::Unavailable;
        *self.has_secret_mut() = self.credential_ref_mut().is_some();
        self.clear_plaintext_key();
    }
}

impl<T: CredentialStateAccess> CredentialStateOps for T {}

mod sealed {
    /// 实现者限定:仅 prefs 模块树内的凭据载体(`SavedModel` / `SearchCredential`)。
    pub trait Sealed {}
}

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

/// `settings.json` 读-改-写事务的跨进程写者锁（见
/// [`UserPrefs::update_transaction`]）。
///
/// 锁文件是 settings.json 的兄弟文件：每个 PINVOU3_HOME 一把，GUI 与 CLI
/// 天然竞争同一把。只用 OS 锁裁决，锁文件本身永不删除——崩溃进程残留的
/// 空文件不会阻塞下一个写者。用阻塞 `lock()`
/// （Unix flock / Windows LockFileEx，与 organize/install 锁同一类原语）
/// 而不是 try+busy：设置写入是真正的事务、不能静默跳过，偶发排队比把
/// 并发写报成失败更可用。
struct SettingsFileLock {
    file: std::fs::File,
}

impl Drop for SettingsFileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn lock_settings_file() -> std::io::Result<SettingsFileLock> {
    let path = super::paths::settings_path().with_file_name("settings.json.lock");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    file.lock()?;
    Ok(SettingsFileLock { file })
}

/// [`load()`](UserPrefs::load) 的时间上限版设置文件锁：规范化持久化是一个
/// 幂等的优化写（下次 load 会重跑迁移），所以锁被占住时跳过持久化，而不是
/// 像 [`update_transaction`](UserPrefs::update_transaction) 那样无限阻塞——
/// 持锁方是毫秒级临界区，正常竞争在数十毫秒内拿到；超时说明对端被挂起
/// （SIGSTOP/ wedged），此时静默跳过比整文档旧快照写回更安全。
const NORMALIZATION_PERSIST_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

fn try_lock_settings_file_bounded() -> std::io::Result<Option<SettingsFileLock>> {
    let path = super::paths::settings_path().with_file_name("settings.json.lock");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    let deadline = std::time::Instant::now() + NORMALIZATION_PERSIST_LOCK_WAIT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(SettingsFileLock { file })),
            Err(std::fs::TryLockError::WouldBlock) => {
                if std::time::Instant::now() >= deadline {
                    return Ok(None);
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
        }
    }
}

/// Round-43 review: whether settings.json on disk still holds the bytes the
/// current load started from. The normalization persist is only safe when a
/// concurrent `update_transaction` did not commit entirely inside this
/// load's read→lock window — a whole-document stale-snapshot write would
/// revert that commit. An unreadable/absent file counts as changed (skip)
/// unless the load itself saw no file either: writing defaults-shaped
/// normalized content into a home whose settings.json vanished since load
/// loses nothing.
fn disk_still_holds_the_loaded_snapshot(
    path: &std::path::Path,
    raw_at_load: &Option<String>,
) -> bool {
    let disk_now = std::fs::read_to_string(path).ok();
    match (raw_at_load, disk_now) {
        (Some(loaded), Some(disk)) => loaded == &disk,
        (None, None) => true,
        _ => false,
    }
}

/// Boot-bootstrap write (the GUI's `!exists → save()` default snapshot).
/// The exists-check and the write are one step under the same cross-process
/// flock as `update_transaction` (round-41 review): a CLI settings write
/// landing between the check and the save used to be overwritten by the
/// defaults snapshot — the exact whole-file last-writer-wins shape the
/// flock closed for transactions. Failure keeps the old silent-degrade
/// contract (boot must not fail over boot defaults); returns whether a
/// defaults file was written. Round-42 review: the acquire is the BOUNDED
/// try-lock like every other boot-adjacent consumer — the plain blocking
/// lock let a wedged CLI holder (a SIGSTOP'd writer, a keychain prompt
/// under `update_transaction`) stall the GUI's window indefinitely, where
/// the defaults write is idempotent and simply retries next boot.
pub(crate) fn save_boot_defaults_if_absent(prefs: &UserPrefs) -> bool {
    let Ok(Some(_file_lock)) = try_lock_settings_file_bounded() else {
        return false;
    };
    if super::paths::settings_path().exists() {
        return false;
    }
    prefs.save_unlocked().is_ok()
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
            .split(['-', '_', '.', '@', ':'])
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
                 user's latest message. Keep code, file paths, tool names, \
                 environment variables, command-line flags, and URLs \
                 verbatim — only natural-language prose follows the language \
                 rule.",
            ),
            // 底座已注入对应 bookend,避免重复。
            Language::ZhHans | Language::Ja => None,
        }
    }
}

pub(crate) mod model;
pub use model::{
    MODEL_PROVIDER_KIND_CODING_PLAN, MODEL_PROVIDER_KIND_CUSTOM, MODEL_PROVIDER_KIND_OFFICIAL_API,
    ModelPreset,
};
use model::{
    identify_coding_plan_endpoint, migrated_minimax_base_url, strip_chat_completions_suffix,
};

/// 用户对某条 [`SavedModel`] 图片输入能力的显式覆盖(模型设置页「图片输入能力」,
/// 设计 §6.3/§7.3)。`Pinvou` = 走能力解析链(内置已验证表→Unknown);
/// `Enabled`/`Disabled` 直接钉死,供本地自定义模型人工确认用。
///
/// 反序列化手写兜底:未知档位值落 `Pinvou` 而非报错。没有这一层,单个未知值
/// (未来版本新增枚举后降级运行/手工编辑)会让整份 `UserPrefs` 反序列化失败,
/// `load` 整体回退默认值,此后任意一次设置写入都会把用户的全部模型条目与
/// 凭据引用不可逆覆盖。落 `Pinvou` 后写回即规范化为 `"pinvou"`。不能改用
/// `#[serde(other)]`:它要求挂在最后一个变体上,而"未知=Disabled"语义危险,
/// 新增兜底变体又会破坏穷举 match 且无法序列化。
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ImageCapabilityOverride {
    /// pinvou 决策(默认):按内置已验证能力表判断,不探测(原「自动判断」语义)。
    /// 旧 settings.json 无该字段、或残留已下线的 `"auto"`(保存时检测)档,
    /// 反序列化即落这里,无感迁移。
    #[default]
    Pinvou,
    /// 用户确认该模型支持图片输入(「能」)。
    Enabled,
    /// 用户确认该模型不支持图片输入(「不能」)。
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
                formatter.write_str("image capability override (pinvou/enabled/disabled)")
            }
            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    // "auto" 是已下线的「保存时检测」档,残留值按 pinvou 决策迁移。
                    "pinvou" | "auto" => Ok(ImageCapabilityOverride::Pinvou),
                    "enabled" => Ok(ImageCapabilityOverride::Enabled),
                    "disabled" => Ok(ImageCapabilityOverride::Disabled),
                    // 未知值兜底:见枚举头注释。走 Pinvou 而非 Disabled——
                    // "判不出"绝不能冒充"确认不支持"。
                    _ => Ok(ImageCapabilityOverride::Pinvou),
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
    /// Optional user-facing label for cloud model selectors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// 决定 provider 路由 + 模板,复用现有 9 预设枚举。
    pub preset: ModelPreset,
    /// 该具体部署允许的 context window；与发给服务端的 `model` wire name 解耦。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u32>,
    /// Pinvou 对该 route 声明的单轮 output 上限；最终仍受进程级请求上限约束。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// 用户选择的思考深度档位（透传底座 reasoning_effort：off/low/medium/high/max）。
    /// None = not explicitly set; the provider default applies (the local
    /// default is the lowest thinking tier: vllm→low, probed ollama→high —
    /// real-machine testing shows local models cannot reliably turn thinking
    /// off; everything else→high).
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
    /// 图片输入能力覆盖(设计 §6.3):Pinvou 走能力解析链;Enabled/Disabled 强制。
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

/// Frozen snapshot of the pre-#622 route predicates for the one-time
/// machine-written-thinking-off migration. These only have to classify
/// records written by pre-#622 builds, so they deliberately mirror
/// `bridge::base_url_uses_local_or_private` /
/// `bridge::is_official_deepseek_base_url` as of #622 and are allowed to
/// drift afterwards: this is a historical classifier, not live routing. The
/// contract test `prefs_legacy_local_route_snapshot_matches_bridge_predicate`
/// (bridge tests) pins today's equality so a future change to the bridge
/// functions forces a conscious snapshot decision.
pub(crate) fn legacy_official_deepseek_base_url(base_url: &str) -> bool {
    let normalized = base_url
        .trim()
        .trim_end_matches('/')
        .trim_end_matches("/beta")
        .trim_end_matches("/v1")
        .to_ascii_lowercase();
    normalized == "https://api.deepseek.com"
}

pub(crate) fn legacy_local_route_base_url(base_url: &str) -> bool {
    reqwest::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .is_some_and(|host| {
            let host = host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim_end_matches('.');
            if host.eq_ignore_ascii_case("localhost") {
                return true;
            }
            if host.eq_ignore_ascii_case("host.docker.internal")
                || host.eq_ignore_ascii_case("host.lima.internal")
                || host.eq_ignore_ascii_case("host.orbstack.internal")
                || host.ends_with(".docker.internal")
            {
                return true;
            }
            let Ok(address) = host.parse::<std::net::IpAddr>() else {
                return false;
            };
            if address.is_loopback() {
                return true;
            }
            match address {
                std::net::IpAddr::V4(v4) => v4.is_private(),
                std::net::IpAddr::V6(_) => false,
            }
        })
}

impl SavedModel {
    /// Operator-owned endpoint: locally self-hosted (LocalVllm) or a
    /// user-entered OpenAI-compatible / custom endpoint. The output ceiling
    /// of these endpoints is the deployer's own responsibility; the host
    /// declares the route output fact by window tier on their behalf (see
    /// `bridge::route_limits_for_model`). `coding_plan` is the official
    /// managed entry point and does not count as operator-owned even when it
    /// rides on the `OpenaiCompatible` preset — it must stay base
    /// fail-closed, with no declaration on its behalf.
    pub fn is_operator_owned_endpoint(&self) -> bool {
        (self.preset == ModelPreset::LocalVllm
            || self.preset == ModelPreset::OpenaiCompatible
            || self.provider_kind.as_deref() == Some("custom"))
            && self.provider_kind.as_deref() != Some("coding_plan")
    }

    fn normalize_alias(&mut self) {
        if self.preset == ModelPreset::LocalVllm {
            self.alias = None;
            return;
        }
        self.alias = self
            .alias
            .take()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
    }

    /// Whether this record's stored `reasoning_effort:"off"` may be a
    /// machine-written pre-#622 local default rather than an explicit user
    /// choice (see `UserPrefs::migrate_legacy_local_thinking_default`).
    /// Deliberately conservative — wrongly keeping a legacy `off` only
    /// preserves the old behavior, while wrongly stripping would take away
    /// a real choice — so anything the old form would have resolved to a
    /// non-local provider keeps its `off`:
    /// - official DeepSeek base URLs won provider resolution even for local
    ///   presets (old default `high`);
    /// - known-vendor or coding-plan records resolved through the vendor
    ///   table (old default `high`/null); unknown free-text vendors keep
    ///   their `off` too — the old form fell through to the local check
    ///   for those, but saved vendor values only come from catalog groups,
    ///   and keeping a machine-written `off` merely preserves the old
    ///   behavior;
    /// - public OpenAI-compatible endpoints had no tier control at all.
    pub(crate) fn legacy_machine_written_local_thinking_off(&self) -> bool {
        if self.reasoning_effort.as_deref() != Some("off") {
            return false;
        }
        if legacy_official_deepseek_base_url(&self.base_url) {
            return false;
        }
        if self.preset == ModelPreset::LocalVllm {
            // `local_vllm` resolved to the vLLM wire before any vendor
            // check, so the old settings page machine-wrote `off` for
            // every one of these records.
            return true;
        }
        self.preset == ModelPreset::OpenaiCompatible
            && self.provider_kind.as_deref() != Some(MODEL_PROVIDER_KIND_CODING_PLAN)
            && self
                .vendor
                .as_deref()
                .map_or(true, |vendor| vendor.trim().is_empty())
            && legacy_local_route_base_url(&self.base_url)
    }

    fn normalize_route_limits(&mut self) {
        self.context_window_tokens = self.context_window_tokens.filter(|tokens| *tokens > 0);
        self.max_output_tokens = self.max_output_tokens.filter(|tokens| *tokens > 0);
        if self.preset == ModelPreset::LocalVllm {
            if self.context_window_tokens.is_none() && self.model == "qwen36_35b_256k" {
                self.context_window_tokens = Some(262_144);
            }
            // The output cap is no longer forced to 24K for the LocalVllm
            // preset: when not explicitly configured it takes the same path
            // as custom OpenAI-compatible endpoints and is declared uniformly
            // by window tier in route_limits_for_model (>=500K→131072 /
            // >=250K→65536 / otherwise min(window/4, 32768)). Older versions
            // machine-wrote the 24K into existing configs (forced here +
            // prefilled in the settings page, then saved), making it
            // indistinguishable on disk from explicit user input; a local
            // model whose value is exactly 24576 is normalized away as
            // "legacy unset" so the window tiers also apply to upgraded
            // users (the persistence gate writes this normalization back to
            // disk on load). Trade-off: from now on explicitly configuring
            // 24576 on a local model is also treated as unset — 24576 is
            // thereby kept as a legacy sentinel value, in the same family as
            // the <=0 filter above. 24576 on non-LocalVllm endpoints is
            // always explicit input and is not migrated.
            if self.max_output_tokens == Some(24_576) {
                self.max_output_tokens = None;
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
                if matches!(
                    self.preset,
                    ModelPreset::OpenaiCompatible | ModelPreset::OpenaiResponses
                ) {
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
}

impl sealed::Sealed for SavedModel {}

impl CredentialStateAccess for SavedModel {
    fn api_key_mut(&mut self) -> &mut String {
        &mut self.api_key
    }
    fn credential_ref_mut(&mut self) -> &mut Option<CredentialReference> {
        &mut self.credential_ref
    }
    fn credential_state_mut(&mut self) -> &mut CredentialState {
        &mut self.credential_state
    }
    fn has_secret_mut(&mut self) -> &mut bool {
        &mut self.has_secret
    }
    fn credential_action_mut(&mut self) -> &mut Option<CredentialEditAction> {
        &mut self.credential_action
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
    /// Max subagents. Note: with swarm mode on (the multi-agent switch), this
    /// is overridden by the foundation hard ceilings (128 pins both the
    /// engine's max_subagents and launch_concurrency; 1024 pins
    /// max_admitted_subagents), an explicit 0 included — 0 was never a runtime
    /// "disable" (the base itself clamps to at least 1).
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

/// Computer Use preferences. Only the master switch is stored: off by default; only after
/// the user explicitly enables it can the model see the `computer_use` tool. The switch is
/// written by the dedicated `computer_use_set_enabled` command via a field-level
/// transaction (same as PetPrefs, not part of the generic settings patch), and replayed at
/// startup by lib.rs into `ComputerUseShared`'s AtomicBool; session grants are never
/// persisted (they live only in memory).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ComputerUsePrefs {
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

/// Global memory of pinvou's native code-session permission mode. Product
/// semantics (settled in review):
/// - when code mode has never been used, a new code session defaults to Plan
///   (read-only);
/// - a new code session's default mode = the code lane's global last_mode;
/// - last_mode is written only by an explicit draft-state switch on the code
///   page (a switch inside an already-materialized session writes only that
///   session's own record and never leaks into globals — the two-lane
///   semantics settled in review);
/// - the first switch to yolo shows a one-shot confirmation card; once
///   confirmed it is remembered globally and later switches do not prompt.
///
/// Not exposed in the settings UI; writes go through field-level
/// transactions (same as PetPrefs); per-session mode is stored separately in
/// `sessions/_session_mode_states.json` (see `features::sessions`).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CodePermissionPrefs {
    /// The code lane's global default mode (written on an explicit
    /// draft-state switch). None = code mode has never been used.
    pub last_mode: Option<SerializableMode>,
    /// One-shot yolo confirmation ("full read/write of the project directory,
    /// shell execution allowed, no step-by-step approval") flag.
    pub yolo_confirmed: bool,
}

/// The work lane's global default mode.
/// Parallel to the code lane (`code_permission.last_mode`) — each workspace
/// lane keeps an independent global default; written only by an explicit
/// draft-state switch in the matching lane, never touched by switches inside
/// already-materialized sessions.
/// None = the lane was never explicitly chosen → default Yolo (matching the
/// historical plain default).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModeDefaultPrefs {
    pub work: Option<SerializableMode>,
    /// Kept only for backward-compatible reads of legacy settings.json (the
    /// design lane was merged into work): at startup, if work is empty this
    /// field backfills the work in-memory mirror (read fold, see
    /// `features/sessions/store.rs::from_paths`); it never receives a
    /// semantic write.
    /// Must round-trip verbatim through whole-preferences writes (no
    /// skip_serializing): the fold is not written back to disk, so if an
    /// unrelated preferences write dropped this field, one write plus a
    /// restart before the user explicitly sets work would leave the fold
    /// without a source and silently lose the explicitly chosen default.
    pub design: Option<SerializableMode>,
}

/// 用户偏好。`settings.json` 顶层结构。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UserPrefs {
    pub theme: Theme,
    /// Color-scheme preference: `system` follows the OS (decided by the
    /// frontend, light when undeterminable), light/dark are explicit picks.
    /// Introduced later than `theme`; when missing from old settings, `load`
    /// derives it from `theme`, while fresh installs keep the `system` default.
    pub color_scheme: ColorScheme,
    pub language: Language,
    pub memory_enabled: bool,
    pub search: SearchPrefs,
    pub notifications: NotificationPrefs,
    pub pet: PetPrefs,
    pub sidebar: SidebarPrefs,
    pub code_permission: CodePermissionPrefs,
    pub mode_defaults: ModeDefaultPrefs,
    /// Global Alt voice shortcut switch (native keyboard hook, currently
    /// effective on Windows only; other platforms keep the in-window Alt
    /// fallback path). Off by default; the user must opt in. The authoritative
    /// persistence lives in settings.json (frontend localStorage is only a
    /// mirror): `set_voice_shortcut_enabled` writes through a field-level
    /// transaction, and at startup lib.rs setup replays it into the native
    /// hook's AtomicBool — preventing the setting from being lost when WebView
    /// storage is cleared, and races where a later multi-window mount invoke
    /// overwrites an earlier one.
    pub voice_shortcut_enabled: bool,
    pub computer_use: ComputerUsePrefs,
    /// One-time migration marker (#622): pre-#622 settings pages prefilled
    /// the local thinking default `off` into the form and the save handler
    /// wrote it back unconditionally, so form-saved local records carry a
    /// `reasoning_effort:"off"` no user chose (see
    /// `UserPrefs::migrate_legacy_local_thinking_default`). The first
    /// post-#622 load strips those and sets this flag so an `off` saved
    /// after the upgrade is never re-stripped. Skipped while false so
    /// pre-#622 files and fresh installs carry no marker noise; note a
    /// downgrade-then-re-upgrade drops the flag (old builds ignore unknown
    /// fields), which re-strips once — the same accepted family as the
    /// marker-free 24576 sentinel below.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub local_thinking_default_migrated: bool,
    pub advanced: AdvancedPrefs,
    /// Ids of builtin-plugin features the user has turned off
    /// (docs/builtin-toolset-contract.md §3.3; the feature registry and the
    /// union removal semantics live in `features::marketplace::builtin`).
    /// Empty by default = everything enabled.
    pub disabled_builtin_features: Vec<String>,
}

struct ParsedSettings {
    prefs: UserPrefs,
    allow_normalization_persist: bool,
    /// Set when `color_scheme` was just derived from `theme` because the old
    /// settings lacked the key (see the parse comment). Counted into the
    /// normalization persist gate so the derived value lands on disk at the
    /// first load and the file becomes self-describing.
    color_scheme_derived: bool,
}

fn should_persist_normalization(allow_persist: bool, requested: bool, changed: bool) -> bool {
    requested && changed && allow_persist
}

impl UserPrefs {
    /// One-time migration for #622 ("local models default to the lowest
    /// thinking tier"): pre-#622 settings pages prefilled the local default
    /// `off` into the thinking control and the save handler wrote it back
    /// unconditionally, so form-saved local records carry a
    /// `reasoning_effort:"off"` that no user chose. Stored values win over
    /// provider defaults at runtime, so without this migration the new
    /// lowest-tier default never reaches upgraded users. Same disclosed
    /// trade-off as the 24576 output-cap sentinel: an explicitly chosen
    /// `off` on a local route is also reset once (one click to restore, and
    /// the marker keeps every later load from touching it again).
    fn migrate_legacy_local_thinking_default(&mut self) {
        if self.local_thinking_default_migrated {
            return;
        }
        for model in &mut self.advanced.saved_models {
            if model.legacy_machine_written_local_thinking_off() {
                model.reasoning_effort = None;
            }
        }
        // Marked done even when nothing matched: the flag means "this file
        // has been seen by post-#622 semantics", so an `off` a user saves
        // after the upgrade is never re-stripped by a later load.
        self.local_thinking_default_migrated = true;
    }

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
                color_scheme_derived: false,
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
                    color_scheme_derived: false,
                };
            }
        };
        let has_language = value
            .as_object()
            .is_some_and(|settings| settings.contains_key("language"));
        // `color_scheme` is the color-scheme preference (system = follow the
        // OS). The field was introduced later than `theme`: when old settings
        // lack the key we cannot tell "the user explicitly picked theme" from
        // "never touched (default genesis, dark)", so derive from the legacy
        // effective appearance to keep the upgrade invisible; only a true
        // fresh install (no settings.json, handled by the default branch
        // above) stays on System and follows the OS.
        let has_color_scheme = value
            .as_object()
            .is_some_and(|settings| settings.contains_key("color_scheme"));
        let mut prefs: Self = match serde_json::from_value(value) {
            Ok(prefs) => prefs,
            Err(error) => {
                eprintln!(
                    "[pinvou3-app] settings.json parse failed ({error}), using system-language defaults"
                );
                return ParsedSettings {
                    prefs: Self::defaults_for_system_locale(system_locale),
                    allow_normalization_persist: false,
                    color_scheme_derived: false,
                };
            }
        };
        if !has_language {
            prefs.language = Language::from_system_locale(system_locale);
        }
        let mut color_scheme_derived = false;
        if !has_color_scheme {
            prefs.color_scheme = match prefs.theme {
                Theme::LiquidLight => ColorScheme::Light,
                Theme::Genesis | Theme::LiquidDark => ColorScheme::Dark,
            };
            color_scheme_derived = true;
        }
        ParsedSettings {
            prefs,
            allow_normalization_persist: true,
            color_scheme_derived,
        }
    }

    #[cfg(test)]
    fn parse_settings(raw: Option<&str>, system_locale: Option<&str>) -> Self {
        Self::parse_settings_with_state(raw, system_locale).prefs
    }

    fn load_unlocked(persist_normalized: bool) -> Self {
        Self::load_unlocked_with_migration(persist_normalized).0
    }

    /// `load_unlocked` plus the plaintext→keychain migration outcome. The
    /// transactional save path needs it (round-41 review M1): on a migration
    /// failure the plaintext key is still in memory only, and the in-memory
    /// sanitize below means a subsequent `save_unlocked` would rewrite
    /// settings.json without it — erasing the credential from its only
    /// durable location. Callers that persist must refuse first (same gate
    /// as the GUI's `prepare_prefs_for_save`).
    fn load_unlocked_with_migration(persist_normalized: bool) -> (Self, CredentialMigrationResult) {
        let path = super::paths::settings_path();
        let raw = std::fs::read_to_string(&path).ok();
        let system_locale = crate::platform::os::current_system_locale();
        let mut parsed = Self::parse_settings_with_state(raw.as_deref(), system_locale.as_deref());
        let allow_normalization_persist = parsed.allow_normalization_persist;
        let color_scheme_derived = parsed.color_scheme_derived;
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
        let local_model_alias_changed = prefs
            .advanced
            .saved_models
            .iter()
            .any(|model| model.preset == ModelPreset::LocalVllm && model.alias.is_some());
        // The local 24576 legacy-sentinel migration (`normalize_route_limits`
        // normalizes the machine-written 24K to unset) is the same case: it
        // must be recorded before migrate/normalize rewrite the models,
        // otherwise the save gate sees no change and the 24K in existing
        // settings.json stays on disk forever.
        let local_output_sentinel_changed = prefs.advanced.saved_models.iter().any(|model| {
            model.preset == ModelPreset::LocalVllm && model.max_output_tokens == Some(24_576)
        });
        // The #622 machine-written-local-off migration must persist its
        // marker even when no record matched: "migration ran" is what keeps
        // a later explicitly-saved `off` from being stripped by a
        // subsequent load.
        let local_thinking_default_migrated = !prefs.local_thinking_default_migrated;
        prefs.migrate_legacy_local_thinking_default();
        prefs.migrate_models();
        prefs.normalize_saved_model_metadata();
        // deferred_search_deletes is intentionally ignored here: a Delete
        // action never reaches disk through the app's own flows
        // (mark_missing clears credential_action before anything persists),
        // so a deferral on load can only come from a hand-edited settings
        // file, where not deleting is the benign orphan direction.
        let migration = prefs.migrate_plaintext_api_keys_with_store(&SystemCredentialStore::new());
        let memory_policy_changed = prefs.enforce_memory_locale_policy();
        let normalization_changed = minimax_endpoint_changed
            || local_model_alias_changed
            || local_output_sentinel_changed
            || local_thinking_default_migrated
            || migration.settings_sanitized
            || memory_policy_changed
            || color_scheme_derived;
        if should_persist_normalization(
            allow_normalization_persist,
            persist_normalized,
            normalization_changed,
        ) {
            // Round-42 review (extends round-41 M1 to the read lane): the
            // search migration arm sets `settings_sanitized` before the
            // keychain move and never rolls it back, so a broken credential
            // store still reports a normalization change here. Persisting
            // would rewrite settings.json through `save_unlocked`'s
            // unconditional sanitize and erase the plaintext key from its
            // only durable location — the exact hazard
            // [`UserPrefs::update_transaction`] refuses at its gate, reached
            // through every persisting read (`UserPrefs::load()`). SKIP the
            // persist instead: it is idempotent and re-runs once the
            // credential store works again.
            if !migration.failed_model_ids.is_empty()
                || !migration.failed_search_providers.is_empty()
            {
                eprintln!(
                    "[pinvou3-app] settings normalization save skipped: the credential store \
                     rejected {} model and {} search credential migration(s); settings.json is \
                     left untouched so the legacy plaintext key survives (repair the credential \
                     store or select the file backend, then rerun)",
                    migration.failed_model_ids.len(),
                    migration.failed_search_providers.len()
                );
            } else {
                // Round-41 review: this persist writes the WHOLE document from
                // this load's snapshot, so without cross-process exclusion it is
                // exactly the second-writer lost-update race
                // [`UserPrefs::update_transaction`] closes — and the window is
                // not sub-millisecond, because the migration arm above can touch
                // the OS keychain (`migrate_plaintext_api_keys_with_store`)
                // between the read and this save. Take the same
                // settings.json.lock, bounded: on a held lock SKIP the persist
                // instead of writing unlocked. Round-43 review: the lock alone
                // only excludes writers from here on — a field commit that
                // landed ENTIRELY inside the read→lock window is already on
                // disk, and writing the stale snapshot would revert it. So
                // re-read under the lock and persist only when the disk still
                // holds the bytes this load started from; otherwise skip (the
                // normalization is idempotent and re-runs on the next load,
                // while a reverted field commit is not recoverable).
                match try_lock_settings_file_bounded() {
                    Ok(Some(_file_lock)) => {
                        if disk_still_holds_the_loaded_snapshot(&path, &raw) {
                            if let Err(e) = prefs.save_unlocked() {
                                eprintln!(
                                    "[pinvou3-app] settings normalization save failed: {e:#}"
                                );
                            }
                        } else {
                            eprintln!(
                                "[pinvou3-app] settings normalization save skipped: settings.json \
                                 changed while the migration ran (a concurrent writer committed); \
                                 the normalization re-runs on the next load"
                            );
                        }
                    }
                    Ok(None) => eprintln!(
                        "[pinvou3-app] settings normalization save skipped: the settings lock stayed \
                         held for over {}s (a concurrent writer is wedged?); the migration re-runs \
                         on the next load",
                        NORMALIZATION_PERSIST_LOCK_WAIT.as_secs()
                    ),
                    Err(e) => eprintln!("[pinvou3-app] settings normalization save skipped: {e:#}"),
                }
            }
        }
        prefs.sanitize_plaintext_api_keys();
        (parsed.prefs, migration)
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
        // UserPrefs is plain data (no map keys or custom Serialize failure
        // paths), so serialization should never fail; still fall back to
        // error propagation instead of panicking.
        let s = serde_json::to_string_pretty(&normalized).map_err(std::io::Error::other)?;
        // 原子写：直接 std::fs::write 在进程中断时可能留下截断文件，而 load 对
        // 损坏的 settings.json 是回退默认值——code_permission.last_mode 等持久化
        // 偏好会整体丢失，表现为「重启后设置回到默认」。tmp + rename 保证目标
        // 文件永远完整。
        crate::platform::filesystem::atomic_write(&path, s.as_bytes())
    }

    /// 在同一临界区内读取磁盘最新偏好、修改指定字段并写回。
    ///
    /// 闭包必须只修改自己负责的设置域，避免把调用方持有的整份旧快照写回。
    ///
    /// 读-改-写跨进程串行化（round-40 review M7）：CLI 是 settings.json 的
    /// 第二个写者进程（`pinvou models add/edit/remove/use-model`、
    /// `settings set`），进程内 `USER_PREFS_LOCK` 够不到另一个进程；没有
    /// OS 级锁时两个进程的整文档读-改-写是静默 last-writer-wins（后写方
    /// 丢掉先写方的整个改动，两侧都 exit 0）。flock 挂在
    /// `settings.json.lock` 上，跨 load→mutate→save→reload 全程持有，
    /// 持锁进程死亡时由内核释放；持有窗口通常是一次磁盘往返，但闭包内的
    /// 钥匙串 I/O 可能把它拉长到秒级，阻塞等待在那种场景下是有感的——
    /// 这是事务语义（不能静默跳过）换来的代价。`load()` 的规范化持久化
    /// 参与该锁但用
    /// `try_lock_settings_file_bounded` 的时间上限变体（round-41 review：该
    /// 持久化写的是整文档旧快照，且迁移臂会做钥匙串 I/O，读→写窗口不是亚毫秒
    /// ——无锁时并发 `update_transaction` 的字段提交会被静默回退；锁被占住时
    /// 跳过持久化，幂等迁移下次 load 重跑）。一次性 `save()` 仍不参与该锁：
    /// 它本就是整文档覆盖语义。
    pub fn update_transaction<F>(mutate: F) -> Result<Self, String>
    where
        F: FnOnce(&mut Self) -> Result<(), String>,
    {
        Self::update_transaction_with_post_commit(mutate, || ())
    }

    /// Same read-modify-write critical section as [`Self::update_transaction`],
    /// plus a `post_commit` hook that runs right after `save_unlocked()`
    /// succeeded and BEFORE the prefs lock is released.
    ///
    /// The hook exists for best-effort keyring cleanup (the deferred
    /// credential deletes captured by `migrate_plaintext_api_keys_with_store`):
    /// credential replacement writes run `store.set` inside transaction
    /// closures, under this same `USER_PREFS_LOCK`, so the deferred delete
    /// must execute before the lock is released too. Running it after release
    /// would reopen the window where a concurrent replacement commits a fresh
    /// secret for the same deterministic reference between this save's commit
    /// and this cleanup — the stale delete would then destroy the
    /// replacement's secret, and both saves would report success while the
    /// persisted prefs describe a configured provider whose secret is gone.
    /// Under the lock the two saves are totally ordered: the replacement
    /// either lands entirely before this transaction (its keyring write
    /// happens first, and this command's Delete edit is the later user
    /// intent) or blocks until after the delete finished.
    ///
    /// Hook failures must stay non-fatal: the disk commit has already
    /// succeeded, and a failed delete merely orphans the credential (the
    /// reference is deterministic per provider, so a re-set overwrites it).
    pub fn update_transaction_with_post_commit<F, C>(
        mutate: F,
        post_commit: C,
    ) -> Result<Self, String>
    where
        F: FnOnce(&mut Self) -> Result<(), String>,
        C: FnOnce(),
    {
        let _guard = lock_user_prefs();
        let _file_lock = lock_settings_file()
            .map_err(|error| format!("acquire settings lock failed: {error}"))?;
        let (mut prefs, migration) = Self::load_unlocked_with_migration(false);
        // round-41 review M1：迁移失败时明文 key 只存在于内存，而 save_unlocked
        // 无条件 sanitize 后整文档落盘——凭据会从它唯一的持久位置被抹掉，写入方
        // 还报成功。与 GUI `prepare_prefs_for_save` 同一闸门：拒绝整个事务，
        // settings.json 保持原样，等用户修复凭据存储（或切 file 后端）再写。
        if !migration.failed_model_ids.is_empty() || !migration.failed_search_providers.is_empty() {
            return Err("credential store unavailable; please reconfigure API Key".to_string());
        }
        mutate(&mut prefs)?;
        prefs
            .save_unlocked()
            .map_err(|error| format!("save settings failed: {error:#}"))?;
        post_commit();
        // 返回再次从磁盘解析的规范化结果，保证桥接层内存状态与实际持久化内容一致。
        Ok(Self::load_unlocked(false))
    }

    /// Test-only probe for the deferred-delete barrier test: reports whether
    /// `USER_PREFS_LOCK` is currently free. Once the gated deferred delete has
    /// started, "free" means it runs outside the transaction's critical
    /// section (the racy ordering this probe exists to catch in red runs) and
    /// "held" means it runs under the lock (serialized with replacements).
    /// Callers hold `ENV_LOCK`, and every prefs-lock-using test takes that
    /// lock first, so no unrelated test can hold `USER_PREFS_LOCK` here.
    #[cfg(test)]
    pub(crate) fn user_prefs_lock_free_for_test() -> bool {
        matches!(USER_PREFS_LOCK.try_lock(), Ok(_))
    }

    pub fn normalize_saved_model_metadata(&mut self) {
        for model in &mut self.advanced.saved_models {
            model.normalize_alias();
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
                model.normalize_alias();
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
            alias: None,
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
        self.advanced.saved_models[0].normalize_alias();
        self.advanced.saved_models[0].normalize_route_limits();
        self.advanced.saved_models[0].normalize_provider_metadata();
        self.advanced.custom_api_key = None;
        if self.advanced.active_model_id.is_none() {
            self.advanced.active_model_id = Some(id);
        }
    }

    pub fn migrate_plaintext_api_keys_with_store(
        &mut self,
        store: &dyn CredentialStore,
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
                    // The keyring delete is DEFERRED, not run here: this method
                    // executes inside `UserPrefs::update_transaction` closures
                    // (via `prepare_prefs_for_save`; it also runs transaction-free
                    // on load, where the caller drops the deferral), so deleting
                    // before the commit would leave the provider
                    // configured-but-secretless when a later step of the same
                    // transaction fails and the save aborts. The caller must
                    // delete the captured reference only after its save
                    // committed — via `update_transaction_with_post_commit`, so
                    // the delete still runs inside the prefs-lock critical
                    // section and cannot interleave after a concurrent
                    // replacement of the same deterministic reference — where a
                    // failure merely orphans the credential (the benign
                    // direction — the reference is deterministic per provider,
                    // so a re-set overwrites the orphan).
                    if let Some(reference) = credential.credential_ref.clone().or_else(|| {
                        provider
                            .supports_api_key()
                            .then(|| provider.credential_reference())
                    }) {
                        result.deferred_search_deletes.push(reference);
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

    pub fn refresh_credential_states_with_store(&mut self, store: &dyn CredentialStore) {
        let env_override = std::env::var(MODEL_API_KEY_ENV_OVERRIDE)
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
                Ok(_) => {
                    // Round-51 review: a miss against an unreachable OS
                    // keyring (the file-fallback active) must not read as
                    // `missing` — the honest state is `unavailable`, the
                    // same doctrine the value lanes' `os_keyring_unreachable`
                    // re-classification applies (a script acting on a false
                    // `missing`/`has_secret: false` might overwrite a key
                    // the store still holds).
                    if store.os_keyring_unreachable(&reference) {
                        model.mark_unavailable();
                    } else {
                        model.mark_missing();
                    }
                }
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
                Ok(_) => {
                    // Round-51 review: same unreachable-keyring honesty as
                    // the models loop above.
                    if store.os_keyring_unreachable(&reference) {
                        credential.mark_unavailable();
                    } else {
                        credential.mark_missing();
                    }
                }
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
        m.normalize_alias();
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
    fn saved_model_alias_is_backward_compatible_and_normalized() {
        let legacy: SavedModel = serde_json::from_value(serde_json::json!({
            "id": "legacy-cloud",
            "name": "Legacy cloud",
            "preset": "deepseek",
            "model": "deepseek-v4-pro",
            "base_url": "https://api.deepseek.com"
        }))
        .expect("deserialize legacy model without alias");
        assert!(legacy.alias.is_none());

        let mut prefs = UserPrefs::default();
        prefs.advanced.saved_models.clear();
        prefs.upsert_model(SavedModel {
            alias: Some("  Daily assistant  ".to_string()),
            ..legacy.clone()
        });
        assert_eq!(
            prefs.model_by_id("legacy-cloud").unwrap().alias.as_deref(),
            Some("Daily assistant")
        );

        prefs.upsert_model(SavedModel {
            alias: Some("   ".to_string()),
            ..legacy
        });
        assert!(prefs.model_by_id("legacy-cloud").unwrap().alias.is_none());
    }

    #[test]
    fn local_model_alias_is_cleared_by_upsert_and_normalization() {
        let local = SavedModel {
            id: "local-model".into(),
            name: "Local model".into(),
            alias: Some("Must not persist".into()),
            preset: ModelPreset::LocalVllm,
            context_window_tokens: None,
            max_output_tokens: None,
            reasoning_effort: None,
            model: "qwen36_35b_256k".into(),
            base_url: "http://127.0.0.1:8000/v1".into(),
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
        };
        let mut prefs = UserPrefs::default();
        prefs.advanced.saved_models.clear();

        prefs.upsert_model(local);
        assert!(prefs.model_by_id("local-model").unwrap().alias.is_none());

        prefs.advanced.saved_models[0].alias = Some("Legacy local alias".into());
        prefs.normalize_saved_model_metadata();
        assert!(prefs.model_by_id("local-model").unwrap().alias.is_none());
        assert!(!serde_json::to_string(&prefs).unwrap().contains("alias"));
    }

    #[test]
    fn load_clears_and_persists_legacy_local_model_alias() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let old_home = std::env::var_os("PINVOU3_HOME");
        let temporary_home = std::env::temp_dir().join(format!(
            "pinvou3-prefs-local-alias-migration-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&temporary_home);
        std::fs::create_dir_all(&temporary_home).expect("create temporary prefs home");
        // SAFETY: holding ENV_LOCK (first line of this test); env writes in the
        // test process are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &temporary_home) };

        // Fixture must pin LocalVllm explicitly: `ModelPreset::default()` is
        // platform-aware (Linux=LocalVllm, macOS/Windows=Deepseek), so relying
        // on `migrate_models()`'s default model makes this test red on
        // non-Linux while passing the ubuntu-only `rust-test` gate.
        let mut prefs = UserPrefs::default();
        prefs.advanced.saved_models.push(SavedModel {
            id: "local-model".into(),
            name: "Local model".into(),
            alias: Some("Legacy local alias".into()),
            preset: ModelPreset::LocalVllm,
            context_window_tokens: None,
            max_output_tokens: None,
            reasoning_effort: None,
            model: "qwen36_35b_256k".into(),
            base_url: "http://127.0.0.1:8000/v1".into(),
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
        prefs.advanced.active_model_id = Some("local-model".into());
        let settings_path = super::super::paths::settings_path();
        std::fs::write(
            &settings_path,
            serde_json::to_string_pretty(&prefs).expect("serialize legacy prefs"),
        )
        .expect("write legacy prefs");

        let loaded = UserPrefs::load();
        assert!(loaded.active_model().unwrap().alias.is_none());
        let persisted = std::fs::read_to_string(&settings_path).expect("read normalized prefs");
        assert!(!persisted.contains("Legacy local alias"));

        let _ = std::fs::remove_dir_all(&temporary_home);
        match old_home {
            // SAFETY: holding ENV_LOCK (first line of this test); restore-side
            // env writes serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: same as above; removal serialized under ENV_LOCK.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
    }

    /// Round-41 review: the load-time normalization persist writes the whole
    /// document from this load's snapshot, so it must not land unlocked while
    /// another process holds settings.json.lock mid-`update_transaction` —
    /// the stale snapshot would revert that writer's committed fields. With
    /// the lock held past the bounded wait, the persist is SKIPPED (the
    /// legacy value survives on disk and the migration re-runs on the next
    /// unlocked load) instead of writing the stale document.
    #[test]
    fn load_skips_the_normalization_persist_while_the_settings_lock_is_held() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let old_home = std::env::var_os("PINVOU3_HOME");
        let temporary_home = std::env::temp_dir().join(format!(
            "pinvou3-prefs-normalization-lock-skip-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&temporary_home);
        std::fs::create_dir_all(&temporary_home).expect("create temporary prefs home");
        // SAFETY: holding ENV_LOCK (first line of this test); env writes in the
        // test process are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &temporary_home) };

        // Same legacy-local-alias fixture as the persisting twin above: load()
        // observes a normalization change and wants to persist it.
        let mut prefs = UserPrefs::default();
        prefs.advanced.saved_models.push(SavedModel {
            id: "local-model".into(),
            name: "Local model".into(),
            alias: Some("Legacy local alias".into()),
            preset: ModelPreset::LocalVllm,
            context_window_tokens: None,
            max_output_tokens: None,
            reasoning_effort: None,
            model: "qwen36_35b_256k".into(),
            base_url: "http://127.0.0.1:8000/v1".into(),
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
        prefs.advanced.active_model_id = Some("local-model".into());
        let settings_path = super::super::paths::settings_path();
        let legacy_bytes = serde_json::to_string_pretty(&prefs).expect("serialize legacy prefs");
        std::fs::write(&settings_path, &legacy_bytes).expect("write legacy prefs");

        // Hold the settings.json.lock the way a concurrent `update_transaction`
        // on the other surface would, for longer than the bounded persist's
        // wait, and keep the guard across the load.
        let lock_path = settings_path.with_file_name("settings.json.lock");
        let held = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .expect("open settings lock file");
        held.lock().expect("hold the settings lock");

        let loaded = UserPrefs::load();
        // The in-memory value is still normalized (the migration is local);
        // what must NOT happen is the whole-document persist of the stale
        // snapshot over the concurrent writer's head.
        assert!(loaded.active_model().unwrap().alias.is_none());
        let persisted = std::fs::read_to_string(&settings_path).expect("read legacy prefs");
        assert_eq!(
            persisted, legacy_bytes,
            "the normalization persist must be skipped while the settings lock is held"
        );

        drop(held);
        let _ = std::fs::remove_dir_all(&temporary_home);
        match old_home {
            // SAFETY: holding ENV_LOCK (first line of this test); restore-side
            // env writes serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: same as above; restore-side env writes serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
    }

    /// Round-43 review: the normalization persist re-validates under the
    /// settings lock — a field commit that landed entirely inside this
    /// load's read→lock window must be detected (skip), and an unchanged
    /// disk must stay writable. Without the re-validation the whole-document
    /// stale-snapshot write silently reverts the committed fields.
    #[test]
    fn disk_still_holds_the_loaded_snapshot_detects_a_concurrent_commit() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-prefs-snapshot-revalidate-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create temporary prefs root");
        let path = root.join("settings.json");
        let loaded = Some("{\"theme\":\"dark\"}".to_string());

        std::fs::write(&path, loaded.as_ref().unwrap()).expect("write loaded snapshot");
        assert!(disk_still_holds_the_loaded_snapshot(&path, &loaded));

        // A concurrent writer committed different bytes inside the window.
        std::fs::write(&path, "{\"theme\":\"liquid-dark\"}").expect("write committed field");
        assert!(
            !disk_still_holds_the_loaded_snapshot(&path, &loaded),
            "a committed field change must make the stale-snapshot persist skip"
        );

        // The file vanishing since load counts as changed too.
        std::fs::remove_file(&path).expect("remove settings file");
        assert!(!disk_still_holds_the_loaded_snapshot(&path, &loaded));

        // A load that saw no file at all may still write when none appeared.
        assert!(disk_still_holds_the_loaded_snapshot(&path, &None));
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Round-44 review: pin the re-validation at its call site. A field
    /// commit that lands entirely inside this load's read→lock window (the
    /// lock is already held when the load's initial read happens, the commit
    /// lands while the load waits, then the lock frees) must be detected by
    /// the under-lock re-read and skipped — deleting the
    /// `disk_still_holds_the_loaded_snapshot` gate at the persist call site
    /// would silently revert the committed field with the stale snapshot.
    #[test]
    fn normalization_persist_skips_when_a_commit_lands_inside_the_read_to_lock_window() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let old_home = std::env::var_os("PINVOU3_HOME");
        let temporary_home = std::env::temp_dir().join(format!(
            "pinvou3-prefs-window-commit-skip-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&temporary_home);
        std::fs::create_dir_all(&temporary_home).expect("create temporary prefs home");
        // SAFETY: holding ENV_LOCK (first line of this test); env writes in the
        // test process are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &temporary_home) };

        // Same legacy-local-alias fixture as the persisting twin: load()
        // observes a normalization change and wants to persist it.
        let mut prefs = UserPrefs::default();
        prefs.advanced.saved_models.push(SavedModel {
            id: "local-model".into(),
            name: "Local model".into(),
            alias: Some("Legacy local alias".into()),
            preset: ModelPreset::LocalVllm,
            context_window_tokens: None,
            max_output_tokens: None,
            reasoning_effort: None,
            model: "qwen36_35b_256k".into(),
            base_url: "http://127.0.0.1:8000/v1".into(),
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
        prefs.advanced.active_model_id = Some("local-model".into());
        let settings_path = super::super::paths::settings_path();
        let legacy_bytes = serde_json::to_string_pretty(&prefs).expect("serialize legacy prefs");
        std::fs::write(&settings_path, &legacy_bytes).expect("write legacy prefs");

        // The concurrent writer takes the lock BEFORE the load starts,
        // commits a renamed model while the load blocks on the lock, then
        // releases. The load's initial read necessarily happened before the
        // commit; only the under-lock re-read can tell the two snapshots
        // apart and skip the stale-snapshot persist.
        let committed_bytes = {
            let mut value: serde_json::Value =
                serde_json::from_str(&legacy_bytes).expect("parse legacy prefs");
            value["advanced"]["saved_models"][0]["name"] =
                serde_json::Value::String("Renamed by the concurrent writer".into());
            serde_json::to_string_pretty(&value).expect("serialize committed prefs")
        };
        let lock_path = settings_path.with_file_name("settings.json.lock");
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let writer = {
            let settings_path = settings_path.clone();
            let committed_bytes = committed_bytes.clone();
            std::thread::spawn(move || {
                let held = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(&lock_path)
                    .expect("open settings lock file");
                held.lock().expect("hold the settings lock");
                ready_tx.send(()).expect("signal the lock is held");
                std::thread::sleep(std::time::Duration::from_millis(300));
                std::fs::write(&settings_path, &committed_bytes).expect("commit the field");
                drop(held);
            })
        };
        ready_rx.recv().expect("the writer holds the lock");
        UserPrefs::load();
        writer.join().expect("the writer thread finishes");

        let persisted = std::fs::read_to_string(&settings_path).expect("read settings");
        assert_eq!(
            persisted, committed_bytes,
            "the commit that landed inside the read→lock window must survive the \
             normalization persist (the re-validation must skip on drift)"
        );

        let _ = std::fs::remove_dir_all(&temporary_home);
        match old_home {
            // SAFETY: holding ENV_LOCK (first line of this test); restore-side
            // env writes serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: same as above; restore-side env writes serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
    }

    /// Round-42 review (extends round-41 M1 to the read lane): the
    /// load-time normalization persist must not land when the credential
    /// migration failed. The search arm sets `settings_sanitized` before the
    /// keychain move, so a broken store still reports a change; persisting
    /// would run `save_unlocked`'s unconditional sanitize and erase the
    /// plaintext key from its only durable location — through a pure
    /// `UserPrefs::load()` (every `models list/show/test`, `settings get`
    /// read). The persist is skipped instead and re-runs once the store
    /// works; settings.json keeps both the legacy plaintext key and the
    /// unrelated pending normalization.
    #[test]
    fn load_skips_the_normalization_persist_when_credential_migration_failed() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let old_home = std::env::var_os("PINVOU3_HOME");
        struct RestoreEnv(Vec<(&'static str, Option<std::ffi::OsString>)>);
        impl Drop for RestoreEnv {
            fn drop(&mut self) {
                // SAFETY: holding ENV_LOCK (acquired on this test's first
                // line); restore-side env writes serialized.
                for (name, value) in self.0.drain(..) {
                    match value {
                        Some(v) => unsafe { std::env::set_var(name, v) },
                        None => unsafe { std::env::remove_var(name) },
                    }
                }
            }
        }
        let _restore = RestoreEnv(vec![
            (
                "CODEWHALE_SECRET_BACKEND",
                std::env::var_os("CODEWHALE_SECRET_BACKEND"),
            ),
            ("CODEWHALE_HOME", std::env::var_os("CODEWHALE_HOME")),
            ("HOME", std::env::var_os("HOME")),
            ("DEEPSEEK_API_KEY", std::env::var_os("DEEPSEEK_API_KEY")),
        ]);
        let temporary_home = std::env::temp_dir().join(format!(
            "pinvou3-prefs-migration-fail-skip-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&temporary_home);
        std::fs::create_dir_all(&temporary_home).expect("create temporary prefs home");
        // SAFETY: holding ENV_LOCK (first line of this test); env writes in the
        // test process are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &temporary_home) };
        unsafe { std::env::set_var("CODEWHALE_SECRET_BACKEND", "file") };
        // A relative credential home makes the facade's file store refuse
        // writes (same degradation the CLI write-lane contract test uses).
        unsafe { std::env::set_var("CODEWHALE_HOME", "relative-broken-home") };
        unsafe { std::env::set_var("HOME", "relative-broken-home") };
        unsafe { std::env::remove_var("DEEPSEEK_API_KEY") };

        // Same legacy-local-alias fixture as the persisting twins (a real
        // normalization change independent of the keychain) plus a legacy
        // plaintext model key with no stored reference: the migration arm
        // attempts the keychain move and fails.
        let mut prefs = UserPrefs::default();
        prefs.advanced.saved_models.push(SavedModel {
            id: "local-model".into(),
            name: "Local model".into(),
            alias: Some("Legacy local alias".into()),
            preset: ModelPreset::LocalVllm,
            context_window_tokens: None,
            max_output_tokens: None,
            reasoning_effort: None,
            model: "qwen36_35b_256k".into(),
            base_url: "http://127.0.0.1:8000/v1".into(),
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
        prefs.advanced.active_model_id = Some("local-model".into());
        let settings_path = super::super::paths::settings_path();
        let legacy_bytes = serde_json::to_string_pretty(&prefs).expect("serialize legacy prefs");
        // `SavedModel::api_key` is `skip_serializing`, so the legacy plaintext
        // key must be injected as a value-level edit of the known-good
        // document (same fixture technique as the CLI write-lane contract
        // test) — that IS the legacy on-disk shape this migration scrubs.
        let mut document: serde_json::Value =
            serde_json::from_str(&legacy_bytes).expect("parse legacy prefs");
        document["advanced"]["saved_models"][0]["api_key"] =
            serde_json::Value::String("sk-legacy-plaintext-r42".to_owned());
        let legacy_bytes =
            serde_json::to_string_pretty(&document).expect("serialize legacy prefs with key");
        std::fs::write(&settings_path, &legacy_bytes).expect("write legacy prefs");

        let loaded = UserPrefs::load();
        // The in-memory record is still marked unavailable (the store said
        // so); what must NOT happen is the whole-document persist that
        // sanitizes the plaintext key out of its only durable copy.
        let persisted = std::fs::read_to_string(&settings_path).expect("read legacy prefs");
        assert!(
            persisted.contains("sk-legacy-plaintext-r42"),
            "a failed credential migration must keep the legacy plaintext key on disk: \
             {persisted}"
        );
        assert!(
            persisted.contains("Legacy local alias"),
            "the whole normalization persist must be skipped on a failed migration: {persisted}"
        );
        assert!(loaded.active_model().unwrap().alias.is_none());

        let _ = std::fs::remove_dir_all(&temporary_home);
        match old_home {
            // SAFETY: holding ENV_LOCK (first line of this test); restore-side
            // env writes serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: same as above; removal serialized under ENV_LOCK.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
    }

    /// LocalVllm no longer forces a 24K output (stays None when not
    /// explicitly configured; declared uniformly by the runtime window
    /// tiers); the operator-owned predicate covers both local and custom
    /// endpoints and excludes the coding_plan official entry.
    #[test]
    fn normalize_keeps_local_output_unset_and_operator_owned_detection() {
        let mut local = SavedModel {
            id: "m1".into(),
            name: "m1".into(),
            alias: None,
            preset: ModelPreset::LocalVllm,
            context_window_tokens: None,
            max_output_tokens: None,
            reasoning_effort: None,
            model: "qwen36_35b_256k".into(),
            base_url: "http://127.0.0.1:8000/v1".into(),
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
        local.normalize_route_limits();
        assert_eq!(
            local.context_window_tokens,
            Some(262_144),
            "the qwen36_35b_256k window fallback is kept"
        );
        assert_eq!(
            local.max_output_tokens, None,
            "local models no longer force a 24K output; unset stays None"
        );
        assert!(local.is_operator_owned_endpoint());

        // A user's explicitly configured output limit (including the
        // positive-value filter) is kept verbatim.
        let mut explicit = local.clone();
        explicit.max_output_tokens = Some(32_768);
        explicit.normalize_route_limits();
        assert_eq!(explicit.max_output_tokens, Some(32_768));

        // A machine-written legacy local 24K (normalize force + settings
        // page prefill) counts as legacy unset and is normalized to None to
        // take the window tiers; 24576 on non-LocalVllm endpoints is
        // explicit input and is kept verbatim.
        let mut legacy = local.clone();
        legacy.max_output_tokens = Some(24_576);
        legacy.normalize_route_limits();
        assert_eq!(
            legacy.max_output_tokens, None,
            "a legacy machine-written local 24K is normalized to unset and takes the window tiers"
        );
        let mut custom_legacy = legacy.clone();
        custom_legacy.preset = ModelPreset::OpenaiCompatible;
        custom_legacy.provider_kind = Some("custom".into());
        custom_legacy.max_output_tokens = Some(24_576);
        custom_legacy.normalize_route_limits();
        assert_eq!(
            custom_legacy.max_output_tokens,
            Some(24_576),
            "24576 on a custom endpoint is explicit input and is not migrated"
        );

        let mut custom = local.clone();
        custom.preset = ModelPreset::OpenaiCompatible;
        custom.provider_kind = Some("custom".into());
        assert!(custom.is_operator_owned_endpoint());

        // A null provider_kind normalizes to custom for the OpenaiResponses
        // preset too (same custom-endpoint class as OpenaiCompatible), so
        // hand-written or migrated rows keep the operator-owned declaration
        // instead of being re-classified as official API (which would drop
        // the window-tier output declaration for aggregator ids).
        let mut responses = local.clone();
        responses.preset = ModelPreset::OpenaiResponses;
        responses.provider_kind = None;
        responses.normalize_provider_metadata();
        assert_eq!(
            responses.provider_kind.as_deref(),
            Some("custom"),
            "openai_responses records normalize to the custom provider kind"
        );
        assert!(responses.is_operator_owned_endpoint());

        // coding_plan is not operator-owned even on the OpenaiCompatible
        // preset.
        let mut plan = custom.clone();
        plan.provider_kind = Some("coding_plan".into());
        assert!(!plan.is_operator_owned_endpoint());

        // Official cloud presets are not operator-owned.
        let mut cloud = local;
        cloud.preset = ModelPreset::Deepseek;
        assert!(!cloud.is_operator_owned_endpoint());
    }

    /// reasoning_effort 归一为底座 `ReasoningEffort::parse_strict` 认识的规范档位：
    /// 非法值置 None（避免被底座静默回退成 Max），合法别名规范化为对应档位
    /// （对齐 `as_setting()`，避免 wire 层 `apply_reasoning_effort` 静默丢弃）。
    #[test]
    fn normalize_reasoning_effort_canonicalizes_aliases_and_rejects_unknown() {
        let base = SavedModel {
            id: "m1".into(),
            name: "m1".into(),
            alias: None,
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
    fn voice_shortcut_enabled_defaults_off_and_round_trips() {
        // Older settings.json lacks the field: the serde container-level
        // default falls back to false (off by default).
        let legacy = UserPrefs::parse_settings(Some(r#"{"theme":"genesis"}"#), Some("zh-CN"));
        assert!(!legacy.voice_shortcut_enabled);
        let enabled = UserPrefs::parse_settings(
            Some(r#"{"theme":"genesis","voice_shortcut_enabled":true}"#),
            Some("zh-CN"),
        );
        assert!(enabled.voice_shortcut_enabled);
        let serialized = serde_json::to_string(&enabled).expect("UserPrefs serialize");
        assert!(serialized.contains("\"voice_shortcut_enabled\":true"));
    }

    #[test]
    fn computer_use_enabled_defaults_off_and_round_trips() {
        // Older settings.json lacks the field: the serde container-level
        // default falls back to false (off by default).
        let legacy = UserPrefs::parse_settings(Some(r#"{"theme":"genesis"}"#), Some("zh-CN"));
        assert!(!legacy.computer_use.enabled);
        let enabled = UserPrefs::parse_settings(
            Some(r#"{"theme":"genesis","computer_use":{"enabled":true}}"#),
            Some("zh-CN"),
        );
        assert!(enabled.computer_use.enabled);
        let serialized = serde_json::to_string(&enabled).expect("UserPrefs serialize");
        assert!(serialized.contains("\"computer_use\":{\"enabled\":true}"));
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
            alias: None,
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
            alias: None,
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
            alias: None,
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
        // SAFETY: holding the crate-level ENV_LOCK (acquired on this test's first line); env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let mut prefs = UserPrefs::default();
        prefs.advanced.saved_models.push(SavedModel {
            id: "minimax-api".into(),
            name: "MiniMax".into(),
            alias: None,
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
            // SAFETY: holding the crate-level ENV_LOCK (acquired on this test's first line); env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: same as above; restore-side removal serialized under ENV_LOCK.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
    }

    /// The local 24576 legacy-sentinel migration must be persisted at load
    /// time (round-2 review MAJOR: it once only changed memory, the save
    /// gate saw no change, and the machine-written 24K stayed in
    /// settings.json forever). Also pins: 24576 on non-LocalVllm endpoints
    /// is explicit input the migration must not touch.
    #[test]
    fn load_persists_legacy_local_output_sentinel_migration() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // RAII cleanup: PINVOU3_HOME and the temporary home must be reclaimed
        // even on assertion/panic — the previous restore was only written at
        // the normal end, so a mid-test failure leaked the env + directory
        // and polluted later tests in the same process (same recipe as the
        // bridge tests' TempDirGuard; the directory name stacks pid + an
        // in-process atomic suffix so concurrent cargo test from two
        // terminals does not collide).
        struct PrefsHomeGuard {
            previous: Option<std::ffi::OsString>,
            home: std::path::PathBuf,
        }
        impl PrefsHomeGuard {
            fn set(home: std::path::PathBuf) -> Self {
                let previous = std::env::var_os("PINVOU3_HOME");
                // SAFETY: holding the crate-level ENV_LOCK (acquired on this test's first line); env writes are serialized.
                unsafe { std::env::set_var("PINVOU3_HOME", &home) };
                Self { previous, home }
            }
        }
        impl Drop for PrefsHomeGuard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.home);
                match self.previous.take() {
                    // SAFETY: holding ENV_LOCK (first line of this test); restore-side writes serialized.
                    Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
                    // SAFETY: same as above; restore-side removal serialized under ENV_LOCK.
                    None => unsafe { std::env::remove_var("PINVOU3_HOME") },
                }
            }
        }
        let home = std::env::temp_dir().join(format!(
            "pinvou3-prefs-local-output-sentinel-{}-{}",
            std::process::id(),
            super::super::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("create temporary prefs home");
        let _prefs_home = PrefsHomeGuard::set(home);

        let mut prefs = UserPrefs::default();
        prefs.advanced.saved_models.push(SavedModel {
            id: "legacy-local".into(),
            name: "Legacy Local".into(),
            alias: None,
            preset: ModelPreset::LocalVllm,
            context_window_tokens: Some(262_144),
            // The machine-written 24K from older normalize + settings page
            // prefill: must be migrated away and persisted.
            max_output_tokens: Some(24_576),
            reasoning_effort: None,
            model: "qwen36_35b_256k".into(),
            base_url: "http://127.0.0.1:8000/v1".into(),
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
        prefs.advanced.saved_models.push(SavedModel {
            id: "custom-explicit".into(),
            name: "Custom Explicit".into(),
            alias: None,
            preset: ModelPreset::OpenaiCompatible,
            context_window_tokens: None,
            // 24576 on a custom endpoint is explicit input: must be kept
            // verbatim.
            max_output_tokens: Some(24_576),
            reasoning_effort: None,
            model: "custom-model".into(),
            base_url: "https://gateway.example.com/v1".into(),
            provider_kind: Some(MODEL_PROVIDER_KIND_CUSTOM.into()),
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
        prefs.advanced.active_model_id = Some("legacy-local".into());
        let path = super::super::paths::settings_path();
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&prefs).expect("serialize legacy prefs"),
        )
        .expect("write legacy prefs");

        let loaded = UserPrefs::load();
        let legacy_local = loaded
            .model_by_id("legacy-local")
            .expect("legacy local model");
        assert_eq!(
            legacy_local.max_output_tokens, None,
            "the legacy in-memory 24K must already be normalized to unset"
        );
        let custom = loaded.model_by_id("custom-explicit").expect("custom model");
        assert_eq!(
            custom.max_output_tokens,
            Some(24_576),
            "the explicit 24576 on the custom endpoint must not be migrated"
        );

        // Key assertion: the normalization was written back to disk (not
        // just changed in memory).
        let persisted: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read migrated prefs"))
                .expect("parse migrated prefs");
        let models = persisted["advanced"]["saved_models"]
            .as_array()
            .expect("saved_models array");
        let legacy_on_disk = models
            .iter()
            .find(|model| model["id"] == "legacy-local")
            .expect("legacy local on disk");
        assert!(
            legacy_on_disk["max_output_tokens"].is_null(),
            "the legacy on-disk 24K must be cleared, got {}",
            legacy_on_disk["max_output_tokens"]
        );
        let custom_on_disk = models
            .iter()
            .find(|model| model["id"] == "custom-explicit")
            .expect("custom on disk");
        assert_eq!(custom_on_disk["max_output_tokens"], 24_576);
        // PINVOU3_HOME restoration and temporary home reclamation are
        // handled by PrefsHomeGuard's Drop (also on assertion/panic paths);
        // no hand-written restore block here.
    }

    /// SavedModel fixture with only the fields the legacy-thinking-off
    /// migration reads; everything else at neutral defaults.
    fn legacy_model_fixture(
        id: &str,
        preset: ModelPreset,
        base_url: &str,
        vendor: Option<&str>,
        reasoning_effort: Option<&str>,
    ) -> SavedModel {
        SavedModel {
            id: id.into(),
            name: id.into(),
            alias: None,
            preset,
            context_window_tokens: None,
            max_output_tokens: None,
            reasoning_effort: reasoning_effort.map(str::to_string),
            model: "qwen3-32b".into(),
            base_url: base_url.into(),
            provider_kind: Some(
                match preset {
                    ModelPreset::OpenaiCompatible => MODEL_PROVIDER_KIND_CUSTOM,
                    _ => MODEL_PROVIDER_KIND_OFFICIAL_API,
                }
                .into(),
            ),
            vendor: vendor.map(str::to_string),
            endpoint_mode: None,
            image_capability_override: ImageCapabilityOverride::default(),
            vision_model_id: None,
            api_key: String::new(),
            credential_ref: None,
            credential_state: CredentialState::Missing,
            has_secret: false,
            credential_action: None,
        }
    }

    /// Pins each arm of the migration predicate individually, so a one-line
    /// relaxation cannot slip past the end-to-end load test (whose fixtures
    /// only exercise the arms on records the old UI could produce):
    /// LocalVllm strips regardless of base URL — yet the official-DeepSeek
    /// keep wins first — and the OpenaiCompatible strip requires a
    /// non-coding-plan kind, an empty vendor, and a local route.
    #[test]
    fn legacy_machine_written_local_thinking_off_predicate_arms() {
        let strips = |preset, base_url: &str, vendor: Option<&str>| {
            legacy_model_fixture("m", preset, base_url, vendor, Some("off"))
                .legacy_machine_written_local_thinking_off()
        };
        // LocalVllm: machine-written for every record, public URL included…
        assert!(strips(
            ModelPreset::LocalVllm,
            "http://127.0.0.1:8000/v1",
            None
        ));
        assert!(strips(
            ModelPreset::LocalVllm,
            "https://relay.example.com/v1",
            None
        ));
        // …but the official DeepSeek URL keep wins even for LocalVllm.
        assert!(!strips(
            ModelPreset::LocalVllm,
            "https://api.deepseek.com",
            None
        ));
        // OpenaiCompatible: local route with an empty vendor strips…
        assert!(strips(
            ModelPreset::OpenaiCompatible,
            "http://192.168.1.20:11434/v1",
            None
        ));
        // …a known vendor keeps even on a local route…
        assert!(!strips(
            ModelPreset::OpenaiCompatible,
            "http://192.168.1.20:11434/v1",
            Some("glm")
        ));
        // …a coding-plan kind keeps even on a local route…
        let mut coding_plan = legacy_model_fixture(
            "plan",
            ModelPreset::OpenaiCompatible,
            "http://127.0.0.1:8317/v1",
            None,
            Some("off"),
        );
        coding_plan.provider_kind = Some(MODEL_PROVIDER_KIND_CODING_PLAN.into());
        assert!(!coding_plan.legacy_machine_written_local_thinking_off());
        // …and only the exact machine-written value strips.
        let low = legacy_model_fixture(
            "m",
            ModelPreset::OpenaiCompatible,
            "http://192.168.1.20:11434/v1",
            None,
            Some("low"),
        );
        assert!(!low.legacy_machine_written_local_thinking_off());
    }

    /// #622: the settings page machine-wrote the local default `off` into
    /// every form-saved local record. The first post-#622 load must strip
    /// those (in memory AND on disk) while keeping every `off` the old form
    /// could not have written (public custom endpoints, vendor-tagged cloud
    /// routes, official cloud presets).
    #[test]
    fn load_migrates_legacy_local_thinking_off() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        struct PrefsHomeGuard {
            previous: Option<std::ffi::OsString>,
            home: std::path::PathBuf,
        }
        impl PrefsHomeGuard {
            fn set(home: std::path::PathBuf) -> Self {
                let previous = std::env::var_os("PINVOU3_HOME");
                // SAFETY: holding ENV_LOCK (first line of this test); env writes serialized.
                unsafe { std::env::set_var("PINVOU3_HOME", &home) };
                Self { previous, home }
            }
        }
        impl Drop for PrefsHomeGuard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.home);
                match self.previous.take() {
                    // SAFETY: holding ENV_LOCK (first line of this test); restore-side writes serialized.
                    Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
                    // SAFETY: same as above; restore-side removal serialized under ENV_LOCK.
                    None => unsafe { std::env::remove_var("PINVOU3_HOME") },
                }
            }
        }
        let home = std::env::temp_dir().join(format!(
            "pinvou3-prefs-legacy-thinking-off-{}-{}",
            std::process::id(),
            super::super::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("create temporary prefs home");
        let _prefs_home = PrefsHomeGuard::set(home);

        let mut prefs = UserPrefs::default();
        // Machine-written population: must be stripped.
        prefs.advanced.saved_models.push(legacy_model_fixture(
            "local-vllm",
            ModelPreset::LocalVllm,
            "http://127.0.0.1:8000/v1",
            None,
            Some("off"),
        ));
        prefs.advanced.saved_models.push(legacy_model_fixture(
            "lan-endpoint",
            ModelPreset::OpenaiCompatible,
            "http://192.168.1.20:11434/v1",
            None,
            Some("off"),
        ));
        prefs.advanced.saved_models.push(legacy_model_fixture(
            "docker-local",
            ModelPreset::OpenaiCompatible,
            "http://host.docker.internal:8080/v1",
            None,
            Some("off"),
        ));
        // Explicit populations: must be kept verbatim.
        prefs.advanced.saved_models.push(legacy_model_fixture(
            "public-custom",
            ModelPreset::OpenaiCompatible,
            "https://gateway.example.com/v1",
            None,
            Some("off"),
        ));
        prefs.advanced.saved_models.push(legacy_model_fixture(
            "vendor-glm",
            ModelPreset::OpenaiCompatible,
            "https://api.z.ai/api/paas/v4",
            Some("glm"),
            Some("off"),
        ));
        prefs.advanced.saved_models.push(legacy_model_fixture(
            "cloud-deepseek",
            ModelPreset::Deepseek,
            "https://api.deepseek.com",
            None,
            Some("off"),
        ));
        prefs.advanced.active_model_id = Some("local-vllm".into());
        let path = super::super::paths::settings_path();
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&prefs).expect("serialize legacy prefs"),
        )
        .expect("write legacy prefs");

        let loaded = UserPrefs::load();
        let effort_of = |id: &str| {
            loaded
                .model_by_id(id)
                .unwrap_or_else(|| panic!("model {id}"))
                .reasoning_effort
                .clone()
        };
        assert_eq!(effort_of("local-vllm"), None, "machine-written local off");
        assert_eq!(effort_of("lan-endpoint"), None, "LAN local off");
        assert_eq!(effort_of("docker-local"), None, "docker-alias local off");
        assert_eq!(
            effort_of("public-custom"),
            Some("off".into()),
            "public custom endpoint could not receive tiers pre-#622: off is explicit"
        );
        assert_eq!(
            effort_of("vendor-glm"),
            Some("off".into()),
            "vendor routes resolved to cloud providers with default high: off is explicit"
        );
        assert_eq!(
            effort_of("cloud-deepseek"),
            Some("off".into()),
            "official cloud presets always defaulted high: off is explicit"
        );

        // Key assertion: the migration (stripped values AND the marker) was
        // written back to disk, not just changed in memory.
        let persisted: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read migrated prefs"))
                .expect("parse migrated prefs");
        assert_eq!(
            persisted["local_thinking_default_migrated"], true,
            "the marker must land on disk at the migrating load"
        );
        let models = persisted["advanced"]["saved_models"]
            .as_array()
            .expect("saved_models array");
        let local_on_disk = models
            .iter()
            .find(|model| model["id"] == "local-vllm")
            .expect("local model on disk");
        assert!(
            local_on_disk["reasoning_effort"].is_null(),
            "machine-written local off must be cleared on disk"
        );
        let vendor_on_disk = models
            .iter()
            .find(|model| model["id"] == "vendor-glm")
            .expect("vendor model on disk");
        assert_eq!(vendor_on_disk["reasoning_effort"], "off");
    }

    /// Once the marker is set, a stored local `off` is an explicit
    /// post-upgrade choice: later loads must never strip it again.
    #[test]
    fn load_marker_blocks_second_thinking_off_migration() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        struct PrefsHomeGuard {
            previous: Option<std::ffi::OsString>,
            home: std::path::PathBuf,
        }
        impl PrefsHomeGuard {
            fn set(home: std::path::PathBuf) -> Self {
                let previous = std::env::var_os("PINVOU3_HOME");
                // SAFETY: holding ENV_LOCK (first line of this test); env writes serialized.
                unsafe { std::env::set_var("PINVOU3_HOME", &home) };
                Self { previous, home }
            }
        }
        impl Drop for PrefsHomeGuard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.home);
                match self.previous.take() {
                    // SAFETY: holding ENV_LOCK (first line of this test); restore-side writes serialized.
                    Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
                    // SAFETY: same as above; restore-side removal serialized under ENV_LOCK.
                    None => unsafe { std::env::remove_var("PINVOU3_HOME") },
                }
            }
        }
        let home = std::env::temp_dir().join(format!(
            "pinvou3-prefs-thinking-off-marker-{}-{}",
            std::process::id(),
            super::super::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("create temporary prefs home");
        let _prefs_home = PrefsHomeGuard::set(home);

        let mut prefs = UserPrefs::default();
        prefs.local_thinking_default_migrated = true;
        prefs.advanced.saved_models.push(legacy_model_fixture(
            "explicit-off",
            ModelPreset::LocalVllm,
            "http://127.0.0.1:8000/v1",
            None,
            Some("off"),
        ));
        let path = super::super::paths::settings_path();
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&prefs).expect("serialize marked prefs"),
        )
        .expect("write marked prefs");

        let loaded = UserPrefs::load();
        assert_eq!(
            loaded
                .model_by_id("explicit-off")
                .expect("explicit off model")
                .reasoning_effort,
            Some("off".into()),
            "an off saved after the migrating load is an explicit choice"
        );
    }

    /// A settings file with no local models at all still gets its marker
    /// persisted at the first post-#622 load — otherwise a local model
    /// added (and explicitly set to off) after the upgrade would be
    /// stripped by the next load.
    #[test]
    fn load_persists_thinking_marker_without_local_models() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        struct PrefsHomeGuard {
            previous: Option<std::ffi::OsString>,
            home: std::path::PathBuf,
        }
        impl PrefsHomeGuard {
            fn set(home: std::path::PathBuf) -> Self {
                let previous = std::env::var_os("PINVOU3_HOME");
                // SAFETY: holding ENV_LOCK (first line of this test); env writes serialized.
                unsafe { std::env::set_var("PINVOU3_HOME", &home) };
                Self { previous, home }
            }
        }
        impl Drop for PrefsHomeGuard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.home);
                match self.previous.take() {
                    // SAFETY: holding ENV_LOCK (first line of this test); restore-side writes serialized.
                    Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
                    // SAFETY: same as above; restore-side removal serialized under ENV_LOCK.
                    None => unsafe { std::env::remove_var("PINVOU3_HOME") },
                }
            }
        }
        let home = std::env::temp_dir().join(format!(
            "pinvou3-prefs-thinking-marker-cloud-only-{}-{}",
            std::process::id(),
            super::super::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("create temporary prefs home");
        let _prefs_home = PrefsHomeGuard::set(home);

        let mut prefs = UserPrefs::default();
        prefs.advanced.saved_models.push(legacy_model_fixture(
            "cloud-only",
            ModelPreset::Deepseek,
            "https://api.deepseek.com",
            None,
            Some("high"),
        ));
        let path = super::super::paths::settings_path();
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&prefs).expect("serialize prefs"),
        )
        .expect("write prefs");

        let loaded = UserPrefs::load();
        assert!(
            loaded.local_thinking_default_migrated,
            "marker set in memory even with nothing to migrate"
        );
        let persisted: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read prefs"))
                .expect("parse prefs");
        assert_eq!(
            persisted["local_thinking_default_migrated"], true,
            "marker-only change must still be persisted"
        );
    }

    #[test]
    fn remove_active_model_falls_back_to_first() {
        let mut prefs = UserPrefs::default();
        prefs.migrate_models();
        prefs.upsert_model(SavedModel {
            id: "m2".into(),
            name: "Kimi".into(),
            alias: None,
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
        // serde default 保证无感迁移 → Pinvou / None(设计 §6.3,阶段 C)。
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
            ImageCapabilityOverride::Pinvou
        );
        assert!(model.vision_model_id.is_none());

        // 显式值能序列化往返;vision_model_id 为 None 时不写入(保持 settings.json 干净)。
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
    fn saved_model_image_capability_unknown_value_falls_back_to_pinvou() {
        // 未知档位值(未来版本新增枚举后降级运行/手工编辑)必须落 Pinvou,
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
            ImageCapabilityOverride::Pinvou
        );
    }

    #[test]
    fn saved_model_image_capability_legacy_auto_migrates_to_pinvou() {
        // 已下线的「保存时检测」(auto)档:残留 settings.json 按 pinvou 决策迁移。
        let legacy = r#"{
            "id": "m1",
            "name": "DeepSeek 线上",
            "preset": "deepseek",
            "model": "deepseek-v4-pro",
            "base_url": "https://api.deepseek.com",
            "image_capability_override": "auto"
        }"#;
        let model: SavedModel = serde_json::from_str(legacy).expect("legacy auto override");
        assert_eq!(
            model.image_capability_override,
            ImageCapabilityOverride::Pinvou
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
            voice_shortcut_enabled: false,
            computer_use: ComputerUsePrefs::default(),
            local_thinking_default_migrated: false,
            advanced: AdvancedPrefs {
                allow_shell: Some(false),
                max_output_tokens: Some(8192),
                max_subagents: Some(2),
                ..Default::default()
            },
            disabled_builtin_features: Vec::new(),
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
        // Legacy settings.json without the builtin-feature field → defaults
        // to empty (all enabled), docs/builtin-toolset-contract.md §3.3.
        assert!(prefs.disabled_builtin_features.is_empty());
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

    /// Old settings (no color_scheme key) derive the preference from the legacy
    /// effective appearance in `theme`, keeping the upgrade invisible; only a
    /// true fresh install (no settings.json → default branch) stays on System.
    #[test]
    fn legacy_settings_derive_color_scheme_from_theme() {
        for (raw, expected) in [
            (r#"{"theme":"genesis"}"#, ColorScheme::Dark),
            (r#"{"theme":"liquid-dark"}"#, ColorScheme::Dark),
            (r#"{"theme":"liquid-light"}"#, ColorScheme::Light),
        ] {
            let parsed = UserPrefs::parse_settings_with_state(Some(raw), Some("en-US"));
            assert_eq!(parsed.prefs.color_scheme, expected, "raw: {raw}");
            assert!(parsed.color_scheme_derived, "raw: {raw}");
            assert!(parsed.allow_normalization_persist, "raw: {raw}");
        }
    }

    /// An explicitly saved color_scheme (including system = follow the OS) is
    /// never overwritten by the theme derivation.
    #[test]
    fn explicit_color_scheme_is_never_rederived() {
        for (raw, expected) in [
            (
                r#"{"theme":"liquid-light","color_scheme":"system"}"#,
                ColorScheme::System,
            ),
            (
                r#"{"theme":"genesis","color_scheme":"light"}"#,
                ColorScheme::Light,
            ),
            (
                r#"{"theme":"genesis","color_scheme":"dark"}"#,
                ColorScheme::Dark,
            ),
        ] {
            let parsed = UserPrefs::parse_settings_with_state(Some(raw), Some("en-US"));
            assert_eq!(parsed.prefs.color_scheme, expected, "raw: {raw}");
            assert!(!parsed.color_scheme_derived, "raw: {raw}");
        }
    }

    /// Fresh install (no settings.json): color_scheme stays System and no
    /// normalization write-back fires.
    #[test]
    fn fresh_settings_keep_color_scheme_system() {
        let parsed = UserPrefs::parse_settings_with_state(None, Some("en-US"));
        assert_eq!(parsed.prefs.color_scheme, ColorScheme::System);
        assert!(!parsed.color_scheme_derived);
        assert!(!parsed.allow_normalization_persist);
    }

    /// load writes the derived value for old settings back exactly once so the
    /// file becomes self-describing; settings with an explicit color_scheme
    /// are never rewritten.
    #[test]
    fn load_persists_derived_color_scheme_once() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let old_home = std::env::var_os("PINVOU3_HOME");
        let temporary_home = std::env::temp_dir().join(format!(
            "pinvou3-prefs-color-scheme-derive-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&temporary_home);
        std::fs::create_dir_all(&temporary_home).expect("create temporary prefs home");
        // SAFETY: holding the crate-level ENV_LOCK (acquired on this test's first line); env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &temporary_home) };

        let settings_path = super::super::paths::settings_path();
        std::fs::write(&settings_path, r#"{"theme":"liquid-light"}"#).expect("write legacy prefs");

        let loaded = UserPrefs::load();
        assert_eq!(loaded.color_scheme, ColorScheme::Light);
        let persisted = std::fs::read_to_string(&settings_path).expect("read normalized prefs");
        assert!(
            persisted.contains(r#""color_scheme": "light""#),
            "persisted: {persisted}"
        );

        // An explicitly saved system value is not rewritten by load.
        std::fs::write(
            &settings_path,
            r#"{"theme":"genesis","color_scheme":"system"}"#,
        )
        .expect("write explicit prefs");
        let loaded = UserPrefs::load();
        assert_eq!(loaded.color_scheme, ColorScheme::System);
        let persisted = std::fs::read_to_string(&settings_path).expect("read explicit prefs");
        let value: serde_json::Value = serde_json::from_str(&persisted).expect("parse persisted");
        assert_eq!(
            value.get("color_scheme").and_then(|v| v.as_str()),
            Some("system"),
            "persisted: {persisted}"
        );

        let _ = std::fs::remove_dir_all(&temporary_home);
        match old_home {
            // SAFETY: holding the crate-level ENV_LOCK (acquired on this test's first line); env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: same as above; restore-side removal serialized under ENV_LOCK.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
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
        for raw in [None, Some(String::new()), Some("   \n\t ".to_string())] {
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
        // SAFETY: holding the crate-level ENV_LOCK (acquired on this test's first line); env writes are serialized.
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
            // SAFETY: holding the crate-level ENV_LOCK (acquired on this test's first line); env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: same as above; restore-side removal serialized under ENV_LOCK.
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
        // SAFETY: holding the crate-level ENV_LOCK (acquired on this test's first line); env writes are serialized.
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
        assert!(
            saved
                .search
                .enabled_providers
                .contains(&SearchProvider::Tavily)
        );

        let _ = std::fs::remove_dir_all(&tmp);
        match old_home {
            // SAFETY: holding the crate-level ENV_LOCK (acquired on this test's first line); env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: same as above; restore-side removal serialized under ENV_LOCK.
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
    fn migrate_search_delete_action_defers_the_keyring_delete() {
        let store = crate::platform::credential_store::RecordingCredentialStore::new();
        let mut prefs = UserPrefs {
            search: SearchPrefs {
                provider: SearchProvider::Tavily,
                credentials: [(
                    SearchProvider::Tavily,
                    SearchCredential {
                        credential_ref: Some(SearchProvider::Tavily.credential_reference()),
                        credential_state: CredentialState::Configured,
                        has_secret: true,
                        credential_action: Some(CredentialEditAction::Delete),
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            },
            ..Default::default()
        };

        let result = prefs.migrate_plaintext_api_keys_with_store(&store);

        assert_eq!(
            result.deferred_search_deletes,
            vec![SearchProvider::Tavily.credential_reference()],
            "the delete action must defer the provider's credential reference"
        );
        assert_eq!(result.migrated_count, 0);
        assert!(result.settings_sanitized);
        let credential = &prefs.search.credentials[&SearchProvider::Tavily];
        assert_eq!(credential.credential_state, CredentialState::Missing);
        assert!(credential.credential_ref.is_none());
        assert!(
            store.ops().is_empty(),
            "no keyring call may run inside the migration: {:?}",
            store.ops()
        );
    }

    #[test]
    fn migrate_search_delete_action_defers_the_synthesized_reference() {
        let store = crate::platform::credential_store::RecordingCredentialStore::new();
        let mut prefs = UserPrefs {
            search: SearchPrefs {
                provider: SearchProvider::Tavily,
                credentials: [(
                    SearchProvider::Tavily,
                    SearchCredential {
                        credential_state: CredentialState::Configured,
                        has_secret: true,
                        credential_action: Some(CredentialEditAction::Delete),
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            },
            ..Default::default()
        };

        let result = prefs.migrate_plaintext_api_keys_with_store(&store);

        // No stored reference: the Delete arm must fall back to the
        // provider's deterministic reference, so the deferred delete still
        // targets the keyring entry a prior set created.
        assert_eq!(
            result.deferred_search_deletes,
            vec![SearchProvider::Tavily.credential_reference()],
            "the fallback must synthesize the provider's credential reference"
        );
        assert!(result.settings_sanitized);
        let credential = &prefs.search.credentials[&SearchProvider::Tavily];
        assert_eq!(credential.credential_state, CredentialState::Missing);
        assert!(credential.credential_ref.is_none());
        assert!(
            store.ops().is_empty(),
            "no keyring call may run inside the migration: {:?}",
            store.ops()
        );
    }

    #[test]
    fn refresh_skips_credential_entries_without_a_reference() {
        let store = crate::platform::credential_store::RecordingCredentialStore::new();
        let mut prefs = UserPrefs {
            search: SearchPrefs {
                provider: SearchProvider::Tavily,
                credentials: [(
                    SearchProvider::Tavily,
                    SearchCredential {
                        credential_state: CredentialState::Missing,
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            },
            ..Default::default()
        };

        // A ref-less entry is what the deferred Delete arm leaves behind.
        // The refresh must NOT probe the keyring for it: probing by the
        // provider's deterministic reference would mark the entry Configured
        // while its secret is still queued for post-commit deletion — the
        // configured-but-secretless state the deferral exists to prevent.
        prefs.refresh_credential_states_with_store(&store);

        assert!(
            store.ops().is_empty(),
            "the refresh must not touch the keyring for a ref-less entry: {:?}",
            store.ops()
        );
        let credential = &prefs.search.credentials[&SearchProvider::Tavily];
        assert_eq!(credential.credential_state, CredentialState::Missing);
    }

    /// Round-51 review: a `get` miss served by the file fallback (the OS
    /// keyring unreachable) must classify as `Unavailable`, not `Missing` —
    /// a false `missing`/`has_secret: false` is exactly what a script might
    /// act on by overwriting a key the store still holds (the value lanes'
    /// `os_keyring_unreachable` doctrine, now on the state lanes too).
    #[test]
    fn refresh_classifies_a_fallback_miss_as_unavailable_not_missing() {
        struct FallbackStore;
        impl crate::platform::credential_store::CredentialStore for FallbackStore {
            fn get(
                &self,
                _reference: &crate::platform::credential_store::CredentialReference,
            ) -> Result<Option<String>, crate::platform::credential_store::CredentialError>
            {
                Ok(None)
            }
            fn set(
                &self,
                _reference: &crate::platform::credential_store::CredentialReference,
                _value: &str,
            ) -> Result<(), crate::platform::credential_store::CredentialError> {
                unimplemented!("not used by the refresh");
            }
            fn delete(
                &self,
                _reference: &crate::platform::credential_store::CredentialReference,
            ) -> Result<(), crate::platform::credential_store::CredentialError> {
                unimplemented!("not used by the refresh");
            }
            fn os_keyring_unreachable(
                &self,
                _reference: &crate::platform::credential_store::CredentialReference,
            ) -> bool {
                true
            }
        }

        let mut prefs = UserPrefs::default();
        prefs.upsert_model(SavedModel {
            id: "m1".to_string(),
            name: "M1".to_string(),
            alias: None,
            preset: ModelPreset::Deepseek,
            context_window_tokens: None,
            max_output_tokens: None,
            reasoning_effort: None,
            model: "deepseek-chat".to_string(),
            base_url: "https://api.deepseek.com".to_string(),
            provider_kind: None,
            vendor: None,
            endpoint_mode: None,
            image_capability_override: ImageCapabilityOverride::default(),
            vision_model_id: None,
            api_key: String::new(),
            credential_ref: Some(crate::platform::credential_store::CredentialReference {
                service: "pinvou3-test".to_string(),
                account: "m1".to_string(),
                version: 1,
            }),
            credential_state: CredentialState::Missing,
            has_secret: false,
            credential_action: None,
        });
        prefs.refresh_credential_states_with_store(&FallbackStore);
        assert_eq!(
            prefs.advanced.saved_models[0].credential_state,
            CredentialState::Unavailable,
            "an unreachable-keyring miss must read as Unavailable"
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

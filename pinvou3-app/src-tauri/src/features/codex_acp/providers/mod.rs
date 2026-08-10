//! ACP Agent 的第三方 Provider（中转）管理。
//!
//! 移植自 cc-switch 的核心能力：为 Codex / Claude Code / Kimi 三个外部 CLI
//! 维护 Provider 条目（base URL / wire 协议 / API key / 模型），一键切换并改写
//! 各 CLI 自身的配置文件，支持恢复官方登录。API key 不落仓库、不落日志、不落
//! 明文 JSON——只以 `CredentialReference` 存系统凭据库（导入导出文件除外）。

mod claude;
mod codex;
mod kimi;
pub(crate) mod lifecycle;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::platform::credential_store::{
    CredentialEditAction, CredentialReference, CredentialStore, SystemCredentialStore,
};

pub(crate) const PROVIDER_ID_PREFIX: &str = "pv-";
const STORE_VERSION: u32 = 1;
/// Kimi 未指定上下文窗口时的兜底窗口（200k 保守默认），`kimi_runtime_config_ready`
/// 要求该字段 > 0。Provider 显式填了 context_window 时该值优先（见 kimi.rs）。
const KIMI_DEFAULT_CONTEXT_SIZE: i64 = 200_000;
/// Kimi 未指定模型时的兜底模型名（仅影响 models 表字段，不改写用户请求）。
const KIMI_DEFAULT_MODEL: &str = "kimi-k3";

/// 与厂商通信的 wire 协议。写入器按目标 CLI 的配置格式映射
/// （codex: `chat`/`anthropic`，kimi: `kimi`/`openai`/`anthropic`）。
/// `Kimi` 是 Kimi Code 官方文档定义的专用类型（托管服务与 Kimi Platform
/// API key 均使用），支持视频等 Kimi 专属能力，仅适用于 Kimi Agent。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderWireApi {
    #[default]
    Anthropic,
    Openai,
    Kimi,
}

impl ProviderWireApi {
    pub fn parse(value: Option<&str>) -> Result<Self> {
        match value.unwrap_or("anthropic") {
            "anthropic" => Ok(Self::Anthropic),
            "openai" | "openai_compatible" | "chat" => Ok(Self::Openai),
            "kimi" => Ok(Self::Kimi),
            other => anyhow::bail!("不支持的 wire 协议: {other}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderRecord {
    pub id: String,
    pub name: String,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Claude Code 细化模型槽位（opus/sonnet/haiku/fable/subagent → 实际模型名）。
    /// 仅 claude 使用：槽位留空时 CC 的子 agent 会回落官方模型走官方流量，
    /// 因此保存 claude Provider 时五个槽位均为必填。BTreeMap 保证序列化有序。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_slots: Option<std::collections::BTreeMap<String, String>>,
    /// 上下文窗口（可选）：codex 写入模型 catalog 的 context_window、
    /// kimi 写 models.<id>.max_context_size；claude 无对应配置项（用 [1m] 变体）。
    /// 未填时 writer 用各自默认值（200_000）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i64>,
    pub wire_api: ProviderWireApi,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<CredentialReference>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created_at: String,
}

/// 写入 CLI 配置文件所需的完整切换目标。
#[derive(Debug, Clone)]
pub struct ProviderTarget {
    pub provider_id: String,
    pub name: String,
    pub base_url: String,
    pub model: Option<String>,
    pub model_slots: Option<std::collections::BTreeMap<String, String>>,
    pub context_window: Option<i64>,
    pub wire_api: ProviderWireApi,
    pub api_key: Option<String>,
}

impl ProviderTarget {
    pub fn from_record(record: &ProviderRecord, api_key: Option<String>) -> Self {
        Self {
            provider_id: record.id.clone(),
            name: record.name.clone(),
            base_url: record.base_url.clone(),
            model: record.model.clone(),
            model_slots: record.model_slots.clone(),
            context_window: record.context_window,
            wire_api: record.wire_api,
            api_key,
        }
    }
}

/// 生效中配置的单个条目（展示用）。`secret=true` 表示凭据类变量：**value 恒为
/// 空串、原值不回传**，前端只渲染「已设置」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveEntry {
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub secret: bool,
}

/// 从配置文件读出的当前生效状态。`provider_hint` 是能直接从文件反推的 provider id
/// （claude 的 env 无法反推，为 None）。`entries` 供前端「生效中配置」只读区展示，
/// 值全部来自实际配置文件（F4 可见化）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveConfig {
    pub relay_active: bool,
    pub provider_hint: Option<String>,
    pub entries: Vec<EffectiveEntry>,
}

impl Default for EffectiveConfig {
    fn default() -> Self {
        Self {
            relay_active: false,
            provider_hint: None,
            entries: Vec::new(),
        }
    }
}

/// 统一配置写入契约。根目录由构造参数注入（生产传各 CLI 的 home，单测传临时目录）。
pub trait AgentConfigWriter: Send + Sync {
    fn apply(&self, target: &ProviderTarget) -> Result<()>;
    fn revert_to_official(&self, reverted: Option<&ProviderTarget>) -> Result<()>;
    fn effective(&self) -> Result<EffectiveConfig>;

    /// 切换前 CLI 配置文件的官方 default_model（Kimi 为 "kimi-code/k3" 这类
    /// 官方登录写入的值）；无概念时返回 None（默认实现）。
    fn current_default_model(&self) -> Result<Option<String>> {
        Ok(None)
    }

    /// 恢复官方登录后写回官方 default_model（默认 no-op）。Kimi 在
    /// default_model 缺失或仍指向受管 pv-* 时恢复，避免官方登录态断裂。
    fn restore_default_model(&self, _model: Option<&str>) -> Result<()> {
        Ok(())
    }
}

/// 前端展示的 Provider 条目。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    pub id: String,
    pub name: String,
    pub base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub wire_api: ProviderWireApi,
    pub has_credential: bool,
    /// Claude Code 细化模型槽位（仅 claude 有值），编辑表单回填用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_slots: Option<std::collections::BTreeMap<String, String>>,
    /// 上下文窗口（可选，codex/kimi 有值），编辑表单回填用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub created_at: String,
}

/// `list_acp_providers` 返回值：当前态以 CLI 实际配置文件为准。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpProvidersView {
    pub providers: Vec<ProviderView>,
    /// store 记录的用户选择；null = 官方登录（隐式默认）。
    pub current_provider_id: Option<String>,
    /// 配置文件中无 relay 配置（官方登录生效中）。
    pub official_active: bool,
    /// 配置文件有 relay 配置但无法归因到 store 中当前 provider（外部工具/手动改动）。
    pub external_active: bool,
    /// 已设置的环境变量名（env 优先于配置文件，切换可能不生效）。
    pub env_conflicts: Vec<String>,
    /// CLI 配置文件不可解析（损坏）时置 true；前端需提示用户修复，
    /// 否则「显示官方登录、点切换报拒绝覆盖」的状态不一致会让用户困惑。
    pub config_unreadable: bool,
    /// 从实际配置文件读出的生效值（base_url/model 等，不含凭据）；
    /// 官方登录态或配置文件不可解析时为空。
    pub effective_entries: Vec<EffectiveEntry>,
    /// 已设置的环境变量的生效值：URL/模型名明文，凭据类 secret=true 且
    /// value 为空（值不回传）。env 覆盖时这才是实际生效的配置。
    pub env_effective_entries: Vec<EffectiveEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct AgentProvidersState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_provider_id: Option<String>,
    /// 切换 Provider 前 CLI 配置文件的官方 default_model（如 Kimi 的
    /// "kimi-code/k3"）。恢复官方登录时写回，避免官方登录态因
    /// default_model 缺失而断裂（kimi_runtime_config_ready 校验失败）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub official_default_model: Option<String>,
    #[serde(default)]
    pub providers: Vec<ProviderRecord>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AcpProvidersFile {
    #[serde(default = "store_version")]
    version: u32,
    #[serde(default)]
    agents: HashMap<String, AgentProvidersState>,
}

fn store_version() -> u32 {
    STORE_VERSION
}

/// `~/.pinvou3/acp-providers.json`。按 Agent 分键（agent_id: codex/claude/kimi），
/// 原子写，JSON 只存 `CredentialReference` 不存明文 key。
#[derive(Clone)]
pub struct AcpProvidersStore {
    path: PathBuf,
    agents: Arc<RwLock<HashMap<String, AgentProvidersState>>>,
}

impl AcpProvidersStore {
    pub fn load() -> Result<Self> {
        let path = crate::platform::paths::pinvou3_home().join("acp-providers.json");
        let agents = if path.exists() {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("读取 {} 失败", path.display()))?;
            serde_json::from_str::<AcpProvidersFile>(&raw)
                .with_context(|| format!("解析 {} 失败", path.display()))?
                .agents
        } else {
            HashMap::new()
        };
        Ok(Self {
            path,
            agents: Arc::new(RwLock::new(agents)),
        })
    }

    /// 损坏不阻断启动；**先备份损坏文件**（.pinvou3-bak，仅首次），避免后续
    /// persist 静默覆盖导致用户原有 Provider 列表无法找回。
    pub fn load_or_empty() -> Self {
        let path = crate::platform::paths::pinvou3_home().join("acp-providers.json");
        match Self::load() {
            Ok(store) => store,
            Err(error) => {
                eprintln!("[pinvou3-app] ACP providers unavailable, starting empty: {error:#}");
                if path.exists() {
                    let mut name = path.file_name().unwrap_or_default().to_os_string();
                    name.push(".pinvou3-bak");
                    let backup = path.with_file_name(name);
                    if !backup.exists() {
                        if let Err(backup_error) = fs::copy(&path, &backup) {
                            eprintln!(
                                "[pinvou3-app] backup broken ACP providers store failed: {backup_error:#}"
                            );
                        }
                    }
                }
                Self {
                    path,
                    agents: Arc::new(RwLock::new(HashMap::new())),
                }
            }
        }
    }

    pub(crate) fn state(&self, agent: &str) -> AgentProvidersState {
        self.agents.read().get(agent).cloned().unwrap_or_default()
    }

    pub fn current(&self, agent: &str) -> Option<String> {
        self.agents
            .read()
            .get(agent)
            .and_then(|state| state.current_provider_id.clone())
    }

    pub fn get(&self, agent: &str, provider_id: &str) -> Option<ProviderRecord> {
        self.agents
            .read()
            .get(agent)
            .and_then(|state| {
                state
                    .providers
                    .iter()
                    .find(|record| record.id == provider_id)
            })
            .cloned()
    }

    pub fn upsert(&self, agent: &str, record: ProviderRecord) -> Result<()> {
        {
            let mut agents = self.agents.write();
            let state = agents.entry(agent.to_string()).or_default();
            if let Some(existing) = state
                .providers
                .iter_mut()
                .find(|candidate| candidate.id == record.id)
            {
                *existing = record;
            } else {
                state.providers.push(record);
            }
        }
        self.persist()
    }

    pub fn remove(&self, agent: &str, provider_id: &str) -> Result<Option<ProviderRecord>> {
        let removed = {
            let mut agents = self.agents.write();
            let Some(state) = agents.get_mut(agent) else {
                return Ok(None);
            };
            match state
                .providers
                .iter()
                .position(|candidate| candidate.id == provider_id)
            {
                Some(index) => Some(state.providers.remove(index)),
                None => None,
            }
        };
        self.persist()?;
        Ok(removed)
    }

    pub fn set_current(&self, agent: &str, provider_id: Option<&str>) -> Result<()> {
        {
            let mut agents = self.agents.write();
            let state = agents.entry(agent.to_string()).or_default();
            state.current_provider_id = provider_id.map(str::to_string);
        }
        self.persist()
    }

    pub fn official_default_model(&self, agent: &str) -> Option<String> {
        self.agents
            .read()
            .get(agent)
            .and_then(|state| state.official_default_model.clone())
    }

    /// 一次持久化设置切换状态（current + official_default_model）：
    /// 两次独立 persist 中途失败会留下半切换态（复审低危 3）。
    pub fn set_switch_state(
        &self,
        agent: &str,
        provider_id: Option<&str>,
        official_default_model: Option<&str>,
    ) -> Result<()> {
        {
            let mut agents = self.agents.write();
            let state = agents.entry(agent.to_string()).or_default();
            state.current_provider_id = provider_id.map(str::to_string);
            state.official_default_model = official_default_model.map(str::to_string);
        }
        self.persist()
    }

    fn persist(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let value = AcpProvidersFile {
            version: STORE_VERSION,
            agents: self.agents.read().clone(),
        };
        fs::write(&tmp, serde_json::to_vec_pretty(&value)?)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

fn validate_agent(agent: &str) -> Result<()> {
    match agent {
        "codex" | "claude" | "kimi" => Ok(()),
        other => anyhow::bail!("不支持的 ACP Agent: {other}"),
    }
}

/// Claude Code 细化模型槽位：槽位 id → 写入 settings.json env 的变量名。
/// 槽位不填时 CC 的子 agent/辅助调用会回落官方模型（走官方流量），因此
/// 保存 claude Provider 时以下槽位全部必填。
pub(crate) const CLAUDE_MODEL_SLOTS: [(&str, &str); 5] = [
    ("opus", "ANTHROPIC_DEFAULT_OPUS_MODEL"),
    ("sonnet", "ANTHROPIC_DEFAULT_SONNET_MODEL"),
    ("haiku", "ANTHROPIC_DEFAULT_HAIKU_MODEL"),
    ("fable", "ANTHROPIC_DEFAULT_FABLE_MODEL"),
    ("subagent", "CLAUDE_CODE_SUBAGENT_MODEL"),
];

/// 各 Agent 需要检测的环境变量：(变量名, 是否凭据)。env 优先于配置文件，
/// 设置后切换 Provider 可能不生效；凭据类的值永不回传前端。
fn env_var_specs(agent: &str) -> &'static [(&'static str, bool)] {
    match agent {
        "claude" => &[
            ("ANTHROPIC_BASE_URL", false),
            ("ANTHROPIC_MODEL", false),
            ("ANTHROPIC_API_KEY", true),
            ("ANTHROPIC_AUTH_TOKEN", true),
            ("CLAUDE_CODE_OAUTH_TOKEN", true),
        ],
        "codex" => &[("OPENAI_API_KEY", true)],
        "kimi" => &[("KIMI_MODEL_NAME", false), ("KIMI_MODEL_API_KEY", true)],
        _ => &[],
    }
}

fn validate_base_url(url: &str) -> Result<()> {
    let trimmed = url.trim();
    if !trimmed.starts_with("https://") && !trimmed.starts_with("http://") {
        anyhow::bail!("Base URL 必须是 http(s):// 开头的完整地址");
    }
    Ok(())
}

fn trim_base_url(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

fn generate_provider_id() -> String {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    format!(
        "{PROVIDER_ID_PREFIX}{:08x}{:04x}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed) & 0xffff
    )
}

/// 首次受管写入前把原文件备份为 `<file>.pinvou3-bak`（只备份一次，保留初始状态）。
fn backup_once(path: &Path) -> Result<()> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".pinvou3-bak");
    let backup = path.with_file_name(name);
    if path.exists() && !backup.exists() {
        fs::copy(path, &backup).with_context(|| format!("备份 {} 失败", path.display()))?;
    }
    Ok(())
}

/// read-modify-write + `*.tmp` + `fs::rename` 原子写。
fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    backup_once(path)?;
    let tmp = path.with_extension("tmp");
    // 直接以 0600 创建临时文件（配置含明文 key，kimi/claude 的 CLI 配置），
    // 避免默认 umask 0644 让同机其他用户可读，也无「先 0644 写、后收紧」的
    // 暴露窗口（评审中危项 + 复审低危 4）。平台细节在 platform/filesystem.rs，
    // 本层不含目标平台 cfg。
    {
        use std::io::Write as _;
        let mut file = crate::platform::filesystem::create_secret_file(&tmp)
            .with_context(|| format!("创建临时文件 {} 失败", tmp.display()))?;
        file.write_all(content)
            .with_context(|| format!("写入临时文件 {} 失败", tmp.display()))?;
    }
    fs::rename(&tmp, path).with_context(|| format!("替换 {} 失败", path.display()))?;
    Ok(())
}

pub(crate) use claude::ClaudeConfigWriter;
pub(crate) use codex::codex_config_relay_env_key_present;
pub(crate) use codex::CodexConfigWriter;
pub(crate) use kimi::KimiConfigWriter;

/// 统一编排：store + 凭据 + 三写入器。命令层与 AcpPool 只与它交互。
#[derive(Clone)]
pub struct ProviderManager {
    store: AcpProvidersStore,
    credentials: SystemCredentialStore,
    claude_root: PathBuf,
    codex_root: PathBuf,
    kimi_root: PathBuf,
    /// per-agent 配置切换锁：apply 与 store 持久化之间互斥，防两个 switch 交错
    /// 导致 CLI 配置与 store.current 分裂（评审中危项）。按 agent 惰性建锁。
    switch_locks: Arc<parking_lot::Mutex<HashMap<String, Arc<parking_lot::Mutex<()>>>>>,
}

impl ProviderManager {
    pub fn new(credentials: SystemCredentialStore) -> Result<Self> {
        let home = crate::platform::os::user_home_dir();
        Ok(Self {
            store: AcpProvidersStore::load_or_empty(),
            credentials,
            claude_root: home.join(".claude"),
            codex_root: home.join(".codex"),
            kimi_root: super::kimi_data_root(),
            switch_locks: Arc::new(parking_lot::Mutex::new(HashMap::new())),
        })
    }

    /// 取（必要时创建）per-agent 配置切换锁。
    fn switch_lock(&self, agent: &str) -> Arc<parking_lot::Mutex<()>> {
        self.switch_locks
            .lock()
            .entry(agent.to_string())
            .or_insert_with(|| Arc::new(parking_lot::Mutex::new(())))
            .clone()
    }

    fn writer_for(&self, agent: &str) -> Result<Box<dyn AgentConfigWriter>> {
        validate_agent(agent)?;
        Ok(match agent {
            "claude" => Box::new(ClaudeConfigWriter::new(&self.claude_root)),
            "codex" => Box::new(CodexConfigWriter::new(&self.codex_root)),
            "kimi" => Box::new(KimiConfigWriter::new(&self.kimi_root)),
            _ => unreachable!(),
        })
    }

    /// 取 Provider 的 API key（keyring）。配置了引用但取不到时按未配置处理。
    pub fn api_key(&self, agent: &str, provider_id: &str) -> Result<Option<String>> {
        let reference = CredentialReference::for_acp_provider(agent, provider_id);
        match self.credentials.get(&reference) {
            Ok(value) => Ok(value),
            Err(error) => {
                eprintln!(
                    "[pinvou3-app] read {} provider credential failed: {}",
                    agent, error
                );
                Ok(None)
            }
        }
    }

    pub fn store(&self) -> &AcpProvidersStore {
        &self.store
    }

    fn env_conflicts(&self, agent: &str) -> Vec<String> {
        env_var_specs(agent)
            .iter()
            .map(|(name, _)| *name)
            .filter(|name| super::nonempty_env(name))
            .map(str::to_string)
            .collect()
    }

    /// env 来源的生效值（改动 5）：URL/模型名明文；凭据类 secret=true 且
    /// value 为空串——值永不离开进程。
    fn env_effective_entries(&self, agent: &str) -> Vec<EffectiveEntry> {
        env_var_specs(agent)
            .iter()
            .filter(|(name, _)| super::nonempty_env(name))
            .map(|(name, secret)| EffectiveEntry {
                key: name.to_string(),
                value: if *secret {
                    String::new()
                } else {
                    std::env::var(name).unwrap_or_default()
                },
                secret: *secret,
            })
            .collect()
    }

    /// 当前生效状态；返回 (状态, 配置文件是否不可解析)。
    fn effective(&self, agent: &str) -> (EffectiveConfig, bool) {
        match self.writer_for(agent).and_then(|writer| writer.effective()) {
            Ok(effective) => (effective, false),
            Err(error) => {
                eprintln!("[pinvou3-app] read {agent} provider config failed: {error:#}");
                (EffectiveConfig::default(), true)
            }
        }
    }

    pub fn list(&self, agent: &str) -> Result<AcpProvidersView> {
        validate_agent(agent)?;
        let state = self.store.state(agent);
        let (effective, config_unreadable) = self.effective(agent);
        // 配置文件的 relay 配置归因：codex/kimi 可反推 id 精确匹配；claude 的 env
        // 无法反推，只要 store 有当前 provider 且文件有 relay 配置即归因 App 写入。
        let managed = effective.relay_active
            && state.current_provider_id.as_deref().is_some_and(|current| {
                match effective.provider_hint.as_deref() {
                    Some(hint) => hint == current,
                    None => state.providers.iter().any(|record| record.id == current),
                }
            });
        Ok(AcpProvidersView {
            providers: state
                .providers
                .iter()
                .map(|record| ProviderView {
                    id: record.id.clone(),
                    name: record.name.clone(),
                    base_url: record.base_url.clone(),
                    model: record.model.clone(),
                    wire_api: record.wire_api,
                    has_credential: record.credential.is_some(),
                    model_slots: record.model_slots.clone(),
                    context_window: record.context_window,
                    created_at: record.created_at.clone(),
                })
                .collect(),
            current_provider_id: state.current_provider_id.clone(),
            official_active: !effective.relay_active,
            external_active: effective.relay_active && !managed,
            env_conflicts: self.env_conflicts(agent),
            config_unreadable,
            effective_entries: if config_unreadable {
                Vec::new()
            } else {
                effective.entries
            },
            env_effective_entries: self.env_effective_entries(agent),
        })
    }

    pub fn save(
        &self,
        agent: &str,
        provider_id: Option<&str>,
        name: String,
        base_url: String,
        model: Option<String>,
        model_slots: Option<HashMap<String, String>>,
        context_window: Option<i64>,
        wire_api: ProviderWireApi,
        api_key: Option<String>,
        api_key_action: CredentialEditAction,
    ) -> Result<ProviderRecord> {
        validate_agent(agent)?;
        if wire_api == ProviderWireApi::Kimi && agent != "kimi" {
            anyhow::bail!("Kimi 原生协议仅适用于 Kimi Agent");
        }
        if let Some(window) = context_window {
            if window <= 0 {
                anyhow::bail!("上下文窗口必须是正整数");
            }
        }
        // codex 的 wire_api 固定为 responses（CLI 唯一合法值），记录统一归一为
        // openai（Responses 属 OpenAI 协议家族）；早期版本可能存过 anthropic。
        let wire_api = if agent == "codex" {
            ProviderWireApi::Openai
        } else {
            wire_api
        };
        // 细化模型槽位仅 Claude Code：必填（留空槽位会让 CC 子 agent 回落官方
        // 模型走官方流量）；其余 Agent 不支持，传了直接报错而非静默丢弃。
        let model_slots = match agent {
            "claude" => {
                let mut slots = std::collections::BTreeMap::new();
                let provided = model_slots.unwrap_or_default();
                for (slot, _) in CLAUDE_MODEL_SLOTS {
                    let value = provided
                        .get(slot)
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty())
                        .with_context(|| {
                            format!("Claude Code 的 {slot} 模型为必填项（缺省会走官方流量）")
                        })?;
                    slots.insert(slot.to_string(), value);
                }
                Some(slots)
            }
            _ => {
                if model_slots.as_ref().is_some_and(|slots| !slots.is_empty()) {
                    anyhow::bail!("细化模型槽位仅 Claude Code 支持");
                }
                None
            }
        };
        let name = name.trim().to_string();
        if name.is_empty() {
            anyhow::bail!("Provider 名称不能为空");
        }
        validate_base_url(&base_url)?;
        let base_url = trim_base_url(&base_url);
        let existing = match provider_id {
            Some(id) => Some(
                self.store
                    .get(agent, id)
                    .with_context(|| format!("Provider 不存在: {id}"))?,
            ),
            None => None,
        };
        let id = existing
            .as_ref()
            .map(|record| record.id.clone())
            .unwrap_or_else(generate_provider_id);
        let reference = CredentialReference::for_acp_provider(agent, &id);
        let credential = match api_key_action {
            CredentialEditAction::Replace => {
                let key = api_key
                    .as_deref()
                    .with_context(|| "替换 Provider key 时 api_key 不能为空")?;
                self.credentials.set(&reference, key)?;
                Some(reference)
            }
            CredentialEditAction::KeepExisting => match self.credentials.get(&reference)? {
                Some(_) => Some(reference),
                None => match api_key {
                    Some(key) => {
                        self.credentials.set(&reference, &key)?;
                        Some(reference)
                    }
                    None => existing
                        .as_ref()
                        .and_then(|record| record.credential.clone()),
                },
            },
            CredentialEditAction::Delete => {
                self.credentials.delete(&reference).ok();
                None
            }
        };
        let record = ProviderRecord {
            id,
            name,
            base_url,
            model,
            model_slots,
            context_window,
            wire_api,
            credential,
            created_at: existing
                .as_ref()
                .map(|record| record.created_at.clone())
                .unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
        };
        // 编辑**生效中**的 Provider 必须同步重写 CLI 配置，否则 base_url/模型
        // 不生效、生效区展示陈旧（评审中危项）。与 switch 同锁防交错。
        if self.store.current(agent).as_deref() == Some(record.id.as_str()) {
            let lock = self.switch_lock(agent);
            let _switch_guard = lock.lock();
            let key = self.api_key(agent, &record.id)?;
            let writer = self.writer_for(agent)?;
            writer.apply(&ProviderTarget::from_record(&record, key))?;
            if let Err(error) = self.store.upsert(agent, record.clone()) {
                // store 持久化失败：回滚配置写入（含 kimi 的 default_model），
                // 保持「失败 = 什么都没发生」语义。回滚失败如实附加（复审 F3）。
                let mut context = "保存失败：配置已写入但无法保存 Provider 状态，已尝试回滚配置；请检查磁盘后重试".to_string();
                if let Err(rollback) =
                    writer.revert_to_official(Some(&ProviderTarget::from_record(&record, None)))
                {
                    context = format!("{context}；回滚配置也失败: {rollback:#}");
                } else if let Err(rollback) = writer
                    .restore_default_model(self.store.official_default_model(agent).as_deref())
                {
                    context = format!("{context}；写回官方 default_model 也失败: {rollback:#}");
                }
                return Err(error.context(context));
            }
        } else {
            self.store.upsert(agent, record.clone())?;
        }
        Ok(record)
    }

    /// 删除 Provider。若删除的是当前 Provider：**先回退 CLI 配置**（成功后才
    /// 删除 store 记录与凭据）——避免「配置残留 pv-* 但 store 已无记录、
    /// 无法再通过 App 恢复」的状态（M1 delete 路径）。
    pub fn delete(&self, agent: &str, provider_id: &str) -> Result<Option<ProviderRecord>> {
        validate_agent(agent)?;
        // 删除当前 Provider 会回退 CLI 配置：与 switch/save 同锁，防交错。
        let lock = self.switch_lock(agent);
        let _switch_guard = lock.lock();
        let was_current = self.store.current(agent).as_deref() == Some(provider_id);
        let removed = self.store.get(agent, provider_id);
        if was_current {
            match removed.as_ref() {
                Some(record) => self.switch_official_after_removal(agent, record)?,
                // 已持锁：调无锁实现，避免重复加锁死锁（parking_lot 非可重入）。
                None => self.switch_official_locked(agent)?,
            }
        }
        let removed = self.store.remove(agent, provider_id)?;
        if removed.is_some() {
            let reference = CredentialReference::for_acp_provider(agent, provider_id);
            self.credentials.delete(&reference).ok();
        }
        Ok(removed)
    }

    /// 切换 Provider：写入 CLI 配置文件 → store 持久化。失败不报成功。
    /// 切换前记录官方 default_model，供恢复官方登录时写回。
    ///
    /// apply 成功后若 store 持久化失败，**回滚配置文件**，避免出现
    /// 「UI 显示未切换、实际配置已改」的分裂状态。
    pub fn switch(&self, agent: &str, provider_id: &str) -> Result<()> {
        validate_agent(agent)?;
        // 全程持 per-agent 锁：apply 与 store 持久化之间互斥，防双 switch
        // 交错（评审中危项）。
        let lock = self.switch_lock(agent);
        let _switch_guard = lock.lock();
        let record = self
            .store
            .get(agent, provider_id)
            .with_context(|| format!("Provider 不存在: {provider_id}"))?;
        let key = self
            .api_key(agent, provider_id)?
            .with_context(|| "Provider 未配置 API key，无法切换；请先保存 API key")?;
        let writer = self.writer_for(agent)?;
        // 官方 default_model 只在首次切换时记录；连切 A→B 时 config 的
        // default_model 已是受管值（读到 None），必须**保留 store 旧值**，
        // 否则恢复官方时登录态断裂（N5）。
        let official_default_model = match writer.current_default_model()? {
            Some(value) => Some(value),
            None => self.store.official_default_model(agent),
        };
        writer.apply(&ProviderTarget::from_record(&record, Some(key)))?;
        // 单次持久化写入切换状态（低危 3：两步 persist 会留下半切换态）。
        let persisted = self.store.set_switch_state(
            agent,
            Some(provider_id),
            official_default_model.as_deref(),
        );
        if let Err(error) = persisted {
            // 回滚配置写入，保持「失败 = 什么都没发生」语义；kimi 的 apply 会
            // 覆盖 default_model，必须一并写回官方值，否则官方登录态断裂。
            // 回滚本身失败时如实附加，不无条件声称已回滚（复审 F3）。
            let mut context = "切换 Provider 失败：配置已写入但无法保存切换状态，已尝试回滚配置；请检查磁盘后重试".to_string();
            if let Err(rollback) =
                writer.revert_to_official(Some(&ProviderTarget::from_record(&record, None)))
            {
                context = format!("{context}；回滚配置也失败: {rollback:#}");
            } else if let Err(rollback) =
                writer.restore_default_model(official_default_model.as_deref())
            {
                context = format!("{context}；写回官方 default_model 也失败: {rollback:#}");
            }
            return Err(error.context(context));
        }
        Ok(())
    }

    /// 恢复官方登录：只删除本功能写入的键/表，并写回切换前记录的官方
    /// default_model（Kimi 必需，否则官方登录态会因 default_model 缺失断裂）。
    pub fn switch_official(&self, agent: &str) -> Result<()> {
        validate_agent(agent)?;
        // 与 switch/save/delete 同锁：防「恢复官方」与并发切换 Provider 交错
        // 出现「配置已回官方、store.current=B」的分裂态（复审 F2）。
        let lock = self.switch_lock(agent);
        let _switch_guard = lock.lock();
        self.switch_official_locked(agent)
    }

    /// 无锁内部实现：调用方必须已持 per-agent 切换锁（delete 持锁后调用）。
    fn switch_official_locked(&self, agent: &str) -> Result<()> {
        let current = self.store.current(agent);
        let reverted = current
            .as_deref()
            .and_then(|id| self.store.get(agent, id))
            .map(|record| ProviderTarget::from_record(&record, None));
        let official_default_model = self.store.official_default_model(agent);
        let writer = self.writer_for(agent)?;
        writer.revert_to_official(reverted.as_ref())?;
        writer.restore_default_model(official_default_model.as_deref())?;
        self.store.set_current(agent, None)?;
        Ok(())
    }

    /// 删除当前 Provider 后的回退：记录已从 store 移除，显式传入 removed 记录
    /// 供 revert 精确清理（例如 codex 顶层 model 只在与被删 Provider 的 model
    /// 相同时删除）。
    pub fn switch_official_after_removal(
        &self,
        agent: &str,
        removed: &ProviderRecord,
    ) -> Result<()> {
        validate_agent(agent)?;
        let official_default_model = self.store.official_default_model(agent);
        let writer = self.writer_for(agent)?;
        writer.revert_to_official(Some(&ProviderTarget::from_record(removed, None)))?;
        writer.restore_default_model(official_default_model.as_deref())?;
        self.store.set_current(agent, None)?;
        Ok(())
    }

    /// 导出为 JSON（**含明文 key**，调用方必须警示用户；文件仅保存在本机）。
    pub fn export(&self, agent: &str) -> Result<String> {
        validate_agent(agent)?;
        #[derive(Serialize)]
        struct ExportEntry {
            id: String,
            name: String,
            base_url: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            model: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            model_slots: Option<std::collections::BTreeMap<String, String>>,
            #[serde(skip_serializing_if = "Option::is_none")]
            context_window: Option<i64>,
            wire_api: ProviderWireApi,
            #[serde(skip_serializing_if = "Option::is_none")]
            api_key: Option<String>,
        }
        let mut entries = Vec::new();
        for record in &self.store.state(agent).providers {
            entries.push(ExportEntry {
                id: record.id.clone(),
                name: record.name.clone(),
                base_url: record.base_url.clone(),
                model: record.model.clone(),
                model_slots: record.model_slots.clone(),
                context_window: record.context_window,
                wire_api: record.wire_api,
                api_key: self.api_key(agent, &record.id)?,
            });
        }
        serde_json::to_string_pretty(&entries).context("序列化 Provider 导出失败")
    }

    pub fn import(&self, agent: &str, json: &str) -> Result<ImportResult> {
        validate_agent(agent)?;
        // 同时接受 snake_case（App 导出格式）与 camelCase（表单提示/手写格式），
        // 避免「按提示填写却被跳过」的格式矛盾。
        #[derive(Deserialize)]
        struct ImportEntry {
            #[serde(default)]
            id: String,
            name: String,
            #[serde(alias = "baseUrl", default)]
            base_url: String,
            #[serde(default)]
            model: Option<String>,
            #[serde(alias = "modelSlots", default)]
            model_slots: Option<HashMap<String, String>>,
            #[serde(alias = "contextWindow", default)]
            context_window: Option<i64>,
            #[serde(alias = "wireApi", default)]
            wire_api: Option<ProviderWireApi>,
            #[serde(alias = "apiKey", default)]
            api_key: Option<String>,
        }
        let entries: Vec<ImportEntry> =
            serde_json::from_str(json).context("导入文件不是有效的 Provider JSON")?;
        let mut result = ImportResult::default();
        for entry in entries {
            // context_window ≤ 0 会写坏 kimi 配置（max_context_size 必须为正，
            // 评审中危项）；名称/地址非法一并跳过。
            if entry.name.trim().is_empty()
                || validate_base_url(&entry.base_url).is_err()
                || entry.context_window.is_some_and(|window| window <= 0)
            {
                result.skipped += 1;
                continue;
            }
            // id 冲突（已存在或格式非法——仅前缀匹配不够，非法字符会写进 TOML
            // 表名）时重新生成并告警。
            let id = if entry.id.starts_with(PROVIDER_ID_PREFIX)
                && entry.id.len() <= 64
                && entry
                    .id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                && self.store.get(agent, &entry.id).is_none()
            {
                entry.id.clone()
            } else {
                result.id_conflicts += 1;
                generate_provider_id()
            };
            let reference = CredentialReference::for_acp_provider(agent, &id);
            // 细化模型槽位仅 claude 使用：导入缺槽位时用条目 model 兜底填充，
            // 既无槽位又无 model 的 claude 条目按无效跳过（必填约束同 save）。
            let model_slots = if agent == "claude" {
                let provided = entry.model_slots.unwrap_or_default();
                let mut slots = std::collections::BTreeMap::new();
                let mut complete = true;
                for (slot, _) in CLAUDE_MODEL_SLOTS {
                    let value = provided
                        .get(slot)
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty())
                        .or_else(|| entry.model.clone());
                    match value {
                        Some(value) => {
                            slots.insert(slot.to_string(), value);
                        }
                        None => {
                            complete = false;
                            break;
                        }
                    }
                }
                if !complete {
                    result.skipped += 1;
                    continue;
                }
                Some(slots)
            } else {
                None
            };
            let entry_name = entry.name.trim().to_string();
            // per-entry 错误收集：任一条目失败不中断整批，最后汇总（复审低危 2）。
            let credential = match &entry.api_key {
                Some(key) => match self.credentials.set(&reference, key) {
                    Ok(()) => Some(reference.clone()),
                    Err(error) => {
                        result
                            .errors
                            .push(format!("写入 {entry_name} 密钥失败: {error:#}"));
                        result.skipped += 1;
                        continue;
                    }
                },
                None => None,
            };
            match self.store.upsert(
                agent,
                ProviderRecord {
                    id,
                    name: entry_name.clone(),
                    base_url: trim_base_url(&entry.base_url),
                    model: entry.model,
                    model_slots,
                    context_window: entry.context_window,
                    wire_api: entry.wire_api.unwrap_or_default(),
                    credential: credential.clone(),
                    created_at: chrono::Utc::now().to_rfc3339(),
                },
            ) {
                Ok(()) => result.imported += 1,
                Err(error) => {
                    // store 落盘失败：删除刚写入的孤儿凭据，避免无记录的 keyring 残留
                    if credential.is_some() {
                        self.credentials.delete(&reference).ok();
                    }
                    result
                        .errors
                        .push(format!("保存 {entry_name} 失败: {error:#}"));
                    result.skipped += 1;
                }
            }
        }
        Ok(result)
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub imported: usize,
    pub id_conflicts: usize,
    pub skipped: usize,
    /// per-entry 错误明细（凭据写入/落盘失败），前端可展示（复审低危 2）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

/// 写入器共享的常量与助手（供各 writer 使用，避免重复定义）。
pub(crate) fn kimi_default_model() -> &'static str {
    KIMI_DEFAULT_MODEL
}

pub(crate) fn kimi_default_context_size() -> i64 {
    KIMI_DEFAULT_CONTEXT_SIZE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_store(dir: &Path) -> AcpProvidersStore {
        let path = dir.join("acp-providers.json");
        AcpProvidersStore {
            path,
            agents: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    #[test]
    fn store_roundtrip_and_atomic() {
        let current = std::thread::current();
        let test = current.name().unwrap_or_default();
        let dir = std::env::temp_dir().join(format!("acp-providers-test-{test}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let store = tmp_store(&dir);
        let record = ProviderRecord {
            id: "pv-1234567890ab".into(),
            name: "我的中转".into(),
            base_url: "https://api.example.com/v1".into(),
            model: Some("gpt-5.2".into()),
            model_slots: None,
            context_window: None,
            wire_api: ProviderWireApi::Openai,
            credential: Some(CredentialReference::for_acp_provider(
                "codex",
                "pv-1234567890ab",
            )),
            created_at: "2026-08-04T00:00:00Z".into(),
        };
        store.upsert("codex", record.clone()).unwrap();
        store.set_current("codex", Some("pv-1234567890ab")).unwrap();
        assert_eq!(
            store.get("codex", "pv-1234567890ab").unwrap().name,
            "我的中转"
        );
        assert_eq!(store.current("codex").unwrap(), "pv-1234567890ab");
        // 原子写：无残留 .tmp 文件
        assert!(!store.path.with_extension("json.tmp").exists());
        // 从同一路径重新加载验证往返
        let reloaded = load_from_path(&store.path);
        assert_eq!(reloaded.state("codex").providers.len(), 1);
        // 明文 key 字段不存在
        let raw = fs::read_to_string(&store.path).unwrap();
        assert!(!raw.contains("api_key"));
        assert!(raw.contains("pinvou3-acp-provider-key"));
        let _ = fs::remove_dir_all(&dir);
    }

    fn load_from_path(path: &Path) -> AcpProvidersStore {
        let agents = if path.exists() {
            let raw = fs::read_to_string(path).unwrap();
            serde_json::from_str::<AcpProvidersFile>(&raw)
                .unwrap()
                .agents
        } else {
            HashMap::new()
        };
        AcpProvidersStore {
            path: path.to_path_buf(),
            agents: Arc::new(RwLock::new(agents)),
        }
    }

    #[test]
    fn remove_clears_current_candidate() {
        let current = std::thread::current();
        let test = current.name().unwrap_or_default();
        let dir = std::env::temp_dir().join(format!("acp-providers-test-rm-{test}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let store = tmp_store(&dir);
        store
            .upsert(
                "claude",
                ProviderRecord {
                    id: "pv-111111111111".into(),
                    name: "t".into(),
                    base_url: "https://api.example.com".into(),
                    model: None,
                    model_slots: None,
                    context_window: None,
                    wire_api: ProviderWireApi::Anthropic,
                    credential: None,
                    created_at: String::new(),
                },
            )
            .unwrap();
        let removed = store.remove("claude", "pv-111111111111").unwrap();
        assert!(removed.is_some());
        assert!(store.remove("claude", "pv-000000000000").unwrap().is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_and_trim() {
        assert!(validate_base_url("https://api.example.com").is_ok());
        assert!(validate_base_url("api.example.com").is_err());
        assert!(validate_base_url("ftp://x").is_err());
        assert_eq!(trim_base_url(" https://a.com/v1/ "), "https://a.com/v1");
    }

    #[test]
    fn provider_id_has_prefix_and_is_unique() {
        let a = generate_provider_id();
        let b = generate_provider_id();
        assert!(a.starts_with(PROVIDER_ID_PREFIX));
        assert!(a.len() == 12 + PROVIDER_ID_PREFIX.len());
        assert_ne!(a, b);
    }

    #[test]
    fn wire_api_parse() {
        assert_eq!(
            ProviderWireApi::parse(None).unwrap(),
            ProviderWireApi::Anthropic
        );
        assert_eq!(
            ProviderWireApi::parse(Some("openai")).unwrap(),
            ProviderWireApi::Openai
        );
        assert!(ProviderWireApi::parse(Some("bogus")).is_err());
    }
}

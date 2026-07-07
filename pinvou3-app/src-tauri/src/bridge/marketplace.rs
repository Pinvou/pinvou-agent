//! 工具市场管理器 — 管理 MCP 工具的安装/卸载/状态查询。
//!
//! 每个工具是一个 MCP server，元数据定义在 `manifest.json`。
//! 安装状态持久化在 `~/.pinvou3/marketplace/installed.json`。
//! 安装/卸载时同步修改 `~/.pinvou3/bundle/mcp.json`。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::paths;
use crate::credential_store::{
    redact_secret, CredentialError, CredentialReference, CredentialStore, SystemCredentialStore,
};

// ---------------------------------------------------------------------------
// Manifest — 每个 MCP 工具的元数据
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolManifest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub icon: String,
    pub category: String,
    pub mcp_tools: Vec<String>,
    pub command: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub secret_env: Vec<SecretEnv>,
    #[serde(default)]
    pub secret_headers: Vec<SecretHeader>,
    #[serde(default)]
    pub config_fields: Vec<ConfigField>,
    #[serde(default)]
    pub routing_rules: Vec<String>,
    #[serde(default)]
    pub tool_table_entries: Vec<String>,
    #[serde(default)]
    pub pip_dependencies: Vec<String>,
    #[serde(default)]
    pub servers: Vec<RemoteServer>,
    /// 配套技能 id:装该 MCP 时一并装、卸时一并删(让"一个能力"=引擎+引导整体装卸)。
    #[serde(default)]
    pub companion_skills: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteServer {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretEnv {
    pub key: String,
    pub provider: String,
    #[serde(default = "default_required")]
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretHeader {
    pub header: String,
    #[serde(default = "default_bearer_scheme")]
    pub scheme: String,
    pub source_key: String,
    pub provider: String,
    #[serde(default = "default_required")]
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigField {
    pub key: String,
    pub label: String,
    pub required: bool,
    /// "env" = 写入 mcp.json env 字段, "bearer" = 写入 headers Authorization
    #[serde(default = "default_target")]
    pub target: String,
    #[serde(default)]
    pub secret: bool,
}

fn default_target() -> String {
    "env".to_string()
}

fn default_required() -> bool {
    true
}

fn default_bearer_scheme() -> String {
    "Bearer".to_string()
}

fn is_sensitive_key_name(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    upper.ends_with("_API_KEY")
        || upper.ends_with("_TOKEN")
        || upper.ends_with("_SECRET")
        || upper == "API_KEY"
        || upper == "TOKEN"
        || upper == "SECRET"
        || upper == "KEY"
}

fn mcp_secret_env_var(secret_name: &str) -> String {
    let suffix = secret_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("PINVOU3_MCP_SECRET_{suffix}")
}

fn mcp_secret_placeholder(secret_name: &str) -> String {
    format!("${{{}}}", mcp_secret_env_var(secret_name))
}

fn write_json_pretty(path: &std::path::Path, value: &serde_json::Value) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| format!("写入 {} 失败: {e}", path.display()))
}

fn mcp_secret_reference(tool_id: &str, target: &str, key: &str) -> CredentialReference {
    CredentialReference::for_mcp_secret(tool_id, target, key)
}

fn mcp_secret_missing_error(tool_id: &str, key: &str) -> String {
    format!("MCP 工具 '{tool_id}' 缺少密钥 {key}，请重新配置后再启用该工具")
}

fn mcp_secret_store_error(tool_id: &str, key: &str, error: CredentialError) -> String {
    redact_secret(&format!(
        "MCP 工具 '{tool_id}' 的密钥 {key} 无法访问: {}",
        error.user_message()
    ))
}

#[derive(Debug, Clone, Copy)]
struct BuiltinMcpSecretSpec {
    tool_id: &'static str,
    target: &'static str,
    key: &'static str,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpSecretMigrationResult {
    pub migrated_count: usize,
    pub skipped_count: usize,
    pub failed_count: usize,
    pub messages: Vec<String>,
}

fn builtin_mcp_secret_specs() -> &'static [BuiltinMcpSecretSpec] {
    &[
        BuiltinMcpSecretSpec {
            tool_id: "weather",
            target: "env",
            key: "AMAP_KEY",
        },
        BuiltinMcpSecretSpec {
            tool_id: "iwencai",
            target: "env",
            key: "IWENCAI_API_KEY",
        },
        BuiltinMcpSecretSpec {
            tool_id: "qcc",
            target: "header",
            key: "QCC_API_KEY",
        },
    ]
}

fn builtin_spec_for_tool(tool_id: &str) -> Option<&'static BuiltinMcpSecretSpec> {
    builtin_mcp_secret_specs()
        .iter()
        .find(|spec| spec.tool_id == tool_id)
}

fn builtin_spec_for_server_name(server_name: &str) -> Option<&'static BuiltinMcpSecretSpec> {
    if server_name == "weather" {
        builtin_spec_for_tool("weather")
    } else if server_name == "iwencai" {
        builtin_spec_for_tool("iwencai")
    } else if server_name.starts_with("qcc-") {
        builtin_spec_for_tool("qcc")
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// MarketplaceToolInfo — 前端展示用
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceToolInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub icon: String,
    pub category: String,
    pub installed: bool,
    /// 配套技能 id(来自 manifest `companion_skills`)。前端据此把「有配套 MCP 的技能卡」的
    /// 状态/装卸联动到本 MCP,单一真源在 manifest,避免命名不一致(gongwen↔government-writing)
    /// 时前端漏建映射导致两卡状态分叉。
    #[serde(default)]
    pub companion_skills: Vec<String>,
}

// ---------------------------------------------------------------------------
// 会话工具开关:全局持久的"被禁用连接器"列表(用户关一次,所有新对话/窗口都继承,
// 直到手动开回 —— 见「工具开关」方案,持久语义)。落盘到 ~/.pinvou3/disabled_connectors.json。
// ---------------------------------------------------------------------------

fn disabled_connectors_path() -> std::path::PathBuf {
    paths::pinvou3_home().join("disabled_connectors.json")
}

/// 读全局被禁用的连接器 id 列表(读不到/空 → 空)。
pub fn load_disabled_connectors() -> Vec<String> {
    std::fs::read_to_string(disabled_connectors_path())
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default()
}

/// 写全局被禁用的连接器 id 列表。
pub fn save_disabled_connectors(ids: &[String]) {
    if let Ok(json) = serde_json::to_string(ids) {
        let _ = std::fs::write(disabled_connectors_path(), json);
    }
}

/// 内置共享 key 的编译期注入值:仅当 release 构建 export 了对应 env 才有值。
/// key 因此不落盘明文、不进 git、不进源码 —— 发布构建在 release-deb.sh 里 export
/// 真 key(见步8 轮换);开发构建不设则为 None(开发自行配 key 或不用这三个内置工具)。
fn builtin_shared_secret_value(key: &str) -> Option<&'static str> {
    let v = match key {
        "AMAP_KEY" => option_env!("PINVOU3_BUILTIN_AMAP_KEY"),
        "IWENCAI_API_KEY" => option_env!("PINVOU3_BUILTIN_IWENCAI_KEY"),
        "QCC_API_KEY" => option_env!("PINVOU3_BUILTIN_QCC_KEY"),
        _ => None,
    }?;
    let v = v.trim();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// 仅**内置三件套**(精确匹配 tool_id + target + key)才返回共享 key ——
/// 防自定义/上传工具声明同名 key(AMAP_KEY 等)、用户留空时蹭内置额度。
fn builtin_shared_secret_value_for(tool_id: &str, target: &str, key: &str) -> Option<&'static str> {
    let is_builtin = builtin_mcp_secret_specs()
        .iter()
        .any(|s| s.tool_id == tool_id && s.target == target && s.key == key);
    if is_builtin {
        builtin_shared_secret_value(key)
    } else {
        None
    }
}

/// 从 manifest 提取所有 secret 的 (keyring target, key):
/// `secret_env`→("env",key)、`secret_headers`→("header",source_key)、
/// `config_fields`(secret=true)→(env 或 header, key)。同一 (target,key) 去重一次。
/// 与 install 时 `resolve_secret_placeholder` 用的 target 对齐(config_fields 的
/// "bearer" 在 install 里落成 reference target "header")。
fn manifest_secret_targets(manifest: &ToolManifest) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut push = |target: &str, key: &str| {
        let pair = (target.to_string(), key.to_string());
        if !out.contains(&pair) {
            out.push(pair);
        }
    };
    for s in &manifest.secret_env {
        push("env", &s.key);
    }
    for s in &manifest.secret_headers {
        push("header", &s.source_key);
    }
    for f in &manifest.config_fields {
        if f.secret {
            let target = if f.target == "bearer" {
                "header"
            } else {
                f.target.as_str()
            };
            push(target, &f.key);
        }
    }
    out
}

/// 重启后把**所有已安装工具**的 secret 从 keyring 重灌进进程 env(MCP 子进程 expand
/// `${...}` 占位符用)。不再硬编码内置 3 个 —— 自定义/上传的带 secret 工具重启后同样生效。
pub fn sync_mcp_secret_env_vars() -> Result<(), String> {
    let store = SystemCredentialStore::new();
    let mgr = MarketplaceManager::new();
    for tool_id in mgr.installed_ids() {
        let Some(manifest) = mgr.load_manifest(&tool_id) else {
            continue;
        };
        for (target, key) in manifest_secret_targets(&manifest) {
            let reference = mcp_secret_reference(&tool_id, &target, &key);
            match store.get(&reference) {
                Ok(Some(value)) if !value.trim().is_empty() => {
                    std::env::set_var(mcp_secret_env_var(&key), value);
                }
                Ok(_) => {}
                Err(e) => return Err(mcp_secret_store_error(&tool_id, &key, e)),
            }
        }
    }
    Ok(())
}

/// 当前被禁用连接器 → 模型可见工具全名(喂给引擎 disallowed_tools 的)。
pub fn disabled_tool_names() -> Vec<String> {
    MarketplaceManager::new().model_tool_names(&load_disabled_connectors())
}

// ---------------------------------------------------------------------------
// MarketplaceManager
// ---------------------------------------------------------------------------

pub struct MarketplaceManager<S: CredentialStore = SystemCredentialStore> {
    /// bundle 解包后的 MCP servers 目录 (~/.pinvou3/bundle/mcp-servers/)
    servers_dir: PathBuf,
    /// 已安装工具列表文件 (~/.pinvou3/marketplace/installed.json)
    installed_file: PathBuf,
    credential_store: S,
}

impl MarketplaceManager<SystemCredentialStore> {
    pub fn new() -> Self {
        Self::with_store(SystemCredentialStore::new())
    }
}

impl<S: CredentialStore> MarketplaceManager<S> {
    pub fn with_store(credential_store: S) -> Self {
        let servers_dir = paths::bundle_mcp_servers_dir();
        let installed_file = paths::pinvou3_home().join("marketplace").join("installed.json");
        Self {
            servers_dir,
            installed_file,
            credential_store,
        }
    }

    /// 扫描 bundle mcp-servers/ 下所有含 manifest.json 的子目录
    pub fn available_tools(&self) -> Vec<ToolManifest> {
        let mut tools = Vec::new();
        let dir = match std::fs::read_dir(&self.servers_dir) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("[marketplace] read_dir error: {e}");
                return tools;
            }
        };
        for entry in dir.flatten() {
            let manifest_path = entry.path().join("manifest.json");
            if manifest_path.is_file() {
                match std::fs::read_to_string(&manifest_path) {
                    Ok(content) => match serde_json::from_str::<ToolManifest>(&content) {
                        Ok(manifest) => {
                            tools.push(manifest);
                        }
                        Err(e) => eprintln!("[marketplace] parse error: {e}"),
                    },
                    Err(e) => eprintln!("[marketplace] read error: {e}"),
                }
            }
        }
        tools
    }

    /// 已安装的工具 ID 列表
    pub fn installed_ids(&self) -> Vec<String> {
        let content = match std::fs::read_to_string(&self.installed_file) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        match serde_json::from_str::<Vec<String>>(&content) {
            Ok(ids) => ids,
            Err(e) => {
                eprintln!(
                    "[marketplace] installed.json is invalid: {e}; backing up and rebuilding from mcp.json"
                );
                self.backup_corrupt_installed(&content);
                let recovered = self.recover_installed_ids_from_mcp();
                if let Err(write_err) = self.save_installed(&recovered) {
                    eprintln!("[marketplace] failed to rewrite installed.json: {write_err}");
                }
                recovered
            }
        }
    }

    /// 前端列表：所有可用工具 + 安装状态
    pub fn list_tools(&self) -> Vec<MarketplaceToolInfo> {
        let installed = self.installed_ids();
        self.available_tools()
            .into_iter()
            .map(|m| MarketplaceToolInfo {
                installed: installed.contains(&m.id),
                id: m.id,
                name: m.name,
                description: m.description,
                version: m.version,
                icon: m.icon,
                category: m.category,
                companion_skills: m.companion_skills,
            })
            .collect()
    }

    /// 安装工具：写 installed.json + 更新 mcp.json
    /// `user_config` 是前端传入的用户配置（如 API Key），对应 config_fields
    pub fn install(
        &self,
        tool_id: &str,
        user_config: &std::collections::HashMap<String, String>,
    ) -> Result<(), String> {
        self.migrate_mcp_plaintext_secrets()?;
        let manifest = self
            .load_manifest(tool_id)
            .ok_or_else(|| format!("工具 '{tool_id}' 不存在"))?;

        // 先装 Python 依赖（跨平台 pip）；失败就不注册，让用户可重试。零依赖工具会直接跳过。
        self.pip_install_deps(&manifest)?;

        // 先写 mcp.json(含 resolve_secret_placeholder,缺密钥会失败);成功后才落
        // installed.json —— 避免「installed 已写、mcp 没注册」的半安装状态。
        self.add_to_mcp_json(&manifest, user_config)?;

        let mut installed = self.installed_ids();
        if !installed.contains(&tool_id.to_string()) {
            installed.push(tool_id.to_string());
        }
        self.save_installed(&installed)?;

        Ok(())
    }

    pub fn migrate_mcp_plaintext_secrets(&self) -> Result<McpSecretMigrationResult, String> {
        let mut result = McpSecretMigrationResult::default();
        for spec in builtin_mcp_secret_specs() {
            let path = self.servers_dir.join(spec.tool_id).join("manifest.json");
            if path.is_file() {
                self.migrate_manifest_file(&path, spec, &mut result)?;
            }
        }
        let mcp_path = paths::mcp_config_path();
        if mcp_path.is_file() {
            self.migrate_mcp_json_file(&mcp_path, &mut result)?;
        }
        Ok(result)
    }

    /// 装 `manifest.pip_dependencies` 里的 Python 依赖（跨平台）。
    /// 用 `python -m pip install`（保证装进跑 MCP server 的同一个 python，不裸 `pip`）。
    /// ① 先预检依赖是否已可用（系统已装/此前装过）→ 命中即跳过，不跑 pip；
    /// ② 否则按序兜底：`--user` → `--user --break-system-packages`（PEP 668）→ `--break-system-packages`，任一成功即 Ok。
    /// 零依赖工具（pip_dependencies 为空）直接返回 Ok，不影响 weather/obsidian 等。
    fn pip_install_deps(&self, manifest: &ToolManifest) -> Result<(), String> {
        if manifest.pip_dependencies.is_empty() {
            return Ok(());
        }
        // Windows:python-pptx 等依赖已随内置 python(python-win)预装,不在用户机器
        // 跑 pip —— 用户也就不需要自己装 python。仅 Linux/macOS 走系统 python3 联网 pip。
        if cfg!(target_os = "windows") {
            return Ok(());
        }
        let python_cmd = "python3";
        let deps = &manifest.pip_dependencies;

        // ① 预检:依赖已可用就直接 Ok,不跑 pip。用 importlib.metadata 按 PyPI 包名查
        //    (python-pptx 等),全部命中即满足。修「明明已装(系统包/此前装过)却仍判失败」。
        let satisfied = std::process::Command::new(python_cmd)
            .arg("-c")
            .arg("import importlib.metadata as m, sys; [m.version(p) for p in sys.argv[1:]]")
            .args(deps)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if satisfied {
            return Ok(());
        }

        // ② pip 安装,按序兜底,任一成功即 Ok:
        //    --user(常规)→ --user --break-system-packages(PEP 668:现代 Debian/Ubuntu 拦 --user,
        //    装进 ~/.local 用户目录、不动系统/发行版包)→ --break-system-packages(某些环境 --user 不可用)。
        let run = |extra: &[&str]| -> std::io::Result<std::process::Output> {
            let mut cmd = std::process::Command::new(python_cmd);
            cmd.args(["-m", "pip", "install", "--disable-pip-version-check", "--no-input"]);
            cmd.args(extra);
            cmd.args(deps);
            cmd.output()
        };
        let attempts: [&[&str]; 3] = [
            &["--user"],
            &["--user", "--break-system-packages"],
            &["--break-system-packages"],
        ];
        let mut last_err = String::new();
        for extra in attempts {
            match run(extra) {
                Ok(o) if o.status.success() => return Ok(()),
                Ok(o) => {
                    last_err = String::from_utf8_lossy(&o.stderr)
                        .trim()
                        .lines()
                        .last()
                        .unwrap_or("")
                        .to_string();
                }
                Err(e) => {
                    return Err(format!(
                        "无法运行 {python_cmd}（请确认已安装 Python 且在 PATH 中）：{e}"
                    ));
                }
            }
        }
        Err(format!(
            "依赖安装失败（pip）：{last_err}（已尝试 --user 与 --break-system-packages;请确认网络可达且 python3 自带 pip）"
        ))
    }

    fn resolve_secret_placeholder(
        &self,
        tool_id: &str,
        target: &str,
        key: &str,
        user_config: &std::collections::HashMap<String, String>,
        legacy_env: &std::collections::HashMap<String, String>,
    ) -> Result<String, String> {
        let reference = mcp_secret_reference(tool_id, target, key);
        if let Some(value) = user_config.get(key).filter(|v| !v.trim().is_empty()) {
            self.credential_store
                .set(&reference, value)
                .map_err(|e| mcp_secret_store_error(tool_id, key, e))?;
            std::env::set_var(mcp_secret_env_var(key), value);
            return Ok(mcp_secret_placeholder(key));
        }

        match self.credential_store.get(&reference) {
            Ok(Some(value)) if !value.trim().is_empty() => {
                std::env::set_var(mcp_secret_env_var(key), value);
                Ok(mcp_secret_placeholder(key))
            }
            Ok(_) => {
                if let Some(value) = legacy_env.get(key).filter(|v| !v.trim().is_empty()) {
                    self.credential_store
                        .set(&reference, value)
                        .map_err(|e| mcp_secret_store_error(tool_id, key, e))?;
                    std::env::set_var(mcp_secret_env_var(key), value);
                    Ok(mcp_secret_placeholder(key))
                } else if let Some(shared) = builtin_shared_secret_value_for(tool_id, target, key) {
                    // 内置三件套共享 key 兜底:用户留空即用注入的共享额度(开箱即用)。
                    // 精确匹配 tool_id+target+key → 自定义工具声明同名 key 不会误拿内置额度。
                    // 注入时机在此(install/启用),不在启动全量 → 卸载删 secret 后不会被注回。
                    self.credential_store
                        .set(&reference, shared)
                        .map_err(|e| mcp_secret_store_error(tool_id, key, e))?;
                    std::env::set_var(mcp_secret_env_var(key), shared);
                    Ok(mcp_secret_placeholder(key))
                } else {
                    Err(mcp_secret_missing_error(tool_id, key))
                }
            }
            Err(e) => Err(mcp_secret_store_error(tool_id, key, e)),
        }
    }

    fn migrate_manifest_file(
        &self,
        path: &std::path::Path,
        spec: &BuiltinMcpSecretSpec,
        result: &mut McpSecretMigrationResult,
    ) -> Result<(), String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
        let mut json: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("解析 {} 失败: {e}", path.display()))?;
        let Some(value) = json
            .get("env")
            .and_then(|env| env.get(spec.key))
            .and_then(|v| v.as_str())
            .filter(|v| !v.trim().is_empty())
            .map(ToOwned::to_owned)
        else {
            return Ok(());
        };

        self.store_migrated_secret(spec, &value, result)?;
        if let Some(env) = json.get_mut("env").and_then(|env| env.as_object_mut()) {
            env.remove(spec.key);
            if env.is_empty() {
                json.as_object_mut().map(|obj| obj.remove("env"));
            }
        }
        write_json_pretty(path, &json)?;
        Ok(())
    }

    fn migrate_mcp_json_file(
        &self,
        path: &std::path::Path,
        result: &mut McpSecretMigrationResult,
    ) -> Result<(), String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
        let mut json: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("解析 {} 失败: {e}", path.display()))?;
        let mut changed = false;
        let Some(servers) = json.get_mut("servers").and_then(|servers| servers.as_object_mut())
        else {
            return Ok(());
        };

        for (server_name, entry) in servers.iter_mut() {
            let Some(spec) = builtin_spec_for_server_name(server_name) else {
                continue;
            };
            if let Some(env) = entry.get_mut("env").and_then(|env| env.as_object_mut()) {
                if let Some(value) = env
                    .get(spec.key)
                    .and_then(|v| v.as_str())
                    .filter(|v| !v.trim().is_empty())
                    .map(ToOwned::to_owned)
                {
                    self.store_migrated_secret(spec, &value, result)?;
                    env.insert(
                        spec.key.to_string(),
                        serde_json::Value::String(mcp_secret_placeholder(spec.key)),
                    );
                    changed = true;
                }
            }
            if let Some(headers) = entry
                .get_mut("headers")
                .and_then(|headers| headers.as_object_mut())
            {
                if let Some(auth) = headers
                    .get("Authorization")
                    .and_then(|v| v.as_str())
                    .filter(|v| !v.trim().is_empty())
                    .map(ToOwned::to_owned)
                {
                    if let Some(secret) = auth.strip_prefix("Bearer ").filter(|v| !v.is_empty()) {
                        self.store_migrated_secret(spec, secret, result)?;
                        headers.insert(
                            "Authorization".to_string(),
                            serde_json::Value::String(format!(
                                "Bearer {}",
                                mcp_secret_placeholder(spec.key)
                            )),
                        );
                        changed = true;
                    }
                }
            }
        }

        if changed {
            write_json_pretty(path, &json)?;
        }
        Ok(())
    }

    fn store_migrated_secret(
        &self,
        spec: &BuiltinMcpSecretSpec,
        value: &str,
        result: &mut McpSecretMigrationResult,
    ) -> Result<(), String> {
        let reference = mcp_secret_reference(spec.tool_id, spec.target, spec.key);
        let env_value = match self.credential_store.get(&reference) {
            Ok(Some(existing)) if !existing.trim().is_empty() => {
                result.skipped_count += 1;
                result.messages.push(format!(
                    "MCP 工具 '{}' 的密钥 {} 已存在，已跳过覆盖并清理旧明文",
                    spec.tool_id, spec.key
                ));
                existing
            }
            Ok(_) => {
                self.credential_store
                    .set(&reference, value)
                    .map_err(|e| {
                        result.failed_count += 1;
                        mcp_secret_store_error(spec.tool_id, spec.key, e)
                    })?;
                result.migrated_count += 1;
                result.messages.push(format!(
                    "MCP 工具 '{}' 的密钥 {} 已迁移到系统凭据存储",
                    spec.tool_id, spec.key
                ));
                value.to_string()
            }
            Err(e) => {
                result.failed_count += 1;
                return Err(mcp_secret_store_error(spec.tool_id, spec.key, e));
            }
        };
        std::env::set_var(mcp_secret_env_var(spec.key), env_value);
        Ok(())
    }

    /// 卸载工具：从 installed.json + mcp.json 中移除
    pub fn uninstall(&self, tool_id: &str) -> Result<(), String> {
        // 删该工具在 keyring 的 secret(防孤儿;此时 manifest 未删、仍可读声明)。
        // 删不掉不阻断卸载;内置工具重装时 inject_builtin_shared_secrets 会重新注入。
        if let Some(manifest) = self.load_manifest(tool_id) {
            for (target, key) in manifest_secret_targets(&manifest) {
                let reference = mcp_secret_reference(tool_id, &target, &key);
                let _ = self.credential_store.delete(&reference);
            }
        }
        // 更新 installed.json
        let mut installed = self.installed_ids();
        installed.retain(|id| id != tool_id);
        self.save_installed(&installed)?;

        // 更新 mcp.json
        self.remove_from_mcp_json(tool_id)?;

        Ok(())
    }

    /// manifest 声明的配套技能 id(装该 MCP 时一并装、卸时一并删)。
    /// uninstall 不删 manifest 文件,故卸载后仍可读到。
    pub fn companion_skills(&self, tool_id: &str) -> Vec<String> {
        self.load_manifest(tool_id)
            .map(|m| m.companion_skills)
            .unwrap_or_default()
    }

    /// 根据已安装工具生成 instructions 路由规则段 + 工具表条目
    pub fn build_instructions_fragment(&self) -> String {
        let installed = self.installed_ids();
        if installed.is_empty() {
            return String::new();
        }

        let mut tool_table_lines = Vec::new();
        let mut routing_lines = Vec::new();

        for tool_id in &installed {
            if let Some(manifest) = self.load_manifest(tool_id) {
                for entry in &manifest.tool_table_entries {
                    tool_table_lines.push(entry.clone());
                }
                for rule in &manifest.routing_rules {
                    routing_lines.push(format!("- {rule}"));
                }
            }
        }

        let mut fragment = String::new();

        // 工具表条目
        if !tool_table_lines.is_empty() {
            for line in &tool_table_lines {
                fragment.push_str(line);
                fragment.push('\n');
            }
        }

        // 路由规则
        if !routing_lines.is_empty() {
            fragment.push_str("\n### 工具路由(优先用专用工具,不要用 web_search 替代)\n\n");
            for line in &routing_lines {
                fragment.push_str(line);
                fragment.push('\n');
            }
        }

        fragment
    }

    // --- internal ---

    fn load_manifest(&self, tool_id: &str) -> Option<ToolManifest> {
        let path = self.servers_dir.join(tool_id).join("manifest.json");
        let content = std::fs::read_to_string(&path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// pinvou3 工具开关:把"连接器 id 列表"映射成"模型可见工具全名"
    /// (`mcp_{server}_{tool}`,小写 —— 引擎 `command_denies_tool` 按小写精确匹配)。
    /// 关一个连接器要把它名下所有工具都列出来。
    pub fn model_tool_names(&self, connector_ids: &[String]) -> Vec<String> {
        let mut names = Vec::new();
        for cid in connector_ids {
            if let Some(m) = self.load_manifest(cid) {
                for server in &m.servers {
                    names.push(format!("mcp_{}_*", server.name).to_ascii_lowercase());
                }
                for t in &m.mcp_tools {
                    // manifest 的 mcp_tools 不统一:部分已是全名(mcp_xxx_yyy),部分是裸工具名。
                    // 已带 `mcp_` 前缀的原样用,否则补 `mcp_{id}_` —— 与引擎 mcp_{server}_{tool} 对齐。
                    let name = if t.starts_with("mcp_") {
                        t.clone()
                    } else {
                        format!("mcp_{}_{}", m.id, t)
                    };
                    names.push(name.to_ascii_lowercase());
                }
            }
        }
        names
    }

    fn save_installed(&self, ids: &[String]) -> Result<(), String> {
        let dir = self.installed_file.parent().unwrap();
        std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败: {e}"))?;
        let json = serde_json::to_string_pretty(ids).map_err(|e| e.to_string())?;
        std::fs::write(&self.installed_file, json).map_err(|e| format!("写入失败: {e}"))
    }

    fn backup_corrupt_installed(&self, content: &str) {
        let Some(parent) = self.installed_file.parent() else {
            return;
        };
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let backup = parent.join(format!("installed.json.corrupt.{ts}"));
        if let Err(e) = std::fs::write(&backup, content) {
            eprintln!(
                "[marketplace] failed to backup corrupt installed.json to {}: {e}",
                backup.display()
            );
        }
    }

    fn recover_installed_ids_from_mcp(&self) -> Vec<String> {
        let content = match std::fs::read_to_string(paths::mcp_config_path()) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let Ok(mcp) = serde_json::from_str::<serde_json::Value>(&content) else {
            return Vec::new();
        };
        let Some(servers) = mcp.get("servers").and_then(|s| s.as_object()) else {
            return Vec::new();
        };
        let mut recovered = Vec::new();
        for manifest in self.available_tools() {
            let registered = if manifest.servers.is_empty() {
                servers.contains_key(&manifest.id)
            } else {
                manifest
                    .servers
                    .iter()
                    .any(|server| servers.contains_key(&server.name))
            };
            if registered && !recovered.contains(&manifest.id) {
                recovered.push(manifest.id);
            }
        }
        recovered
    }

    fn add_to_mcp_json(
        &self,
        manifest: &ToolManifest,
        user_config: &std::collections::HashMap<String, String>,
    ) -> Result<(), String> {
        let mcp_path = paths::mcp_config_path();
        let mut mcp: serde_json::Value = if mcp_path.is_file() {
            let content =
                std::fs::read_to_string(&mcp_path).map_err(|e| format!("读取 mcp.json: {e}"))?;
            serde_json::from_str(&content).unwrap_or_else(|_| default_mcp_json())
        } else {
            default_mcp_json()
        };

        let servers = mcp
            .get_mut("servers")
            .and_then(|s| s.as_object_mut())
            .ok_or("mcp.json 格式错误")?;

        if !manifest.servers.is_empty() {
            // ── 远程工具：遍历 manifest.servers[]，写 url/headers ──
            for server in &manifest.servers {
                let mut headers = serde_json::Map::new();

                // 1. config_fields 中 target="bearer" 的字段（用户填入）
                for field in &manifest.config_fields {
                    if field.target == "bearer" {
                        if let Some(val) = user_config.get(&field.key) {
                            if field.secret {
                                let placeholder = self.resolve_secret_placeholder(
                                    &manifest.id,
                                    "header",
                                    &field.key,
                                    user_config,
                                    &manifest.env,
                                )?;
                                headers.insert(
                                    "Authorization".to_string(),
                                    serde_json::Value::String(format!("Bearer {}", placeholder)),
                                );
                            } else {
                                headers.insert(
                                    "Authorization".to_string(),
                                    serde_json::Value::String(format!("Bearer {}", val)),
                                );
                            }
                        }
                    }
                }

                // 2. manifest.secret_headers 声明的敏感 header（不落明文）
                for secret in &manifest.secret_headers {
                    let placeholder = self.resolve_secret_placeholder(
                        &manifest.id,
                        "header",
                        &secret.source_key,
                        user_config,
                        &manifest.env,
                    )?;
                    let value = if secret.scheme.trim().is_empty() {
                        placeholder
                    } else {
                        format!("{} {}", secret.scheme, placeholder)
                    };
                    headers.insert(secret.header.clone(), serde_json::Value::String(value));
                }

                // 3. 兼容旧 manifest.env 中以 _API_KEY 结尾的字段，迁移后只写占位。
                if headers.is_empty() {
                    for k in manifest.env.keys() {
                        if is_sensitive_key_name(k) {
                            let placeholder = self.resolve_secret_placeholder(
                                &manifest.id,
                                "header",
                                k,
                                user_config,
                                &manifest.env,
                            )?;
                            headers.insert(
                                "Authorization".to_string(),
                                serde_json::Value::String(format!("Bearer {}", placeholder)),
                            );
                            break;
                        }
                    }
                }

                let mut entry = serde_json::json!({ "url": server.url });
                if !headers.is_empty() {
                    entry["headers"] = serde_json::Value::Object(headers);
                }
                servers.insert(server.name.clone(), entry);
            }
        } else {
            // ── 本地工具：command/args/env ──
            let server_dir = self.servers_dir.join(&manifest.id);
            let args: Vec<String> = manifest
                .args
                .iter()
                .map(|a| {
                    if a == "server.py" || a.ends_with("/server.py") {
                        server_dir.join("server.py").to_string_lossy().to_string()
                    } else {
                        a.clone()
                    }
                })
                .collect();

            let mut env = manifest
                .env
                .iter()
                .filter(|(k, _)| !is_sensitive_key_name(k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<std::collections::HashMap<_, _>>();
            for field in &manifest.config_fields {
                if field.target == "env" {
                    if let Some(val) = user_config.get(&field.key) {
                        if field.secret || is_sensitive_key_name(&field.key) {
                            let placeholder = self.resolve_secret_placeholder(
                                &manifest.id,
                                "env",
                                &field.key,
                                user_config,
                                &manifest.env,
                            )?;
                            env.insert(field.key.clone(), placeholder);
                        } else {
                            env.insert(field.key.clone(), val.clone());
                        }
                    }
                }
            }
            for secret in &manifest.secret_env {
                let placeholder = self.resolve_secret_placeholder(
                    &manifest.id,
                    "env",
                    &secret.key,
                    user_config,
                    &manifest.env,
                )?;
                env.insert(secret.key.clone(), placeholder);
            }
            for key in manifest.env.keys().filter(|k| is_sensitive_key_name(k)) {
                if !env.contains_key(key) {
                    let placeholder = self.resolve_secret_placeholder(
                        &manifest.id,
                        "env",
                        key,
                        user_config,
                        &manifest.env,
                    )?;
                    env.insert(key.clone(), placeholder);
                }
            }

            // python 工具:Windows 用内置 pythonw(无窗口 + 自带依赖),其他平台系统 python3。
            let command = if manifest.command == "python" || manifest.command == "python3" {
                paths::python_command()
            } else {
                manifest.command.clone()
            };
            let mut entry = serde_json::json!({
                "command": command,
                "args": args,
            });
            if !env.is_empty() {
                entry["env"] = serde_json::to_value(&env).unwrap_or_default();
            }

            servers.insert(manifest.id.clone(), entry);
        }

        write_json_pretty(&mcp_path, &mcp)
    }

    fn remove_from_mcp_json(&self, tool_id: &str) -> Result<(), String> {
        let mcp_path = paths::mcp_config_path();
        if !mcp_path.is_file() {
            return Ok(());
        }
        let content =
            std::fs::read_to_string(&mcp_path).map_err(|e| format!("读取 mcp.json: {e}"))?;
        let mut mcp: serde_json::Value =
            serde_json::from_str(&content).unwrap_or_else(|_| default_mcp_json());

        if let Some(servers) = mcp.get_mut("servers").and_then(|s| s.as_object_mut()) {
            // 先尝试加载 manifest 看是否有 servers 字段（远程工具有多条目）
            if let Some(manifest) = self.load_manifest(tool_id) {
                if !manifest.servers.is_empty() {
                    for server in &manifest.servers {
                        servers.remove(&server.name);
                    }
                } else {
                    servers.remove(tool_id);
                }
            } else {
                servers.remove(tool_id);
            }
        }

        let json = serde_json::to_string_pretty(&mcp).map_err(|e| e.to_string())?;
        std::fs::write(&mcp_path, json).map_err(|e| format!("写入 mcp.json: {e}"))
    }
}

fn default_mcp_json() -> serde_json::Value {
    serde_json::json!({"servers": {}})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::paths::tests::ENV_LOCK;
    use crate::credential_store::{CredentialStore, MemoryCredentialStore};

    #[test]
    fn manifest_secret_targets_dedups_and_maps_bearer_to_header() {
        // 同一 key 在 secret_env/secret_headers 与 config_fields 重复声明 → 去重一次;
        // config_fields 的 "bearer" 落成 keyring target "header"(与 install 对齐)。
        let manifest: ToolManifest = serde_json::from_str(
            r#"{
            "id":"t","name":"T","description":"","version":"1","icon":"","category":"",
            "mcp_tools":[],"command":"","args":[],
            "secret_env":[{"key":"AMAP_KEY","provider":"amap","required":true}],
            "secret_headers":[{"header":"Authorization","scheme":"Bearer","source_key":"QCC_API_KEY","provider":"qcc","required":true}],
            "config_fields":[
                {"key":"AMAP_KEY","label":"","required":false,"target":"env","secret":true},
                {"key":"QCC_API_KEY","label":"","required":false,"target":"bearer","secret":true}
            ]
        }"#,
        )
        .unwrap();
        let targets = manifest_secret_targets(&manifest);
        assert_eq!(targets.len(), 2, "AMAP/QCC 各去重一次");
        assert!(targets.contains(&("env".to_string(), "AMAP_KEY".to_string())));
        assert!(targets.contains(&("header".to_string(), "QCC_API_KEY".to_string())));
    }

    /// 把 PINVOU3_HOME 指到一个干净临时目录跑闭包,跑完恢复并清理。
    /// 借 paths 的 ENV_LOCK 跟其它 mutate PINVOU3_HOME 的测试串行,避免互相覆盖。
    fn with_temp_home<F: FnOnce()>(f: F) {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let prev_amap = std::env::var("PINVOU3_MCP_SECRET_AMAP_KEY").ok();
        let prev_iwencai = std::env::var("PINVOU3_MCP_SECRET_IWENCAI_API_KEY").ok();
        let prev_qcc = std::env::var("PINVOU3_MCP_SECRET_QCC_API_KEY").ok();
        let dir = std::env::temp_dir().join(format!("pinvou3-mkt-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("PINVOU3_HOME", &dir);
        f();
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        for (key, value) in [
            ("PINVOU3_MCP_SECRET_AMAP_KEY", prev_amap),
            ("PINVOU3_MCP_SECRET_IWENCAI_API_KEY", prev_iwencai),
            ("PINVOU3_MCP_SECRET_QCC_API_KEY", prev_qcc),
        ] {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn write_tool_manifest(tool_id: &str, manifest: &str) {
        let dir = crate::bridge::paths::bundle_mcp_servers_dir().join(tool_id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), manifest).unwrap();
    }

    fn read_mcp_json() -> serde_json::Value {
        let content = std::fs::read_to_string(crate::bridge::paths::mcp_config_path()).unwrap();
        serde_json::from_str(&content).unwrap()
    }

    fn secret_value(name: &str) -> String {
        format!("test-secret-{name}-value-123456")
    }

    /// 连接器 → 模型可见工具全名:裸名补 `mcp_{id}_` 前缀,已带 `mcp_` 的原样(不双前缀),
    /// 一律小写;不存在的连接器跳过。这正是当初打印映射时抓到"双前缀 bug"的那段逻辑。
    #[test]
    fn installed_ids_recovers_corrupt_file_from_mcp_json() {
        with_temp_home(|| {
            write_tool_manifest(
                "weather",
                r#"{
                    "id":"weather","name":"Weather","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":["get_weather"],"command":"python","args":["server.py"]
                }"#,
            );
            let mcp_path = crate::bridge::paths::mcp_config_path();
            std::fs::create_dir_all(mcp_path.parent().unwrap()).unwrap();
            std::fs::write(
                &mcp_path,
                r#"{"servers":{"weather":{"command":"python3","args":["server.py"]}}}"#,
            )
            .unwrap();
            let installed_path = crate::bridge::paths::pinvou3_home()
                .join("marketplace")
                .join("installed.json");
            std::fs::create_dir_all(installed_path.parent().unwrap()).unwrap();
            std::fs::write(&installed_path, "[\"weather\"").unwrap();

            let ids = MarketplaceManager::new().installed_ids();

            assert_eq!(ids, vec!["weather".to_string()]);
            let repaired = std::fs::read_to_string(&installed_path).unwrap();
            assert_eq!(
                serde_json::from_str::<Vec<String>>(&repaired).unwrap(),
                vec!["weather".to_string()]
            );
            let backups: Vec<_> = std::fs::read_dir(installed_path.parent().unwrap())
                .unwrap()
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with("installed.json.corrupt."))
                .collect();
            assert_eq!(backups.len(), 1);
        });
    }

    #[test]
    fn model_tool_names_prefix_dedup_and_lowercase() {
        with_temp_home(|| {
            let dir = crate::bridge::paths::bundle_mcp_servers_dir().join("demo");
            std::fs::create_dir_all(&dir).unwrap();
            let manifest = r#"{
                "id":"demo","name":"Demo","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":["bare_tool","mcp_demo_already","UPPER_Tool"],
                "command":"python","args":[]
            }"#;
            std::fs::write(dir.join("manifest.json"), manifest).unwrap();

            let mgr = MarketplaceManager::new();
            let names = mgr.model_tool_names(&["demo".to_string()]);
            assert_eq!(
                names,
                vec![
                    "mcp_demo_bare_tool".to_string(),  // 裸名 → 补前缀
                    "mcp_demo_already".to_string(),    // 已带 mcp_ → 原样,不变成 mcp_demo_mcp_demo_already
                    "mcp_demo_upper_tool".to_string(), // 小写化
                ]
            );
            // 没装/不存在的连接器 → 跳过(不报错,空)
            assert!(mgr.model_tool_names(&["nope".to_string()]).is_empty());
        });
    }

    /// 远程多 server 连接器可能没有静态 mcp_tools 列表(qcc 即如此)。禁用时必须按
    /// server 名生成前缀规则,否则底座仍会暴露该连接器动态发现出来的全部工具。
    #[test]
    fn model_tool_names_generates_prefix_rules_for_remote_servers() {
        with_temp_home(|| {
            let dir = crate::bridge::paths::bundle_mcp_servers_dir().join("qcc");
            std::fs::create_dir_all(&dir).unwrap();
            let manifest = r#"{
                "id":"qcc","name":"企查查","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],
                "command":"python","args":[],
                "servers":[
                    {"name":"qcc-company","url":"https://agent.qcc.com/mcp/company/stream"},
                    {"name":"qcc-risk","url":"https://agent.qcc.com/mcp/risk/stream"},
                    {"name":"qcc-ipr","url":"https://agent.qcc.com/mcp/ipr/stream"},
                    {"name":"qcc-operation","url":"https://agent.qcc.com/mcp/operation/stream"}
                ]
            }"#;
            std::fs::write(dir.join("manifest.json"), manifest).unwrap();

            let mgr = MarketplaceManager::new();
            // 对齐真实 qcc manifest 的 4 个远程 server —— 每个生成一条前缀规则。
            assert_eq!(
                mgr.model_tool_names(&["qcc".to_string()]),
                vec![
                    "mcp_qcc-company_*".to_string(),
                    "mcp_qcc-risk_*".to_string(),
                    "mcp_qcc-ipr_*".to_string(),
                    "mcp_qcc-operation_*".to_string(),
                ]
            );
        });
    }

    /// 全局禁用列表落盘往返:存→读一致;清空→读空;没文件→读空。
    #[test]
    fn disabled_connectors_persist_roundtrip() {
        with_temp_home(|| {
            assert!(load_disabled_connectors().is_empty()); // 无文件 → 空
            save_disabled_connectors(&["weather".to_string(), "pptx".to_string()]);
            assert_eq!(
                load_disabled_connectors(),
                vec!["weather".to_string(), "pptx".to_string()]
            );
            save_disabled_connectors(&[]); // 全开回去
            assert!(load_disabled_connectors().is_empty());
        });
    }

    /// 连接器禁用联动技能:禁用声明了 companion_skills 的连接器 → 该技能进底座停用集;
    /// 开回来 → 移出。守"公文 MCP 关掉 → government-writing 从 ## Skills 隐藏"这条链路。
    #[test]
    fn disabling_connector_hides_companion_skill() {
        with_temp_home(|| {
            write_tool_manifest(
                "gongwen",
                r#"{"id":"gongwen","name":"公文写作","description":"d","version":"1.0.0","icon":"file-text","category":"办公","mcp_tools":["mcp_gongwen_make_gongwen"],"command":"python","args":["server.py"],"companion_skills":["government-writing"]}"#,
            );

            // 禁用公文 MCP → 联动刷新 → 关联技能进底座停用集
            save_disabled_connectors(&["gongwen".to_string()]);
            crate::bridge::skill_marketplace::refresh_disabled_skills();
            assert!(
                deepseek_tui::skills::is_skill_disabled("government-writing"),
                "禁用公文 MCP 后关联技能应被停用"
            );

            // 开回来 → 移出停用集
            save_disabled_connectors(&[]);
            crate::bridge::skill_marketplace::refresh_disabled_skills();
            assert!(
                !deepseek_tui::skills::is_skill_disabled("government-writing"),
                "启用公文 MCP 后关联技能应恢复"
            );
        });
    }

    #[test]
    fn secret_manifest_parses_declarations_without_plain_secret_values() {
        let weather: ToolManifest = serde_json::from_str(
            r#"{
                "id":"weather","name":"Weather","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":["mcp_weather_get_weather"],"command":"python","args":["server.py"],
                "secret_env":[{"key":"AMAP_KEY","provider":"amap","required":true}]
            }"#,
        )
        .unwrap();
        assert!(weather.env.is_empty());
        assert_eq!(weather.secret_env[0].key, "AMAP_KEY");

        let qcc: ToolManifest = serde_json::from_str(
            r#"{
                "id":"qcc","name":"QCC","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"","args":[],
                "secret_headers":[{"header":"Authorization","scheme":"Bearer","source_key":"QCC_API_KEY","provider":"qcc","required":true}],
                "servers":[{"name":"qcc-company","url":"https://example.invalid/mcp"}]
            }"#,
        )
        .unwrap();
        assert!(qcc.env.is_empty());
        assert_eq!(qcc.secret_headers[0].source_key, "QCC_API_KEY");
    }

    #[test]
    fn install_local_secret_env_writes_placeholder_without_plain_secret() {
        with_temp_home(|| {
            write_tool_manifest(
                "weather",
                r#"{
                    "id":"weather","name":"Weather","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":["mcp_weather_get_weather"],"command":"python","args":["server.py"],
                    "secret_env":[{"key":"AMAP_KEY","provider":"amap","required":true}]
                }"#,
            );
            let store = MemoryCredentialStore::default();
            let secret = secret_value("amap");
            store
                .set(&mcp_secret_reference("weather", "env", "AMAP_KEY"), &secret)
                .unwrap();
            let mgr = MarketplaceManager::with_store(store);

            mgr.install("weather", &std::collections::HashMap::new())
                .unwrap();

            let mcp = read_mcp_json();
            let amap = mcp["servers"]["weather"]["env"]["AMAP_KEY"]
                .as_str()
                .unwrap();
            assert_eq!(amap, "${PINVOU3_MCP_SECRET_AMAP_KEY}");
            assert!(!mcp.to_string().contains(&secret));
        });
    }

    #[test]
    fn install_remote_secret_header_writes_placeholder_without_plain_secret() {
        with_temp_home(|| {
            write_tool_manifest(
                "qcc",
                r#"{
                    "id":"qcc","name":"QCC","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"","args":[],
                    "secret_headers":[{"header":"Authorization","scheme":"Bearer","source_key":"QCC_API_KEY","provider":"qcc","required":true}],
                    "servers":[{"name":"qcc-company","url":"https://example.invalid/mcp"}]
                }"#,
            );
            let store = MemoryCredentialStore::default();
            let secret = secret_value("qcc");
            store
                .set(&mcp_secret_reference("qcc", "header", "QCC_API_KEY"), &secret)
                .unwrap();
            let mgr = MarketplaceManager::with_store(store);

            mgr.install("qcc", &std::collections::HashMap::new()).unwrap();

            let mcp = read_mcp_json();
            let authorization = mcp["servers"]["qcc-company"]["headers"]["Authorization"]
                .as_str()
                .unwrap();
            assert_eq!(authorization, "Bearer ${PINVOU3_MCP_SECRET_QCC_API_KEY}");
            assert!(!mcp.to_string().contains(&secret));
        });
    }

    #[test]
    fn install_failure_does_not_leave_half_installed_state() {
        // #2 半安装回归:缺密钥导致 add_to_mcp_json 失败时,installed.json 不该记录该工具
        // (顺序修复=先写 mcp 成功、再 save_installed)。
        with_temp_home(|| {
            write_tool_manifest(
                "weather",
                r#"{
                    "id":"weather","name":"Weather","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":["mcp_weather_get_weather"],"command":"python","args":["server.py"],
                    "secret_env":[{"key":"AMAP_KEY","provider":"amap","required":true}]
                }"#,
            );
            let mgr = MarketplaceManager::with_store(MemoryCredentialStore::default());
            // 不提供 key + keyring 空 + 无内置共享 key(测试 option_env=None)→ install 必失败
            assert!(
                mgr.install("weather", &std::collections::HashMap::new())
                    .is_err(),
                "缺密钥应安装失败"
            );
            assert!(
                !mgr.installed_ids().contains(&"weather".to_string()),
                "失败时 installed.json 不该记录该工具(否则半安装)"
            );
        });
    }

    #[test]
    fn shared_secret_bounty_only_matches_exact_builtin_spec() {
        // 兜底范围收窄:自定义/上传工具声明同名 key 不该拿到内置共享额度。
        // (内置精确匹配的返回值取决于编译期 option_env,单测环境无 env → 也 None,
        //  故这里只断言「非精确匹配恒 None」这条安全属性。)
        assert!(
            builtin_shared_secret_value_for("evil-tool", "env", "AMAP_KEY").is_none(),
            "非内置 tool_id 声明 AMAP_KEY 不该匹配内置额度"
        );
        assert!(
            builtin_shared_secret_value_for("weather", "header", "AMAP_KEY").is_none(),
            "target 不符(内置 weather 是 env)不该匹配"
        );
        assert!(
            builtin_shared_secret_value_for("weather", "env", "OTHER_KEY").is_none(),
            "key 不符不该匹配"
        );
    }

    #[test]
    fn migrate_legacy_manifest_env_moves_secret_to_store_and_removes_plaintext() {
        with_temp_home(|| {
            let secret = secret_value("legacy-amap");
            write_tool_manifest(
                "weather",
                &format!(
                    r#"{{
                        "id":"weather","name":"Weather","description":"d","version":"1","icon":"x","category":"c",
                        "mcp_tools":["mcp_weather_get_weather"],"command":"python","args":["server.py"],
                        "env":{{"AMAP_KEY":"{secret}","SAFE_VALUE":"kept"}}
                    }}"#
                ),
            );
            let store = MemoryCredentialStore::default();
            let mgr = MarketplaceManager::with_store(store.clone());

            let result = mgr.migrate_mcp_plaintext_secrets().unwrap();

            assert_eq!(result.migrated_count, 1);
            let stored = store
                .get(&mcp_secret_reference("weather", "env", "AMAP_KEY"))
                .unwrap();
            assert_eq!(stored.as_deref(), Some(secret.as_str()));
            let content = std::fs::read_to_string(
                crate::bridge::paths::bundle_mcp_servers_dir()
                    .join("weather")
                    .join("manifest.json"),
            )
            .unwrap();
            assert!(!content.contains(&secret));
            assert!(content.contains("SAFE_VALUE"));
        });
    }

    #[test]
    fn migrate_legacy_qcc_bearer_header_writes_placeholder_and_stores_secret() {
        with_temp_home(|| {
            let secret = secret_value("legacy-qcc");
            let mcp_path = crate::bridge::paths::mcp_config_path();
            std::fs::create_dir_all(mcp_path.parent().unwrap()).unwrap();
            std::fs::write(
                &mcp_path,
                format!(
                    r#"{{
                        "servers": {{
                            "qcc-company": {{
                                "url": "https://example.invalid/mcp",
                                "headers": {{"Authorization": "Bearer {secret}"}}
                            }}
                        }}
                    }}"#
                ),
            )
            .unwrap();
            let store = MemoryCredentialStore::default();
            let mgr = MarketplaceManager::with_store(store.clone());

            let result = mgr.migrate_mcp_plaintext_secrets().unwrap();

            assert_eq!(result.migrated_count, 1);
            let stored = store
                .get(&mcp_secret_reference("qcc", "header", "QCC_API_KEY"))
                .unwrap();
            assert_eq!(stored.as_deref(), Some(secret.as_str()));
            let content = std::fs::read_to_string(&mcp_path).unwrap();
            assert!(!content.contains(&secret));
            assert!(content.contains("Bearer ${PINVOU3_MCP_SECRET_QCC_API_KEY}"));
        });
    }

    #[test]
    fn migration_does_not_overwrite_existing_credential_but_cleans_file() {
        with_temp_home(|| {
            let old_secret = secret_value("old-qcc");
            let kept_secret = secret_value("kept-qcc");
            let mcp_path = crate::bridge::paths::mcp_config_path();
            std::fs::create_dir_all(mcp_path.parent().unwrap()).unwrap();
            std::fs::write(
                &mcp_path,
                format!(
                    r#"{{
                        "servers": {{
                            "qcc-company": {{
                                "url": "https://example.invalid/mcp",
                                "headers": {{"Authorization": "Bearer {old_secret}"}}
                            }}
                        }}
                    }}"#
                ),
            )
            .unwrap();
            let store = MemoryCredentialStore::default();
            store
                .set(
                    &mcp_secret_reference("qcc", "header", "QCC_API_KEY"),
                    &kept_secret,
                )
                .unwrap();
            let mgr = MarketplaceManager::with_store(store.clone());

            let result = mgr.migrate_mcp_plaintext_secrets().unwrap();

            assert_eq!(result.skipped_count, 1);
            let stored = store
                .get(&mcp_secret_reference("qcc", "header", "QCC_API_KEY"))
                .unwrap();
            assert_eq!(stored.as_deref(), Some(kept_secret.as_str()));
            let content = std::fs::read_to_string(&mcp_path).unwrap();
            assert!(!content.contains(&old_secret));
            assert!(!content.contains(&kept_secret));
            assert!(content.contains("Bearer ${PINVOU3_MCP_SECRET_QCC_API_KEY}"));
        });
    }

    #[test]
    fn install_missing_required_secret_returns_recoverable_redacted_error() {
        with_temp_home(|| {
            write_tool_manifest(
                "iwencai",
                r#"{
                    "id":"iwencai","name":"Iwencai","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":["mcp_iwencai_query"],"command":"python","args":["server.py"],
                    "secret_env":[{"key":"IWENCAI_API_KEY","provider":"iwencai","required":true}]
                }"#,
            );
            let mgr = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let err = mgr
                .install("iwencai", &std::collections::HashMap::new())
                .unwrap_err();

            assert!(err.contains("iwencai"));
            assert!(err.contains("IWENCAI_API_KEY"));
            assert!(!err.contains("test-secret"));
        });
    }
}

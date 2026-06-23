//! 工具市场管理器 — 管理 MCP 工具的安装/卸载/状态查询。
//!
//! 每个工具是一个 MCP server，元数据定义在 `manifest.json`。
//! 安装状态持久化在 `~/.pinvou3/marketplace/installed.json`。
//! 安装/卸载时同步修改 `~/.pinvou3/bundle/mcp.json`。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::paths;

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
    pub config_fields: Vec<ConfigField>,
    #[serde(default)]
    pub routing_rules: Vec<String>,
    #[serde(default)]
    pub tool_table_entries: Vec<String>,
    #[serde(default)]
    pub pip_dependencies: Vec<String>,
    #[serde(default)]
    pub servers: Vec<RemoteServer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteServer {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigField {
    pub key: String,
    pub label: String,
    pub required: bool,
    /// "env" = 写入 mcp.json env 字段, "bearer" = 写入 headers Authorization
    #[serde(default = "default_target")]
    pub target: String,
}

fn default_target() -> String {
    "env".to_string()
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
}

// ---------------------------------------------------------------------------
// MarketplaceManager
// ---------------------------------------------------------------------------

pub struct MarketplaceManager {
    /// bundle 解包后的 MCP servers 目录 (~/.pinvou3/bundle/mcp-servers/)
    servers_dir: PathBuf,
    /// 已安装工具列表文件 (~/.pinvou3/marketplace/installed.json)
    installed_file: PathBuf,
}

impl MarketplaceManager {
    pub fn new() -> Self {
        let servers_dir = paths::bundle_mcp_servers_dir();
        let installed_file = paths::pinvou3_home().join("marketplace").join("installed.json");
        Self {
            servers_dir,
            installed_file,
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
        serde_json::from_str(&content).unwrap_or_default()
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
        let manifest = self
            .load_manifest(tool_id)
            .ok_or_else(|| format!("工具 '{tool_id}' 不存在"))?;

        // 先装 Python 依赖（跨平台 pip）；失败就不注册，让用户可重试。零依赖工具会直接跳过。
        self.pip_install_deps(&manifest)?;

        // 更新 installed.json
        let mut installed = self.installed_ids();
        if !installed.contains(&tool_id.to_string()) {
            installed.push(tool_id.to_string());
        }
        self.save_installed(&installed)?;

        // 更新 mcp.json
        self.add_to_mcp_json(&manifest, user_config)?;

        Ok(())
    }

    /// 装 `manifest.pip_dependencies` 里的 Python 依赖（跨平台）。
    /// 用 `python -m pip install`（保证装进跑 MCP server 的同一个 python，不裸 `pip`）；
    /// 先试 `--user`（免管理员），venv 下 `--user` 不可用则去掉重试；Windows 抑制黑窗口闪现。
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

        let run = |user: bool| -> std::io::Result<std::process::Output> {
            let mut cmd = std::process::Command::new(python_cmd);
            cmd.args(["-m", "pip", "install", "--disable-pip-version-check", "--no-input"]);
            if user {
                cmd.arg("--user");
            }
            cmd.args(&manifest.pip_dependencies);
            cmd.output()
        };

        let out = run(true).map_err(|e| {
            format!("无法运行 {python_cmd}（请确认已安装 Python 且在 PATH 中）：{e}")
        })?;
        if out.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        // venv 里 `--user` 不可用 → 去掉重试
        if stderr.contains("--user") || stderr.contains("user site-packages") {
            let out2 = run(false).map_err(|e| format!("无法运行 {python_cmd}：{e}"))?;
            if out2.status.success() {
                return Ok(());
            }
            let s2 = String::from_utf8_lossy(&out2.stderr);
            return Err(format!(
                "依赖安装失败（pip）：{}",
                s2.trim().lines().last().unwrap_or("未知错误")
            ));
        }
        Err(format!(
            "依赖安装失败（pip）：{}",
            stderr.trim().lines().last().unwrap_or("未知错误")
        ))
    }

    /// 卸载工具：从 installed.json + mcp.json 中移除
    pub fn uninstall(&self, tool_id: &str) -> Result<(), String> {
        // 更新 installed.json
        let mut installed = self.installed_ids();
        installed.retain(|id| id != tool_id);
        self.save_installed(&installed)?;

        // 更新 mcp.json
        self.remove_from_mcp_json(tool_id)?;

        Ok(())
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

    fn save_installed(&self, ids: &[String]) -> Result<(), String> {
        let dir = self.installed_file.parent().unwrap();
        std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败: {e}"))?;
        let json = serde_json::to_string_pretty(ids).map_err(|e| e.to_string())?;
        std::fs::write(&self.installed_file, json).map_err(|e| format!("写入失败: {e}"))
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
                            headers.insert(
                                "Authorization".to_string(),
                                serde_json::Value::String(format!("Bearer {}", val)),
                            );
                        }
                    }
                }

                // 2. manifest.env 中以 _API_KEY 结尾的字段（内置 Key 阶段）
                if headers.is_empty() {
                    for (k, v) in &manifest.env {
                        if k.ends_with("_API_KEY") && !v.is_empty() {
                            headers.insert(
                                "Authorization".to_string(),
                                serde_json::Value::String(format!("Bearer {}", v)),
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

            let mut env = manifest.env.clone();
            for field in &manifest.config_fields {
                if field.target == "env" {
                    if let Some(val) = user_config.get(&field.key) {
                        env.insert(field.key.clone(), val.clone());
                    }
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

        let json = serde_json::to_string_pretty(&mcp).map_err(|e| e.to_string())?;
        std::fs::write(&mcp_path, json).map_err(|e| format!("写入 mcp.json: {e}"))
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

//! Connector(连接器)的注册/注销:把工具写进 `mcp.json`(`servers` 表)或从中移除,
//! 以及装 Python 依赖等"让 connector 跑起来"的前置准备。
//!
//! 原 god-method `add_to_mcp_json`(198 行)在此拆为 remote/local 两条路径:
//! facade `add_to_mcp_json` 负责加载 mcp.json + 取出 `servers` map,再按 manifest
//! 是否含远程 server 委托给 `add_remote_to_mcp_json` / `add_local_to_mcp_json`。

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use crate::platform::paths;

use super::bundle;
use super::python_dependencies;
use super::secrets::{is_sensitive_key_name, set_remote_secret_header};
use super::types::ToolManifest;
use super::MarketplaceManager;

/// mcp.json 读-改-写的进程内串行化（四轮评审 M-8）：add/remove 是裸读-改-写，
/// 并发安装/卸载会交错丢更新；与 store.rs 的 BUNDLES_FILE_LOCK 同一范式。
static MCP_JSON_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
static NEXT_PIP_INSTALL_RESULT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

#[cfg(test)]
pub(super) fn set_next_pip_install_result_for_test(result: u8) {
    NEXT_PIP_INSTALL_RESULT.store(result, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
pub(super) fn take_pending_pip_install_result_for_test() -> u8 {
    NEXT_PIP_INSTALL_RESULT.swap(0, std::sync::atomic::Ordering::SeqCst)
}

pub(super) fn mcp_json_lock() -> MutexGuard<'static, ()> {
    MCP_JSON_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 把 JSON 值以 pretty 形式写盘(迁移与 connector 注册共用)。
/// 写前创建父目录：全新 PINVOU3_HOME 下 `bundle/` 尚不存在，直接写会 ENOENT。
/// tmp + rename 原子落盘（底座 `write_atomic`，与 store.rs 同一做法）——安装
/// 中途崩溃不得留下半写的 mcp.json（幽灵 server，四轮评审 M-8）。
pub(super) fn write_json_pretty(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    deepseek_tui::utils::write_atomic(path, json.as_bytes())
        .map_err(|e| format!("写入 {} 失败: {e}", path.display()))
}

fn default_mcp_json() -> serde_json::Value {
    serde_json::json!({"servers": {}})
}

impl<S: crate::platform::credential_store::CredentialStore> MarketplaceManager<S> {
    /// 装 `manifest.pip_dependencies` 里的 Python 依赖（跨平台）。
    /// 用 `python -m pip install`（保证装进跑 MCP server 的同一个 python，不裸 `pip`）。
    /// ① 先预检依赖是否已可用（系统已装/此前装过）→ 命中即跳过，不跑 pip；
    /// ② 否则按序兜底：`--user` → `--user --break-system-packages`（PEP 668）→ `--break-system-packages`，任一成功即 Ok。
    /// 零依赖工具（pip_dependencies 为空）直接返回 Ok，不影响 weather/obsidian 等。
    pub(super) fn pip_install_deps(&self, manifest: &ToolManifest) -> Result<(), String> {
        if manifest.pip_dependencies.is_empty() {
            return Ok(());
        }
        #[cfg(test)]
        match NEXT_PIP_INSTALL_RESULT.swap(0, std::sync::atomic::Ordering::SeqCst) {
            1 => return Err("test-injected pip dependency failure".to_string()),
            2 => return Ok(()),
            _ => {}
        }
        // The bundled Windows interpreter is not an implicit dependency source. Every non-empty
        // dependency set must have a verified platform wheel lock; silently accepting ambient or
        // preinstalled packages would make the manifest incomplete and non-reproducible.
        if crate::platform::capabilities::is_windows() {
            return Err(format!(
                "工具 '{}' 缺少 Windows Python 依赖锁，无法安全安装",
                manifest.id
            ));
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
            cmd.args([
                "-m",
                "pip",
                "install",
                "--disable-pip-version-check",
                "--no-input",
            ]);
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

    /// 把 manifest 注册进 `mcp.json`(`servers` 表)。
    ///
    /// facade:加载 mcp.json → 取 `servers` map → 按 manifest 是否含远程 server
    /// 委托给 `add_remote_to_mcp_json`(远程:url/headers/oauth)或
    /// `add_local_to_mcp_json`(本地:command/args/env)→ 落盘。
    pub(super) fn add_to_mcp_json(
        &self,
        manifest: &ToolManifest,
        user_config: &HashMap<String, String>,
        server_dir: &std::path::Path,
        python_environment: Option<&python_dependencies::InstalledPythonEnvironment>,
    ) -> Result<(), String> {
        let _guard = mcp_json_lock();
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
            self.add_remote_to_mcp_json(manifest, user_config, servers)?;
        } else {
            self.add_local_to_mcp_json(
                manifest,
                user_config,
                server_dir,
                python_environment,
                servers,
            )?;
        }

        write_json_pretty(&mcp_path, &mcp)
    }

    /// 远程工具路径:遍历 manifest.servers[],写 url/headers/oauth/env_headers/bearer。
    /// 密钥不落明文,只写 `${ENV}` 占位 + 进程环境变量(底座不展开 headers 字面量)。
    fn add_remote_to_mcp_json(
        &self,
        manifest: &ToolManifest,
        user_config: &HashMap<String, String>,
        servers: &mut serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        for server in &manifest.servers {
            let mut headers = serde_json::Map::new();
            let mut env_headers = serde_json::Map::new();
            let mut bearer_token_env_var = None;

            // 1. config_fields 中 target="bearer" 的字段（用户填入）。
            //    同一 source_key 已由 secret_headers 声明 Authorization 时跳过:两个通道
            //    都写 Authorization,scheme 分歧时(bearer 前缀 vs 原始值)会同时落
            //    bearer_token_env_var 与 env_headers,产生自相矛盾的条目。secret_headers
            //    是权威声明;此处仍先 resolve_secret_placeholder,保证用户当次输入落库。
            let secret_header_auth_keys: std::collections::HashSet<&str> = manifest
                .secret_headers
                .iter()
                .filter(|s| s.header.eq_ignore_ascii_case("authorization"))
                .map(|s| s.source_key.as_str())
                .collect();
            for field in &manifest.config_fields {
                if field.target == "bearer" {
                    if let Some(val) = user_config.get(&field.key) {
                        if field.secret {
                            self.resolve_secret_placeholder(
                                &manifest.id,
                                bundle::keyring_target(bundle::CredentialTarget::Bearer),
                                &field.key,
                                user_config,
                                &manifest.env,
                            )?;
                            if secret_header_auth_keys.contains(field.key.as_str()) {
                                continue;
                            }
                            set_remote_secret_header(
                                &mut env_headers,
                                &mut bearer_token_env_var,
                                "Authorization",
                                "Bearer",
                                &field.key,
                            )?;
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
                self.resolve_secret_placeholder(
                    &manifest.id,
                    bundle::keyring_target(bundle::CredentialTarget::Bearer),
                    &secret.source_key,
                    user_config,
                    &manifest.env,
                )?;
                set_remote_secret_header(
                    &mut env_headers,
                    &mut bearer_token_env_var,
                    &secret.header,
                    &secret.scheme,
                    &secret.source_key,
                )?;
            }

            // 3. 兼容旧 manifest.env 中以 _API_KEY 结尾的字段，迁移后只写占位。
            if headers.is_empty() {
                for k in manifest.env.keys() {
                    if is_sensitive_key_name(k) {
                        let placeholder = self.resolve_secret_placeholder(
                            &manifest.id,
                            bundle::keyring_target(bundle::CredentialTarget::Bearer),
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
            if !server.scopes.is_empty() {
                entry["scopes"] = serde_json::to_value(&server.scopes).unwrap_or_default();
            }
            if let Some(oauth) = &server.oauth {
                entry["oauth"] = serde_json::to_value(oauth).unwrap_or_default();
            }
            if let Some(resource) = &server.oauth_resource {
                if !resource.trim().is_empty() {
                    entry["oauth_resource"] = serde_json::Value::String(resource.clone());
                }
            }
            if !headers.is_empty() {
                entry["headers"] = serde_json::Value::Object(headers);
            }
            if !env_headers.is_empty() {
                entry["env_headers"] = serde_json::Value::Object(env_headers);
            }
            if let Some(env_var) = bearer_token_env_var {
                entry["bearer_token_env_var"] = serde_json::Value::String(env_var);
            }
            servers.insert(server.name.clone(), entry);
        }
        Ok(())
    }

    fn local_server_args(manifest: &ToolManifest, server_dir: &Path) -> Vec<String> {
        manifest
            .args
            .iter()
            .map(|a| {
                if a == "server.py" || a.ends_with("/server.py") {
                    server_dir.join("server.py").to_string_lossy().to_string()
                } else {
                    a.clone()
                }
            })
            .collect()
    }

    fn managed_python_runtime_fields(
        manifest: &ToolManifest,
        server_dir: &Path,
        environment: &python_dependencies::InstalledPythonEnvironment,
    ) -> Result<(String, Vec<String>), String> {
        if manifest.command != "python" && manifest.command != "python3" {
            return Err(format!(
                "tool '{}' declares Python dependencies but does not use a Python command",
                manifest.id
            ));
        }
        let args = Self::local_server_args(manifest, server_dir);
        let Some(server_script) = args.first().cloned() else {
            return Err(format!(
                "tool '{}' has no Python server argument",
                manifest.id
            ));
        };
        if !Path::new(&server_script).is_file() {
            return Err(format!(
                "tool '{}' Python server does not exist: {}",
                manifest.id, server_script
            ));
        }
        let runner = paths::bundle_mcp_python_runner();
        if !runner.is_file() {
            return Err("Python MCP dependency runner is missing; restart and retry".to_string());
        }
        let mut wrapped_args = vec![
            "-I".to_string(),
            "-S".to_string(),
            "-B".to_string(),
            runner.to_string_lossy().into_owned(),
            environment.site_packages.to_string_lossy().into_owned(),
            server_script,
        ];
        wrapped_args.extend(args.into_iter().skip(1));
        Ok((environment.python_command.clone(), wrapped_args))
    }

    /// Repair only the managed launcher fields. Existing configuration, secret placeholders,
    /// enabled state, timeouts, and forward-compatible fields retain their JSON values.
    pub(super) fn patch_managed_python_runtime(
        &self,
        manifest: &ToolManifest,
        server_dir: &Path,
        environment: &python_dependencies::InstalledPythonEnvironment,
    ) -> Result<(), String> {
        let (command, args) =
            Self::managed_python_runtime_fields(manifest, server_dir, environment)?;
        let _guard = mcp_json_lock();
        let mcp_path = paths::mcp_config_path();
        let content = std::fs::read_to_string(&mcp_path)
            .map_err(|error| format!("read mcp.json: {error}"))?;
        let mut mcp: serde_json::Value =
            serde_json::from_str(&content).map_err(|error| format!("parse mcp.json: {error}"))?;
        let entry = mcp
            .get_mut("servers")
            .and_then(serde_json::Value::as_object_mut)
            .and_then(|servers| servers.get_mut(&manifest.id))
            .and_then(serde_json::Value::as_object_mut)
            .ok_or_else(|| format!("mcp.json has no local server entry for '{}'", manifest.id))?;
        let command = serde_json::Value::String(command);
        let args = serde_json::to_value(args).map_err(|error| error.to_string())?;
        if entry.get("command") == Some(&command) && entry.get("args") == Some(&args) {
            return Ok(());
        }
        entry.insert("command".to_string(), command);
        entry.insert("args".to_string(), args);
        write_json_pretty(&mcp_path, &mcp)
    }

    /// 本地工具路径:command/args/env。Python 工具用内置 python(Windows)或系统 python3。
    /// 敏感字段走 `${ENV}` 占位,非敏感原样写。
    fn add_local_to_mcp_json(
        &self,
        manifest: &ToolManifest,
        user_config: &HashMap<String, String>,
        server_dir: &std::path::Path,
        python_environment: Option<&python_dependencies::InstalledPythonEnvironment>,
        servers: &mut serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        let (command, args) = if let Some(environment) = python_environment {
            Self::managed_python_runtime_fields(manifest, server_dir, environment)?
        } else {
            let command = if manifest.command == "python" || manifest.command == "python3" {
                paths::python_command()
            } else {
                manifest.command.clone()
            };
            (command, Self::local_server_args(manifest, server_dir))
        };

        let mut env = manifest
            .env
            .iter()
            .filter(|(k, _)| !is_sensitive_key_name(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<HashMap<_, _>>();
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

        let mut entry = serde_json::json!({
            "command": command,
            "args": args,
        });
        if !env.is_empty() {
            entry["env"] = serde_json::to_value(&env).unwrap_or_default();
        }

        servers.insert(manifest.id.clone(), entry);
        Ok(())
    }

    pub(super) fn remove_from_mcp_json(&self, tool_id: &str) -> Result<(), String> {
        let _guard = mcp_json_lock();
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

        write_json_pretty(&mcp_path, &mcp)
    }
}

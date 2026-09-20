//! Connector(连接器)的注册/注销:把工具写进 `mcp.json`(`servers` 表)或从中移除,
//! 以及装 Python 依赖等"让 connector 跑起来"的前置准备。
//!
//! 原 god-method `add_to_mcp_json`(198 行)在此拆为 remote/local 两条路径:
//! facade `add_to_mcp_json` 负责加载 mcp.json + 取出 `servers` map,再按 manifest
//! 是否含远程 server 委托给 `add_remote_to_mcp_json` / `add_local_to_mcp_json`。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use crate::platform::paths;

use super::MarketplaceManager;
use super::bundle;
use super::python_dependencies;
use super::secrets::{
    is_sensitive_key_name, mcp_secret_env_var, mcp_secret_missing_error, set_remote_secret_header,
};
use super::types::ToolManifest;

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
pub(crate) fn write_json_pretty(path: &Path, value: &serde_json::Value) -> Result<(), String> {
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

/// Load mcp.json for any read-modify-write that must never reset a file it
/// cannot read (reconcile restores, rebuilt entries, and the UI install /
/// uninstall writers all go through here). A file that exists but cannot be
/// parsed is backed up (`mcp.json.corrupt.<ts>`, mirroring the corrupt
/// `installed.json` handling) and reported as an error. Resetting would
/// destroy custom entries and preserved user fields, re-creating the exact
/// parse-failure drift the startup reconcile exists to heal. The same
/// guarantee holds across the whole boot: `run_mcp_startup_maintenance` keeps
/// every later writer (the builtin upsert included) off a file reported by
/// [`mcp_json_unparseable`], so the live file stays byte-identical until the
/// user fixes or removes it.
pub(super) fn load_mcp_json_for_reconcile() -> Result<(PathBuf, serde_json::Value), String> {
    let mcp_path = paths::mcp_config_path();
    if !mcp_path.is_file() {
        return Ok((mcp_path, default_mcp_json()));
    }
    let content = std::fs::read_to_string(&mcp_path).map_err(|e| format!("读取 mcp.json: {e}"))?;
    match serde_json::from_str(&content) {
        Ok(mcp) => Ok((mcp_path, mcp)),
        Err(error) => {
            backup_corrupt_mcp_json(&mcp_path, &content);
            Err(format!(
                "mcp.json is unparseable; the original file was backed up as \
                 mcp.json.corrupt.<timestamp> and left untouched — fix or remove it and \
                 retry: {error}"
            ))
        }
    }
}

/// Whether mcp.json exists on disk but cannot currently be parsed (or read).
/// The startup maintenance keeps every writer off such a file — in
/// particular the builtin upsert's repair loader, which would otherwise
/// reset it to a builtin-only skeleton in the same boot that the reconcile
/// just backed it up. Sessions degrade to an empty MCP pool instead
/// (engine `load_config` failure → empty pool), so preserving the file never
/// blocks a session; the recovery path is the backup plus the timeline note.
pub(crate) fn mcp_json_unparseable() -> bool {
    let mcp_path = paths::mcp_config_path();
    if !mcp_path.is_file() {
        return false;
    }
    // An unreadable file is treated like an unparseable one: the builtin
    // repair loader reads through `unwrap_or_default`, so a permission
    // failure would reset the file to an empty skeleton.
    std::fs::read_to_string(&mcp_path)
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        .is_none()
}

fn backup_corrupt_mcp_json(mcp_path: &Path, content: &str) {
    super::backup_corrupt_json_file(mcp_path, "mcp.json.corrupt", content);
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
                "tool '{}' is missing the Windows Python dependency lock required for a safe install",
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
        // An unparseable mcp.json is backed up and refused, never reset: an
        // install that silently reset the file would destroy the custom
        // entries and preserved user fields the startup reconcile exists to
        // heal. The transaction in `install_inner` rolls the install back, so
        // the user fixes or removes the file and retries.
        let (mcp_path, mut mcp) = load_mcp_json_for_reconcile()?;

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
            let entry = self.build_remote_server_entry(manifest, server, user_config, false)?;
            servers.insert(server.name.clone(), entry);
        }
        Ok(())
    }

    /// Build the fresh-install mcp.json entry for one remote server (url/headers/oauth).
    /// Extracted from `add_remote_to_mcp_json` so the startup reconciliation
    /// (`reconcile_remote_mcp_entries`) reuses the exact same serialization as a UI
    /// install instead of maintaining a second writer. One deliberate addition over
    /// the pre-extraction install writer: the `field.secret` bearer fallback below
    /// also re-derives wiring from the credential store when `user_config` omits
    /// the field — startup restores have no user input at all, and a re-install
    /// over a previously configured tool keeps its credential instead of coming
    /// back unwired (behavior disclosed in the PR description).
    ///
    /// `degrade_unresolved_secrets` marks the startup-reconciliation caller: startup
    /// has no user input this run, so a secret whose credential-store entry no longer
    /// resolves (never entered, or the keyring entry was removed) leaves its wiring
    /// unwritten instead of failing the whole entry — the auth failure then surfaces
    /// through the engine's boot receipt instead of blocking the restore. Installs
    /// keep the fail-loud contract so the user is asked for the key; in both modes
    /// a credential-store read failure propagates instead of degrading, so a
    /// transiently locked keyring never gets baked into a permanently unwired
    /// entry (at install it fails the install; at startup it skips the tool and
    /// the next startup retries).
    fn build_remote_server_entry(
        &self,
        manifest: &ToolManifest,
        server: &super::types::RemoteServer,
        user_config: &HashMap<String, String>,
        degrade_unresolved_secrets: bool,
    ) -> Result<serde_json::Value, String> {
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
                } else if field.secret {
                    // `user_config` omitted this secret bearer field: re-derive the
                    // wiring from the credential store. This serves the startup
                    // restore (which has no user input at all) and also a re-install
                    // over a previously configured tool, whose credential would
                    // otherwise be dropped. A genuinely absent credential leaves the
                    // field unwritten — installs do so silently (historical behavior
                    // for an omitted optional key), the startup reconcile degrades by
                    // design and the auth failure surfaces through the boot receipt
                    // and the restore note. A credential-store read failure must not
                    // degrade into a permanently unwired entry, so it fails the build
                    // in every mode and the next startup retries the restore.
                    match self.try_resolve_secret_placeholder(
                        &manifest.id,
                        bundle::keyring_target(bundle::CredentialTarget::Bearer),
                        &field.key,
                        user_config,
                        &manifest.env,
                    ) {
                        Ok(Some(_)) => {
                            if !secret_header_auth_keys.contains(field.key.as_str()) {
                                set_remote_secret_header(
                                    &mut env_headers,
                                    &mut bearer_token_env_var,
                                    "Authorization",
                                    "Bearer",
                                    &field.key,
                                )?;
                            }
                        }
                        Ok(None) => {}
                        Err(error) => return Err(error),
                    }
                }
            }
        }

        // 2. manifest.secret_headers 声明的敏感 header（不落明文）
        for secret in &manifest.secret_headers {
            match self.try_resolve_secret_placeholder(
                &manifest.id,
                bundle::keyring_target(bundle::CredentialTarget::Bearer),
                &secret.source_key,
                user_config,
                &manifest.env,
            ) {
                // The user's current input must still be persisted to the
                // store; the resolved placeholder value itself is unused here.
                Ok(Some(_)) => {}
                // An absent credential degrades at startup; a store failure
                // propagates in both modes so a locked keyring is never baked
                // into an unwired entry.
                Ok(None) if degrade_unresolved_secrets => continue,
                Ok(None) => return Err(mcp_secret_missing_error(&manifest.id, &secret.source_key)),
                Err(error) => return Err(error),
            }
            set_remote_secret_header(
                &mut env_headers,
                &mut bearer_token_env_var,
                &secret.header,
                &secret.scheme,
                &secret.source_key,
            )?;
        }

        // 3. 兼容旧 manifest.env 中以 _API_KEY 结尾的字段。与明文迁移的落盘同形：
        //    走 `bearer_token_env_var`（env 变量名，请求时经宿主 resolver 解析），
        //    而不是把 `${...}` 占位符写进 `headers` —— 引擎按原样发送 headers 的
        //    字面值、不展开占位符（底座 mcp.rs `headers` 字段文档），字面占位符
        //    是一条永远鉴权失败的接线。
        if headers.is_empty() && env_headers.is_empty() && bearer_token_env_var.is_none() {
            for k in manifest.env.keys() {
                if is_sensitive_key_name(k) {
                    match self.try_resolve_secret_placeholder(
                        &manifest.id,
                        bundle::keyring_target(bundle::CredentialTarget::Bearer),
                        k,
                        user_config,
                        &manifest.env,
                    ) {
                        // The resolved value is persisted to the store/registry
                        // inside `try_resolve_secret_placeholder`; the entry
                        // only ever carries the env-var NAME.
                        Ok(Some(_)) => {
                            set_remote_secret_header(
                                &mut env_headers,
                                &mut bearer_token_env_var,
                                "Authorization",
                                "Bearer",
                                k,
                            )?;
                        }
                        Ok(None) if degrade_unresolved_secrets => continue,
                        Ok(None) => return Err(mcp_secret_missing_error(&manifest.id, k)),
                        Err(error) => return Err(error),
                    }
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
                // Same trimmed form `align_remote_entry_fields` writes and
                // `remote_entry_matches_manifest` compares, so a padded manifest
                // converges at install time instead of costing a realign write.
                entry["oauth_resource"] = serde_json::Value::String(resource.trim().to_string());
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
        Ok(entry)
    }

    pub(super) fn local_server_args(manifest: &ToolManifest, server_dir: &Path) -> Vec<String> {
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

    /// The command a local mcp.json entry launches: the bare python family resolves to
    /// the current runtime, anything else is the manifest value verbatim. Shared by the
    /// install writer (`add_local_to_mcp_json`) and the startup rebuild validator so
    /// both judge the same launch target.
    pub(super) fn local_server_command(manifest: &ToolManifest) -> String {
        if manifest.command == "python" || manifest.command == "python3" {
            paths::python_command()
        } else {
            manifest.command.clone()
        }
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

    /// Local tool layout: command/args/env. Python tools use the bundled python (Windows)
    /// or the system python3; sensitive fields go through `${ENV}` placeholders, non-sensitive ones verbatim.
    fn add_local_to_mcp_json(
        &self,
        manifest: &ToolManifest,
        user_config: &HashMap<String, String>,
        server_dir: &std::path::Path,
        python_environment: Option<&python_dependencies::InstalledPythonEnvironment>,
        servers: &mut serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        let entry = self.build_local_server_entry(
            manifest,
            user_config,
            server_dir,
            python_environment,
            false,
        )?;
        servers.insert(manifest.id.clone(), entry);
        Ok(())
    }

    /// Build the fresh-install mcp.json entry for one local tool (command/args/env).
    /// Extracted from `add_local_to_mcp_json` so the startup rebuild can merge the
    /// preserved user fields in memory and land them in the same single atomic write
    /// as a fresh install (`write_rebuilt_local_entry`) instead of a
    /// rebuild-then-reapply write pair.
    ///
    /// `degrade_unresolved_secrets` marks the startup-reconciliation caller: startup
    /// has no user input, so a secret whose credential-store entry no longer resolves
    /// is left out of `env` instead of failing the whole entry (the spawn failure
    /// then surfaces through the engine's boot receipt). Installs keep the fail-loud
    /// contract so the user is asked for the key.
    fn build_local_server_entry(
        &self,
        manifest: &ToolManifest,
        user_config: &HashMap<String, String>,
        server_dir: &std::path::Path,
        python_environment: Option<&python_dependencies::InstalledPythonEnvironment>,
        degrade_unresolved_secrets: bool,
    ) -> Result<serde_json::Value, String> {
        let (command, args) = if let Some(environment) = python_environment {
            Self::managed_python_runtime_fields(manifest, server_dir, environment)?
        } else {
            (
                Self::local_server_command(manifest),
                Self::local_server_args(manifest, server_dir),
            )
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
                } else if degrade_unresolved_secrets
                    && (field.secret || is_sensitive_key_name(&field.key))
                {
                    // Startup rebuild has no user input: re-derive a secret
                    // config field from the credential store, the same channel
                    // the install writer filled from the user's input. An
                    // absent credential degrades; a store failure fails the
                    // rebuild so the next startup retries instead of silently
                    // dropping the wiring.
                    match self.try_resolve_secret_placeholder(
                        &manifest.id,
                        "env",
                        &field.key,
                        user_config,
                        &manifest.env,
                    ) {
                        Ok(Some(placeholder)) => {
                            env.insert(field.key.clone(), placeholder);
                        }
                        Ok(None) => {}
                        Err(error) => return Err(error),
                    }
                }
            }
        }
        for secret in &manifest.secret_env {
            match self.try_resolve_secret_placeholder(
                &manifest.id,
                "env",
                &secret.key,
                user_config,
                &manifest.env,
            ) {
                Ok(Some(placeholder)) => {
                    env.insert(secret.key.clone(), placeholder);
                }
                Ok(None) if degrade_unresolved_secrets => continue,
                Ok(None) => return Err(mcp_secret_missing_error(&manifest.id, &secret.key)),
                Err(error) => return Err(error),
            }
        }
        for key in manifest.env.keys().filter(|k| is_sensitive_key_name(k)) {
            if !env.contains_key(key) {
                match self.try_resolve_secret_placeholder(
                    &manifest.id,
                    "env",
                    key,
                    user_config,
                    &manifest.env,
                ) {
                    Ok(Some(placeholder)) => {
                        env.insert(key.clone(), placeholder);
                    }
                    Ok(None) if degrade_unresolved_secrets => continue,
                    Ok(None) => return Err(mcp_secret_missing_error(&manifest.id, key)),
                    Err(error) => return Err(error),
                }
            }
        }

        let mut entry = serde_json::json!({
            "command": command,
            "args": args,
        });
        if !env.is_empty() {
            entry["env"] = serde_json::to_value(&env).unwrap_or_default();
        }
        Ok(entry)
    }

    /// Single-write startup rebuild of a local tool's mcp.json entry: build the
    /// fresh-install form (empty user config — startup has no user input, so secret
    /// placeholders resolve from the credential store, degrading per
    /// `build_local_server_entry` when they no longer resolve), merge the
    /// caller-supplied preserved user fields (everything except command/args/env)
    /// in memory, and write once. One atomic write instead of a
    /// rebuild-then-reapply pair: a crash can never leave the fresh form without
    /// the preserved fields.
    pub(super) fn write_rebuilt_local_entry(
        &self,
        manifest: &ToolManifest,
        server_dir: &std::path::Path,
        preserved: serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        let mut entry =
            self.build_local_server_entry(manifest, &HashMap::new(), server_dir, None, true)?;
        if let Some(object) = entry.as_object_mut() {
            for (key, value) in preserved {
                object.insert(key, value);
            }
        }
        let _guard = mcp_json_lock();
        let (mcp_path, mut mcp) = load_mcp_json_for_reconcile()?;
        let servers = mcp
            .get_mut("servers")
            .and_then(|s| s.as_object_mut())
            .ok_or("mcp.json 格式错误")?;
        servers.insert(manifest.id.clone(), entry);
        write_json_pretty(&mcp_path, &mcp)
    }

    pub(super) fn remove_from_mcp_json(&self, tool_id: &str) -> Result<(), String> {
        let _guard = mcp_json_lock();
        let mcp_path = paths::mcp_config_path();
        if !mcp_path.is_file() {
            return Ok(());
        }
        // An unparseable mcp.json is backed up and refused, never reset. This
        // writer is reachable at boot from the retired-tool cleanup and the
        // Python-repair downgrade, and from the UI uninstall: a reset there
        // would destroy the file before (cleanup) or despite (downgrade,
        // uninstall) the reconcile's backup — the callers' transactions roll
        // back instead, and the cleanup simply retries on a later boot.
        let (mcp_path, mut mcp) = load_mcp_json_for_reconcile()?;

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

    /// Startup reconciliation for the given remote servers of one tool (callers pass
    /// `&manifest.servers` or a subset with contested keys removed):
    /// - A missing per-server entry is added with the exact fresh-install serialization
    ///   (startup has no user input, so secret placeholders resolve from the credential
    ///   store; a credential that is absent from the store leaves its wiring unwritten
    ///   rather than failing the restore, while a credential-store read failure fails
    ///   the restore so the next startup retries — installs keep the fail-loud
    ///   contract for both cases).
    /// - An existing entry whose manifest-derived shape (url/scopes/oauth_resource)
    ///   drifted from the current manifest is realigned; credential fields
    ///   (headers/env_headers/bearer_token_env_var) and forward-compatible fields are
    ///   preserved because they cannot be re-derived without user input. The `oauth`
    ///   block is the documented user-set client override, so an existing block is
    ///   never judged or rewritten — only a missing one is healed from the manifest.
    /// - Entries not owned by this manifest are never touched.
    /// Idempotent: returns Ok(None) without writing when everything already matches.
    pub(super) fn reconcile_remote_mcp_entries(
        &self,
        manifest: &ToolManifest,
        servers: &[&super::types::RemoteServer],
    ) -> Result<Option<String>, String> {
        let _guard = mcp_json_lock();
        let (mcp_path, mut mcp) = load_mcp_json_for_reconcile()?;
        let servers_map = mcp
            .get_mut("servers")
            .and_then(|s| s.as_object_mut())
            .ok_or("mcp.json 格式错误")?;

        let mut changed: Vec<String> = Vec::new();
        for server in servers {
            if super::ENGINE_OWNED_MCP_SERVER_KEYS.contains(&server.name.as_str()) {
                // Unreachable via reconcile_installed_mcp_entries (the caller
                // filters these with a timeline note); kept as defense in depth
                // so a direct caller can never fight the boot-time builtin upsert.
                continue;
            }
            match servers_map
                .get_mut(&server.name)
                .and_then(|entry| entry.as_object_mut())
            {
                // Present as an object: realign in place when the manifest-derived
                // shape drifted. A non-object value takes the restore branch below.
                Some(object) => {
                    if !remote_entry_matches_manifest(object, server) {
                        align_remote_entry_fields(object, server);
                        changed.push(format!("realigned remote entry '{}'", server.name));
                    }
                }
                _ => {
                    let entry =
                        self.build_remote_server_entry(manifest, server, &HashMap::new(), true)?;
                    servers_map.insert(server.name.clone(), entry.clone());
                    // Say what the restore could not do instead of reporting an
                    // unqualified success that 401s on first use.
                    let mut note = format!("restored missing remote entry '{}'", server.name);
                    if manifest
                        .config_fields
                        .iter()
                        .any(|field| field.target == "bearer" && !field.secret)
                    {
                        note.push_str(
                            "; its non-secret bearer key is not stored and could not be \
                             re-derived — reinstall the tool to re-enter it",
                        );
                    }
                    let missing_secrets = entry_secret_keys_without_wiring(manifest, &entry);
                    if !missing_secrets.is_empty() {
                        note.push_str(&format!(
                            "; no stored credential for {} — restored without that auth \
                             wiring; reinstall the tool to re-enter it",
                            missing_secrets.join(", ")
                        ));
                    }
                    changed.push(note);
                }
            }
        }

        if changed.is_empty() {
            return Ok(None);
        }
        write_json_pretty(&mcp_path, &mcp)?;
        Ok(Some(changed.join("; ")))
    }

    // Preserved user fields are folded into the rebuild itself
    // (`write_rebuilt_local_entry`): the fresh form and the preserved fields
    // land in one atomic write, so a crash between phases can never leave the
    // fresh form without them.
}

/// Read-only snapshot of the current mcp.json `servers` map for reconciliation
/// decisions, taken without `mcp_json_lock`. It gates *whether* a repair is
/// attempted and sources the user fields preserved across a rebuilt local
/// entry; the writes themselves re-read the file under the lock, and every
/// concurrent marketplace writer is excluded by
/// MARKETPLACE_TRANSACTION_LOCK (the boot-path writers run sequentially on the
/// same thread), so the merge cannot clobber a concurrent change.
pub(super) fn read_mcp_servers_snapshot() -> serde_json::Map<String, serde_json::Value> {
    let mcp_path = paths::mcp_config_path();
    let parsed: serde_json::Value = if mcp_path.is_file() {
        std::fs::read_to_string(&mcp_path)
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
            .unwrap_or_else(default_mcp_json)
    } else {
        default_mcp_json()
    };
    parsed
        .get("servers")
        .and_then(|s| s.as_object())
        .cloned()
        .unwrap_or_default()
}

/// First dead target of a local mcp.json entry: an absolute path (command or arg)
/// that no longer exists on disk. Bare interpreter names and relative args are
/// never judged (PATH/cwd resolution is out of scope), and neither are existing
/// directories, so managed-environment lifecycle stays with the Python repair path.
pub(super) fn dead_local_entry_target(entry: &serde_json::Value) -> Option<String> {
    let mut candidates: Vec<&str> = Vec::new();
    if let Some(command) = entry.get("command").and_then(|v| v.as_str()) {
        candidates.push(command);
    }
    if let Some(args) = entry.get("args").and_then(|v| v.as_array()) {
        candidates.extend(args.iter().filter_map(|v| v.as_str()));
    }
    candidates
        .into_iter()
        .find(|s| Path::new(s).is_absolute() && !Path::new(s).exists())
        .map(str::to_string)
}

/// Secret source keys of `manifest` (secret_headers, secret bearer config
/// fields, legacy sensitive manifest.env keys) whose env-var NAME appears
/// nowhere in a restored remote entry: the restore degraded to an entry
/// without that wiring because no stored credential resolved. Only names are
/// reported — never values.
fn entry_secret_keys_without_wiring(
    manifest: &ToolManifest,
    entry: &serde_json::Value,
) -> Vec<String> {
    let rendered = entry.to_string();
    let mut keys: Vec<String> = Vec::new();
    let mut push = |key: &str| {
        if rendered.contains(&mcp_secret_env_var(key)) {
            return;
        }
        if !keys.iter().any(|k| k == key) {
            keys.push(key.to_string());
        }
    };
    for secret in &manifest.secret_headers {
        push(&secret.source_key);
    }
    for field in &manifest.config_fields {
        if field.target == "bearer" && field.secret {
            push(&field.key);
        }
    }
    for key in manifest.env.keys().filter(|k| is_sensitive_key_name(k)) {
        push(key);
    }
    keys
}

/// Whether an existing remote entry carries the exact manifest-derived fields a fresh
/// install would write. Credential fields are deliberately not compared: they come from
/// user input/keyring and may legitimately differ from what an empty startup config
/// would produce.
fn remote_entry_matches_manifest(
    object: &serde_json::Map<String, serde_json::Value>,
    server: &super::types::RemoteServer,
) -> bool {
    if object.get("url").and_then(|v| v.as_str()) != Some(server.url.as_str()) {
        return false;
    }
    let expected_scopes = if server.scopes.is_empty() {
        None
    } else {
        Some(serde_json::to_value(&server.scopes).unwrap_or_default())
    };
    match (expected_scopes.as_ref(), object.get("scopes")) {
        (None, None) => {}
        // An empty array is the absent form's alias; a fresh install never writes it.
        (None, Some(value)) if value.as_array().is_some_and(Vec::is_empty) => {}
        (None, Some(_)) => return false,
        (Some(expected), actual) => {
            if actual != Some(expected) {
                return false;
            }
        }
    }
    let expected_oauth: Option<serde_json::Value> = match &server.oauth {
        Some(oauth) => serde_json::to_value(oauth).ok(),
        None => None,
    };
    // The engine documents `oauth` as the user-set client override for
    // servers that require a pre-registered public client (instead of
    // dynamic registration). An existing block therefore belongs to the
    // user, not the manifest: it is never judged drifted and never
    // rewritten — only a missing (or null) block is healed from the
    // manifest (`align_remote_entry_fields`).
    let user_oauth_override = object.get("oauth").is_some_and(|value| !value.is_null());
    if !user_oauth_override && expected_oauth.is_some() {
        return false;
    }
    let resource = server
        .oauth_resource
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty());
    match (
        resource,
        object.get("oauth_resource").and_then(|v| v.as_str()),
    ) {
        (None, None | Some("")) => {}
        (None, Some(_)) => return false,
        (Some(expected), actual) => {
            if actual != Some(expected) {
                return false;
            }
        }
    }
    true
}

/// Rewrite only the manifest-derived fields of an existing remote entry; everything
/// else (headers/env_headers/bearer_token_env_var/timeout/…) is kept as-is. The
/// one exception to "manifest-derived" is `oauth`: the engine documents it as the
/// user-set client override for servers requiring a pre-registered public client,
/// so an existing block is preserved verbatim and only a missing one is healed
/// from the manifest (matching `remote_entry_matches_manifest`).
fn align_remote_entry_fields(
    object: &mut serde_json::Map<String, serde_json::Value>,
    server: &super::types::RemoteServer,
) {
    object.insert(
        "url".to_string(),
        serde_json::Value::String(server.url.clone()),
    );
    if server.scopes.is_empty() {
        object.remove("scopes");
    } else if let Ok(scopes) = serde_json::to_value(&server.scopes) {
        object.insert("scopes".to_string(), scopes);
    }
    if object.get("oauth").is_some_and(|value| !value.is_null()) {
        // User-set client override: preserved verbatim (see
        // `remote_entry_matches_manifest`).
    } else {
        // Absent or an explicit null: write the manifest default, or clear a
        // null placeholder so the entry converges on the canonical absent form.
        match &server.oauth {
            Some(oauth) => {
                if let Ok(value) = serde_json::to_value(oauth) {
                    object.insert("oauth".to_string(), value);
                }
            }
            None => {
                object.remove("oauth");
            }
        }
    }
    match server
        .oauth_resource
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
    {
        Some(resource) => {
            object.insert(
                "oauth_resource".to_string(),
                serde_json::Value::String(resource.to_string()),
            );
        }
        None => {
            object.remove("oauth_resource");
        }
    }
}

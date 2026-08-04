use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use wait_timeout::ChildExt;

use super::{kimi_data_root, AcpPool, AgentBackend};

const AUTH_STATUS_TTL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub(super) struct CachedAuthStatus {
    executable: PathBuf,
    authenticated: bool,
    checked_at: Instant,
    generation: u64,
}

#[derive(Debug, Default)]
struct AgentAuthProbeSlot {
    gate: parking_lot::Mutex<()>,
    generation: AtomicU64,
}

#[derive(Debug, Default)]
pub(super) struct AgentAuthProbeState {
    codex: AgentAuthProbeSlot,
    claude: AgentAuthProbeSlot,
    kimi: AgentAuthProbeSlot,
}

impl AgentAuthProbeState {
    fn slot(&self, backend: AgentBackend) -> &AgentAuthProbeSlot {
        match backend {
            AgentBackend::CodexAcp => &self.codex,
            AgentBackend::ClaudeAcp => &self.claude,
            AgentBackend::KimiAcp => &self.kimi,
            AgentBackend::Deepseek => unreachable!("Deepseek does not use ACP authentication"),
        }
    }
}

impl AcpPool {
    pub(super) async fn agent_authenticated_async(
        &self,
        backend: AgentBackend,
        executable: &Path,
    ) -> bool {
        let pool = self.clone();
        let executable = executable.to_path_buf();
        tokio::task::spawn_blocking(move || pool.cached_agent_authenticated(backend, &executable))
            .await
            .unwrap_or(false)
    }

    fn agent_authenticated(&self, backend: AgentBackend, executable: &Path) -> bool {
        match backend {
            AgentBackend::CodexAcp => codex_authenticated(executable),
            AgentBackend::ClaudeAcp => claude_authenticated(executable),
            AgentBackend::KimiAcp => kimi_authenticated(),
            AgentBackend::Deepseek => true,
        }
    }

    pub(super) fn cached_agent_authenticated(
        &self,
        backend: AgentBackend,
        executable: &Path,
    ) -> bool {
        let slot = self.auth_probe.slot(backend);
        loop {
            let observed_generation = slot.generation.load(Ordering::Acquire);
            if let Some(authenticated) =
                self.valid_cached_auth(backend, executable, observed_generation)
            {
                return authenticated;
            }

            let _gate = slot.gate.lock();
            let generation = slot.generation.load(Ordering::Acquire);
            if let Some(authenticated) = self.valid_cached_auth(backend, executable, generation) {
                return authenticated;
            }
            if generation != observed_generation {
                continue;
            }

            let authenticated = self.agent_authenticated(backend, executable);
            if !self.store_auth_cache_if_current(
                backend,
                executable.to_path_buf(),
                authenticated,
                generation,
            ) {
                continue;
            }
            return authenticated;
        }
    }

    fn valid_cached_auth(
        &self,
        backend: AgentBackend,
        executable: &Path,
        generation: u64,
    ) -> Option<bool> {
        let cached = self.auth_cache.read().get(&backend).cloned()?;
        (cached.generation == generation
            && cached.executable == executable
            && cached.checked_at.elapsed() < AUTH_STATUS_TTL)
            .then_some(cached.authenticated)
    }

    fn store_auth_cache_if_current(
        &self,
        backend: AgentBackend,
        executable: PathBuf,
        authenticated: bool,
        generation: u64,
    ) -> bool {
        let mut cache = self.auth_cache.write();
        if self
            .auth_probe
            .slot(backend)
            .generation
            .load(Ordering::Acquire)
            != generation
        {
            return false;
        }
        cache.insert(
            backend,
            CachedAuthStatus {
                executable,
                authenticated,
                checked_at: Instant::now(),
                generation,
            },
        );
        true
    }

    pub(super) fn invalidate_auth_cache(&self, backend: AgentBackend) {
        if backend == AgentBackend::Deepseek {
            return;
        }
        self.auth_probe
            .slot(backend)
            .generation
            .fetch_add(1, Ordering::AcqRel);
        self.auth_cache.write().remove(&backend);
    }
}

fn codex_authenticated(codex: &Path) -> bool {
    if nonempty_env("OPENAI_API_KEY") || deepseek_tui::oauth::credentials_present() {
        return true;
    }
    // 第三方 Provider 的 key 只注入被托管的 Codex 子进程，状态探测进程
    // 看不到该环境变量。配置指向有效的受管 Provider 时仍应视为已认证。
    if let Ok(raw) = std::fs::read_to_string(
        crate::platform::os::user_home_dir()
            .join(".codex")
            .join("config.toml"),
    ) {
        if super::providers::codex_config_relay_env_key_present(&raw) {
            return true;
        }
    }
    cli_status_success(codex, &["login", "status"])
}

fn claude_authenticated(claude: &Path) -> bool {
    if [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
    ]
    .into_iter()
    .any(nonempty_env)
    {
        return true;
    }
    cli_status_success(claude, &["auth", "status"])
}

fn kimi_authenticated() -> bool {
    // Kimi Code 0.31+ ignores a bare KIMI_API_KEY. Only the paired model
    // overrides can synthesize a provider/model entirely in memory.
    if nonempty_env("KIMI_MODEL_NAME") && nonempty_env("KIMI_MODEL_API_KEY") {
        return true;
    }
    let root = kimi_data_root();
    let oauth_credentials_valid =
        std::fs::read_to_string(root.join("credentials").join("kimi-code.json"))
            .is_ok_and(|raw| kimi_credentials_valid(&raw));
    let Ok(config) = std::fs::read_to_string(root.join("config.toml")) else {
        return false;
    };
    kimi_runtime_config_ready(&config, oauth_credentials_valid)
}

/// OAuth credentials alone are insufficient: the official Kimi login also
/// needs to persist a default model that resolves to a usable provider.
pub(super) fn kimi_runtime_config_ready(raw: &str, oauth_credentials_valid: bool) -> bool {
    let Ok(config) = raw.parse::<toml::Value>() else {
        return false;
    };
    let Some(default_model) = config
        .get("default_model")
        .and_then(toml::Value::as_str)
        .filter(|value| !value.trim().is_empty())
    else {
        return false;
    };
    let Some(model) = config
        .get("models")
        .and_then(|models| models.get(default_model))
        .and_then(toml::Value::as_table)
    else {
        return false;
    };
    let Some(provider) = model
        .get("provider")
        .and_then(toml::Value::as_str)
        .filter(|value| !value.trim().is_empty())
    else {
        return false;
    };
    let model_ready = model
        .get("model")
        .and_then(toml::Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
        && model
            .get("max_context_size")
            .and_then(toml::Value::as_integer)
            .is_some_and(|value| value > 0);
    if !model_ready {
        return false;
    }
    let Some(provider) = config
        .get("providers")
        .and_then(|providers| providers.get(provider))
        .and_then(toml::Value::as_table)
    else {
        return false;
    };
    if !provider
        .get("type")
        .and_then(toml::Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
    {
        return false;
    }
    let direct_api_key = provider
        .get("api_key")
        .and_then(toml::Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    let configured_env_api_key = provider
        .get("env")
        .and_then(toml::Value::as_table)
        .is_some_and(|env| {
            env.iter().any(|(name, value)| {
                name.ends_with("_API_KEY")
                    && value.as_str().is_some_and(|value| !value.trim().is_empty())
            })
        });
    let oauth_ready =
        provider.get("oauth").is_some_and(toml::Value::is_table) && oauth_credentials_valid;
    direct_api_key || configured_env_api_key || oauth_ready
}

pub(super) fn kimi_credentials_valid(raw: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return false;
    };
    let token_present = ["access_token", "refresh_token"].into_iter().all(|key| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|token| !token.trim().is_empty())
    });
    // The access token is short-lived and the CLI refreshes it. A positive
    // expires_at is therefore a corruption check, not a current-time check.
    let expiry_valid = value
        .get("expires_at")
        .and_then(serde_json::Value::as_i64)
        .is_some_and(|expiry| expiry > 0);
    token_present && expiry_valid
}

pub(super) fn nonempty_env(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

pub(super) fn cli_status_success(executable: &Path, args: &[&str]) -> bool {
    let mut command = crate::platform::process::external_command(executable);
    command.args(args);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    // Node 版 CLI 冷启动实测约 9 秒；与版本探测使用相同上限，避免误报未认证。
    match child.wait_timeout(Duration::from_secs(15)) {
        Ok(Some(status)) => status.success(),
        Ok(None) | Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            false
        }
    }
}

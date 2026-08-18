use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;

use super::{AcpPool, AgentBackend};

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
            AgentBackend::CodexAcp => super::codex_authenticated(executable),
            AgentBackend::ClaudeAcp => super::login::claude_authenticated(executable),
            AgentBackend::KimiAcp => super::introspect::kimi_authenticated(executable),
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

/// Recognize a usable Codex OAuth grant without refreshing it. The 60-second
/// safety margin matches the engine boundary: when freshness cannot be proven,
/// callers fall back to the official CLI status probe.
pub(super) fn codex_oauth_credentials_present() -> bool {
    if ["OPENAI_CODEX_ACCESS_TOKEN", "CODEX_ACCESS_TOKEN"]
        .into_iter()
        .any(super::login::nonempty_env)
    {
        return true;
    }
    let home = crate::platform::os::user_home_dir();
    let auth_file = std::env::var("OPENAI_CODEX_AUTH_FILE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var("CODEX_HOME")
                .ok()
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".codex"))
                .join("auth.json")
        });
    let Ok(raw) = std::fs::read_to_string(auth_file) else {
        return false;
    };
    codex_auth_json_credentials_present(&raw)
}

fn codex_auth_json_credentials_present(raw: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return false;
    };
    let Some(token) = value
        .get("tokens")
        .and_then(|tokens| tokens.get("access_token"))
        .and_then(serde_json::Value::as_str)
        .filter(|token| !token.trim().is_empty())
    else {
        return false;
    };
    codex_access_token_is_fresh(token)
}

fn codex_access_token_is_fresh(token: &str) -> bool {
    let Some(payload) = token.split('.').nth(1) else {
        return false;
    };
    let Ok(decoded) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload) else {
        return false;
    };
    let Ok(claims) = serde_json::from_slice::<serde_json::Value>(&decoded) else {
        return false;
    };
    let Some(expires_at) = claims.get("exp").and_then(serde_json::Value::as_u64) else {
        return false;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    now.saturating_add(60) < expires_at
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt(exp: u64) -> String {
        let payload =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!(r#"{{"exp":{exp}}}"#));
        format!("header.{payload}.signature")
    }

    #[test]
    fn auth_json_reads_flat_access_token_and_rejects_unusable_tokens() {
        let valid = serde_json::json!({
            "tokens": {
                "access_token": jwt(9_999_999_999),
                "account_id": "account-a"
            }
        });
        assert!(codex_auth_json_credentials_present(&valid.to_string()));

        let expired = serde_json::json!({
            "tokens": { "access_token": jwt(1_000_000_000) }
        });
        assert!(!codex_auth_json_credentials_present(&expired.to_string()));
        assert!(!codex_auth_json_credentials_present(
            r#"{"tokens":{"access_token":"  "}}"#
        ));
        assert!(!codex_auth_json_credentials_present(
            r#"{"tokens":{"openai":{"access_token":"legacy-shape"}}}"#
        ));
        assert!(!codex_auth_json_credentials_present("{not-json"));
    }
}

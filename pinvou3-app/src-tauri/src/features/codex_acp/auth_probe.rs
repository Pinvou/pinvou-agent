use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

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

use std::collections::HashMap;
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

    /// Cache validity rule, kept as pure logic for unit testing: the entry is
    /// usable only when the seqlock generation, executable path, and TTL all
    /// match the probe that wants to reuse it.
    pub(super) fn cached_auth_valid(
        cache: &HashMap<AgentBackend, CachedAuthStatus>,
        backend: AgentBackend,
        executable: &Path,
        generation: u64,
    ) -> Option<bool> {
        let cached = cache.get(&backend).cloned()?;
        (cached.generation == generation
            && cached.executable == executable
            && cached.checked_at.elapsed() < AUTH_STATUS_TTL)
            .then_some(cached.authenticated)
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
        AgentAuthProbeState::cached_auth_valid(
            &self.auth_cache.read(),
            backend,
            executable,
            generation,
        )
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn auth_cache_validity_follows_generation_executable_and_ttl() {
        let backend = AgentBackend::ClaudeAcp;
        let executable = PathBuf::from("/opt/cli/agent");
        let mut cache = HashMap::new();

        let entry = |generation: u64, authenticated: bool, age: Duration| CachedAuthStatus {
            executable: executable.clone(),
            authenticated,
            checked_at: Instant::now() - age,
            generation,
        };

        // Nothing cached yet.
        assert!(AgentAuthProbeState::cached_auth_valid(&cache, backend, &executable, 0).is_none());

        // Matching generation/executable/fresh entry is valid.
        cache.insert(backend, entry(0, true, Duration::ZERO));
        assert_eq!(
            AgentAuthProbeState::cached_auth_valid(&cache, backend, &executable, 0),
            Some(true)
        );

        // A bumped generation (invalidate) must reject the cached entry.
        assert!(AgentAuthProbeState::cached_auth_valid(&cache, backend, &executable, 1).is_none());

        // A different executable path must not reuse the cached verdict.
        let other = PathBuf::from("/opt/cli/other");
        assert!(AgentAuthProbeState::cached_auth_valid(&cache, backend, &other, 0).is_none());

        // An expired TTL must invalidate even a matching entry.
        cache.insert(
            backend,
            entry(0, true, AUTH_STATUS_TTL + Duration::from_secs(1)),
        );
        assert!(AgentAuthProbeState::cached_auth_valid(&cache, backend, &executable, 0).is_none());

        // A false verdict with a fresh matching entry round-trips as false.
        cache.insert(backend, entry(0, false, Duration::ZERO));
        assert_eq!(
            AgentAuthProbeState::cached_auth_valid(&cache, backend, &executable, 0),
            Some(false)
        );
    }
}

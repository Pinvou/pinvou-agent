use std::path::PathBuf;

use super::{
    claude_version_supported, detect_install_source, kimi_version_supported, path_install_source,
    probe_cli_version, resolve_claude_cli, resolve_kimi_path, AcpPool, AgentBackend,
    CliVersionProbe,
};

/// Claude / Kimi 独立的 CLI 探测缓存：选择一个 Agent 不应顺带探测另一个 Agent。
/// 外层 Option 区分“尚未探测”，内层 Option 表示“已经探测但未找到 CLI”。
#[derive(Debug, Clone, Default)]
pub(super) struct CliProbeCache {
    claude: CliProbeSlot,
    kimi: CliProbeSlot,
}

#[derive(Debug, Clone, Default)]
struct CliProbeSlot {
    generation: u64,
    value: Option<Option<ResolvedCli>>,
}

#[derive(Debug, Default)]
pub(super) struct CliProbeGates {
    claude: parking_lot::Mutex<()>,
    kimi: parking_lot::Mutex<()>,
}

impl CliProbeGates {
    fn for_backend(&self, backend: AgentBackend) -> &parking_lot::Mutex<()> {
        match backend {
            AgentBackend::ClaudeAcp => &self.claude,
            AgentBackend::KimiAcp => &self.kimi,
            AgentBackend::Deepseek | AgentBackend::CodexAcp => {
                unreachable!("Codex uses the dedicated runtime probe")
            }
        }
    }
}

impl CliProbeCache {
    fn slot(&self, backend: AgentBackend) -> &CliProbeSlot {
        match backend {
            AgentBackend::ClaudeAcp => &self.claude,
            AgentBackend::KimiAcp => &self.kimi,
            AgentBackend::Deepseek | AgentBackend::CodexAcp => {
                unreachable!("Codex uses the dedicated runtime probe")
            }
        }
    }

    fn slot_mut(&mut self, backend: AgentBackend) -> &mut CliProbeSlot {
        match backend {
            AgentBackend::ClaudeAcp => &mut self.claude,
            AgentBackend::KimiAcp => &mut self.kimi,
            AgentBackend::Deepseek | AgentBackend::CodexAcp => {
                unreachable!("Codex uses the dedicated runtime probe")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct ResolvedCli {
    pub(super) path: PathBuf,
    /// `--version` 原始输出；版本门禁单独校验，解析失败一律不合规。
    pub(super) version: Option<String>,
    /// 安装来源（"brew"/"npm"/"script"），供版本过旧时按来源分派升级。
    pub(super) install_source: Option<&'static str>,
}

impl AcpPool {
    /// 首次选中时只探测该 Agent，之后状态轮询只读缓存。
    pub(super) fn cli_probe_for(&self, backend: AgentBackend) -> Option<ResolvedCli> {
        if matches!(backend, AgentBackend::Deepseek | AgentBackend::CodexAcp) {
            return None;
        }

        loop {
            let observed_generation = {
                let probe = self.cli_probe.read();
                let slot = probe.slot(backend);
                if let Some(cached) = slot.value.clone() {
                    return cached;
                }
                slot.generation
            };

            let _gate = self.cli_probe_gates.for_backend(backend).lock();
            let generation = {
                let probe = self.cli_probe.read();
                let slot = probe.slot(backend);
                if let Some(cached) = slot.value.clone() {
                    return cached;
                }
                slot.generation
            };
            if generation != observed_generation {
                continue;
            }

            let detected = match backend {
                AgentBackend::ClaudeAcp => probe_cli(
                    backend,
                    resolve_claude_cli(self.resolve_claude_adapter().as_deref()),
                ),
                AgentBackend::KimiAcp => probe_cli(backend, resolve_kimi_path()),
                AgentBackend::Deepseek | AgentBackend::CodexAcp => unreachable!(),
            };
            let mut probe = self.cli_probe.write();
            let slot = probe.slot_mut(backend);
            if slot.generation != generation {
                continue;
            }
            slot.value = Some(detected.clone());
            return detected;
        }
    }

    pub(super) fn invalidate_cli_probe(&self, backend: AgentBackend) {
        if matches!(backend, AgentBackend::Deepseek | AgentBackend::CodexAcp) {
            return;
        }
        let mut probe = self.cli_probe.write();
        let slot = probe.slot_mut(backend);
        slot.generation = slot.generation.wrapping_add(1);
        slot.value = None;
    }
}

fn probe_cli(backend: AgentBackend, path: Option<PathBuf>) -> Option<ResolvedCli> {
    let path = path?;
    // 官方原生 CLI 首次经过系统安全扫描时也可能偶发超过单次自检上限。
    // 仅在首轮失败且路径确实存在时补一次重试；正常路径仍只执行一次，
    // 缺失 CLI 不会触发任何额外进程。
    let probe = probe_cli_version(&path);
    let probe = if matches!(probe, CliVersionProbe::TimedOut) {
        probe_cli_version(&path)
    } else {
        probe
    };
    let version = match probe {
        CliVersionProbe::Found(version) => Some(version),
        CliVersionProbe::TimedOut | CliVersionProbe::Failed => None,
    };
    let version_supported = version.as_deref().is_some_and(|version| match backend {
        AgentBackend::ClaudeAcp => claude_version_supported(version),
        AgentBackend::KimiAcp => kimi_version_supported(version),
        AgentBackend::Deepseek | AgentBackend::CodexAcp => false,
    });
    // 正常可用的 CLI 不需要知道未来如何升级，避免首次选中时等待 npm/brew；
    // 版本过旧时仍完整判定来源，确保升级动作不会打到同机的另一份 CLI。
    let install_source = if version_supported {
        path_install_source(backend, &path)
    } else {
        detect_install_source(backend, &path)
    };
    Some(ResolvedCli {
        path,
        version,
        install_source,
    })
}

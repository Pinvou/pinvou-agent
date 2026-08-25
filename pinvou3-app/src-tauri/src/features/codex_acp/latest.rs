use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use parking_lot::RwLock;
use reqwest::Client;
use serde_json::Value;
use tokio::sync::Mutex;

use super::{
    diagnostics, install_action_for, official_script_supported, AcpPool, AgentBackend,
    CodexAcpStatus,
};

const CODEX_LATEST_URL: &str = "https://releases.openai.com/codex/channels/latest";
const CODEX_GITHUB_LATEST_URL: &str = "https://github.com/openai/codex/releases/latest";
const CLAUDE_LATEST_URL: &str = "https://downloads.claude.ai/claude-code-releases/latest";
const KIMI_LATEST_URL: &str = "https://code.kimi.com/kimi-code/latest";
const CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_RESPONSE_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone)]
struct LatestVersionEntry {
    checked_at: Instant,
    version: Option<String>,
}

/// 三个 CLI 的官方 latest 探测器。
///
/// 前端会按秒读取状态，因此成功和失败结果都缓存五分钟；每个 Agent 使用独立异步锁，
/// 首次打开代码模式时可并行查询，且不会把网络请求塞进同步 CLI 探测线程。
#[derive(Clone)]
pub(super) struct LatestVersionProbe {
    client: Client,
    entries: Arc<RwLock<HashMap<AgentBackend, LatestVersionEntry>>>,
    gates: Arc<HashMap<AgentBackend, Arc<Mutex<()>>>>,
}

impl LatestVersionProbe {
    pub(super) fn new() -> Result<Self> {
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("Pinvou-Agent/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("创建 ACP Agent 最新版本查询客户端失败")?;
        let gates = [
            AgentBackend::CodexAcp,
            AgentBackend::ClaudeAcp,
            AgentBackend::KimiAcp,
        ]
        .into_iter()
        .map(|backend| (backend, Arc::new(Mutex::new(()))))
        .collect();
        Ok(Self {
            client,
            entries: Arc::new(RwLock::new(HashMap::new())),
            gates: Arc::new(gates),
        })
    }

    pub(super) fn cached(&self, backend: AgentBackend) -> Option<String> {
        self.entries
            .read()
            .get(&backend)
            .and_then(|entry| entry.version.clone())
    }

    pub(super) fn apply_to_status(&self, backend: AgentBackend, status: &mut CodexAcpStatus) {
        let latest = self.cached(backend);
        let requires_update = latest_update_available(
            status.codex_available,
            status.version.as_deref(),
            latest.as_deref(),
        );
        if !requires_update {
            // latest_version 是升级目标，不是单纯的探测遥测。这样 Codex 服务端动态
            // 门禁在官方 latest 尚未发布时不会显示“当前版本等于目标版本”的矛盾提示。
            status.latest_version = None;
            return;
        }
        status.latest_version = latest;
        status.update_available = true;
        let source = match status.install_source.as_deref() {
            Some("brew") => Some("brew"),
            Some("npm") => Some("npm"),
            Some("script") => Some("script"),
            _ => None,
        };
        status.install_action = install_action_for(
            backend,
            source,
            status.npm_available,
            official_script_supported(backend),
        );
        // CLI 已满足最低门禁，missing 类提示不能再误报；认证类提示仍然有效。
        if status
            .setup_hint
            .is_some_and(|hint| hint.ends_with("_cli_missing"))
        {
            status.setup_hint = None;
        }
    }

    pub(super) async fn refresh(&self, backend: AgentBackend, force: bool) -> Option<String> {
        if !backend.is_acp() {
            return None;
        }
        if !force {
            if let Some(cached) = self.fresh_entry(backend) {
                return cached.version;
            }
        }
        let Some(gate) = self.gates.get(&backend) else {
            return None;
        };
        let _guard = gate.lock().await;
        if !force {
            if let Some(cached) = self.fresh_entry(backend) {
                return cached.version;
            }
        }

        let operation_id = diagnostics::operation_id("latest-probe");
        let started = Instant::now();
        let result = fetch_latest_version(&self.client, backend).await;
        let version = match result {
            Ok(version) => {
                diagnostics::write(
                    &operation_id,
                    "latest:complete",
                    format!(
                        "agent={} version={version} elapsed_ms={}",
                        backend.agent_id().unwrap_or("unknown"),
                        started.elapsed().as_millis()
                    ),
                );
                Some(version)
            }
            Err(error) => {
                // latest 仅用于主动升级提示。离线、超时或厂商接口异常时继续沿用最低
                // 兼容版本门禁，不能让已经可用的本地 Agent 因联网失败而不可用。
                diagnostics::write(
                    &operation_id,
                    "latest:failed",
                    format!(
                        "agent={} elapsed_ms={} error={error:#}",
                        backend.agent_id().unwrap_or("unknown"),
                        started.elapsed().as_millis()
                    ),
                );
                None
            }
        };
        self.entries.write().insert(
            backend,
            LatestVersionEntry {
                checked_at: Instant::now(),
                version: version.clone(),
            },
        );
        version
    }

    fn fresh_entry(&self, backend: AgentBackend) -> Option<LatestVersionEntry> {
        self.entries
            .read()
            .get(&backend)
            .filter(|entry| entry.checked_at.elapsed() < CACHE_TTL)
            .cloned()
    }
}

impl AcpPool {
    /// latest 走异步 HTTP 并按 Agent 缓存；本地 status_for 会同步 spawn CLI，必须
    /// 经 spawn_blocking。没有合规本地 CLI 时不发出无意义的 latest 请求。
    pub(super) async fn status_for_async(&self, backend: AgentBackend) -> CodexAcpStatus {
        let pool = self.clone();
        let mut status = tokio::task::spawn_blocking(move || pool.status_for(backend))
            .await
            .expect("ACP 状态探测任务异常退出");
        if status.codex_available {
            self.latest_version_probe.refresh(backend, false).await;
        }
        self.latest_version_probe
            .apply_to_status(backend, &mut status);
        status
    }

    pub async fn status_for_agent(&self, agent_id: &str) -> Result<CodexAcpStatus> {
        let backend = AgentBackend::parse(Some(agent_id))?;
        if !backend.is_acp() {
            bail!("Agent 不是 ACP 后端: {agent_id}");
        }
        if backend == AgentBackend::CodexAcp {
            self.refresh_runtime_probe(false).await;
        }
        Ok(self.status_for_async(backend).await)
    }
}

/// 升级完成后校验 CLI 版本门禁：仍不合规说明包管理器源没有提供更新版本。
/// 官方 latest 提醒是可暂缓的 advisory：包管理器源滞后于官网发布是常态，
/// 升级后仍低于 latest 不算失败（前端会继续显示升级提醒），不能写入
/// sticky runtime error 把可用的 Agent 标成错误状态。
pub(super) fn ensure_agent_cli_ready(backend: AgentBackend, status: &CodexAcpStatus) -> Result<()> {
    let ready = match backend {
        // Codex 的安装动作只负责 CLI；Bridge 缺失由独立状态处理，不能把它误报成
        // CLI 安装失败。动态升级门禁则必须保持失败，直到实际版本发生变化。
        AgentBackend::CodexAcp => status.codex_available && !status.update_required,
        _ => status.installed,
    };
    if ready {
        return Ok(());
    }
    if backend == AgentBackend::CodexAcp && status.update_required {
        bail!(
            "Codex 升级流程已完成，但检测到的版本 {} 未发生变化，仍无法支持所选模型；请确认官方或包管理器已提供更新版本",
            status.codex_version.as_deref().unwrap_or("未知版本")
        );
    }
    bail!(
        "{} 升级流程已完成，但检测到的版本 {} 仍低于最低要求 {}；\
         请检查包管理器源，或按官方文档手动升级",
        backend.display_name(),
        status.codex_version.as_deref().unwrap_or("未知版本"),
        status.min_version
    )
}

fn latest_url(backend: AgentBackend) -> Result<&'static str> {
    match backend {
        AgentBackend::ClaudeAcp => Ok(CLAUDE_LATEST_URL),
        AgentBackend::KimiAcp => Ok(KIMI_LATEST_URL),
        AgentBackend::CodexAcp | AgentBackend::Deepseek => {
            bail!("该 Agent 不使用纯文本 latest 接口")
        }
    }
}

async fn fetch_latest_version(client: &Client, backend: AgentBackend) -> Result<String> {
    if backend == AgentBackend::CodexAcp {
        return fetch_codex_latest_version(client).await;
    }
    let url = latest_url(backend)?;
    let body = fetch_limited_body(client, backend, url).await?;
    parse_latest_response(backend, &body)
}

/// OpenAI 官方安装器优先读取 releases.openai.com，并在不可达时回退官方 GitHub
/// Release。这里并发竞速两个官方来源：任一先成功即返回，避免 113 一类网络环境
/// 等满主源超时；先失败时继续等待另一个来源。
async fn fetch_codex_latest_version(client: &Client) -> Result<String> {
    let release_metadata = async {
        let body = fetch_limited_body(client, AgentBackend::CodexAcp, CODEX_LATEST_URL).await?;
        parse_latest_response(AgentBackend::CodexAcp, &body)
    };
    let github_release = fetch_codex_github_release(client);
    tokio::pin!(release_metadata, github_release);
    tokio::select! {
        result = &mut release_metadata => match result {
            Ok(version) => Ok(version),
            Err(primary) => github_release.await.with_context(|| {
                format!("Codex 官方主源与 GitHub Release 均不可用；主源错误：{primary:#}")
            }),
        },
        result = &mut github_release => match result {
            Ok(version) => Ok(version),
            Err(fallback) => release_metadata.await.with_context(|| {
                format!("Codex 官方 GitHub Release 与主源均不可用；GitHub 错误：{fallback:#}")
            }),
        },
    }
}

async fn fetch_codex_github_release(client: &Client) -> Result<String> {
    let response = client
        .get(CODEX_GITHUB_LATEST_URL)
        .send()
        .await
        .context("查询 Codex 官方 GitHub Release 失败")?
        .error_for_status()
        .context("Codex 官方 GitHub Release 返回错误")?;
    parse_codex_release_url(response.url())
}

async fn fetch_limited_body(client: &Client, backend: AgentBackend, url: &str) -> Result<Vec<u8>> {
    let mut response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("查询 {} 官方最新版本失败", backend.display_name()))?
        .error_for_status()
        .with_context(|| format!("{} 官方最新版本接口返回错误", backend.display_name()))?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        bail!("{} 官方最新版本响应过大", backend.display_name());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .with_context(|| format!("读取 {} 官方最新版本响应失败", backend.display_name()))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            bail!("{} 官方最新版本响应超过大小限制", backend.display_name());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn parse_codex_release_url(url: &reqwest::Url) -> Result<String> {
    if url.scheme() != "https" || url.host_str() != Some("github.com") {
        bail!("Codex GitHub latest 跳转到了非官方地址");
    }
    let tag = url
        .path()
        .strip_prefix("/openai/codex/releases/tag/")
        .context("Codex GitHub latest 未跳转到版本页面")?;
    parse_codex_tag(tag)
}

fn parse_codex_tag(tag: &str) -> Result<String> {
    let raw = tag
        .strip_prefix("rust-v")
        .context("Codex latest tag 格式不受支持")?;
    normalize_semver(raw).context("Codex latest tag 不是三段数字版本")
}

fn parse_latest_response(backend: AgentBackend, body: &[u8]) -> Result<String> {
    let raw = match backend {
        AgentBackend::CodexAcp => {
            let value: Value =
                serde_json::from_slice(body).context("解析 Codex latest JSON 失败")?;
            return parse_codex_tag(
                value["tag_name"]
                    .as_str()
                    .context("Codex latest JSON 缺少 tag_name")?,
            );
        }
        AgentBackend::ClaudeAcp | AgentBackend::KimiAcp => std::str::from_utf8(body)
            .context("官方 latest 响应不是 UTF-8")?
            .trim()
            .to_string(),
        AgentBackend::Deepseek => bail!("品悟不是外部 ACP Agent"),
    };
    normalize_semver(&raw).context("官方 latest 响应不是三段数字版本")
}

fn semver_parts(raw: &str) -> Option<[u64; 3]> {
    let token = raw.split_whitespace().next()?;
    let mut parts = token.split('.');
    let parsed = [
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ];
    parts.next().is_none().then_some(parsed)
}

fn normalize_semver(raw: &str) -> Option<String> {
    let [major, minor, patch] = semver_parts(raw)?;
    Some(format!("{major}.{minor}.{patch}"))
}

fn update_available(installed: &str, latest: &str) -> bool {
    match (semver_parts(installed), semver_parts(latest)) {
        (Some(installed), Some(latest)) => installed < latest,
        _ => false,
    }
}

pub(super) fn latest_update_available(
    minimum_ready: bool,
    installed: Option<&str>,
    latest: Option<&str>,
) -> bool {
    minimum_ready
        && installed
            .zip(latest)
            .is_some_and(|(installed, latest)| update_available(installed, latest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready_codex_status() -> CodexAcpStatus {
        CodexAcpStatus {
            agent_id: "codex",
            agent_name: "Codex",
            version: Some("0.144.6".to_string()),
            latest_version: None,
            installed: true,
            update_available: false,
            update_required: false,
            bridge_ready: true,
            adapter_path: None,
            node_available: true,
            node_version: Some("20.0.0".to_string()),
            node_supported: true,
            npm_available: false,
            codex_available: true,
            codex_path: None,
            codex_version: Some("0.144.6".to_string()),
            runtime_source: Some("system"),
            min_codex_version: "0.144.6",
            min_version: "0.144.6",
            install_action: "none",
            install_source: Some("script".to_string()),
            brew_available: false,
            system_codex_incompatible: false,
            authenticated: true,
            login_in_progress: false,
            login_url: None,
            login_code: None,
            login_input_required: false,
            installing: false,
            error: None,
            install_command: None,
            install_latest_line: None,
            setup_hint: None,
        }
    }

    #[test]
    fn parses_each_official_latest_format() {
        assert_eq!(
            parse_latest_response(
                AgentBackend::CodexAcp,
                br#"{"tag_name":"rust-v0.146.0","assets":[]}"#,
            )
            .unwrap(),
            "0.146.0"
        );
        assert_eq!(
            parse_latest_response(AgentBackend::ClaudeAcp, b"2.1.220\n").unwrap(),
            "2.1.220"
        );
        assert_eq!(
            parse_latest_response(AgentBackend::KimiAcp, b"0.31.1").unwrap(),
            "0.31.1"
        );
        assert_eq!(
            parse_codex_release_url(
                &reqwest::Url::parse("https://github.com/openai/codex/releases/tag/rust-v0.146.0")
                    .unwrap()
            )
            .unwrap(),
            "0.146.0"
        );
    }

    #[test]
    fn rejects_invalid_or_unexpected_latest_payloads() {
        assert!(
            parse_latest_response(AgentBackend::CodexAcp, br#"{"tag_name":"v0.146.0"}"#).is_err()
        );
        assert!(parse_latest_response(AgentBackend::ClaudeAcp, b"<html>failed</html>").is_err());
        assert!(parse_latest_response(AgentBackend::KimiAcp, b"0.31").is_err());
        assert!(parse_codex_release_url(
            &reqwest::Url::parse("https://github.com/openai/codex/releases/latest").unwrap()
        )
        .is_err());
        assert!(parse_codex_release_url(
            &reqwest::Url::parse("https://example.com/openai/codex/releases/tag/rust-v0.146.0")
                .unwrap()
        )
        .is_err());
    }

    #[test]
    fn compares_current_cli_output_with_latest_semver() {
        assert!(update_available("0.144.6", "0.146.0"));
        assert!(update_available("2.1.163 (Claude Code)", "2.1.220"));
        assert!(!update_available("2.1.300 (Claude Code)", "2.1.220"));
        assert!(!update_available("0.31.1", "0.31.1"));
        assert!(!update_available("unknown", "0.31.1"));
        assert!(latest_update_available(
            true,
            Some("0.30.0"),
            Some("0.31.1")
        ));
        assert!(!latest_update_available(true, Some("0.30.0"), None));
        assert!(!latest_update_available(
            false,
            Some("0.30.0"),
            Some("0.31.1")
        ));
    }

    #[test]
    fn latest_update_is_advisory_without_weakening_mandatory_gates() {
        let probe = LatestVersionProbe::new().unwrap();
        probe.entries.write().insert(
            AgentBackend::CodexAcp,
            LatestVersionEntry {
                checked_at: Instant::now(),
                version: Some("0.146.0".to_string()),
            },
        );
        let mut advisory = ready_codex_status();
        probe.apply_to_status(AgentBackend::CodexAcp, &mut advisory);
        assert!(advisory.installed);
        assert!(advisory.update_available);
        assert!(!advisory.update_required);
        assert_eq!(advisory.latest_version.as_deref(), Some("0.146.0"));
        assert_eq!(advisory.install_action, "official_script");

        let mut mandatory = ready_codex_status();
        mandatory.installed = false;
        mandatory.update_required = true;
        probe.apply_to_status(AgentBackend::CodexAcp, &mut mandatory);
        assert!(!mandatory.installed);
        assert!(mandatory.update_available);
        assert!(mandatory.update_required);
    }

    #[test]
    fn advisory_update_keeps_auth_hint_but_clears_missing_hint() {
        let probe = LatestVersionProbe::new().unwrap();
        let mut unauthenticated = ready_codex_status();
        unauthenticated.setup_hint = Some("claude_auth_required");
        probe.apply_to_status(AgentBackend::ClaudeAcp, &mut unauthenticated);
        assert!(
            !unauthenticated.update_available,
            "latest 未探测时不应改动提示"
        );
        probe.entries.write().insert(
            AgentBackend::ClaudeAcp,
            LatestVersionEntry {
                checked_at: Instant::now(),
                version: Some("9.9.9".to_string()),
            },
        );
        probe.apply_to_status(AgentBackend::ClaudeAcp, &mut unauthenticated);
        assert!(unauthenticated.update_available);
        assert_eq!(
            unauthenticated.setup_hint,
            Some("claude_auth_required"),
            "认证提示在 latest 提醒下必须保留"
        );

        let mut missing = ready_codex_status();
        missing.setup_hint = Some("claude_cli_missing");
        probe.apply_to_status(AgentBackend::ClaudeAcp, &mut missing);
        assert_eq!(
            missing.setup_hint, None,
            "CLI 已满足最低门禁时 missing 提示必须清除"
        );
    }

    #[test]
    fn caches_success_and_failure_without_hiding_expiry() {
        let probe = LatestVersionProbe::new().unwrap();
        probe.entries.write().insert(
            AgentBackend::CodexAcp,
            LatestVersionEntry {
                checked_at: Instant::now(),
                version: Some("0.146.0".to_string()),
            },
        );
        assert_eq!(
            probe.fresh_entry(AgentBackend::CodexAcp).unwrap().version,
            Some("0.146.0".to_string())
        );
        probe.entries.write().insert(
            AgentBackend::ClaudeAcp,
            LatestVersionEntry {
                checked_at: Instant::now(),
                version: None,
            },
        );
        assert!(probe.fresh_entry(AgentBackend::ClaudeAcp).is_some());
        probe.entries.write().insert(
            AgentBackend::KimiAcp,
            LatestVersionEntry {
                checked_at: Instant::now() - CACHE_TTL,
                version: Some("0.31.1".to_string()),
            },
        );
        assert!(probe.fresh_entry(AgentBackend::KimiAcp).is_none());
    }
}

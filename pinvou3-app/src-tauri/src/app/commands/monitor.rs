use super::prelude::*;

use crate::core::model_endpoint::LocalServerKind;

/// Monitor 视图完整数据。**按需采样**——前端只在监控页面 mount 时启 1s
/// interval 调本 command，每次都重新跑 sample_all。GPU util 瞬时易错过推理
/// 峰，前端维护 5 个值滑窗 max 弥补。
#[tauri::command]
pub async fn get_monitor_snapshot(
    monitor: State<'_, MonitorState>,
) -> Result<MonitorSnapshot, String> {
    let snapshot = crate::features::monitor::sample_all(
        &monitor,
        &crate::features::monitor::vllm_base_url(),
        crate::features::monitor::vllm_configured_model(),
    )
    .await;
    Ok(snapshot)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverLocalVllmRequest {
    pub current_base_url: Option<String>,
    pub saved_base_url: Option<String>,
    /// Probe port explicitly specified by the user in the local-model panel. It is
    /// only ever joined onto 127.0.0.1 for single-address probing; user input never
    /// contributes a host name or network range, so the probe surface does not grow
    /// with input. Zero and absence both mean "unspecified"; values beyond u16 are
    /// rejected by deserialization.
    pub custom_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalVllmModelEntry {
    pub id: String,
    /// 是否已加载到内存：`None` = 未知。Ollama/LM Studio 的列表接口返回全部
    /// 已下载模型且均为 JIT 加载，前端据此区分"就绪"与"未加载"，避免把未加载
    /// 的大模型自动填充为可用模型（首次推理会静默载入内存）。
    pub loaded: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalVllmCandidate {
    pub base_url: String,
    pub status: VllmStatus,
    pub provider: String,
    pub label: String,
    pub model: Option<String>,
    pub models: Vec<LocalVllmModelEntry>,
    pub max_model_len: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalVllmDiscovery {
    pub candidates: Vec<LocalVllmCandidate>,
}

/// Manually probe local OpenAI-compatible model services. Probes only the small
/// whitelist of candidate addresses plus the single user-specified custom port;
/// no port scanning, no LAN probing.
#[tauri::command]
pub async fn discover_local_vllm(
    request: Option<DiscoverLocalVllmRequest>,
) -> Result<LocalVllmDiscovery, String> {
    let mut candidates = Vec::new();
    for base_url in discovery_probe_urls(request.as_ref()) {
        let Some((probe, provider, label)) = probe_local_candidate(&base_url).await else {
            continue;
        };
        // Per-framework probing: Ollama / LM Studio list endpoints return every
        // downloaded model, so their native endpoints are needed to tell loaded
        // from downloaded; for vLLM and friends served means loaded.
        // probe_local_candidate already picked the endpoint; this loop only
        // maps protocol fields.
        let models = probe
            .models
            .iter()
            .map(|model| LocalVllmModelEntry {
                id: model.id.clone(),
                loaded: model.loaded,
            })
            .collect::<Vec<_>>();
        let first = probe.models.first();
        candidates.push(LocalVllmCandidate {
            base_url,
            status: VllmStatus::Ready,
            provider: provider.to_string(),
            label: label.to_string(),
            model: first.map(|model| model.id.clone()),
            models,
            max_model_len: first.and_then(|model| model.max_model_len),
        });
    }
    Ok(LocalVllmDiscovery { candidates })
}

/// Assemble the probe URL list: request-carried current/saved URLs (whitelist
/// normalized) → fixed default ports → the request-specified custom port,
/// deduplicated in order. The custom port is only ever joined onto the
/// 127.0.0.1 host and bypasses whitelist normalization (the whitelist only
/// admits known ports and would drop it).
fn discovery_probe_urls(request: Option<&DiscoverLocalVllmRequest>) -> Vec<String> {
    let mut urls = Vec::new();
    if let Some(req) = request {
        push_local_vllm_candidate(&mut urls, req.current_base_url.as_deref());
        push_local_vllm_candidate(&mut urls, req.saved_base_url.as_deref());
    }
    for port in [8000u16, 8001, 8002, 11434, 1234] {
        push_local_vllm_candidate(&mut urls, Some(&format!("http://127.0.0.1:{port}/v1")));
    }
    if let Some(port) = request
        .and_then(|req| req.custom_port)
        .filter(|port| *port > 0)
    {
        let url = format!("http://127.0.0.1:{port}/v1");
        if !urls.iter().any(|existing| existing == &url) {
            urls.push(url);
        }
    }
    urls
}

/// Probe a single candidate address; returns the model list and the service
/// identity (provider, label). Known default ports keep the port-to-kind fast
/// path; other ports (user-specified custom ports) probe the server kind first
/// and then choose the list endpoint, so on-demand services such as Ollama keep
/// their "downloaded but not loaded" state instead of being mislabeled.
async fn probe_local_candidate(
    base_url: &str,
) -> Option<(
    crate::core::model_endpoint::OpenAiModelsProbe,
    &'static str,
    &'static str,
)> {
    let identity = local_model_provider_for_url(base_url);
    match local_port_of(base_url) {
        Some(11434) => crate::core::model_endpoint::probe_ollama_models(base_url, None)
            .await
            .map(|probe| (probe, identity.0, identity.1)),
        Some(1234) => crate::core::model_endpoint::probe_lmstudio_models(base_url)
            .await
            .map(|probe| (probe, identity.0, identity.1)),
        Some(8000..=8002) => probe_openai_served_models(base_url)
            .await
            .map(|probe| (probe, identity.0, identity.1)),
        _ => probe_custom_port_candidate(base_url).await,
    }
}

/// Custom-port candidate: probe the server kind first (candidate probes run in
/// parallel, one round trip), then pick the model list endpoint by kind —
/// Ollama / LM Studio use their native endpoints to tell loaded from
/// downloaded-only, while everything else (vLLM / SGLang / llama.cpp / KoboldCpp
/// / LMDeploy / Docker Model Runner / generic endpoints) treats served as
/// loaded. When the kind cannot be recognized the identity falls back to a
/// generic OpenAI-compatible endpoint; a failed list probe means the address is
/// offline.
async fn probe_custom_port_candidate(
    base_url: &str,
) -> Option<(
    crate::core::model_endpoint::OpenAiModelsProbe,
    &'static str,
    &'static str,
)> {
    let kind = crate::core::model_endpoint::probe_local_server_kind(base_url, None).await;
    let identity = local_model_provider_for_kind(kind);
    let probe = match kind {
        LocalServerKind::Ollama => {
            crate::core::model_endpoint::probe_ollama_models(base_url, None).await
        }
        LocalServerKind::LmStudio => {
            crate::core::model_endpoint::probe_lmstudio_models(base_url).await
        }
        _ => probe_openai_served_models(base_url).await,
    };
    probe.map(|probe| (probe, identity.0, identity.1))
}

/// OpenAI-compatible `/v1/models` list probe; for vLLM-like services served means loaded.
async fn probe_openai_served_models(
    base_url: &str,
) -> Option<crate::core::model_endpoint::OpenAiModelsProbe> {
    let mut probe = crate::core::model_endpoint::probe_openai_models(base_url).await?;
    for model in &mut probe.models {
        model.loaded = Some(true);
    }
    Some(probe)
}

fn push_local_vllm_candidate(out: &mut Vec<String>, raw: Option<&str>) {
    let Some(raw) = raw else {
        return;
    };
    let Some(url) = normalize_local_vllm_base_url(raw) else {
        return;
    };
    if !out.iter().any(|existing| existing == &url) {
        out.push(url);
    }
}

fn normalize_local_vllm_base_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let rest = trimmed.strip_prefix("http://")?;
    let host_port = rest.split('/').next()?;
    let (host, port) = host_port.rsplit_once(':')?;
    if !matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
        return None;
    }
    let port: u16 = port.parse().ok()?;
    if !matches!(port, 8000..=8002 | 11434 | 1234) {
        return None;
    }
    Some(format!("http://{host}:{port}/v1"))
}

fn local_port_of(base_url: &str) -> Option<u16> {
    base_url
        .trim()
        .trim_end_matches('/')
        .split('/')
        .nth(2)
        .and_then(|host_port| host_port.rsplit_once(':').map(|(_, port)| port))
        .and_then(|port| port.parse::<u16>().ok())
}

fn local_model_provider_for_url(base_url: &str) -> (&'static str, &'static str) {
    match local_port_of(base_url) {
        Some(11434) => ("ollama", "Ollama"),
        Some(1234) => ("lm_studio", "LM Studio"),
        Some(8000..=8002) => ("vllm", "vLLM"),
        _ => ("openai_compatible", "OpenAI Compatible"),
    }
}

/// Service identity for custom-port candidates: derived from the probed kind
/// (not port conventions). Generic means no known framework was recognized and
/// the endpoint is treated as a generic OpenAI-compatible service, matching the
/// legacy behavior for ports absent from the port table.
fn local_model_provider_for_kind(kind: LocalServerKind) -> (&'static str, &'static str) {
    match kind {
        LocalServerKind::Vllm => ("vllm", "vLLM"),
        LocalServerKind::Ollama => ("ollama", "Ollama"),
        LocalServerKind::LmStudio => ("lm_studio", "LM Studio"),
        LocalServerKind::Sglang => ("sglang", "SGLang"),
        LocalServerKind::LlamaCpp => ("llamacpp", "llama.cpp"),
        LocalServerKind::KoboldCpp => ("koboldcpp", "KoboldCpp"),
        LocalServerKind::LmDeploy => ("lmdeploy", "LMDeploy"),
        LocalServerKind::DockerModelRunner => ("dockermodelrunner", "Docker Model Runner"),
        LocalServerKind::Generic => ("openai_compatible", "OpenAI Compatible"),
    }
}

/// ChatRoom 顶部 live dot 简版指示：vLLM 是否在线。
#[derive(Debug, Clone, Serialize)]
pub struct BackendStatus {
    pub vllm_online: bool,
    pub last_check_ms: u64,
    /// vLLM 真实上下文窗口（前端 token 进度数据的分母）。
    /// 随 live-dot 轮询下发，监控页未打开时也能保持准确。
    pub max_model_len: Option<u32>,
}

#[tauri::command]
pub async fn get_backend_status(
    _monitor: State<'_, MonitorState>,
) -> Result<BackendStatus, String> {
    // Lightweight: 只 probe 当前 active model,不跑 nvidia-smi / RAM 采样。
    let vllm = crate::features::monitor::active_model_snapshot().await;
    let vllm_online = vllm.as_ref().is_some_and(|v| {
        v.health_status == "verified" && matches!(v.status, VllmStatus::Ready | VllmStatus::Busy)
    });
    let max_model_len = vllm.as_ref().and_then(|v| v.max_model_len);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    Ok(BackendStatus {
        vllm_online,
        last_check_ms: now_ms,
        max_model_len,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_local_model_base_url_allows_known_loopback_ports() {
        assert_eq!(
            normalize_local_vllm_base_url("http://127.0.0.1:8000/v1"),
            Some("http://127.0.0.1:8000/v1".to_string())
        );
        assert_eq!(
            normalize_local_vllm_base_url("http://127.0.0.1:8001/v1"),
            Some("http://127.0.0.1:8001/v1".to_string())
        );
        assert_eq!(
            normalize_local_vllm_base_url("http://127.0.0.1:8002/v1"),
            Some("http://127.0.0.1:8002/v1".to_string())
        );
        assert_eq!(
            normalize_local_vllm_base_url("http://127.0.0.1:11434/v1"),
            Some("http://127.0.0.1:11434/v1".to_string())
        );
        assert_eq!(
            normalize_local_vllm_base_url("http://localhost:1234/v1"),
            Some("http://localhost:1234/v1".to_string())
        );
    }

    #[test]
    fn normalize_local_model_base_url_rejects_non_whitelisted_targets() {
        assert_eq!(
            normalize_local_vllm_base_url("http://127.0.0.1:9999/v1"),
            None
        );
        assert_eq!(
            normalize_local_vllm_base_url("http://192.168.1.2:8000/v1"),
            None
        );
        assert_eq!(
            normalize_local_vllm_base_url("https://example.com/v1"),
            None
        );
    }

    #[test]
    fn local_model_provider_uses_known_default_ports() {
        assert_eq!(
            local_model_provider_for_url("http://127.0.0.1:8000/v1"),
            ("vllm", "vLLM")
        );
        assert_eq!(
            local_model_provider_for_url("http://127.0.0.1:11434/v1"),
            ("ollama", "Ollama")
        );
        assert_eq!(
            local_model_provider_for_url("http://127.0.0.1:1234/v1"),
            ("lm_studio", "LM Studio")
        );
    }

    #[test]
    fn discovery_urls_cover_defaults_then_custom_port() {
        let request = DiscoverLocalVllmRequest {
            current_base_url: None,
            saved_base_url: None,
            custom_port: Some(8080),
        };
        assert_eq!(
            discovery_probe_urls(Some(&request)),
            vec![
                "http://127.0.0.1:8000/v1".to_string(),
                "http://127.0.0.1:8001/v1".to_string(),
                "http://127.0.0.1:8002/v1".to_string(),
                "http://127.0.0.1:11434/v1".to_string(),
                "http://127.0.0.1:1234/v1".to_string(),
                "http://127.0.0.1:8080/v1".to_string(),
            ]
        );
    }

    #[test]
    fn discovery_urls_dedup_custom_port_hitting_a_default() {
        let request = DiscoverLocalVllmRequest {
            current_base_url: None,
            saved_base_url: None,
            custom_port: Some(8001),
        };
        let urls = discovery_probe_urls(Some(&request));
        assert_eq!(
            urls.iter()
                .filter(|url| **url == "http://127.0.0.1:8001/v1")
                .count(),
            1
        );
        assert_eq!(urls.len(), 5);
    }

    #[test]
    fn discovery_urls_drop_unspecified_or_zero_custom_port() {
        let none = DiscoverLocalVllmRequest {
            current_base_url: None,
            saved_base_url: None,
            custom_port: None,
        };
        assert_eq!(discovery_probe_urls(Some(&none)).len(), 5);
        let zero = DiscoverLocalVllmRequest {
            current_base_url: None,
            saved_base_url: None,
            custom_port: Some(0),
        };
        assert_eq!(discovery_probe_urls(Some(&zero)).len(), 5);
        assert_eq!(discovery_probe_urls(None).len(), 5);
    }

    #[test]
    fn discover_request_parses_camel_case_custom_port() {
        let request: DiscoverLocalVllmRequest = serde_json::from_str(
            r#"{"currentBaseUrl":null,"savedBaseUrl":null,"customPort":8080}"#,
        )
        .expect("camelCase customPort must deserialize");
        assert_eq!(request.custom_port, Some(8080));
    }

    #[test]
    fn local_model_provider_for_kind_maps_discriminated_services() {
        assert_eq!(
            local_model_provider_for_kind(LocalServerKind::Vllm),
            ("vllm", "vLLM")
        );
        assert_eq!(
            local_model_provider_for_kind(LocalServerKind::Ollama),
            ("ollama", "Ollama")
        );
        assert_eq!(
            local_model_provider_for_kind(LocalServerKind::LmStudio),
            ("lm_studio", "LM Studio")
        );
        assert_eq!(
            local_model_provider_for_kind(LocalServerKind::Sglang),
            ("sglang", "SGLang")
        );
        assert_eq!(
            local_model_provider_for_kind(LocalServerKind::LlamaCpp),
            ("llamacpp", "llama.cpp")
        );
        assert_eq!(
            local_model_provider_for_kind(LocalServerKind::KoboldCpp),
            ("koboldcpp", "KoboldCpp")
        );
        assert_eq!(
            local_model_provider_for_kind(LocalServerKind::LmDeploy),
            ("lmdeploy", "LMDeploy")
        );
        assert_eq!(
            local_model_provider_for_kind(LocalServerKind::DockerModelRunner),
            ("dockermodelrunner", "Docker Model Runner")
        );
        // Unrecognized framework falls back to the legacy behavior for ports absent from the port table.
        assert_eq!(
            local_model_provider_for_kind(LocalServerKind::Generic),
            ("openai_compatible", "OpenAI Compatible")
        );
    }
}

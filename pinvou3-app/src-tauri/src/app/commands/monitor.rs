use super::prelude::*;

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

/// 手动探测本机 OpenAI-compatible 模型服务。只探小白名单候选地址；
/// 不做端口扫描,不探局域网。
#[tauri::command]
pub async fn discover_local_vllm(
    request: Option<DiscoverLocalVllmRequest>,
) -> Result<LocalVllmDiscovery, String> {
    let mut urls = Vec::new();
    if let Some(req) = request {
        push_local_vllm_candidate(&mut urls, req.current_base_url.as_deref());
        push_local_vllm_candidate(&mut urls, req.saved_base_url.as_deref());
    }
    for port in [8000u16, 8001, 8002, 11434, 1234] {
        push_local_vllm_candidate(&mut urls, Some(&format!("http://127.0.0.1:{port}/v1")));
    }

    let mut candidates = Vec::new();
    for base_url in urls {
        // 按框架选探测方式：Ollama / LM Studio 的列表接口返回全部已下载模型，
        // 需用各自原生接口区分已加载；vLLM 等 served 即已加载。
        let probe = match local_port_of(&base_url) {
            Some(11434) => crate::features::monitor::probe_ollama_models(&base_url).await,
            Some(1234) => crate::features::monitor::probe_lmstudio_models(&base_url).await,
            _ => crate::features::monitor::probe_openai_models(&base_url)
                .await
                .map(|mut probe| {
                    for model in &mut probe.models {
                        model.loaded = Some(true);
                    }
                    probe
                }),
        };
        if let Some(probe) = probe {
            let (provider, label) = local_model_provider_for_url(&base_url);
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
    }
    Ok(LocalVllmDiscovery { candidates })
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
        Some(8000 | 8001 | 8002) => ("vllm", "vLLM"),
        _ => ("openai_compatible", "OpenAI Compatible"),
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
}

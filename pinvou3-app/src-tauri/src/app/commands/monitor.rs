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
pub struct LocalVllmCandidate {
    pub base_url: String,
    pub status: VllmStatus,
    pub model: Option<String>,
    pub max_model_len: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalVllmDiscovery {
    pub candidates: Vec<LocalVllmCandidate>,
}

/// 手动探测本机 vLLM。只探小白名单候选地址；不做端口扫描,不探局域网。
#[tauri::command]
pub async fn discover_local_vllm(
    request: Option<DiscoverLocalVllmRequest>,
) -> Result<LocalVllmDiscovery, String> {
    let mut urls = Vec::new();
    if let Some(req) = request {
        push_local_vllm_candidate(&mut urls, req.current_base_url.as_deref());
        push_local_vllm_candidate(&mut urls, req.saved_base_url.as_deref());
    }
    for port in [8000u16, 8001, 8002] {
        push_local_vllm_candidate(&mut urls, Some(&format!("http://127.0.0.1:{port}/v1")));
    }

    let mut candidates = Vec::new();
    for base_url in urls {
        if let Some(snapshot) = crate::features::monitor::vllm_snapshot(&base_url, None).await {
            candidates.push(LocalVllmCandidate {
                base_url: snapshot.upstream,
                status: snapshot.status,
                model: snapshot.model,
                max_model_len: snapshot.max_model_len,
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
    if !matches!(port, 8000 | 8001 | 8002) {
        return None;
    }
    Some(format!("http://{host}:{port}/v1"))
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

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LocalVllmEngineState {
    Ready,
    Starting,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalVllmSetupStatus {
    pub eligible: bool,
    pub is_megacube: bool,
    pub has_packages: bool,
    pub vllm_online: bool,
    pub engine_state: LocalVllmEngineState,
    pub already_bootstrapped: bool,
    pub may_offer_setup: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct BootstrapResult {
    pub base_url: String,
    pub model: String,
}

/// 社区版不探测或启动厂商预装环境。
///
/// 用户仍可在设置中连接自己已经运行的 OpenAI-compatible / vLLM endpoint。
pub async fn detect_local_vllm_setup() -> Result<LocalVllmSetupStatus, String> {
    Ok(LocalVllmSetupStatus {
        eligible: false,
        is_megacube: false,
        has_packages: false,
        vllm_online: false,
        engine_state: LocalVllmEngineState::Stopped,
        already_bootstrapped: false,
        may_offer_setup: false,
    })
}

pub fn decline_local_vllm_setup() -> Result<(), String> {
    Ok(())
}

pub async fn bootstrap_local_vllm(_app: tauri::AppHandle) -> Result<BootstrapResult, String> {
    Err("社区版不提供厂商预装环境的自动启用；请在设置中连接你自己的 vLLM endpoint。".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn community_setup_never_offers_vendor_bootstrap() {
        let status = detect_local_vllm_setup().await.unwrap();
        assert!(!status.eligible);
        assert!(!status.has_packages);
        assert!(!status.may_offer_setup);
    }
}

use super::prelude::*;
use crate::features::local_llm::setup as local_llm_domain;
use local_llm_domain::*;

async_command_passthrough!(local_llm_domain, detect_local_vllm_setup() -> Result<LocalVllmSetupStatus, String>);
sync_command_passthrough!(local_llm_domain, decline_local_vllm_setup() -> Result<(), String>);
async_command_passthrough!(local_llm_domain, bootstrap_local_vllm(app: AppHandle) -> Result<BootstrapResult, String>);

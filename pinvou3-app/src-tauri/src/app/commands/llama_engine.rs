//! 本地多模态引擎命令（薄传输层：透传 features::llama_engine 域函数）。

use super::prelude::*;
use crate::features::llama_engine as llama_engine_domain;
use llama_engine_domain::*;

sync_command_passthrough!(llama_engine_domain, llama_engine_status() -> LlamaEngineStatus);
async_command_passthrough!(llama_engine_domain, llama_engine_install_engine(app: AppHandle) -> Result<(), String>);
async_command_passthrough!(llama_engine_domain, llama_engine_install_model(app: AppHandle, model: String) -> Result<(), String>);
sync_command_passthrough!(llama_engine_domain, llama_engine_cancel_download());
async_command_passthrough!(llama_engine_domain, llama_engine_start(app: AppHandle, model: String, device: String) -> Result<(), String>);
sync_command_passthrough!(llama_engine_domain, llama_engine_stop());

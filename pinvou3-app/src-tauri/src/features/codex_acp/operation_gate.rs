use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{bail, Result};
use serde_json::json;

use super::{events::patch_acp_state, AcpPool, AcpSession, CodexAcpSessionInfo};

/// Owns the session's configuration slot until the operation finishes. Drop
/// based release keeps cancellation and early-return paths from leaving the
/// session permanently stuck in `configuring`.
pub(super) struct ConfigurationGuard<'a> {
    configuring: &'a AtomicBool,
}

impl Drop for ConfigurationGuard<'_> {
    fn drop(&mut self) {
        self.configuring.store(false, Ordering::Release);
    }
}

pub(super) fn begin_configuration<'a>(
    busy: &AtomicBool,
    configuring: &'a AtomicBool,
    busy_error: &str,
) -> Result<ConfigurationGuard<'a>> {
    if busy.load(Ordering::Acquire) {
        bail!(busy_error.to_string());
    }
    if configuring.swap(true, Ordering::AcqRel) {
        bail!("ACP 会话已有配置正在同步");
    }
    if busy.load(Ordering::Acquire) {
        configuring.store(false, Ordering::Release);
        bail!(busy_error.to_string());
    }
    Ok(ConfigurationGuard { configuring })
}

/// Reserve the prompt slot with a final configuration recheck. Together with
/// `begin_configuration`'s final busy recheck this closes both interleavings
/// without serializing unrelated sessions behind a global mutex.
pub(super) fn begin_prompt(busy: &AtomicBool, configuring: &AtomicBool) -> Result<()> {
    if configuring.load(Ordering::Acquire) {
        bail!("ACP 会话配置仍在同步，请稍候再发送");
    }
    if busy.swap(true, Ordering::AcqRel) {
        bail!("ACP 会话仍在生成");
    }
    if configuring.load(Ordering::Acquire) {
        busy.store(false, Ordering::Release);
        bail!("ACP 会话配置仍在同步，请稍候再发送");
    }
    Ok(())
}

enum SessionConfigChange<'a> {
    Model(&'a str),
    Mode(&'a str),
    Option {
        config_id: &'a str,
        value_id: &'a str,
    },
}

impl SessionConfigChange<'_> {
    fn config_id(&self) -> &str {
        match self {
            Self::Model(_) => "model",
            Self::Mode(_) => "mode",
            Self::Option { config_id, .. } => config_id,
        }
    }

    fn value_id(&self) -> &str {
        match self {
            Self::Model(value_id) | Self::Mode(value_id) => value_id,
            Self::Option { value_id, .. } => value_id,
        }
    }

    fn busy_error(&self) -> &'static str {
        match self {
            Self::Model(_) => "Agent 正在处理当前任务，模型将在本轮结束后才能修改",
            Self::Mode(_) => "Agent 正在处理当前任务，权限模式将在本轮结束后才能修改",
            Self::Option { .. } => "Agent 正在处理当前任务，配置将在本轮结束后才能修改",
        }
    }

    async fn apply(&self, runtime: &AcpSession) -> Result<()> {
        match self {
            Self::Model(model_id) => runtime.set_model(model_id).await,
            Self::Mode(mode_id) => runtime.set_mode(mode_id).await,
            Self::Option {
                config_id,
                value_id,
            } => runtime.set_config_option(config_id, value_id).await,
        }
    }
}

impl AcpPool {
    fn remember_config_choice(
        &self,
        session_id: &str,
        runtime: &AcpSession,
        config_id: &str,
        value_id: &str,
    ) {
        let backend = self.backend(session_id);
        let mut errors = Vec::new();
        if let Err(error) = self
            .agents
            .set_acp_config_value(session_id, config_id, value_id)
        {
            errors.push(format!("会话配置: {error:#}"));
        }
        if let Err(error) = self.config_defaults.set(backend, config_id, value_id) {
            errors.push(format!("新会话默认值: {error:#}"));
        }
        if !errors.is_empty() {
            let message = errors.join("；");
            eprintln!(
                "[pinvou3-app] failed to persist {} ACP config {}={}: {}",
                backend.display_name(),
                config_id,
                value_id,
                message
            );
            runtime.bridge.emit(
                "config_persistence_failed",
                json!({
                    "configId": config_id,
                    "valueId": value_id,
                    "message": message,
                }),
            );
        }
    }

    async fn apply_config_change(
        &self,
        session_id: &str,
        change: SessionConfigChange<'_>,
    ) -> Result<CodexAcpSessionInfo> {
        let runtime = self.get_or_spawn(session_id).await?;
        let _configuration =
            begin_configuration(&runtime.busy, &runtime.configuring, change.busy_error())?;
        let config_id = change.config_id();
        let value_id = change.value_id();
        runtime.bridge.emit(
            "config_change_requested",
            json!({ "configId": config_id, "valueId": value_id }),
        );
        if let Err(error) = change.apply(&runtime).await {
            runtime.bridge.emit(
                "config_change_failed",
                json!({
                    "configId": config_id,
                    "valueId": value_id,
                    "message": format!("{error:#}"),
                }),
            );
            return Err(error);
        }
        self.remember_config_choice(session_id, &runtime, config_id, value_id);
        runtime.bridge.emit(
            "config_change_applied",
            json!({ "configId": config_id, "valueId": value_id }),
        );
        let info = runtime.info(
            self.pending_permissions_for(session_id).await,
            self.pending_elicitations_for(session_id).await,
        );
        patch_acp_state(session_id, json!({ "session": &info }))?;
        Ok(info)
    }

    pub async fn set_model(&self, session_id: &str, model_id: &str) -> Result<CodexAcpSessionInfo> {
        self.apply_config_change(session_id, SessionConfigChange::Model(model_id))
            .await
    }

    pub async fn set_config_option(
        &self,
        session_id: &str,
        config_id: &str,
        value_id: &str,
    ) -> Result<CodexAcpSessionInfo> {
        self.apply_config_change(
            session_id,
            SessionConfigChange::Option {
                config_id,
                value_id,
            },
        )
        .await
    }

    pub async fn set_mode(&self, session_id: &str, mode_id: &str) -> Result<CodexAcpSessionInfo> {
        self.apply_config_change(session_id, SessionConfigChange::Mode(mode_id))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_and_configuration_slots_are_mutually_exclusive() {
        let busy = AtomicBool::new(false);
        let configuring = AtomicBool::new(false);

        let guard = begin_configuration(&busy, &configuring, "busy").unwrap();
        assert!(begin_prompt(&busy, &configuring).is_err());
        drop(guard);

        begin_prompt(&busy, &configuring).unwrap();
        assert!(begin_configuration(&busy, &configuring, "busy").is_err());
    }

    #[test]
    fn configuration_guard_releases_on_drop() {
        let busy = AtomicBool::new(false);
        let configuring = AtomicBool::new(false);
        {
            let _guard = begin_configuration(&busy, &configuring, "busy").unwrap();
            assert!(configuring.load(Ordering::Acquire));
        }
        assert!(!configuring.load(Ordering::Acquire));
    }
}

use anyhow::Result;
use async_trait::async_trait;
use serde::Serialize;
use std::fmt;

use crate::platform::prefs::SavedModel;

/// 模型凭据由谁负责提供。前端只消费这一语义，不需要认识具体模型或认证后端。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCredentialMode {
    None,
    UserManaged,
    BackendManaged,
}

/// 引擎创建或复用前交给运行时模型提供器的上下文。
#[derive(Clone)]
pub struct RuntimeModelRequest {
    pub session_id: String,
    pub model: SavedModel,
    pub scheduled_unattended: bool,
}

/// 运行时提供器准备的敏感模型凭据。
///
/// 凭据只保存在内存中，不参与序列化；`Debug` 固定脱敏，避免 bridge 或测试日志
/// 意外输出明文。存在时它是本次引擎配置的最终凭据，优先于环境变量和本地凭据库。
#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeModelCredential {
    api_key: String,
}

impl RuntimeModelCredential {
    pub fn api_key(api_key: impl Into<String>) -> Result<Self> {
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            anyhow::bail!("runtime model API key must not be empty");
        }
        Ok(Self { api_key })
    }

    pub(crate) fn expose_api_key(&self) -> &str {
        &self.api_key
    }
}

impl fmt::Debug for RuntimeModelCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RuntimeModelCredential([REDACTED])")
    }
}

/// 已完成运行时准备的模型。
///
/// `revision` 只能包含非敏感版本标识（如令牌记录 ID 或更新时间），不得包含 API Key。
/// 当同一会话的 revision 或模型配置变化时，EnginePool 会回收旧引擎并使用新配置重建。
#[derive(Clone, PartialEq, Eq)]
pub struct PreparedRuntimeModel {
    pub model: SavedModel,
    pub credential: Option<RuntimeModelCredential>,
    pub revision: Option<String>,
}

impl PreparedRuntimeModel {
    pub fn unchanged(model: SavedModel) -> Self {
        Self {
            model,
            credential: None,
            revision: None,
        }
    }

    /// 判断当前准备结果是否要求替换已有引擎。
    ///
    /// 比较覆盖模型路由、运行时凭据和显式 revision；调用方无需读取或记录密钥。
    pub fn requires_rebuild_from(&self, previous: &Self) -> bool {
        self != previous
    }
}

/// Community 默认实现无需外部服务；Official/Enterprise 可在私有层实现后台托管凭据。
#[async_trait]
pub trait RuntimeModelProvider: Send + Sync {
    fn credential_mode(
        &self,
        _model: &SavedModel,
        user_api_key_required: bool,
    ) -> ModelCredentialMode {
        if user_api_key_required {
            ModelCredentialMode::UserManaged
        } else {
            ModelCredentialMode::None
        }
    }

    async fn prepare(&self, request: RuntimeModelRequest) -> Result<PreparedRuntimeModel>;
}

#[derive(Debug, Default)]
pub struct PassthroughRuntimeModelProvider;

#[async_trait]
impl RuntimeModelProvider for PassthroughRuntimeModelProvider {
    async fn prepare(&self, request: RuntimeModelRequest) -> Result<PreparedRuntimeModel> {
        Ok(PreparedRuntimeModel::unchanged(request.model))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ModelCredentialMode, PassthroughRuntimeModelProvider, PreparedRuntimeModel,
        RuntimeModelCredential, RuntimeModelProvider, RuntimeModelRequest,
    };
    use crate::platform::credential_store::{CredentialEditAction, CredentialState};
    use crate::platform::prefs::{ModelPreset, SavedModel};

    fn model() -> SavedModel {
        SavedModel {
            id: "model-1".to_string(),
            name: "Model 1".to_string(),
            preset: ModelPreset::OpenaiCompatible,
            context_window_tokens: None,
            max_output_tokens: None,
            model: "model-1".to_string(),
            base_url: "https://example.invalid/v1".to_string(),
            provider_kind: None,
            vendor: None,
            endpoint_mode: None,
            image_capability_override: Default::default(),
            vision_model_id: None,
            api_key: String::new(),
            credential_ref: None,
            credential_state: CredentialState::Missing,
            has_secret: false,
            credential_action: None::<CredentialEditAction>,
        }
    }

    #[tokio::test]
    async fn passthrough_provider_preserves_model_without_private_dependencies() {
        let provider = PassthroughRuntimeModelProvider;
        let prepared = provider
            .prepare(RuntimeModelRequest {
                session_id: "session-1".to_string(),
                model: model(),
                scheduled_unattended: false,
            })
            .await
            .expect("prepare model");

        assert_eq!(prepared.model.id, "model-1");
        assert_eq!(prepared.revision, None);
        assert_eq!(
            provider.credential_mode(&prepared.model, true),
            ModelCredentialMode::UserManaged
        );
        assert_eq!(
            provider.credential_mode(&prepared.model, false),
            ModelCredentialMode::None
        );
    }

    #[test]
    fn credential_modes_have_stable_wire_names() {
        assert_eq!(
            serde_json::to_string(&ModelCredentialMode::BackendManaged)
                .expect("serialize credential mode"),
            "\"backend_managed\""
        );
    }

    #[test]
    fn runtime_credential_debug_output_is_redacted() {
        let credential =
            RuntimeModelCredential::api_key("runtime-secret").expect("runtime credential");
        let rendered = format!("{credential:?}");
        assert!(rendered.contains("[REDACTED]"));
        assert!(!rendered.contains("runtime-secret"));
        assert!(RuntimeModelCredential::api_key("  ").is_err());
    }

    #[test]
    fn revision_change_requires_engine_rebuild() {
        let previous = PreparedRuntimeModel {
            model: model(),
            credential: Some(
                RuntimeModelCredential::api_key("runtime-secret").expect("runtime credential"),
            ),
            revision: Some("revision-1".to_string()),
        };
        let unchanged = previous.clone();
        let rotated = PreparedRuntimeModel {
            model: model(),
            credential: Some(
                RuntimeModelCredential::api_key("runtime-secret").expect("runtime credential"),
            ),
            revision: Some("revision-2".to_string()),
        };
        let rotated_credential = PreparedRuntimeModel {
            model: model(),
            credential: Some(
                RuntimeModelCredential::api_key("runtime-secret-2")
                    .expect("rotated runtime credential"),
            ),
            revision: Some("revision-1".to_string()),
        };

        assert!(!unchanged.requires_rebuild_from(&previous));
        assert!(rotated.requires_rebuild_from(&previous));
        assert!(rotated_credential.requires_rebuild_from(&previous));
    }
}

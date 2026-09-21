use std::fmt;

use crate::platform::prefs::SavedModel;

/// 运行时准备阶段产出的敏感模型凭据。
///
/// 凭据只保存在内存中，不参与序列化；`Debug` 固定脱敏，避免 bridge 或测试日志
/// 意外输出明文。存在时它是本次引擎配置的最终凭据，优先于环境变量和本地凭据库。
/// Community 默认准备路径固定 passthrough（`credential` 保持 None，见
/// engine_pool 的 `prepare_runtime_model`）；字段为 enterprise seam 保留。
#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeModelCredential {
    api_key: String,
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
}

#[cfg(test)]
mod tests {
    use super::{PreparedRuntimeModel, RuntimeModelCredential};
    use crate::platform::credential_store::{CredentialEditAction, CredentialState};
    use crate::platform::prefs::{ModelPreset, SavedModel};

    fn model() -> SavedModel {
        SavedModel {
            id: "model-1".to_string(),
            name: "Model 1".to_string(),
            alias: None,
            preset: ModelPreset::OpenaiCompatible,
            context_window_tokens: None,
            max_output_tokens: None,
            reasoning_effort: None,
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

    fn credential(api_key: &str) -> RuntimeModelCredential {
        RuntimeModelCredential {
            api_key: api_key.to_string(),
        }
    }

    /// Community 默认准备路径不依赖任何私有服务：unchanged 原样保留模型，
    /// 不携带运行时凭据或显式 revision。
    #[test]
    fn unchanged_preparation_preserves_model_without_private_dependencies() {
        let prepared = PreparedRuntimeModel::unchanged(model());

        assert_eq!(prepared.model.id, "model-1");
        assert_eq!(prepared.revision, None);
        assert_eq!(prepared.credential, None);
    }

    #[test]
    fn runtime_credential_debug_output_is_redacted() {
        let rendered = format!("{:?}", credential("runtime-secret"));
        assert!(rendered.contains("[REDACTED]"));
        assert!(!rendered.contains("runtime-secret"));
    }

    #[test]
    fn revision_change_requires_engine_rebuild() {
        // EnginePool 的重建判定（PreparedRuntimeState::requires_rebuild_from）
        // 是整体相等比较：revision 或运行时凭据任一变化即视为需要回收重建。
        let previous = PreparedRuntimeModel {
            model: model(),
            credential: Some(credential("runtime-secret")),
            revision: Some("revision-1".to_string()),
        };
        let unchanged = previous.clone();
        let rotated = PreparedRuntimeModel {
            model: model(),
            credential: Some(credential("runtime-secret")),
            revision: Some("revision-2".to_string()),
        };
        let rotated_credential = PreparedRuntimeModel {
            model: model(),
            credential: Some(credential("runtime-secret-2")),
            revision: Some("revision-1".to_string()),
        };

        assert!(unchanged == previous);
        assert!(rotated != previous);
        assert!(rotated_credential != previous);
    }
}

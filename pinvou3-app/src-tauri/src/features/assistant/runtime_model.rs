use anyhow::Result;
use std::fmt;

use crate::platform::prefs::SavedModel;

/// 运行时准备阶段产出的敏感模型凭据。
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

//! 评测模型身份与快照类型。
//!
//! 原 app 侧 smoke 评测栈(runner/cases/mock/report/markdown 报告与 judge
//! 运行时)已由 `pinvou-cli/crates/adapter-smoke` 统一实现并删除;本模块只
//! 保留 EnginePool 与 headless 评测宿主仍消费的模型身份、不可变选中快照
//! 与 judge 身份校验。

use anyhow::{bail, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelIdentity {
    pub provider: String,
    pub model: String,
}

impl ModelIdentity {
    pub(crate) fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }
}

/// A resolved, non-sensitive model snapshot passed from validation to session creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalModelSelection {
    token: String,
    model_id: Option<String>,
    wire_model: String,
    identity: ModelIdentity,
}

impl EvalModelSelection {
    pub(crate) fn new(token: String, model_id: Option<String>, identity: ModelIdentity) -> Self {
        let wire_model = identity.model.clone();
        Self {
            token,
            model_id,
            wire_model,
            identity,
        }
    }

    pub(crate) fn model_id(&self) -> Option<&str> {
        self.model_id.as_deref()
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }

    pub(crate) fn wire_model(&self) -> &str {
        &self.wire_model
    }

    pub(crate) fn identity(&self) -> &ModelIdentity {
        &self.identity
    }
}

/// Opaque handle for one suite-wide tested-model snapshot. The complete saved model remains
/// private to EnginePool; callers can only inspect the non-sensitive identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EvalSuiteModelSnapshot {
    token: String,
    identity: ModelIdentity,
}

impl EvalSuiteModelSnapshot {
    pub(crate) fn new(token: String, identity: ModelIdentity) -> Self {
        Self { token, identity }
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }

    pub(crate) fn identity(&self) -> &ModelIdentity {
        &self.identity
    }
}

pub(crate) fn validate_judge_identity(tested: &ModelIdentity, judge: &ModelIdentity) -> Result<()> {
    let tested_provider = tested.provider.trim();
    let tested_model = tested.model.trim();
    let judge_provider = judge.provider.trim();
    let judge_model = judge.model.trim();
    if tested_provider.is_empty()
        || tested_model.is_empty()
        || judge_provider.is_empty()
        || judge_model.is_empty()
    {
        bail!("tested and judge model identities must include provider and model");
    }
    if tested_provider.eq_ignore_ascii_case(judge_provider)
        && tested_model.eq_ignore_ascii_case(judge_model)
    {
        bail!("judge model must differ from the tested model");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{validate_judge_identity, EvalModelSelection, ModelIdentity};

    #[test]
    fn selection_is_a_non_sensitive_immutable_snapshot() {
        let selection = EvalModelSelection::new(
            "opaque-test-token".to_string(),
            Some("judge-id".to_string()),
            ModelIdentity::new("actual-provider", "actual-wire-model"),
        );

        assert_eq!(selection.model_id(), Some("judge-id"));
        assert_eq!(selection.wire_model(), "actual-wire-model");
        assert_eq!(selection.identity().model, "actual-wire-model");
        assert!(!format!("{selection:?}").contains("api_key"));
        assert!(!format!("{selection:?}").contains("base_url"));
    }

    #[test]
    fn different_provider_or_model_is_allowed() {
        let tested = ModelIdentity::new("deepseek", "chat");
        assert!(validate_judge_identity(&tested, ModelIdentity::new("openai", "chat")).is_ok());
        assert!(
            validate_judge_identity(&tested, ModelIdentity::new("deepseek", "reasoner")).is_ok()
        );
    }

    #[test]
    fn same_normalized_provider_and_model_is_rejected() {
        let tested = ModelIdentity::new(" DeepSeek ", " Chat ");
        let judge = ModelIdentity::new("deepseek", "chat");

        assert!(validate_judge_identity(&tested, &judge).is_err());
    }

    #[test]
    fn empty_identity_is_rejected() {
        let valid = ModelIdentity::new("deepseek", "chat");

        assert!(validate_judge_identity(&ModelIdentity::new(" ", "chat"), &valid).is_err());
        assert!(validate_judge_identity(&valid, ModelIdentity::new("deepseek", " ")).is_err());
    }
}

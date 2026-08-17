use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::security::{validate_component, validate_revision, validate_safe_text};
use crate::{BenchmarkDescriptor, Result, Split, ToolPolicyId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelIdentity {
    provider: String,
    model: String,
}

impl ModelIdentity {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Result<Self> {
        let identity = Self {
            provider: provider.into(),
            model: model.into(),
        };
        validate_safe_text(&identity.provider)?;
        validate_safe_text(&identity.model)?;
        Ok(identity)
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunManifest {
    schema_version: u16,
    run_id: String,
    benchmark: String,
    adapter_version: String,
    dataset_revision: String,
    scorer_revision: String,
    split: String,
    model: ModelIdentity,
    tool_policy: String,
    concurrency: u16,
    pass: u16,
    created_at_ms: u64,
}

impl RunManifest {
    pub fn new(
        run_id: impl Into<String>,
        descriptor: &BenchmarkDescriptor,
        split: Split,
        model: ModelIdentity,
        tool_policy: ToolPolicyId,
        pass: u16,
    ) -> Result<Self> {
        let manifest = Self {
            schema_version: 1,
            run_id: run_id.into(),
            benchmark: descriptor.id().as_str().into(),
            adapter_version: descriptor.adapter_version().into(),
            dataset_revision: descriptor.dataset_revision().into(),
            scorer_revision: descriptor.scorer_revision().into(),
            split: split.as_str().into(),
            model,
            tool_policy: tool_policy.as_str().into(),
            concurrency: 1,
            pass,
            created_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        validate_component(&self.run_id)?;
        validate_component(&self.benchmark)?;
        validate_revision(&self.adapter_version)?;
        validate_revision(&self.dataset_revision)?;
        validate_revision(&self.scorer_revision)?;
        validate_safe_text(&self.split)?;
        validate_safe_text(&self.tool_policy)?;
        if self.schema_version != 1 || self.concurrency != 1 || self.pass == 0 {
            return Err(crate::BenchmarkError::coded("invalid_manifest"));
        }
        Ok(())
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn benchmark(&self) -> &str {
        &self.benchmark
    }

    pub fn concurrency(&self) -> u16 {
        self.concurrency
    }

    pub(crate) fn matches_expected(&self, expected: &Self) -> bool {
        self.schema_version == expected.schema_version
            && self.run_id == expected.run_id
            && self.benchmark == expected.benchmark
            && self.adapter_version == expected.adapter_version
            && self.dataset_revision == expected.dataset_revision
            && self.scorer_revision == expected.scorer_revision
            && self.split == expected.split
            && self.model == expected.model
            && self.tool_policy == expected.tool_policy
            && self.concurrency == expected.concurrency
            && self.pass == expected.pass
    }

    pub(crate) fn matches_descriptor(&self, descriptor: &BenchmarkDescriptor) -> bool {
        self.benchmark == descriptor.id().as_str()
            && self.adapter_version == descriptor.adapter_version()
            && self.dataset_revision == descriptor.dataset_revision()
            && self.scorer_revision == descriptor.scorer_revision()
    }

    pub fn matches_contract(
        &self,
        descriptor: &BenchmarkDescriptor,
        split: &str,
        tool_policy: &str,
        pass: u16,
    ) -> bool {
        self.validate().is_ok()
            && self.matches_descriptor(descriptor)
            && self.split == split
            && self.tool_policy == tool_policy
            && self.pass == pass
    }

    pub fn matches_resume(
        &self,
        descriptor: &BenchmarkDescriptor,
        split: &str,
        model: &ModelIdentity,
        tool_policy: &str,
    ) -> bool {
        self.schema_version == 1
            && self.concurrency == 1
            && self.pass == 1
            && self.benchmark == descriptor.id().as_str()
            && self.adapter_version == descriptor.adapter_version()
            && self.dataset_revision == descriptor.dataset_revision()
            && self.scorer_revision == descriptor.scorer_revision()
            && self.split == split
            && &self.model == model
            && self.tool_policy == tool_policy
    }
}

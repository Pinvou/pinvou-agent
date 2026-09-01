use std::fmt;

pub(crate) const GAIA_PUBLIC_WEB_V1_ALLOWED_TOOLS: &[&str] = &["File", "Web", "image_analyze"];

pub(crate) const GAIA_OFFLINE_V1_ALLOWED_TOOLS: &[&str] = &["File"];

pub(crate) const PRODUCT_V1_ALLOWED_TOOLS: &[&str] = &["File", "Web", "image_analyze"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EvalToolPolicy {
    ProductV1,
    GaiaPublicWebV1,
    GaiaOfflineV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EvalNetworkClass {
    PublicWeb,
    Offline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EvalTurnPolicy {
    pub id: EvalToolPolicy,
    pub allowed_tools: &'static [&'static str],
    pub network: EvalNetworkClass,
}

impl EvalTurnPolicy {
    pub(crate) fn allows(&self, tool: &str) -> bool {
        self.allowed_tools.contains(&tool)
    }
}

static GAIA_PUBLIC_WEB_V1: EvalTurnPolicy = EvalTurnPolicy {
    id: EvalToolPolicy::GaiaPublicWebV1,
    allowed_tools: GAIA_PUBLIC_WEB_V1_ALLOWED_TOOLS,
    network: EvalNetworkClass::PublicWeb,
};

static PRODUCT_V1: EvalTurnPolicy = EvalTurnPolicy {
    id: EvalToolPolicy::ProductV1,
    allowed_tools: PRODUCT_V1_ALLOWED_TOOLS,
    network: EvalNetworkClass::PublicWeb,
};

static GAIA_OFFLINE_V1: EvalTurnPolicy = EvalTurnPolicy {
    id: EvalToolPolicy::GaiaOfflineV1,
    allowed_tools: GAIA_OFFLINE_V1_ALLOWED_TOOLS,
    network: EvalNetworkClass::Offline,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EvalToolPolicyError;

impl EvalToolPolicyError {
    pub(crate) const fn code(self) -> &'static str {
        "unsupported_tool_policy"
    }
}

impl fmt::Display for EvalToolPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for EvalToolPolicyError {}

pub(crate) fn resolve_eval_policy(
    policy_id: &str,
) -> Result<&'static EvalTurnPolicy, EvalToolPolicyError> {
    match policy_id {
        "pinvou-product/v1" => Ok(&PRODUCT_V1),
        "pinvou-gaia-public-web/v1" => Ok(&GAIA_PUBLIC_WEB_V1),
        "pinvou-gaia-offline/v1" => Ok(&GAIA_OFFLINE_V1),
        _ => Err(EvalToolPolicyError),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EvalNetworkClass, EvalToolPolicy, GAIA_OFFLINE_V1_ALLOWED_TOOLS,
        GAIA_PUBLIC_WEB_V1_ALLOWED_TOOLS, PRODUCT_V1_ALLOWED_TOOLS, resolve_eval_policy,
    };
    use crate::features::assistant::tool_policy::is_pinvou3_allowed;
    use deepseek_tui::config::VisionModelConfig;
    use deepseek_tui::tools::registry::ToolRegistryBuilder;
    use deepseek_tui::tools::spec::ToolContext;
    use std::collections::HashSet;

    fn verified_product_catalog_snapshot() -> Vec<String> {
        ToolRegistryBuilder::new()
            .with_file_tools()
            .with_web_tools()
            .with_vision_tools(VisionModelConfig {
                model: "catalog-fixture".to_string(),
                api_key: None,
                base_url: None,
            })
            .build(ToolContext::new(
                std::env::temp_dir().join("pinvou-gaia-catalog-fixture"),
            ))
            .to_api_tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect()
    }

    #[test]
    fn resolves_registered_eval_profiles() {
        assert_eq!(
            resolve_eval_policy("pinvou-product/v1").unwrap().id,
            EvalToolPolicy::ProductV1
        );
        assert_eq!(
            resolve_eval_policy("pinvou-gaia-public-web/v1").unwrap().id,
            EvalToolPolicy::GaiaPublicWebV1
        );
        assert_eq!(
            resolve_eval_policy("pinvou-gaia-offline/v1").unwrap().id,
            EvalToolPolicy::GaiaOfflineV1
        );
        assert_eq!(
            resolve_eval_policy("unknown/v1").unwrap_err().code(),
            "unsupported_tool_policy"
        );
    }

    #[test]
    fn profiles_have_exact_network_separation() {
        let product = resolve_eval_policy("pinvou-product/v1").unwrap();
        let public = resolve_eval_policy("pinvou-gaia-public-web/v1").unwrap();
        let offline = resolve_eval_policy("pinvou-gaia-offline/v1").unwrap();

        assert_eq!(public.network, EvalNetworkClass::PublicWeb);
        assert_eq!(offline.network, EvalNetworkClass::Offline);
        assert_eq!(product.network, EvalNetworkClass::PublicWeb);
        assert!(product.allows("File"));
        assert!(product.allows("Web"));
        assert!(product.allows("image_analyze"));
        assert!(public.allows("Web"));
        assert!(!offline.allows("Web"));
        assert!(!offline.allows("image_analyze"));
    }

    #[test]
    fn profile_snapshots_are_unique_and_exist_in_the_product_catalog() {
        let catalog = verified_product_catalog_snapshot();

        assert_eq!(
            GAIA_PUBLIC_WEB_V1_ALLOWED_TOOLS,
            &["File", "Web", "image_analyze"]
        );
        assert_eq!(GAIA_OFFLINE_V1_ALLOWED_TOOLS, &["File"]);
        assert_eq!(PRODUCT_V1_ALLOWED_TOOLS, &["File", "Web", "image_analyze"]);

        for profile in [
            PRODUCT_V1_ALLOWED_TOOLS,
            GAIA_PUBLIC_WEB_V1_ALLOWED_TOOLS,
            GAIA_OFFLINE_V1_ALLOWED_TOOLS,
        ] {
            assert_eq!(
                profile.len(),
                profile.iter().copied().collect::<HashSet<_>>().len()
            );
            for &name in profile.iter() {
                assert_eq!(
                    catalog
                        .iter()
                        .filter(|actual| actual.as_str() == name)
                        .count(),
                    1
                );
            }
        }

        // The upstream read-only builder registers this compatibility helper,
        // but Pinvou deliberately excludes it from the product tool policy.
        assert!(!is_pinvou3_allowed("retrieve_tool_result"));
        assert!(!PRODUCT_V1_ALLOWED_TOOLS.contains(&"retrieve_tool_result"));
        assert!(!GAIA_PUBLIC_WEB_V1_ALLOWED_TOOLS.contains(&"retrieve_tool_result"));
        assert!(!GAIA_OFFLINE_V1_ALLOWED_TOOLS.contains(&"retrieve_tool_result"));
    }
}

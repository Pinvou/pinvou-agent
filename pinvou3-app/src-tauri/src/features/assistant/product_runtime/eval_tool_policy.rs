use std::fmt;

pub(crate) const GAIA_PUBLIC_WEB_V1_ALLOWED_TOOLS: &[&str] = &[
    "read_file",
    "list_dir",
    "grep_files",
    "file_search",
    "web_search",
    "fetch_url",
    "image_analyze",
];

pub(crate) const GAIA_OFFLINE_V1_ALLOWED_TOOLS: &[&str] =
    &["read_file", "list_dir", "grep_files", "file_search"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EvalToolPolicy {
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
        "pinvou-gaia-public-web/v1" => Ok(&GAIA_PUBLIC_WEB_V1),
        "pinvou-gaia-offline/v1" => Ok(&GAIA_OFFLINE_V1),
        _ => Err(EvalToolPolicyError),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        resolve_eval_policy, EvalNetworkClass, EvalToolPolicy, GAIA_OFFLINE_V1_ALLOWED_TOOLS,
        GAIA_PUBLIC_WEB_V1_ALLOWED_TOOLS,
    };
    use deepseek_tui::config::VisionModelConfig;
    use deepseek_tui::tools::pinvou3_blocklist::PINVOU3_HIDDEN_TOOLS;
    use deepseek_tui::tools::plan::new_shared_plan_state;
    use deepseek_tui::tools::registry::{AgentToolSurfaceOptions, ToolRegistryBuilder};
    use deepseek_tui::tools::spec::ToolContext;
    use deepseek_tui::tools::todo::new_shared_todo_list;
    use deepseek_tui::worker_profile::ShellPolicy;
    use std::collections::HashSet;

    fn verified_product_catalog_snapshot() -> Vec<String> {
        let mut options = AgentToolSurfaceOptions::new(ShellPolicy::None);
        options.web_search_enabled = true;
        options.vision_config = Some(VisionModelConfig {
            model: "catalog-fixture".to_string(),
            api_key: None,
            base_url: None,
        });
        ToolRegistryBuilder::new()
            .with_agent_runtime_surface(
                None,
                "catalog-fixture".to_string(),
                options,
                new_shared_todo_list(),
                new_shared_plan_state(),
            )
            .build(ToolContext::new(
                std::env::temp_dir().join("pinvou-gaia-catalog-fixture"),
            ))
            .to_api_tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect()
    }

    #[test]
    fn resolves_only_registered_gaia_v1_profiles() {
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
        let public = resolve_eval_policy("pinvou-gaia-public-web/v1").unwrap();
        let offline = resolve_eval_policy("pinvou-gaia-offline/v1").unwrap();

        assert_eq!(public.network, EvalNetworkClass::PublicWeb);
        assert_eq!(offline.network, EvalNetworkClass::Offline);
        assert!(public.allows("web_search"));
        assert!(public.allows("fetch_url"));
        assert!(!offline.allows("web_search"));
        assert!(!offline.allows("fetch_url"));
        assert!(!offline.allows("image_analyze"));
    }

    #[test]
    fn profile_snapshots_are_unique_and_exist_in_the_product_catalog() {
        let catalog = verified_product_catalog_snapshot();

        assert_eq!(
            GAIA_PUBLIC_WEB_V1_ALLOWED_TOOLS,
            &[
                "read_file",
                "list_dir",
                "grep_files",
                "file_search",
                "web_search",
                "fetch_url",
                "image_analyze",
            ]
        );
        assert_eq!(
            GAIA_OFFLINE_V1_ALLOWED_TOOLS,
            &["read_file", "list_dir", "grep_files", "file_search"]
        );

        for profile in [
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

        // Registered by the upstream read-only builder, but Pinvou deliberately
        // hides it from the product catalog, so the GAIA candidates must drop it.
        assert!(PINVOU3_HIDDEN_TOOLS.contains(&"retrieve_tool_result"));
        assert!(!GAIA_PUBLIC_WEB_V1_ALLOWED_TOOLS.contains(&"retrieve_tool_result"));
        assert!(!GAIA_OFFLINE_V1_ALLOWED_TOOLS.contains(&"retrieve_tool_result"));
    }
}

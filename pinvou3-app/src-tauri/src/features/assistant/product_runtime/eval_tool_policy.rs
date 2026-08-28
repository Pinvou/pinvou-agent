use std::fmt;

pub(crate) const GAIA_PUBLIC_WEB_V1_ALLOWED_TOOLS: &[&str] = &["File", "Web", "image_analyze"];

pub(crate) const GAIA_OFFLINE_V1_ALLOWED_TOOLS: &[&str] = &["File"];

pub(crate) const PRODUCT_V1_ALLOWED_TOOLS: &[&str] = &["File", "Web", "image_analyze"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EvalToolPolicy {
    ProductV1,
    GaiaPublicWebV1,
    GaiaOfflineV1,
    GaiaFinalAnswerOnlyV1,
}

impl EvalToolPolicy {
    pub(crate) fn allows(self, tool: &str) -> bool {
        match self {
            Self::ProductV1 => PRODUCT_V1_ALLOWED_TOOLS.contains(&tool),
            Self::GaiaPublicWebV1 => GAIA_PUBLIC_WEB_V1_ALLOWED_TOOLS.contains(&tool),
            Self::GaiaOfflineV1 => GAIA_OFFLINE_V1_ALLOWED_TOOLS.contains(&tool),
            Self::GaiaFinalAnswerOnlyV1 => false,
        }
    }

    pub(crate) fn model_reminder(self) -> &'static str {
        match self {
            Self::ProductV1 => {
                "<system-reminder>Evaluation tool contract: answer directly without tools by default. The only callable tools are exactly `File`, `Web`, and `image_analyze`; never invent or call weather, date, time, browser, web_search, fetch_url, read_file, or other tool names. Use `Web` only when the question requires current or external public information. In that case, make exactly one `Web` call promptly, with `action: \"search\"` and a concise `query`; use its result to answer and do not perform follow-up Web calls. Never call `File` unless the user message includes an attachment. With an attachment, use only a read-only File action: `read`, `list`, `search_name`, or `search_content`. Every `File` or `Web` call must include the `action` field. If a tool fails, do not retry it; always produce a concise final answer from available evidence and state the limitation.</system-reminder>"
            }
            Self::GaiaPublicWebV1 => {
                "<system-reminder>GAIA evaluation tool contract: answer directly without tools when the answer is already known. The only callable tools are exactly `File`, `Web`, and `image_analyze`; never invent or call weather, date, time, browser, web_search, fetch_url, read_file, or other tool names. For public evidence, use `Web` with `action: \"search\"` and a concise `query`; use `action: \"fetch\"` with a result URL when the source page must be inspected. Every JSONPath in Web `fields` must start with `$`. If fetch reports JavaScript-only or unreadable content, search for the exact page title or an alternate authoritative source instead of retrying the same URL. Multiple Web calls are allowed when a task genuinely requires multi-step research, but stop as soon as the evidence determines the answer and never exceed 8 total tool calls for one task. Do not repeat an unchanged failed call. Never call `File` unless the user message includes an attachment. With an attachment, use only a read-only File action: `read`, `list`, `search_name`, or `search_content`, and copy the exact attachment path shown in the user message instead of guessing a basename or `/dev/stdin`. Every `File` or `Web` call must include the `action` field. Always finish with a concise final answer and state any evidence limitation.</system-reminder>"
            }
            Self::GaiaOfflineV1 => {
                "<system-reminder>Evaluation tool contract: the only callable tool is exactly `File`. Never invent or call read_file, browser, web, search, date, time, or other tool names. Use `File` only when the user message includes an attachment, only with a read-only action (`read`, `list`, `search_name`, or `search_content`), and always include the `action` field. Do not retry an unchanged failed call.</system-reminder>"
            }
            Self::GaiaFinalAnswerOnlyV1 => {
                "<system-reminder>GAIA final-answer recovery: all tools are disabled. Do not request or describe a tool call. Use only evidence already present in the conversation. Respond with exactly one non-empty line in the form `FINAL ANSWER: <answer>` and no additional text.</system-reminder>"
            }
        }
    }
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

static GAIA_FINAL_ANSWER_ONLY_V1: EvalTurnPolicy = EvalTurnPolicy {
    id: EvalToolPolicy::GaiaFinalAnswerOnlyV1,
    allowed_tools: &[],
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
        "pinvou-gaia-final-answer-only/v1" => Ok(&GAIA_FINAL_ANSWER_ONLY_V1),
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
            resolve_eval_policy("pinvou-gaia-final-answer-only/v1")
                .unwrap()
                .id,
            EvalToolPolicy::GaiaFinalAnswerOnlyV1
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
        let final_only = resolve_eval_policy("pinvou-gaia-final-answer-only/v1").unwrap();
        assert_eq!(final_only.network, EvalNetworkClass::Offline);
        assert!(final_only.allowed_tools.is_empty());
        assert!(!final_only.allows("File"));
        assert_eq!(product.network, EvalNetworkClass::PublicWeb);
        assert!(product.allows("File"));
        assert!(product.allows("Web"));
        assert!(product.allows("image_analyze"));
        assert!(public.allows("Web"));
        assert!(!offline.allows("Web"));
        assert!(!offline.allows("image_analyze"));
        assert!(EvalToolPolicy::ProductV1.allows("Web"));
        assert!(!EvalToolPolicy::ProductV1.allows("web_search"));
        assert!(
            EvalToolPolicy::ProductV1
                .model_reminder()
                .contains("make exactly one `Web` call promptly")
        );
        assert!(
            EvalToolPolicy::GaiaPublicWebV1
                .model_reminder()
                .contains("Multiple Web calls are allowed")
        );
        assert!(
            EvalToolPolicy::GaiaPublicWebV1
                .model_reminder()
                .contains("never exceed 8 total tool calls")
        );
        assert!(
            EvalToolPolicy::GaiaPublicWebV1
                .model_reminder()
                .contains("`action: \"fetch\"`")
        );
        assert!(
            EvalToolPolicy::GaiaOfflineV1
                .model_reminder()
                .contains("only callable tool is exactly `File`")
        );
        assert!(
            EvalToolPolicy::GaiaFinalAnswerOnlyV1
                .model_reminder()
                .contains("exactly one non-empty line")
        );
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

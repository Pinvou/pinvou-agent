//! Model context-window resolution shared by pinvou3 runtime state and Engine
//! routing.
//!
//! Model facts maintained by CodeWhale reuse the base catalog first; this module
//! only supplements cloud models that the pinvou3 settings page already provides
//! but the base does not cover yet. Both the window facts (resolved by name) and
//! the window precedence (declared vs probed vs inferred) have their single entry
//! point here, so the page display never uses a different scale than
//! `active_route_limits` / compaction thresholds / the monitor denominator.

/// 精确匹配模型名，并容忍 `-` 分隔的日期、快照或服务档位后缀。
fn model_name_matches(lower: &str, name: &str) -> bool {
    lower == name
        || lower
            .strip_prefix(name)
            .is_some_and(|rest| rest.starts_with('-'))
}

/// CodeWhale 已统一解析 `Nk` 后缀；这里只补它尚未覆盖的 `1m` 写法。
fn explicit_one_million_hint(lower: &str) -> Option<u32> {
    lower.contains("1m").then_some(1_048_576)
}

/// pinvou3 对底座的定向覆盖：底座已收录但数值落后于官方口径的模型。
/// 优先于底座 catalog 生效；上游修复后应移除对应条目。
const PINVOU_OVERRIDES: &[(&str, u32)] = &[
    // Both the base catalog and the known table already list claude-opus-5 at
    // 1M. The entry stays as an explicit anchor: if the base figure ever
    // regresses, the official 1M figure still governs (the Claude 5 family
    // except haiku is 1M, platform.claude.com models overview).
    ("claude-opus-5", 1_000_000),
    // The base context heuristic only grants 1M to deepseek-family names that
    // contain the "v4" substring; deepseek-flash has no "v4" and would fall to
    // the 128K legacy heuristic (verified to return Some(128_000) and to hit
    // before PINVOU_KNOWN is consulted); V4.1-Flash is officially 1M of context
    // (api-docs.deepseek.com/quick_start/pricing, checked 2026-09-11).
    // So it must live in this override table rather than PINVOU_KNOWN.
    ("deepseek-flash", 1_000_000),
    // The base known table still records grok-4.20-0309-* as 2M (stale rows in
    // crates/tui/src/models.rs); the docs.x.ai model detail page was re-checked
    // as 1M on 2026-09-11. The known-table hit happens before the prefs
    // context_window_fallback, so correcting the base value requires this
    // override table (the catalog desc is already annotated as 1M).
    ("grok-4.20-0309-reasoning", 1_000_000),
    ("grok-4.20-0309-non-reasoning", 1_000_000),
    // The ACP preset keeps the pre-retirement legacy spelling
    // grok-4.20-reasoning (an append-only legacy datalist option); the base
    // known table only recognizes the -0309- spelling, so the old spelling
    // resolves None → the engine falls to 128K, while the monitor page has the
    // prefs "grok-4.20" substring fallback of 1M — the two diverge, hence the
    // same-value override (re-checked as 1M on the docs.x.ai model page on
    // 2026-09-11). Note this entry also matches the -0309- spellings via
    // suffix tolerance; both values are identical, so there is no conflict.
    ("grok-4.20-reasoning", 1_000_000),
    // The base known table only lists claude-fable-5 exactly; claude-fable-5-1
    // would miss and fall to the "unknown-name claude wildcard 200K" fallback;
    // fable-5-1 is officially 1M
    // (platform.claude.com models overview, 2026-09-11).
    // model_name_matches also tolerates future -date/snapshot suffixes.
    ("claude-fable-5-1", 1_000_000),
    // No gpt-6-family row exists anywhere in the base chain (bundled catalog /
    // known table / heuristics / claude wildcard), so resolved returns None:
    // the engine side falls to 128K while the monitor page still has the Openai
    // preset fallback of 1.05M — the two diverge. The catalog row's official
    // figure is 1,050,000
    // (developers.openai.com/api/docs/models, 2026-09-11).
    // Note: this model's tool calling is only available over the Responses wire
    // protocol, so it is not the openai preset default (see prefs::model), but
    // the catalog row stays selectable and the engine side must match the
    // monitor page.
    ("gpt-6-astra", 1_050_000),
    // Gemini preset default model; the base known table's gemini rows stop at
    // 3.7, so without a 3.8 row the engine falls to 128K and diverges from the
    // monitor page's Gemini 1M fallback. Official input limit is 1,048,576
    // (ai.google.dev gemini-3.8-flash page, 2026-09-11).
    ("gemini-3.8-flash", 1_048_576),
    // The catalog desc says 256K; the base known table only has bare
    // "grok-build" → 512K (exact equality misses the -0.1 wire id), so the
    // engine resolves None and falls to 128K. Official figure is 256K
    // (docs.x.ai grok-build-0.1 page, 2026-09-11).
    ("grok-build-0.1", 256_000),
];

/// pinvou3 supplemental table: applies only when the base chain (catalog /
/// known table / heuristics / wildcard) has no row at all. Unlike
/// PINVOU_OVERRIDES it is consulted after the base, so it only fills gaps and
/// never overrides values.
const PINVOU_KNOWN: &[(&str, u32)] = &[
    // The Kimi K3 direct-platform model is officially rated at 1M tokens
    // (platform.kimi.com/docs/models).
    ("kimi-k3", 1_048_576),
    // The Coding Plan bare-k3 window depends on the plan; the base records the
    // safe 256K value and 1M plans should configure it explicitly.
    // kimi-for-coding-highspeed belongs to K2.7 Code HighSpeed, officially 256K;
    // bare kimi-for-coding is now K2.8 Preview (officially up to 1M,
    // plan-tier dependent), deliberately not listed separately here pending
    // the next refresh.
    ("kimi-for-coding-highspeed", 262_144),
    ("kimi-k2.7-code-highspeed", 262_144),
    // Alibaba Cloud's official docs give qwen3.7-plus/max/flash a 1M context.
    ("qwen3.7-plus", 1_000_000),
    ("qwen3.7-max", 1_000_000),
    ("qwen3.7-flash", 1_000_000),
    // qwen3.8-max is GA: officially a 1M context (983,616 is the thinking-mode
    // max input, not the context window); the `-preview` suffix is tolerated
    // by model_name_matches.
    ("qwen3.8-max", 1_000_000),
    // The base currently only covers qwen3.6-flash with the `qwen/` prefix.
    ("qwen3.6-flash", 1_000_000),
    // Volcengine announcement (2026-07): doubao-seed-evolving upgraded to a
    // 1M context.
    ("doubao-seed-evolving", 1_048_576),
    // Active Doubao 2.x rows: the base chain (catalog / known table /
    // heuristics) has no doubao rows at all, so resolved returns None and
    // the engine falls to 128K while the monitor page falls to the Doubao
    // preset fallback of 262,144 — the two diverge. The official model list
    // gives every 2.x model a 256k context window and 224k max input
    // (volcengine docs 82379/1330310, checked 2026-09-12), matching the
    // preset fallback's binary 256K. The base has neither a row nor a
    // conflicting value for these, so they belong in this supplemental
    // table rather than PINVOU_OVERRIDES.
    ("doubao-seed-2-1-pro-260628", 262_144),
    ("doubao-seed-2-1-turbo-260628", 262_144),
    ("doubao-seed-2-0-code-preview-260215", 262_144),
    ("doubao-seed-2-0-pro-260215", 262_144),
    ("doubao-seed-2-0-lite-260428", 262_144),
    // Zhipu officially rates GLM-4.7 at 200K; following the settings page's
    // binary-K display convention.
    ("glm-4.7", 204_800),
];

/// 解析 pinvou3 已知模型的上下文窗口。
///
/// 顺序为：显式 `1m` → pinvou3 定向覆盖 → CodeWhale 模型 catalog/`Nk` 启发式 → pinvou3 补充表。
#[must_use]
pub fn resolved_context_window(model: &str) -> Option<u32> {
    let lower = model.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return None;
    }
    if let Some(window) = explicit_one_million_hint(&lower) {
        return Some(window);
    }
    if let Some((_, window)) = PINVOU_OVERRIDES
        .iter()
        .find(|(name, _)| model_name_matches(&lower, name))
    {
        return Some(*window);
    }
    if let Some(window) = deepseek_tui::models::context_window_for_model(&lower) {
        return Some(window);
    }
    PINVOU_KNOWN
        .iter()
        .find(|(name, _)| model_name_matches(&lower, name))
        .map(|(_, window)| *window)
}

/// The unified context-window precedence: the host `bridge::route_limits_for_model`
/// (which decides inference and compaction thresholds) and the monitor display
/// (`model_probe`) must both call this function; writing a second match elsewhere
/// is not allowed. Precedence: the window the user explicitly declares in the
/// model form wins and is min-clamped against the probed value (the probe is
/// deployment ground truth); without a declaration the probed value applies, and
/// only when that is absent too does the caller-resolved inferred fallback apply.
/// The second return value flags "the inferred fallback was actually adopted",
/// for the monitor page to label the `context_window_inferred` diagnostic.
///
/// "Whether the probed value is trustworthy / participates in clamping" is the
/// caller's decision: the host only probes locally introspectable vLLM (cloud
/// is always `probed = None`); the monitor's gate lives in `model_probe`.
#[must_use]
pub fn resolve_context_window(
    configured: Option<u32>,
    probed: Option<u32>,
    inferred: Option<u32>,
) -> (Option<u32>, bool) {
    match (configured, probed) {
        (Some(configured), Some(probed)) => (Some(configured.min(probed)), false),
        (Some(configured), None) => (Some(configured), false),
        (None, probed) => (probed.or(inferred), probed.is_none() && inferred.is_some()),
    }
}

/// Output-cap tier declaration for operator-owned endpoints (local vLLM,
/// custom OpenAI-compatible / custom; see
/// `SavedModel::is_operator_owned_endpoint` for the predicate) — acting as
/// the deployer's proxy, the host declares the route output fact by window
/// tier (replacing the base's 8192 fail-close guess for uncatalogued
/// models).
///
/// Single source of truth: both the declaration arm of
/// `bridge::route_limits_for_model` and the monitor page's live-probe test
/// take their values from here; do not inline the formula again (an inlined
/// copy drifted in the very round it was introduced by missing the 500K
/// tier).
#[must_use]
pub fn operator_owned_output_declaration(window: Option<u32>) -> Option<u32> {
    let declared = match window {
        Some(window) if window >= 500_000 => 131_072,
        Some(window) if window >= 250_000 => 65_536,
        Some(window) => (window / 4).min(32_768),
        // No window fact: fall back to a quarter of the base's 128K default
        // window (not a base-native value: the base model-level fallback is
        // 64000 and the route-level fail-close is <=8192; after
        // min(64000, 32768) the effective value is exactly 32768).
        None => 32_768,
    };
    // For tiny windows window/4 cannot fit a meaningful output budget (<4K):
    // stay undeclared (fail-closed) instead of emitting a Some(<4K) route
    // fact.
    (declared >= 4_096).then_some(declared)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supplemental_cloud_models_resolve_to_verified_windows() {
        for (model, expected) in [
            ("kimi-k3", 1_048_576),
            ("k3", 262_144),
            ("qwen3.7-plus", 1_000_000),
            ("qwen3.7-max", 1_000_000),
            ("qwen3.7-flash", 1_000_000),
            ("qwen3.6-flash", 1_000_000),
            ("qwen3.8-max", 1_000_000),
            ("qwen3.8-max-preview", 1_000_000),
            ("doubao-seed-evolving", 1_048_576),
            ("doubao-seed-2-1-pro-260628", 262_144),
            ("doubao-seed-2-1-turbo-260628", 262_144),
            ("doubao-seed-2-0-code-preview-260215", 262_144),
            ("doubao-seed-2-0-pro-260215", 262_144),
            ("doubao-seed-2-0-lite-260428", 262_144),
            ("glm-4.7", 204_800),
        ] {
            assert_eq!(resolved_context_window(model), Some(expected), "{model}");
        }
    }

    #[test]
    fn pinvou_overrides_take_precedence_over_codewhale_catalog() {
        // The base already lists opus-5 at 1M; the override entry acts as an
        // explicit anchor that takes effect first and pins the figure.
        assert_eq!(resolved_context_window("claude-opus-5"), Some(1_000_000));
        // The base falls back to legacy 128K for deepseek names without "v4";
        // V4.1-Flash is officially 1M and must be corrected first by the
        // override table (see the PINVOU_OVERRIDES comment).
        assert_eq!(resolved_context_window("deepseek-flash"), Some(1_000_000));
        // The base known table still records grok-4.20-0309-* as 2M (behind the
        // docs.x.ai 2026-09 figure), so the override table must correct it
        // first; the base does not list fable-5-1 exactly, otherwise it would
        // fall to the claude wildcard 200K.
        assert_eq!(
            resolved_context_window("grok-4.20-0309-reasoning"),
            Some(1_000_000)
        );
        assert_eq!(
            resolved_context_window("grok-4.20-0309-non-reasoning"),
            Some(1_000_000)
        );
        // The pre-retirement legacy spelling kept by the ACP preset: the base
        // has no row, so without the override the engine falls to 128K and
        // diverges from the monitor page's prefs "grok-4.20" substring
        // fallback of 1M — pinned here.
        assert_eq!(
            resolved_context_window("grok-4.20-reasoning"),
            Some(1_000_000)
        );
        assert_eq!(resolved_context_window("claude-fable-5-1"), Some(1_000_000));
        // model_name_matches' date/snapshot suffix tolerance applies to the new
        // override entries too.
        assert_eq!(
            resolved_context_window("claude-fable-5-1-20260901"),
            Some(1_000_000)
        );
        // The gpt-6-astra / grok-build-0.1 override entries previously had no
        // shared-entry assertion: the monitor page's prefs fallback values are
        // coincidentally identical, so removing an entry kept every test green
        // while the engine side silently fell back to a divergent 128K — pin
        // them here so removing either entry turns this test red.
        assert_eq!(resolved_context_window("gpt-6-astra"), Some(1_050_000));
        assert_eq!(resolved_context_window("grok-build-0.1"), Some(256_000));
        // Models the base already lists with correct figures are unaffected.
        assert_eq!(resolved_context_window("claude-haiku-4-5"), Some(200_000));
        assert_eq!(resolved_context_window("claude-sonnet-5"), Some(1_000_000));
    }

    #[test]
    fn explicit_window_wins_and_codewhale_remains_the_base_catalog() {
        assert_eq!(resolved_context_window("kimi-k3-256k"), Some(256_000));
        assert_eq!(resolved_context_window("kimi-k3-1m"), Some(1_048_576));
        assert_eq!(
            resolved_context_window("gpt-5.6-sol"),
            deepseek_tui::models::context_window_for_model("gpt-5.6-sol")
        );
        assert_eq!(resolved_context_window("unknown-cloud-model"), None);
    }

    /// Window precedence table (single source of truth): declaration+probe →
    /// min, declaration beats an absent probe, probe beats inference without a
    /// declaration, and the flag is set only when inference is truly adopted.
    #[test]
    fn context_window_precedence_is_single_sourced() {
        // Declaration + probe → min (ground-truth clamping for local deployments;
        // under-declaring likewise keeps the declaration).
        assert_eq!(
            resolve_context_window(Some(1_048_576), Some(131_072), None),
            (Some(131_072), false)
        );
        assert_eq!(
            resolve_context_window(Some(32_768), Some(131_072), None),
            (Some(32_768), false)
        );
        // Declaration + no probe → the declaration applies as-is (the host never
        // probes cloud; a failed local probe lands here too).
        assert_eq!(
            resolve_context_window(Some(1_048_576), None, Some(131_072)),
            (Some(1_048_576), false)
        );
        // No declaration → probe first; only when the probe is absent does
        // inference apply, and the adopted flag is set only in that case.
        assert_eq!(
            resolve_context_window(None, Some(262_144), Some(131_072)),
            (Some(262_144), false)
        );
        assert_eq!(
            resolve_context_window(None, None, Some(1_000_000)),
            (Some(1_000_000), true)
        );
        assert_eq!(resolve_context_window(None, None, None), (None, false));
    }

    #[test]
    fn operator_owned_output_declaration_tiers_and_fail_closed_floor() {
        // Tier boundaries (same table as the bridge tier test; this pins the
        // pure function itself).
        assert_eq!(
            operator_owned_output_declaration(Some(1_048_576)),
            Some(131_072)
        );
        assert_eq!(
            operator_owned_output_declaration(Some(500_000)),
            Some(131_072)
        );
        assert_eq!(
            operator_owned_output_declaration(Some(499_999)),
            Some(65_536)
        );
        assert_eq!(
            operator_owned_output_declaration(Some(250_000)),
            Some(65_536)
        );
        assert_eq!(
            operator_owned_output_declaration(Some(249_999)),
            Some(32_768)
        );
        assert_eq!(
            operator_owned_output_declaration(Some(262_144)),
            Some(65_536)
        );
        assert_eq!(
            operator_owned_output_declaration(Some(131_072)),
            Some(32_768)
        );
        assert_eq!(
            operator_owned_output_declaration(Some(65_536)),
            Some(16_384)
        );
        assert_eq!(operator_owned_output_declaration(Some(16_384)), Some(4_096));
        assert_eq!(operator_owned_output_declaration(Some(16_383)), None);
        assert_eq!(operator_owned_output_declaration(Some(4_096)), None);
        // No window fact → quarter of the 128K default window fallback.
        assert_eq!(operator_owned_output_declaration(None), Some(32_768));
    }
}

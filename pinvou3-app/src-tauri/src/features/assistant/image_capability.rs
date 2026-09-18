//! 模型图片输入能力解析(设计 §6.3,阶段 C)。
//!
//! 能力判断按**具体模型**而非 provider/preset/ACP adapter:不能因为 local_vllm
//! 或某 provider 协议上能收图片,就假定当前模型能识图(设计 §1.5/§7)。
//!
//! 解析优先级:
//! 1. 用户对 SavedModel 的显式 override(`Enabled`→Supported,`Disabled`→Unsupported);
//! 2. Builtin verified-capability table (substring table `VERIFIED_IMAGE_CAPABLE_MODELS` OR-merged
//!    with exact-equality table `EXACT_VERIFIED_IMAGE_CAPABLE_MODELS`, see each
//!    table's comment); before v0.9.5 there was also a base
//!    model-catalog tier (`deepseek_tui::model_catalog`), no longer exposed (see
//!    the level-② note on `effective_image_capability`);
//! 3. 都判不出 → `Unknown`(默认不冒充支持,允许用户在设置里 override Enabled)。
//!
//! ⚠️ 内置表宁可 Unknown 不可误判 Supported:只对明确多模态的模型名子串判中。
//! 本地自定义模型(尤其 LocalVllm 的 `qwen36_35b_256k`,文本/多模态两种部署都存在,
//! 见设计 §7.1/§7.2)一律 Unknown,交给用户显式确认。

use crate::platform::prefs::{ImageCapabilityOverride, SavedModel};

/// 一次解析后生效的图片输入能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectiveImageCapability {
    /// 确认支持图片输入(override Enabled / 内置表命中)。
    Supported,
    /// 确认不支持(override Disabled)。
    Unsupported,
    /// 判不出来:默认不冒充支持,路由上按"需视觉模型兜底"处理。
    Unknown,
}

/// 普通会话图片输入路由(设计 §6.3 路由表)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageInputMode {
    /// 文字 + 图片同一条用户消息直发主模型,不走 image_analyze。
    Native,
    /// 主模型不能看图:保留 image_analyze 工具回退链路(需已配置可用视觉模型)。
    VisionToolFallback,
    /// 两条路都没有:发送前拒绝,提示切换模型或配置视觉模型。
    Unsupported,
}

impl EffectiveImageCapability {
    /// 稳定 wire 值:`get_image_input_capability` 命令返回给前端,前端按字符串匹配。
    /// 改名必须同步前端展示逻辑与 commands 层序列化稳定性测试。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Unsupported => "unsupported",
            Self::Unknown => "unknown",
        }
    }
}

impl ImageInputMode {
    /// 稳定 wire 值:见 `EffectiveImageCapability::as_str`。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::VisionToolFallback => "vision_tool_fallback",
            Self::Unsupported => "unsupported",
        }
    }
}

/// Moonshot always-thinking 模型判定(**探测专用**名单,真实链路不需要):
/// 这些模型官方接入要求 `thinking: {"type":"enabled"}` 保持开启,省略该参数的
/// 请求会被网关拒绝。真实链路由 bridge `request_reasoning_effort` 默认 high
/// (底座翻译成 `thinking: {"type":"enabled"}`)天然满足,无名单;探测 payload
/// 不带 reasoning 设置,必须按本名单显式注入 thinking,否则 kimi-for-coding
/// 等模型的识图探测会 400 误判(2026-08 kimi-for-coding 实测)。
pub fn moonshot_model_requires_explicit_thinking(model: &str) -> bool {
    matches!(
        model.trim().to_ascii_lowercase().as_str(),
        "k3" | "k3-256k"
            | "kimi-k3"
            | "kimi-k2.7-code"
            | "kimi-k2.7-code-highspeed"
            | "kimi-for-coding"
            | "kimi-for-coding-highspeed"
            | "kimi-k2.6"
    )
}

/// 内置已验证能力表:模型名小写后按子串匹配,命中即 Supported。
/// Inclusion rule: only model families the official docs explicitly describe
/// as multimodal (checked vendor by vendor against official docs on
/// 2026-09-11, synced in the same batch as the frontend model-catalog.js);
/// anything uncertain is left out (resolves to Unknown + user override).
/// Note that substring matching cannot cross tier words: when admitting a
/// whole family, confirm the family has no text-only members
/// (e.g. qwen3.7-max, glm-5.3), otherwise admit entries one by one.
const VERIFIED_IMAGE_CAPABLE_MODELS: &[&str] = &[
    // OpenAI 多模态世代。OpenaiCompatible preset 默认模型 `gpt-5.6-terra`
    // (prefs `default_model`) is a gpt-5-family model.
    "gpt-4o",
    "gpt-4.1",
    "gpt-5",
    // gpt-6-astra is officially multimodal (models page "All latest OpenAI
    // models support text and image input", 2026-09-11); the "gpt-5" substring
    // cannot match it, and the catalog already annotates it, so the backend
    // must stay in sync, otherwise the official route degrades to Unknown.
    "gpt-6",
    // Anthropic: the platform.claude.com models overview states "All current
    // models support text and image input" (2026-09-11). claude-3/4/5 covers
    // both claude-N-tier naming styles; sonnet-5 / opus-5 / haiku / fable need
    // separate entries — substring matching cannot cross tier words, and
    // claude-haiku-4-5, missed by the old table, contains neither claude-4 nor
    // claude-haiku-5, so claude-haiku replaces the old claude-haiku-5 entry
    // (also covering the haiku-5 family);
    // claude-fable-5(-5-1) had no entry at all, hence the new claude-fable.
    "claude-3",
    "claude-4",
    "claude-5",
    "claude-sonnet-5",
    "claude-opus-5",
    "claude-haiku",
    "claude-fable",
    // Google Gemini 全系多模态。
    "gemini",
    // xAI Grok family-wide vision input (default preset grok-4.6 hits).
    "grok",
    // DeepSeek V4.1-Flash has native vision (api-docs.deepseek.com/guides/vision,
    // 2026-09-11); the official pricing page states Vision Not supported for
    // deepseek-v4-pro, so it is not admitted.
    "deepseek-flash",
    // Existing configs still save the retired aliases deepseek-v4-flash /
    // -vision-exp: the official docs state the old names are still accepted
    // and routed to V4.1-Flash (multimodal) billing, so they are admitted by
    // exact equality in EXACT_VERIFIED_IMAGE_CAPABLE_MODELS, synced in the
    // same batch as the frontend catalog's legacyAliases imageCapable:true.
    // Not in the substring table: a substring would also match third-party
    // gateway snapshot spellings (deepseek-v4-flash-0731 / -202605 /
    // deepseek/deepseek-v4-*); those deployments are not officially verified
    // for multimodality and the frontend catalog deliberately leaves them
    // unannotated, so they should resolve to Unknown.
    // Alibaba Qwen (help.aliyun.com Model Studio vision docs, 2026-09-11):
    // admit the whole qwen3.8-max / qwen3.8-flash generation; qwen3.7 only
    // plus/flash (3.7-max is text-only, so the qwen3.7 prefix cannot be used
    // as one substring); qwen3.6-flash admitted. The VL series stays. The
    // "qwen3.8" entry covers the whole generation: if the vendor later ships
    // a text-only 3.8 variant (e.g. a coder line), it must be split into
    // per-entry admissions.
    "qwen-vl",
    "qwen2-vl",
    "qwen2.5-vl",
    "qwen3-vl",
    "qwen3.8",
    "qwen3.7-plus",
    "qwen3.7-flash",
    "qwen3.6-flash",
    // Doubao: the official capability column of the five active doubao-seed-*
    // rows and the coding-specialized preview row all include multimodal
    // understanding (volcengine docs 82379/1330310, 2026-09-11), so admitting
    // the whole family is justified.
    "doubao-seed",
    // MiniMax: only M3 supports image input, M2.x does not, so the bare
    // minimax substring must not be used (official docs, 2026-09-11).
    "minimax-m3",
    // Zhipu: GLM-5.3-Flash is natively multimodal; glm-5.3 / glm-5.2 are
    // text-only, so the glm-5.3 prefix cannot be used as one substring
    // (2026-09-11). The glm-4v entry stays for compatibility with existing
    // configs: only glm-4v-flash (free tier) is still sold; glm-4v /
    // glm-4v-plus are gone from the on-sale table and the API enum.
    "glm-5.3-flash",
    "glm-4v",
    // Kimi (2026-09-11): Kimi direct kimi-k3 and Kimi Code k3 / k3-256k are
    // officially image-input models; kimi-k3 goes through the substring entry
    // while k3 / k3-256k are short generic ids, so they are admitted exactly
    // via EXACT_VERIFIED_IMAGE_CAPABLE_MODELS (see its comment);
    // kimi-k2.7-code and kimi-k2.6 list text/image/video input on the official
    // pricing page (platform.kimi.com); kimi-k2.5 and other text models are
    // not admitted.
    "kimi-for-coding",
    "kimi-k3",
    "kimi-k2.7-code",
    "kimi-k2.6",
];

/// Exact (lowercased equality) entries: short generic ids whose substring
/// match surface is too wide, "same-name text-only variants", or official
/// parallel spellings can only be admitted by equality — the bare "k3"
/// substring would classify any custom name containing "k3" (unrelated
/// third-party/aggregator models etc.) as natively vision-capable and inline
/// images on send; mimo-v2.5 via substring would also catch the text-only
/// mimo-v2.5-pro; deepseek-v4-flash(-vision-exp) are official retired aliases
/// whose substring would also catch unverified third-party gateway snapshot
/// spellings. All of these would violate this table's inclusion principle of
/// "prefer Unknown over a false Supported", so they are admitted by equality
/// only. Future -tier spellings should be appended here instead of falling
/// back to substrings.
const EXACT_VERIFIED_IMAGE_CAPABLE_MODELS: &[&str] = &[
    "k3",
    "k3-256k",
    "mimo-v2.5",
    // Tencent Token Plan official hyphenated parallel spelling (same as the
    // frontend minimax-m3 row's legacyAliases): it does not contain the
    // "minimax-m3" substring, and a missing entry would resolve existing
    // configs to Unknown on send, diverging from the form's prefilled
    // "supported" state.
    "minimax-m-3-0",
    // DeepSeek official retired aliases; the official docs state they are
    // still accepted and routed to multimodal V4.1-Flash billing
    // (api-docs.deepseek.com/news260910, checked 2026-09-11).
    "deepseek-v4-flash",
    "deepseek-v4-flash-vision-exp",
];

/// 内置表查询:模型名(小写化)是否命中已验证多模态条目。
fn builtin_verified_supports_image(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return false;
    }
    EXACT_VERIFIED_IMAGE_CAPABLE_MODELS
        .iter()
        .any(|entry| normalized == *entry)
        || VERIFIED_IMAGE_CAPABLE_MODELS
            .iter()
            .any(|entry| normalized.contains(entry))
}

/// 解析一条 SavedModel 的生效图片输入能力(优先级见模块头注释)。
pub fn effective_image_capability(model: &SavedModel) -> EffectiveImageCapability {
    // ① 显式档位优先:Enabled(能)/Disabled(不能)直接钉死。
    match model.image_capability_override {
        ImageCapabilityOverride::Enabled => return EffectiveImageCapability::Supported,
        ImageCapabilityOverride::Disabled => return EffectiveImageCapability::Unsupported,
        // Pinvou(pinvou 决策,默认;旧 auto 档残留反序列化时已迁移到这里)
        // 走内置表判断链。
        ImageCapabilityOverride::Pinvou => {}
    }
    // ②(v0.9.5 起移除)底座 model_catalog 不再公开,目录级 modalities 查询
    // 不可用;模型目录的 image 判定由底座 image_attach::strip_images_when_unsupported
    // 按 route 能力在请求前执行,父仓不再重复判定。
    // ③ 内置已验证能力表。
    if builtin_verified_supports_image(&model.model) {
        return EffectiveImageCapability::Supported;
    }
    // ④ 判不出。
    EffectiveImageCapability::Unknown
}

/// 按设计 §6.3 路由表把能力 + 视觉模型可用性映射为图片输入模式。
/// `has_vision_model` 表示是否配置了**可用**的独立视觉模型
/// (vision_model_id 命中且凭据可解析,见 bridge `resolve_vision_model_config`)。
pub fn image_input_mode(
    capability: EffectiveImageCapability,
    has_vision_model: bool,
) -> ImageInputMode {
    match capability {
        // Supported(含 override Enabled)→ Native,无论有无视觉模型。
        EffectiveImageCapability::Supported => ImageInputMode::Native,
        // Unsupported(含 override Disabled)/ Unknown:有视觉模型走工具兜底,否则拒绝。
        EffectiveImageCapability::Unsupported | EffectiveImageCapability::Unknown => {
            if has_vision_model {
                ImageInputMode::VisionToolFallback
            } else {
                ImageInputMode::Unsupported
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::credential_store::CredentialState;
    use crate::platform::prefs::ModelPreset;

    fn saved_model(preset: ModelPreset, model: &str) -> SavedModel {
        SavedModel {
            id: "m1".to_string(),
            name: model.to_string(),
            alias: None,
            preset,
            context_window_tokens: None,
            max_output_tokens: None,
            reasoning_effort: None,
            model: model.to_string(),
            base_url: "https://example.invalid/v1".to_string(),
            provider_kind: None,
            vendor: None,
            endpoint_mode: None,
            image_capability_override: ImageCapabilityOverride::Pinvou,
            vision_model_id: None,
            api_key: String::new(),
            credential_ref: None,
            credential_state: CredentialState::Missing,
            has_secret: false,
            credential_action: None,
        }
    }

    #[test]
    fn moonshot_always_thinking_list_covers_bridge_canonical_names() {
        // 钉住 always-thinking 名单的命中/误伤边界：探测 payload 与真实链路
        // 必须同一口径,否则 always-thinking 模型识图探测会被网关 400 误判
        // (2026-08 kimi-for-coding 实测)。
        for name in [
            "kimi-for-coding",
            "kimi-for-coding-highspeed",
            "kimi-k3",
            "kimi-k2.7-code",
            "kimi-k2.7-code-highspeed",
            "kimi-k2.6",
            "K3",
            "k3-256k",
        ] {
            assert!(
                moonshot_model_requires_explicit_thinking(name),
                "{name} 应命中 always-thinking 名单"
            );
        }
        for name in ["gpt-4o", "deepseek-v4-pro", "qwen-vl-max", "kimi-k2.5"] {
            assert!(
                !moonshot_model_requires_explicit_thinking(name),
                "{name} 不应误判为 always-thinking"
            );
        }
    }

    #[test]
    fn wire_strings_are_stable() {
        // 前端按这些字符串匹配(选图即时警告),改动属于 wire 协议破坏。
        assert_eq!(EffectiveImageCapability::Supported.as_str(), "supported");
        assert_eq!(
            EffectiveImageCapability::Unsupported.as_str(),
            "unsupported"
        );
        assert_eq!(EffectiveImageCapability::Unknown.as_str(), "unknown");
        assert_eq!(ImageInputMode::Native.as_str(), "native");
        assert_eq!(
            ImageInputMode::VisionToolFallback.as_str(),
            "vision_tool_fallback"
        );
        assert_eq!(ImageInputMode::Unsupported.as_str(), "unsupported");
    }

    #[test]
    fn unknown_local_model_defaults_to_unknown() {
        // 本地 vLLM 默认模型:文本/多模态部署都存在(设计 §7.1/§7.2),不得冒充支持。
        let model = saved_model(ModelPreset::LocalVllm, "qwen36_35b_256k");
        assert_eq!(
            effective_image_capability(&model),
            EffectiveImageCapability::Unknown
        );
        // 任意自定义本地模型同样 Unknown。
        let custom = saved_model(ModelPreset::OpenaiCompatible, "my-finetune-7b");
        assert_eq!(
            effective_image_capability(&custom),
            EffectiveImageCapability::Unknown
        );
    }

    #[test]
    fn builtin_table_hits_verified_multimodal_models() {
        for (preset, name) in [
            (ModelPreset::OpenaiCompatible, "gpt-4o-mini"),
            (ModelPreset::OpenaiCompatible, "gpt-4.1"),
            // The preset default models (prefs `default_model`) must hit,
            // otherwise the official route degrades to Unknown.
            (ModelPreset::OpenaiCompatible, "gpt-5.6-terra"),
            // gpt-6-astra is officially multimodal, covered by the "gpt-6"
            // entry ("gpt-5" cannot match it).
            (ModelPreset::OpenaiCompatible, "gpt-6-astra"),
            (ModelPreset::OpenaiCompatible, "claude-3-5-sonnet-20241022"),
            (ModelPreset::OpenaiCompatible, "claude-4-opus"),
            // Default presets (claude-sonnet-5 / grok-4.6 / deepseek-flash /
            // qwen3.8-max / MiniMax-M3 / kimi-k3) must hit, otherwise the
            // official route degrades to Unknown.
            (ModelPreset::OpenaiCompatible, "claude-sonnet-5"),
            (ModelPreset::OpenaiCompatible, "gemini-2.5-pro"),
            (ModelPreset::OpenaiCompatible, "grok-4.6"),
            // Old-table gap fix: claude-haiku-4-5 / claude-fable-5-1 are both
            // current multimodal models
            // (platform.claude.com models overview, 2026-09-11).
            (ModelPreset::OpenaiCompatible, "claude-haiku-4-5"),
            (ModelPreset::OpenaiCompatible, "claude-fable-5"),
            (ModelPreset::OpenaiCompatible, "claude-fable-5-1"),
            // V4.1-Flash native vision (api-docs.deepseek.com/guides/vision).
            (ModelPreset::Deepseek, "deepseek-flash"),
            // The retired aliases still route to V4.1-Flash; existing configs
            // must likewise resolve to Supported.
            (ModelPreset::Deepseek, "deepseek-v4-flash"),
            (ModelPreset::Deepseek, "deepseek-v4-flash-vision-exp"),
            (ModelPreset::Qwen, "qwen-vl-max"),
            (ModelPreset::Qwen, "Qwen2.5-VL-72B-Instruct"),
            // Per-entry anchors for the VL series: the qwen2-vl / qwen3-vl
            // substring entries previously had no test coverage, so silently
            // deleting an entry kept every test green (qwen3-vl-* would degrade
            // to Unknown).
            (ModelPreset::Qwen, "qwen2-vl-72b-instruct"),
            (ModelPreset::Qwen, "qwen3-vl-max"),
            (ModelPreset::Qwen, "qwen3.8-max"),
            (ModelPreset::Qwen, "qwen3.8-flash"),
            (ModelPreset::Qwen, "qwen3.7-plus"),
            (ModelPreset::Qwen, "qwen3.7-flash"),
            (ModelPreset::Qwen, "qwen3.6-flash"),
            (ModelPreset::Glm, "glm-4v-plus"),
            (ModelPreset::Glm, "glm-5.3-flash"),
            (ModelPreset::Doubao, "doubao-seed-evolving"),
            (ModelPreset::Minimax, "MiniMax-M3"),
            // Tencent Token Plan official hyphenated parallel spelling (same
            // as the frontend legacyAliases); must also resolve to Supported
            // via the exact table, otherwise existing configs degrade to
            // Unknown on send.
            (ModelPreset::OpenaiCompatible, "minimax-m-3-0"),
            // MiMo (2026-09-11): multimodal is mimo-v2.5; the text-only
            // mimo-v2.5-pro must not be caught by accident, hence the exact
            // equality table (see EXACT_VERIFIED_IMAGE_CAPABLE_MODELS).
            (ModelPreset::Mimo, "mimo-v2.5"),
            // Kimi direct kimi-k3 and Kimi Code k3 / k3-256k are officially
            // image-input (2026-09-11); kimi-for-coding was user-verified as
            // vision-capable (2026-07).
            (ModelPreset::Kimi, "kimi-for-coding"),
            (ModelPreset::Kimi, "kimi-k3"),
            (ModelPreset::OpenaiCompatible, "k3"),
            (ModelPreset::OpenaiCompatible, "k3-256k"),
            (ModelPreset::Kimi, "kimi-k2.7-code"),
            (ModelPreset::Kimi, "kimi-k2.6"),
        ] {
            let model = saved_model(preset, name);
            assert_eq!(
                effective_image_capability(&model),
                EffectiveImageCapability::Supported,
                "{name} 应命中内置已验证能力表"
            );
        }
    }

    #[test]
    fn builtin_table_misses_stay_unknown() {
        // Unified matrix for "miss the builtin vetted table → Unknown". Since
        // v0.9.5 the foundation model_catalog is no longer exposed, catalog-level
        // modalities detection is gone, and there is no catalog-based upgrade path.
        // - Text-only official models must stay Unknown (qwen3.7-max / glm-5.3 /
        //   glm-5.2 / MiniMax-M2.x per the 2026-09-11 vendor docs);
        // - deepseek-v4-pro is not on the official vision page;
        // - mimo-v2.5-pro / muse-spark-1.1 are outside the builtin table and
        //   must also resolve to Unknown;
        // - unrelated custom names containing "k3" must not be mismatched by
        //   the substring (bare k3 is now admitted exactly);
        // - third-party gateway snapshot spellings (deepseek-v4-flash-202605
        //   etc.) are not officially multimodal-verified, and the exact entries
        //   for the retired aliases must not catch them (the frontend catalog
        //   leaves them unannotated too).
        for (preset, name) in [
            (ModelPreset::Deepseek, "deepseek-v4-pro"),
            (ModelPreset::OpenaiCompatible, "deepseek-v4-flash-202605"),
            (ModelPreset::Qwen, "qwen3.7-max"),
            (ModelPreset::Glm, "glm-5.3"),
            (ModelPreset::Glm, "glm-5.2"),
            (ModelPreset::Minimax, "MiniMax-M2.7"),
            (ModelPreset::Minimax, "MiniMax-M2.7-highspeed"),
            (ModelPreset::Mimo, "mimo-v2.5-pro"),
            (ModelPreset::OpenaiCompatible, "muse-spark-1.1"),
            (ModelPreset::OpenaiCompatible, "k3s-local-text"),
        ] {
            let model = saved_model(preset, name);
            assert_eq!(
                effective_image_capability(&model),
                EffectiveImageCapability::Unknown,
                "{name} is not in the builtin vetted table; must resolve to Unknown"
            );
        }
    }

    #[test]
    fn catalog_negative_never_vetoes() {
        // 目录标 text-only 但内置表命中:目录否定不否决,仍 Supported。
        // (gpt-5-codex 在底座目录为 ["text"],内置表 gpt-5 子串命中。)
        let model = saved_model(ModelPreset::OpenaiCompatible, "gpt-5-codex");
        assert_eq!(
            effective_image_capability(&model),
            EffectiveImageCapability::Supported
        );
        // 目录 text-only 且内置表也不命中:落 Unknown 而非 Unsupported——
        // 否定结论只允许来自用户 override Disabled。
        let model = saved_model(ModelPreset::OpenaiCompatible, "claude-opus-4-8");
        assert_eq!(
            effective_image_capability(&model),
            EffectiveImageCapability::Unknown
        );
    }

    #[test]
    fn override_wins_over_builtin_table() {
        // Enabled:未知本地模型 → Supported。
        let mut model = saved_model(ModelPreset::LocalVllm, "qwen36_35b_256k");
        model.image_capability_override = ImageCapabilityOverride::Enabled;
        assert_eq!(
            effective_image_capability(&model),
            EffectiveImageCapability::Supported
        );
        // Disabled:内置表命中的模型 → Unsupported。
        let mut model = saved_model(ModelPreset::OpenaiCompatible, "gpt-4o");
        model.image_capability_override = ImageCapabilityOverride::Disabled;
        assert_eq!(
            effective_image_capability(&model),
            EffectiveImageCapability::Unsupported
        );
    }

    #[test]
    fn pinvou_decision_follows_builtin_table() {
        // Pinvou(pinvou 决策,默认)= 原 Auto 判定链:内置表命中 → Supported,
        // 未命中 → Unknown;不参与探测回填。
        let mut hit = saved_model(ModelPreset::OpenaiCompatible, "gpt-4o");
        hit.image_capability_override = ImageCapabilityOverride::Pinvou;
        assert_eq!(
            effective_image_capability(&hit),
            EffectiveImageCapability::Supported
        );
        let mut miss = saved_model(ModelPreset::LocalVllm, "qwen36_35b_256k");
        miss.image_capability_override = ImageCapabilityOverride::Pinvou;
        assert_eq!(
            effective_image_capability(&miss),
            EffectiveImageCapability::Unknown
        );
    }

    #[test]
    fn routing_table_covers_all_branches() {
        use EffectiveImageCapability as C;
        use ImageInputMode as M;
        // Supported → Native(无论有无视觉模型)。
        assert_eq!(image_input_mode(C::Supported, true), M::Native);
        assert_eq!(image_input_mode(C::Supported, false), M::Native);
        // Unsupported:有视觉模型 → 工具兜底;无 → 拒绝。
        assert_eq!(
            image_input_mode(C::Unsupported, true),
            M::VisionToolFallback
        );
        assert_eq!(image_input_mode(C::Unsupported, false), M::Unsupported);
        // Unknown:有视觉模型 → 工具兜底;无 → 拒绝(提示用户确认能力)。
        assert_eq!(image_input_mode(C::Unknown, true), M::VisionToolFallback);
        assert_eq!(image_input_mode(C::Unknown, false), M::Unsupported);
    }
}

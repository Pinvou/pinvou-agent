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
/// 名单内的模型官方接入要求 `thinking: {"type":"enabled"}` 保持开启,省略该
/// 参数的请求会被网关拒绝。真实链路由 bridge `request_reasoning_effort` 默认
/// high (底座翻译成 `thinking: {"type":"enabled"}`)天然满足,无名单;探测
/// payload 不带 reasoning 设置,必须按本名单显式注入 thinking,否则
/// kimi-for-coding 等模型的识图探测会 400 误判(2026-08 kimi-for-coding 实测)。
///
/// K3.1 拼写刻意不在名单内(2026-11-04 round-4 review): K3 系列的文档化契约
/// 是顶层 `reasoning_effort`(low/high/max,默认 max),明确「不使用 K2.x 的
/// `thinking` 参数」(platform.kimi.com/docs/api/chat);省略即合法形态,与
/// bridge 发送链路对 K3.1 的省略口径一致。早前「thinking 关不掉 → 省略也被拒」
/// 的同推把 K2.x 实测的省略拒绝错误外推到了 K3.1;若实测发现网关仍拒绝省略,
/// 应改为注入顶层 `reasoning_effort` 而非 `thinking`(待有 key 时回归验证)。
pub fn moonshot_model_requires_explicit_thinking(model: &str) -> bool {
    matches!(
        model.trim().to_ascii_lowercase().as_str(),
        "k3" | "k3-256k"
            | "kimi-k3"
            // kimi-k3[1m] is the moonshot preset's official `models1m`
            // suggestion (the platform CC guide still documents it,
            // re-verified 2026-11-04) — the bracket declaration form of the
            // bare kimi-k3 arm above, and this list is exact-match, so it
            // needs its own arm to probe identically. K3.1 spellings are
            // deliberately NOT listed: see the K3-series contract note above.
            | "kimi-k3[1m]"
            | "kimi-k2.7-code"
            | "kimi-k2.7-code-highspeed"
            | "kimi-for-coding"
            | "kimi-for-coding-highspeed"
            | "kimi-k2.6"
    )
}

/// Moonshot always-reasoning budget predicate (**probe-only** list): models
/// on this list always reason before answering, the reasoning text precedes
/// the final answer and is billed against the completion budget, and the
/// probe extractor only reads the final content — so a 64-token probe cap is
/// consumed by reasoning before any color answer is produced, and a
/// vision-capable model gets misreported as unverified. These models probe
/// with the reasoning-tier cap instead
/// (`IMAGE_CAPABILITY_TEST_MAX_TOKENS_THINKING`; live-tested 2026-08
/// kimi-for-coding / k3). The four K3.1 spellings are on this list per the
/// same family contract — the models-overview page states both K3.1
/// variants' parameter constraints are identical to kimi-k3
/// (platform.kimi.com/docs/api/models-overview) — while they must keep the
/// legacy `thinking` parameter omitted (see
/// `moonshot_model_requires_explicit_thinking` above). Bracket `[1m]`
/// declarations are excluded like k3[1m]: the base fail-closes those as CC
/// environment conventions, so no probe legitimately reaches the gateway
/// with them.
///
/// This is deliberately a separate predicate from
/// `moonshot_model_requires_explicit_thinking`: budget selection answers
/// "is this an always-reasoning family whose reasoning consumes the
/// completion budget", while thinking-parameter injection answers "does the
/// legacy `thinking` parameter contract require the field". The two
/// responsibilities must not share one boolean — sharing one is exactly what
/// dropped the K3.1 probe budget to 64 in the 2026-11-04 round-4 fix.
pub fn moonshot_model_needs_reasoning_budget(model: &str) -> bool {
    matches!(
        model.trim().to_ascii_lowercase().as_str(),
        "k3" | "k3-256k"
            | "kimi-k3"
            | "kimi-k3[1m]"
            | "kimi-k2.7-code"
            | "kimi-k2.7-code-highspeed"
            | "kimi-for-coding"
            | "kimi-for-coding-highspeed"
            | "kimi-k2.6"
            // K3.1 spellings — same spelling set as the VERIFIED/exact
            // capability tables above (2026-11-04 round-5 review: always
            // reasoning, reasoning billed against the completion budget).
            | "kimi-k3.1-pro"
            | "kimi-k3.1-pro-highspeed"
            | "k3.1-pro"
            | "k3.1-pro-highspeed"
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
    // MiniMax: M3 and the newer M3.1-Flash-Preview support image (and video)
    // input, M2.x does not, so the bare minimax substring must not be used
    // (official docs, re-checked 2026-09-28). "minimax-m3" covers both.
    "minimax-m3",
    // Zhipu: GLM-5.3-Flash is natively multimodal; glm-5.3 / glm-5.2 are
    // text-only, so the glm-5.3 prefix cannot be used as one substring
    // (2026-09-11). "glm-5.3-flash" also covers the 2026-09 multimodal speed
    // tier glm-5.3-flashx (re-checked 2026-09-28). The glm-4v entry stays for
    // compatibility with existing configs: only glm-4v-flash (free tier) is
    // still sold; glm-4v / glm-4v-plus are gone from the on-sale table and
    // the API enum.
    "glm-5.3-flash",
    "glm-4v",
    // Xiaomi MiMo: the 2026-09-22 V2.6 series is omni-modal (text/image/
    // audio/video input; mimo.mi.com model pages, checked 2026-09-28).
    // "mimo-v2.6" covers pro / flash / pro-ultraspeed; the text-only
    // mimo-v2.5-pro stays Unknown while multimodal mimo-v2.5 keeps its
    // exact-equality entry below.
    "mimo-v2.6",
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
    // Meta Model API (2026-11-03 catalog refresh, dev.meta.ai): muse-spark-1.3
    // (the current default) is officially multimodal (text/image/PDF/video
    // input). muse-spark-1.1 / muse-spark-1.2 / muse-wonder-1.0 joined in
    // the 2026-11-04 fix pass: the models page states all three Muse Spark
    // versions take text, image, video, audio, and PDF input (and names 1.1
    // as a served id in the quickstart troubleshooting), and the Muse Wonder
    // section lists the same modalities — the earlier "no per-model
    // modality statement covers 1.1" ground was refuted live. Entries stay
    // spelled with the full minor version on purpose: the open-weight
    // "Muse Spark Open" gateway deployments are a different supply chain
    // and must keep resolving through their own (absent) entries.
    "muse-spark-1.3",
    "muse-spark-1.2",
    "muse-spark-1.1",
    "muse-wonder-1.0",
    // Mistral (2026-11-03 catalog refresh): the models overview lists
    // mistral-medium-latest as multimodal and mistral-small-latest with
    // image understanding; codestral-latest carries no modality statement
    // and stays Unknown. mistral-large-latest joined in the 2026-11-04 fix
    // pass per models.dev (the base's reference source), which lists its
    // modalities as text/image/pdf — docs.mistral.ai itself was unreachable
    // from every channel on both passes, so the annotation is scoped to
    // that source (re-check at the next refresh).
    "mistral-medium-latest",
    "mistral-small-latest",
    "mistral-large-latest",
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
    // K3.1 Pro (2026-10-23, kimi.com/code/docs): officially "native vision"
    // like K3, but the bare-id spelling contains no "kimi-k3" substring and
    // a bare "k3.1" or "k3" substring would over-match, so it rides the
    // exact table like its K3 siblings. The highspeed variant carries the
    // same constraints as the prefixed spelling — the models-overview page
    // states both K3.1 spellings' "参数约束也与 `kimi-k3` 完全一致"
    // (identical constraints to kimi-k3; re-checked 2026-11-04 — the
    // "同一模型、参数约束完全一致，仅输出速度不同" sentence quoted here
    // earlier is that page's K2.7 Code family wording, not K3.1's) — so the
    // prefixed kimi-k3.1-pro-highspeed already resolves Supported through
    // the "kimi-k3" substring entry, and the bare spelling joins this exact
    // table on the same grounds as k3.1-pro (spelling-consistent: both
    // spellings of each tier resolve identically).
    "k3.1-pro",
    "k3.1-pro-highspeed",
    // The bare [1m] bracket declaration (the ACP datalists' models1m
    // suggestion): dash-suffix tolerance cannot carry a "[1m]" remainder and
    // the bare spelling has no "kimi-k3" substring, so unlike the prefixed
    // forms (which ride the "kimi-k3" substring entry) it needs its own
    // exact arm. Only the officially documented spellings join.
    "k3.1-pro[1m]",
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
            // Official moonshot ACP models1m suggestion (platform CC
            // guide, re-verified 2026-11-04) — the bracket declaration form
            // of the bare kimi-k3 arm above.
            "kimi-k3[1m]",
        ] {
            assert!(
                moonshot_model_requires_explicit_thinking(name),
                "{name} 应命中 always-thinking 名单"
            );
        }
        // K3.1 spellings (bare + [1m] declaration forms) deliberately do NOT
        // hit the list (2026-11-04 round-4 review): the K3-series contract is
        // the top-level reasoning_effort ladder and the docs state the K2.x
        // `thinking` parameter is not used at all (platform.kimi.com
        // docs/api/chat), so the documented-valid probe shape is omission —
        // the same shape the bridge send path resolves for K3.1. The earlier
        // "same class as bare-K3" arms extended the K2.x live-tested
        // omission-rejection to a generation whose docs say otherwise; if a
        // live probe ever shows the gateway rejecting omission for K3.1 too,
        // the fix is a top-level reasoning_effort injection, not a thinking
        // arm.
        for name in [
            "k3.1-pro",
            "k3.1-pro-highspeed",
            "kimi-k3.1-pro",
            "kimi-k3.1-pro-highspeed",
            "k3.1-pro[1m]",
            "kimi-k3.1-pro[1m]",
        ] {
            assert!(
                !moonshot_model_requires_explicit_thinking(name),
                "{name} 走 K3 系列省略契约,不应注入 thinking"
            );
        }
        // k3[1m] must NOT hit the list, but not because bare k3 rides the
        // effort ladder — bare k3 IS in the list above. The real ground:
        // the base fail-closes `k3[1m]` as a Claude Code environment
        // convention, not an API model id (crates/tui/src/config.rs
        // KIMI_CODE_CLAUDE_ALIAS_GUIDANCE + validate_kimi_code_api_model_id
        // rejects it on canonical Kimi Code endpoints), so a probe never
        // legitimately reaches the gateway with that spelling — an arm
        // would only legitimize an id the base refuses to send.
        for name in [
            "gpt-4o",
            "deepseek-v4-pro",
            "qwen-vl-max",
            "kimi-k2.5",
            "k3[1m]",
        ] {
            assert!(
                !moonshot_model_requires_explicit_thinking(name),
                "{name} 不应误判为 always-thinking"
            );
        }
    }

    #[test]
    fn moonshot_reasoning_budget_list_covers_always_reasoning_families() {
        // Pins the budget predicate independently of the thinking predicate:
        // budget membership = always-reasoning families (reasoning billed
        // against the completion budget), NOT the legacy thinking-parameter
        // contract. All pre-existing thinking-list members keep the
        // reasoning-tier probe budget, and the four K3.1 spellings join on
        // the kimi-k3-identical contract (2026-11-04 round-5 review: the
        // shared-boolean version dropped the K3.1 probe budget to 64).
        for name in [
            "k3",
            "k3-256k",
            "kimi-k3",
            "kimi-k3[1m]",
            "kimi-k2.7-code",
            "kimi-k2.7-code-highspeed",
            "kimi-for-coding",
            "kimi-for-coding-highspeed",
            "kimi-k2.6",
            "kimi-k3.1-pro",
            "kimi-k3.1-pro-highspeed",
            "k3.1-pro",
            "k3.1-pro-highspeed",
            "Kimi-K3.1-Pro",
        ] {
            assert!(
                moonshot_model_needs_reasoning_budget(name),
                "{name} is an always-reasoning spelling; probe budget must stay at the reasoning tier"
            );
        }
        // The [1m] bracket declarations stay off the budget list (the base
        // fail-closes them as CC environment conventions, so no probe
        // legitimately reaches the gateway with them), alongside plain
        // non-Moonshot misses.
        for name in [
            "k3.1-pro[1m]",
            "kimi-k3.1-pro[1m]",
            "gpt-4o",
            "deepseek-v4-pro",
            "kimi-k2.5",
        ] {
            assert!(
                !moonshot_model_needs_reasoning_budget(name),
                "{name} must not be given the reasoning-tier probe budget"
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
            // Bare "gpt-4o" pins the standalone table entry itself: the row
            // below only proves the "gpt-4o" substring via -mini, so a
            // narrowing edit would otherwise strand bare gpt-4o (pin carried
            // over from the removed pinvou_decision_follows_builtin_table).
            (ModelPreset::OpenaiCompatible, "gpt-4o"),
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
            // 2026-10-13 Anthropic additions, both image-capable per their
            // anthropic.com announcement pages (Sonnet 5.5: beat Pokémon Red
            // from screenshots / Chartography 61.6%; Haiku 5.5: Visual
            // reasoning Chartography 46.4%). Neither needs a new table
            // entry: sonnet-5-5 rides the "claude-sonnet-5" substring and
            // haiku-5-5 the "claude-haiku" substring (which already covers
            // the haiku-5 family).
            (ModelPreset::OpenaiCompatible, "claude-sonnet-5-5"),
            (ModelPreset::OpenaiCompatible, "claude-haiku-5-5"),
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
            // The multimodal speed tier (standard API, 2026-09) is covered by
            // the same "glm-5.3-flash" substring.
            (ModelPreset::Glm, "glm-5.3-flashx"),
            (ModelPreset::Doubao, "doubao-seed-evolving"),
            (ModelPreset::Doubao, "doubao-seed-2-1-pro-260915"),
            (ModelPreset::Minimax, "MiniMax-M3"),
            // The newer preview tier shares M3's multimodal input and is
            // covered by the same "minimax-m3" substring.
            (ModelPreset::Minimax, "MiniMax-M3.1-Flash-Preview"),
            // Tencent Token Plan official hyphenated parallel spelling (same
            // as the frontend legacyAliases); must also resolve to Supported
            // via the exact table, otherwise existing configs degrade to
            // Unknown on send.
            (ModelPreset::OpenaiCompatible, "minimax-m-3-0"),
            // MiMo (2026-09-11): multimodal is mimo-v2.5; the text-only
            // mimo-v2.5-pro must not be caught by accident, hence the exact
            // equality table (see EXACT_VERIFIED_IMAGE_CAPABLE_MODELS).
            // The mimo rows left the frontend catalog in the 2026-11-03
            // refresh (v2.5 retired 2026-10-21 per the deprecation page);
            // this entry stays for stored configs of the retired rows.
            (ModelPreset::Mimo, "mimo-v2.5"),
            // The 2026-09-22 V2.6 series is omni-modal; all three tiers hit
            // the "mimo-v2.6" substring entry.
            (ModelPreset::Mimo, "mimo-v2.6-pro"),
            (ModelPreset::Mimo, "mimo-v2.6-flash"),
            (ModelPreset::Mimo, "mimo-v2.6-pro-ultraspeed"),
            // Kimi direct kimi-k3 and Kimi Code k3 / k3-256k are officially
            // image-input (2026-09-11); kimi-for-coding was user-verified as
            // vision-capable (2026-07).
            (ModelPreset::Kimi, "kimi-for-coding"),
            (ModelPreset::Kimi, "kimi-k3"),
            (ModelPreset::OpenaiCompatible, "k3"),
            (ModelPreset::OpenaiCompatible, "k3-256k"),
            (ModelPreset::Kimi, "kimi-k2.7-code"),
            (ModelPreset::Kimi, "kimi-k2.6"),
            // 2026-11-03 refresh additions — per-entry anchors so silently
            // deleting an entry cannot keep the suite green (qwen2-vl
            // precedent): K3.1 Pro is natively multimodal (whats-new
            // 2026-10-23; the highspeed variant is the same model per the
            // models-overview page, so all four spellings resolve — the
            // prefixed ones through the "kimi-k3" substring, the bare ones
            // through the exact table).
            (ModelPreset::Kimi, "kimi-k3.1-pro"),
            (ModelPreset::Kimi, "kimi-k3.1-pro-highspeed"),
            (ModelPreset::OpenaiCompatible, "k3.1-pro"),
            (ModelPreset::OpenaiCompatible, "k3.1-pro-highspeed"),
            // The documented [1m] declarations resolve like their base
            // spellings (2026-11-04 fix pass: the bracket forms previously
            // fell to Unknown — the exact table is equality-only and the
            // prefixed bare forms ride the "kimi-k3" substring).
            (ModelPreset::OpenaiCompatible, "k3.1-pro[1m]"),
            (ModelPreset::Kimi, "kimi-k3.1-pro[1m]"),
            // Meta muse family (dev.meta.ai: the models page states all
            // three Muse Spark versions and Muse Wonder take
            // text/image/PDF/video input — 2026-11-04 fix pass; 1.3 was
            // pinned since the 2026-11-03 refresh) and the
            // Mistral multimodal rows (mistral-large-latest per models.dev,
            // the base's reference source — docs.mistral.ai unreachable).
            (ModelPreset::OpenaiCompatible, "muse-spark-1.3"),
            (ModelPreset::OpenaiCompatible, "muse-wonder-1.0"),
            (ModelPreset::OpenaiCompatible, "muse-spark-1.2"),
            (ModelPreset::OpenaiCompatible, "muse-spark-1.1"),
            (ModelPreset::OpenaiCompatible, "mistral-medium-latest"),
            (ModelPreset::OpenaiCompatible, "mistral-small-latest"),
            (ModelPreset::OpenaiCompatible, "mistral-large-latest"),
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
        // - mimo-v2.5-pro is outside the builtin table and must resolve to
        //   Unknown (muse-spark-1.1 joined the supported matrix in the
        //   2026-11-04 fix pass once the models page's all-versions
        //   modality statement was verified live);
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
            (ModelPreset::OpenaiCompatible, "k3s-local-text"),
            // 2026-11-03 conservative boundary pins, updated 2026-11-04:
            // codestral-latest (no modality statement) and unrelated
            // K3.1-suffixed names must stay Unknown (the exact table
            // admits only the documented tiers, not suffix families).
            (ModelPreset::OpenaiCompatible, "codestral-latest"),
            (ModelPreset::OpenaiCompatible, "k3.1-pro-max"),
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

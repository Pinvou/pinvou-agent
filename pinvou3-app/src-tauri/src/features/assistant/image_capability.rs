//! 模型图片输入能力解析(设计 §6.3,阶段 C)。
//!
//! 能力判断按**具体模型**而非 provider/preset/ACP adapter:不能因为 local_vllm
//! 或某 provider 协议上能收图片,就假定当前模型能识图(设计 §1.5/§7)。
//!
//! 解析优先级:
//! 1. 用户对 SavedModel 的显式 override(`Enabled`→Supported,`Disabled`→Unsupported);
//! 2. 内置已验证能力表(`VERIFIED_IMAGE_CAPABLE_MODELS`);v0.9.5 前还有底座
//!    模型目录(`deepseek_tui::model_catalog`)一级,现已不再公开(见
//!    `effective_image_capability` 第②级注释);
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
/// 收录原则:仅官方文档明确多模态的模型族(2026-09-11 逐厂商按官方文档核对,
/// 与前端 model-catalog.js 同批同步);拿不准的一律不收(走 Unknown + 用户 override)。
/// 注意子串匹配跨不过 tier 词:收整族时必须确认族内没有纯文本成员
/// (如 qwen3.7-max、glm-5.3),否则只能逐条目收。
const VERIFIED_IMAGE_CAPABLE_MODELS: &[&str] = &[
    // OpenAI 多模态世代。OpenaiCompatible preset 默认模型 `gpt-5.6-terra`
    // (prefs `default_model`)即 gpt-5 族。
    "gpt-4o",
    "gpt-4.1",
    "gpt-5",
    // gpt-6-astra 官方多模态(models 页 "All latest OpenAI models support
    // text and image input",2026-09-11);子串 "gpt-5" 覆盖不到它,目录已标注、
    // 后端须同步,否则官方路由退化成 Unknown。
    "gpt-6",
    // Anthropic:platform.claude.com models overview 明示「All current models
    // support text and image input」(2026-09-11)。claude-3/4/5 覆盖 claude-N-tier
    // 两式命名;sonnet-5 / opus-5 / haiku / fable 需单独条目——子串匹配跨不过
    // tier 词,旧表漏配的 claude-haiku-4-5 既不含 claude-4 也不含 claude-haiku-5,
    // 故以 claude-haiku 取代旧条目 claude-haiku-5(顺带覆盖 haiku-5 系);
    // claude-fable-5(-5-1)则完全无条目,新增 claude-fable。
    "claude-3",
    "claude-4",
    "claude-5",
    "claude-sonnet-5",
    "claude-opus-5",
    "claude-haiku",
    "claude-fable",
    // Google Gemini 全系多模态。
    "gemini",
    // xAI Grok 全系视觉输入(默认预设 grok-4.6 命中)。
    "grok",
    // DeepSeek V4.1-Flash 原生视觉(api-docs.deepseek.com/guides/vision,
    // 2026-09-11);deepseek-v4-pro 官方 pricing 页明示 Vision Not supported,不收。
    "deepseek-flash",
    // 存量配置仍保存退役别名 deepseek-v4-flash / -vision-exp:官方明示旧名仍被
    // 接受并路由至 V4.1-Flash(多模态)计费,子串条目同时覆盖两个别名,与前端
    // 目录的 legacyAliases imageCapable:true 同批同步。
    "deepseek-v4-flash",
    // 阿里 Qwen(help.aliyun.com Model Studio vision 文档,2026-09-11):
    // qwen3.8-max / qwen3.8-flash 全系收;qwen3.7 仅 plus/flash(3.7-max 纯文本,
    // 不能用 qwen3.7 整体子串);qwen3.6-flash 收。VL 系列保留。"qwen3.8" 为
    // 整代条目:若官方未来发布纯文本 3.8 变体(如 coder 系),须拆为逐条目收。
    "qwen-vl",
    "qwen2-vl",
    "qwen2.5-vl",
    "qwen3-vl",
    "qwen3.8",
    "qwen3.7-plus",
    "qwen3.7-flash",
    "qwen3.6-flash",
    // 豆包 doubao-seed-* 现役五行与编程特化预览行的官方能力列均含多模态理解
    // (volcengine docs 82379/1330310,2026-09-11),整族收录成立。
    "doubao-seed",
    // MiniMax 仅 M3 支持图片输入,M2.x 不支持,不能用 minimax 整体子串
    // (2026-09-11 官方口径)。
    "minimax-m3",
    // 智谱 GLM-5.3-Flash 原生多模态;glm-5.3 / glm-5.2 纯文本,不能用 glm-5.3
    // 整体子串(2026-09-11)。glm-4v 条目保留为兼容存量配置:在售仅剩
    // glm-4v-flash(免费档),glm-4v / glm-4v-plus 已不在在售表与 API enum。
    "glm-5.3-flash",
    "glm-4v",
    // Kimi(2026-09-11):Kimi 直连 kimi-k3 与 Kimi Code k3 / k3-256k 官方均为
    // 图片输入;kimi-k3 走子串条目,k3 / k3-256k 属短泛型 id,改按
    // EXACT_VERIFIED_IMAGE_CAPABLE_MODELS 精确收录(见其注释);
    // kimi-k2.7-code 与 kimi-k2.6 官方定价页为文本/图片/视频输入
    // (platform.kimi.com);kimi-k2.5 等其余文本模型不收。
    "kimi-for-coding",
    "kimi-k3",
    "kimi-k2.7-code",
    "kimi-k2.6",
];

/// 精确(小写全等)收录条目:子串命中面过宽的短泛型 id 或「同名纯文本变体」
/// 只能按全等收录——裸 "k3" 子串会让任何含 "k3" 的自定义名(第三方/聚合器的
/// 无关模型等)被判为原生识图并在发送时内联图片;mimo-v2.5 若走子串会连带
/// 纯文本的 mimo-v2.5-pro。两者都违反本表「宁可 Unknown 不可误判 Supported」
/// 的收录原则,故只按全等收录。未来出现新的 -档位拼写时应在此追加,而不是
/// 回退子串。
const EXACT_VERIFIED_IMAGE_CAPABLE_MODELS: &[&str] = &["k3", "k3-256k", "mimo-v2.5"];

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
            // preset 默认模型(prefs `default_model`)必须命中,否则官方路由退化成 Unknown。
            (ModelPreset::OpenaiCompatible, "gpt-5.6-terra"),
            // gpt-6-astra 官方多模态,由 "gpt-6" 条目覆盖("gpt-5" 命中不了)。
            (ModelPreset::OpenaiCompatible, "gpt-6-astra"),
            (ModelPreset::OpenaiCompatible, "claude-3-5-sonnet-20241022"),
            (ModelPreset::OpenaiCompatible, "claude-4-opus"),
            // 默认预设(claude-sonnet-5 / grok-4.6 / deepseek-flash / qwen3.8-max /
            // MiniMax-M3 / kimi-k3)必须命中,否则官方路由退化成 Unknown。
            (ModelPreset::OpenaiCompatible, "claude-sonnet-5"),
            (ModelPreset::OpenaiCompatible, "gemini-2.5-pro"),
            (ModelPreset::OpenaiCompatible, "grok-4.6"),
            // 旧表漏配修复:claude-haiku-4-5 / claude-fable-5-1 均为现役多模态
            // (platform.claude.com models overview,2026-09-11)。
            (ModelPreset::OpenaiCompatible, "claude-haiku-4-5"),
            (ModelPreset::OpenaiCompatible, "claude-fable-5"),
            (ModelPreset::OpenaiCompatible, "claude-fable-5-1"),
            // V4.1-Flash 原生视觉(api-docs.deepseek.com/guides/vision)。
            (ModelPreset::Deepseek, "deepseek-flash"),
            // 退役别名仍路由至 V4.1-Flash,存量配置须同样解析为 Supported。
            (ModelPreset::Deepseek, "deepseek-v4-flash"),
            (ModelPreset::Deepseek, "deepseek-v4-flash-vision-exp"),
            (ModelPreset::Qwen, "qwen-vl-max"),
            (ModelPreset::Qwen, "Qwen2.5-VL-72B-Instruct"),
            (ModelPreset::Qwen, "qwen3.8-max"),
            (ModelPreset::Qwen, "qwen3.8-flash"),
            (ModelPreset::Qwen, "qwen3.7-plus"),
            (ModelPreset::Qwen, "qwen3.7-flash"),
            (ModelPreset::Qwen, "qwen3.6-flash"),
            (ModelPreset::Glm, "glm-4v-plus"),
            (ModelPreset::Glm, "glm-5.3-flash"),
            (ModelPreset::Doubao, "doubao-seed-evolving"),
            (ModelPreset::Minimax, "MiniMax-M3"),
            // MiMo(2026-09-11):多模态在 mimo-v2.5,纯文本的 mimo-v2.5-pro 不得
            // 连带命中,故走精确全等表(见 EXACT_VERIFIED_IMAGE_CAPABLE_MODELS)。
            (ModelPreset::Mimo, "mimo-v2.5"),
            // Kimi 直连 kimi-k3 与 Kimi Code k3 / k3-256k 官方均为图片输入
            // (2026-09-11);kimi-for-coding 用户实测可识图(2026-07)。
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
        // - 含 "k3" 的无关自定义名不得被子串误判(裸 k3 已改精确收录)。
        for (preset, name) in [
            (ModelPreset::Deepseek, "deepseek-v4-pro"),
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
    fn kimi_for_coding_unaffected_by_k27_code_text_marker() {
        // 底座目录把 kimi-k2.7-code 标为 text-only,但 kimi-for-coding 是另一部署
        // (用户实测可识图):目录查不到该 id,内置表(实测记录)判 Supported。
        let model = saved_model(ModelPreset::Kimi, "kimi-for-coding");
        assert_eq!(
            effective_image_capability(&model),
            EffectiveImageCapability::Supported
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

//! 模型图片输入能力解析(设计 §6.3,阶段 C)。
//!
//! 能力判断按**具体模型**而非 provider/preset/ACP adapter:不能因为 local_vllm
//! 或某 provider 协议上能收图片,就假定当前模型能识图(设计 §1.5/§7)。
//!
//! 解析优先级:
//! 1. 用户对 SavedModel 的显式 override(`Enabled`→Supported,`Disabled`→Unsupported);
//! 2. 底座模型目录(`deepseek_tui::model_catalog`,合并链:用户覆盖 → provider 缓存
//!    → 内置快照)条目 `modalities` 含 `image` → Supported;命中但不含 image **不下
//!    否定结论**(目录否定标记不可全信,如 claude-opus-4-8 标 text-only),继续下落;
//! 3. 内置已验证能力表(`VERIFIED_IMAGE_CAPABLE_MODELS`);
//! 4. 都判不出 → `Unknown`(默认不冒充支持,允许用户在设置里 override Enabled)。
//!
//! ⚠️ 内置表宁可 Unknown 不可误判 Supported:只对明确多模态的模型名子串判中。
//! 本地自定义模型(尤其 LocalVllm 的 `qwen36_35b_256k`,文本/多模态两种部署都存在,
//! 见设计 §7.1/§7.2)一律 Unknown,交给用户显式确认。
//!
//! ACP 链路(设计 §6.4,阶段 F)复用同一份解析规则:Codex ACP session 的当前模型
//! 由 ACP agent 自己管理(session config option "model"),与 pinvou3 SavedModel
//! 无绑定;`acp_model_image_capability` 按 wire model 名承接用户 override 与内置表。

use crate::platform::prefs::{ImageCapabilityOverride, SavedModel, UserPrefs};

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

/// Moonshot always-thinking 模型判定(bridge `moonshot_model_requires_explicit_thinking`
/// 同一名单):这些模型官方接入要求 `thinking: {"type":"enabled"}` 保持开启,
/// 省略该参数的请求会被网关拒绝。探测请求与真实链路必须同一口径,否则
/// kimi-for-coding 等模型的识图探测会 400 误判(2026-08 kimi-for-coding 实测)。
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
/// 收录原则:仅明确多模态的模型族,且能从仓内 preset 默认模型或公开事实佐证;
/// 拿不准的一律不收(走 Unknown + 用户 override)。
const VERIFIED_IMAGE_CAPABLE_MODELS: &[&str] = &[
    // OpenAI 多模态世代。OpenaiCompatible preset 默认模型 `gpt-5.6-terra`
    // (prefs.rs `default_model`)即 gpt-5 族。
    "gpt-4o",
    "gpt-4.1",
    "gpt-5",
    // Anthropic Claude 3/4/5 全系视觉输入(默认预设 claude-sonnet-5 命中 claude-5)。
    "claude-3",
    "claude-4",
    "claude-5",
    // Google Gemini 全系多模态。
    "gemini",
    // xAI Grok 全系视觉输入(默认预设 grok-4.3 命中)。
    "grok",
    // 阿里 Qwen VL 系列(qwen-vl / qwen2-vl / qwen2.5-vl / qwen3-vl)。
    // 裸 qwen 名(qwen3.7-plus 等文本模型)不收——见设计 §7.2。
    "qwen-vl",
    "qwen2-vl",
    "qwen2.5-vl",
    "qwen3-vl",
    // 智谱 GLM-4V 视觉系列;glm-5.x 未经验证不收。
    "glm-4v",
    // Kimi for Coding(Moonshot 编程计划模型):用户实测可原生识图(2026-07)。
    // 其余 kimi 文本模型(kimi-k3 等)不收。
    "kimi-for-coding",
];

/// 内置表查询:模型名(小写化)是否命中已验证多模态条目。
fn builtin_verified_supports_image(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return false;
    }
    VERIFIED_IMAGE_CAPABLE_MODELS
        .iter()
        .any(|entry| normalized.contains(entry))
}

/// 解析一条 SavedModel 的生效图片输入能力(优先级见模块头注释)。
pub fn effective_image_capability(model: &SavedModel) -> EffectiveImageCapability {
    // ① 显式档位优先:Enabled(能)/Disabled(不能)直接钉死。
    match model.image_capability_override {
        ImageCapabilityOverride::Enabled => return EffectiveImageCapability::Supported,
        ImageCapabilityOverride::Disabled => return EffectiveImageCapability::Unsupported,
        // Auto(自动探测,保存后即被回填,残留值按 pinvou 决策兜底)
        // 与 Pinvou(pinvou 决策)共用同一条内置表判断链。
        ImageCapabilityOverride::Auto | ImageCapabilityOverride::Pinvou => {}
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

/// ACP 会话当前模型的图片输入能力(设计 §6.4,阶段 F)。
///
/// ACP session 的模型由 ACP agent 管理,不是 pinvou3 SavedModel;解析规则与普通
/// 会话保持一致:wire model 名命中用户 SavedModel 时走完整解析链(承接显式
/// override),否则依次查底座模型目录与内置已验证表,判不出 → Unknown(不冒充
/// 支持,由 ACP 侧提示用户在模型设置里确认)。`acp_model_id` 缺失(会话未上报
/// 模型)同样 Unknown。
pub fn acp_model_image_capability(
    prefs: &UserPrefs,
    acp_model_id: Option<&str>,
) -> EffectiveImageCapability {
    let Some(model_id) = acp_model_id
        .map(str::trim)
        .filter(|model_id| !model_id.is_empty())
    else {
        return EffectiveImageCapability::Unknown;
    };
    if let Some(saved) = prefs
        .advanced
        .saved_models
        .iter()
        .find(|model| model.model.eq_ignore_ascii_case(model_id))
    {
        return effective_image_capability(saved);
    }
    // wire model 未命中 SavedModel:与普通会话同一规则,落内置已验证表
    // (v0.9.5 起底座 model_catalog 不再公开,目录级判定由底座请求前剥离承担)。
    if builtin_verified_supports_image(model_id) {
        EffectiveImageCapability::Supported
    } else {
        EffectiveImageCapability::Unknown
    }
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
            preset,
            context_window_tokens: None,
            max_output_tokens: None,
            reasoning_effort: None,
            model: model.to_string(),
            base_url: "https://example.invalid/v1".to_string(),
            provider_kind: None,
            vendor: None,
            endpoint_mode: None,
            image_capability_override: ImageCapabilityOverride::Auto,
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
        // 名单与 bridge.rs moonshot_model_requires_explicit_thinking 保持一致;
        // 探测 payload 与真实链路必须同一口径,否则 always-thinking 模型
        // 识图探测会被网关 400 误判(2026-08 kimi-for-coding 实测)。
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
            // preset 默认模型(prefs.rs)必须命中,否则官方 OpenAI 路由退化成 Unknown。
            (ModelPreset::OpenaiCompatible, "gpt-5.6-terra"),
            (ModelPreset::OpenaiCompatible, "claude-3-5-sonnet-20241022"),
            (ModelPreset::OpenaiCompatible, "claude-4-opus"),
            // 默认预设(claude-sonnet-5 / grok-4.3)必须命中,否则官方路由退化成 Unknown。
            (ModelPreset::OpenaiCompatible, "claude-sonnet-5"),
            (ModelPreset::OpenaiCompatible, "gemini-2.5-pro"),
            (ModelPreset::OpenaiCompatible, "grok-4.3"),
            (ModelPreset::Qwen, "qwen-vl-max"),
            (ModelPreset::Qwen, "Qwen2.5-VL-72B-Instruct"),
            (ModelPreset::Glm, "glm-4v-plus"),
            // 用户实测可原生识图(2026-07),与 kimi-k3 等文本模型区分。
            (ModelPreset::Kimi, "kimi-for-coding"),
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
    fn builtin_table_misses_text_models() {
        // 各 preset 默认文本模型不得误判 Supported(目录级判定已移除,
        // 仅内置已验证表决定;mimo-v2.5-pro 见 catalog_models_not_in_builtin_table_stay_unknown)。
        for (preset, name) in [
            (ModelPreset::Deepseek, "deepseek-v4-pro"),
            (ModelPreset::Kimi, "kimi-k3"),
            (ModelPreset::Qwen, "qwen3.7-plus"),
            (ModelPreset::Doubao, "doubao-seed-evolving"),
            (ModelPreset::Minimax, "MiniMax-M3"),
            (ModelPreset::Glm, "glm-5.2"),
        ] {
            let model = saved_model(preset, name);
            assert_eq!(
                effective_image_capability(&model),
                EffectiveImageCapability::Unknown,
                "{name} 不应被误判为支持图片"
            );
        }
    }

    #[test]
    fn catalog_models_not_in_builtin_table_stay_unknown() {
        // v0.9.5 起底座 model_catalog 不再公开,目录级 modalities 判定已移除
        // (image_capability.rs 第②级为死代码)。mimo-v2.5-pro / muse-spark-1.1
        // 不在内置已验证表内,能力判定落 Unknown——不再有「目录级升级」路径。
        for (preset, name) in [
            (ModelPreset::Mimo, "mimo-v2.5-pro"),
            (ModelPreset::OpenaiCompatible, "muse-spark-1.1"),
        ] {
            let model = saved_model(preset, name);
            assert_eq!(
                effective_image_capability(&model),
                EffectiveImageCapability::Unknown,
                "{name} 不在内置已验证表,目录级判定已移除,应判 Unknown"
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
    fn pinvou_decision_follows_builtin_table_like_auto() {
        // Pinvou(pinvou 决策)= 原 Auto 判定链:内置表命中 → Supported,
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
        // Auto 与 Pinvou 对同一模型判定一致。
        let auto = saved_model(ModelPreset::OpenaiCompatible, "gpt-4o");
        let pinvou = {
            let mut m = saved_model(ModelPreset::OpenaiCompatible, "gpt-4o");
            m.image_capability_override = ImageCapabilityOverride::Pinvou;
            m
        };
        assert_eq!(
            effective_image_capability(&auto),
            effective_image_capability(&pinvou)
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

    #[test]
    fn acp_model_without_saved_model_uses_builtin_table() {
        let prefs = UserPrefs::default();
        // 会话未上报模型 / 空 id:Unknown,不冒充支持。
        assert_eq!(
            acp_model_image_capability(&prefs, None),
            EffectiveImageCapability::Unknown
        );
        assert_eq!(
            acp_model_image_capability(&prefs, Some("  ")),
            EffectiveImageCapability::Unknown
        );
        // Codex ACP 当前模型(gpt-5 族)命中内置已验证表。
        assert_eq!(
            acp_model_image_capability(&prefs, Some("gpt-5.6-sol")),
            EffectiveImageCapability::Supported
        );
        // 已知文本模型与自定义模型:Unknown,交给用户显式确认。
        assert_eq!(
            acp_model_image_capability(&prefs, Some("deepseek-v4-pro")),
            EffectiveImageCapability::Unknown
        );
        assert_eq!(
            acp_model_image_capability(&prefs, Some("my-finetune-7b")),
            EffectiveImageCapability::Unknown
        );
    }

    #[test]
    fn acp_model_inherits_override_from_same_named_saved_model() {
        let mut prefs = UserPrefs::default();
        // override Enabled:内置表判不出的模型 → Supported(用户显式确认的出口)。
        let mut enabled = saved_model(ModelPreset::OpenaiCompatible, "my-finetune-7b");
        enabled.id = "enabled".to_string();
        enabled.image_capability_override = ImageCapabilityOverride::Enabled;
        // override Disabled:内置表命中的模型 → Unsupported(已知文本模型不再显示支持)。
        let mut disabled = saved_model(ModelPreset::OpenaiCompatible, "GPT-5.6-SOL");
        disabled.id = "disabled".to_string();
        disabled.image_capability_override = ImageCapabilityOverride::Disabled;
        prefs.advanced.saved_models = vec![enabled, disabled];
        assert_eq!(
            acp_model_image_capability(&prefs, Some("my-finetune-7b")),
            EffectiveImageCapability::Supported
        );
        // wire model 名匹配大小写不敏感(ACP 上报 id 与 SavedModel 书写形式可能不同)。
        assert_eq!(
            acp_model_image_capability(&prefs, Some("gpt-5.6-sol")),
            EffectiveImageCapability::Unsupported
        );
    }
}

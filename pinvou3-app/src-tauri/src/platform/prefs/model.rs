//! 模型预设（ModelPreset）与其模板/端点事实，自 prefs 模块抽离。
//!
//! `ModelPreset` 决定 provider 路由 + 添加模型模板；默认 base_url/model、
//! Coding Plan 端点识别、直连 chat/completions 的能力校验与上下文窗口兜底
//! 都收敛在这一处，bridge / monitor / memory / review 不再各自抄一份。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelPreset {
    /// 默认本地 vLLM：qwen36_35b_256k @ 127.0.0.1:8000/v1
    LocalVllm,
    /// DeepSeek 官方 API
    Deepseek,
    /// Kimi (Moonshot)
    Kimi,
    /// OpenAI 兼容 API（自托管 / 代理 / 其他 OpenAI 兼容厂商；OpenAI 官方请用 Openai）
    OpenaiCompatible,
    /// 通义千问 (Qwen)
    Qwen,
    /// 豆包 (火山方舟)
    Doubao,
    /// MiniMax
    Minimax,
    /// 智谱 GLM
    Glm,
    /// 小米 MiMo
    Mimo,
    /// OpenAI 官方 API
    Openai,
    /// Anthropic Claude（Messages 原生协议，底座内建 anthropic provider）
    Anthropic,
    /// Google Gemini（OpenAI 兼容端点）
    Gemini,
    /// xAI Grok
    Xai,
}

pub const MODEL_PROVIDER_KIND_CODING_PLAN: &str = "coding_plan";
pub const MODEL_PROVIDER_KIND_OFFICIAL_API: &str = "official_api";
pub const MODEL_PROVIDER_KIND_CUSTOM: &str = "custom";

pub(super) fn trim_url_tail(value: &str) -> String {
    value.trim().trim_end_matches('/').to_string()
}

pub(crate) fn strip_chat_completions_suffix(value: &str) -> String {
    let trimmed = trim_url_tail(value);
    let lower = trimmed.to_ascii_lowercase();
    if lower.ends_with("/chat/completions") {
        trimmed[..trimmed.len() - "/chat/completions".len()].to_string()
    } else {
        trimmed
    }
}

pub(super) fn migrated_minimax_base_url(value: &str) -> Option<String> {
    const LEGACY_ORIGIN: &str = "https://api.minimax.chat";
    const CURRENT_ORIGIN: &str = "https://api.minimaxi.com";

    let trimmed = value.trim().trim_end_matches('/');
    let lower = trimmed.to_ascii_lowercase();
    if lower == LEGACY_ORIGIN {
        return Some(CURRENT_ORIGIN.to_string());
    }
    lower.strip_prefix(LEGACY_ORIGIN).and_then(|suffix| {
        suffix
            .starts_with('/')
            .then(|| format!("{CURRENT_ORIGIN}{}", &trimmed[LEGACY_ORIGIN.len()..]))
    })
}

pub(super) fn identify_coding_plan_endpoint(
    base_url: &str,
) -> Option<(&'static str, &'static str)> {
    let base = strip_chat_completions_suffix(base_url);
    let normalized = base.to_ascii_lowercase();
    match normalized.as_str() {
        "https://open.bigmodel.cn/api/coding/paas/v4" => {
            Some(("glm", "https://open.bigmodel.cn/api/coding/paas/v4"))
        }
        "https://api.z.ai/api/coding/paas/v4" => {
            Some(("glm", "https://api.z.ai/api/coding/paas/v4"))
        }
        "https://api.kimi.com/coding/v1" => Some(("kimi", "https://api.kimi.com/coding/v1")),
        "https://api.lkeap.cloud.tencent.com/coding/v3" => {
            Some(("tencent", "https://api.lkeap.cloud.tencent.com/coding/v3"))
        }
        "https://api.lkeap.cloud.tencent.com/plan/v3" => {
            Some(("tencent", "https://api.lkeap.cloud.tencent.com/plan/v3"))
        }
        _ => None,
    }
}

impl Default for ModelPreset {
    /// 平台感知默认预设:macOS/Windows 无本地 vLLM 支持(相关后端命令已 cfg 掉),
    /// 默认到 DeepSeek 远程 API,否则新用户首启即落在 127.0.0.1:8000 永远连不上。
    /// Linux 保持 LocalVllm(麒麟环境默认有本地大模型)。
    fn default() -> Self {
        #[cfg(target_os = "linux")]
        {
            ModelPreset::LocalVllm
        }
        #[cfg(not(target_os = "linux"))]
        {
            ModelPreset::Deepseek
        }
    }
}
impl ModelPreset {
    /// 与前端 preset key、settings.json 序列化值一致的稳定串(snake_case)。
    pub fn as_str(&self) -> &'static str {
        match self {
            ModelPreset::LocalVllm => "local_vllm",
            ModelPreset::Deepseek => "deepseek",
            ModelPreset::Kimi => "kimi",
            ModelPreset::OpenaiCompatible => "openai_compatible",
            ModelPreset::Qwen => "qwen",
            ModelPreset::Doubao => "doubao",
            ModelPreset::Minimax => "minimax",
            ModelPreset::Glm => "glm",
            ModelPreset::Mimo => "mimo",
            ModelPreset::Openai => "openai",
            ModelPreset::Anthropic => "anthropic",
            ModelPreset::Gemini => "gemini",
            ModelPreset::Xai => "xai",
        }
    }
    /// 各预设默认 base_url(bridge `default_base_url_for_preset` 委托到这里;迁移/添加模型模板兜底)。
    /// LocalVllm 用 127.0.0.1 让 .deb 装到任何机器都默认连本机 vLLM(全量包 install.sh
    /// 起 systemd 容器 --network host 绑 0.0.0.0:8000);vLLM 与应用同机,
    /// 用 loopback 免疫 DHCP 换 IP,别再写具体内网 IP。
    pub fn default_base_url(&self) -> &'static str {
        match self {
            ModelPreset::LocalVllm => "http://127.0.0.1:8000/v1",
            ModelPreset::Deepseek => "https://api.deepseek.com",
            ModelPreset::Kimi => "https://api.moonshot.cn/v1",
            ModelPreset::OpenaiCompatible => "https://api.openai.com/v1",
            ModelPreset::Qwen => "https://dashscope.aliyuncs.com/compatible-mode/v1",
            ModelPreset::Doubao => "https://ark.cn-beijing.volces.com/api/v3",
            ModelPreset::Minimax => "https://api.minimaxi.com/v1",
            ModelPreset::Glm => "https://open.bigmodel.cn/api/paas/v4",
            ModelPreset::Mimo => "https://api.xiaomimimo.com/v1",
            ModelPreset::Openai => "https://api.openai.com/v1",
            ModelPreset::Anthropic => "https://api.anthropic.com/v1",
            ModelPreset::Gemini => "https://generativelanguage.googleapis.com/v1beta/openai",
            ModelPreset::Xai => "https://api.x.ai/v1",
        }
    }
    /// 各预设默认模型名(bridge `default_model_for_preset` 委托到这里)。
    /// LocalVllm 的 `qwen36_35b_256k` 在 vLLM 里是 passthrough 字符串(不走 alias):
    /// 后缀 `_256k` 由 fork B1 (`context_window_for_model` 的 `_Nk` hint) 识别,
    /// 让底座为本地 Qwen 派生 256K 窗口 → context_input_budget / capacity ratio /
    /// compaction 派生路径全部能算对。若改名为无后缀,底座立刻退化到 `None`,
    /// preflight + emergency recovery 默认不生效。回归测试
    /// `bridge::tests::default_model_window_recognized` 锁住这个不变量。
    /// ⚠️ ops 同步要求:vLLM 启动也要带 `--served-model-name qwen36_35b_256k`,
    /// 否则 OpenAI-compat API 报 `model_not_found`。
    pub fn default_model(&self) -> &'static str {
        match self {
            ModelPreset::LocalVllm => "qwen36_35b_256k",
            ModelPreset::Deepseek => "deepseek-v4-pro",
            ModelPreset::Kimi => "kimi-k3",
            ModelPreset::OpenaiCompatible => "gpt-5.6-terra",
            ModelPreset::Qwen => "qwen3.8-max",
            ModelPreset::Doubao => "doubao-seed-evolving",
            ModelPreset::Minimax => "MiniMax-M3",
            ModelPreset::Glm => "glm-5.2",
            ModelPreset::Mimo => "mimo-v2.5-pro",
            ModelPreset::Openai => "gpt-5.6-terra",
            ModelPreset::Anthropic => "claude-sonnet-5",
            ModelPreset::Gemini => "gemini-3.6-flash",
            ModelPreset::Xai => "grok-4.3",
        }
    }
}

impl ModelPreset {
    /// 底座 catalog 与 pinvou 补充表（core::model_context）都无法识别模型时的
    /// 供应商预设上下文窗口兜底（自 monitor `infer_context_window` 归位）。
    pub fn context_window_fallback(self, model: Option<&str>) -> Option<u32> {
        match self {
            ModelPreset::LocalVllm => Some(262_144),
            ModelPreset::Deepseek => Some(131_072),
            ModelPreset::Kimi => Some(262_144),
            ModelPreset::OpenaiCompatible => Some(131_072),
            ModelPreset::Qwen => Some(131_072),
            ModelPreset::Doubao => Some(262_144),
            ModelPreset::Minimax => Some(204_800),
            ModelPreset::Glm => Some(131_072),
            ModelPreset::Mimo => Some(1_000_000),
            // OpenAI 官方口径：gpt-5.4-mini / gpt-5.3-codex 为 400K，其余现役旗舰 1.05M。
            ModelPreset::Openai => match model.map(str::to_ascii_lowercase) {
                Some(m) if m.contains("gpt-5.4-mini") || m.contains("gpt-5.3-codex") => {
                    Some(400_000)
                }
                _ => Some(1_050_000),
            },
            // Anthropic 官方口径：haiku 200K，opus/sonnet/fable 1M
            // （claude-opus-5 由 model_context 的 PINVOU_OVERRIDES 先行覆盖，此处兜
            // 底只承接底座不认识的命名）。
            ModelPreset::Anthropic => match model.map(str::to_ascii_lowercase) {
                Some(m) if m.contains("haiku") => Some(200_000),
                _ => Some(1_000_000),
            },
            // Gemini 全系标称 1M。
            ModelPreset::Gemini => Some(1_048_576),
            // xAI 官方口径：grok-4.20 系 2M、grok-4.3 1M、grok-4.5 500K、grok-build 256K。
            ModelPreset::Xai => match model.map(str::to_ascii_lowercase) {
                Some(m) if m.contains("grok-4.20") => Some(2_000_000),
                Some(m) if m.contains("grok-4.3") => Some(1_000_000),
                Some(m) if m.contains("grok-4.5") => Some(500_000),
                Some(m) if m.contains("grok-build") => Some(256_000),
                _ => Some(256_000),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 预设上下文窗口兜底：各厂商官方口径与未知型号的缺省值。
    #[test]
    fn context_window_fallback_matches_vendor_defaults() {
        let cases: &[(ModelPreset, Option<&str>, u32)] = &[
            // OpenAI 预设兜底：gpt-5.4-mini / gpt-5.3-codex 400K，其余 1.05M
            (ModelPreset::Openai, Some("gpt-5.4-mini"), 400_000),
            (ModelPreset::Openai, Some("gpt-5.3-codex"), 400_000),
            (ModelPreset::Openai, Some("gpt-5.6-sol"), 1_050_000),
            (ModelPreset::Openai, None, 1_050_000),
            // Anthropic：haiku 200K；底座不认识的非 claude 命名模型兜底 1M
            (
                ModelPreset::Anthropic,
                Some("anthropic-future-model"),
                1_000_000,
            ),
            (ModelPreset::Anthropic, None, 1_000_000),
            // Gemini 全系标称 1M
            (ModelPreset::Gemini, Some("gemini-3.6-flash"), 1_048_576),
            // xAI 预设兜底：grok-4.20 系 2M、grok-4.3 1M、grok-4.5 500K、grok-build 256K
            (
                ModelPreset::Xai,
                Some("grok-4.20-0309-reasoning"),
                2_000_000,
            ),
            (ModelPreset::Xai, Some("grok-4.3"), 1_000_000),
            (ModelPreset::Xai, Some("grok-4.5"), 500_000),
            (ModelPreset::Xai, Some("grok-build-0.1"), 256_000),
            (ModelPreset::Xai, Some("grok-future-x"), 256_000),
            (ModelPreset::Xai, None, 256_000),
            // 其余预设的固定兜底
            (ModelPreset::Deepseek, Some("my-custom-finetune"), 131_072),
            (ModelPreset::Minimax, Some("minimax-future-x"), 204_800),
            (ModelPreset::Mimo, Some("mimo-future-x"), 1_000_000),
            (ModelPreset::Kimi, None, 262_144),
        ];
        for (preset, model, expected) in cases {
            assert_eq!(
                preset.context_window_fallback(*model),
                Some(*expected),
                "{preset:?}/{model:?} 上下文窗口兜底错误"
            );
        }
    }
}

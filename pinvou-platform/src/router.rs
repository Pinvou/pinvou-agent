//! 模型路由器 — 根据任务类型自动选择合适的模型和量化等级。
//!
//! GB10 策略: 不搞多模型常驻，而是任务驱动热切换。
//! 统一内存让切换延迟在百毫秒级，用户无感。

#![allow(dead_code)] // Phase 1 定义，Phase 2 使用

/// 任务复杂度
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskComplexity {
    /// 简单分类/摘要/shell 命令
    Simple,
    /// 常规分析、文档生成
    Medium,
    /// 复杂推理、多步规划、方案对比
    Complex,
}

/// 路由决策
#[derive(Debug, Clone)]
pub struct RouteDecision {
    /// 推荐模型 ID
    pub model_id: String,
    /// 推荐 provider
    pub provider: String,
    /// 推荐量化等级: "fp8" / "q4" / "q2"
    pub quantization: Option<String>,
    /// 推荐上下文长度
    pub context_length: u32,
}

/// 模型路由器
#[derive(Debug)]
pub struct ModelRouter {
    /// 可用模型列表
    models: Vec<ModelEntry>,
    /// 默认 model
    default_model: String,
    /// 当前显存预算 (MiB)
    vram_budget_mib: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ModelEntry {
    pub id: String,
    pub provider: String,
    pub capability: String,
    pub vram_required_mib: u64,
    pub context_length: u32,
    pub quantizations: Vec<String>,
}

impl ModelRouter {
    pub fn new(models: Vec<ModelEntry>, default_model: String) -> Self {
        Self {
            models,
            default_model,
            vram_budget_mib: None,
        }
    }

    /// 设置显存预算（从 NVML 获取，GB10 优化）
    pub fn set_vram_budget(&mut self, budget_mib: u64) {
        self.vram_budget_mib = Some(budget_mib);
    }

    /// 根据任务复杂度路由到最佳模型
    pub fn route(&self, complexity: TaskComplexity) -> RouteDecision {
        let capability = match complexity {
            TaskComplexity::Simple => "small",
            TaskComplexity::Medium => "medium",
            TaskComplexity::Complex => "large",
        };

        // 先按能力匹配，再按显存过滤，最后选最佳的
        let candidates: Vec<_> = self
            .models
            .iter()
            .filter(|m| m.capability == capability)
            .filter(|m| {
                self.vram_budget_mib
                    .map(|budget| m.vram_required_mib <= budget)
                    .unwrap_or(true)
            })
            .collect();

        if let Some(best) = candidates.first() {
            RouteDecision {
                model_id: best.id.clone(),
                provider: best.provider.clone(),
                quantization: best.quantizations.first().cloned(),
                context_length: best.context_length,
            }
        } else {
            // fallback 到默认
            let default = self
                .models
                .iter()
                .find(|m| m.id == self.default_model)
                .unwrap_or_else(|| &self.models[0]);
            RouteDecision {
                model_id: default.id.clone(),
                provider: default.provider.clone(),
                quantization: default.quantizations.first().cloned(),
                context_length: default.context_length,
            }
        }
    }

    /// 从应用配置推断任务复杂度
    pub fn complexity_from_app(app_model_preference: &str) -> TaskComplexity {
        match app_model_preference {
            "small" => TaskComplexity::Simple,
            "large" => TaskComplexity::Complex,
            _ => TaskComplexity::Medium,
        }
    }

    /// 列出可用模型
    pub fn list_models(&self) -> &[ModelEntry] {
        &self.models
    }
}

/// GB10 预置模型配置
pub fn gb10_default_models() -> Vec<ModelEntry> {
    vec![
        ModelEntry {
            id: "qwen3-0.6b".into(),
            provider: "ollama".into(),
            capability: "small".into(),
            vram_required_mib: 1200,
            context_length: 32768,
            quantizations: vec!["q4".into()],
        },
        ModelEntry {
            id: "qwen3-8b".into(),
            provider: "ollama".into(),
            capability: "medium".into(),
            vram_required_mib: 6000,
            context_length: 131072,
            quantizations: vec!["q4".into(), "fp8".into()],
        },
        ModelEntry {
            id: "qwen3-35b-a3b".into(),
            provider: "vllm".into(),
            capability: "large".into(),
            vram_required_mib: 45000,
            context_length: 131072,
            quantizations: vec!["fp8".into()],
        },
        ModelEntry {
            id: "bge-small-zh".into(),
            provider: "ollama".into(),
            capability: "embedding".into(),
            vram_required_mib: 130,
            context_length: 512,
            quantizations: vec!["fp16".into()],
        },
    ]
}

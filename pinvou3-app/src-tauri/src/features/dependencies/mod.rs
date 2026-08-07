mod platform;

/// 平台对“依赖体检应展示哪些可修复能力”的语义化策略。
/// 调用层只消费能力，不直接判断操作系统；具体修复仍由对应业务功能执行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DependencyCheckPolicy {
    pub(crate) include_voice_runtime: bool,
    pub(crate) include_voice_model: bool,
    pub(crate) include_knowledge_model: bool,
}

pub(crate) fn dependency_check_policy() -> DependencyCheckPolicy {
    platform::dependency_check_policy()
}

pub fn install_dependencies(packages: Vec<String>) -> Result<(), String> {
    platform::install_dependencies(packages)
}

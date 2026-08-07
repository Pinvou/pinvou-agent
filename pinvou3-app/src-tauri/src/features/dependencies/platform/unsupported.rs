use super::super::DependencyCheckPolicy;

pub(super) fn dependency_check_policy() -> DependencyCheckPolicy {
    DependencyCheckPolicy {
        include_voice_runtime: true,
        include_voice_model: false,
        include_knowledge_model: false,
    }
}

pub fn install_dependencies(_packages: Vec<String>) -> Result<(), String> {
    Err("当前系统不支持一键安装依赖；请按本系统方式手动安装缺失工具".into())
}

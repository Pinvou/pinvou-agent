#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
pub use windows::{check_dependencies, install_dependencies};

#[cfg(not(target_os = "windows"))]
pub fn check_dependencies() -> Vec<crate::features::files::file_ingest::DependencyCheckItem> {
    crate::features::files::file_ingest::check_dependencies()
}

#[cfg(not(target_os = "windows"))]
pub async fn install_dependencies(
    packages: Vec<String>,
    actions: Vec<String>,
    _app: tauri::AppHandle,
    _knowledge: tauri::State<'_, crate::features::knowledge::KnowledgeService>,
    _pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<(), String> {
    if !actions.is_empty() {
        return Err("当前平台不支持该依赖安装动作".to_string());
    }
    crate::features::files::file_ingest::install_dependencies(packages).await
}

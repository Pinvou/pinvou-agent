use super::prelude::*;
use crate::features::files::file_ingest::DependencyCheckItem;

mod platform;

#[tauri::command]
pub fn check_dependencies() -> Vec<DependencyCheckItem> {
    platform::check_dependencies()
}

#[tauri::command]
pub async fn install_dependencies(
    packages: Vec<String>,
    actions: Option<Vec<String>>,
    app: AppHandle,
    knowledge: State<'_, KnowledgeService>,
    pool: State<'_, EnginePool>,
) -> Result<(), String> {
    platform::install_dependencies(packages, actions.unwrap_or_default(), app, knowledge, pool)
        .await
}

use super::prelude::*;
use crate::features::files::file_ingest as dependencies_domain;
use dependencies_domain::*;

const INSTALL_VOICE_MODEL: &str = "voice_asr_model";
const INSTALL_KNOWLEDGE_MODEL: &str = "knowledge_embedding_model";

fn dependency_items() -> Vec<DependencyCheckItem> {
    let mut items = dependencies_domain::check_dependencies();
    let policy = crate::features::dependencies::dependency_check_policy();
    if policy.include_voice_model {
        items.push(DependencyCheckItem {
            key: INSTALL_VOICE_MODEL.into(),
            installed: crate::features::voice::voice_asr::status().model,
            apt: String::new(),
            install_action: Some(INSTALL_VOICE_MODEL.into()),
        });
    }
    if policy.include_knowledge_model {
        items.push(DependencyCheckItem {
            key: INSTALL_KNOWLEDGE_MODEL.into(),
            installed: crate::features::knowledge::model_download::model_installed(),
            apt: String::new(),
            install_action: Some(INSTALL_KNOWLEDGE_MODEL.into()),
        });
    }
    items
}

#[tauri::command]
pub async fn check_dependencies() -> Vec<DependencyCheckItem> {
    tokio::task::spawn_blocking(dependency_items)
        .await
        .unwrap_or_else(|_| dependency_items())
}

fn requested_model_installs(actions: &[String]) -> Result<(bool, bool), String> {
    let mut voice = false;
    let mut knowledge = false;
    for action in actions {
        match action.as_str() {
            INSTALL_VOICE_MODEL => voice = true,
            INSTALL_KNOWLEDGE_MODEL => knowledge = true,
            _ => return Err(format!("不支持的依赖安装动作: {action}")),
        }
    }
    Ok((voice, knowledge))
}

#[tauri::command]
pub async fn install_dependencies(
    packages: Vec<String>,
    actions: Option<Vec<String>>,
    app: AppHandle,
    knowledge: State<'_, KnowledgeService>,
    pool: State<'_, EnginePool>,
) -> Result<(), String> {
    let actions = actions.unwrap_or_default();
    let (install_voice_model, install_knowledge_model) = requested_model_installs(&actions)?;

    if !packages.is_empty() {
        dependencies_domain::install_dependencies(packages).await?;
    }
    if install_voice_model {
        crate::features::voice::voice_asr::install_voice_asr_model(app.clone()).await?;
    }
    if install_knowledge_model {
        crate::features::knowledge::model_download::kb_model_download(app, knowledge, pool, None)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_install_actions_are_whitelisted_and_deduplicated() {
        assert_eq!(
            requested_model_installs(&[
                INSTALL_VOICE_MODEL.into(),
                INSTALL_VOICE_MODEL.into(),
                INSTALL_KNOWLEDGE_MODEL.into(),
            ])
            .unwrap(),
            (true, true)
        );
        assert!(requested_model_installs(&["unknown".into()]).is_err());
    }

    #[test]
    fn windows_dependency_check_excludes_runtime_and_includes_repairable_models() {
        let policy = crate::features::dependencies::dependency_check_policy();
        if !policy.include_voice_model || !policy.include_knowledge_model {
            return;
        }
        let items = dependency_items();
        assert!(items.iter().all(|item| item.key != "voice_asr"));
        for key in [INSTALL_VOICE_MODEL, INSTALL_KNOWLEDGE_MODEL] {
            let item = items
                .iter()
                .find(|item| item.key == key)
                .expect("repairable model dependency should be listed");
            assert_eq!(item.install_action.as_deref(), Some(key));
            assert!(item.apt.is_empty());
        }
    }
}

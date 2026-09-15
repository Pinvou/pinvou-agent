use tauri::{AppHandle, State};

use crate::features::assistant::engine_pool::EnginePool;
use crate::features::files::file_ingest::{self as dependency_checks, DependencyCheckItem};
use crate::features::knowledge::{KnowledgeService, model_download};
use crate::features::voice::voice_asr;

const INSTALL_VOICE_MODEL: &str = "voice_asr_model";
const INSTALL_KNOWLEDGE_MODEL: &str = "knowledge_embedding_model";

pub fn check_dependencies() -> Vec<DependencyCheckItem> {
    let mut items = dependency_checks::check_dependencies();
    items.retain(|item| item.key != "voice_asr");
    items.push(DependencyCheckItem {
        key: INSTALL_VOICE_MODEL.into(),
        installed: voice_asr::status().model,
        apt: String::new(),
        install_action: Some(INSTALL_VOICE_MODEL.into()),
        hint: None,
    });
    items.push(DependencyCheckItem {
        key: INSTALL_KNOWLEDGE_MODEL.into(),
        installed: model_download::model_installed(),
        apt: String::new(),
        install_action: Some(INSTALL_KNOWLEDGE_MODEL.into()),
        hint: None,
    });
    items
}

fn requested_model_installs(actions: &[String]) -> Result<(bool, bool), String> {
    let mut voice = false;
    let mut knowledge = false;
    for action in actions {
        match action.as_str() {
            INSTALL_VOICE_MODEL => voice = true,
            INSTALL_KNOWLEDGE_MODEL => knowledge = true,
            _ => return Err("Windows 不支持该依赖安装动作".to_string()),
        }
    }
    Ok((voice, knowledge))
}

type InstallFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>>;

struct InstallTask<'a> {
    failure_name: &'static str,
    future: InstallFuture<'a>,
}

async fn run_install_tasks(tasks: Vec<InstallTask<'_>>) -> Result<(), String> {
    let mut failures = Vec::new();
    for task in tasks {
        if let Err(error) = task.future.await {
            failures.push(format!("{}: {error}", task.failure_name));
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("依赖安装未完成: {}", failures.join("、")))
    }
}

pub async fn install_dependencies(
    packages: Vec<String>,
    actions: Vec<String>,
    app: AppHandle,
    knowledge: State<'_, KnowledgeService>,
    pool: State<'_, EnginePool>,
) -> Result<(), String> {
    let (install_voice_model, install_knowledge_model) = requested_model_installs(&actions)?;
    let mut tasks = Vec::new();

    if !packages.is_empty() {
        tasks.push(InstallTask {
            failure_name: "系统依赖",
            future: Box::pin(dependency_checks::install_dependencies(
                app.clone(),
                packages,
            )),
        });
    }
    if install_voice_model {
        let voice_app = app.clone();
        tasks.push(InstallTask {
            failure_name: "本地语音识别模型",
            future: Box::pin(async move {
                voice_asr::install_voice_asr_model(voice_app)
                    .await
                    .map(|_| ())
            }),
        });
    }
    if install_knowledge_model {
        tasks.push(InstallTask {
            failure_name: "知识库向量模型",
            future: Box::pin(async {
                model_download::kb_model_download(app, knowledge, pool, None)
                    .await
                    .map(|_| ())
            }),
        });
    }

    run_install_tasks(tasks).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_check_replaces_runtime_with_repairable_models() {
        let items = check_dependencies();
        assert!(items.iter().all(|item| item.key != "voice_asr"));
        for key in [INSTALL_VOICE_MODEL, INSTALL_KNOWLEDGE_MODEL] {
            let item = items
                .iter()
                .find(|item| item.key == key)
                .expect("Windows repairable model dependency should be listed");
            assert_eq!(item.install_action.as_deref(), Some(key));
            assert!(item.apt.is_empty());
        }
    }

    #[test]
    fn windows_model_actions_are_whitelisted_and_deduplicated() {
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

    #[tokio::test]
    async fn install_tasks_continue_after_failure_and_keep_details() {
        let attempts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut tasks = Vec::new();
        for (name, result) in [
            ("系统依赖", Err("winget 启动失败".to_string())),
            ("本地语音识别模型", Ok(())),
            ("知识库向量模型", Err("模型校验失败".to_string())),
        ] {
            let attempts = attempts.clone();
            tasks.push(InstallTask {
                failure_name: name,
                future: Box::pin(async move {
                    attempts.lock().unwrap().push(name);
                    result
                }),
            });
        }

        assert_eq!(
            run_install_tasks(tasks).await,
            Err(
                "依赖安装未完成: 系统依赖: winget 启动失败、知识库向量模型: 模型校验失败"
                    .to_string()
            )
        );
        assert_eq!(
            *attempts.lock().unwrap(),
            vec!["系统依赖", "本地语音识别模型", "知识库向量模型"]
        );
    }
}

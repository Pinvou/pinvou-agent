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

fn requested_model_installs(
    actions: &[String],
    policy: crate::features::dependencies::DependencyCheckPolicy,
) -> Result<(bool, bool), String> {
    let mut voice = false;
    let mut knowledge = false;
    for action in actions {
        match action.as_str() {
            INSTALL_VOICE_MODEL if policy.include_voice_model => voice = true,
            INSTALL_VOICE_MODEL => {
                return Err("当前平台不支持安装本地语音识别模型".to_string());
            }
            INSTALL_KNOWLEDGE_MODEL if policy.include_knowledge_model => knowledge = true,
            INSTALL_KNOWLEDGE_MODEL => {
                return Err("当前平台不支持安装知识库向量模型".to_string());
            }
            _ => return Err("不支持的依赖安装动作".to_string()),
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

#[tauri::command]
pub async fn install_dependencies(
    packages: Vec<String>,
    actions: Option<Vec<String>>,
    app: AppHandle,
    knowledge: State<'_, KnowledgeService>,
    pool: State<'_, EnginePool>,
) -> Result<(), String> {
    let actions = actions.unwrap_or_default();
    let policy = crate::features::dependencies::dependency_check_policy();
    let (install_voice_model, install_knowledge_model) =
        requested_model_installs(&actions, policy)?;

    let mut tasks = Vec::new();
    if !packages.is_empty() {
        tasks.push(InstallTask {
            failure_name: "系统依赖",
            future: Box::pin(dependencies_domain::install_dependencies(packages)),
        });
    }
    if install_voice_model {
        let voice_app = app.clone();
        tasks.push(InstallTask {
            failure_name: "本地语音识别模型",
            future: Box::pin(async move {
                crate::features::voice::voice_asr::install_voice_asr_model(voice_app)
                    .await
                    .map(|_| ())
            }),
        });
    }
    if install_knowledge_model {
        tasks.push(InstallTask {
            failure_name: "知识库向量模型",
            future: Box::pin(async {
                crate::features::knowledge::model_download::kb_model_download(
                    app, knowledge, pool, None,
                )
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
    fn model_install_actions_are_whitelisted_and_deduplicated() {
        let policy = crate::features::dependencies::DependencyCheckPolicy {
            include_voice_runtime: false,
            include_voice_model: true,
            include_knowledge_model: true,
        };
        assert_eq!(
            requested_model_installs(
                &[
                    INSTALL_VOICE_MODEL.into(),
                    INSTALL_VOICE_MODEL.into(),
                    INSTALL_KNOWLEDGE_MODEL.into(),
                ],
                policy
            )
            .unwrap(),
            (true, true)
        );
        assert!(requested_model_installs(&["unknown".into()], policy).is_err());
    }

    #[test]
    fn disabled_model_install_actions_are_rejected() {
        let policy = crate::features::dependencies::DependencyCheckPolicy {
            include_voice_runtime: true,
            include_voice_model: false,
            include_knowledge_model: false,
        };
        assert_eq!(
            requested_model_installs(&[INSTALL_VOICE_MODEL.into()], policy),
            Err("当前平台不支持安装本地语音识别模型".to_string())
        );
        assert_eq!(
            requested_model_installs(&[INSTALL_KNOWLEDGE_MODEL.into()], policy),
            Err("当前平台不支持安装知识库向量模型".to_string())
        );
    }

    #[tokio::test]
    async fn install_tasks_continue_after_failure_and_aggregate_errors() {
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

    #[tokio::test]
    async fn successful_install_tasks_return_no_error() {
        let tasks = vec![InstallTask {
            failure_name: "系统依赖",
            future: Box::pin(async { Ok(()) }),
        }];
        assert_eq!(run_install_tasks(tasks).await, Ok(()));
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

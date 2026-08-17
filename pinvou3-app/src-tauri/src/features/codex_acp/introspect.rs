//! Kimi Code 运行时内省：解析 config.toml / credentials / kimi-code.log
//! 与会话日志失败标记。本波留在 codex_acp 簇内，Wave 3 才迁出边界。

use super::*;

#[derive(Debug)]
pub(super) struct KimiDiagnosticCursor {
    session_id: String,
    log_path: Option<PathBuf>,
    offset: u64,
}
pub(super) fn kimi_authenticated(_kimi: &Path) -> bool {
    // Kimi Code 0.31+ 不读取裸 KIMI_API_KEY；只有成对的 KIMI_MODEL_* 覆盖
    // 会在内存中合成 provider/model。
    if nonempty_env("KIMI_MODEL_NAME") && nonempty_env("KIMI_MODEL_API_KEY") {
        return true;
    }
    let root = kimi_data_root();
    let oauth_credentials_valid =
        std::fs::read_to_string(root.join("credentials").join("kimi-code.json"))
            .is_ok_and(|raw| kimi_credentials_valid(&raw));
    let Ok(config) = std::fs::read_to_string(root.join("config.toml")) else {
        return false;
    };
    kimi_runtime_config_ready(&config, oauth_credentials_valid)
}
/// 仅有 OAuth 凭证并不代表 Kimi 已可用：官方登录还必须把 `/models` 返回结果
/// 写为默认模型及其 provider。要求默认模型能解析到现有 provider，避免登录后半段
/// 失败时把“凭证已写入”误报成“已登录”。
pub(super) fn kimi_runtime_config_ready(raw: &str, oauth_credentials_valid: bool) -> bool {
    let Ok(config) = toml::from_str::<toml::Value>(raw) else {
        return false;
    };
    let Some(default_model) = config
        .get("default_model")
        .and_then(toml::Value::as_str)
        .filter(|value| !value.trim().is_empty())
    else {
        return false;
    };
    let Some(model) = config
        .get("models")
        .and_then(|models| models.get(default_model))
        .and_then(toml::Value::as_table)
    else {
        return false;
    };
    let Some(provider) = model
        .get("provider")
        .and_then(toml::Value::as_str)
        .filter(|value| !value.trim().is_empty())
    else {
        return false;
    };
    let model_ready = model
        .get("model")
        .and_then(toml::Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
        && model
            .get("max_context_size")
            .and_then(toml::Value::as_integer)
            .is_some_and(|value| value > 0);
    if !model_ready {
        return false;
    }
    let Some(provider) = config
        .get("providers")
        .and_then(|providers| providers.get(provider))
        .and_then(toml::Value::as_table)
    else {
        return false;
    };
    if !provider
        .get("type")
        .and_then(toml::Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
    {
        return false;
    }
    let direct_api_key = provider
        .get("api_key")
        .and_then(toml::Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    let configured_env_api_key = provider
        .get("env")
        .and_then(toml::Value::as_table)
        .is_some_and(|env| {
            env.iter().any(|(name, value)| {
                name.ends_with("_API_KEY")
                    && value.as_str().is_some_and(|value| !value.trim().is_empty())
            })
        });
    let oauth_ready =
        provider.get("oauth").is_some_and(toml::Value::is_table) && oauth_credentials_valid;
    direct_api_key || configured_env_api_key || oauth_ready
}
pub(super) fn kimi_data_root() -> PathBuf {
    std::env::var_os("KIMI_CODE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::platform::os::user_home_dir().join(".kimi-code"))
}
pub(super) async fn kimi_diagnostic_cursor(session_id: &str) -> KimiDiagnosticCursor {
    let log_path = resolve_kimi_session_log_path(session_id).await;
    let offset = match log_path.as_ref() {
        Some(path) => tokio::fs::metadata(path)
            .await
            .map(|metadata| metadata.len())
            .unwrap_or(0),
        None => 0,
    };
    KimiDiagnosticCursor {
        session_id: session_id.to_string(),
        log_path,
        offset,
    }
}
pub(super) async fn kimi_failure_after(cursor: &KimiDiagnosticCursor) -> Option<String> {
    // Kimi 在返回 ACP end_turn 前先写日志，但文件 sink 可能有极短刷新延迟。
    for delay_ms in [0, 25, 75] {
        if delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
        let log_path = match cursor.log_path.clone() {
            Some(path) => path,
            None => match resolve_kimi_session_log_path(&cursor.session_id).await {
                Some(path) => path,
                None => continue,
            },
        };
        // 按 offset seek 只增量读取本回合新增内容，不再每回合整读日志文件。
        let offset = if cursor.log_path.as_ref() == Some(&log_path) {
            cursor.offset
        } else {
            0
        };
        let Ok(mut file) = tokio::fs::File::open(&log_path).await else {
            continue;
        };
        if file.seek(std::io::SeekFrom::Start(offset)).await.is_err() {
            continue;
        }
        let mut raw = Vec::new();
        if file.read_to_end(&mut raw).await.is_err() {
            continue;
        }
        if let Some(error) = parse_kimi_acp_failure(&String::from_utf8_lossy(&raw)) {
            return Some(error);
        }
    }
    None
}
pub(super) async fn resolve_kimi_session_log_path(session_id: &str) -> Option<PathBuf> {
    let root = kimi_data_root();
    let raw = tokio::fs::read_to_string(root.join("session_index.jsonl"))
        .await
        .ok()?;
    kimi_session_log_path_from_index(&raw, &root, session_id)
}
pub(super) fn kimi_session_log_path_from_index(
    index: &str,
    data_root: &Path,
    session_id: &str,
) -> Option<PathBuf> {
    let sessions_root = data_root.join("sessions");
    index.lines().rev().find_map(|line| {
        let record = serde_json::from_str::<serde_json::Value>(line).ok()?;
        (record.get("sessionId")?.as_str()? == session_id).then_some(())?;
        let raw_dir = PathBuf::from(record.get("sessionDir")?.as_str()?);
        let session_dir = if raw_dir.is_absolute() {
            raw_dir
        } else {
            data_root.join(raw_dir)
        };
        if session_dir
            .components()
            .any(|component| component == std::path::Component::ParentDir)
            || !session_dir.starts_with(&sessions_root)
        {
            return None;
        }
        Some(session_dir.join("logs").join("kimi-code.log"))
    })
}
pub(super) fn parse_kimi_acp_failure(log_tail: &str) -> Option<String> {
    const MARKER: &str = "acp: turn ended with failed reason";
    log_tail.lines().rev().find_map(|line| {
        let (_, details) = line.split_once(MARKER)?;
        let (_, raw_error) = details.split_once("error=")?;
        let raw_error = raw_error.trim();
        let decoded = if raw_error.starts_with('"') {
            serde_json::from_str::<String>(raw_error).ok()?
        } else {
            raw_error.to_string()
        };
        let error = serde_json::from_str::<serde_json::Value>(&decoded).ok()?;
        let code = error
            .get("code")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("provider.error");
        let message = error
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Kimi Code 模型请求失败");
        Some(format_kimi_provider_error(code, message))
    })
}
pub(super) fn format_kimi_provider_error(code: &str, message: &str) -> String {
    let normalized = message.to_ascii_lowercase();
    if normalized.contains("402") && normalized.contains("membership benefits") {
        return "Kimi Code 会员权益校验失败（HTTP 402）：请确认当前登录账号已开通且会员仍有效"
            .to_string();
    }
    if code.eq_ignore_ascii_case("model.not_configured")
        || normalized.contains("llm not set")
        || normalized.contains("send \"/login\"")
    {
        return "Kimi Code 尚未完成模型配置（model.not_configured），请重新登录".to_string();
    }
    if code.contains("auth") || normalized.contains("authentication") || normalized.contains("401")
    {
        return "Kimi Code 登录已失效（HTTP 401），请重新登录".to_string();
    }
    if normalized.contains("429")
        || normalized.contains("rate limit")
        || normalized.contains("quota")
    {
        return "Kimi Code 请求受限或额度不足，请稍后重试或检查账号额度".to_string();
    }
    let message = message
        .chars()
        .filter(|character| !character.is_control())
        .take(1000)
        .collect::<String>();
    format!("Kimi Code 请求失败（{code}）：{message}")
}
pub(super) fn kimi_credentials_valid(raw: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return false;
    };
    let token_present = ["access_token", "refresh_token"].into_iter().all(|key| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|token| !token.trim().is_empty())
    });
    // Kimi 的 access_token 约 15 分钟即过期，Kimi CLI 运行时会用 refresh_token 自动续期，
    // 因此 expires_at（Unix 秒）过期不判未认证，否则登录 15 分钟后状态就会误报。
    // 这里仅要求 expires_at 是合法的正数时间戳，用于识别损坏的凭证文件。
    let expiry_valid = value
        .get("expires_at")
        .and_then(serde_json::Value::as_i64)
        .is_some_and(|expiry| expiry > 0);
    token_present && expiry_valid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    pub(super) fn kimi_credentials_require_tokens_and_nonzero_expiry() {
        assert!(kimi_credentials_valid(
            r#"{"access_token":"access","refresh_token":"refresh","expires_at":1}"#
        ));
        // access_token 过期但 refresh_token 仍在时不判未认证（Kimi CLI 会自动续期）。
        assert!(kimi_credentials_valid(
            r#"{"access_token":"access","refresh_token":"refresh","expires_at":1700000000}"#
        ));
        assert!(!kimi_credentials_valid(
            r#"{"access_token":"access","refresh_token":"refresh","expires_at":0}"#
        ));
        assert!(!kimi_credentials_valid(
            r#"{"access_token":"","refresh_token":"refresh","expires_at":1}"#
        ));
        assert!(!kimi_credentials_valid("not-json"));
    }

    #[test]
    pub(super) fn kimi_runtime_config_requires_resolvable_default_model() {
        let ready = r#"
default_model = "kimi-code/kimi-k2"

[providers."managed:kimi-code"]
type = "kimi"

[providers."managed:kimi-code".oauth]
storage = "file"
key = "oauth/kimi-code"

[models."kimi-code/kimi-k2"]
provider = "managed:kimi-code"
model = "kimi-k2"
max_context_size = 262144
"#;
        assert!(kimi_runtime_config_ready(ready, true));
        assert!(!kimi_runtime_config_ready(ready, false));
        let api_key_ready = r#"
default_model = "custom/kimi-k2"

[providers.custom]
type = "kimi"
api_key = "configured-in-file"

[models."custom/kimi-k2"]
provider = "custom"
model = "kimi-k2"
max_context_size = 262144
"#;
        assert!(kimi_runtime_config_ready(api_key_ready, false));
        assert!(!kimi_runtime_config_ready(
            "# Login will populate managed Kimi provider and model entries.",
            true
        ));
        assert!(!kimi_runtime_config_ready(
            "default_model = \"kimi-code/missing\"\n[providers.\"managed:kimi-code\"]\ntype = \"kimi\"",
            true
        ));
        assert!(!kimi_runtime_config_ready("not = [valid", true));
    }

    #[test]
    pub(super) fn parses_kimi_provider_failure_from_session_log() {
        let log = concat!(
            "2026-07-27T08:18:51Z INFO llm request\n",
            "2026-07-27T08:18:51Z WARN acp: turn ended with failed reason  ",
            "error=\"{\\\"code\\\":\\\"provider.api_error\\\",",
            "\\\"message\\\":\\\"402 We're unable to verify your membership benefits at this time.\\\"}\"\n",
        );
        assert_eq!(
            parse_kimi_acp_failure(log).as_deref(),
            Some("Kimi Code 会员权益校验失败（HTTP 402）：请确认当前登录账号已开通且会员仍有效")
        );
        assert!(parse_kimi_acp_failure("INFO turn completed").is_none());
    }

    #[test]
    pub(super) fn maps_kimi_auth_and_quota_failures_to_actionable_messages() {
        assert_eq!(
            format_kimi_provider_error(
                "model.not_configured",
                "LLM not set, send \"/login\" to login"
            ),
            "Kimi Code 尚未完成模型配置（model.not_configured），请重新登录"
        );
        assert_eq!(
            format_kimi_provider_error("provider.auth_failed", "401 unauthorized"),
            "Kimi Code 登录已失效（HTTP 401），请重新登录"
        );
        assert_eq!(
            format_kimi_provider_error("provider.api_error", "429 quota exceeded"),
            "Kimi Code 请求受限或额度不足，请稍后重试或检查账号额度"
        );
    }

    #[test]
    pub(super) fn resolves_only_kimi_session_logs_under_the_data_root() {
        let root = Path::new("/tmp/kimi-home");
        let index = concat!(
            "{\"sessionId\":\"session-safe\",\"sessionDir\":\"/tmp/kimi-home/sessions/wd_project/session-safe\"}\n",
            "{\"sessionId\":\"session-escape\",\"sessionDir\":\"/tmp/kimi-home/sessions/../credentials\"}\n",
        );
        assert_eq!(
            kimi_session_log_path_from_index(index, root, "session-safe"),
            Some(PathBuf::from(
                "/tmp/kimi-home/sessions/wd_project/session-safe/logs/kimi-code.log"
            ))
        );
        assert_eq!(
            kimi_session_log_path_from_index(index, root, "session-escape"),
            None
        );
    }

    #[test]
    pub(super) fn detects_server_request_for_newer_codex_runtime() {
        assert!(codex_upgrade_required(
            "The 'gpt-5.6-sol' model requires a newer version of Codex."
        ));
        assert!(!codex_upgrade_required("Codex ACP connection closed"));
    }
}

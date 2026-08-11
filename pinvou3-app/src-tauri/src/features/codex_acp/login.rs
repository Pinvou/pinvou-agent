//! CLI 登录 / 授权流程：解析登录 URL、设备码与 OAuth 凭证有效性探测。

use super::*;

/// readiness 报错面向用户展示中文，把结构化 setup_hint 代码映射回中文文案。
pub(super) fn setup_hint_message(hint: Option<&str>) -> &'static str {
    match hint {
        Some("kimi_cli_missing") => "请先安装 Kimi Code CLI",
        Some("kimi_auth_required") => "使用 Kimi 账号完成设备码授权",
        Some("claude_auth_required") => "使用 Claude 账号完成浏览器授权，或设置 ANTHROPIC_API_KEY",
        _ => "请检查 Agent 安装和 PATH",
    }
}
pub(super) fn agent_login_command(backend: AgentBackend, executable: &Path) -> Command {
    if backend == AgentBackend::CodexAcp {
        return platform::codex_login_command(executable);
    }
    let args: &[&str] = match backend {
        AgentBackend::CodexAcp => unreachable!(),
        AgentBackend::ClaudeAcp => &["auth", "login"],
        AgentBackend::KimiAcp => &["login"],
        AgentBackend::Deepseek => &[],
    };
    let mut command = crate::platform::process::external_tokio_command(executable);
    command.args(args);
    command
}
pub(super) async fn capture_agent_login_output<R>(
    mut reader: R,
    backend: AgentBackend,
    states: Arc<parking_lot::RwLock<HashMap<AgentBackend, AgentLoginState>>>,
) -> String
where
    R: AsyncRead + Unpin,
{
    let mut chunk = [0_u8; 2048];
    let mut output = String::new();
    loop {
        let read = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        output.push_str(&String::from_utf8_lossy(&chunk[..read]));
        if output.len() > 65_536 {
            output.drain(..output.len() - 65_536);
        }
        let url = extract_agent_login_url(backend, &output);
        let code = extract_device_code(&output, url.as_deref());
        if url.is_some() || code.is_some() {
            let mut states = states.write();
            let state = states.entry(backend).or_default();
            if url.is_some() {
                state.url = url;
            }
            if code.is_some() {
                state.code = code;
            }
        }
    }
    output
}
/// Kimi 的登录流程会先落 OAuth 凭证，再请求模型列表并写入 config.toml。
/// 后半段失败时 CLI 会以 `Login failed:` 输出可操作原因；只提取该明确前缀，
/// 同时隐藏 URL 并拒绝可能包含凭证的内容，避免把设备码或令牌写入诊断日志/UI。
pub(super) fn kimi_login_failure_detail(stdout: &str, stderr: &str) -> Option<String> {
    [stderr, stdout].into_iter().find_map(|output| {
        output.lines().rev().find_map(|line| {
            let (_, detail) = line.rsplit_once("Login failed:")?;
            sanitize_kimi_login_failure_detail(detail)
        })
    })
}
pub(super) fn sanitize_kimi_login_failure_detail(detail: &str) -> Option<String> {
    let normalized = detail.to_ascii_lowercase();
    if [
        "access_token",
        "refresh_token",
        "authorization:",
        "bearer ",
        "api_key",
        "api-key",
        "secret=",
        "cookie:",
        "user_code=",
        "device_code=",
    ]
    .into_iter()
    .any(|marker| normalized.contains(marker))
    {
        return None;
    }

    let mut sanitized = String::new();
    for word in detail.split_whitespace() {
        if !sanitized.is_empty() {
            sanitized.push(' ');
        }
        if word.contains("https://") || word.contains("http://") || looks_like_device_code(word) {
            sanitized.push_str("[敏感信息已隐藏]");
        } else {
            sanitized.extend(word.chars().filter(|character| !character.is_control()));
        }
        if sanitized.chars().count() >= 500 {
            break;
        }
    }
    let sanitized = sanitized.chars().take(500).collect::<String>();
    (!sanitized.is_empty()).then_some(sanitized)
}
pub(super) fn looks_like_device_code(word: &str) -> bool {
    let candidate =
        word.trim_matches(|character: char| !character.is_ascii_alphanumeric() && character != '-');
    let Some((left, right)) = candidate.split_once('-') else {
        return false;
    };
    [left, right].into_iter().all(|part| {
        part.len() == 4
            && part
                .chars()
                .all(|character| character.is_ascii_uppercase() || character.is_ascii_digit())
    })
}
pub(super) fn extract_agent_login_url(backend: AgentBackend, output: &str) -> Option<String> {
    output
        .match_indices("https://")
        .filter_map(|(start, _)| {
            let tail = &output[start..];
            let end = tail
                .char_indices()
                .find_map(|(index, character)| {
                    (character.is_whitespace()
                        || character.is_control()
                        || matches!(character, '"' | '\'' | '<' | '>'))
                    .then_some(index)
                })
                .unwrap_or(tail.len());
            let candidate = tail[..end].trim_end_matches(['.', ',', ')', ']']);
            agent_login_url_allowed(backend, candidate).then(|| candidate.to_string())
        })
        .last()
}
pub(super) fn agent_login_url_allowed(backend: AgentBackend, url: &str) -> bool {
    match backend {
        AgentBackend::CodexAcp => {
            url.starts_with("https://auth.openai.com/")
                || url.starts_with("https://platform.openai.com/")
        }
        AgentBackend::ClaudeAcp => {
            url.starts_with("https://claude.com/")
                || url.starts_with("https://claude.ai/")
                || url.starts_with("https://platform.claude.com/")
        }
        AgentBackend::KimiAcp => {
            url.starts_with("https://www.kimi.com/") || url.starts_with("https://kimi.com/")
        }
        AgentBackend::Deepseek => false,
    }
}
pub(super) fn extract_device_code(output: &str, login_url: Option<&str>) -> Option<String> {
    if let Some(url) = login_url {
        if let Some(value) = url.split("user_code=").nth(1) {
            let code = value
                .split(|character: char| character == '&' || character.is_whitespace())
                .next()
                .unwrap_or_default();
            if valid_device_code(code) {
                return Some(code.to_string());
            }
        }
    }
    ["enter code:", "user code:"]
        .into_iter()
        .find_map(|marker| {
            let start = output.to_ascii_lowercase().rfind(marker)? + marker.len();
            let code = output[start..].split_whitespace().next()?;
            valid_device_code(code).then(|| code.to_string())
        })
}
pub(super) fn valid_device_code(code: &str) -> bool {
    (4..=32).contains(&code.len())
        && code
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}
pub(super) fn codex_authenticated(codex: &Path) -> bool {
    if nonempty_env("OPENAI_API_KEY") {
        return true;
    }
    // 第三方 Provider（中转）激活时，注入的 key 只存在于被 spawn 的 Codex 子进程
    // env 中，探测进程看不到；config.toml 有指向存在的表且 env_key 非空的
    // model_provider 即视为已认证，避免在 relay 场景误报需要登录。
    if let Ok(raw) = std::fs::read_to_string(
        crate::platform::os::user_home_dir()
            .join(".codex")
            .join("config.toml"),
    ) {
        if providers::codex_config_relay_env_key_present(&raw) {
            return true;
        }
    }
    cli_status_success(codex, &["login", "status"])
}
pub(super) fn claude_authenticated(claude: &Path) -> bool {
    if [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
    ]
    .into_iter()
    .any(nonempty_env)
    {
        return true;
    }
    cli_status_success(claude, &["auth", "status"])
}
pub(super) fn nonempty_env(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}
pub(super) fn cli_status_success(executable: &Path, args: &[&str]) -> bool {
    let mut command = crate::platform::process::external_command(executable);
    command.args(args);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    // 15s：Node 版 CLI（npm 安装的 codex）冷启动实测 ~9s，3s 会误判
    match child.wait_timeout(Duration::from_secs(15)) {
        Ok(Some(status)) => status.success(),
        Ok(None) | Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    pub(super) fn extracts_only_allowed_agent_authorization_urls() {
        assert_eq!(
            extract_agent_login_url(
                AgentBackend::CodexAcp,
                "https://auth.openai.com/oauth/authorize?response_type=code&state=test",
            ),
            Some(
                "https://auth.openai.com/oauth/authorize?response_type=code&state=test".to_string()
            )
        );
        assert_eq!(
            extract_agent_login_url(
                AgentBackend::ClaudeAcp,
                "If the browser did not open, visit: \u{1b}]8;;https://claude.com/cai/oauth/authorize?state=test\u{7}https://claude.com/cai/oauth/authorize?state=test\u{1b}]8;;\u{7}",
            ),
            Some("https://claude.com/cai/oauth/authorize?state=test".to_string())
        );
        assert_eq!(
            extract_agent_login_url(
                AgentBackend::ClaudeAcp,
                "Legacy Claude login: https://claude.ai/oauth/authorize?state=test",
            ),
            Some("https://claude.ai/oauth/authorize?state=test".to_string())
        );
        assert_eq!(
            extract_agent_login_url(
                AgentBackend::KimiAcp,
                "Opening https://www.kimi.com/code/authorize_device?user_code=ABCD-1234",
            ),
            Some("https://www.kimi.com/code/authorize_device?user_code=ABCD-1234".to_string())
        );
        assert_eq!(
            extract_agent_login_url(AgentBackend::ClaudeAcp, "https://example.com/not-claude",),
            None
        );
    }

    #[test]
    pub(super) fn extracts_kimi_device_code_without_accepting_arbitrary_text() {
        let url = "https://www.kimi.com/code/authorize_device?user_code=MO3M-6JFK";
        assert_eq!(
            extract_device_code("Opening browser", Some(url)),
            Some("MO3M-6JFK".to_string())
        );
        assert_eq!(extract_device_code("enter code: <script>", None), None);
    }

    #[test]
    pub(super) fn captures_safe_kimi_login_failure_without_urls_or_credentials() {
        assert_eq!(
            kimi_login_failure_detail(
                "",
                "Login failed: Kimi Code models endpoint https://api.kimi.com/coding/v1 rejected OAuth credentials: HTTP 402 membership required\n",
            )
            .as_deref(),
            Some("Kimi Code models endpoint [敏感信息已隐藏] rejected OAuth credentials: HTTP 402 membership required")
        );
        assert_eq!(
            kimi_login_failure_detail("", "Login failed: access_token=private-value\n"),
            None
        );
        assert_eq!(
            kimi_login_failure_detail("device code: ABCD-EFGH", ""),
            None
        );
        assert_eq!(
            sanitize_kimi_login_failure_detail("device code ABCD-EFGH expired").as_deref(),
            Some("device code [敏感信息已隐藏] expired")
        );
    }
}

use std::time::Duration;

/// Total number of apt install attempts: the initial try plus two retries.
pub(super) const MAX_ATTEMPTS: usize = 3;

/// Pause between attempts: long enough for a flaky mirror or link to settle,
/// short enough not to visibly stall the dependency setup flow.
pub(super) const RETRY_DELAY: Duration = Duration::from_secs(2);

/// Builds the `pkexec sh -c` script for one attempt. The first try installs
/// only; retry attempts prepend an index refresh to the same script, so each
/// attempt costs a single polkit prompt (pkexec's `auth_admin` has no
/// retention — a separate pkexec invocation would re-prompt). The refresh is
/// sequenced with `;` rather than `&&`, keeping it best-effort: a failed
/// refresh never blocks the attempt, and the script exits with the install's
/// own exit code.
pub(super) fn install_script(packages: &[String], with_refresh: bool) -> String {
    let refresh = if with_refresh {
        "DEBIAN_FRONTEND=noninteractive apt-get update; "
    } else {
        ""
    };
    format!(
        "{refresh}DEBIAN_FRONTEND=noninteractive apt-get install -y {}",
        packages.join(" ")
    )
}

/// pkexec/apt exit codes that never resolve by retrying: the user dismissed
/// the polkit prompt (126) or pkexec could not authorize/run the command
/// (127: not authorized, no authentication agent, or an execution error).
/// Any other outcome (including install failures and signals with no exit
/// code) is treated as transient and worth one more attempt.
fn failure_retryable(exit_code: Option<i32>) -> bool {
    !matches!(exit_code, Some(126) | Some(127))
}

/// Whether the 1-based `attempt` that exited with `exit_code` should be
/// followed by another attempt.
pub(super) fn should_retry(attempt: usize, exit_code: Option<i32>) -> bool {
    attempt < MAX_ATTEMPTS && failure_retryable(exit_code)
}

/// User-facing error for a failed attempt. Messages are kept verbatim from
/// the original single-shot implementation, so permanent failures surface
/// exactly as before.
pub(super) fn failure_message(exit_code: Option<i32>, stderr: &str) -> String {
    let code = exit_code.unwrap_or(-1);
    match code {
        126 => "用户取消授权".to_string(),
        127 => "未授权或 pkexec 不可用".to_string(),
        _ => {
            let tail: Vec<&str> = stderr.lines().rev().take(4).collect();
            let tail: Vec<&str> = tail.into_iter().rev().collect();
            format!("安装失败 (exit {code}): {}", tail.join(" / "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Auth-cancelled (126) and not-authorized/unusable pkexec (127) are
    // permanent: retrying cannot fix them, and each retry would re-prompt
    // the user for nothing.
    #[test]
    fn permanent_pkexec_failures_are_never_retried() {
        assert!(!failure_retryable(Some(126)));
        assert!(!failure_retryable(Some(127)));
        assert!(!should_retry(1, Some(126)));
        assert!(!should_retry(1, Some(127)));
    }

    // Ordinary install failures (apt uses exit 100) and signal kills (no exit
    // code) look transient: they retry up to the two-extra-attempts limit.
    #[test]
    fn transient_failures_retry_up_to_the_limit() {
        assert!(should_retry(1, Some(100)));
        assert!(should_retry(2, Some(100)));
        assert!(should_retry(1, None));
        // The third attempt is the last one: no further retries.
        assert!(!should_retry(3, Some(100)));
        assert!(!should_retry(MAX_ATTEMPTS, None));
    }

    // Keep the error surfaced on permanent or final failures unchanged from
    // the single-shot version, including the last-4-stderr-lines tail.
    #[test]
    fn failure_messages_match_the_single_shot_version() {
        assert_eq!(failure_message(Some(126), "ignored"), "用户取消授权");
        assert_eq!(
            failure_message(Some(127), "ignored"),
            "未授权或 pkexec 不可用"
        );
        let err = failure_message(Some(100), "l1\nl2\nl3\nl4\nl5\nl6");
        assert_eq!(err, "安装失败 (exit 100): l3 / l4 / l5 / l6");
        // Signal kill has no exit code today; it is reported as exit -1.
        assert!(failure_message(None, "").starts_with("安装失败 (exit -1)"));
    }

    // The first attempt installs only; retry attempts prepend the index
    // refresh to the same authorized script (single polkit prompt per
    // attempt), sequenced with `;` so the install's exit code wins.
    #[test]
    fn install_script_targets_apt_get_with_optional_refresh() {
        let script = install_script(&["ffmpeg".to_string(), "pandoc".to_string()], false);
        assert_eq!(
            script,
            "DEBIAN_FRONTEND=noninteractive apt-get install -y ffmpeg pandoc"
        );
        assert_eq!(
            install_script(&["ffmpeg".to_string()], true),
            "DEBIAN_FRONTEND=noninteractive apt-get update; \
             DEBIAN_FRONTEND=noninteractive apt-get install -y ffmpeg"
        );
    }

    // The pause between attempts is part of the observable behavior; pinning
    // it here also keeps the constant warning-free in non-Linux test builds,
    // where the Linux-gated loop that consumes it is not compiled.
    #[test]
    fn retry_delay_is_pinned() {
        assert_eq!(RETRY_DELAY, Duration::from_secs(2));
    }
}

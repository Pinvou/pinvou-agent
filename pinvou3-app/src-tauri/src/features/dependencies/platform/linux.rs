use std::process::Command;
use std::thread;

use super::linux_packages::validate_packages;
use super::linux_retry::{
    MAX_ATTEMPTS, RETRY_DELAY, failure_message, install_script, should_retry,
};

/// Installs the batch on Linux via `pkexec apt-get`. The `progress` callback
/// signature is documented on the macOS side: `(package, current, total,
/// detail)`. Progress stays coarse (no per-line apt output to stream): one
/// event before the first attempt and one detail event between retries.
///
/// apt installs can fail transiently (mirror sync lag, flaky network), so
/// retryable failures are retried a bounded number of times (`MAX_ATTEMPTS` =
/// initial try + two retries). Each attempt is a single pkexec authorization:
/// retry attempts refresh the apt index inside the same authorized script,
/// because a stale index is the most common transient cause and pkexec's
/// `auth_admin` grant has no retention — a separate pkexec invocation would
/// re-prompt for the password. Permanent failures (auth cancelled, pkexec
/// unusable) are never retried, and every failure surfaces the same message
/// the single-shot version used.
pub fn install_dependencies(
    packages: Vec<String>,
    progress: Option<&(dyn Fn(&str, usize, usize, Option<&str>) + Sync)>,
) -> Result<(), String> {
    validate_packages(&packages)?;
    // Batch progress stays coarse (1/1): no per-line apt output to stream.
    // The "apt" fallback is unreachable: validate_packages rejects empty.
    let package_label = packages
        .first()
        .cloned()
        .unwrap_or_else(|| "apt".to_string());
    if let Some(report) = progress {
        report(&package_label, 1, 1, None);
    }
    let mut attempt = 1usize;
    loop {
        // The first try installs only; retries prepend the index refresh to
        // the same authorized script, so each attempt costs one polkit prompt.
        let script = install_script(&packages, attempt > 1);
        let output = Command::new("pkexec")
            .args(["sh", "-c", &script])
            .output()
            // A spawn failure means pkexec itself is broken or missing;
            // running the same command again cannot fix that, so surface it
            // directly (unchanged from the single-shot version).
            .map_err(|e| format!("pkexec 启动失败: {e}"))?;

        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let exit_code = output.status.code();
        let error = failure_message(exit_code, &stderr);
        if !should_retry(attempt, exit_code) {
            return Err(error);
        }
        // Transient-looking failure: tell the UI we are retrying, pause
        // briefly, then run the next attempt (its script refreshes the index
        // first; a stale mirror is the usual cause).
        if let Some(report) = progress {
            report(
                &package_label,
                1,
                1,
                Some(&format!("第 {attempt}/{MAX_ATTEMPTS} 次安装失败，正在重试")),
            );
        }
        thread::sleep(RETRY_DELAY);
        attempt += 1;
    }
}

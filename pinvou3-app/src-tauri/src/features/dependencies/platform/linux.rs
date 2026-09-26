use std::process::Command;
use std::thread;

use super::linux_packages::validate_packages;
use super::linux_retry::{
    MAX_ATTEMPTS, RETRY_DELAY, failure_message, install_script, should_retry, update_script,
};

/// `progress` 回调签名见 macOS 侧文档 `(package, current, total, detail)`。
/// Linux 用 pkexec apt 一次性安装整批,只在执行前发一次粗粒度进度(无逐行
/// 输出可流式),保持既有行为不变。
///
/// apt installs can fail transiently (mirror sync lag, flaky network), so
/// retryable failures are retried a bounded number of times (`MAX_ATTEMPTS` =
/// initial try + two retries). Before each retry the apt index is refreshed
/// and we pause briefly; a stale index is the most common transient cause and
/// `apt-get update` is cheap next to the install itself. Permanent failures
/// (auth cancelled, pkexec missing) are never retried, and every failure
/// surfaces the same message the single-shot version used.
pub fn install_dependencies(
    packages: Vec<String>,
    progress: Option<&(dyn Fn(&str, usize, usize, Option<&str>) + Sync)>,
) -> Result<(), String> {
    validate_packages(&packages)?;
    // Batch progress stays coarse (1/1): no per-line apt output to stream.
    let package_label = packages
        .first()
        .cloned()
        .unwrap_or_else(|| "apt".to_string());
    if let Some(report) = progress {
        report(&package_label, 1, 1, None);
    }
    let script = install_script(&packages);
    let mut attempt = 1usize;
    loop {
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
        // Transient-looking failure: tell the UI we are retrying, refresh the
        // apt index (best effort; a stale mirror is the usual cause), pause
        // briefly, then run the same install again.
        if let Some(report) = progress {
            report(
                &package_label,
                1,
                1,
                Some(&format!("第 {attempt}/{MAX_ATTEMPTS} 次安装失败，正在重试")),
            );
        }
        refresh_apt_index();
        thread::sleep(RETRY_DELAY);
        attempt += 1;
    }
}

/// Between-attempts `apt-get update`, wrapped in pkexec like the install
/// itself (polkit remembers the recent authorization, so this normally does
/// not re-prompt). Best effort: a failed refresh does not abort the retry —
/// the install attempt that follows is the actual recovery path.
fn refresh_apt_index() {
    let _ = Command::new("pkexec")
        .args(["sh", "-c", update_script()])
        .output();
}

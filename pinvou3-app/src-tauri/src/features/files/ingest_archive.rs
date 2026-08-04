//! 压缩包摄入（.zip/.rar/.7z）：7z 解压 + 递归 ingest 成员文件。
//!
//! 解压前先用 `7z l -slt` 做压缩炸弹预检（条目数 + 解压后总字节），通过后解压到
//! 唯一临时目录，对每个成员递归调用 facade 主 [`ingest`]，汇总成 markdown。
//! 嵌套压缩包不再展开（防套娃炸弹）。
//!
//! [`ingest`]: super::ingest

use std::path::{Path, PathBuf};

use super::classify;
use super::ingest;
use super::ingest_deps::{archive_tool_command, system_tools};
use super::IngestResult;

/// 压缩包（.zip/.rar/.7z）：先用 7z 列出内容做炸弹预检（条目数 + 解压后总大小，
/// 解压前就拦），通过后解压到临时目录，递归调主 `ingest` 处理每个文件并汇总。
/// 嵌套压缩包不再展开（防套娃炸弹）。因为复用主 ingest，包里的 PDF/Office/图片
/// 都会按各自管线（含 OCR）处理。
pub(super) fn ingest_archive(
    path: &Path,
    basename: String,
    path_str: String,
    byte_size: u64,
) -> IngestResult {
    const MAX_ENTRIES: usize = 50;
    const MAX_TOTAL_BYTES: u64 = 100 * 1024 * 1024; // 解压后总量上限，防压缩炸弹

    let mk_err = |msg: String| {
        IngestResult::warning("archive", &basename, Path::new(&path_str), byte_size, msg)
    };

    if !system_tools().sevenzip {
        return mk_err(archive_tool_missing_message());
    }

    // 预检：解压前就用 7z 列表拦截压缩炸弹。
    match archive_list_stats(path) {
        Ok((count, total)) => {
            if count > MAX_ENTRIES {
                return mk_err(format!(
                    "压缩包条目过多（{count} > {MAX_ENTRIES}），拒绝展开"
                ));
            }
            if total > MAX_TOTAL_BYTES {
                return mk_err(format!(
                    "压缩包解压后约 {:.0} MB，超过 {} MB 上限（疑似压缩炸弹），拒绝展开",
                    total as f64 / 1024.0 / 1024.0,
                    MAX_TOTAL_BYTES / 1024 / 1024
                ));
            }
        }
        Err(e) => return mk_err(format!("压缩包内容读取失败: {e}")),
    }

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmpdir = std::env::temp_dir().join(format!("pinvou3-archive-{ts}"));
    if let Err(e) = std::fs::create_dir_all(&tmpdir) {
        return mk_err(format!("创建临时目录失败: {e}"));
    }

    let extract = archive_tool_command()
        .arg("x")
        .arg("-y")
        .arg(format!("-o{}", tmpdir.display()))
        .arg(path)
        .output();
    if !matches!(&extract, Ok(o) if o.status.success()) {
        let detail = match extract {
            Ok(o) => String::from_utf8_lossy(&o.stderr).trim().to_string(),
            Err(e) => e.to_string(),
        };
        let _ = std::fs::remove_dir_all(&tmpdir);
        return mk_err(format!("7z 解压失败: {detail}"));
    }

    // 递归收集文件，对每个调主 ingest；嵌套压缩包不展开。
    let mut files = Vec::new();
    collect_files(&tmpdir, &mut files, MAX_ENTRIES);

    let mut sections = Vec::new();
    for f in &files {
        let rel = f
            .strip_prefix(&tmpdir)
            .unwrap_or(f)
            .to_string_lossy()
            .to_string();
        let ext = f
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        if classify(&ext) == "archive" {
            sections.push(format!("### {rel}\n⚠️ 嵌套压缩包，未展开（防套娃）\n"));
            continue;
        }
        let r = ingest(f);
        let body = match (&r.markdown, &r.warning) {
            (Some(md), _) => md.clone(),
            (None, Some(w)) => format!("⚠️ {w}"),
            (None, None) => "(无文本内容)".to_string(),
        };
        sections.push(format!("### {rel} ({})\n{body}\n", r.kind));
    }
    let _ = std::fs::remove_dir_all(&tmpdir);

    if sections.is_empty() {
        return mk_err("压缩包为空或无可识别文件".into());
    }
    let content = format!(
        "压缩包 {} 含 {} 个文件：\n\n{}",
        basename,
        files.len(),
        sections.join("\n")
    );
    IngestResult::with_markdown("archive", &basename, path, byte_size, content)
}

/// `7z l -slt` 列出条目，返回 (文件数, 解压后总字节)。用于解压前的炸弹预检。
fn archive_list_stats(path: &Path) -> Result<(usize, u64), String> {
    let out = archive_tool_command()
        .arg("l")
        .arg("-slt")
        .arg(path)
        .output()
        .map_err(|e| format!("7z 调用失败: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let total: u64 = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("Size = "))
        .filter_map(|v| v.trim().parse::<u64>().ok())
        .sum();
    // `-slt` 的第一个 "Path =" 块是归档自身，扣掉。
    let paths = text
        .lines()
        .filter(|l| l.trim_start().starts_with("Path = "))
        .count();
    Ok((paths.saturating_sub(1), total))
}

fn archive_tool_missing_message() -> String {
    if crate::platform::os::show_archive_dependency_check() {
        let packages = crate::platform::os::archive_dependency_packages();
        if packages.trim().is_empty() {
            "压缩包解析需要 7z，请按当前系统方式安装压缩包解析工具".into()
        } else {
            format!("压缩包解析需要 7z: sudo apt install {packages}")
        }
    } else {
        "内置压缩包解析组件缺失或不可用，请修复或重新安装 pinvou。".into()
    }
}

/// 递归收集目录下的普通文件（不含目录本身），到达 `limit` 即停。
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>, limit: usize) {
    if out.len() >= limit {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        if out.len() >= limit {
            return;
        }
        let p = entry.path();
        if p.is_dir() {
            collect_files(&p, out, limit);
        } else if p.is_file() {
            out.push(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_archive_missing_message_points_to_bundled_runtime() {
        if !crate::platform::capabilities::is_windows() {
            return;
        }
        let message = archive_tool_missing_message();

        assert!(message.contains("内置压缩包解析组件"));
        assert!(!message.contains("sudo apt install"));
        assert!(!message.contains("p7zip-full"));
    }
}

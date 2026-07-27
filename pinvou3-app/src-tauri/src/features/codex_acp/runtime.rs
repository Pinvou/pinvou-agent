use std::fs::File as StdFile;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use base64::Engine;
use flate2::read::GzDecoder;
use futures_util::StreamExt;
use sha2::{Digest, Sha512};
use tokio::io::AsyncWriteExt;
use wait_timeout::ChildExt;

use super::diagnostics;

pub const MANAGED_CODEX_VERSION: &str = "0.144.6";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexRuntimeSource {
    Override,
    System,
    Managed,
    LegacyBundled,
}

impl CodexRuntimeSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Override => "override",
            Self::System => "system",
            Self::Managed => "managed",
            Self::LegacyBundled => "legacy_bundled",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedCodex {
    pub path: PathBuf,
    pub source: CodexRuntimeSource,
    pub version: String,
}

struct ManagedCodexArtifact {
    urls: &'static [&'static str],
    integrity: &'static str,
    vendor_triple: &'static str,
}

fn managed_artifact() -> Result<ManagedCodexArtifact> {
    managed_artifact_for(std::env::consts::OS, std::env::consts::ARCH)
}

fn managed_artifact_for(os: &str, arch: &str) -> Result<ManagedCodexArtifact> {
    match (os, arch) {
        ("linux", "x86_64") => Ok(ManagedCodexArtifact {
            urls: &[
                "https://registry.npmjs.org/@openai/codex/-/codex-0.144.6-linux-x64.tgz",
                "https://registry.npmmirror.com/@openai/codex/-/codex-0.144.6-linux-x64.tgz",
            ],
            integrity: "sha512-4E7EnzCg0OnBxCyYnwJ+qnZwWHYe0YScr5ucKWbngE9u4+0XrpWELqq2Kn9jl5GZK8MDjU7PrJwFIwusHOHjuw==",
            vendor_triple: "x86_64-unknown-linux-musl",
        }),
        ("linux", "aarch64") => Ok(ManagedCodexArtifact {
            urls: &[
                "https://registry.npmjs.org/@openai/codex/-/codex-0.144.6-linux-arm64.tgz",
                "https://registry.npmmirror.com/@openai/codex/-/codex-0.144.6-linux-arm64.tgz",
            ],
            integrity: "sha512-PGiLXMN+2IQRkf7tOLi64dMInjU1pRLbz0Rwfj/yt2Y97SZQqAjFQoi2wmswmqtqMDnfwCPTC1DRXVQkvU6T6Q==",
            vendor_triple: "aarch64-unknown-linux-musl",
        }),
        ("windows", "x86_64") => Ok(ManagedCodexArtifact {
            urls: &[
                "https://registry.npmjs.org/@openai/codex/-/codex-0.144.6-win32-x64.tgz",
                "https://registry.npmmirror.com/@openai/codex/-/codex-0.144.6-win32-x64.tgz",
            ],
            integrity: "sha512-dN39VnjEthKz5io1RNWwZDtErdSn07nW3pGUgvlA6DMxgm/nuGaIAZO/sG/Hgxq/x5j9HteAENfrFgVkpZ0lFg==",
            vendor_triple: "x86_64-pc-windows-msvc",
        }),
        _ => bail!("当前托管 Codex 下载不支持平台: {os}-{arch}"),
    }
}

fn runtime_root() -> PathBuf {
    crate::platform::paths::pinvou3_home()
        .join("runtimes")
        .join("codex")
}

fn managed_release_dir() -> Result<PathBuf> {
    managed_artifact()?;
    Ok(runtime_root().join(format!(
        "codex-{MANAGED_CODEX_VERSION}-{}-{}",
        std::env::consts::OS,
        std::env::consts::ARCH
    )))
}

pub fn managed_codex_path() -> Option<PathBuf> {
    let artifact = managed_artifact().ok()?;
    let candidate = managed_release_dir()
        .ok()?
        .join("vendor")
        .join(artifact.vendor_triple)
        .join("bin")
        .join(if crate::platform::capabilities::is_windows() {
            "codex.exe"
        } else {
            "codex"
        });
    candidate.is_file().then_some(candidate)
}

pub fn resolve_codex_path(
    system_codex: Option<PathBuf>,
    legacy_bundled: Option<PathBuf>,
) -> Option<ResolvedCodex> {
    if let Some(path) = std::env::var_os("PINVOU3_CODEX_PATH").map(PathBuf::from) {
        if let Some(resolved) = probe_codex(path, CodexRuntimeSource::Override) {
            if version_meets_minimum(&resolved.version) {
                return Some(resolved);
            }
        }
    }

    select_newest_eligible([
        legacy_bundled.and_then(|path| probe_codex(path, CodexRuntimeSource::LegacyBundled)),
        system_codex.and_then(|path| probe_codex(path, CodexRuntimeSource::System)),
        managed_codex_path().and_then(|path| probe_codex(path, CodexRuntimeSource::Managed)),
    ])
}

fn probe_codex(path: PathBuf, source: CodexRuntimeSource) -> Option<ResolvedCodex> {
    if !path.is_file() {
        return None;
    }
    let version = codex_version(&path)?;
    Some(ResolvedCodex {
        path,
        source,
        version,
    })
}

fn select_newest_eligible<const N: usize>(
    candidates: [Option<ResolvedCodex>; N],
) -> Option<ResolvedCodex> {
    candidates
        .into_iter()
        .flatten()
        .filter(|candidate| version_meets_minimum(&candidate.version))
        .max_by(|left, right| compare_versions(&left.version, &right.version))
}

pub fn version_meets_minimum(version: &str) -> bool {
    compare_versions(version, MANAGED_CODEX_VERSION).is_ge()
}

pub fn is_managed_newer_than(version: &str) -> bool {
    compare_versions(MANAGED_CODEX_VERSION, version).is_gt()
}

fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    parse_version(left).cmp(&parse_version(right))
}

fn parse_version(version: &str) -> Vec<u64> {
    version
        .split(['.', '-', '+'])
        .take_while(|part| part.chars().all(|character| character.is_ascii_digit()))
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

pub fn codex_version(path: &Path) -> Option<String> {
    codex_version_result(path).ok()
}

fn codex_version_result(path: &Path) -> Result<String> {
    let mut child = std::process::Command::new(path)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("启动 Codex 自检失败: {}", path.display()))?;
    let status = match child
        .wait_timeout(Duration::from_secs(3))
        .context("等待 Codex 自检进程失败")?
    {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Codex 自检超过 3 秒");
        }
    };
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    if !status.success() {
        bail!("Codex 自检进程退出: {status}; stderr={}", stderr.trim());
    }
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .context("读取 Codex 自检标准输出失败")?
        .read_to_string(&mut stdout)
        .context("解析 Codex 自检标准输出失败")?;
    stdout
        .split_whitespace()
        .last()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .context("Codex 自检未返回版本号")
}

pub async fn install_managed_codex(
    downloaded_bytes: Arc<AtomicU64>,
    total_bytes: Arc<AtomicU64>,
    operation_id: &str,
) -> Result<PathBuf> {
    if let Some(path) = managed_codex_path().filter(|path| {
        codex_version(path)
            .as_deref()
            .is_some_and(version_meets_minimum)
    }) {
        diagnostics::write(
            operation_id,
            "runtime:already_available",
            format!("path={}", path.display()),
        );
        return Ok(path);
    }
    let artifact = managed_artifact()?;
    let runtime_root = runtime_root();
    diagnostics::write(
        operation_id,
        "runtime:start",
        format!(
            "version={MANAGED_CODEX_VERSION} platform={}-{} vendor_triple={} runtime_root={}",
            std::env::consts::OS,
            std::env::consts::ARCH,
            artifact.vendor_triple,
            runtime_root.display()
        ),
    );
    tokio::fs::create_dir_all(&runtime_root)
        .await
        .context("创建 Codex Runtime 目录失败")?;

    let target = managed_release_dir()?;
    let stamp = chrono::Utc::now().timestamp_millis();
    let staging = runtime_root.join(format!(".staging-{}-{stamp}", std::process::id()));
    let archive_path = staging.join("codex.tgz");
    let extracted = staging.join("runtime");
    tokio::fs::create_dir_all(&extracted)
        .await
        .context("创建 Codex 下载 staging 目录失败")?;
    diagnostics::write(
        operation_id,
        "runtime:staging_ready",
        format!("staging={} target={}", staging.display(), target.display()),
    );

    downloaded_bytes.store(0, Ordering::Release);
    total_bytes.store(0, Ordering::Release);
    let result = async {
        let client = reqwest::Client::new();
        let mut response = None;
        let mut last_error = None;
        for (source_index, url) in artifact.urls.iter().enumerate() {
            diagnostics::write(
                operation_id,
                "download:attempt",
                format!("source_index={source_index}"),
            );
            match client.get(*url).send().await {
                Ok(candidate) => match candidate.error_for_status() {
                    Ok(candidate) => {
                        diagnostics::write(
                            operation_id,
                            "download:response",
                            format!(
                                "source_index={source_index} status={} content_length={}",
                                candidate.status(),
                                candidate
                                    .content_length()
                                    .map(|value| value.to_string())
                                    .unwrap_or_else(|| "unknown".to_string())
                            ),
                        );
                        response = Some(candidate);
                        break;
                    }
                    Err(error) => {
                        diagnostics::write(
                            operation_id,
                            "download:http_error",
                            format!("source_index={source_index} error={error:#}"),
                        );
                        last_error = Some(format!("{url}: {error}"));
                    }
                },
                Err(error) => {
                    diagnostics::write(
                        operation_id,
                        "download:transport_error",
                        format!("source_index={source_index} error={error:#}"),
                    );
                    last_error = Some(format!("{url}: {error}"));
                }
            }
        }
        let response = response.with_context(|| {
            format!(
                "下载托管 Codex 失败{}",
                last_error
                    .as_deref()
                    .map(|error| format!(": {error}"))
                    .unwrap_or_default()
            )
        })?;
        if let Some(total) = response.content_length() {
            total_bytes.store(total, Ordering::Release);
        }
        let mut file = tokio::fs::File::create(&archive_path)
            .await
            .context("创建 Codex 下载文件失败")?;
        let mut hasher = Sha512::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("读取 Codex 下载数据失败")?;
            hasher.update(&chunk);
            file.write_all(&chunk)
                .await
                .context("写入 Codex 下载文件失败")?;
            downloaded_bytes.fetch_add(chunk.len() as u64, Ordering::AcqRel);
        }
        file.flush().await.context("刷新 Codex 下载文件失败")?;
        drop(file);

        diagnostics::write(
            operation_id,
            "download:complete",
            format!(
                "downloaded_bytes={} expected_bytes={}",
                downloaded_bytes.load(Ordering::Acquire),
                total_bytes.load(Ordering::Acquire)
            ),
        );

        verify_integrity(&hasher.finalize(), artifact.integrity)?;
        diagnostics::write(operation_id, "integrity:verified", "algorithm=sha512");
        let archive_for_extract = archive_path.clone();
        let extract_for_task = extracted.clone();
        let triple = artifact.vendor_triple.to_string();
        diagnostics::write(operation_id, "extract:start", "format=tgz");
        tokio::task::spawn_blocking(move || {
            extract_vendor_archive(&archive_for_extract, &extract_for_task, &triple)
        })
        .await
        .context("等待 Codex 解压任务失败")??;
        diagnostics::write(operation_id, "extract:complete", "result=success");

        let codex = extracted
            .join("vendor")
            .join(artifact.vendor_triple)
            .join("bin")
            .join(if crate::platform::capabilities::is_windows() {
                "codex.exe"
            } else {
                "codex"
            });
        if !codex.is_file() {
            bail!("托管 Codex 解压完成，但未找到可执行文件");
        }
        let version = codex_version_result(&codex).context("托管 Codex 可执行文件自检失败")?;
        diagnostics::write(
            operation_id,
            "self_test:complete",
            format!("path={} version={version}", codex.display()),
        );

        if target.exists() {
            diagnostics::write(
                operation_id,
                "activation:remove_existing",
                format!("target={}", target.display()),
            );
            tokio::fs::remove_dir_all(&target)
                .await
                .context("清理损坏的旧 Codex Runtime 失败")?;
        }
        activate_runtime_with_retry(&extracted, &target, operation_id).await?;
        diagnostics::write(
            operation_id,
            "activation:renamed",
            format!("target={}", target.display()),
        );
        let activated = managed_codex_path().context("托管 Codex 激活后仍不可用")?;
        diagnostics::write(
            operation_id,
            "activation:verified",
            format!("path={}", activated.display()),
        );
        Ok(activated)
    }
    .await;

    match remove_staging_with_retry(&staging, operation_id).await {
        Ok(()) => diagnostics::write(operation_id, "staging:cleaned", "result=success"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            diagnostics::write(operation_id, "staging:cleaned", "result=already_absent")
        }
        Err(error) => diagnostics::write(
            operation_id,
            "staging:cleanup_failed",
            format!("path={} error={error}", staging.display()),
        ),
    }
    if let Err(error) = &result {
        diagnostics::write(operation_id, "runtime:failed", format!("error={error:#}"));
        downloaded_bytes.store(0, Ordering::Release);
        total_bytes.store(0, Ordering::Release);
    } else {
        diagnostics::write(operation_id, "runtime:complete", "result=success");
    }
    result
}

async fn activate_runtime_with_retry(
    extracted: &Path,
    target: &Path,
    operation_id: &str,
) -> Result<()> {
    const MAX_ATTEMPTS: u32 = 12;
    for attempt in 1..=MAX_ATTEMPTS {
        match tokio::fs::rename(extracted, target).await {
            Ok(()) => return Ok(()),
            Err(error) if should_retry_windows_file_lock(&error) && attempt < MAX_ATTEMPTS => {
                let delay_ms = u64::from(attempt.min(5)) * 500;
                diagnostics::write(
                    operation_id,
                    "activation:retry",
                    format!(
                        "attempt={attempt} max_attempts={MAX_ATTEMPTS} delay_ms={delay_ms} error={error}"
                    ),
                );
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            Err(error) => {
                return Err(error).context("激活托管 Codex Runtime 失败");
            }
        }
    }
    unreachable!("activation retry loop always returns")
}

async fn remove_staging_with_retry(staging: &Path, operation_id: &str) -> io::Result<()> {
    const MAX_ATTEMPTS: u32 = 5;
    for attempt in 1..=MAX_ATTEMPTS {
        match tokio::fs::remove_dir_all(staging).await {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(error),
            Err(error) if should_retry_windows_file_lock(&error) && attempt < MAX_ATTEMPTS => {
                let delay_ms = u64::from(attempt) * 300;
                diagnostics::write(
                    operation_id,
                    "staging:cleanup_retry",
                    format!(
                        "attempt={attempt} max_attempts={MAX_ATTEMPTS} delay_ms={delay_ms} error={error}"
                    ),
                );
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("staging cleanup retry loop always returns")
}

fn should_retry_windows_file_lock(error: &io::Error) -> bool {
    should_retry_file_lock_for_platform(error, crate::platform::capabilities::is_windows())
}

fn should_retry_file_lock_for_platform(error: &io::Error, is_windows: bool) -> bool {
    is_windows
        && (error.kind() == io::ErrorKind::PermissionDenied
            || matches!(error.raw_os_error(), Some(5 | 32)))
}

fn verify_integrity(actual: &[u8], integrity: &str) -> Result<()> {
    let encoded = integrity
        .strip_prefix("sha512-")
        .context("托管 Codex integrity 不是 SHA-512")?;
    let expected = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .context("解析托管 Codex SHA-512 失败")?;
    if actual != expected {
        bail!("托管 Codex 完整性校验失败");
    }
    Ok(())
}

fn extract_vendor_archive(archive_path: &Path, target: &Path, triple: &str) -> Result<()> {
    let file = StdFile::open(archive_path).context("打开 Codex 下载包失败")?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let prefix = PathBuf::from("package").join("vendor").join(triple);
    let mut extracted_any = false;
    for entry in archive.entries().context("读取 Codex 下载包失败")? {
        let mut entry = entry.context("读取 Codex 下载包条目失败")?;
        let path = entry.path().context("解析 Codex 下载包路径失败")?;
        if !path.starts_with(&prefix) {
            continue;
        }
        validate_relative_archive_path(&path)?;
        let relative = path
            .strip_prefix("package")
            .context("解析 Codex 下载包相对路径失败")?;
        let output = target.join(relative);
        let entry_type = entry.header().entry_type();
        if !entry_type.is_file() && !entry_type.is_dir() {
            bail!("托管 Codex 下载包包含不支持的链接或特殊文件");
        }
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent).context("创建 Codex 解压目录失败")?;
        }
        entry
            .unpack(&output)
            .with_context(|| format!("解压 Codex 文件失败: {}", output.display()))?;
        extracted_any = true;
    }
    if !extracted_any {
        bail!("托管 Codex 下载包中没有当前平台文件");
    }
    Ok(())
}

fn validate_relative_archive_path(path: &Path) -> Result<()> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "下载包包含不安全路径").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_source_names_are_stable() {
        assert_eq!(CodexRuntimeSource::System.as_str(), "system");
        assert_eq!(CodexRuntimeSource::Managed.as_str(), "managed");
    }

    #[test]
    fn integrity_rejects_wrong_digest() {
        let actual = Sha512::digest(b"pinvou");
        assert!(verify_integrity(
            &actual,
            "sha512-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=="
        )
        .is_err());
    }

    #[test]
    fn archive_path_rejects_parent_segments() {
        assert!(validate_relative_archive_path(Path::new("../codex")).is_err());
        assert!(validate_relative_archive_path(Path::new("package/vendor/bin/codex")).is_ok());
    }

    #[test]
    fn windows_x64_managed_artifact_is_available() {
        let artifact = managed_artifact_for("windows", "x86_64").unwrap();
        assert_eq!(artifact.vendor_triple, "x86_64-pc-windows-msvc");
        assert!(artifact.urls[0].starts_with("https://"));
        assert!(artifact.integrity.starts_with("sha512-"));
    }

    #[test]
    fn minimum_version_rejects_old_system_codex() {
        assert!(!version_meets_minimum("0.139.0"));
        assert!(version_meets_minimum("0.144.6"));
        assert!(version_meets_minimum("0.145.0"));
    }

    #[test]
    fn newest_eligible_candidate_wins() {
        let selected = select_newest_eligible([
            Some(ResolvedCodex {
                path: PathBuf::from("system"),
                source: CodexRuntimeSource::System,
                version: "0.145.0".to_string(),
            }),
            Some(ResolvedCodex {
                path: PathBuf::from("managed"),
                source: CodexRuntimeSource::Managed,
                version: MANAGED_CODEX_VERSION.to_string(),
            }),
        ])
        .unwrap();
        assert_eq!(selected.source, CodexRuntimeSource::System);
        assert_eq!(selected.version, "0.145.0");
    }

    #[test]
    fn managed_candidate_replaces_old_system_candidate() {
        let selected = select_newest_eligible([
            Some(ResolvedCodex {
                path: PathBuf::from("system"),
                source: CodexRuntimeSource::System,
                version: "0.139.0".to_string(),
            }),
            Some(ResolvedCodex {
                path: PathBuf::from("managed"),
                source: CodexRuntimeSource::Managed,
                version: MANAGED_CODEX_VERSION.to_string(),
            }),
        ])
        .unwrap();
        assert_eq!(selected.source, CodexRuntimeSource::Managed);
        assert_eq!(selected.version, MANAGED_CODEX_VERSION);
    }

    #[test]
    fn windows_file_lock_errors_are_retryable() {
        assert!(should_retry_file_lock_for_platform(
            &io::Error::from_raw_os_error(5),
            true
        ));
        assert!(should_retry_file_lock_for_platform(
            &io::Error::from_raw_os_error(32),
            true
        ));
        assert!(!should_retry_file_lock_for_platform(
            &io::Error::from_raw_os_error(2),
            true
        ));
        assert!(!should_retry_file_lock_for_platform(
            &io::Error::from_raw_os_error(5),
            false
        ));
    }
}

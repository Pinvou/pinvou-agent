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
}

struct ManagedCodexArtifact {
    urls: &'static [&'static str],
    integrity: &'static str,
    vendor_triple: &'static str,
}

fn managed_artifact() -> Result<ManagedCodexArtifact> {
    if std::env::consts::OS != "linux" {
        bail!(
            "当前托管 Codex 下载仅支持 Linux，当前平台: {}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
    }
    match std::env::consts::ARCH {
        "x86_64" => Ok(ManagedCodexArtifact {
            urls: &[
                "https://registry.npmjs.org/@openai/codex/-/codex-0.144.6-linux-x64.tgz",
                "https://registry.npmmirror.com/@openai/codex/-/codex-0.144.6-linux-x64.tgz",
            ],
            integrity: "sha512-4E7EnzCg0OnBxCyYnwJ+qnZwWHYe0YScr5ucKWbngE9u4+0XrpWELqq2Kn9jl5GZK8MDjU7PrJwFIwusHOHjuw==",
            vendor_triple: "x86_64-unknown-linux-musl",
        }),
        "aarch64" => Ok(ManagedCodexArtifact {
            urls: &[
                "https://registry.npmjs.org/@openai/codex/-/codex-0.144.6-linux-arm64.tgz",
                "https://registry.npmmirror.com/@openai/codex/-/codex-0.144.6-linux-arm64.tgz",
            ],
            integrity: "sha512-PGiLXMN+2IQRkf7tOLi64dMInjU1pRLbz0Rwfj/yt2Y97SZQqAjFQoi2wmswmqtqMDnfwCPTC1DRXVQkvU6T6Q==",
            vendor_triple: "aarch64-unknown-linux-musl",
        }),
        arch => bail!("当前托管 Codex 下载不支持 CPU 架构: {arch}"),
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
        if codex_is_usable(&path) {
            return Some(ResolvedCodex {
                path,
                source: CodexRuntimeSource::Override,
            });
        }
    }
    if let Some(path) = system_codex.filter(|path| codex_is_usable(path)) {
        return Some(ResolvedCodex {
            path,
            source: CodexRuntimeSource::System,
        });
    }
    if let Some(path) = managed_codex_path().filter(|path| codex_is_usable(path)) {
        return Some(ResolvedCodex {
            path,
            source: CodexRuntimeSource::Managed,
        });
    }
    legacy_bundled
        .filter(|path| codex_is_usable(path))
        .map(|path| ResolvedCodex {
            path,
            source: CodexRuntimeSource::LegacyBundled,
        })
}

fn codex_is_usable(path: &Path) -> bool {
    path.is_file() && codex_version(path).is_some()
}

pub fn codex_version(path: &Path) -> Option<String> {
    let mut child = std::process::Command::new(path)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let status = match child.wait_timeout(Duration::from_secs(3)).ok()? {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    };
    if !status.success() {
        return None;
    }
    let mut stdout = String::new();
    child.stdout.take()?.read_to_string(&mut stdout).ok()?;
    stdout
        .split_whitespace()
        .last()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub async fn install_managed_codex(
    downloaded_bytes: Arc<AtomicU64>,
    total_bytes: Arc<AtomicU64>,
) -> Result<PathBuf> {
    if let Some(path) = managed_codex_path().filter(|path| codex_is_usable(path)) {
        return Ok(path);
    }
    let artifact = managed_artifact()?;
    let runtime_root = runtime_root();
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

    downloaded_bytes.store(0, Ordering::Release);
    total_bytes.store(0, Ordering::Release);
    let result = async {
        let client = reqwest::Client::new();
        let mut response = None;
        let mut last_error = None;
        for url in artifact.urls {
            match client.get(*url).send().await {
                Ok(candidate) => match candidate.error_for_status() {
                    Ok(candidate) => {
                        response = Some(candidate);
                        break;
                    }
                    Err(error) => last_error = Some(format!("{url}: {error}")),
                },
                Err(error) => last_error = Some(format!("{url}: {error}")),
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

        verify_integrity(&hasher.finalize(), artifact.integrity)?;
        let archive_for_extract = archive_path.clone();
        let extract_for_task = extracted.clone();
        let triple = artifact.vendor_triple.to_string();
        tokio::task::spawn_blocking(move || {
            extract_vendor_archive(&archive_for_extract, &extract_for_task, &triple)
        })
        .await
        .context("等待 Codex 解压任务失败")??;

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
        if codex_version(&codex).is_none() {
            bail!("托管 Codex 可执行文件自检失败");
        }

        if target.exists() {
            tokio::fs::remove_dir_all(&target)
                .await
                .context("清理损坏的旧 Codex Runtime 失败")?;
        }
        tokio::fs::rename(&extracted, &target)
            .await
            .context("激活托管 Codex Runtime 失败")?;
        managed_codex_path().context("托管 Codex 激活后仍不可用")
    }
    .await;

    let _ = tokio::fs::remove_dir_all(&staging).await;
    if result.is_err() {
        downloaded_bytes.store(0, Ordering::Release);
        total_bytes.store(0, Ordering::Release);
    }
    result
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
}

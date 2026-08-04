//! 飞书、企微、钉钉原生 CLI 的按需安装器。
//!
//! 版本、下载地址与两层 SHA-256 都来自随程序编译的目标平台 lock；运行时只在用户
//! 首次启用连接器时联网，校验归档后只提取预期的单个可执行文件，避免路径穿越。

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path};
use std::sync::Mutex;
use std::time::Duration;

use flate2::read::GzDecoder;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const MAX_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_BINARY_BYTES: u64 = 128 * 1024 * 1024;
static INSTALL_LOCK: Mutex<()> = Mutex::new(());
const DWS_LICENSE: &str =
    include_str!("../../../resources/common/bundle/dingtalk-skills/dws/LICENSE");
const LARK_LICENSE: &str = include_str!(
    "../../../resources/platforms/linux/x86_64/bundle/connectors/linux-x64/licenses/LICENSE-lark-cli"
);
const WECOM_LICENSE: &str = include_str!(
    "../../../resources/platforms/linux/x86_64/bundle/connectors/linux-x64/licenses/LICENSE-wecom-cli"
);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectorLock {
    schema_version: u32,
    platform: String,
    artifacts: Vec<Artifact>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Artifact {
    name: String,
    version: String,
    url: String,
    archive_sha256: String,
    binary_sha256: String,
}

/// 安装一个锁定版本的厂家原生 CLI。返回 `true` 表示本次实际写入了文件。
pub fn ensure_native_cli(name: &str) -> Result<bool, String> {
    let _guard = INSTALL_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let lock = load_lock()?;
    let artifact = lock
        .artifacts
        .iter()
        .find(|artifact| artifact.name == name)
        .cloned()
        .ok_or_else(|| format!("当前平台没有 {name} 的已审核安装记录"))?;

    let bin_dir = crate::platform::paths::managed_connector_bin_dir()
        .ok_or_else(|| "当前平台暂不支持此连接器 CLI".to_string())?;
    let filename = super::platform::executable_name(name);
    let destination = bin_dir.join(&filename);
    write_license(&bin_dir, name)?;
    if file_sha256_matches(&destination, &artifact.binary_sha256) {
        return Ok(false);
    }

    fs::create_dir_all(&bin_dir).map_err(|e| format!("创建连接器目录失败: {e}"))?;
    let cache_dir = crate::platform::paths::pinvou3_home()
        .join("cache/connectors")
        .join(&lock.platform);
    fs::create_dir_all(&cache_dir).map_err(|e| format!("创建连接器缓存目录失败: {e}"))?;
    let archive_ext = if artifact.url.ends_with(".zip") {
        "zip"
    } else {
        "tar.gz"
    };
    let archive = cache_dir.join(format!(
        "{}-{}.{}",
        artifact.name, artifact.version, archive_ext
    ));
    if !file_sha256_matches(&archive, &artifact.archive_sha256) {
        download_verified(&artifact, &archive)?;
    }

    let binary = extract_expected_binary(&archive, &artifact)
        .map_err(|e| format!("解压 {} 失败: {e}", artifact.name))?;
    let actual = sha256_bytes(&binary);
    if actual != artifact.binary_sha256 {
        return Err(format!(
            "{} 可执行文件校验失败(expected {}, got {})",
            artifact.name, artifact.binary_sha256, actual
        ));
    }

    let staging = bin_dir.join(format!(".{filename}.installing-{}", std::process::id()));
    let _ = fs::remove_file(&staging);
    let mut file = File::create(&staging).map_err(|e| format!("创建安装暂存文件失败: {e}"))?;
    file.write_all(&binary)
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("写入安装暂存文件失败: {e}"))?;
    super::platform::set_executable_permissions(&staging)
        .map_err(|e| format!("设置连接器执行权限失败: {e}"))?;
    if destination.exists() {
        fs::remove_file(&destination).map_err(|e| format!("替换旧连接器失败: {e}"))?;
    }
    fs::rename(&staging, &destination).map_err(|e| format!("完成连接器安装失败: {e}"))?;
    Ok(true)
}

fn write_license(bin_dir: &Path, name: &str) -> Result<(), String> {
    let text = match name {
        "dws" => DWS_LICENSE,
        "lark-cli" => LARK_LICENSE,
        "wecom-cli" => WECOM_LICENSE,
        _ => return Err(format!("未知连接器: {name}")),
    };
    let platform_dir = bin_dir
        .parent()
        .ok_or_else(|| "连接器安装目录无效".to_string())?;
    let licenses = platform_dir.join("licenses");
    fs::create_dir_all(&licenses).map_err(|e| format!("创建连接器许可证目录失败: {e}"))?;
    fs::write(licenses.join(format!("LICENSE-{name}")), text)
        .map_err(|e| format!("写入连接器许可证失败: {e}"))
}

fn load_lock() -> Result<ConnectorLock, String> {
    let lock_json = super::platform::lock_json();
    if lock_json.is_empty() {
        return Err("当前平台暂不支持此连接器 CLI".to_string());
    }
    let lock: ConnectorLock =
        serde_json::from_str(lock_json).map_err(|e| format!("连接器锁文件无效: {e}"))?;
    if lock.schema_version != 1 {
        return Err(format!("不支持的连接器锁文件版本: {}", lock.schema_version));
    }
    let expected = crate::platform::paths::connector_platform_dir(
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
    .ok_or_else(|| "当前平台暂不支持此连接器 CLI".to_string())?;
    if lock.platform != expected {
        return Err(format!(
            "连接器锁文件平台不匹配(expected {expected}, got {})",
            lock.platform
        ));
    }
    Ok(lock)
}

fn download_verified(artifact: &Artifact, destination: &Path) -> Result<(), String> {
    let url = reqwest::Url::parse(&artifact.url).map_err(|e| format!("下载地址无效: {e}"))?;
    if url.scheme() != "https" {
        return Err("连接器下载仅允许 HTTPS".to_string());
    }
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(180))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 || attempt.url().scheme() != "https" {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .user_agent("Pinvou-Agent connector-installer")
        .build()
        .map_err(|e| format!("创建下载客户端失败: {e}"))?;
    let response = client
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|e| format!("下载 {} 失败: {e}", artifact.name))?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_ARCHIVE_BYTES)
    {
        return Err("连接器归档超过 128 MiB 安全上限".to_string());
    }

    let partial = destination.with_extension("part");
    let _ = fs::remove_file(&partial);
    let mut reader = response.take(MAX_ARCHIVE_BYTES + 1);
    let mut file = File::create(&partial).map_err(|e| format!("创建下载暂存文件失败: {e}"))?;
    let copied = io::copy(&mut reader, &mut file).map_err(|e| format!("保存下载失败: {e}"))?;
    file.sync_all()
        .map_err(|e| format!("同步下载文件失败: {e}"))?;
    if copied > MAX_ARCHIVE_BYTES {
        let _ = fs::remove_file(&partial);
        return Err("连接器归档超过 128 MiB 安全上限".to_string());
    }
    let actual = crate::platform::hashing::sha256_file(&partial)
        .map_err(|e| format!("读取下载文件失败: {e}"))?;
    if actual != artifact.archive_sha256 {
        let _ = fs::remove_file(&partial);
        return Err(format!(
            "{} 下载校验失败(expected {}, got {})",
            artifact.name, artifact.archive_sha256, actual
        ));
    }
    if destination.exists() {
        fs::remove_file(destination).map_err(|e| format!("替换连接器缓存失败: {e}"))?;
    }
    fs::rename(&partial, destination).map_err(|e| format!("保存连接器缓存失败: {e}"))
}

fn extract_expected_binary(archive: &Path, artifact: &Artifact) -> io::Result<Vec<u8>> {
    let expected = super::platform::archive_member(&artifact.name);
    let file = File::open(archive)?;
    if artifact.url.ends_with(".zip") {
        extract_zip_member(file, expected)
    } else {
        extract_tar_member(GzDecoder::new(file), expected)
    }
}

fn extract_tar_member<R: Read>(reader: R, expected: &str) -> io::Result<Vec<u8>> {
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries()? {
        let mut entry = entry?;
        if normalized_path_eq(&entry.path()?, expected) {
            return read_limited(&mut entry, MAX_BINARY_BYTES);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("归档中缺少 {expected}"),
    ))
}

fn extract_zip_member<R: Read + io::Seek>(reader: R, expected: &str) -> io::Result<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(reader)?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if normalized_path_eq(Path::new(entry.name()), expected) {
            return read_limited(&mut entry, MAX_BINARY_BYTES);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("归档中缺少 {expected}"),
    ))
}

fn normalized_path_eq(path: &Path, expected: &str) -> bool {
    let actual = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy()),
            Component::CurDir => None,
            _ => Some("<unsafe>".into()),
        })
        .collect::<Vec<_>>()
        .join("/");
    actual == expected
}

fn read_limited(reader: &mut impl Read, max: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "连接器可执行文件超过 128 MiB 安全上限",
        ));
    }
    Ok(bytes)
}

fn file_sha256_matches(path: &Path, expected: &str) -> bool {
    crate::platform::hashing::sha256_file(path).is_ok_and(|actual| actual == expected)
}

fn sha256_bytes(bytes: &[u8]) -> String {
    crate::platform::encoding::hex_lower(&Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_matches_current_target_and_has_three_pinned_artifacts() {
        let lock = load_lock().unwrap();
        assert_eq!(lock.artifacts.len(), 3);
        for name in ["dws", "lark-cli", "wecom-cli"] {
            let artifact = lock
                .artifacts
                .iter()
                .find(|item| item.name == name)
                .unwrap();
            assert!(artifact.url.starts_with("https://"));
            assert_eq!(artifact.archive_sha256.len(), 64);
            assert_eq!(artifact.binary_sha256.len(), 64);
            assert!(!artifact.version.is_empty());
        }
    }

    #[test]
    fn archive_path_matching_rejects_parent_and_absolute_paths() {
        assert!(normalized_path_eq(
            Path::new("./package/bin/wecom-cli"),
            "package/bin/wecom-cli"
        ));
        assert!(!normalized_path_eq(
            Path::new("../package/bin/wecom-cli"),
            "package/bin/wecom-cli"
        ));
        assert!(!normalized_path_eq(
            Path::new("/package/bin/wecom-cli"),
            "package/bin/wecom-cli"
        ));
    }
}

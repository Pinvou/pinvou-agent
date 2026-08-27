//! 连接器 CLI 二进制的下载/校验/落盘核心（跨功能低层原语）。
//!
//! 版本化资产库 `assets/cli/<name>/<version>/` 的唯一写入点：lock 表驱动
//! （内置连接器，`features/connectors::native_installer`）与 plugin.json 声明
//! 驱动（第三方声明式 CLI 连接器包，`features/marketplace::plugin_import`，
//! §14.3）共用同一管线——按「跨功能复用的低层能力进全局 platform」的边界
//! （`connector_lock` 同例）从这里供数，避免 marketplace ↔ connectors 功能环。

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use flate2::read::GzDecoder;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const MAX_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_BINARY_BYTES: u64 = 128 * 1024 * 1024;
static INSTALL_LOCK: Mutex<()> = Mutex::new(());

/// 安装互斥锁（内置 lock 驱动与声明驱动共用，两条管线对同一资产目录的写入
/// 必须串行）。`pub(crate)`：内置安装器（features/connectors::native_installer）
/// 与旧布局迁移复用同一把锁。
pub(crate) fn install_lock() -> MutexGuard<'static, ()> {
    INSTALL_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 单个 CLI 制品的下载 pin（lock 表条目与声明驱动共用 schema）。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Artifact {
    pub name: String,
    pub version: String,
    pub url: String,
    pub archive_sha256: String,
    pub binary_sha256: String,
}

/// 声明式 CLI 连接器包的下载声明（§14.3，M2）：版本/URL/双哈希来自包的
/// plugin.json 而非平台 lock 表。license 文本可选（缺省跳过写 license）。
#[derive(Debug, Clone)]
pub(crate) struct DeclaredCliArtifact {
    pub bin: String,
    pub version: String,
    pub url: String,
    pub archive_sha256: String,
    pub binary_sha256: String,
    pub license: Option<String>,
}

/// 声明驱动安装（第三方声明式 CLI 连接器包）：与 lock 表驱动同一管线
/// （下载 → archive SHA-256 → 防穿越解出目标 exe → binary SHA-256 → 原子落
/// 版本目录），但不读 lock 表、无旧布局迁移（声明式包是全新安装形态）。
/// 归档内二进制成员约定为根级 `<bin>[.exe]`（内置连接器的历史成员布局
/// 仅适用于 lock 表路径）。
pub(crate) fn ensure_declared_native_cli(decl: &DeclaredCliArtifact) -> Result<bool, String> {
    let _guard = install_lock();
    let artifact = Artifact {
        name: decl.bin.clone(),
        version: decl.version.clone(),
        url: decl.url.clone(),
        archive_sha256: decl.archive_sha256.clone(),
        binary_sha256: decl.binary_sha256.clone(),
    };
    let member = super::connector_lock::executable_name(&decl.bin);
    install_artifact(&artifact, &member, decl.license.as_deref())
}

/// 下载/校验/落盘共用核心：暂存下载（缓存命中跳过）→ 双哈希校验 → 原子落
/// `assets/cli/<name>/<version>/`。`member` = 归档内目标二进制路径；
/// `license` = 可选许可证文本（内置连接器查内嵌表，声明式包取声明字段）。
/// `pub(crate)`：内置安装器（lock 表驱动）复用。
pub(crate) fn install_artifact(
    artifact: &Artifact,
    member: &str,
    license: Option<&str>,
) -> Result<bool, String> {
    let version_dir = super::paths::assets_cli_dir(&artifact.name, &artifact.version);
    let filename = super::connector_lock::executable_name(&artifact.name);
    let destination = version_dir.join(&filename);
    if file_sha256_matches(&destination, &artifact.binary_sha256) {
        // 二进制已就位(hash 比对通过)时 license 必然随上次释放落过盘,不再重写,
        // 避免每次按需检查都白写一次 license 文件。
        return Ok(false);
    }
    if let Some(text) = license {
        write_license_text(&version_dir, &artifact.name, text)?;
    }

    fs::create_dir_all(&version_dir).map_err(|e| format!("创建连接器目录失败: {e}"))?;
    let platform =
        super::paths::connector_platform_dir(std::env::consts::OS, std::env::consts::ARCH)
            .ok_or_else(|| "当前平台暂不支持此连接器 CLI".to_string())?;
    let staging_dir = super::paths::assets_staging_dir().join(platform);
    fs::create_dir_all(&staging_dir).map_err(|e| format!("创建连接器暂存目录失败: {e}"))?;
    let archive_ext = if artifact.url.ends_with(".zip") {
        "zip"
    } else {
        "tar.gz"
    };
    let archive = staging_dir.join(format!(
        "{}-{}.{}",
        artifact.name, artifact.version, archive_ext
    ));
    if !file_sha256_matches(&archive, &artifact.archive_sha256) {
        download_verified(artifact, &archive)?;
    }

    let binary = extract_expected_binary(&archive, artifact, member)
        .map_err(|e| format!("解压 {} 失败: {e}", artifact.name))?;
    let actual = sha256_bytes(&binary);
    if actual != artifact.binary_sha256 {
        return Err(format!(
            "{} 可执行文件校验失败(expected {}, got {})",
            artifact.name, artifact.binary_sha256, actual
        ));
    }

    let staging = version_dir.join(format!(".{filename}.installing-{}", std::process::id()));
    let _ = fs::remove_file(&staging);
    let mut file = File::create(&staging).map_err(|e| format!("创建安装暂存文件失败: {e}"))?;
    file.write_all(&binary)
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("写入安装暂存文件失败: {e}"))?;
    set_executable_permissions(&staging).map_err(|e| format!("设置连接器执行权限失败: {e}"))?;
    if destination.exists() {
        fs::remove_file(&destination).map_err(|e| format!("替换旧连接器失败: {e}"))?;
    }
    fs::rename(&staging, &destination).map_err(|e| format!("完成连接器安装失败: {e}"))?;
    // GC 策略：同 name 的旧版本目录**保守保留暂不删**——资产按「包只引用不拥有」
    // 共享（§4），删除需要引用计数支撑；CLI 二进制体积小，滞留成本低。
    // 引用计数/GC 随存储布局迁移 PR 一并落地。
    Ok(true)
}

/// 许可证文本落盘到版本目录旁的 `licenses/`（内置连接器内嵌文本 / 声明式包
/// 声明字段共用）。`pub(crate)`：内置安装器复用。
pub(crate) fn write_license_text(bin_dir: &Path, name: &str, text: &str) -> Result<(), String> {
    let platform_dir = bin_dir
        .parent()
        .ok_or_else(|| "连接器安装目录无效".to_string())?;
    let licenses = platform_dir.join("licenses");
    fs::create_dir_all(&licenses).map_err(|e| format!("创建连接器许可证目录失败: {e}"))?;
    fs::write(licenses.join(format!("LICENSE-{name}")), text)
        .map_err(|e| format!("写入连接器许可证失败: {e}"))
}

/// 文件 SHA-256 与期望值的比对（不存在/读失败即 false）。
/// `pub(crate)`：内置安装器的旧布局迁移复用同一口径。
pub(crate) fn file_sha256_matches(path: &Path, expected: &str) -> bool {
    super::hashing::sha256_file(path).is_ok_and(|actual| actual == expected)
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
    let actual =
        super::hashing::sha256_file(&partial).map_err(|e| format!("读取下载文件失败: {e}"))?;
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

fn extract_expected_binary(
    archive: &Path,
    artifact: &Artifact,
    member: &str,
) -> io::Result<Vec<u8>> {
    let file = File::open(archive)?;
    if artifact.url.ends_with(".zip") {
        extract_zip_member(file, member)
    } else {
        extract_tar_member(GzDecoder::new(file), member)
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

/// 归档成员路径的防穿越归一比对（`..`/绝对路径分量一律不命中）。
/// `pub(crate)`：内置安装器测试复用。
pub(crate) fn normalized_path_eq(path: &Path, expected: &str) -> bool {
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

fn sha256_bytes(bytes: &[u8]) -> String {
    super::encoding::hex_lower(&Sha256::digest(bytes))
}

/// Unix 上补可执行位（Windows 无-op）。OS 原语，platform 层就地实现。
fn set_executable_permissions(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

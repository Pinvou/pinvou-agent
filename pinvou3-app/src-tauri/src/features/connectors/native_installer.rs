//! 飞书、企微、钉钉原生 CLI 的按需安装器。
//!
//! 版本、下载地址与两层 SHA-256 都来自随程序编译的目标平台 lock；运行时只在用户
//! 首次启用连接器时联网，校验归档后只提取预期的单个可执行文件，避免路径穿越。
// architecture-guard: allow-target-cfg -- 平台专属 license 文本必须按目标平台各自内嵌(对齐 platform.rs LOCK_JSON 门控),数据选择而非适配逻辑,留在安装器内最内聚。

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
// lark/wecom 是平台专属二进制,license 文本随平台包走——按目标平台 cfg 各自内嵌
// (写法对齐 platform.rs 的 LOCK_JSON 5 平台门控),避免非 Linux 构建误嵌 Linux 版文本。
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const LARK_LICENSE: &str = include_str!(
    "../../../resources/platforms/linux/x86_64/bundle/connectors/linux-x64/licenses/LICENSE-lark-cli"
);
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const WECOM_LICENSE: &str = include_str!(
    "../../../resources/platforms/linux/x86_64/bundle/connectors/linux-x64/licenses/LICENSE-wecom-cli"
);
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const LARK_LICENSE: &str = include_str!(
    "../../../resources/platforms/linux/aarch64/bundle/connectors/linux-arm64/licenses/LICENSE-lark-cli"
);
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const WECOM_LICENSE: &str = include_str!(
    "../../../resources/platforms/linux/aarch64/bundle/connectors/linux-arm64/licenses/LICENSE-wecom-cli"
);
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const LARK_LICENSE: &str = include_str!(
    "../../../resources/platforms/macos/aarch64/bundle/connectors/darwin-arm64/licenses/LICENSE-lark-cli"
);
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const WECOM_LICENSE: &str = include_str!(
    "../../../resources/platforms/macos/aarch64/bundle/connectors/darwin-arm64/licenses/LICENSE-wecom-cli"
);
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const LARK_LICENSE: &str = include_str!(
    "../../../resources/platforms/macos/x86_64/bundle/connectors/darwin-x64/licenses/LICENSE-lark-cli"
);
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const WECOM_LICENSE: &str = include_str!(
    "../../../resources/platforms/macos/x86_64/bundle/connectors/darwin-x64/licenses/LICENSE-wecom-cli"
);
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const LARK_LICENSE: &str = include_str!(
    "../../../resources/platforms/windows/x86_64/bundle/connectors/windows-x64/licenses/LICENSE-lark-cli"
);
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const WECOM_LICENSE: &str = include_str!(
    "../../../resources/platforms/windows/x86_64/bundle/connectors/windows-x64/licenses/LICENSE-wecom-cli"
);
// 非支持平台(如未来 Windows ARM64)兜底为空串,保证可编译——对齐 platform/mod.rs
// LOCK_JSON 的 not(any(...)) 兜底;运行时 load_lock 同样返回"当前平台暂不支持"。
#[cfg(not(any(
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "aarch64"),
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "windows", target_arch = "x86_64"),
)))]
const LARK_LICENSE: &str = "";
#[cfg(not(any(
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "aarch64"),
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "windows", target_arch = "x86_64"),
)))]
const WECOM_LICENSE: &str = "";

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
///
/// 版本化布局（marketplace-unification §4）：二进制落
/// `~/.pinvou3/assets/cli/<name>/<version>/<exe>`，升级 = 新版本目录就位，
/// 不再原地覆盖；同版本同哈希已在盘 → 直接返回（幂等语义不变）。
/// 下载/解包暂存收编到 `assets/.staging/`（旧 `cache/connectors/` 退役，
/// 残留不清理——内容只是缓存，重下自愈）。
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

    // 连接时自愈：旧布局（connectors/<platform>/bin/）里已验证的存量二进制
    // 先迁移到版本目录（幂等；已持 INSTALL_LOCK，走 locked 实现）。
    migrate_legacy_binary(&artifact.name, &artifact.version, &artifact.binary_sha256);

    let version_dir = crate::platform::paths::assets_cli_dir(&artifact.name, &artifact.version);
    let filename = super::platform::executable_name(name);
    let destination = version_dir.join(&filename);
    if file_sha256_matches(&destination, &artifact.binary_sha256) {
        // 二进制已就位(hash 比对通过)时 license 必然随上次释放落过盘,不再重写,
        // 避免每次按需检查都白写一次 license 文件。
        return Ok(false);
    }
    write_license(&version_dir, name)?;

    fs::create_dir_all(&version_dir).map_err(|e| format!("创建连接器目录失败: {e}"))?;
    let staging_dir = crate::platform::paths::assets_staging_dir().join(&lock.platform);
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

    let staging = version_dir.join(format!(".{filename}.installing-{}", std::process::id()));
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
    // GC 策略：同 name 的旧版本目录**保守保留暂不删**——资产按「包只引用不拥有」
    // 共享（§4），删除需要引用计数支撑；CLI 二进制体积小，滞留成本低。
    // 引用计数/GC 随存储布局迁移 PR 一并落地。
    Ok(true)
}

/// 旧布局（`connectors/<platform>/bin/`，无版本）→ 版本化资产库的一次性迁移
/// 入口（§9.3）。启动路径调用；幂等：迁移后旧文件不在即 no-op。
pub fn migrate_legacy_cli_binaries() {
    let _guard = INSTALL_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // 平台不支持/lock 缺失 → 无旧布局可迁
    let Ok(lock) = load_lock() else {
        return;
    };
    for artifact in &lock.artifacts {
        migrate_legacy_binary(&artifact.name, &artifact.version, &artifact.binary_sha256);
    }
}

/// 单个 CLI 的旧布局迁移（调用前须已持 INSTALL_LOCK；参数显式传入便于测试）。
/// 对照 lock 钉住的 SHA-256：匹配 → **移动**（非复制）到版本目录；不匹配 → 不动
/// （store 侧已是 degraded 语义，重连会重下）。旧 bin 目录腾空后清理。
fn migrate_legacy_binary(name: &str, version: &str, expected_sha256: &str) {
    let Some(bin_dir) = crate::platform::paths::managed_connector_bin_dir() else {
        return;
    };
    let exe = super::platform::executable_name(name);
    let legacy = bin_dir.join(&exe);
    if !legacy.is_file() {
        return;
    }
    let version_dir = crate::platform::paths::assets_cli_dir(name, version);
    let destination = version_dir.join(&exe);
    if file_sha256_matches(&destination, expected_sha256) {
        // 版本目录已有校验通过的二进制：旧文件是经校验相同的重复残留才删，
        // 内容不符则不动（不替用户删来历不明的文件）。
        if file_sha256_matches(&legacy, expected_sha256) {
            let _ = fs::remove_file(&legacy);
        }
    } else if file_sha256_matches(&legacy, expected_sha256) {
        if fs::create_dir_all(&version_dir).is_ok() && fs::rename(&legacy, &destination).is_ok() {
            log::info!("[connectors] 旧布局 CLI 迁移到版本目录: {name}@{version}");
        }
        // rename 失败（跨盘/占用）不阻塞：下次启动/连接重试
    }
    // bin 目录腾空后清理（licenses 等旁挂内容在平台目录，不在 bin 内）
    if bin_dir.is_dir() {
        let empty = fs::read_dir(&bin_dir).map(|mut rd| rd.next().is_none());
        if empty.unwrap_or(false) {
            let _ = fs::remove_dir(&bin_dir);
        }
    }
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

    /// 把 PINVOU3_HOME 指到干净临时目录跑闭包（借 ENV_LOCK 与其它 env 测试串行）。
    fn with_temp_home<F: FnOnce()>(f: F) {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-native-installer-test-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: 持 platform::paths::tests::ENV_LOCK,进程内 env 写已串行化。
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        f();
        match prev {
            // SAFETY: 持 platform::paths::tests::ENV_LOCK,进程内 env 写已串行化。
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            // SAFETY: 持 platform::paths::tests::ENV_LOCK,进程内 env 写已串行化。
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 旧布局迁移（§9.3）：SHA-256 匹配 → 移动到版本目录并清理腾空的 bin 目录；
    /// 不匹配 → 原样保留（degraded 语义）；版本目录已有同哈希二进制时旧文件
    /// 属重复残留 → 删除；旧版本目录保守保留（GC 留后续 PR）。全程幂等。
    #[test]
    fn migrate_legacy_binary_moves_matching_keeps_mismatching() {
        with_temp_home(|| {
            let Some(bin_dir) = crate::platform::paths::managed_connector_bin_dir() else {
                return; // 当前平台无旧布局目录（不支持的架构），无从断言
            };
            let exe = crate::features::connectors::platform::executable_name("test-cli");
            let legacy = bin_dir.join(&exe);
            let dest = crate::platform::paths::assets_cli_dir("test-cli", "9.9.9").join(&exe);
            // 旧版本目录残留（GC 保守保留的断言对象）
            let old_version_exe =
                crate::platform::paths::assets_cli_dir("test-cli", "9.9.8").join(&exe);

            fs::create_dir_all(&bin_dir).unwrap();
            fs::write(&legacy, b"fake-cli-binary").unwrap();
            let sha = crate::platform::connector_lock::file_sha256_hex(&legacy).unwrap();
            fs::create_dir_all(old_version_exe.parent().unwrap()).unwrap();
            fs::write(&old_version_exe, b"older").unwrap();

            // 匹配 → 移动；bin 目录腾空清理；幂等
            migrate_legacy_binary("test-cli", "9.9.9", &sha);
            assert!(dest.is_file(), "匹配应移动到版本目录");
            assert!(!legacy.exists(), "移动后旧文件不在");
            assert!(!bin_dir.exists(), "腾空后 bin 目录应清理");
            migrate_legacy_binary("test-cli", "9.9.9", &sha);
            assert!(dest.is_file(), "二次调用幂等");
            assert!(
                old_version_exe.is_file(),
                "旧版本目录保守保留（GC 后续 PR）"
            );

            // 版本目录已就位 + 旧文件是同内容残留 → 删除重复
            fs::create_dir_all(&bin_dir).unwrap();
            fs::write(&legacy, b"fake-cli-binary").unwrap();
            migrate_legacy_binary("test-cli", "9.9.9", &sha);
            assert!(!legacy.exists(), "经校验相同的重复残留应删除");
            assert!(dest.is_file());

            // 不匹配 → 原样保留（不替用户删来历不明的文件），bin 目录不动
            fs::create_dir_all(&bin_dir).unwrap();
            fs::write(&legacy, b"tampered-content").unwrap();
            migrate_legacy_binary("test-cli", "9.9.9", &sha);
            assert!(legacy.is_file(), "不匹配应原样保留");
            assert!(bin_dir.is_dir(), "未腾空的 bin 目录保留");
        });
    }
}

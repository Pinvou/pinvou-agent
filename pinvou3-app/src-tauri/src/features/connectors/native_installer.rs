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
/// GitHub 资产加速前缀（如自建 gh-proxy）。设置后对官方源在 github.com 的制品，
/// 下载顺序变为「前缀加速地址 → lock 表审核镜像 → 官方源」；不适用于其他站点。
const GITHUB_ASSET_MIRROR_PREFIX_ENV: &str = "PINVOU3_GITHUB_ASSET_MIRROR_PREFIX";
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
    /// 已验证可达且字节与官方源一致的国内镜像（可选）。当前仅 wecom-cli 配置
    /// npmmirror 镜像；dws/lark-cli 仅发布在 GitHub Release，暂无官方国内镜像，
    /// 可用 [`GITHUB_ASSET_MIRROR_PREFIX_ENV`] 按环境加速。
    mirror_url: Option<String>,
    archive_sha256: String,
    binary_sha256: String,
}

/// GitHub 加速前缀的纯函数核心：仅对官方源在 github.com 的地址生效，其余
/// 地址原样返回 `None`（避免把任意站点误包进第三方代理）。前缀结尾斜杠可有
/// 可无，统一归一化成 gh-proxy 规范形 `<proxy>/https://github.com/...`——
/// 无斜杠前缀若按原样黏连（`format!("{prefix}{url}")`）会拼出形如
/// `proxy.examplehttps` 的非法域名，在 DNS 阶段静默退化成官方源直连。
/// 拼错的加速地址会在校验失败后自然落到下一候选。
fn github_prefixed_url(prefix: &str, url: &str) -> Option<String> {
    let prefix = prefix.trim();
    if prefix.is_empty() {
        return None;
    }
    let parsed = reqwest::Url::parse(url).ok()?;
    if parsed.host_str() != Some("github.com") {
        return None;
    }
    Some(format!("{}/{}", prefix.trim_end_matches('/'), url))
}

/// 日志/报错里展示候选地址前抹掉 userinfo（`user:pass@host`）：用户可能在
/// 加速前缀里带入凭据，诊断输出不应落凭据。解析失败时原样返回——该地址
/// 本来也会在 HTTPS 门禁处被跳过，不会再被请求。
fn redact_url_credentials(url_text: &str) -> String {
    let mut parsed = match reqwest::Url::parse(url_text) {
        Ok(parsed) => parsed,
        Err(_) => return url_text.to_string(),
    };
    if !parsed.username().is_empty() || parsed.password().is_some() {
        let _ = parsed.set_username("");
        let _ = parsed.set_password(None);
    }
    parsed.to_string()
}

/// 按序尝试的下载地址：环境变量显式指定的 GitHub 加速前缀 → lock 表审核过的
/// 镜像 → 官方源兜底。官方源恒在列表末尾；每个候选下载后都要过
/// `archive_sha256` 校验，镜像字节被篡改时会被校验拦截并落到下一候选。
fn artifact_download_urls(artifact: &Artifact) -> Vec<String> {
    let prefix = std::env::var(GITHUB_ASSET_MIRROR_PREFIX_ENV).ok();
    artifact_download_urls_with_prefix(prefix.as_deref(), artifact)
}

/// [`artifact_download_urls`] 的纯函数核心（便于单测，不触环境变量）。
fn artifact_download_urls_with_prefix(prefix: Option<&str>, artifact: &Artifact) -> Vec<String> {
    let mut urls = Vec::new();
    if let Some(prefix) = prefix
        && let Some(prefixed) = github_prefixed_url(prefix, &artifact.url)
    {
        urls.push(prefixed);
    }
    if let Some(mirror) = artifact
        .mirror_url
        .as_deref()
        .map(str::trim)
        .filter(|mirror| !mirror.is_empty())
    {
        urls.push(mirror.to_string());
    }
    urls.push(artifact.url.clone());
    urls
}

/// 安装一个锁定版本的厂家原生 CLI。
///
/// 版本化布局（marketplace-unification §4）：二进制落
/// `~/.pinvou3/assets/cli/<name>/<version>/<exe>`，升级 = 新版本目录就位，
/// 不再原地覆盖；同版本同哈希已在盘 → 直接返回（幂等语义不变）。
/// 下载/解包暂存收编到 `assets/.staging/`（旧 `cache/connectors/` 退役，
/// 残留不清理——内容只是缓存，重下自愈）。
pub fn ensure_native_cli(name: &str) -> Result<(), String> {
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
    let filename = crate::platform::connector_lock::executable_name(name);
    let destination = version_dir.join(&filename);
    if file_sha256_matches(&destination, &artifact.binary_sha256) {
        // 二进制已就位(hash 比对通过)时 license 必然随上次释放落过盘,不再重写,
        // 避免每次按需检查都白写一次 license 文件。
        return Ok(());
    }
    write_license(&version_dir, name)?;

    fs::create_dir_all(&version_dir).map_err(|e| format!("创建连接器目录失败: {e}"))?;
    let staging_dir = crate::platform::paths::assets_staging_dir().join(&lock.platform);
    fs::create_dir_all(&staging_dir).map_err(|e| format!("创建连接器暂存目录失败: {e}"))?;
    // 归档格式按首个候选地址判定：镜像与官方源对同一制品的归档格式一致
    // （wecom 同一 tgz、dws/lark 同一 tar.gz/zip），缓存文件名因此稳定。
    let candidate_urls = artifact_download_urls(&artifact);
    let archive_ext = if candidate_urls[0].ends_with(".zip") {
        "zip"
    } else {
        "tar.gz"
    };
    let archive = staging_dir.join(format!(
        "{}-{}.{}",
        artifact.name, artifact.version, archive_ext
    ));
    let source_url = if file_sha256_matches(&archive, &artifact.archive_sha256) {
        candidate_urls[0].clone()
    } else {
        download_verified(&artifact, &archive)?
    };

    let binary = extract_expected_binary(&archive, &source_url, &artifact)
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
    Ok(())
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
    let exe = crate::platform::connector_lock::executable_name(name);
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
    } else if file_sha256_matches(&legacy, expected_sha256)
        && fs::create_dir_all(&version_dir).is_ok()
        && fs::rename(&legacy, &destination).is_ok()
    {
        log::info!("[connectors] 旧布局 CLI 迁移到版本目录: {name}@{version}");
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
    let lock_json = crate::platform::connector_lock::lock_json();
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

/// 按候选地址顺序下载归档并校验归档 SHA-256，返回实际命中的下载地址
/// （调用方据此判定归档格式）。任何候选的网络失败或校验不符都会清掉 `.part`
/// 并尝试下一候选；全部候选失败时返回带候选总数的汇总错误。
fn download_verified(artifact: &Artifact, destination: &Path) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        // 每个候选源 15 分钟（reqwest 的 client timeout 按单次请求计）：归档
        // 上限 128 MiB,180s 只够 ~730 KB/s 的链路,慢网用户每次都恰好死在半途
        // 且无断点续传;15 分钟覆盖到 ~150 KB/s,同时仍保证卡死连接最终会失败
        // 而不是挂住安装流程。多候选回退时最坏情形按候选数翻倍。
        .timeout(crate::platform::download::ARTIFACT_DOWNLOAD_TOTAL_TIMEOUT)
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

    let candidates = artifact_download_urls(artifact);
    let total_candidates = candidates.len();
    let mut failures: Vec<String> = Vec::new();
    for url_text in candidates {
        // 非法候选（环境变量前缀拼错、非 HTTPS 等）只跳过并告警，不整体失败：
        // 后面的审核镜像/官方源兜底不受用户配置错误牵连。
        let url = match reqwest::Url::parse(&url_text) {
            Ok(url) if url.scheme() == "https" => url,
            _ => {
                // 候选地址整体进日志与报错，先抹掉 userinfo 再落文。
                let error = format!(
                    "下载地址无效或非 HTTPS: {}",
                    redact_url_credentials(&url_text)
                );
                log::warn!("[connectors] {} 跳过候选地址: {error}", artifact.name);
                failures.push(error);
                continue;
            }
        };
        match download_from_url(&client, &url, artifact, destination) {
            Ok(()) => return Ok(url_text),
            Err(error) => {
                // 非默认端口写进前缀，避免同机多端口候选在报错里无法区分。
                let host = match url.port() {
                    Some(port) => {
                        format!("{}:{port}", url.host_str().unwrap_or("<unknown-host>"))
                    }
                    None => url.host_str().unwrap_or("<unknown-host>").to_string(),
                };
                log::warn!(
                    "[connectors] {} 下载源失败，尝试下一候选地址: {error}",
                    artifact.name
                );
                failures.push(format!("[{host}] {error}"));
            }
        }
    }
    // 全部候选失败时把「试过多少个源、每个源各自的失败原因」都带进报错
    // （release 构建没有 logger，逐候选的 log::warn! 不可见，报错本身要能
    // 说明镜像被试过、失败出在哪一层；只保留最后一个错误会把触发镜像重试
    // 的首个根因藏掉）。
    Err(match failures.as_slice() {
        [] => "无可用下载地址".to_string(),
        list => format!(
            "{} 归档下载失败（{} 个候选下载源全部未成功）: {}",
            artifact.name,
            total_candidates,
            list.join("；")
        ),
    })
}

/// 从单一地址下载归档到 `destination`（`.part` 暂存 → SHA-256 校验 → 原子
/// rename）。校验不符按失败处理，由调用方决定是否换下一候选地址。
fn download_from_url(
    client: &reqwest::blocking::Client,
    url: &reqwest::Url,
    artifact: &Artifact,
    destination: &Path,
) -> Result<(), String> {
    let response = client
        .get(url.clone())
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
    // 写入阶段任一步失败（磁盘满/连接中断）同样清掉 .part：失败残留既占
    // 磁盘（上限 128 MiB），也与「任何候选失败都清理暂存」的语义一致。
    let write_result = (|| -> Result<u64, String> {
        let mut file = File::create(&partial).map_err(|e| format!("创建下载暂存文件失败: {e}"))?;
        let copied = io::copy(&mut reader, &mut file).map_err(|e| format!("保存下载失败: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("同步下载文件失败: {e}"))?;
        Ok(copied)
    })();
    let copied = match write_result {
        Ok(copied) => copied,
        Err(error) => {
            let _ = fs::remove_file(&partial);
            return Err(error);
        }
    };
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

fn extract_expected_binary(
    archive: &Path,
    source_url: &str,
    artifact: &Artifact,
) -> io::Result<Vec<u8>> {
    let expected = super::platform::archive_member(&artifact.name);
    let file = File::open(archive)?;
    if source_url.ends_with(".zip") {
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
    use crate::platform::test_support::with_temp_home;

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

    /// wecom-cli 的 lock 镜像必须与官方 URL 同路径、仅域名换成 npmmirror
    /// （npmmirror 是 registry 同步镜像——不承诺字节级一致，两端归档以
    /// SHA-256 pin 为准，review 中实测一致）；dws/lark-cli 只发布在
    /// GitHub Release，暂无审核过的国内镜像。
    fn assert_wecom_mirror_invariants(lock: &ConnectorLock) {
        for artifact in &lock.artifacts {
            if artifact.name != "wecom-cli" {
                assert!(
                    artifact.mirror_url.is_none(),
                    "{} 不应有审核镜像",
                    artifact.name
                );
                continue;
            }
            let mirror = artifact.mirror_url.as_deref().expect("wecom 应配置镜像");
            assert!(
                mirror.starts_with("https://registry.npmmirror.com/")
                    && artifact.url.starts_with("https://registry.npmjs.org/"),
                "mirror={mirror} url={}",
                artifact.url
            );
            assert_eq!(
                mirror.strip_prefix("https://registry.npmmirror.com/"),
                artifact.url.strip_prefix("https://registry.npmjs.org/"),
                "镜像与官方源必须同路径"
            );
        }
    }

    #[test]
    fn current_platform_lock_mirror_invariants_hold() {
        let lock = load_lock().unwrap();
        assert_wecom_mirror_invariants(&lock);
    }

    /// 全部五个平台的 lock 都要过同一镜像不变量：全量 cargo test 只在
    /// linux-x86_64 上执行（macos/windows 仅跑过滤子集），linux-aarch64 与
    /// macos-x86_64 的 lock 不出现在任何测试执行环境里，其 mirrorUrl 的路径
    /// 拼写错误只能在这里被静态拦下。
    #[test]
    fn all_platform_locks_pass_mirror_invariants() {
        const ALL_PLATFORM_LOCKS: [&str; 5] = [
            include_str!(
                "../../../resources/platforms/linux/aarch64/bundle/connectors/connectors.lock.json"
            ),
            include_str!(
                "../../../resources/platforms/linux/x86_64/bundle/connectors/connectors.lock.json"
            ),
            include_str!(
                "../../../resources/platforms/macos/aarch64/bundle/connectors/connectors.lock.json"
            ),
            include_str!(
                "../../../resources/platforms/macos/x86_64/bundle/connectors/connectors.lock.json"
            ),
            include_str!(
                "../../../resources/platforms/windows/x86_64/bundle/connectors/connectors.lock.json"
            ),
        ];
        for lock_json in ALL_PLATFORM_LOCKS {
            let lock: ConnectorLock =
                serde_json::from_str(lock_json).expect("平台 lock 必须能反序列化");
            assert_eq!(lock.schema_version, 1);
            assert_wecom_mirror_invariants(&lock);
        }
    }

    /// 候选地址进日志/报错前必须抹掉 userinfo：用户可能在加速前缀里带入
    /// 凭据，诊断输出不应落凭据。
    #[test]
    fn candidate_url_display_redacts_userinfo() {
        let redacted = redact_url_credentials(
            "https://user:pass@proxy.example/https://github.com/openai/dws/archive/v1.tar.gz",
        );
        assert!(!redacted.contains("user:pass"), "{redacted}");
        assert!(redacted.contains("proxy.example"), "{redacted}");
        // 无 userinfo 的地址原样保留。
        assert_eq!(
            redact_url_credentials("https://proxy.example/x"),
            "https://proxy.example/x"
        );
        // 解析失败的串原样返回（不会被请求，只在 HTTPS 门禁处跳过）。
        assert_eq!(redact_url_credentials("not a url"), "not a url");
    }

    /// 候选顺序：GitHub 加速前缀（仅对 github.com 生效）→ 审核镜像 → 官方源；
    /// 官方源恒在末尾，非 GitHub 制品不受前缀影响。走纯函数核心，不触环境
    /// 变量（导出了该环境变量的开发机上照样成立）。
    #[test]
    fn artifact_download_urls_order_prefix_mirror_then_official() {
        let artifact = Artifact {
            name: "wecom-cli".into(),
            version: "1.0.0".into(),
            url: "https://registry.npmjs.org/@wecom/cli-linux-x64/-/cli-linux-x64-1.0.0.tgz".into(),
            mirror_url: Some(
                "https://registry.npmmirror.com/@wecom/cli-linux-x64/-/cli-linux-x64-1.0.0.tgz"
                    .into(),
            ),
            archive_sha256: "0".repeat(64),
            binary_sha256: "0".repeat(64),
        };
        // 无前缀：审核镜像 → 官方源。
        assert_eq!(
            artifact_download_urls_with_prefix(None, &artifact),
            vec![
                "https://registry.npmmirror.com/@wecom/cli-linux-x64/-/cli-linux-x64-1.0.0.tgz",
                "https://registry.npmjs.org/@wecom/cli-linux-x64/-/cli-linux-x64-1.0.0.tgz",
            ]
        );
        // npmjs 制品即使配了前缀也不套加速地址。
        assert_eq!(
            artifact_download_urls_with_prefix(Some("https://gh-proxy.example"), &artifact),
            artifact_download_urls_with_prefix(None, &artifact),
            "非 github.com 官方源不得套加速前缀"
        );

        let github_artifact = Artifact {
            name: "dws".into(),
            version: "1.0.0".into(),
            url: "https://github.com/DingTalk-Real-AI/dingtalk-workspace-cli/releases/download/v1.0.0/dws-linux-amd64.tar.gz".into(),
            mirror_url: None,
            archive_sha256: "0".repeat(64),
            binary_sha256: "0".repeat(64),
        };
        // 无前缀：仅官方源（dws/lark-cli 暂无审核镜像）。
        assert_eq!(
            artifact_download_urls_with_prefix(None, &github_artifact),
            vec![github_artifact.url.clone()]
        );
        // 有前缀：前缀加速地址在前，官方源兜底。
        assert_eq!(
            artifact_download_urls_with_prefix(Some("https://mirror.example/gh/"), &github_artifact),
            vec![
                "https://mirror.example/gh/https://github.com/DingTalk-Real-AI/dingtalk-workspace-cli/releases/download/v1.0.0/dws-linux-amd64.tar.gz".to_string(),
                github_artifact.url.clone(),
            ]
        );

        // 前缀拼进候选后仍会在下载前过 HTTPS 复查；空白前缀无效。结尾斜杠
        // 带不带、带几个都必须归一化成同一规范形（文档承诺两种写法均可）。
        let expected =
            "https://mirror.example/gh/https://github.com/org/repo/releases/download/v1/a.tar.gz";
        assert_eq!(
            github_prefixed_url(
                "https://mirror.example/gh/",
                "https://github.com/org/repo/releases/download/v1/a.tar.gz"
            ),
            Some(expected.to_string())
        );
        assert_eq!(
            github_prefixed_url(
                "https://mirror.example/gh",
                "https://github.com/org/repo/releases/download/v1/a.tar.gz"
            ),
            Some(expected.to_string()),
            "无结尾斜杠的前缀不得黏连出非法域名静默退化成直连"
        );
        assert_eq!(
            github_prefixed_url(
                "https://mirror.example/gh///",
                "https://github.com/org/repo/releases/download/v1/a.tar.gz"
            ),
            Some(expected.to_string())
        );
        assert_eq!(github_prefixed_url("  ", "https://github.com/o/r"), None);
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

    /// 旧布局迁移（§9.3）：SHA-256 匹配 → 移动到版本目录并清理腾空的 bin 目录；
    /// 不匹配 → 原样保留（degraded 语义）；版本目录已有同哈希二进制时旧文件
    /// 属重复残留 → 删除；旧版本目录保守保留（GC 留后续 PR）。全程幂等。
    #[test]
    fn migrate_legacy_binary_moves_matching_keeps_mismatching() {
        with_temp_home("pinvou3-native-installer-test", || {
            let Some(bin_dir) = crate::platform::paths::managed_connector_bin_dir() else {
                return; // 当前平台无旧布局目录（不支持的架构），无从断言
            };
            let exe = crate::platform::connector_lock::executable_name("test-cli");
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

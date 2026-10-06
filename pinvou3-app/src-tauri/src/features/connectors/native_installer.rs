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

const MAX_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_BINARY_BYTES: u64 = 128 * 1024 * 1024;
/// GitHub asset acceleration prefix (e.g. a self-hosted gh-proxy). When set,
/// the download order for artifacts whose official source is github.com
/// becomes "acceleration prefix URL → lock-table reviewed mirror → official
/// source"; it does not apply to other sites.
pub const GITHUB_ASSET_MIRROR_PREFIX_ENV: &str = "PINVOU3_GITHUB_ASSET_MIRROR_PREFIX";
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
    /// Optional domestic (China) mirror verified reachable and byte-identical
    /// to the official source. Currently only wecom-cli configures an
    /// npmmirror mirror; dws/lark-cli are only published on GitHub Release
    /// with no official domestic mirror yet, so acceleration is available
    /// per environment via [`GITHUB_ASSET_MIRROR_PREFIX_ENV`].
    mirror_url: Option<String>,
    archive_sha256: String,
    binary_sha256: String,
}

/// Pure-function core of the GitHub acceleration prefix: applies only to
/// addresses whose official source is github.com; every other address
/// returns `None` as-is (avoiding wrapping arbitrary sites into the
/// third-party proxy). The prefix's trailing slash is optional; it is always
/// normalized to the gh-proxy canonical form
/// `<proxy>/https://github.com/...` — a slash-less prefix concatenated
/// blindly (`format!("{prefix}{url}")`) would produce an invalid domain like
/// `proxy.examplehttps`, silently degrading to a direct connection to the
/// official source at the DNS stage. A mistyped acceleration URL naturally
/// falls through to the next candidate once verification fails.
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

/// Download URLs tried in order: the GitHub acceleration prefix explicitly
/// set via environment variable → the lock-table reviewed mirror → the
/// official source as fallback. The official source is always last; every
/// candidate download must pass `archive_sha256` verification, so a mirror
/// with tampered bytes is caught by the check and the next candidate is
/// tried.
fn artifact_download_urls(artifact: &Artifact) -> Vec<String> {
    let prefix = std::env::var(GITHUB_ASSET_MIRROR_PREFIX_ENV).ok();
    artifact_download_urls_with_prefix(prefix.as_deref(), artifact)
}

/// Pure-function core of [`artifact_download_urls`] (unit-testable, does not
/// touch environment variables).
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

    // 连接时自愈：旧布局（connectors/<platform>/bin/）里的存量二进制按 lock
    // 校验迁移到版本目录；钉住版本已在版本目录时，同名残留（滞留旧版本）被
    // 清除（幂等；已持 INSTALL_LOCK，走 locked 实现）。
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
    // The archive format is decided by the first candidate URL: the mirror
    // and the official source use the same archive format for the same
    // artifact (the same tgz for wecom, the same tar.gz/zip for dws/lark),
    // so the cache file name stays stable.
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
    let actual = crate::platform::hashing::sha256_hex(&binary);
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
    // Once the install lands, a legacy same-name leftover turns from "the only
    // local runtime" into "a stale copy shadowing the pinned version"; reuse
    // the migration entry to remove it (the migration call at the top of this
    // function ran before the version dir was populated and could only keep it).
    migrate_legacy_binary(&artifact.name, &artifact.version, &artifact.binary_sha256);
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

/// Single-CLI legacy-layout migration (caller must hold INSTALL_LOCK; args
/// passed explicitly for testability). Dispatch on the lock-pinned SHA-256:
/// - Versioned copy present and verified: the legacy same-name file is removed
///   unconditionally — byte-identical means a duplicate leftover, different
///   bytes mean a stale old version. Keeping it would let it shadow the pinned
///   runtime in by-name resolution / PATH (observed: a leftover wecom-cli 0.1.9
///   kept running after the upgrade to 1.2.1). The connector bin dir is
///   app-managed state (only the installer writes there), not user data; the
///   removal is strictly conditioned on the pinned copy being verified in
///   place — while the pinned version is absent, the legacy binary is the
///   connector's only local runtime and must not be touched.
/// - Versioned copy absent: a legacy file matching the pin is MOVED (not
///   copied) into the version dir (skips the download); mismatched → left in
///   place (the store side already models this as degraded; reconnect
///   re-downloads). The legacy bin dir is tidied once emptied.
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
        match fs::remove_file(&legacy) {
            Ok(()) => log::info!(
                "[connectors] legacy CLI leftover removed (pinned version in place): {name}@{version}"
            ),
            // 例：Windows 上残留正在执行（共享冲突）；下次启动/连接重试，
            // PATH 次序修复保证窗口期内按名解析仍命中钉住版本。
            Err(e) => log::warn!(
                "[connectors] legacy CLI leftover removal failed (retried on next boot/connect): {name}@{version}: {e}"
            ),
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

/// Round-40 review: the headless CLI's `connectors ensure-cli` installs the
/// same pinned native payloads into the same `assets_cli_dir` layout but
/// used to skip the license side-files the GUI installer writes next to
/// them — a machine provisioned only through the CLI held pinned
/// third-party binaries with no license text anywhere. This exposes the
/// GUI's writer unchanged (same texts, same `licenses/` location derived
/// from the version dir) so both surfaces ship identical license
/// side-files; the unknown-connector error keeps the npm-lane connectors
/// (tmeet) out, which have no bundled license text.
pub fn write_managed_license(bin_dir: &Path, connector: &str) -> Result<(), String> {
    write_license(bin_dir, connector)
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

/// Downloads the archive in candidate-URL order and verifies the archive's
/// SHA-256, returning the download URL that actually hit (the caller uses it
/// to decide the archive format). Any candidate's network failure or hash
/// mismatch removes the `.part` and tries the next candidate; when all
/// candidates fail, a summary error carrying the candidate count is
/// returned.
fn download_verified(artifact: &Artifact, destination: &Path) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        // 15 minutes per candidate source (reqwest's client timeout counts
        // per single request): with the archive capped at 128 MiB, 180s only
        // sustains a ~730 KB/s link, so slow-network users would die
        // mid-download every time with no resumable download; 15 minutes
        // covers down to ~150 KB/s while still guaranteeing a stuck
        // connection eventually fails instead of hanging the install flow.
        // With multi-candidate fallback the worst case scales by the
        // candidate count.
        .timeout(crate::platform::download::ARTIFACT_DOWNLOAD_TOTAL_TIMEOUT)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let refused =
                attempt.previous().len() >= 10 || attempt.url().scheme() != "https";
            if refused {
                // Name the refused redirect instead of letting the 3xx's
                // empty body finish the download and then report a
                // misleading "verification failed" at the SHA-256 comparison
                // (same policy as the marketplace wheel loop; cross-host
                // HTTPS redirects are legitimate here, no host allowlist).
                let redirect_url = attempt.url().clone();
                attempt.error(format!("connector download redirect left HTTPS: {redirect_url}"))
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
        // Invalid candidates (a mistyped env-var prefix, non-HTTPS, etc.) are
        // only skipped with a warning, not a whole-run failure: the reviewed
        // mirror / official source fallbacks that follow are not dragged down
        // by user misconfiguration.
        let url = match reqwest::Url::parse(&url_text) {
            Ok(url) if url.scheme() == "https" => url,
            _ => {
                // The candidate URL goes into logs and errors as a whole;
                // strip the userinfo before writing it out.
                let error = format!(
                    "download URL is invalid or not HTTPS: {}",
                    crate::platform::download::redact_url_credentials(&url_text)
                );
                log::warn!(
                    "[connectors] {} skipping candidate URL: {error}",
                    artifact.name
                );
                failures.push(error);
                continue;
            }
        };
        match download_from_url(&client, &url, artifact, destination) {
            Ok(()) => return Ok(url_text),
            Err(error) => {
                // Write non-default ports into the prefix so multi-port
                // candidates on the same host stay distinguishable in
                // errors.
                let host = match url.port() {
                    Some(port) => {
                        format!("{}:{port}", url.host_str().unwrap_or("<unknown-host>"))
                    }
                    None => url.host_str().unwrap_or("<unknown-host>").to_string(),
                };
                log::warn!(
                    "[connectors] {} download source failed, trying next candidate: {error}",
                    artifact.name
                );
                failures.push(format!("[{host}] {error}"));
            }
        }
    }
    // When all candidates fail, carry "how many sources were tried and each
    // source's own failure reason" into the error (release builds have no
    // logger, so the per-candidate log::warn! is invisible; the error itself
    // must show that the mirror was tried and at which layer the failure
    // happened; keeping only the last error would hide the first root cause
    // that triggered the mirror retry).
    Err(match failures.as_slice() {
        [] => "no download URLs available".to_string(),
        list => format!(
            "{} archive download failed (all {} candidate download sources exhausted): {}",
            artifact.name,
            total_candidates,
            list.join("; ")
        ),
    })
}

/// Downloads the archive from a single URL into `destination` (`.part`
/// staging → SHA-256 verification → atomic rename). A hash mismatch is
/// treated as failure; the caller decides whether to switch to the next
/// candidate URL.
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
        // Round-40 review: the reqwest error embeds the requested URL
        // (`... for url (...)`); the aggregation below is echoed to the
        // user, so userinfo a mirror prefix carried gets scrubbed here like
        // the invalid-candidate arm above already does.
        .map_err(|e| {
            format!(
                "下载 {} 失败: {}",
                artifact.name,
                crate::platform::download::redact_url_credentials_in_text(&e.to_string())
            )
        })?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_ARCHIVE_BYTES)
    {
        return Err("连接器归档超过 128 MiB 安全上限".to_string());
    }

    let partial = destination.with_extension("part");
    let _ = fs::remove_file(&partial);
    let mut reader = response.take(MAX_ARCHIVE_BYTES + 1);
    // Any failure in the write phase (disk full / connection dropped) also
    // removes the .part: a leftover from a failure would occupy disk (up to
    // 128 MiB) and would break the "any candidate failure cleans up the
    // staging file" semantics.
    let write_result = (|| -> Result<u64, String> {
        let mut file = File::create(&partial)
            .map_err(|e| format!("failed to create partial download file: {e}"))?;
        let copied = io::copy(&mut reader, &mut file)
            .map_err(|e| format!("failed to save download: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("failed to flush downloaded file: {e}"))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::test_support::{
        managed_connector_bin_dir_or_assert_unsupported, with_temp_home,
    };

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

    /// The wecom-cli lock mirror must share the official URL's path with only
    /// the domain swapped to npmmirror (npmmirror is a registry sync mirror —
    /// it does not promise byte-level identity; both ends' archives are
    /// pinned by SHA-256 and measured identical during review); dws/lark-cli
    /// are only published on GitHub Release and have no reviewed domestic
    /// mirror yet.
    fn assert_wecom_mirror_invariants(lock: &ConnectorLock) {
        for artifact in &lock.artifacts {
            if artifact.name != "wecom-cli" {
                assert!(
                    artifact.mirror_url.is_none(),
                    "{} must not have a reviewed mirror",
                    artifact.name
                );
                continue;
            }
            let mirror = artifact
                .mirror_url
                .as_deref()
                .expect("wecom must have a mirror configured");
            assert!(
                mirror.starts_with("https://registry.npmmirror.com/")
                    && artifact.url.starts_with("https://registry.npmjs.org/"),
                "mirror={mirror} url={}",
                artifact.url
            );
            assert_eq!(
                mirror.strip_prefix("https://registry.npmmirror.com/"),
                artifact.url.strip_prefix("https://registry.npmjs.org/"),
                "mirror and official source must share the same path"
            );
        }
    }

    #[test]
    fn current_platform_lock_mirror_invariants_hold() {
        let lock = load_lock().unwrap();
        assert_wecom_mirror_invariants(&lock);
    }

    /// All five platforms' locks must pass the same mirror invariants: the
    /// full cargo test run only executes on linux-x86_64 (macos/windows only
    /// run a filtered subset), so the linux-aarch64 and macos-x86_64 locks
    /// never appear in any test execution environment; a typo in their
    /// mirrorUrl paths can only be caught statically here.
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
                serde_json::from_str(lock_json).expect("platform lock must deserialize");
            assert_eq!(lock.schema_version, 1);
            assert_wecom_mirror_invariants(&lock);
        }
    }

    /// Candidate order: GitHub acceleration prefix (only effective for
    /// github.com) → reviewed mirror → official source; the official source
    /// is always last, and non-GitHub artifacts are unaffected by the prefix.
    /// Goes through the pure-function core, so it holds even on dev machines
    /// that export that environment variable.
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
        // No prefix: reviewed mirror → official source.
        assert_eq!(
            artifact_download_urls_with_prefix(None, &artifact),
            vec![
                "https://registry.npmmirror.com/@wecom/cli-linux-x64/-/cli-linux-x64-1.0.0.tgz",
                "https://registry.npmjs.org/@wecom/cli-linux-x64/-/cli-linux-x64-1.0.0.tgz",
            ]
        );
        // npmjs artifacts never get the acceleration URL even when a prefix
        // is set.
        assert_eq!(
            artifact_download_urls_with_prefix(Some("https://gh-proxy.example"), &artifact),
            artifact_download_urls_with_prefix(None, &artifact),
            "non-github.com official sources must not get the acceleration prefix"
        );

        let github_artifact = Artifact {
            name: "dws".into(),
            version: "1.0.0".into(),
            url: "https://github.com/DingTalk-Real-AI/dingtalk-workspace-cli/releases/download/v1.0.0/dws-linux-amd64.tar.gz".into(),
            mirror_url: None,
            archive_sha256: "0".repeat(64),
            binary_sha256: "0".repeat(64),
        };
        // No prefix: official source only (dws/lark-cli have no reviewed
        // mirror yet).
        assert_eq!(
            artifact_download_urls_with_prefix(None, &github_artifact),
            vec![github_artifact.url.clone()]
        );
        // With prefix: prefix acceleration URL first, official source as the
        // fallback.
        assert_eq!(
            artifact_download_urls_with_prefix(Some("https://mirror.example/gh/"), &github_artifact),
            vec![
                "https://mirror.example/gh/https://github.com/DingTalk-Real-AI/dingtalk-workspace-cli/releases/download/v1.0.0/dws-linux-amd64.tar.gz".to_string(),
                github_artifact.url.clone(),
            ]
        );

        // Once a prefix is folded into a candidate it still passes the HTTPS
        // re-check before download; an empty prefix is invalid. With or
        // without a trailing slash — no matter how many — everything must
        // normalize to the same canonical form (the docs promise both
        // spellings work).
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
            "a slash-less prefix must not concatenate into an invalid domain and silently degrade to a direct connection"
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
    /// 钉住版本已在版本目录校验通过时，同名旧文件一律删除——内容相同属重复
    /// 残留，内容不同属滞留旧版本（滞留只会在按名解析/PATH 中遮蔽升级后的
    /// 运行时，即 wecom-cli 0.1.9 案例）；旧版本目录保守保留（GC 留后续 PR）。
    /// 全程幂等。
    #[test]
    fn migrate_legacy_binary_moves_match_and_reaps_stale_leftover() {
        with_temp_home("pinvou3-native-installer-test", || {
            let Some(bin_dir) = managed_connector_bin_dir_or_assert_unsupported() else {
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
            let sha = crate::platform::hashing::sha256_file(&legacy).unwrap();
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

            // Version dir populated + content mismatch (a stale old version —
            // the wecom-cli 0.1.9 shadow case) → remove: the pinned copy is the
            // verified authoritative runtime, and a legacy leftover would only
            // shadow it. The emptied bin dir is tidied.
            fs::create_dir_all(&bin_dir).unwrap();
            fs::write(&legacy, b"stale-old-version").unwrap();
            migrate_legacy_binary("test-cli", "9.9.9", &sha);
            assert!(
                !legacy.exists(),
                "stale old-version leftover must be removed once the pinned version is in place (it would shadow the upgraded CLI)"
            );
            assert!(dest.is_file(), "versioned pinned copy must be untouched");
            assert!(!bin_dir.exists(), "emptied bin dir must be tidied");
        });
    }

    /// While the pinned version is absent (no version dir), a legacy-layout
    /// binary must be kept even on hash mismatch: it is the connector's only
    /// locally runnable copy (e.g. a lark-cli not yet reconnected after an
    /// upgrade).
    #[test]
    fn mismatched_legacy_binary_kept_without_verified_pinned_copy() {
        with_temp_home("pinvou3-native-installer-test-keep", || {
            let Some(bin_dir) = managed_connector_bin_dir_or_assert_unsupported() else {
                return;
            };
            let exe = crate::platform::connector_lock::executable_name("test-cli");
            let legacy = bin_dir.join(&exe);
            let dest = crate::platform::paths::assets_cli_dir("test-cli", "9.9.9").join(&exe);

            fs::create_dir_all(&bin_dir).unwrap();
            fs::write(&legacy, b"only-local-runtime").unwrap();
            let legacy_sha = crate::platform::hashing::sha256_file(&legacy).unwrap();

            // Hash mismatch + pinned version absent → keep the legacy file
            migrate_legacy_binary("test-cli", "9.9.9", "deadbeef");
            assert!(
                legacy.is_file(),
                "with the pinned version absent, the legacy binary is the only local runtime and must not be removed"
            );
            assert!(!dest.exists());

            // Same file, matching hash → the move semantics are unaffected by
            // this change
            migrate_legacy_binary("test-cli", "9.9.9", &legacy_sha);
            assert!(
                dest.is_file(),
                "a match must still move into the version dir"
            );
            assert!(!legacy.exists());
        });
    }
}

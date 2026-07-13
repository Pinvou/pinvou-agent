use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::time::timeout;
use zip::ZipArchive;

use super::windows_domain_bootstrap;
use crate::bridge::paths;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowsInstallerKind {
    Msi,
    NsisExe,
}

const DEFAULT_SOFTWARE_ID: &str = "Pinvou3_Win";
const DEFAULT_SOFTWARE_TYPE: &str = "Pinvou3";
const OTA_SOFTWARE_ID_ENV: &str = "PINVOU3_OTA_SOFTWARE_ID";
const CHECK_UPDATE_PATH: &str = "/ota/pkg/package/upgrade/check";
const DOWNLOAD_INFO_PATH: &str = "/ota/pkg/package/upgrade/getDownloadInfo";
const UPDATE_LOG_PATH: &str = "/ota/pkg/package/updateLog";
const NO_AVAILABLE_UPDATE_CODE: i64 = 405000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowsUpdateInfo {
    pub available: bool,
    pub current_version: String,
    pub latest_version: String,
    pub notes: String,
    pub url: String,
    pub package_md5: String,
    pub size: u64,
    pub software_id: String,
    pub sn: String,
    pub update_type: String,
    pub ota_host: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowsPreparedUpdate {
    pub package_path: String,
    pub installer_path: String,
    pub latest_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowsInstallContext {
    pub software_id: String,
    pub sn: String,
    pub ota_host: String,
    pub current_version: String,
    pub update_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingUpdateReportResult {
    pub had_pending: bool,
    pub reported: bool,
    pub result: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateFeedbackRecord {
    pub software_identification: String,
    pub sn: String,
    pub current_version: String,
    pub update_version: String,
    pub update_result: String,
    pub update_error_info: String,
    pub installer_path: String,
    #[serde(default)]
    pub ota_host: String,
    pub created_at: DateTime<Utc>,
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub attempts: u32,
    pub reported: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CheckUpdateVersionRequest {
    sn: String,
    software_id: String,
    version: String,
    hardware_info: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct OtaResponse<T> {
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    code: i64,
    #[serde(default, rename = "msg")]
    message: String,
    data: Option<T>,
}

impl<T> OtaResponse<T> {
    fn is_success(&self) -> bool {
        if self.code != 200 {
            return false;
        }
        self.success.unwrap_or(true) || self.message.contains("操作成功")
    }

    fn is_no_available_update(&self) -> bool {
        self.code == NO_AVAILABLE_UPDATE_CODE
    }

    fn into_data(self, context: &str) -> Result<T, String> {
        if self.is_success() {
            return self
                .data
                .ok_or_else(|| format!("{context}失败：响应缺少 data"));
        }
        Err(format!(
            "{context}失败：code={} msg={}",
            self.code, self.message
        ))
    }
}

#[derive(Debug, Clone, Deserialize)]
struct UpgradeData {
    #[serde(default, rename = "updateInfo")]
    update_info: String,
    #[serde(default, rename = "updateType")]
    update_type: i64,
    #[serde(default, rename = "updateVersion")]
    update_version: String,
    #[serde(default, rename = "pkgMd5")]
    package_md5: String,
    #[serde(default, rename = "pkgUrl")]
    package_url: String,
}

#[derive(Debug, Deserialize)]
struct OtaPackageInfo {
    #[serde(default, rename = "softwareInfos")]
    software_infos: Vec<PackageSoftwareInfo>,
}

#[derive(Debug, Deserialize)]
struct PackageSoftwareInfo {
    #[serde(default, rename = "softwareId")]
    software_id: String,
    #[serde(default, rename = "softwareVersion")]
    software_version: String,
    #[serde(default, rename = "softwareType")]
    software_type: String,
    #[serde(default, rename = "sourceDir")]
    source_dir: String,
    #[serde(default, rename = "fileMetaInfos")]
    file_meta_infos: Vec<FileMetaInfo>,
    #[serde(default, rename = "attachData")]
    attach_data: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct FileMetaInfo {
    #[serde(default, rename = "fileName")]
    file_name: String,
    #[serde(default, rename = "filePath")]
    file_path: String,
    #[serde(default)]
    hash: String,
    #[serde(default, rename = "ignoreHash")]
    ignore_hash: bool,
}

#[derive(Debug, Deserialize)]
struct AttachData {
    #[serde(default, rename = "exeName")]
    exe_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateOtaLogRequest {
    #[serde(rename = "softwareIdentification")]
    software_identification: String,
    sn: String,
    current_version: String,
    update_version: String,
    update_error_info: String,
    update_result: String,
}

pub fn check_update_platform_support() -> Result<(), String> {
    Ok(())
}

pub async fn check_for_update_info(
    client: &reqwest::Client,
    current_version: &str,
) -> Result<crate::updater::UpdateInfo, String> {
    let info = check_for_update(client, current_version).await?;
    Ok(crate::updater::UpdateInfo {
        available: info.available,
        current_version: info.current_version,
        latest_version: info.latest_version,
        notes: info.notes,
        pub_date: String::new(),
        url: info.url,
        sha256: String::new(),
        size: info.size,
        package_md5: info.package_md5,
        software_id: info.software_id,
        sn: info.sn,
        update_type: info.update_type,
        platform: "windows".to_string(),
        ota_host: info.ota_host,
    })
}

pub async fn check_for_update(
    client: &reqwest::Client,
    current_version: &str,
) -> Result<WindowsUpdateInfo, String> {
    let config = OtaConfig::from_bootstrap(client, current_version).await?;
    let req = config.check_request();
    let response: OtaResponse<UpgradeData> =
        post_json(client, &config.endpoint(CHECK_UPDATE_PATH), &req).await?;
    if response.is_no_available_update() {
        return Ok(WindowsUpdateInfo {
            available: false,
            current_version: current_version.to_string(),
            latest_version: current_version.to_string(),
            notes: String::new(),
            url: String::new(),
            package_md5: String::new(),
            size: 0,
            software_id: config.software_id,
            sn: config.sn,
            update_type: String::new(),
            ota_host: config.host,
        });
    }
    let mut data: UpgradeData = response.into_data("获取升级信息")?;

    if data.update_version.trim().is_empty()
        || !is_newer_version(&data.update_version, current_version)
    {
        return Ok(WindowsUpdateInfo {
            available: false,
            current_version: current_version.to_string(),
            latest_version: current_version.to_string(),
            notes: data.update_info,
            url: String::new(),
            package_md5: String::new(),
            size: 0,
            software_id: config.software_id,
            sn: config.sn,
            update_type: update_type_name(data.update_type).to_string(),
            ota_host: config.host,
        });
    }

    if data.package_url.trim().is_empty() {
        let download: UpgradeData = post_json(client, &config.endpoint(DOWNLOAD_INFO_PATH), &req)
            .await?
            .into_data("获取下载信息")?;
        data.package_url = download.package_url;
        if data.package_md5.trim().is_empty() {
            data.package_md5 = download.package_md5;
        }
    }

    if data.package_url.trim().is_empty() {
        return Err("获取下载信息失败：完整包下载地址为空".to_string());
    }

    Ok(WindowsUpdateInfo {
        available: true,
        current_version: current_version.to_string(),
        latest_version: data.update_version,
        notes: data.update_info,
        url: data.package_url,
        package_md5: data.package_md5.to_ascii_lowercase(),
        size: 0,
        software_id: config.software_id,
        sn: config.sn,
        update_type: update_type_name(data.update_type).to_string(),
        ota_host: config.host,
    })
}

pub async fn download_update_package(
    info: &crate::updater::UpdateInfo,
    app: AppHandle,
    cancel: &AtomicBool,
    stall_timeout: Duration,
) -> Result<crate::updater::DownloadUpdateResult, String> {
    check_update_platform_support()?;
    let dir = paths::updates_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建下载目录失败: {e}"))?;
    let dest = dir.join(format!("pinvou3_{}_windows.zip", info.latest_version));
    let expected = info.package_md5.to_ascii_lowercase();

    if dest.exists()
        && !expected.is_empty()
        && file_md5(&dest).is_ok_and(|actual| actual.eq_ignore_ascii_case(&expected))
    {
        let ctx = install_context_from_update_info(info);
        let prepared = prepare_update_package(&dest, &ctx)?;
        return Ok(crate::updater::DownloadUpdateResult::Prepared(
            prepared_update_for_tauri(prepared),
        ));
    }

    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            if e.path().extension().is_some_and(|x| x == "zip") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP client 构建失败: {e}"))?;
    let mut resp = client
        .get(&info.url)
        .send()
        .await
        .map_err(|e| format!("下载请求失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("下载响应异常: {e}"))?;

    let total = if info.size > 0 {
        info.size
    } else {
        resp.content_length().unwrap_or(0)
    };
    let mut file = std::fs::File::create(&dest).map_err(|e| format!("创建文件失败: {e}"))?;
    let mut hasher = md5::Context::new();
    let mut downloaded: u64 = 0;
    let mut last_emit: u64 = 0;
    cancel.store(false, Ordering::SeqCst);
    loop {
        if cancel.load(Ordering::SeqCst) {
            drop(file);
            let _ = std::fs::remove_file(&dest);
            return Err("已取消下载".to_string());
        }
        let chunk = match timeout(stall_timeout, resp.chunk()).await {
            Err(_) => {
                drop(file);
                let _ = std::fs::remove_file(&dest);
                return Err(format!(
                    "下载停滞：超过 {}s 无数据，已中断（网络异常或更新源无响应）",
                    stall_timeout.as_secs()
                ));
            }
            Ok(Err(e)) => return Err(format!("下载中断: {e}")),
            Ok(Ok(None)) => break,
            Ok(Ok(Some(c))) => c,
        };
        file.write_all(&chunk)
            .map_err(|e| format!("写盘失败: {e}"))?;
        hasher.consume(&chunk);
        downloaded += chunk.len() as u64;
        if downloaded - last_emit >= 262_144 || downloaded == total {
            last_emit = downloaded;
            let _ = app.emit(
                "update:progress",
                serde_json::json!({ "downloaded": downloaded, "total": total }),
            );
        }
    }
    drop(file);

    if !expected.is_empty() {
        let actual = format!("{:x}", hasher.compute());
        if actual != expected {
            let _ = std::fs::remove_file(&dest);
            return Err(format!(
                "MD5 校验失败（期望 {expected} 实际 {actual}），已删除下载文件"
            ));
        }
    }

    let ctx = install_context_from_update_info(info);
    let prepared = prepare_update_package(&dest, &ctx)?;
    Ok(crate::updater::DownloadUpdateResult::Prepared(
        prepared_update_for_tauri(prepared),
    ))
}

pub fn prepare_update_package(
    package_path: &Path,
    context: &WindowsInstallContext,
) -> Result<WindowsPreparedUpdate, String> {
    let updates_dir = ensure_updates_dir()?;
    let package_path = canonical_inside(package_path, &updates_dir, "更新包")?;
    if package_path
        .extension()
        .is_none_or(|x| !x.eq_ignore_ascii_case("zip"))
    {
        return Err("更新包格式不正确：只接受 .zip 文件".to_string());
    }

    let version_dir = updates_dir.join(safe_version_dir(&context.update_version));
    reset_child_dir(&version_dir, &updates_dir)?;
    let full_dir = version_dir.join("full");
    std::fs::create_dir_all(&full_dir).map_err(|e| format!("创建完整包解压目录失败: {e}"))?;

    safe_extract_zip(&package_path, &full_dir)?;
    let ota_info_path = find_ota_info(&full_dir)?;
    let ota_info: OtaPackageInfo = read_json_file(&ota_info_path, "OtaInfo.json")?;
    let installer = locate_installer(&ota_info, &full_dir, context)?;

    Ok(WindowsPreparedUpdate {
        package_path: package_path.to_string_lossy().into_owned(),
        installer_path: windows_tool_path(&installer).to_string_lossy().into_owned(),
        latest_version: context.update_version.clone(),
    })
}

pub fn write_install_started_record(
    context: &WindowsInstallContext,
    installer_path: &Path,
) -> Result<(), String> {
    let updates_dir = ensure_updates_dir()?;
    let installer = canonical_inside(installer_path, &updates_dir, "Windows 安装文件")?;
    installer_kind(&installer)?;
    let record = UpdateFeedbackRecord {
        software_identification: context.software_id.clone(),
        sn: context.sn.clone(),
        current_version: context.current_version.clone(),
        update_version: context.update_version.clone(),
        update_result: "START_INSTALL".to_string(),
        update_error_info: String::new(),
        installer_path: windows_tool_path(&installer).to_string_lossy().into_owned(),
        ota_host: context.ota_host.clone(),
        created_at: Utc::now(),
        last_attempt_at: None,
        attempts: 0,
        reported: false,
    };
    let path = paths::update_feedback_record_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建反馈目录失败: {e}"))?;
    }
    let bytes =
        serde_json::to_vec_pretty(&record).map_err(|e| format!("反馈记录序列化失败: {e}"))?;
    std::fs::write(path, bytes).map_err(|e| format!("写入反馈记录失败: {e}"))
}

pub fn install_update_package(path: &Path) -> Result<(), String> {
    let updates_dir = ensure_updates_dir()?;
    let canon = canonical_inside(path, &updates_dir, "Windows 安装文件")?;
    let kind = installer_kind(&canon)?;
    let installer_arg = windows_tool_path(&canon);
    crate::process::HiddenCommand::new("powershell.exe")
        .args(update_installer_launcher_args(kind, &installer_arg))
        .spawn()
        .map_err(|e| format!("Windows 安装器提权启动失败: {e}"))?;
    Ok(())
}

pub fn install_prepared_update(
    installer_path: &str,
    info: &crate::updater::UpdateInfo,
) -> Result<(), String> {
    let ctx = install_context_from_update_info(info);
    let installer = Path::new(installer_path).to_path_buf();
    write_install_started_record(&ctx, &installer)?;
    install_update_package(&installer)
}

pub fn install_downloaded_update(
    _deb_path: Option<String>,
    installer_path: Option<String>,
    info: Option<crate::updater::UpdateInfo>,
) -> Result<bool, String> {
    let installer_path = installer_path.ok_or_else(|| "缺少 Windows 安装文件路径".to_string())?;
    let info = info.ok_or_else(|| "缺少 Windows 更新信息，无法写入反馈记录".to_string())?;
    install_prepared_update(&installer_path, &info)?;
    Ok(true)
}

pub async fn report_pending_update_result(
    client: &reqwest::Client,
    current_version: &str,
) -> Result<PendingUpdateReportResult, String> {
    let record_path = paths::update_feedback_record_path();
    if !record_path.exists() {
        return Ok(PendingUpdateReportResult {
            had_pending: false,
            reported: false,
            result: String::new(),
            message: "没有待反馈升级结果".to_string(),
        });
    }

    let mut record: UpdateFeedbackRecord = read_json_file(&record_path, "update-feedback.json")?;
    let config = OtaConfig::from_feedback_record(client, current_version, &record).await?;
    let result = if !record.update_version.is_empty()
        && version_at_least(current_version, &record.update_version)
    {
        "UPGRADE_SUCCEED"
    } else {
        "UNKNOWN"
    };
    record.update_result = result.to_string();
    if result == "UNKNOWN" && record.update_error_info.is_empty() {
        record.update_error_info = format!(
            "当前版本 {} 未达到目标版本 {}，升级结果无法确认",
            current_version, record.update_version
        );
    }
    record.attempts = record.attempts.saturating_add(1);
    record.last_attempt_at = Some(Utc::now());

    let req = UpdateOtaLogRequest {
        software_identification: record.software_identification.clone(),
        sn: record.sn.clone(),
        current_version: record.current_version.clone(),
        update_version: record.update_version.clone(),
        update_error_info: record.update_error_info.clone(),
        update_result: record.update_result.clone(),
    };

    let response: OtaResponse<serde_json::Value> =
        post_json(client, &config.endpoint(UPDATE_LOG_PATH), &req).await?;
    if response.is_success() {
        let _ = std::fs::remove_file(&record_path);
        return Ok(PendingUpdateReportResult {
            had_pending: true,
            reported: true,
            result: result.to_string(),
            message: "更新结果反馈成功".to_string(),
        });
    }

    let bytes =
        serde_json::to_vec_pretty(&record).map_err(|e| format!("反馈记录序列化失败: {e}"))?;
    std::fs::write(&record_path, bytes).map_err(|e| format!("保存反馈重试状态失败: {e}"))?;
    Err(format!(
        "更新结果反馈失败：code={} msg={}",
        response.code, response.message
    ))
}

pub async fn report_pending_update_result_info(
    client: &reqwest::Client,
    current_version: &str,
) -> Result<crate::updater::PendingUpdateReportResult, String> {
    let r = report_pending_update_result(client, current_version).await?;
    Ok(crate::updater::PendingUpdateReportResult {
        had_pending: r.had_pending,
        reported: r.reported,
        result: r.result,
        message: r.message,
    })
}

fn install_context_from_update_info(info: &crate::updater::UpdateInfo) -> WindowsInstallContext {
    WindowsInstallContext {
        software_id: if info.software_id.trim().is_empty() {
            DEFAULT_SOFTWARE_ID.to_string()
        } else {
            info.software_id.clone()
        },
        sn: info.sn.clone(),
        ota_host: info.ota_host.clone(),
        current_version: info.current_version.clone(),
        update_version: info.latest_version.clone(),
    }
}

fn prepared_update_for_tauri(value: WindowsPreparedUpdate) -> crate::updater::PreparedUpdate {
    crate::updater::PreparedUpdate {
        package_path: value.package_path,
        installer_path: value.installer_path,
        latest_version: value.latest_version,
    }
}

pub fn is_newer_version(latest: &str, current: &str) -> bool {
    compare_versions(latest, current).is_some_and(|ord| ord == std::cmp::Ordering::Greater)
}

fn version_at_least(current: &str, target: &str) -> bool {
    compare_versions(current, target)
        .is_some_and(|ord| ord == std::cmp::Ordering::Greater || ord == std::cmp::Ordering::Equal)
}

fn compare_versions(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    let av = parse_version_parts(a)?;
    let bv = parse_version_parts(b)?;
    let len = av.len().max(bv.len());
    for i in 0..len {
        let left = av.get(i).copied().unwrap_or(0);
        let right = bv.get(i).copied().unwrap_or(0);
        match left.cmp(&right) {
            std::cmp::Ordering::Equal => {}
            other => return Some(other),
        }
    }
    Some(std::cmp::Ordering::Equal)
}

fn parse_version_parts(v: &str) -> Option<Vec<u64>> {
    let trimmed = v.trim().trim_start_matches('v');
    if trimmed.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for part in trimmed.split('.') {
        out.push(part.parse().ok()?);
    }
    Some(out)
}

async fn post_json<T, B>(
    client: &reqwest::Client,
    url: &str,
    body: &B,
) -> Result<OtaResponse<T>, String>
where
    T: for<'de> Deserialize<'de>,
    B: Serialize + ?Sized,
{
    client
        .post(url)
        .json(body)
        .send()
        .await
        .map_err(|e| format!("OTA 请求失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("OTA 响应异常: {e}"))?
        .json::<OtaResponse<T>>()
        .await
        .map_err(|e| format!("OTA 响应解析失败: {e}"))
}

#[derive(Debug, Clone)]
struct OtaConfig {
    host: String,
    sn: String,
    software_id: String,
    current_version: String,
}

impl OtaConfig {
    async fn from_bootstrap(
        client: &reqwest::Client,
        current_version: &str,
    ) -> Result<Self, String> {
        let resolution = windows_domain_bootstrap::resolve_ota_host(client).await?;
        Self::from_parts(current_version, &resolution.ota_host, &resolution.sn)
    }

    async fn from_feedback_record(
        client: &reqwest::Client,
        current_version: &str,
        record: &UpdateFeedbackRecord,
    ) -> Result<Self, String> {
        if !record.ota_host.trim().is_empty() {
            return Self::from_parts(current_version, &record.ota_host, &record.sn);
        }
        Self::from_bootstrap(client, current_version).await
    }

    fn from_parts(current_version: &str, host: &str, sn: &str) -> Result<Self, String> {
        let host = normalize_ota_host(host)
            .ok_or_else(|| "OTA 后台地址无效，无法执行更新流程".to_string())?;
        let sn = sn.trim();
        if sn.is_empty() {
            return Err("设备 BIOS SN 为空，无法执行更新流程".to_string());
        }
        let software_id = std::env::var(OTA_SOFTWARE_ID_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_SOFTWARE_ID.to_string());
        Ok(Self {
            host,
            sn: sn.to_string(),
            software_id,
            current_version: current_version.to_string(),
        })
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{}", self.host, path)
    }

    fn check_request(&self) -> CheckUpdateVersionRequest {
        CheckUpdateVersionRequest {
            sn: self.sn.clone(),
            software_id: self.software_id.clone(),
            version: self.current_version.clone(),
            hardware_info: None,
        }
    }
}

fn normalize_ota_host(value: &str) -> Option<String> {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let parsed = reqwest::Url::parse(trimmed).ok()?;
    if matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some() {
        Some(trimmed.to_string())
    } else {
        None
    }
}

fn update_type_name(value: i64) -> &'static str {
    match value {
        0 => "Silent",
        1 => "Force",
        2 => "Normal",
        _ => "Unknown",
    }
}

fn ensure_updates_dir() -> Result<PathBuf, String> {
    let dir = paths::updates_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建更新目录失败: {e}"))?;
    dir.canonicalize()
        .map_err(|e| format!("更新目录不可访问: {e}"))
}

fn reset_child_dir(dir: &Path, root: &Path) -> Result<(), String> {
    let parent = dir
        .parent()
        .ok_or_else(|| "更新解压目录无父目录".to_string())?;
    let parent = parent
        .canonicalize()
        .map_err(|e| format!("更新解压父目录不可访问: {e}"))?;
    if !parent.starts_with(root) {
        return Err("拒绝清理更新目录外的路径".to_string());
    }
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|e| format!("清理旧更新解压目录失败: {e}"))?;
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("创建更新解压目录失败: {e}"))
}

fn safe_version_dir(version: &str) -> String {
    let clean: String = version
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("pinvou3-{clean}")
}

fn read_json_file<T>(path: &Path, label: &str) -> Result<T, String>
where
    T: for<'de> Deserialize<'de>,
{
    let text = std::fs::read_to_string(path).map_err(|e| format!("读取 {label} 失败: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("解析 {label} 失败: {e}"))
}

fn safe_extract_zip(zip_path: &Path, dest: &Path) -> Result<(), String> {
    let file = File::open(zip_path).map_err(|e| format!("打开 zip 失败: {e}"))?;
    let mut archive = ZipArchive::new(file).map_err(|e| format!("读取 zip 失败: {e}"))?;
    let dest = dest
        .canonicalize()
        .map_err(|e| format!("解压目标目录不可访问: {e}"))?;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("读取 zip 条目失败: {e}"))?;
        let enclosed = entry
            .enclosed_name()
            .ok_or_else(|| format!("zip 条目路径不安全: {}", entry.name()))?
            .to_path_buf();
        let out = safe_join(&dest, &enclosed)?;
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| format!("创建解压目录失败: {e}"))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建解压父目录失败: {e}"))?;
        }
        let mut outfile = File::create(&out).map_err(|e| format!("创建解压文件失败: {e}"))?;
        io::copy(&mut entry, &mut outfile).map_err(|e| format!("写入解压文件失败: {e}"))?;
    }
    Ok(())
}

fn safe_relative_path(value: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(format!("路径不安全: {value}"));
    }
    for comp in path.components() {
        match comp {
            Component::Normal(_) => {}
            _ => return Err(format!("路径不安全: {value}")),
        }
    }
    Ok(path.to_path_buf())
}

fn safe_join(root: &Path, rel: &Path) -> Result<PathBuf, String> {
    if rel.is_absolute() {
        return Err("路径不安全：拒绝绝对路径".to_string());
    }
    for comp in rel.components() {
        match comp {
            Component::Normal(_) => {}
            _ => return Err("路径不安全：拒绝特殊路径组件".to_string()),
        }
    }
    Ok(root.join(rel))
}

fn canonical_inside(path: &Path, root: &Path, label: &str) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("{label} 根目录不可访问: {e}"))?;
    let canon = path
        .canonicalize()
        .map_err(|e| format!("{label} 不存在或不可访问: {e}"))?;
    if !canon.starts_with(&root) {
        return Err(format!("{label} 必须位于更新目录内"));
    }
    Ok(canon)
}

fn windows_tool_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        use std::path::Prefix;

        let mut components = path.components();
        if let Some(Component::Prefix(prefix)) = components.next() {
            match prefix.kind() {
                Prefix::VerbatimDisk(disk) => {
                    let mut out = PathBuf::from(format!("{}:\\", disk as char));
                    out.extend(components);
                    return out;
                }
                Prefix::VerbatimUNC(server, share) => {
                    let mut out = PathBuf::from(format!(
                        "\\\\{}\\{}",
                        server.to_string_lossy(),
                        share.to_string_lossy()
                    ));
                    out.extend(components);
                    return out;
                }
                _ => {}
            }
        }
    }
    path.to_path_buf()
}

fn msi_install_args(installer: &Path) -> Vec<OsString> {
    vec![
        OsString::from("/i"),
        installer.as_os_str().to_os_string(),
        OsString::from("REINSTALLMODE=vamus"),
        OsString::from("/passive"),
        OsString::from("/norestart"),
    ]
}

fn nsis_install_args(_installer: &Path) -> Vec<OsString> {
    // `/P` 是项目 NSIS 模板提供的被动安装模式：保留可见的安装进度页，
    // 但跳过欢迎、目录、确认和完成页，自动开始并在结束后关闭。
    // `/UPDATE` 让安装器按升级语义处理已有安装和快捷方式。
    vec![OsString::from("/P"), OsString::from("/UPDATE")]
}

fn installer_kind(installer: &Path) -> Result<WindowsInstallerKind, String> {
    match installer
        .extension()
        .and_then(|x| x.to_str())
        .map(|x| x.to_ascii_lowercase())
        .as_deref()
    {
        Some("msi") => Ok(WindowsInstallerKind::Msi),
        Some("exe") => Ok(WindowsInstallerKind::NsisExe),
        _ => Err("非法路径：只接受 .msi 或 .exe Windows 安装文件".to_string()),
    }
}

fn installer_label(kind: WindowsInstallerKind) -> &'static str {
    match kind {
        WindowsInstallerKind::Msi => "MSI 安装文件",
        WindowsInstallerKind::NsisExe => "NSIS 安装文件",
    }
}

fn is_supported_installer_name(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.ends_with(".msi") || lower.ends_with(".exe")
}

fn update_installer_launcher_args(kind: WindowsInstallerKind, installer: &Path) -> Vec<OsString> {
    let (file_path, install_args) = match kind {
        WindowsInstallerKind::Msi => (OsString::from("msiexec.exe"), msi_install_args(installer)),
        WindowsInstallerKind::NsisExe => (
            installer.as_os_str().to_os_string(),
            nsis_install_args(installer),
        ),
    };
    let file_path = powershell_single_quoted(&file_path.to_string_lossy());
    let argument_list = install_args
        .iter()
        .map(|arg| powershell_single_quoted(&arg.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(", ");
    let mut script = format!(
        "$ErrorActionPreference = 'Stop'; $p = Start-Process -FilePath {file_path} -ArgumentList @({argument_list}) -Verb RunAs -Wait -PassThru; "
    );
    script.push_str("$code = $p.ExitCode; if ($code -eq 0 -or $code -eq 3010) { ");
    script.push_str("$installDir = $null; ");
    script.push_str("try { $installDir = (Get-ItemProperty -Path 'HKCU:\\Software\\pinvou\\pinvou3' -Name 'InstallDir' -ErrorAction SilentlyContinue).InstallDir } catch {} ");
    script.push_str("if ([string]::IsNullOrWhiteSpace($installDir)) { $installDir = Join-Path $env:ProgramFiles 'pinvou3' } ");
    script.push_str("$exe = Join-Path $installDir 'pinvou3-tauri.exe'; ");
    script.push_str("if (Test-Path -LiteralPath $exe) { Start-Process -FilePath $exe } ");
    script.push('}');
    vec![
        OsString::from("-NoProfile"),
        OsString::from("-ExecutionPolicy"),
        OsString::from("Bypass"),
        OsString::from("-WindowStyle"),
        OsString::from("Hidden"),
        OsString::from("-Command"),
        OsString::from(script),
    ]
}

fn powershell_single_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn find_ota_info(full_dir: &Path) -> Result<PathBuf, String> {
    for rel in ["OtaInfo.json", "FullPack/OtaInfo.json"] {
        let path = full_dir.join(rel);
        if path.exists() {
            return canonical_inside(&path, full_dir, "OtaInfo.json");
        }
    }
    Err("完整包缺少 OtaInfo.json".to_string())
}

fn locate_installer(
    ota: &OtaPackageInfo,
    full_dir: &Path,
    context: &WindowsInstallContext,
) -> Result<PathBuf, String> {
    let software = ota
        .software_infos
        .iter()
        .find(|s| s.software_id == context.software_id)
        .or_else(|| {
            ota.software_infos
                .iter()
                .find(|s| s.software_type == DEFAULT_SOFTWARE_TYPE)
        })
        .ok_or_else(|| "OtaInfo.json 中未找到 Pinvou3 软件信息".to_string())?;

    let meta = software
        .file_meta_infos
        .iter()
        .find(|m| is_supported_installer_name(&m.file_path))
        .or_else(|| {
            software
                .file_meta_infos
                .iter()
                .find(|m| is_supported_installer_name(&m.file_name))
        })
        .cloned()
        .or_else(|| {
            attach_data_installer(software).map(|name| FileMetaInfo {
                file_name: name.clone(),
                file_path: name,
                hash: String::new(),
                ignore_hash: true,
            })
        })
        .ok_or_else(|| "OtaInfo.json 未声明 Windows 安装文件（.msi 或 .exe）".to_string())?;

    if !software.software_version.is_empty() && software.software_version != context.update_version
    {
        return Err(format!(
            "Windows 安装包软件版本不匹配：期望 {} 实际 {}",
            context.update_version, software.software_version
        ));
    }

    let source_dir = software.source_dir.trim();
    let file_path = if !meta.file_path.trim().is_empty() {
        meta.file_path.trim()
    } else {
        meta.file_name.trim()
    };
    let rel = safe_relative_path(file_path)?;
    let candidates = [
        full_dir.join("Files").join(source_dir).join(&rel),
        full_dir
            .join("Files")
            .join(source_dir)
            .join(&meta.file_name),
        full_dir.join(&rel),
    ];
    let installer = candidates
        .iter()
        .find(|p| p.exists())
        .ok_or_else(|| "清单指向的 Windows 安装文件不存在".to_string())?;
    let installer = canonical_inside(installer, full_dir, "Windows 安装文件")?;
    let kind = installer_kind(&installer)
        .map_err(|_| "清单指向的安装文件不是 .msi 或 .exe".to_string())?;
    if !meta.ignore_hash && !meta.hash.trim().is_empty() {
        verify_md5(&installer, &meta.hash, installer_label(kind))?;
    }
    Ok(installer)
}

fn attach_data_installer(software: &PackageSoftwareInfo) -> Option<String> {
    let raw = software.attach_data.as_ref()?.trim();
    if raw.is_empty() {
        return None;
    }
    let parsed: AttachData = serde_json::from_str(raw).ok()?;
    if is_supported_installer_name(&parsed.exe_name) {
        Some(parsed.exe_name)
    } else {
        None
    }
}

fn verify_md5(path: &Path, expected: &str, label: &str) -> Result<(), String> {
    let actual = file_md5(path)?;
    if actual.eq_ignore_ascii_case(expected.trim()) {
        Ok(())
    } else {
        Err(format!(
            "{label} MD5 校验失败（期望 {} 实际 {}）",
            expected, actual
        ))
    }
}

fn file_md5(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读取文件校验失败: {e}"))?;
    Ok(format!("{:x}", md5::compute(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-windows-update-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let bytes = zip_bytes(entries);
        std::fs::write(path, bytes).unwrap();
    }

    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let opt = SimpleFileOptions::default();
        for (name, bytes) in entries {
            writer.start_file(name, opt).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn context() -> WindowsInstallContext {
        WindowsInstallContext {
            software_id: DEFAULT_SOFTWARE_ID.to_string(),
            sn: "SN001".to_string(),
            ota_host: "https://ota.example.com".to_string(),
            current_version: "0.4.3".to_string(),
            update_version: "0.4.4.0".to_string(),
        }
    }

    #[test]
    fn four_part_version_compare() {
        assert!(is_newer_version("0.4.4.0", "0.4.3"));
        assert!(is_newer_version("0.10.0", "0.2.0"));
        assert!(!is_newer_version("0.4.4.0", "0.4.4.0"));
        assert!(!is_newer_version("0.4.2", "0.4.3"));
        assert!(!is_newer_version("bad", "0.4.3"));
    }

    #[test]
    fn check_response_parse_success_and_failure() {
        let ok: OtaResponse<UpgradeData> = serde_json::from_str(
            r#"{"success":true,"code":200,"msg":"OK","data":{"updateInfo":"n","updateType":2,"updateVersion":"0.4.4.0","pkgMd5":"ABC"}}"#,
        )
        .unwrap();
        let data = ok.into_data("check").unwrap();
        assert_eq!(data.update_version, "0.4.4.0");
        assert_eq!(data.package_md5, "ABC");

        let ok_without_success: OtaResponse<UpgradeData> = serde_json::from_str(
            r#"{"code":200,"msg":"操作成功","data":{"updateInfo":"n","updateType":2,"updateVersion":"0.4.4.0","pkgMd5":"ABC"}}"#,
        )
        .unwrap();
        assert!(ok_without_success.into_data("check").is_ok());

        let fail: OtaResponse<UpgradeData> =
            serde_json::from_str(r#"{"success":false,"code":500,"msg":"no"}"#).unwrap();
        assert!(fail.into_data("check").is_err());

        let no_update: OtaResponse<UpgradeData> =
            serde_json::from_str(r#"{"success":false,"code":405000,"msg":"无可用软件升级版本"}"#)
                .unwrap();
        assert!(no_update.is_no_available_update());
    }

    #[test]
    fn ota_config_endpoints_use_resolved_host() {
        let config = OtaConfig::from_parts("0.4.3", "http://127.0.0.1:8787/", "SN001").unwrap();
        assert_eq!(
            config.endpoint(CHECK_UPDATE_PATH),
            "http://127.0.0.1:8787/ota/pkg/package/upgrade/check"
        );
        assert_eq!(
            config.endpoint(DOWNLOAD_INFO_PATH),
            "http://127.0.0.1:8787/ota/pkg/package/upgrade/getDownloadInfo"
        );
        assert_eq!(
            config.endpoint(UPDATE_LOG_PATH),
            "http://127.0.0.1:8787/ota/pkg/package/updateLog"
        );
        assert_eq!(config.check_request().sn, "SN001");
        assert!(OtaConfig::from_parts("0.4.3", "not-a-url", "SN001").is_err());
        assert!(OtaConfig::from_parts("0.4.3", "http://127.0.0.1:8787", " ").is_err());
    }

    #[test]
    fn ota_info_accepts_root_and_fullpack_paths() {
        let root = temp_root("ota-path");
        std::fs::write(root.join("OtaInfo.json"), "{}").unwrap();
        assert_eq!(
            find_ota_info(&root).unwrap(),
            root.join("OtaInfo.json").canonicalize().unwrap()
        );
        std::fs::remove_file(root.join("OtaInfo.json")).unwrap();
        std::fs::create_dir_all(root.join("FullPack")).unwrap();
        std::fs::write(root.join("FullPack/OtaInfo.json"), "{}").unwrap();
        assert_eq!(
            find_ota_info(&root).unwrap(),
            root.join("FullPack/OtaInfo.json").canonicalize().unwrap()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn locate_msi_from_software_id_and_file_meta() {
        let root = temp_root("locate-msi");
        let msi_dir = root.join("Files/Pinvou3");
        std::fs::create_dir_all(&msi_dir).unwrap();
        let msi = msi_dir.join("pinvou3.msi");
        std::fs::write(&msi, b"fake-msi").unwrap();
        let hash = file_md5(&msi).unwrap();
        let ota = OtaPackageInfo {
            software_infos: vec![PackageSoftwareInfo {
                software_id: DEFAULT_SOFTWARE_ID.to_string(),
                software_version: "0.4.4.0".to_string(),
                software_type: DEFAULT_SOFTWARE_TYPE.to_string(),
                source_dir: "Pinvou3".to_string(),
                file_meta_infos: vec![FileMetaInfo {
                    file_name: "pinvou3.msi".to_string(),
                    file_path: "pinvou3.msi".to_string(),
                    hash,
                    ignore_hash: false,
                }],
                attach_data: None,
            }],
        };
        assert_eq!(
            locate_installer(&ota, &root, &context()).unwrap(),
            msi.canonicalize().unwrap()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn locate_nsis_exe_from_software_id_and_file_meta() {
        let root = temp_root("locate-nsis");
        let exe_dir = root.join("Files/Pinvou3");
        std::fs::create_dir_all(&exe_dir).unwrap();
        let exe = exe_dir.join("pinvou3_0.4.4_x64-setup.exe");
        std::fs::write(&exe, b"fake-nsis").unwrap();
        let hash = file_md5(&exe).unwrap();
        let ota = OtaPackageInfo {
            software_infos: vec![PackageSoftwareInfo {
                software_id: DEFAULT_SOFTWARE_ID.to_string(),
                software_version: "0.4.4.0".to_string(),
                software_type: DEFAULT_SOFTWARE_TYPE.to_string(),
                source_dir: "Pinvou3".to_string(),
                file_meta_infos: vec![FileMetaInfo {
                    file_name: "pinvou3_0.4.4_x64-setup.exe".to_string(),
                    file_path: "pinvou3_0.4.4_x64-setup.exe".to_string(),
                    hash,
                    ignore_hash: false,
                }],
                attach_data: None,
            }],
        };
        assert_eq!(
            locate_installer(&ota, &root, &context()).unwrap(),
            exe.canonicalize().unwrap()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn locate_msi_rejects_traversal_and_wrong_extension() {
        let root = temp_root("reject-msi");
        let ota = OtaPackageInfo {
            software_infos: vec![PackageSoftwareInfo {
                software_id: DEFAULT_SOFTWARE_ID.to_string(),
                software_version: "0.4.4.0".to_string(),
                software_type: DEFAULT_SOFTWARE_TYPE.to_string(),
                source_dir: "Pinvou3".to_string(),
                file_meta_infos: vec![FileMetaInfo {
                    file_name: "evil.txt".to_string(),
                    file_path: "../evil.msi".to_string(),
                    hash: String::new(),
                    ignore_hash: true,
                }],
                attach_data: None,
            }],
        };
        assert!(locate_installer(&ota, &root, &context()).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn safe_extract_rejects_traversal_zip_entry() {
        let root = temp_root("zip-traversal");
        let zip_path = root.join("bad.zip");
        write_zip(&zip_path, &[("../evil.txt", b"bad")]);
        let out = root.join("out");
        std::fs::create_dir_all(&out).unwrap();
        assert!(safe_extract_zip(&zip_path, &out).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn feedback_record_roundtrip() {
        let record = UpdateFeedbackRecord {
            software_identification: DEFAULT_SOFTWARE_ID.to_string(),
            sn: "SN001".to_string(),
            current_version: "0.4.3".to_string(),
            update_version: "0.4.4.0".to_string(),
            update_result: "START_INSTALL".to_string(),
            update_error_info: String::new(),
            installer_path: "C:/x/pinvou3.msi".to_string(),
            ota_host: "https://ota.example.com".to_string(),
            created_at: Utc::now(),
            last_attempt_at: None,
            attempts: 0,
            reported: false,
        };
        let json = serde_json::to_string(&record).unwrap();
        let back: UpdateFeedbackRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.update_result, "START_INSTALL");
        assert_eq!(back.ota_host, "https://ota.example.com");
        assert!(!back.reported);

        let old_json = r#"{
            "software_identification":"Pinvou3_Win",
            "sn":"SN001",
            "current_version":"0.4.3",
            "update_version":"0.4.4.0",
            "update_result":"START_INSTALL",
            "update_error_info":"",
            "installer_path":"C:/x/pinvou3.msi",
            "created_at":"2026-06-17T00:00:00Z",
            "last_attempt_at":null,
            "attempts":0,
            "reported":false
        }"#;
        let old: UpdateFeedbackRecord = serde_json::from_str(old_json).unwrap();
        assert!(old.ota_host.is_empty());
    }

    #[test]
    fn update_log_request_uses_h3c_field_names() {
        let req = UpdateOtaLogRequest {
            software_identification: DEFAULT_SOFTWARE_ID.to_string(),
            sn: "SN001".to_string(),
            current_version: "0.4.3".to_string(),
            update_version: "0.4.4.0".to_string(),
            update_error_info: String::new(),
            update_result: "UPGRADE_SUCCEED".to_string(),
        };
        let v = serde_json::to_value(req).unwrap();
        assert_eq!(v["softwareIdentification"], DEFAULT_SOFTWARE_ID);
        assert_eq!(v["currentVersion"], "0.4.3");
        assert_eq!(v["updateResult"], "UPGRADE_SUCCEED");
    }

    #[test]
    fn windows_tool_path_strips_verbatim_disk_prefix() {
        let path = Path::new(r"\\?\C:\Users\z27014\.pinvou3\updates\pinvou3.msi");
        assert_eq!(
            windows_tool_path(path).to_string_lossy(),
            r"C:\Users\z27014\.pinvou3\updates\pinvou3.msi"
        );
    }

    #[test]
    fn msi_install_args_install_without_reinstall_all() {
        let args: Vec<String> = msi_install_args(Path::new(r"C:\pinvou3.msi"))
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args[0], "/i");
        assert_eq!(args[1], r"C:\pinvou3.msi");
        assert!(!args.contains(&"REINSTALL=ALL".to_string()));
        assert!(args.contains(&"REINSTALLMODE=vamus".to_string()));
        assert!(!args.contains(&"AUTOLAUNCHAPP=1".to_string()));
        assert!(!args.contains(&"WIXUI_EXITDIALOGOPTIONALCHECKBOX=1".to_string()));
        assert!(!args.contains(&"LAUNCHAPPARGS=".to_string()));
        assert!(args.contains(&"/passive".to_string()));
        assert!(args.contains(&"/norestart".to_string()));
    }

    #[test]
    fn nsis_install_args_use_visible_passive_update_mode() {
        let args: Vec<String> = nsis_install_args(Path::new(r"C:\pinvou3-setup.exe"))
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["/P", "/UPDATE"]);
        assert!(!args.contains(&"/S".to_string()));
    }

    #[test]
    fn nsis_template_keeps_visible_passive_mode_contract() {
        let template = include_str!("../../../resources/windows/nsis/installer-template.nsi");
        assert!(template.contains("${GetOptions} $CMDLINE \"/P\" $PassiveMode"));
        assert!(template.contains("!insertmacro MUI_PAGE_INSTFILES"));
        assert!(template.contains("SetAutoClose true"));
    }

    #[test]
    fn update_installer_launcher_uses_runas_wait_and_quotes_args() {
        let launcher_args = update_installer_launcher_args(
            WindowsInstallerKind::Msi,
            Path::new(r"C:\updates\pinvou3's.msi"),
        );
        let rendered = launcher_args
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(rendered.contains("-WindowStyle Hidden"));
        assert!(rendered.contains("-Verb RunAs"));
        assert!(rendered.contains("-Wait"));
        assert!(rendered.contains("msiexec.exe"));
        assert!(rendered.contains("'C:\\updates\\pinvou3''s.msi'"));
        assert!(!rendered.contains("'REINSTALL=ALL'"));
        assert!(rendered.contains("'/passive'"));
        assert!(rendered.contains("HKCU:\\Software\\pinvou\\pinvou3"));
        assert!(rendered.contains("pinvou3-tauri.exe"));
    }

    #[test]
    fn update_installer_launcher_supports_nsis_exe() {
        let launcher_args = update_installer_launcher_args(
            WindowsInstallerKind::NsisExe,
            Path::new(r"C:\updates\pinvou3's-setup.exe"),
        );
        let rendered = launcher_args
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(rendered.contains("-WindowStyle Hidden"));
        assert!(rendered.contains("-Verb RunAs"));
        assert!(rendered.contains("-Wait"));
        assert!(rendered.contains("Start-Process -FilePath 'C:\\updates\\pinvou3''s-setup.exe'"));
        assert!(rendered.contains("'/P'"));
        assert!(rendered.contains("'/UPDATE'"));
        assert!(!rendered.contains("'/S'"));
        assert!(!rendered.contains("msiexec.exe"));
        assert!(rendered.contains("pinvou3-tauri.exe"));
    }

    #[test]
    fn prepare_update_package_accepts_direct_full_pack_zip() {
        let _g = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let root = temp_root("direct-full-pack");
        std::env::set_var("PINVOU3_HOME", &root);
        let updates = paths::updates_dir();
        std::fs::create_dir_all(&updates).unwrap();

        let msi_name = "pinvou3_0.4.4_x64_en-US.msi";
        let msi_bytes = b"fake-msi";
        let msi_hash = format!("{:x}", md5::compute(msi_bytes));
        let ota = format!(
            r#"{{
                "softwareInfos": [{{
                    "softwareId": "Pinvou3_Win",
                    "softwareVersion": "0.4.4.0",
                    "softwareType": "Pinvou3",
                    "sourceDir": "Pinvou3",
                    "fileMetaInfos": [{{
                        "fileName": "{msi_name}",
                        "filePath": "{msi_name}",
                        "hash": "{msi_hash}",
                        "ignoreHash": false
                    }}]
                }}]
            }}"#
        );
        let package = updates.join("pinvou3_0.4.4.0_windows.zip");
        write_zip(
            &package,
            &[
                ("OtaInfo.json", ota.as_bytes()),
                (&format!("Files/Pinvou3/{msi_name}"), msi_bytes),
            ],
        );

        let prepared = prepare_update_package(&package, &context()).unwrap();
        assert!(prepared.installer_path.ends_with(msi_name));

        let _ = std::fs::remove_dir_all(&root);
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
    }

    #[test]
    fn prepare_update_package_accepts_nsis_full_pack_zip() {
        let _g = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let root = temp_root("nsis-full-pack");
        std::env::set_var("PINVOU3_HOME", &root);
        let updates = paths::updates_dir();
        std::fs::create_dir_all(&updates).unwrap();

        let exe_name = "pinvou3_0.4.4_x64-setup.exe";
        let exe_bytes = b"fake-nsis";
        let exe_hash = format!("{:x}", md5::compute(exe_bytes));
        let ota = format!(
            r#"{{
                "softwareInfos": [{{
                    "softwareId": "Pinvou3_Win",
                    "softwareVersion": "0.4.4.0",
                    "softwareType": "Pinvou3",
                    "sourceDir": "Pinvou3",
                    "fileMetaInfos": [{{
                        "fileName": "{exe_name}",
                        "filePath": "{exe_name}",
                        "hash": "{exe_hash}",
                        "ignoreHash": false
                    }}]
                }}]
            }}"#
        );
        let package = updates.join("pinvou3_0.4.4.0_windows_nsis.zip");
        write_zip(
            &package,
            &[
                ("OtaInfo.json", ota.as_bytes()),
                (&format!("Files/Pinvou3/{exe_name}"), exe_bytes),
            ],
        );

        let prepared = prepare_update_package(&package, &context()).unwrap();
        assert!(prepared.installer_path.ends_with(exe_name));

        let _ = std::fs::remove_dir_all(&root);
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
    }
}

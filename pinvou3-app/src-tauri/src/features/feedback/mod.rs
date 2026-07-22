use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fmt, fs,
    future::Future,
    io::{Read, Write},
    path::{Path, PathBuf},
    pin::Pin,
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use chrono::Utc;
use flate2::{write::GzEncoder, Compression};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tar::Builder as TarBuilder;

const MAX_TITLE_CHARS: usize = 120;
const MAX_DESCRIPTION_CHARS: usize = 5000;
const MAX_ATTACHMENTS: usize = 5;
const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_VIDEO_BYTES: u64 = 50 * 1024 * 1024;
const MAX_TOTAL_ATTACHMENT_BYTES: u64 = 80 * 1024 * 1024;
const TOKEN_URL: &str = "https://magic.h3c.com/rest/ihomers/uploadRequest";
const UPLOAD_URL: &str = "https://magic.h3c.com/rest/ihomers/uploadSysinfoFile";
const TEMP_FEEDBACK_GW_SN_OVERRIDE: Option<&str> = Some("219801A4BL522CM00002");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackType {
    Issue,
    Suggestion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackStatus {
    Submitted,
    FailedRetryable,
    FailedValidation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackSubmitRequest {
    #[serde(rename = "type")]
    pub feedback_type: FeedbackType,
    #[serde(default)]
    pub title: Option<String>,
    pub description: String,
    pub entry_point: String,
    #[serde(default)]
    pub error_summary: Option<String>,
    #[serde(default)]
    pub attachments: Vec<FeedbackAttachmentRequest>,
    pub privacy_notice_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackAttachmentRequest {
    pub path: String,
    pub name: String,
    pub media_type: String,
    #[serde(default)]
    pub mime: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackReceipt {
    pub feedback_id: String,
    pub status: FeedbackStatus,
    pub submitted_at: Option<String>,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug)]
pub enum FeedbackError {
    Validation(String),
    Io(std::io::Error),
    Upload(String),
    Http(reqwest::Error),
    Json(serde_json::Error),
}

impl fmt::Display for FeedbackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FeedbackError::Validation(msg) => write!(f, "{msg}"),
            FeedbackError::Io(e) => write!(f, "反馈文件处理失败：{e}"),
            FeedbackError::Upload(msg) => write!(f, "{msg}"),
            FeedbackError::Http(e) => write!(f, "反馈接收通道请求失败：{e}"),
            FeedbackError::Json(e) => write!(f, "反馈数据序列化失败：{e}"),
        }
    }
}

impl std::error::Error for FeedbackError {}

impl From<std::io::Error> for FeedbackError {
    fn from(value: std::io::Error) -> Self {
        FeedbackError::Io(value)
    }
}

impl From<reqwest::Error> for FeedbackError {
    fn from(value: reqwest::Error) -> Self {
        FeedbackError::Http(value)
    }
}

impl From<serde_json::Error> for FeedbackError {
    fn from(value: serde_json::Error) -> Self {
        FeedbackError::Json(value)
    }
}

#[derive(Debug)]
pub struct PreparedFeedbackPackage {
    pub feedback_id: String,
    pub package_dir: PathBuf,
    pub receipt_path: PathBuf,
    pub tar_gz_path: PathBuf,
    pub dbg_path: PathBuf,
}

pub trait FeedbackUploader: Send + Sync {
    fn upload<'a>(
        &'a self,
        package: &'a PreparedFeedbackPackage,
    ) -> Pin<Box<dyn Future<Output = Result<(), FeedbackError>> + Send + 'a>>;
}

pub struct H3cFeedbackUploader;

impl FeedbackUploader for H3cFeedbackUploader {
    fn upload<'a>(
        &'a self,
        package: &'a PreparedFeedbackPackage,
    ) -> Pin<Box<dyn Future<Output = Result<(), FeedbackError>> + Send + 'a>> {
        Box::pin(async move {
            let serial = resolve_device_serial().ok_or_else(|| {
                FeedbackError::Validation(
                    "当前设备缺少反馈上传通道所需标识，请配置 PINVOU3_FEEDBACK_GW_SN 后重试。"
                        .to_string(),
                )
            })?;
            create_tar_gz_archive(&package.package_dir, &package.tar_gz_path)?;
            xor_to_dbg(&package.tar_gz_path, &package.dbg_path)?;
            let token = request_upload_token(&serial).await?;
            let check_code = compute_check_code(&token, &serial);
            upload_dbg_file(&package.dbg_path, &serial, &check_code).await
        })
    }
}

pub async fn submit_feedback(
    request: FeedbackSubmitRequest,
) -> Result<FeedbackReceipt, FeedbackError> {
    submit_feedback_with_uploader(request, &H3cFeedbackUploader).await
}

pub async fn submit_feedback_with_uploader(
    request: FeedbackSubmitRequest,
    uploader: &dyn FeedbackUploader,
) -> Result<FeedbackReceipt, FeedbackError> {
    validate_feedback_request(&request)?;
    let package = prepare_feedback_package(&request)?;
    match uploader.upload(&package).await {
        Ok(()) => {
            cleanup_successful_package(&package)?;
            let receipt = FeedbackReceipt {
                feedback_id: package.feedback_id.clone(),
                status: FeedbackStatus::Submitted,
                submitted_at: Some(now_rfc3339()),
                message: "反馈已提交，感谢你的帮助。".to_string(),
                retryable: false,
            };
            write_receipt(&package.receipt_path, &receipt)?;
            Ok(receipt)
        }
        Err(FeedbackError::Validation(e)) => Err(FeedbackError::Validation(e)),
        Err(e) => Ok(FeedbackReceipt {
            feedback_id: package.feedback_id,
            status: FeedbackStatus::FailedRetryable,
            submitted_at: None,
            message: format!("当前无法连接反馈接收通道，请稍后重试。{e}"),
            retryable: true,
        }),
    }
}

pub fn validate_feedback_request(request: &FeedbackSubmitRequest) -> Result<(), FeedbackError> {
    let desc_len = request.description.trim().chars().count();
    if desc_len == 0 {
        return Err(FeedbackError::Validation("请填写反馈说明。".to_string()));
    }
    if desc_len > MAX_DESCRIPTION_CHARS {
        return Err(FeedbackError::Validation(format!(
            "反馈说明最多 {MAX_DESCRIPTION_CHARS} 个字符。"
        )));
    }
    if let Some(title) = &request.title {
        if title.chars().count() > MAX_TITLE_CHARS {
            return Err(FeedbackError::Validation(format!(
                "反馈标题最多 {MAX_TITLE_CHARS} 个字符。"
            )));
        }
    }
    if !matches!(request.entry_point.as_str(), "settings" | "error_banner") {
        return Err(FeedbackError::Validation("反馈入口来源无效。".to_string()));
    }
    if request.privacy_notice_version.trim().is_empty() {
        return Err(FeedbackError::Validation(
            "缺少反馈隐私提示版本。".to_string(),
        ));
    }
    validate_attachments(&request.attachments)?;
    Ok(())
}

pub fn validate_attachments(
    attachments: &[FeedbackAttachmentRequest],
) -> Result<(), FeedbackError> {
    if attachments.len() > MAX_ATTACHMENTS {
        return Err(FeedbackError::Validation(format!(
            "最多只能上传 {MAX_ATTACHMENTS} 个附件。"
        )));
    }
    let mut total = 0_u64;
    for attachment in attachments {
        let path = Path::new(&attachment.path);
        let metadata = fs::metadata(path).map_err(|_| {
            FeedbackError::Validation(format!(
                "附件不存在：{}",
                display_attachment_name(attachment)
            ))
        })?;
        if !metadata.is_file() {
            return Err(FeedbackError::Validation(format!(
                "附件不是有效文件：{}",
                display_attachment_name(attachment)
            )));
        }
        let size = metadata.len();
        total = total.saturating_add(size);
        let (media_type, limit) = classify_attachment(path)?;
        if media_type != attachment.media_type {
            return Err(FeedbackError::Validation(format!(
                "附件类型与文件格式不匹配：{}",
                display_attachment_name(attachment)
            )));
        }
        if size > limit {
            let mb = limit / 1024 / 1024;
            return Err(FeedbackError::Validation(format!(
                "{} 超过 {mb} MB，请压缩后再上传。",
                display_attachment_name(attachment)
            )));
        }
    }
    if total > MAX_TOTAL_ATTACHMENT_BYTES {
        return Err(FeedbackError::Validation(
            "附件总大小超过 80 MB，请减少附件后再上传。".to_string(),
        ));
    }
    Ok(())
}

pub fn build_app_context(request: &FeedbackSubmitRequest) -> BTreeMap<String, serde_json::Value> {
    let mut context = BTreeMap::new();
    context.insert(
        "app_version".to_string(),
        serde_json::json!(env!("CARGO_PKG_VERSION")),
    );
    context.insert("os".to_string(), serde_json::json!(std::env::consts::OS));
    context.insert(
        "arch".to_string(),
        serde_json::json!(std::env::consts::ARCH),
    );
    context.insert(
        "language".to_string(),
        serde_json::json!(crate::platform::prefs::UserPrefs::load()
            .language
            .locale_tag()),
    );
    context.insert(
        "entry_point".to_string(),
        serde_json::json!(request.entry_point),
    );
    context.insert(
        "error_summary".to_string(),
        serde_json::json!(request.error_summary.clone()),
    );
    context.insert("timestamp".to_string(), serde_json::json!(now_rfc3339()));
    context
}

fn prepare_feedback_package(
    request: &FeedbackSubmitRequest,
) -> Result<PreparedFeedbackPackage, FeedbackError> {
    let feedback_id = new_feedback_id();
    let package_dir = crate::platform::paths::feedback_pending_dir().join(&feedback_id);
    let attachments_dir = package_dir.join("attachments");
    fs::create_dir_all(&attachments_dir)?;
    fs::create_dir_all(crate::platform::paths::feedback_receipts_dir())?;

    let mut attachment_manifest = Vec::new();
    for (idx, attachment) in request.attachments.iter().enumerate() {
        let source = Path::new(&attachment.path);
        let (media_type, _) = classify_attachment(source)?;
        let extension = source
            .extension()
            .and_then(OsStr::to_str)
            .unwrap_or("bin")
            .to_ascii_lowercase();
        let package_name = format!("{:03}-{}.{}", idx + 1, media_type, extension);
        let dest = attachments_dir.join(&package_name);
        fs::copy(source, &dest)?;
        let size = fs::metadata(&dest)?.len();
        attachment_manifest.push(serde_json::json!({
            "id": format!("att-{:03}", idx + 1),
            "original_name": safe_file_name(&attachment.name),
            "package_name": format!("attachments/{package_name}"),
            "media_type": media_type,
            "mime": attachment.mime.clone().unwrap_or_else(|| guess_mime(&dest)),
            "size_bytes": size,
            "sha256": sha256_file(&dest)?,
        }));
    }

    let created_at = now_rfc3339();
    let manifest = serde_json::json!({
        "schema_version": "1.0",
        "feedback_id": feedback_id,
        "created_at": created_at,
        "type": request.feedback_type,
        "title": request.title,
        "description": request.description,
        "entry_point": request.entry_point,
        "privacy_notice_version": request.privacy_notice_version,
        "app_context": build_app_context(request),
        "attachments": attachment_manifest,
    });
    fs::write(
        package_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    fs::write(
        package_dir.join("description.txt"),
        request.description.as_bytes(),
    )?;

    Ok(PreparedFeedbackPackage {
        receipt_path: crate::platform::paths::feedback_receipts_dir()
            .join(format!("{feedback_id}.receipt.json")),
        tar_gz_path: package_dir.with_extension("tar.gz"),
        dbg_path: package_dir.with_extension("dbg"),
        feedback_id,
        package_dir,
    })
}

pub fn create_tar_gz_archive(source_dir: &Path, output_path: &Path) -> Result<(), FeedbackError> {
    if !source_dir.is_dir() {
        return Err(FeedbackError::Validation("反馈包目录不存在。".to_string()));
    }
    let file = fs::File::create(output_path)?;
    let encoder = GzEncoder::new(file, Compression::default());
    let mut tar = TarBuilder::new(encoder);
    append_dir_entries(&mut tar, source_dir, source_dir)?;
    tar.finish()?;
    let encoder = tar.into_inner()?;
    encoder.finish()?;
    Ok(())
}

fn append_dir_entries<W: Write>(
    tar: &mut TarBuilder<W>,
    root: &Path,
    dir: &Path,
) -> Result<(), FeedbackError> {
    let mut entries = fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            append_dir_entries(tar, root, &path)?;
        } else if path.is_file() {
            let rel = path
                .strip_prefix(root)
                .map_err(|_| FeedbackError::Validation("反馈包路径无效。".to_string()))?;
            tar.append_path_with_name(&path, rel)?;
        }
    }
    Ok(())
}

pub fn xor_to_dbg(source_path: &Path, dbg_path: &Path) -> Result<(), FeedbackError> {
    let bytes = fs::read(source_path)?;
    let encrypted: Vec<u8> = bytes.into_iter().map(|b| b ^ 0x55).collect();
    fs::write(dbg_path, encrypted)?;
    Ok(())
}

pub fn swap_serial_pairs(sn: &str) -> String {
    let chars: Vec<char> = sn.chars().collect();
    let mut out = String::new();
    let mut idx = 0;
    while idx < chars.len() {
        if idx + 1 < chars.len() {
            out.push(chars[idx + 1]);
        }
        out.push(chars[idx]);
        idx += 2;
    }
    out
}

pub fn compute_check_code(token: &str, serial: &str) -> String {
    let input = format!("{}{}", token, swap_serial_pairs(serial));
    format!("{:x}", md5::compute(input.as_bytes()))
}

pub async fn request_upload_token(serial: &str) -> Result<String, FeedbackError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;
    match request_upload_token_once(&client, serial).await {
        Ok(token) => Ok(token),
        Err(FeedbackError::Upload(_)) => request_upload_token_once(&client, serial).await,
        Err(e) => Err(e),
    }
}

async fn request_upload_token_once(
    client: &reqwest::Client,
    serial: &str,
) -> Result<String, FeedbackError> {
    #[derive(Serialize)]
    struct TokenRequest<'a> {
        #[serde(rename = "gwSn")]
        gw_sn: &'a str,
    }

    let response = client
        .post(TOKEN_URL)
        .json(&TokenRequest { gw_sn: serial })
        .send()
        .await?;
    let body = response.text().await?;
    parse_token_response(&body)
}

pub async fn upload_dbg_file(
    dbg_path: &Path,
    serial: &str,
    check_code: &str,
) -> Result<(), FeedbackError> {
    let bytes = fs::read(dbg_path)?;
    let file_name = dbg_path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| FeedbackError::Validation("反馈上传文件名无效。".to_string()))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?;
    let response = client
        .put(UPLOAD_URL)
        .header("GwSn", serial)
        .header("FileName", file_name)
        .header("checkCode", check_code)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .body(bytes)
        .send()
        .await?;
    let body = response.text().await?;
    parse_upload_response(&body)
}

pub fn parse_token_response(body: &str) -> Result<String, FeedbackError> {
    #[derive(Deserialize)]
    struct TokenResponse {
        #[serde(rename = "retCode")]
        ret_code: serde_json::Value,
        #[serde(rename = "retString")]
        ret_string: Option<serde_json::Value>,
    }

    let parsed: TokenResponse = serde_json::from_str(body)?;
    let ret_code = parse_h3c_ret_code(&parsed.ret_code)?;
    if ret_code == 0 {
        parsed
            .ret_string
            .and_then(|value| h3c_value_to_string(&value))
            .filter(|token| !token.trim().is_empty())
            .ok_or_else(|| FeedbackError::Upload("反馈上传 token 为空。".to_string()))
    } else {
        Err(FeedbackError::Upload(format!(
            "反馈上传 token 获取失败：{}。请确认设备 SN 已在反馈接收通道登记，或通过 PINVOU3_FEEDBACK_GW_SN 配置正确 SN。",
            format_h3c_error(ret_code, parsed.ret_string.as_ref())
        )))
    }
}

pub fn parse_upload_response(body: &str) -> Result<(), FeedbackError> {
    #[derive(Deserialize)]
    struct UploadResponse {
        #[serde(rename = "retCode")]
        ret_code: serde_json::Value,
        #[serde(rename = "retString")]
        ret_string: Option<serde_json::Value>,
    }

    let parsed: UploadResponse = serde_json::from_str(body)?;
    let ret_code = parse_h3c_ret_code(&parsed.ret_code)?;
    if ret_code == 0 {
        Ok(())
    } else {
        Err(FeedbackError::Upload(format!(
            "反馈上传失败：{}。",
            format_h3c_error(ret_code, parsed.ret_string.as_ref())
        )))
    }
}

fn parse_h3c_ret_code(value: &serde_json::Value) -> Result<i32, FeedbackError> {
    match value {
        serde_json::Value::Number(n) => n
            .as_i64()
            .and_then(|v| i32::try_from(v).ok())
            .ok_or_else(|| FeedbackError::Upload("反馈接收通道返回了无效状态码。".to_string())),
        serde_json::Value::String(s) => s
            .trim()
            .parse::<i32>()
            .map_err(|_| FeedbackError::Upload("反馈接收通道返回了无效状态码。".to_string())),
        _ => Err(FeedbackError::Upload(
            "反馈接收通道返回了无效状态码。".to_string(),
        )),
    }
}

fn format_h3c_error(ret_code: i32, ret_string: Option<&serde_json::Value>) -> String {
    match ret_string.and_then(h3c_value_to_string) {
        Some(message) if !message.trim().is_empty() => {
            format!("retCode={ret_code}, retString={message}")
        }
        _ => format!("retCode={ret_code}"),
    }
}

fn h3c_value_to_string(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(v) => Some(v.to_string()),
        serde_json::Value::Null => None,
        _ => Some(value.to_string()),
    }
}

pub fn resolve_device_serial() -> Option<String> {
    if let Some(serial) = TEMP_FEEDBACK_GW_SN_OVERRIDE {
        return Some(serial.to_string());
    }
    if let Ok(serial) = std::env::var("PINVOU3_FEEDBACK_GW_SN") {
        let serial = serial.trim().to_string();
        if !serial.is_empty() {
            return Some(serial);
        }
    }
    resolve_windows_serial()
}

#[cfg(target_os = "windows")]
fn resolve_windows_serial() -> Option<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let output = Command::new("powershell")
        .creation_flags(CREATE_NO_WINDOW)
        .args([
            "-NoProfile",
            "-Command",
            "(Get-CimInstance Win32_BIOS).SerialNumber",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let serial = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if serial.is_empty() {
        None
    } else {
        Some(serial)
    }
}

#[cfg(not(target_os = "windows"))]
fn resolve_windows_serial() -> Option<String> {
    None
}

fn cleanup_successful_package(package: &PreparedFeedbackPackage) -> Result<(), FeedbackError> {
    let _ = fs::remove_file(&package.tar_gz_path);
    let _ = fs::remove_file(&package.dbg_path);
    if package.package_dir.exists() {
        fs::remove_dir_all(&package.package_dir)?;
    }
    Ok(())
}

fn write_receipt(path: &Path, receipt: &FeedbackReceipt) -> Result<(), FeedbackError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(receipt)?)?;
    Ok(())
}

fn classify_attachment(path: &Path) -> Result<(&'static str, u64), FeedbackError> {
    let ext = path
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" => Ok(("image", MAX_IMAGE_BYTES)),
        "mp4" | "mov" | "webm" => Ok(("video", MAX_VIDEO_BYTES)),
        _ => Err(FeedbackError::Validation("附件格式不支持。".to_string())),
    }
}

fn guess_mime(path: &Path) -> String {
    match path
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn safe_file_name(name: &str) -> String {
    Path::new(name)
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("attachment")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn display_attachment_name(attachment: &FeedbackAttachmentRequest) -> String {
    if attachment.name.trim().is_empty() {
        attachment.path.clone()
    } else {
        attachment.name.clone()
    }
}

fn sha256_file(path: &Path) -> Result<String, FeedbackError> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0_u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

fn new_feedback_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let suffix = format!("{:x}", millis ^ (std::process::id() as u128));
    format!(
        "fb-{}-{}",
        Utc::now().format("%Y%m%d-%H%M%S"),
        &suffix[..6.min(suffix.len())]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct MockUploader {
        result: Result<(), &'static str>,
    }

    impl FeedbackUploader for MockUploader {
        fn upload<'a>(
            &'a self,
            _package: &'a PreparedFeedbackPackage,
        ) -> Pin<Box<dyn Future<Output = Result<(), FeedbackError>> + Send + 'a>> {
            Box::pin(async move {
                self.result
                    .clone()
                    .map_err(|e| FeedbackError::Upload(e.to_string()))
            })
        }
    }

    fn base_request() -> FeedbackSubmitRequest {
        FeedbackSubmitRequest {
            feedback_type: FeedbackType::Issue,
            title: Some("标题".to_string()),
            description: "反馈说明".to_string(),
            entry_point: "settings".to_string(),
            error_summary: None,
            attachments: Vec::new(),
            privacy_notice_version: "2026-06-24".to_string(),
        }
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pinvou-feedback-test-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn validates_basic_text_request() {
        assert!(validate_feedback_request(&base_request()).is_ok());

        let mut empty = base_request();
        empty.description = "  ".to_string();
        assert!(matches!(
            validate_feedback_request(&empty),
            Err(FeedbackError::Validation(_))
        ));

        let mut long_title = base_request();
        long_title.title = Some("x".repeat(MAX_TITLE_CHARS + 1));
        assert!(matches!(
            validate_feedback_request(&long_title),
            Err(FeedbackError::Validation(_))
        ));

        let mut bad_entry = base_request();
        bad_entry.entry_point = "other".to_string();
        assert!(matches!(
            validate_feedback_request(&bad_entry),
            Err(FeedbackError::Validation(_))
        ));
    }

    #[tokio::test]
    async fn mock_uploader_returns_submitted_and_retryable_receipts() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = unique_temp_dir("mock");
        std::env::set_var("PINVOU3_HOME", &root);

        let ok = submit_feedback_with_uploader(base_request(), &MockUploader { result: Ok(()) })
            .await
            .unwrap();
        assert_eq!(ok.status, FeedbackStatus::Submitted);
        assert!(!ok.retryable);

        let fail = submit_feedback_with_uploader(
            base_request(),
            &MockUploader {
                result: Err("down"),
            },
        )
        .await
        .unwrap();
        assert_eq!(fail.status, FeedbackStatus::FailedRetryable);
        assert!(fail.retryable);

        std::env::remove_var("PINVOU3_HOME");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn validates_attachment_limits_and_types() {
        let root = unique_temp_dir("attachments");
        let image = root.join("ok.png");
        fs::write(&image, b"png").unwrap();
        let video = root.join("ok.mp4");
        fs::write(&video, b"mp4").unwrap();
        let bad = root.join("bad.exe");
        fs::write(&bad, b"bad").unwrap();

        assert!(validate_attachments(&[FeedbackAttachmentRequest {
            path: image.to_string_lossy().into_owned(),
            name: "ok.png".to_string(),
            media_type: "image".to_string(),
            mime: None,
            size_bytes: None,
        }])
        .is_ok());

        assert!(validate_attachments(&[FeedbackAttachmentRequest {
            path: video.to_string_lossy().into_owned(),
            name: "ok.mp4".to_string(),
            media_type: "video".to_string(),
            mime: None,
            size_bytes: None,
        }])
        .is_ok());

        assert!(matches!(
            validate_attachments(&[FeedbackAttachmentRequest {
                path: bad.to_string_lossy().into_owned(),
                name: "bad.exe".to_string(),
                media_type: "image".to_string(),
                mime: None,
                size_bytes: None,
            }]),
            Err(FeedbackError::Validation(_))
        ));

        let too_many = (0..=MAX_ATTACHMENTS)
            .map(|_| FeedbackAttachmentRequest {
                path: image.to_string_lossy().into_owned(),
                name: "ok.png".to_string(),
                media_type: "image".to_string(),
                mime: None,
                size_bytes: None,
            })
            .collect::<Vec<_>>();
        assert!(matches!(
            validate_attachments(&too_many),
            Err(FeedbackError::Validation(_))
        ));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn manifest_does_not_include_original_absolute_attachment_path() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = unique_temp_dir("manifest");
        std::env::set_var("PINVOU3_HOME", &root);
        let image = root.join("source.png");
        fs::write(&image, b"png").unwrap();
        let mut req = base_request();
        req.attachments.push(FeedbackAttachmentRequest {
            path: image.to_string_lossy().into_owned(),
            name: "source.png".to_string(),
            media_type: "image".to_string(),
            mime: Some("image/png".to_string()),
            size_bytes: None,
        });

        let package = prepare_feedback_package(&req).unwrap();
        let manifest = fs::read_to_string(package.package_dir.join("manifest.json")).unwrap();
        assert!(manifest.contains("attachments/001-image.png"));
        assert!(!manifest.contains(&image.to_string_lossy().to_string()));

        std::env::remove_var("PINVOU3_HOME");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn swaps_serial_and_computes_check_code() {
        assert_eq!(swap_serial_pairs("abcdef"), "badcfe");
        assert_eq!(swap_serial_pairs("abcde"), "badce");
        let expected = format!("{:x}", md5::compute("tokenbadcfe".as_bytes()));
        assert_eq!(compute_check_code("token", "abcdef"), expected);
    }

    #[test]
    fn xor_dbg_matches_contract() {
        let root = unique_temp_dir("xor");
        let src = root.join("a.tar.gz");
        let dst = root.join("a.dbg");
        fs::write(&src, [0x00_u8, 0x55, 0xff]).unwrap();
        xor_to_dbg(&src, &dst).unwrap();
        assert_eq!(fs::read(&dst).unwrap(), vec![0x55, 0x00, 0xaa]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn tar_gz_contains_feedback_files() {
        let root = unique_temp_dir("tar");
        let package = root.join("pkg");
        fs::create_dir_all(package.join("attachments")).unwrap();
        fs::write(package.join("manifest.json"), b"{}").unwrap();
        fs::write(package.join("description.txt"), b"hello").unwrap();
        fs::write(package.join("attachments").join("001-image.png"), b"png").unwrap();
        let tar_path = root.join("pkg.tar.gz");
        create_tar_gz_archive(&package, &tar_path).unwrap();
        assert!(tar_path.is_file());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn parses_h3c_upload_responses() {
        assert_eq!(
            parse_token_response(r#"{"retCode":0,"retString":"abc"}"#).unwrap(),
            "abc"
        );
        assert_eq!(
            parse_token_response(r#"{"retCode":"0","retString":"abc"}"#).unwrap(),
            "abc"
        );
        assert!(parse_token_response(r#"{"retCode":1,"retString":null}"#).is_err());
        let token_err =
            parse_token_response(r#"{"retCode":"24","retString":"invalid sn"}"#).unwrap_err();
        assert!(token_err.to_string().contains("retCode=24"));
        assert!(token_err.to_string().contains("invalid sn"));
        assert!(parse_upload_response(r#"{"retCode":0}"#).is_ok());
        assert!(parse_upload_response(r#"{"retCode":"0"}"#).is_ok());
        assert!(parse_upload_response(r#"{"retCode":9}"#).is_err());
        let upload_err = parse_upload_response(r#"{"retCode":"24"}"#).unwrap_err();
        assert!(upload_err.to_string().contains("retCode=24"));
    }
}

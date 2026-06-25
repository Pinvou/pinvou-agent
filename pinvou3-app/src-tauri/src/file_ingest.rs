//! 输入文件预处理：把用户上传的文件统一转成 markdown 文本，附到 user message
//! 让 LLM 看懂。
//!
//! 设计：
//! - 文本类(txt/md/json/csv/yaml/code) → `fs::read_to_string`
//! - PDF → `pdftotext -layout` (`poppler-utils`)
//! - docx/pptx/odt → `pandoc -t markdown`
//! - xlsx → `pandoc -t markdown`（pandoc 支持 office 格式）
//! - 图片 → 不读像素，只标记 `model_no_vision`(配合 prompt 防臆测)
//! - 其他 → binary 占位
//!
//! 系统工具检测：启动时缓存 `which pandoc / pdftotext` 结果。缺失时返回
//! `warning: "需要安装 ..."`，前端 chip 显示，不阻塞其它格式。
//!
//! Token 估算：粗算 `chars / 1.6`（中英混合保守值）。不引 tiktoken-rs 减依赖。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// 单文件硬上限 20 MB —— 超大文件就算转 md 后 token 数也炸上下文。
const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestResult {
    /// 类型分类：text / pdf / docx / xlsx / image / binary
    pub kind: String,
    /// 文件名（不含路径）
    pub basename: String,
    /// 原始绝对路径（用于发送时构造 prompt 引用）
    pub path: String,
    /// 转换后的 markdown 内容（image/binary 为 None）
    pub markdown: Option<String>,
    /// token 估算值（粗算）。前端用来累加显示「已用 X / Y」
    pub token_estimate: u32,
    /// 原始字节数
    pub byte_size: u64,
    /// 警告或错误消息：超大、缺工具、不支持视觉等。前端 chip 上 ⚠️ 显示。
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct SystemTools {
    pub pandoc: bool,
    pub pdftotext: bool,
    /// LibreOffice headless —— 用来转 .doc / .ppt / .xls 等旧 office 格式。
    pub libreoffice: bool,
    /// tesseract OCR —— 图片文字识别 + 扫描件 PDF 兜底。
    pub tesseract: bool,
    /// pdftoppm（poppler-utils）—— 把扫描件 PDF 逐页转图再喂给 tesseract。
    pub pdftoppm: bool,
    /// 7z —— 解 zip/7z/rar 等压缩包；Windows 优先使用内置 7-Zip。
    pub sevenzip: bool,
    /// python3 —— 解析 .eml 邮件（标准库 email 模块，无额外依赖）。
    pub python3: bool,
    /// msgconvert（libemail-outlook-message-perl）—— Linux .msg → .eml；Windows 走 Rust 原生解析。
    pub msgconvert: bool,
}

static SYSTEM_TOOLS: OnceLock<SystemTools> = OnceLock::new();

/// 启动时（或第一次 ingest 时）检测一次系统工具。
pub fn system_tools() -> SystemTools {
    *SYSTEM_TOOLS.get_or_init(|| SystemTools {
        pandoc: crate::os::pandoc_tool_exists(),
        pdftotext: crate::os::pdf_tool_exists("pdftotext"),
        libreoffice: crate::os::command_exists("soffice") || crate::os::command_exists("libreoffice"),
        tesseract: crate::os::ocr_tool_exists(),
        pdftoppm: crate::os::pdf_tool_exists("pdftoppm"),
        sevenzip: crate::os::archive_tool_exists(),
        python3: crate::os::command_exists("python3"),
        msgconvert: !crate::os::msg_converter_required() || crate::os::command_exists("msgconvert"),
    })
}

/// 设置页「依赖体检」一项：一类文件解析能力 + 它所需系统工具是否齐全 + 缺失时的 apt 包。
/// `apt` 是空格分隔的包名串，可直接拼进 `sudo apt install <apt>`。能力名走前端 i18n（按 key 映射）。
#[derive(Debug, Clone, Serialize)]
pub struct DependencyCheckItem {
    pub key: String,
    pub installed: bool,
    pub apt: String,
}

/// 体检各项可选依赖的安装状态。**实时检测（不走 `system_tools` 的 OnceLock 缓存）**——
/// 用户照提示装完依赖后重新体检要能立刻反映，不能被首次缓存钉死。命名/分组与
/// `ingest` 内各格式分支的 warning 文案同源，缺啥装啥一致。
#[tauri::command]
pub fn check_dependencies() -> Vec<DependencyCheckItem> {
    let item = |key: &str, installed: bool, apt: &str| DependencyCheckItem {
        key: key.into(),
        installed,
        apt: apt.into(),
    };
    let libreoffice = crate::os::command_exists("soffice") || crate::os::command_exists("libreoffice");
    let mut items = Vec::new();
    if crate::os::show_pdf_dependency_check() {
        items.push(item(
            "pdf",
            crate::os::pdf_tool_exists("pdftotext"),
            crate::os::pdf_dependency_packages(),
        ));
    }
    if crate::os::show_pandoc_dependency_check() {
        items.push(item(
            "office_modern",
            crate::os::pandoc_tool_exists(),
            crate::os::pandoc_dependency_packages(),
        ));
    }
    items.push(item(
        "voice_asr",
        crate::os::asr_tool_exists(),
        crate::os::asr_dependency_packages(),
    ));
    items.push(item("office_legacy", libreoffice, "libreoffice"));
    if crate::os::show_ocr_dependency_check() {
        items.push(item(
            "ocr",
            crate::os::ocr_tool_exists() && crate::os::pdf_tool_exists("pdftoppm"),
            crate::os::ocr_dependency_packages(),
        ));
    }
    if crate::os::show_archive_dependency_check() {
        items.push(item(
            "archive",
            crate::os::archive_tool_exists(),
            crate::os::archive_dependency_packages(),
        ));
    }
    items.push(item(
        "email",
        crate::os::email_tool_exists(),
        crate::os::email_dependency_packages(),
    ));
    items
}

fn pdf_tool_command(command: &str) -> Command {
    crate::process::HiddenCommand::new(crate::os::pdf_tool_path(command))
}

fn pandoc_tool_command() -> Command {
    crate::process::HiddenCommand::new(crate::os::pandoc_tool_path())
}

fn ocr_tool_command() -> Command {
    crate::process::HiddenCommand::new(crate::os::ocr_tool_path())
}

fn archive_tool_command() -> Command {
    crate::process::HiddenCommand::new(crate::os::archive_tool_path())
}

fn add_ocr_tessdata_arg(command: &mut Command) {
    if let Some(tessdata_dir) = crate::os::ocr_tessdata_dir() {
        command.arg("--tessdata-dir").arg(tessdata_dir);
    }
}

/// 体检卡「一键安装」：委托 OS 调度层安装缺失依赖。
/// Linux 由 OS 层保留包名白名单和 pkexec/apt 行为；其他系统清晰降级。
#[tauri::command]
pub async fn install_dependencies(packages: Vec<String>) -> Result<(), String> {
    tokio::task::spawn_blocking(move || crate::os::install_dependencies(packages))
        .await
        .map_err(|e| format!("安装任务失败: {e}"))?
}

/// tesseract 的 `-l` 语言参数。pinvou3 面向国内政企，中文是刚需，所以优先
/// `chi_sim+eng`；若没装中文包(`tesseract-ocr-chi-sim`)则降级 `eng`，不报错。
/// 探测一次缓存：跑 `tesseract --list-langs` 看输出里有没有 `chi_sim`。
fn ocr_lang_arg() -> String {
    static LANG: OnceLock<String> = OnceLock::new();
    LANG.get_or_init(|| {
        let mut command = ocr_tool_command();
        command.arg("--list-langs");
        add_ocr_tessdata_arg(&mut command);
        let listed = command
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        if listed.lines().any(|l| l.trim() == "chi_sim") {
            "chi_sim+eng".to_string()
        } else {
            "eng".to_string()
        }
    })
    .clone()
}

/// 主入口：派发到不同处理函数，返回统一 IngestResult。
pub fn ingest(path: &Path) -> IngestResult {
    let basename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("(unnamed)")
        .to_string();
    let path_str = path.to_string_lossy().to_string();
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            return IngestResult {
                kind: "missing".into(),
                basename,
                path: path_str,
                markdown: None,
                token_estimate: 0,
                byte_size: 0,
                warning: Some(format!("文件不存在: {e}")),
            };
        }
    };
    let byte_size = meta.len();
    if byte_size > MAX_FILE_BYTES {
        return IngestResult {
            kind: "oversize".into(),
            basename,
            path: path_str,
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: Some(format!(
                "文件 {:.1} MB 超过 20 MB 上限,请拆分或裁剪",
                byte_size as f64 / 1024.0 / 1024.0
            )),
        };
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    let kind = classify(&ext);

    match kind {
        "text" => ingest_text(path, basename, path_str, byte_size),
        "pdf" => ingest_pdf(path, basename, path_str, byte_size),
        // 文字文档：pandoc 原生支持 docx/odt。
        "doc_pandoc" => ingest_pandoc(path, basename, path_str, byte_size, &ext),
        // 文字文档：pandoc 吃不下，LibreOffice 转纯文本（doc/rtf/wps）。
        "doc_office" => ingest_office_text(path, basename, path_str, byte_size, &ext),
        // 演示：LibreOffice 转 PDF 再 pdftotext（pptx/ppt/odp/dps）。
        "presentation" => ingest_presentation(path, basename, path_str, byte_size, &ext),
        // 表格：LibreOffice 转 CSV（xlsx/ods/xls/et）—— pandoc 不支持表格输入。
        "spreadsheet" => ingest_spreadsheet(path, basename, path_str, byte_size, &ext),
        "image" => ingest_image(path, basename, path_str, byte_size),
        "archive" => ingest_archive(path, basename, path_str, byte_size),
        "email" => ingest_email(path, basename, path_str, byte_size, &ext),
        "media" => media_placeholder(basename, path_str, byte_size),
        _ => binary_placeholder(basename, path_str, byte_size),
    }
}

/// 按「文档类型」而非新旧分流：pandoc 只吃 docx/odt，pptx/ppt/xlsx/ods/xls 一律
/// 走 LibreOffice（演示→PDF→pdftotext，表格→CSV）。WPS 三件套按用途归类：
/// .wps→文字、.et→表格、.dps→演示。
fn classify(ext: &str) -> &'static str {
    match ext {
        "txt" | "md" | "markdown" | "json" | "csv" | "yaml" | "yml" | "toml" | "xml" | "rs"
        | "py" | "js" | "ts" | "go" | "c" | "cpp" | "h" | "hpp" | "sh" | "log" | "ini" | "conf"
        | "env" | "tsv" => "text",
        "pdf" => "pdf",
        // 文字：pandoc 原生支持
        "docx" | "odt" => "doc_pandoc",
        // 文字：pandoc 不支持 → LibreOffice txt（含 WPS 文字 .wps）
        "doc" | "rtf" | "wps" => "doc_office",
        // 演示：LibreOffice 无 txt 导出 → 转 PDF 再 pdftotext（含 WPS 演示 .dps）
        "pptx" | "ppt" | "odp" | "dps" => "presentation",
        // 表格：pandoc 不支持 xlsx/ods → LibreOffice csv（含 WPS 表格 .et）
        "xlsx" | "ods" | "xls" | "et" => "spreadsheet",
        // 仅底座 image_analyze 支持的位图格式走视觉(vision/tools.rs detect_mime_type)。
        // svg(矢量)/tiff 不在支持列表 —— 不归 image,落到 binary 兜底给"不支持"提示,
        // 避免被当图暂存后 image_analyze 报 "Unsupported image format"。
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => "image",
        // 压缩包：解压后递归识别（7z 统一处理 zip/rar/7z）
        "zip" | "rar" | "7z" => "archive",
        // 邮件：eml 走 python email 标准库；msg 按 OS 策略解析
        "eml" | "msg" => "email",
        // 音视频：本地语音转录(whisper)尚未部署，先优雅降级标「未处理」
        "mp4" | "avi" | "mov" | "mkv" | "webm" | "flv" | "wmv" | "m4v" | "mp3" | "wav"
        | "m4a" | "aac" | "flac" | "ogg" | "wma" => "media",
        _ => "binary",
    }
}

fn ingest_text(path: &Path, basename: String, path_str: String, byte_size: u64) -> IngestResult {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let tokens = estimate_tokens(&content);
            IngestResult {
                kind: "text".into(),
                basename,
                path: path_str,
                markdown: Some(content),
                token_estimate: tokens,
                byte_size,
                warning: None,
            }
        }
        Err(e) => IngestResult {
            kind: "text".into(),
            basename,
            path: path_str,
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: Some(format!("读取失败(可能不是文本): {e}")),
        },
    }
}

fn ingest_pdf(path: &Path, basename: String, path_str: String, byte_size: u64) -> IngestResult {
    let tools = system_tools();
    if !tools.pdftotext {
        return IngestResult {
            kind: "pdf".into(),
            basename,
            path: path_str,
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: Some(crate::os::pdf_text_missing_message().into()),
        };
    }
    // pdftotext -layout <path> -  → stdout
    let out = pdf_tool_command("pdftotext")
        .arg("-layout")
        .arg(path)
        .arg("-")
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let content = String::from_utf8_lossy(&o.stdout).into_owned();
            // 扫描件 PDF（图层 PDF）没有文字层，pdftotext 返回空白 —— 此时
            // 走 OCR 兜底：pdftoppm 逐页转图再喂 tesseract。
            if content.trim().is_empty() {
                return ocr_pdf(path, basename, path_str, byte_size);
            }
            let tokens = estimate_tokens(&content);
            IngestResult {
                kind: "pdf".into(),
                basename,
                path: path_str,
                markdown: Some(content),
                token_estimate: tokens,
                byte_size,
                warning: None,
            }
        }
        Ok(o) => IngestResult {
            kind: "pdf".into(),
            basename,
            path: path_str,
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: Some(format!(
                "pdftotext 失败: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            )),
        },
        Err(e) => IngestResult {
            kind: "pdf".into(),
            basename,
            path: path_str,
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: Some(format!("pdftotext 调用失败: {e}")),
        },
    }
}

fn ingest_pandoc(
    path: &Path,
    basename: String,
    path_str: String,
    byte_size: u64,
    label: &str,
) -> IngestResult {
    let tools = system_tools();
    if !tools.pandoc {
        return IngestResult {
            kind: label.into(),
            basename,
            path: path_str,
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: Some(crate::os::pandoc_missing_message().into()),
        };
    }
    let out = pandoc_tool_command()
        .arg("-t")
        .arg("markdown")
        .arg(path)
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let content = String::from_utf8_lossy(&o.stdout).into_owned();
            let tokens = estimate_tokens(&content);
            IngestResult {
                kind: label.into(),
                basename,
                path: path_str,
                markdown: Some(content),
                token_estimate: tokens,
                byte_size,
                warning: None,
            }
        }
        Ok(o) => IngestResult {
            kind: label.into(),
            basename,
            path: path_str,
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: Some(format!(
                "pandoc 失败: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            )),
        },
        Err(e) => IngestResult {
            kind: label.into(),
            basename,
            path: path_str,
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: Some(format!("pandoc 调用失败: {e}")),
        },
    }
}

/// 用 LibreOffice headless 把文件转成指定 filter 的产物并读回文本。复用于旧
/// office、WPS 文字、电子表格等 pandoc 吃不下的格式。`convert_to` 是 soffice 的
/// 输出 filter 串，`out_ext` 是产物扩展名（用来在临时目录里定位输出文件）。
fn libreoffice_convert_text(
    path: &Path,
    convert_to: &str,
    out_ext: &str,
) -> Result<String, String> {
    if !system_tools().libreoffice {
        return Err("需要 LibreOffice，请运行: sudo apt install libreoffice".into());
    }
    // 临时目录：每次唯一，避免并发文件名冲突。
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmpdir = std::env::temp_dir().join(format!("pinvou3-libreoffice-{ts}"));
    std::fs::create_dir_all(&tmpdir).map_err(|e| format!("创建临时目录失败: {e}"))?;

    // 独立 UserInstallation profile：LibreOffice 同一 profile 不能并发(会 lock)，
    // 用户一次拖多个 office 文件时前端会并发 ingest_file，必须各用各的 profile。
    let out = Command::new("soffice")
        .arg(format!("-env:UserInstallation=file://{}/profile", tmpdir.display()))
        .arg("--headless")
        .arg("--convert-to")
        .arg(convert_to)
        .arg("--outdir")
        .arg(&tmpdir)
        .arg(path)
        .output();

    let result = match out {
        Ok(o) if o.status.success() => {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("converted");
            let out_path = tmpdir.join(format!("{stem}.{out_ext}"));
            std::fs::read_to_string(&out_path)
                // soffice 的 txt/csv 导出会带 UTF-8 BOM，去掉以免污染正文开头。
                .map(|s| s.trim_start_matches('\u{feff}').to_string())
                .map_err(|e| format!("LibreOffice 转换后读取失败: {e}"))
        }
        Ok(o) => Err(format!(
            "LibreOffice 转换失败: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => Err(format!("LibreOffice 调用失败: {e}")),
    };
    let _ = std::fs::remove_dir_all(&tmpdir);
    result
}

// ============== 产物可视化预览助手（commands::render_artifact_visual 复用）==============

/// 极简 base64 标准编码（无换行）。仅为内联图片 data URI 用，不值得引第三方 crate。
pub fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { TABLE[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[(n & 63) as usize] as char } else { '=' });
    }
    out
}

/// 图片扩展名 → MIME。用于 data URI 前缀。
fn image_mime(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// 单个图片文件 → `data:image/...;base64,...`。
pub fn image_file_to_data_uri(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读图失败: {e}"))?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    Ok(format!("data:{};base64,{}", image_mime(ext), base64_encode(&bytes)))
}

/// 把 HTML 里指向本地旁置图片的 `src` 引用 base64 内联,产出自包含 HTML。
/// soffice 导出的 HTML 把图片写成 `<stem>_html_xxx.png` 同目录文件,iframe `srcDoc`
/// 加载不到这些相对路径 → 全部内联。已是 data:/http 的跳过。双引号、单引号都处理。
fn inline_html_images(html: &str, dir: &Path) -> String {
    let mut result = html.to_string();
    for quote in ['"', '\''] {
        let needle = format!("src={quote}");
        let mut rebuilt = String::with_capacity(result.len());
        let mut from = 0;
        loop {
            match result[from..].find(&needle) {
                Some(rel) => {
                    let val_start = from + rel + needle.len();
                    match result[val_start..].find(quote) {
                        Some(endrel) => {
                            let val_end = val_start + endrel;
                            let val = &result[val_start..val_end];
                            rebuilt.push_str(&result[from..val_start]); // 含 src="
                            if val.starts_with("data:") || val.starts_with("http") || val.is_empty()
                            {
                                rebuilt.push_str(val);
                            } else {
                                let fname = val.trim_start_matches("./");
                                match image_file_to_data_uri(&dir.join(fname)) {
                                    Ok(uri) => rebuilt.push_str(&uri),
                                    Err(_) => rebuilt.push_str(val),
                                }
                            }
                            from = val_end; // 闭合引号留给下一轮拼接
                        }
                        None => {
                            rebuilt.push_str(&result[from..]);
                            break;
                        }
                    }
                }
                None => {
                    rebuilt.push_str(&result[from..]);
                    break;
                }
            }
        }
        result = rebuilt;
    }
    result
}

/// office 文档 → 可视化 HTML（版式/图片还原）。soffice `--convert-to html`,旁置图片
/// 内联成自包含 HTML 返回,前端直接喂 iframe srcDoc。复用 libreoffice_convert_text 的
/// 独立 UserInstallation profile + 临时目录约定。
pub fn libreoffice_to_inline_html(path: &Path) -> Result<String, String> {
    if !system_tools().libreoffice {
        return Err("需要 LibreOffice，请运行: sudo apt install libreoffice".into());
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmpdir = std::env::temp_dir().join(format!("pinvou3-lo-html-{ts}"));
    std::fs::create_dir_all(&tmpdir).map_err(|e| format!("创建临时目录失败: {e}"))?;

    let out = Command::new("soffice")
        .arg(format!("-env:UserInstallation=file://{}/profile", tmpdir.display()))
        .arg("--headless")
        .arg("--convert-to")
        // 不写死 `html:HTML`(那是 Writer 专用 filter,套到 Calc/Impress 会无产出)。
        // 只给 `html` → LibreOffice 按文档类型自动选对应 HTML 导出 filter。
        .arg("html")
        .arg("--outdir")
        .arg(&tmpdir)
        .arg(path)
        .output();

    let result = (|| -> Result<String, String> {
        match out {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                return Err(format!(
                    "LibreOffice 转换失败: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                ))
            }
            Err(e) => return Err(format!("LibreOffice 调用失败: {e}")),
        }
        // 不假设产物叫 `<stem>.html`(filter 不同 / 文件名带特殊字符都可能变)——
        // 扫临时目录里产出的 .html(优先匹配 stem,否则取首个)。
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let htmls: Vec<PathBuf> = std::fs::read_dir(&tmpdir)
            .map(|rd| {
                rd.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| {
                        matches!(p.extension().and_then(|e| e.to_str()), Some("html") | Some("htm"))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let pick = htmls
            .iter()
            .find(|p| p.file_stem().and_then(|s| s.to_str()) == Some(stem))
            .or_else(|| htmls.first())
            .ok_or_else(|| "LibreOffice 未产出 HTML".to_string())?;
        let html = std::fs::read_to_string(pick)
            .map_err(|e| format!("读取转换 HTML 失败: {e}"))?;
        Ok(inline_html_images(&html, &tmpdir))
    })();
    let _ = std::fs::remove_dir_all(&tmpdir);
    result
}

/// 演示稿(pptx/ppt/odp)→ 先转 PDF 再逐页转 PNG。Impress 的 HTML 导出会拆成一堆
/// 文件且版式失真,转 PDF→PNG 更可靠:每页 = 一张幻灯片。复用 110 dpi。
pub fn office_to_png_data_uris(path: &Path, max_pages: u32) -> Result<(Vec<String>, bool), String> {
    let tools = system_tools();
    if !tools.libreoffice {
        return Err("需要 LibreOffice，请运行: sudo apt install libreoffice".into());
    }
    if !tools.pdftoppm {
        return Err(crate::os::pdf_render_missing_message().into());
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmpdir = std::env::temp_dir().join(format!("pinvou3-office-png-{ts}"));
    std::fs::create_dir_all(&tmpdir).map_err(|e| format!("创建临时目录失败: {e}"))?;

    let result = (|| -> Result<(Vec<String>, bool), String> {
        // 1) office → PDF
        let out = Command::new("soffice")
            .arg(format!("-env:UserInstallation=file://{}/profile", tmpdir.display()))
            .arg("--headless")
            .arg("--convert-to")
            .arg("pdf")
            .arg("--outdir")
            .arg(&tmpdir)
            .arg(path)
            .output();
        match out {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                return Err(format!(
                    "LibreOffice 转 PDF 失败: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                ))
            }
            Err(e) => return Err(format!("LibreOffice 调用失败: {e}")),
        }
        // 找产出的 PDF(扫目录,别假设文件名)。
        let pdf = std::fs::read_dir(&tmpdir)
            .ok()
            .and_then(|rd| {
                rd.filter_map(|e| e.ok().map(|e| e.path()))
                    .find(|p| p.extension().and_then(|e| e.to_str()) == Some("pdf"))
            })
            .ok_or_else(|| "LibreOffice 未产出 PDF".to_string())?;

        // 2) PDF → PNG 页(直接在同一 tmpdir,避免再开目录)。
        let prefix = tmpdir.join("page");
        let conv = pdf_tool_command("pdftoppm")
            .arg("-png")
            .arg("-r")
            .arg("110")
            .arg("-l")
            .arg(max_pages.to_string())
            .arg(&pdf)
            .arg(&prefix)
            .output();
        match conv {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                return Err(format!(
                    "pdftoppm 转图失败: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                ))
            }
            Err(e) => return Err(format!("pdftoppm 调用失败: {e}")),
        }
        let mut pages: Vec<PathBuf> = std::fs::read_dir(&tmpdir)
            .map(|rd| {
                rd.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| {
                        p.extension().and_then(|e| e.to_str()) == Some("png")
                            && p.file_stem().and_then(|s| s.to_str())
                                .map(|s| s.starts_with("page"))
                                .unwrap_or(false)
                    })
                    .collect()
            })
            .unwrap_or_default();
        pages.sort();
        if pages.is_empty() {
            return Err("未产出可渲染幻灯片页".into());
        }
        let truncated = pages.len() as u32 >= max_pages;
        let mut uris = Vec::with_capacity(pages.len());
        for p in &pages {
            uris.push(image_file_to_data_uri(p)?);
        }
        Ok((uris, truncated))
    })();
    let _ = std::fs::remove_dir_all(&tmpdir);
    result
}

/// PDF → 逐页 PNG 的 data URI 列表(可视化预览)。复用 ocr_pdf 的 pdftoppm 调用样板,
/// 但 110 dpi(预览够清又不至于 data URI 过大)。返回 (data_uris, 是否因上限截断)。
pub fn pdf_to_png_data_uris(path: &Path, max_pages: u32) -> Result<(Vec<String>, bool), String> {
    if !system_tools().pdftoppm {
        return Err(crate::os::pdf_render_missing_message().into());
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmpdir = std::env::temp_dir().join(format!("pinvou3-pdfpreview-{ts}"));
    std::fs::create_dir_all(&tmpdir).map_err(|e| format!("创建临时目录失败: {e}"))?;
    let prefix = tmpdir.join("page");

    let convert = pdf_tool_command("pdftoppm")
        .arg("-png")
        .arg("-r")
        .arg("110")
        .arg("-l")
        .arg(max_pages.to_string())
        .arg(path)
        .arg(&prefix)
        .output();

    let result = (|| -> Result<(Vec<String>, bool), String> {
        match convert {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                return Err(format!(
                    "pdftoppm 转图失败: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                ))
            }
            Err(e) => return Err(format!("pdftoppm 调用失败: {e}")),
        }
        let mut pages: Vec<PathBuf> = std::fs::read_dir(&tmpdir)
            .map(|rd| {
                rd.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("png"))
                    .collect()
            })
            .unwrap_or_default();
        pages.sort();
        if pages.is_empty() {
            return Err("PDF 未产出可渲染页".into());
        }
        let truncated = pages.len() as u32 >= max_pages;
        let mut uris = Vec::with_capacity(pages.len());
        for p in &pages {
            uris.push(image_file_to_data_uri(p)?);
        }
        Ok((uris, truncated))
    })();
    let _ = std::fs::remove_dir_all(&tmpdir);
    result
}

/// 文字类文档（.doc/.rtf + WPS .wps）：pandoc 吃不下，用 LibreOffice 转纯文本。
fn ingest_office_text(
    path: &Path,
    basename: String,
    path_str: String,
    byte_size: u64,
    kind: &str,
) -> IngestResult {
    match libreoffice_convert_text(path, "txt:Text (encoded):UTF8", "txt") {
        Ok(content) => {
            let tokens = estimate_tokens(&content);
            IngestResult {
                kind: kind.into(),
                basename,
                path: path_str,
                markdown: Some(content),
                token_estimate: tokens,
                byte_size,
                warning: None,
            }
        }
        Err(e) => IngestResult {
            kind: kind.into(),
            basename,
            path: path_str,
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: Some(e),
        },
    }
}

/// 电子表格（.xlsx/.ods/.xls + WPS .et）：pandoc **不支持表格输入**，走 LibreOffice
/// 转 CSV。注意 LibreOffice CSV 只导「活动工作表」（通常首个），多工作表会丢——
/// 故在内容前显式标注，让 LLM/用户心里有数。
fn ingest_spreadsheet(
    path: &Path,
    basename: String,
    path_str: String,
    byte_size: u64,
    kind: &str,
) -> IngestResult {
    match libreoffice_convert_text(path, "csv:Text - txt - csv (StarCalc)", "csv") {
        Ok(content) => {
            let body = format!(
                "> 注：电子表格已转 CSV；若原文件含多个工作表，此处仅含首个工作表。\n\n{content}"
            );
            let tokens = estimate_tokens(&body);
            IngestResult {
                kind: kind.into(),
                basename,
                path: path_str,
                markdown: Some(body),
                token_estimate: tokens,
                byte_size,
                warning: None,
            }
        }
        Err(e) => IngestResult {
            kind: kind.into(),
            basename,
            path: path_str,
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: Some(e),
        },
    }
}

/// 演示类（.pptx/.ppt/.odp + WPS .dps）：LibreOffice **没有 Impress→txt 导出**
/// （实测无产出），所以先转 PDF 再用 pdftotext 抽每页文字（标题/要点/中文都保留）。
/// 自己转出的 PDF 必有文字层，不需要扫描件 OCR 兜底。
fn ingest_presentation(
    path: &Path,
    basename: String,
    path_str: String,
    byte_size: u64,
    kind: &str,
) -> IngestResult {
    let tools = system_tools();
    if !tools.libreoffice || !tools.pdftotext {
        return IngestResult {
            kind: kind.into(),
            basename,
            path: path_str,
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: Some(crate::os::presentation_pdf_missing_message().into()),
        };
    }
    match libreoffice_presentation_text(path) {
        Ok(content) if !content.trim().is_empty() => {
            let tokens = estimate_tokens(&content);
            IngestResult {
                kind: kind.into(),
                basename,
                path: path_str,
                markdown: Some(content),
                token_estimate: tokens,
                byte_size,
                warning: None,
            }
        }
        Ok(_) => IngestResult {
            kind: kind.into(),
            basename,
            path: path_str,
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: Some("演示文稿未提取到文字".into()),
        },
        Err(e) => IngestResult {
            kind: kind.into(),
            basename,
            path: path_str,
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: Some(e),
        },
    }
}

/// 演示文稿 → PDF（LibreOffice）→ pdftotext 的串联，返回纯文本。
fn libreoffice_presentation_text(path: &Path) -> Result<String, String> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmpdir = std::env::temp_dir().join(format!("pinvou3-pptpdf-{ts}"));
    std::fs::create_dir_all(&tmpdir).map_err(|e| format!("创建临时目录失败: {e}"))?;

    let convert = Command::new("soffice")
        .arg(format!("-env:UserInstallation=file://{}/profile", tmpdir.display()))
        .arg("--headless")
        .arg("--convert-to")
        .arg("pdf")
        .arg("--outdir")
        .arg(&tmpdir)
        .arg(path)
        .output();

    let result = match convert {
        Ok(o) if o.status.success() => {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("converted");
            let pdf_path = tmpdir.join(format!("{stem}.pdf"));
            pdf_tool_command("pdftotext")
                .arg("-layout")
                .arg(&pdf_path)
                .arg("-")
                .output()
                .map_err(|e| format!("pdftotext 调用失败: {e}"))
                .and_then(|o| {
                    if o.status.success() {
                        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
                    } else {
                        Err(format!(
                            "pdftotext 失败: {}",
                            String::from_utf8_lossy(&o.stderr).trim()
                        ))
                    }
                })
        }
        Ok(o) => Err(format!(
            "LibreOffice 转 PDF 失败: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => Err(format!("LibreOffice 调用失败: {e}")),
    };
    let _ = std::fs::remove_dir_all(&tmpdir);
    result
}

/// 图片：Qwen3.6 有视觉能力(2026-05-28 实证),不再跑 OCR 降级。这里只登记元
/// 数据,真正"看图"由 LLM 在对话里调 `image_analyze` 完成——commands.rs 发消息时
/// 会把图拷进 session workspace 的 `attachments/` 并给出相对路径引导。
/// markdown 留空(不预解析像素),token_estimate=0(视觉 token 量取决于分辨率,
/// 不在此处解码估算,UI 计数会略低,属已知局限)。
fn ingest_image(_path: &Path, basename: String, path_str: String, byte_size: u64) -> IngestResult {
    IngestResult {
        kind: "image".into(),
        basename,
        path: path_str,
        markdown: None,
        token_estimate: 0,
        byte_size,
        warning: None,
    }
}

/// 对单张图片跑 tesseract，识别文字到 stdout。`tesseract <img> - -l <langs>`。
fn ocr_image(path: &Path) -> Result<String, String> {
    let lang = ocr_lang_arg();
    let mut command = ocr_tool_command();
    command.arg(path).arg("-").arg("-l").arg(&lang);
    add_ocr_tessdata_arg(&mut command);
    let out = command
        .output()
        .map_err(|e| format!("tesseract 调用失败: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    } else {
        Err(format!(
            "tesseract 退出码 {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// 扫描件 PDF OCR 兜底：pdftoppm 逐页转 PNG（150 dpi），每页跑 tesseract，拼接。
/// 页数封顶 PDF_OCR_MAX_PAGES，超出截断并在末尾标注，避免几十页扫描件把上下文撑爆。
fn ocr_pdf(path: &Path, basename: String, path_str: String, byte_size: u64) -> IngestResult {
    const PDF_OCR_MAX_PAGES: u32 = 30;
    let tools = system_tools();
    if !tools.tesseract || !tools.pdftoppm {
        return IngestResult {
            kind: "pdf".into(),
            basename,
            path: path_str,
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: Some(crate::os::pdf_ocr_missing_message().into()),
        };
    }

    // 临时目录：每次唯一，避免并发冲突。
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmpdir = std::env::temp_dir().join(format!("pinvou3-pdfocr-{ts}"));
    if let Err(e) = std::fs::create_dir_all(&tmpdir) {
        return IngestResult {
            kind: "pdf".into(),
            basename,
            path: path_str,
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: Some(format!("创建临时目录失败: {e}")),
        };
    }

    let prefix = tmpdir.join("page");
    // pdftoppm -png -r 150 -l <max> <pdf> <prefix> → prefix-1.png, prefix-2.png ...
    let convert = pdf_tool_command("pdftoppm")
        .arg("-png")
        .arg("-r")
        .arg("150")
        .arg("-l")
        .arg(PDF_OCR_MAX_PAGES.to_string())
        .arg(path)
        .arg(&prefix)
        .output();

    let result = match convert {
        Ok(o) if o.status.success() => {
            // 收集生成的 png，按文件名排序保证页序。
            let mut pages: Vec<PathBuf> = std::fs::read_dir(&tmpdir)
                .map(|rd| {
                    rd.filter_map(|e| e.ok().map(|e| e.path()))
                        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("png"))
                        .collect()
                })
                .unwrap_or_default();
            pages.sort();

            if pages.is_empty() {
                IngestResult {
                    kind: "pdf".into(),
                    basename,
                    path: path_str.clone(),
                    markdown: None,
                    token_estimate: 0,
                    byte_size,
                    warning: Some("PDF 无文字层，且 pdftoppm 未产出可识别页".into()),
                }
            } else {
                let mut parts = Vec::new();
                for (idx, page) in pages.iter().enumerate() {
                    match ocr_image(page) {
                        Ok(text) if !text.trim().is_empty() => {
                            parts.push(format!("## 第 {} 页\n\n{}", idx + 1, text.trim()));
                        }
                        _ => {}
                    }
                }
                let mut content = parts.join("\n\n");
                if pages.len() as u32 >= PDF_OCR_MAX_PAGES {
                    content.push_str(&format!(
                        "\n\n> ⚠️ 扫描件页数较多，OCR 仅处理前 {PDF_OCR_MAX_PAGES} 页"
                    ));
                }
                if content.trim().is_empty() {
                    IngestResult {
                        kind: "pdf".into(),
                        basename,
                        path: path_str.clone(),
                        markdown: None,
                        token_estimate: 0,
                        byte_size,
                        warning: Some("扫描件 OCR 未识别到文字".into()),
                    }
                } else {
                    let tokens = estimate_tokens(&content);
                    IngestResult {
                        kind: "pdf".into(),
                        basename,
                        path: path_str.clone(),
                        markdown: Some(content),
                        token_estimate: tokens,
                        byte_size,
                        warning: Some("扫描件 PDF，内容由 OCR 提取，可能有识别误差".into()),
                    }
                }
            }
        }
        Ok(o) => IngestResult {
            kind: "pdf".into(),
            basename,
            path: path_str.clone(),
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: Some(format!(
                "pdftoppm 转图失败: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            )),
        },
        Err(e) => IngestResult {
            kind: "pdf".into(),
            basename,
            path: path_str.clone(),
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: Some(format!("pdftoppm 调用失败: {e}")),
        },
    };

    let _ = std::fs::remove_dir_all(&tmpdir);
    result
}

/// 压缩包（.zip/.rar/.7z）：先用 7z 列出内容做炸弹预检（条目数 + 解压后总大小，
/// 解压前就拦），通过后解压到临时目录，递归调主 `ingest` 处理每个文件并汇总。
/// 嵌套压缩包不再展开（防套娃炸弹）。因为复用主 ingest，包里的 PDF/Office/图片
/// 都会按各自管线（含 OCR）处理。
fn ingest_archive(
    path: &Path,
    basename: String,
    path_str: String,
    byte_size: u64,
) -> IngestResult {
    const MAX_ENTRIES: usize = 50;
    const MAX_TOTAL_BYTES: u64 = 100 * 1024 * 1024; // 解压后总量上限，防压缩炸弹

    let mk_err = |msg: String| IngestResult {
        kind: "archive".into(),
        basename: basename.clone(),
        path: path_str.clone(),
        markdown: None,
        token_estimate: 0,
        byte_size,
        warning: Some(msg),
    };

    if !system_tools().sevenzip {
        return mk_err(archive_tool_missing_message());
    }

    // 预检：解压前就用 7z 列表拦截压缩炸弹。
    match archive_list_stats(path) {
        Ok((count, total)) => {
            if count > MAX_ENTRIES {
                return mk_err(format!("压缩包条目过多（{count} > {MAX_ENTRIES}），拒绝展开"));
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
    let tokens = estimate_tokens(&content);
    IngestResult {
        kind: "archive".into(),
        basename,
        path: path_str,
        markdown: Some(content),
        token_estimate: tokens,
        byte_size,
        warning: None,
    }
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
    if crate::os::show_archive_dependency_check() {
        let packages = crate::os::archive_dependency_packages();
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

/// 邮件（.eml / .msg）：.eml 直接用 python 标准库 email 模块解出收发件人/主题/
/// 日期/正文/附件名；.msg 在 Windows 走 Rust 原生解析，非 Windows 保留 msgconvert 转 .eml。
fn ingest_email(
    path: &Path,
    basename: String,
    path_str: String,
    byte_size: u64,
    kind: &str,
) -> IngestResult {
    let tools = system_tools();
    let mk = |markdown: Option<String>, warning: Option<String>| {
        let token_estimate = markdown.as_deref().map(estimate_tokens).unwrap_or(0);
        IngestResult {
            kind: kind.into(),
            basename: basename.clone(),
            path: path_str.clone(),
            markdown,
            token_estimate,
            byte_size,
            warning,
        }
    };

    if kind == "msg" && crate::os::msg_native_supported() {
        return match parse_msg_via_msg_parser(path) {
            Ok(text) => mk(Some(text), None),
            Err(e) => mk(None, Some(e)),
        };
    }

    if !tools.python3 {
        return mk(None, Some("邮件解析需要 python3，请运行: sudo apt install python3".into()));
    }

    let parsed = if kind == "msg" {
        if !tools.msgconvert {
            return mk(
                None,
                Some(
                    ".msg 解析需要: sudo apt install libemail-outlook-message-perl".into(),
                ),
            );
        }
        // msgconvert 把 .msg 转成 .eml（输出到 cwd），用临时目录承接再解析。
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmpdir = std::env::temp_dir().join(format!("pinvou3-msg-{ts}"));
        if let Err(e) = std::fs::create_dir_all(&tmpdir) {
            return mk(None, Some(format!("创建临时目录失败: {e}")));
        }
        let conv = Command::new("msgconvert")
            .current_dir(&tmpdir)
            .arg(path)
            .output();
        let result = if matches!(&conv, Ok(o) if o.status.success()) {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("mail");
            let eml = tmpdir.join(format!("{stem}.eml"));
            parse_eml_via_python(&eml)
        } else {
            let detail = match conv {
                Ok(o) => String::from_utf8_lossy(&o.stderr).trim().to_string(),
                Err(e) => e.to_string(),
            };
            Err(format!("msgconvert 转换失败: {detail}"))
        };
        let _ = std::fs::remove_dir_all(&tmpdir);
        result
    } else {
        parse_eml_via_python(path)
    };

    match parsed {
        Ok(text) => mk(Some(text), None),
        Err(e) => mk(None, Some(e)),
    }
}

/// Outlook .msg 解析结果格式化为与 .eml 接近的可读邮件文本。
#[derive(Default)]
struct MsgMarkdownParts {
    sender: String,
    to: Vec<String>,
    cc: Vec<String>,
    bcc: Vec<String>,
    subject: String,
    date: String,
    body: String,
    attachments: Vec<String>,
}

fn parse_msg_via_msg_parser(path: &Path) -> Result<String, String> {
    let outlook =
        msg_parser::Outlook::from_path(path).map_err(|e| format!(".msg 解析失败: {e}"))?;
    let body = decode_msg_body(&outlook);
    let parts = MsgMarkdownParts {
        sender: person_to_text(&outlook.sender),
        to: people_to_text(&outlook.to),
        cc: people_to_text(&outlook.cc),
        bcc: people_to_text(&outlook.bcc),
        subject: clean_msg_text(&outlook.subject),
        date: first_non_empty([
            outlook.message_delivery_time.as_str(),
            outlook.client_submit_time.as_str(),
            outlook.headers.date.as_str(),
            outlook.creation_time.as_str(),
        ]),
        body,
        attachments: outlook
            .attachments
            .iter()
            .map(attachment_to_name)
            .filter(|name| !name.is_empty())
            .collect(),
    };
    let markdown = format_msg_as_markdown(&parts);
    if markdown.trim().is_empty() {
        Err(".msg 解析失败: 未提取到邮件内容".into())
    } else {
        Ok(markdown)
    }
}

fn format_msg_as_markdown(parts: &MsgMarkdownParts) -> String {
    let mut out = String::new();
    push_mail_line(&mut out, "发件人", &parts.sender);
    push_mail_line(&mut out, "收件人", &parts.to.join(", "));
    push_mail_line(&mut out, "抄送", &parts.cc.join(", "));
    push_mail_line(&mut out, "密送", &parts.bcc.join(", "));
    push_mail_line(&mut out, "主题", &parts.subject);
    push_mail_line(&mut out, "日期", &parts.date);
    if !parts.body.trim().is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("正文:\n");
        out.push_str(parts.body.trim());
        out.push('\n');
    }
    if !parts.attachments.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("附件: ");
        out.push_str(&parts.attachments.join(", "));
    }
    out.trim_end().to_string()
}

fn push_mail_line(out: &mut String, label: &str, value: &str) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    out.push_str(label);
    out.push_str(": ");
    out.push_str(value);
    out.push('\n');
}

fn people_to_text(people: &[msg_parser::Person]) -> Vec<String> {
    people
        .iter()
        .map(person_to_text)
        .filter(|value| !value.is_empty())
        .collect()
}

fn person_to_text(person: &msg_parser::Person) -> String {
    clean_msg_text(&person.to_string())
}

fn attachment_to_name(attachment: &msg_parser::Attachment) -> String {
    [
        &attachment.long_file_name,
        &attachment.file_name,
        &attachment.display_name,
    ]
    .into_iter()
    .map(|value| clean_msg_text(value))
    .find(|value| !value.is_empty())
    .unwrap_or_default()
}

fn first_non_empty<'a>(values: impl IntoIterator<Item = &'a str>) -> String {
    values
        .into_iter()
        .map(clean_msg_text)
        .find(|value| !value.is_empty())
        .unwrap_or_default()
}

fn decode_msg_body(outlook: &msg_parser::Outlook) -> String {
    let body = clean_msg_text(&outlook.body);
    if !body.is_empty() {
        return body;
    }

    let html = if outlook.html.trim().is_empty() {
        outlook.html_from_rtf().unwrap_or_default()
    } else {
        outlook.html.clone()
    };
    let decoded = decode_msg_html_payload(&html);
    let text = html_to_text(&decoded);
    if text.is_empty() {
        decoded
    } else {
        text
    }
}

fn clean_msg_text(value: &str) -> String {
    value.chars().filter(|ch| *ch != '\0').collect::<String>().trim().to_string()
}

fn decode_msg_html_payload(value: &str) -> String {
    let value = clean_msg_text(value);
    if value.len() < 8 || value.len() % 2 != 0 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return value;
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    let raw = value.as_bytes();
    for pair in raw.chunks_exact(2) {
        let Ok(hex) = std::str::from_utf8(pair) else {
            return value;
        };
        let Ok(byte) = u8::from_str_radix(hex, 16) else {
            return value;
        };
        bytes.push(byte);
    }
    decode_msg_bytes(&bytes)
}

fn decode_msg_bytes(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return decode_utf16le(&bytes[2..]);
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return decode_utf16be(&bytes[2..]);
    }
    if let Ok(text) = String::from_utf8(bytes.to_vec()) {
        return clean_msg_text(&text);
    }
    let nul_count = bytes.iter().filter(|byte| **byte == 0).count();
    if nul_count > bytes.len() / 4 {
        decode_utf16le(bytes)
    } else {
        clean_msg_text(&String::from_utf8_lossy(bytes))
    }
}

fn decode_utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    clean_msg_text(&String::from_utf16_lossy(&units))
}

fn decode_utf16be(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
        .collect();
    clean_msg_text(&String::from_utf16_lossy(&units))
}

fn html_to_text(html: &str) -> String {
    let html = remove_html_section(html, "script");
    let html = remove_html_section(&html, "style");
    let html = remove_html_section(&html, "head");
    let html = html
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("</p>", "\n")
        .replace("</div>", "\n")
        .replace("</tr>", "\n")
        .replace("</li>", "\n");

    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    collapse_text(&decode_html_entities(&out))
}

fn remove_html_section(input: &str, tag: &str) -> String {
    let mut out = input.to_string();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    loop {
        let lower = out.to_ascii_lowercase();
        let Some(start) = lower.find(&open) else {
            break;
        };
        let Some(end_rel) = lower[start..].find(&close) else {
            out.truncate(start);
            break;
        };
        let end = start + end_rel + close.len();
        out.replace_range(start..end, " ");
    }
    out
}

fn decode_html_entities(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after_amp = &rest[start + 1..];
        let Some(end) = after_amp.find(';') else {
            out.push('&');
            rest = after_amp;
            continue;
        };
        let entity = &after_amp[..end];
        if let Some(decoded) = decode_html_entity(entity) {
            out.push(decoded);
        } else {
            out.push('&');
            out.push_str(entity);
            out.push(';');
        }
        rest = &after_amp[end + 1..];
    }
    out.push_str(rest);
    out
}

fn decode_html_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        _ if entity.starts_with("#x") || entity.starts_with("#X") => {
            u32::from_str_radix(&entity[2..], 16).ok().and_then(char::from_u32)
        }
        _ if entity.starts_with('#') => {
            entity[1..].parse::<u32>().ok().and_then(char::from_u32)
        }
        _ => None,
    }
}

fn collapse_text(value: &str) -> String {
    let mut out = String::new();
    let mut blank_lines = 0;
    for line in value.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            blank_lines += 1;
            if blank_lines <= 1 && !out.is_empty() {
                out.push('\n');
            }
        } else {
            blank_lines = 0;
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&line);
        }
    }
    out.trim().to_string()
}

/// 用 python 标准库 email 模块把 .eml 解析成可读文本（收发件人/主题/日期/正文/
/// 附件名）。脚本走 stdin 之外的 argv[1] 取路径，正文优先纯文本、回退 HTML。
fn parse_eml_via_python(path: &Path) -> Result<String, String> {
    const SCRIPT: &str = r#"
import sys, email
from email import policy
with open(sys.argv[1], 'rb') as f:
    msg = email.message_from_binary_file(f, policy=policy.default)
def h(k):
    v = msg[k]
    return str(v) if v else ''
print('发件人:', h('from'))
print('收件人:', h('to'))
if msg['cc']:
    print('抄送:', h('cc'))
print('主题:', h('subject'))
print('日期:', h('date'))
try:
    body = msg.get_body(preferencelist=('plain', 'html'))
    if body is not None:
        print('\n正文:')
        print(body.get_content())
except Exception as e:
    print('\n(正文解析失败:', e, ')')
atts = [p.get_filename() for p in msg.iter_attachments() if p.get_filename()]
if atts:
    print('\n附件:', ', '.join(atts))
"#;
    let out = crate::process::HiddenCommand::new("python3")
        .arg("-c")
        .arg(SCRIPT)
        .arg(path)
        .output()
        .map_err(|e| format!("python3 调用失败: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    } else {
        Err(format!(
            "邮件解析失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// 音视频：本地语音转录（whisper 等）尚未部署到 GB10，先优雅降级，明确告知用户
/// 「未处理」而非臆测内容。真正转录留作未来独立能力（见 process.md）。
fn media_placeholder(basename: String, path_str: String, byte_size: u64) -> IngestResult {
    IngestResult {
        kind: "media".into(),
        basename,
        path: path_str,
        markdown: None,
        token_estimate: 0,
        byte_size,
        warning: Some(
            "检测到音视频文件，当前暂不支持本地语音转录。\
             可改为提供文字稿，或口述其中要点。"
                .into(),
        ),
    }
}

fn binary_placeholder(basename: String, path_str: String, byte_size: u64) -> IngestResult {
    IngestResult {
        kind: "binary".into(),
        basename,
        path: path_str,
        markdown: None,
        token_estimate: 0,
        byte_size,
        warning: Some("不支持的文件类型(二进制)".into()),
    }
}

/// 粗算 token：中英混合按 `chars / 1.6` —— 比较保守，偏向高估避免炸上下文。
/// 实测 cl100k_base 中文 1.0-1.5 char/token、英文 3-4 char/token。
fn estimate_tokens(text: &str) -> u32 {
    let chars = text.chars().count();
    (chars as f64 / 1.6).ceil() as u32
}

/// 把剪贴板粘贴的图片 bytes 写到 `~/.pinvou3/pastes/<timestamp>-<sanitized_name>`。
/// 只用于「Ctrl+V 粘贴图片」——磁盘上没有原 path 的场景。
/// 选文件 / 拖拽都走 Tauri native 拿原 path，不调用这个。
pub fn save_paste_image(filename: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!(
            "图片 {:.1} MB 超过 20 MB 上限",
            bytes.len() as f64 / 1024.0 / 1024.0
        ));
    }
    let home = std::env::var("HOME").map_err(|_| "HOME not set")?;
    let pastes = PathBuf::from(home).join(".pinvou3").join("pastes");
    std::fs::create_dir_all(&pastes).map_err(|e| format!("create pastes dir: {e}"))?;
    let safe_name = sanitize_filename(filename);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let target = pastes.join(format!("{ts}-{safe_name}"));
    std::fs::write(&target, bytes).map_err(|e| format!("write paste: {e}"))?;
    Ok(target)
}

/// 把文件名做 sanitize：去掉路径分隔符、控制字符；保留中英文 + 常见标点。
fn sanitize_filename(raw: &str) -> String {
    let trimmed = raw
        .rsplit(|c| c == '/' || c == '\\')
        .next()
        .unwrap_or("file");
    let cleaned: String = trimmed
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '|' | '"' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    if cleaned.is_empty() {
        "file".into()
    } else {
        cleaned
    }
}

/// 校验路径：必须绝对 + 在 $HOME 下 + 不在敏感目录。
/// 跟 commands::validate_user_path 同语义，单独抽出供前端 ingest 入口调用。
pub fn validate_path(raw: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(raw);
    if !p.is_absolute() {
        return Err(format!("path must be absolute: {raw}"));
    }
    let canon = normalize_validated_path(&std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone()));
    let home_raw = crate::os::user_home_dir();
    let home = normalize_validated_path(
        &std::fs::canonicalize(&home_raw).unwrap_or_else(|_| home_raw.clone()),
    );
    if !canon.starts_with(&home) {
        return Err(format!("path {} not under $HOME", canon.display()));
    }
    for blocked in &[".ssh", ".gnupg", ".aws", ".docker", ".kube"] {
        if canon
            .components()
            .any(|c| c.as_os_str() == std::ffi::OsStr::new(blocked))
        {
            return Err(format!(
                "path {} crosses sensitive dir {}",
                canon.display(),
                blocked
            ));
        }
    }
    Ok(canon)
}

fn normalize_validated_path(path: &Path) -> PathBuf {
    crate::os::platform_compat_path(&path.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_extensions() {
        assert_eq!(classify("md"), "text");
        assert_eq!(classify("json"), "text");
        assert_eq!(classify("pdf"), "pdf");
        // 文字：pandoc 支持 docx/odt
        assert_eq!(classify("docx"), "doc_pandoc");
        assert_eq!(classify("odt"), "doc_pandoc");
        // 文字：LibreOffice txt（含 WPS .wps）
        assert_eq!(classify("doc"), "doc_office");
        assert_eq!(classify("rtf"), "doc_office");
        assert_eq!(classify("wps"), "doc_office");
        // 演示：转 PDF → pdftotext（pptx/ppt 不能再走 pandoc；含 WPS .dps）
        assert_eq!(classify("pptx"), "presentation");
        assert_eq!(classify("ppt"), "presentation");
        assert_eq!(classify("dps"), "presentation");
        // 表格：LibreOffice csv（含 WPS .et）
        assert_eq!(classify("xlsx"), "spreadsheet");
        assert_eq!(classify("ods"), "spreadsheet");
        assert_eq!(classify("xls"), "spreadsheet");
        assert_eq!(classify("et"), "spreadsheet");
        assert_eq!(classify("png"), "image");
        assert_eq!(classify("zip"), "archive");
        assert_eq!(classify("rar"), "archive");
        assert_eq!(classify("eml"), "email");
        assert_eq!(classify("msg"), "email");
        assert_eq!(classify("mp4"), "media");
        assert_eq!(classify("mp3"), "media");
        assert_eq!(classify(""), "binary");
        assert_eq!(classify("exe"), "binary");
    }

    #[test]
    fn estimate_tokens_grows_with_content() {
        let small = estimate_tokens("hi");
        let big = estimate_tokens(&"x".repeat(1000));
        assert!(big > small);
        assert!(big < 1000); // 不应大于字符数
    }

    #[test]
    fn ingest_text_reads_md() {
        // 写一个临时文件，调 ingest
        let tmp = std::env::temp_dir().join("pinvou3-ingest-test.md");
        std::fs::write(&tmp, "# 标题\n\n内容。").unwrap();
        let r = ingest(&tmp);
        assert_eq!(r.kind, "text");
        assert!(r.markdown.as_deref().unwrap_or("").contains("标题"));
        assert!(r.token_estimate > 0);
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn ingest_oversize_rejected() {
        // 模拟超大文件：write 21MB 内容，应被拒
        let tmp = std::env::temp_dir().join("pinvou3-ingest-oversize-test.bin");
        let big = vec![0u8; (MAX_FILE_BYTES + 1024) as usize];
        std::fs::write(&tmp, &big).unwrap();
        let r = ingest(&tmp);
        assert_eq!(r.kind, "oversize");
        assert!(r.warning.is_some());
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn ingest_image_registers_metadata_no_ocr() {
        // 视觉接入后(2026-05-28):图片不再走 OCR 降级,只登记元数据
        // (kind=image, markdown=None, 无 model_no_vision 警告)。真正读图由 LLM
        // 在对话里调 image_analyze 完成(commands.rs 把图拷进 workspace)。
        let tmp = std::env::temp_dir().join("pinvou3-ingest-image-test.png");
        std::fs::write(&tmp, b"fake png bytes").unwrap();
        let r = ingest(&tmp);
        std::fs::remove_file(&tmp).ok();
        assert_eq!(r.kind, "image");
        assert!(r.markdown.is_none(), "图片不预解析 markdown");
        assert!(
            r.warning.is_none(),
            "视觉可用,不应再有 model_no_vision 警告,got warning={:?}",
            r.warning
        );
    }

    #[test]
    fn validate_path_rejects_relative() {
        assert!(validate_path("relative/path.txt").is_err());
    }

    #[test]
    fn validate_path_rejects_outside_home() {
        assert!(validate_path("/etc/passwd").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn validate_path_accepts_windows_canonicalized_home_file() {
        let home = crate::os::user_home_dir();
        let file = home.join(format!(
            "pinvou3-validate-path-{}.txt",
            std::process::id()
        ));
        std::fs::write(&file, "ok").unwrap();

        let validated = validate_path(file.to_str().unwrap()).unwrap();
        assert!(validated.starts_with(&home));

        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn classify_routes_image_formats_by_vision_support() {
        // 视觉(image_analyze)支持的位图走 image。
        for e in ["png", "jpg", "jpeg", "gif", "webp", "bmp"] {
            assert_eq!(classify(e), "image", "{e} 应走 image");
        }
        // svg(矢量)/tiff 不被视觉工具支持 → 落 binary 兜底,不当图,
        // 否则会被暂存后 image_analyze 报 Unsupported image format。
        for e in ["svg", "tiff", "tif"] {
            assert_eq!(classify(e), "binary", "{e} 不应走 image,应落 binary 兜底");
        }
    }

    #[test]
    fn pdf_tool_command_uses_os_layer_program() {
        let command = pdf_tool_command("pdftotext");
        assert_eq!(
            command.get_program(),
            crate::os::pdf_tool_path("pdftotext").as_os_str()
        );
    }

    #[test]
    fn archive_tool_command_uses_os_layer_program() {
        let command = archive_tool_command();
        assert_eq!(command.get_program(), crate::os::archive_tool_path().as_os_str());
    }

    #[test]
    fn dependency_check_respects_pdf_visibility_policy() {
        let deps = check_dependencies();
        let has_pdf = deps.iter().any(|item| item.key == "pdf");
        let has_pandoc = deps.iter().any(|item| item.key == "office_modern");
        let has_ocr = deps.iter().any(|item| item.key == "ocr");
        let has_archive = deps.iter().any(|item| item.key == "archive");
        assert_eq!(has_pdf, crate::os::show_pdf_dependency_check());
        assert_eq!(has_pandoc, crate::os::show_pandoc_dependency_check());
        assert_eq!(has_ocr, crate::os::show_ocr_dependency_check());
        assert_eq!(has_archive, crate::os::show_archive_dependency_check());

        if !crate::os::show_pdf_dependency_check() {
            assert!(
                deps.iter()
                    .all(|item| !item.apt.contains("poppler") && !item.apt.contains("pdfto")),
                "hidden Windows Poppler dependency should not leave install hints: {deps:?}"
            );
        }
        if !crate::os::show_pandoc_dependency_check() {
            assert!(
                deps.iter()
                    .all(|item| !item.apt.contains("pandoc") && item.key != "office_modern"),
                "hidden Windows Pandoc dependency should not leave install hints: {deps:?}"
            );
        }
        if !crate::os::show_ocr_dependency_check() {
            assert!(
                deps.iter().all(|item| {
                    !item.apt.contains("tesseract") && !item.apt.contains("tesseract-ocr")
                }),
                "hidden Windows OCR dependency should not leave install hints: {deps:?}"
            );
        }
        if !crate::os::show_archive_dependency_check() {
            assert!(
                deps.iter()
                    .all(|item| !item.apt.contains("p7zip") && item.key != "archive"),
                "hidden Windows archive dependency should not leave install hints: {deps:?}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_archive_missing_message_points_to_bundled_runtime() {
        let message = archive_tool_missing_message();

        assert!(message.contains("内置压缩包解析组件"));
        assert!(!message.contains("sudo apt install"));
        assert!(!message.contains("p7zip-full"));
    }

    #[test]
    fn msg_markdown_format_includes_headers_body_and_attachments() {
        let parts = MsgMarkdownParts {
            sender: "alice@example.com".into(),
            to: vec!["bob@example.com".into()],
            cc: vec!["carol@example.com".into()],
            bcc: vec!["audit@example.com".into()],
            subject: "项目进展".into(),
            date: "2026-06-25T10:00:00Z".into(),
            body: "这是邮件正文".into(),
            attachments: vec!["report.pdf".into(), "报价.xlsx".into()],
        };

        let markdown = format_msg_as_markdown(&parts);

        assert!(markdown.contains("发件人: alice@example.com"));
        assert!(markdown.contains("收件人: bob@example.com"));
        assert!(markdown.contains("抄送: carol@example.com"));
        assert!(markdown.contains("密送: audit@example.com"));
        assert!(markdown.contains("主题: 项目进展"));
        assert!(markdown.contains("日期: 2026-06-25T10:00:00Z"));
        assert!(markdown.contains("正文:\n这是邮件正文"));
        assert!(markdown.contains("附件: report.pdf, 报价.xlsx"));
    }

    #[test]
    fn msg_text_cleanup_removes_nul_padding() {
        assert_eq!(clean_msg_text("OpenAI\0"), "OpenAI");
        assert_eq!(clean_msg_text("你的临时 OpenAI 登录代码\0"), "你的临时 OpenAI 登录代码");
    }

    #[test]
    fn msg_hex_html_body_decodes_to_readable_text() {
        let html_hex = "3c68746d6c3e3c686561643e3c7374796c653e2e78207b20636f6c6f723a207265643b207d3c2f7374796c653e3c2f686561643e3c626f64793e3c703e4f70656e414920e799bbe5bd95e4bba3e7a081efbc9a203132333435363c2f703e3c703ee8afb7e58bbfe58886e4baab3c2f703e3c2f626f64793e3c2f68746d6c3e";
        let text = html_to_text(&decode_msg_html_payload(html_hex));

        assert!(text.contains("OpenAI 登录代码"));
        assert!(text.contains("123456"));
        assert!(text.contains("请勿分享"));
        assert!(!text.contains("3c68746d6c"));
        assert!(!text.contains("<html>"));
    }

    #[test]
    fn msg_sample_from_env_decodes_when_provided() {
        let Ok(path) = std::env::var("PINVOU3_MSG_SAMPLE") else {
            return;
        };
        let parsed = parse_msg_via_msg_parser(Path::new(&path)).unwrap();

        assert!(!parsed.contains('\0'));
        assert!(!parsed.contains("3c68746d6c"));
        assert!(parsed.contains("OpenAI") || parsed.contains("正文:"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_invalid_msg_returns_warning_without_msgconvert_dependency() {
        let tmp = std::env::temp_dir().join("pinvou3-invalid-msg-test.msg");
        std::fs::write(&tmp, b"not an outlook msg").unwrap();

        let r = ingest(&tmp);

        std::fs::remove_file(&tmp).ok();
        assert_eq!(r.kind, "msg");
        assert_eq!(r.basename, "pinvou3-invalid-msg-test.msg");
        assert_eq!(r.path, tmp.to_string_lossy());
        assert_eq!(r.byte_size, "not an outlook msg".len() as u64);
        assert!(r.markdown.is_none());
        let warning = r.warning.unwrap_or_default();
        assert!(warning.contains(".msg"));
        assert!(!warning.contains("libemail-outlook-message-perl"));
        assert!(!warning.contains("msgconvert"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_email_dependency_check_uses_native_msg_parser() {
        let deps = check_dependencies();
        let email = deps
            .iter()
            .find(|item| item.key == "email")
            .expect("email dependency item should exist");

        assert!(email.installed);
        assert!(email.apt.is_empty());
        assert!(!email.apt.contains("libemail-outlook-message-perl"));
        assert!(!email.apt.contains("msgconvert"));
    }

    #[test]
    fn eml_regression_parses_headers_body_and_attachment_when_python_available() {
        if !crate::os::command_exists("python3") {
            eprintln!("skip: python3 is not available");
            return;
        }

        let dir =
            std::env::temp_dir().join(format!("pinvou3-eml-regression-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let eml = dir.join("m.eml");
        let raw = concat!(
            "From: alice@example.com\r\n",
            "To: bob@example.com\r\n",
            "Subject: Project Update\r\n",
            "Date: Thu, 25 Jun 2026 10:00:00 +0800\r\n",
            "MIME-Version: 1.0\r\n",
            "Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n",
            "--b\r\n",
            "Content-Type: text/plain; charset=utf-8\r\n\r\n",
            "This is email body\r\n",
            "--b\r\n",
            "Content-Type: text/plain; name=\"note.txt\"\r\n",
            "Content-Disposition: attachment; filename=\"note.txt\"\r\n\r\n",
            "attachment\r\n",
            "--b--\r\n",
        );
        std::fs::write(&eml, raw).unwrap();

        let parsed = parse_eml_via_python(&eml).unwrap();

        std::fs::remove_dir_all(&dir).ok();
        assert!(parsed.contains("alice@example.com"));
        assert!(parsed.contains("bob@example.com"));
        assert!(parsed.contains("Project Update"));
        assert!(parsed.contains("Thu, 25 Jun 2026 10:00:00 +0800"));
        assert!(parsed.contains("This is email body"));
        assert!(parsed.contains("note.txt"));
    }

    #[test]
    fn pandoc_tool_command_uses_os_layer_program() {
        let command = pandoc_tool_command();
        assert_eq!(
            command.get_program(),
            crate::os::pandoc_tool_path().as_os_str()
        );
    }

    #[test]
    fn ocr_tool_command_uses_os_layer_program() {
        let command = ocr_tool_command();
        assert_eq!(
            command.get_program(),
            crate::os::ocr_tool_path().as_os_str()
        );
    }

    #[test]
    fn ocr_tessdata_arg_is_added_when_os_layer_provides_dir() {
        let mut command = ocr_tool_command();
        add_ocr_tessdata_arg(&mut command);
        let args: Vec<_> = command.get_args().map(|arg| arg.to_os_string()).collect();
        if let Some(dir) = crate::os::ocr_tessdata_dir() {
            assert!(args.iter().any(|arg| arg == "--tessdata-dir"));
            assert!(args.iter().any(|arg| arg == dir.as_os_str()));
        } else {
            assert!(args.iter().all(|arg| arg != "--tessdata-dir"));
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_pandoc_missing_message_points_to_repair_install() {
        let message = crate::os::pandoc_missing_message();
        assert!(!message.contains("sudo apt install pandoc"));
        assert!(message.contains("修复") || message.contains("重新安装"));
    }

    /// 端到端 OCR 实测：依赖本机装了 tesseract + chi_sim，且 /tmp 下有用
    /// PIL 造的中文测试图/扫描件 PDF（见 PR 说明的造图脚本）。常规 CI 无这些
    /// 前置，故 `#[ignore]`；手动 `cargo test -- --ignored ocr_extracts_chinese`。
    /// 验证两条真实代码路径：图片直 OCR + 扫描件 PDF（pdftotext 空→pdftoppm→OCR）。
    #[test]
    #[ignore = "需要本机 tesseract+chi_sim 与 /tmp 测试文件"]
    fn ocr_extracts_chinese_from_image_and_scanned_pdf() {
        if !system_tools().tesseract {
            eprintln!("跳过：本机无 tesseract");
            return;
        }
        let img = Path::new("/tmp/ocr_test_cn.png");
        if img.exists() {
            let r = ingest(img);
            assert_eq!(r.kind, "image");
            let md = r.markdown.expect("中文图必须 OCR 出文字");
            assert!(md.contains("品悟"), "图片中文 OCR 内容异常: {md}");
        }
        let pdf = Path::new("/tmp/ocr_test_scan.pdf");
        if pdf.exists() {
            let r = ingest(pdf);
            assert_eq!(r.kind, "pdf");
            let md = r.markdown.expect("扫描件 PDF 必须走 OCR 兜底出文字");
            assert!(md.contains("品悟"), "扫描件 OCR 内容异常: {md}");
            assert!(
                r.warning.as_deref().unwrap_or("").contains("OCR"),
                "扫描件应标注内容由 OCR 提取, got {:?}",
                r.warning
            );
        }
    }

    /// L2-9: .docx 扩展名必须 dispatch 到 ingest_pandoc 路径（kind="docx"），
    /// 不能 fallthrough 到 binary_placeholder。pandoc 是否真装好不影响 dispatch
    /// 决策（无 pandoc 时返回 warning，有则 markdown is_some）。这条防的是
    /// classify→dispatch 链路在重构时被改坏，导致 docx 上传走 binary 死路。
    #[test]
    fn file_ingest_pandoc_detects_docx() {
        let tmp = std::env::temp_dir().join("pinvou3-ingest-docx-test.docx");
        std::fs::write(&tmp, b"PK\x03\x04 fake docx zip header").unwrap();
        let r = ingest(&tmp);
        std::fs::remove_file(&tmp).ok();
        assert_eq!(
            r.kind, "docx",
            ".docx 必须 dispatch 到 docx 处理路径,got kind={}",
            r.kind
        );
        // pandoc 装/没装两种情况都接受,但必须有明确产物或警告之一
        assert!(
            r.markdown.is_some() || r.warning.is_some(),
            "docx 路径必须产 markdown 或 warning, got both None"
        );
    }

    /// 端到端验证 pandoc 吃不下、改走 LibreOffice 的两类格式：电子表格（xlsx→CSV）
    /// 与演示（pptx→PDF→pdftotext）。自包含造测试文件（csv→soffice 得 xlsx，
    /// pandoc 从 md 得 pptx）。依赖 libreoffice+pandoc+poppler，故 `#[ignore]`；
    /// 手动 `cargo test -- --ignored office_formats`。
    #[test]
    #[ignore = "需要 libreoffice + pandoc + poppler"]
    fn office_formats_via_libreoffice_extract_text() {
        let dir = std::env::temp_dir().join(format!("pinvou3-office-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // xlsx：csv → soffice 转 xlsx，再走 ingest（应 → CSV 文本）
        let csv = dir.join("d.csv");
        std::fs::write(&csv, "姓名,部门\n张三,采购部\n").unwrap();
        let _ = Command::new("soffice")
            .args(["--headless", "--convert-to", "xlsx", "--outdir"])
            .arg(&dir)
            .arg(&csv)
            .output();
        let xlsx = dir.join("d.xlsx");
        if xlsx.exists() {
            let r = ingest(&xlsx);
            assert_eq!(r.kind, "xlsx");
            let md = r.markdown.expect("xlsx 必须转出内容");
            assert!(md.contains("采购部"), "xlsx 内容异常: {md}");
        }

        // pptx：md → pandoc 转 pptx，再走 ingest（应 → PDF → pdftotext 文本）
        let md_src = dir.join("s.md");
        std::fs::write(&md_src, "# 第一章 政务\n\n- 要点甲\n").unwrap();
        let pptx = dir.join("s.pptx");
        let _ = Command::new("pandoc")
            .arg(&md_src)
            .arg("-o")
            .arg(&pptx)
            .output();
        if pptx.exists() {
            let r = ingest(&pptx);
            assert_eq!(r.kind, "pptx");
            let md = r.markdown.expect("pptx 必须转出文字");
            assert!(md.contains("政务"), "pptx 内容异常: {md}");
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 端到端验证压缩包：造一个含中文 txt 的 zip，ingest 应解压并递归 ingest 内部
    /// 文件、把内容汇总进 markdown。依赖 7z，故 `#[ignore]`。
    #[test]
    #[ignore = "需要 7z (p7zip-full)"]
    fn archive_extracts_and_recurses_into_members() {
        let dir = std::env::temp_dir().join(format!("pinvou3-arch-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let inner = dir.join("doc.txt");
        std::fs::write(&inner, "压缩包内文档 测试内容").unwrap();
        let zip = dir.join("bundle.zip");
        let _ = Command::new("7z").arg("a").arg(&zip).arg(&inner).output();
        if zip.exists() {
            let r = ingest(&zip);
            assert_eq!(r.kind, "archive");
            let md = r.markdown.expect("压缩包必须汇总内容");
            assert!(md.contains("压缩包内文档"), "递归 ingest 内容缺失: {md}");
            assert!(md.contains("doc.txt"), "应列出成员文件名: {md}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 端到端验证 .eml：手写一封带 UTF-8 正文的邮件，ingest 应解出发件人/主题/
    /// 中文正文。依赖 python3（标准库 email），故 `#[ignore]`。
    #[test]
    #[ignore = "需要 python3"]
    fn eml_parses_headers_and_chinese_body() {
        let dir = std::env::temp_dir().join(format!("pinvou3-eml-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let eml = dir.join("m.eml");
        let raw = "From: alice@example.com\r\n\
                   To: bob@example.com\r\n\
                   Subject: Project Update\r\n\
                   Date: Mon, 27 May 2026 10:00:00 +0800\r\n\
                   Content-Type: text/plain; charset=utf-8\r\n\r\n\
                   这是邮件正文 测试内容。\r\n";
        std::fs::write(&eml, raw).unwrap();
        let r = ingest(&eml);
        assert_eq!(r.kind, "eml");
        let md = r.markdown.expect("eml 必须解析出内容");
        assert!(md.contains("alice@example.com"), "应含发件人: {md}");
        assert!(md.contains("Project Update"), "应含主题: {md}");
        assert!(md.contains("邮件正文"), "应含正文中文: {md}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 全类型端到端：遍历 /tmp/e2e_files 下预先造好的各类样本文件，逐个 ingest，
    /// 打印 [文件/kind/markdown/tokens/预览] 汇总表，并断言「预期可解析」的类型都
    /// 真的产出了 markdown。依赖全套外部工具 + 样本目录，故 `#[ignore]`。
    #[test]
    #[ignore = "全类型 e2e: 需 /tmp/e2e_files 与全套外部工具"]
    fn e2e_all_supported_types() {
        let dir = std::path::Path::new("/tmp/e2e_files");
        if !dir.exists() {
            eprintln!("跳过: 无 /tmp/e2e_files");
            return;
        }
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file())
            .collect();
        entries.sort();

        // 预期能解析出正文的扩展（其余 mp3/bin 预期只给 warning）。
        let expect_md = [
            "txt", "md", "csv", "json", "docx", "odt", "rtf", "doc", "pptx", "ppt", "xlsx", "ods",
            "xls", "png", "pdf", "zip", "7z", "eml",
        ];

        println!(
            "\n{:<14} {:<12} {:<5} {:>6}  {}",
            "文件", "kind", "md", "tokens", "warning / 内容预览"
        );
        println!("{}", "-".repeat(100));
        let mut failures = Vec::new();
        for p in &entries {
            let r = ingest(p);
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            let md_flag = if r.markdown.is_some() { "有" } else { "—" };
            let preview = match (&r.markdown, &r.warning) {
                (Some(m), _) => m.chars().take(70).collect::<String>().replace('\n', " ⏎ "),
                (None, Some(w)) => format!("⚠️ {w}"),
                _ => String::new(),
            };
            println!(
                "{:<14} {:<12} {:<5} {:>6}  {}",
                name, r.kind, md_flag, r.token_estimate, preview
            );
            if expect_md.contains(&ext.as_str()) && r.markdown.is_none() {
                failures.push(format!(
                    "{name} ({ext}) 预期产 markdown 但为空: {:?}",
                    r.warning
                ));
            }
        }
        println!("{}", "-".repeat(100));
        assert!(
            failures.is_empty(),
            "以下类型解析失败:\n{}",
            failures.join("\n")
        );
    }
}

#[cfg(test)]
mod visual_preview_smoke {
    use super::*;
    use std::process::Command;

    // 真跑 soffice/pandoc/pdftoppm，验证可视化预览两条路径产出非空 + 图片内联。
    // 依赖系统工具，CI 无则 ignore。
    #[test]
    #[ignore = "需要 libreoffice + pandoc + poppler"]
    fn office_to_inline_html_and_pdf_to_pngs() {
        let dir = std::env::temp_dir().join(format!("pinvou3-visual-smoke-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // 1) md -> docx (pandoc) -> inline html
        let md = dir.join("doc.md");
        std::fs::write(&md, "# 标题\n\n正文一段。\n\n- 列表项\n").unwrap();
        let docx = dir.join("doc.docx");
        let ok = Command::new("pandoc")
            .arg(&md)
            .arg("-o")
            .arg(&docx)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(ok && docx.exists(), "pandoc 造 docx 失败");
        let html = libreoffice_to_inline_html(&docx).expect("office->html 应成功");
        assert!(html.contains("标题"), "HTML 应含正文文字");
        assert!(
            !html.contains("src=\"doc_html"),
            "旁置图片应已内联(不应残留相对 src)"
        );

        // 2) md -> pdf (soffice via docx) -> png 页
        let pdf = dir.join("doc.pdf");
        let _ = Command::new("soffice")
            .arg(format!("-env:UserInstallation=file://{}/p", dir.display()))
            .args(["--headless", "--convert-to", "pdf", "--outdir"])
            .arg(&dir)
            .arg(&docx)
            .status();
        if pdf.exists() {
            let (imgs, _trunc) = pdf_to_png_data_uris(&pdf, 30).expect("pdf->png 应成功");
            assert!(!imgs.is_empty(), "应产出至少一页");
            assert!(
                imgs[0].starts_with("data:image/png;base64,"),
                "应为 png data URI"
            );
        }

        // 3) md -> pptx (pandoc) -> office_to_png(演示稿走 PDF→PNG)
        let pptx = dir.join("deck.pptx");
        if Command::new("pandoc")
            .arg(&md)
            .arg("-o")
            .arg(&pptx)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            let (imgs, _t) = office_to_png_data_uris(&pptx, 30).expect("pptx->png 应成功");
            assert!(
                !imgs.is_empty() && imgs[0].starts_with("data:image/png;base64,"),
                "pptx 应产出页图"
            );
        }

        // 4) csv -> xlsx (soffice) -> inline html(电子表格走 HTML 表格)
        let csv = dir.join("data.csv");
        std::fs::write(&csv, "甲,乙\n1,2\n3,4\n").unwrap();
        let _ = Command::new("soffice")
            .arg(format!("-env:UserInstallation=file://{}/p2", dir.display()))
            .args(["--headless", "--convert-to", "xlsx", "--outdir"])
            .arg(&dir)
            .arg(&csv)
            .status();
        let xlsx = dir.join("data.xlsx");
        if xlsx.exists() {
            let html = libreoffice_to_inline_html(&xlsx).expect("xlsx->html 应成功");
            assert!(
                html.contains('甲') || html.to_lowercase().contains("table"),
                "xlsx HTML 应含表格内容"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}

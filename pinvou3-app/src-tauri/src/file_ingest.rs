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
    /// 7z（p7zip-full）—— 解 zip/7z/rar 等压缩包。
    pub sevenzip: bool,
    /// python3 —— 解析 .eml 邮件（标准库 email 模块，无额外依赖）。
    pub python3: bool,
    /// msgconvert（libemail-outlook-message-perl）—— .msg → .eml。
    pub msgconvert: bool,
}

static SYSTEM_TOOLS: OnceLock<SystemTools> = OnceLock::new();

/// 启动时（或第一次 ingest 时）检测一次系统工具。
pub fn system_tools() -> SystemTools {
    *SYSTEM_TOOLS.get_or_init(|| SystemTools {
        pandoc: which("pandoc"),
        pdftotext: which("pdftotext"),
        libreoffice: which("soffice") || which("libreoffice"),
        tesseract: which("tesseract"),
        pdftoppm: which("pdftoppm"),
        sevenzip: which("7z"),
        python3: which("python3"),
        msgconvert: which("msgconvert"),
    })
}

/// tesseract 的 `-l` 语言参数。pinvou3 面向国内政企，中文是刚需，所以优先
/// `chi_sim+eng`；若没装中文包(`tesseract-ocr-chi-sim`)则降级 `eng`，不报错。
/// 探测一次缓存：跑 `tesseract --list-langs` 看输出里有没有 `chi_sim`。
fn ocr_lang_arg() -> String {
    static LANG: OnceLock<String> = OnceLock::new();
    LANG.get_or_init(|| {
        let listed = Command::new("tesseract")
            .arg("--list-langs")
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

fn which(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "tiff" | "tif" => "image",
        // 压缩包：解压后递归识别（7z 统一处理 zip/rar/7z）
        "zip" | "rar" | "7z" => "archive",
        // 邮件：eml 走 python email 标准库；msg 先 msgconvert 转 eml
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
            warning: Some("PDF 解析需要 pdftotext，请运行: sudo apt install poppler-utils".into()),
        };
    }
    // pdftotext -layout <path> -  → stdout
    let out = Command::new("pdftotext")
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
            warning: Some(format!(
                "{label} 解析需要 pandoc，请运行: sudo apt install pandoc"
            )),
        };
    }
    let out = Command::new("pandoc")
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
            warning: Some(
                "演示文稿解析需要 LibreOffice + poppler-utils: \
                 sudo apt install libreoffice poppler-utils"
                    .into(),
            ),
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
            Command::new("pdftotext")
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

/// 图片：跑 tesseract OCR 把文字抠出来。注意这是**文字识别**不是视觉理解
/// （版式 / 图形 / 颜色全丢），所以 markdown 有值时 commands.rs 仍要标注「非
/// 视觉理解」。缺 tesseract / 图里没文字 → markdown=None，退回 `model_no_vision`
/// 让上层照旧提示「模型看不到图」。
fn ingest_image(path: &Path, basename: String, path_str: String, byte_size: u64) -> IngestResult {
    let tools = system_tools();
    if !tools.tesseract {
        return IngestResult {
            kind: "image".into(),
            basename,
            path: path_str,
            markdown: None,
            token_estimate: 0,
            byte_size,
            // 仍标 model_no_vision（上层提示无视觉），附带装 OCR 的指引。
            warning: Some(
                "model_no_vision；如需识别图中文字请装 OCR: sudo apt install tesseract-ocr tesseract-ocr-chi-sim".into(),
            ),
        };
    }
    match ocr_image(path) {
        Ok(text) if !text.trim().is_empty() => {
            let tokens = estimate_tokens(&text);
            IngestResult {
                kind: "image".into(),
                basename,
                path: path_str,
                markdown: Some(text),
                token_estimate: tokens,
                byte_size,
                warning: None,
            }
        }
        // OCR 跑通但没识别到文字（纯图 / 照片）→ 退回无视觉提示。
        Ok(_) => IngestResult {
            kind: "image".into(),
            basename,
            path: path_str,
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: Some("model_no_vision".into()),
        },
        Err(e) => IngestResult {
            kind: "image".into(),
            basename,
            path: path_str,
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: Some(format!("model_no_vision；图片 OCR 失败: {e}")),
        },
    }
}

/// 对单张图片跑 tesseract，识别文字到 stdout。`tesseract <img> - -l <langs>`。
fn ocr_image(path: &Path) -> Result<String, String> {
    let lang = ocr_lang_arg();
    let out = Command::new("tesseract")
        .arg(path)
        .arg("-")
        .arg("-l")
        .arg(&lang)
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
            warning: Some(
                "PDF 无文字层（疑似扫描件），OCR 兜底需要 poppler-utils + tesseract: \
                 sudo apt install poppler-utils tesseract-ocr tesseract-ocr-chi-sim"
                    .into(),
            ),
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
    let convert = Command::new("pdftoppm")
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
        return mk_err("压缩包解析需要 7z: sudo apt install p7zip-full".into());
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

    let extract = Command::new("7z")
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
    let out = Command::new("7z")
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
/// 日期/正文/附件名；.msg（Outlook 专有 OLE）先用 msgconvert 转成 .eml 再同样处理。
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

    if !tools.python3 {
        return mk(None, Some("邮件解析需要 python3".into()));
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
    let out = Command::new("python3")
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
            "检测到音视频文件，当前未启用本地语音转录（需部署 whisper 后端）。\
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
    let canon = std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
    let home = match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h),
        Err(_) => return Err("HOME not set".into()),
    };
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
    fn ingest_image_falls_back_to_no_vision_without_text() {
        // 伪装的 png（实为文本字节）：无论本机装没装 tesseract，都 OCR 不出
        // 文字 —— 必须退回无视觉提示（markdown None + warning 以 model_no_vision
        // 开头），不能把识别失败当成功，也不能 panic。
        let tmp = std::env::temp_dir().join("pinvou3-ingest-image-test.png");
        std::fs::write(&tmp, b"fake png bytes").unwrap();
        let r = ingest(&tmp);
        std::fs::remove_file(&tmp).ok();
        assert_eq!(r.kind, "image");
        assert!(r.markdown.is_none(), "无文字图片 markdown 必须 None");
        assert!(
            r.warning.as_deref().unwrap_or("").starts_with("model_no_vision"),
            "无文字图片必须退回 model_no_vision，got warning={:?}",
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
            "txt", "md", "csv", "json", "docx", "odt", "rtf", "doc", "pptx", "ppt", "xlsx",
            "ods", "xls", "png", "pdf", "zip", "7z", "eml",
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
                failures.push(format!("{name} ({ext}) 预期产 markdown 但为空: {:?}", r.warning));
            }
        }
        println!("{}", "-".repeat(100));
        assert!(failures.is_empty(), "以下类型解析失败:\n{}", failures.join("\n"));
    }
}

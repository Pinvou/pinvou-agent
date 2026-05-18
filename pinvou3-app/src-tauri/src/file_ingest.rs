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
}

static SYSTEM_TOOLS: OnceLock<SystemTools> = OnceLock::new();

/// 启动时（或第一次 ingest 时）检测一次系统工具。
pub fn system_tools() -> SystemTools {
    *SYSTEM_TOOLS.get_or_init(|| SystemTools {
        pandoc: which("pandoc"),
        pdftotext: which("pdftotext"),
        libreoffice: which("soffice") || which("libreoffice"),
    })
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
        "docx" => ingest_pandoc(path, basename, path_str, byte_size, "docx"),
        "xlsx" => ingest_pandoc(path, basename, path_str, byte_size, "xlsx"),
        "legacy_office" => ingest_legacy_office(path, basename, path_str, byte_size),
        "image" => image_placeholder(basename, path_str, byte_size),
        _ => binary_placeholder(basename, path_str, byte_size),
    }
}

fn classify(ext: &str) -> &'static str {
    match ext {
        "txt" | "md" | "markdown" | "json" | "csv" | "yaml" | "yml" | "toml" | "xml"
        | "rs" | "py" | "js" | "ts" | "go" | "c" | "cpp" | "h" | "hpp" | "sh" | "log"
        | "ini" | "conf" | "env" | "tsv" => "text",
        "pdf" => "pdf",
        "docx" | "pptx" | "odt" => "docx",
        "xlsx" | "ods" => "xlsx",
        // Word/PPT/Excel 95-2003 老格式：pandoc 不直接吃，走 LibreOffice 转 txt
        "doc" | "ppt" | "xls" | "rtf" => "legacy_office",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "tiff" | "tif" => "image",
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
            warning: Some(
                "PDF 解析需要 pdftotext，请运行: sudo apt install poppler-utils".into(),
            ),
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

/// Word/PPT/Excel 95-2003 老格式（.doc/.ppt/.xls/.rtf）：用 LibreOffice headless 转 txt。
/// 这些格式 pandoc 不直接支持，但用户经常会上传（公司发的旧文档）。
fn ingest_legacy_office(
    path: &Path,
    basename: String,
    path_str: String,
    byte_size: u64,
) -> IngestResult {
    let tools = system_tools();
    if !tools.libreoffice {
        return IngestResult {
            kind: "legacy_office".into(),
            basename,
            path: path_str,
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: Some(
                "Word/PPT/Excel 95-2003 老格式需要 LibreOffice，请运行: sudo apt install libreoffice"
                    .into(),
            ),
        };
    }
    // 临时目录：每次 ingest 用唯一子目录，避免并发文件名冲突
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmpdir = std::env::temp_dir().join(format!("pinvou3-libreoffice-{ts}"));
    if let Err(e) = std::fs::create_dir_all(&tmpdir) {
        return IngestResult {
            kind: "legacy_office".into(),
            basename,
            path: path_str,
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: Some(format!("创建临时目录失败: {e}")),
        };
    }
    let out = Command::new("soffice")
        .arg("--headless")
        .arg("--convert-to")
        .arg("txt:Text (encoded):UTF8")
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
            let txt_path = tmpdir.join(format!("{stem}.txt"));
            match std::fs::read_to_string(&txt_path) {
                Ok(content) => {
                    let tokens = estimate_tokens(&content);
                    IngestResult {
                        kind: "legacy_office".into(),
                        basename: basename.clone(),
                        path: path_str.clone(),
                        markdown: Some(content),
                        token_estimate: tokens,
                        byte_size,
                        warning: None,
                    }
                }
                Err(e) => IngestResult {
                    kind: "legacy_office".into(),
                    basename: basename.clone(),
                    path: path_str.clone(),
                    markdown: None,
                    token_estimate: 0,
                    byte_size,
                    warning: Some(format!("LibreOffice 转换后读取失败: {e}")),
                },
            }
        }
        Ok(o) => IngestResult {
            kind: "legacy_office".into(),
            basename: basename.clone(),
            path: path_str.clone(),
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: Some(format!(
                "LibreOffice 转换失败: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            )),
        },
        Err(e) => IngestResult {
            kind: "legacy_office".into(),
            basename: basename.clone(),
            path: path_str.clone(),
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: Some(format!("LibreOffice 调用失败: {e}")),
        },
    };
    // 清理临时目录（best-effort）
    let _ = std::fs::remove_dir_all(&tmpdir);
    result
}

fn image_placeholder(basename: String, path_str: String, byte_size: u64) -> IngestResult {
    IngestResult {
        kind: "image".into(),
        basename,
        path: path_str,
        markdown: None,
        token_estimate: 0,
        byte_size,
        warning: Some("model_no_vision".into()),
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
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '|' | '"' | '?' | '*')
            {
                '_'
            } else {
                c
            }
        })
        .collect();
    if cleaned.is_empty() { "file".into() } else { cleaned }
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
        assert_eq!(classify("docx"), "docx");
        assert_eq!(classify("xlsx"), "xlsx");
        assert_eq!(classify("doc"), "legacy_office");
        assert_eq!(classify("ppt"), "legacy_office");
        assert_eq!(classify("xls"), "legacy_office");
        assert_eq!(classify("rtf"), "legacy_office");
        assert_eq!(classify("png"), "image");
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
    fn ingest_image_does_not_read_pixels() {
        // 写一个伪装的 png（实际是文本，但扩展名 png）
        let tmp = std::env::temp_dir().join("pinvou3-ingest-image-test.png");
        std::fs::write(&tmp, b"fake png bytes").unwrap();
        let r = ingest(&tmp);
        assert_eq!(r.kind, "image");
        assert!(r.markdown.is_none(), "image markdown should be None");
        assert_eq!(r.warning.as_deref(), Some("model_no_vision"));
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn validate_path_rejects_relative() {
        assert!(validate_path("relative/path.txt").is_err());
    }

    #[test]
    fn validate_path_rejects_outside_home() {
        assert!(validate_path("/etc/passwd").is_err());
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
}

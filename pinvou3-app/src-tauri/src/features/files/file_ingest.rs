//! 输入文件预处理：把用户上传的文件统一转成 markdown 文本，附到 user message
//! 让 LLM 看懂。
//!
//! 设计：
//! - 文本类(txt/md/json/csv/yaml/code) → `fs::read_to_string`
//! - PDF → `pdftotext -layout` (`poppler-utils`)
//! - docx/pptx/odt → `pandoc -t markdown`
//! - xlsx/xls/ods → calamine 读**全部工作表**逐行抽取（回退 LibreOffice CSV）
//! - 图片 → 不读像素，只标记 `model_no_vision`(配合 prompt 防臆测)
//! - 其他 → binary 占位
//!
//! 系统工具检测：启动时缓存 `which pandoc / pdftotext` 结果。缺失时返回
//! `warning: "需要安装 ..."`，前端 chip 显示，不阻塞其它格式。
//!
//! Token 估算：粗算 `chars / 1.6`（中英混合保守值）。不引 tiktoken-rs 减依赖。
//!
//! 本文件是 **facade**：只承载主入口 `ingest` 派发、共享类型 [`IngestResult`] /
//! [`MAX_FILE_BYTES`]、路径校验工具与 placeholder。各格式摄入逻辑拆到子模块：
//! - [`ingest_deps`]：系统工具探测 + 依赖体检 + 外部命令构建
//! - [`text_decode`]：纯文本摄入 + 内容嗅探 / 解码
//! - [`ingest_pdf`]：PDF 摄入
//! - [`ingest_office`]：pandoc / LibreOffice / 表格 / 演示摄入
//! - [`ingest_archive`]：压缩包摄入
//! - [`ingest_email`]：邮件摄入
//! - [`visual_preview`]：OCR + 可视化预览（base64 / data URI / 逐页 PNG）

#[path = "ingest_archive.rs"]
mod ingest_archive;
#[path = "ingest_deps.rs"]
mod ingest_deps;
#[path = "ingest_email.rs"]
mod ingest_email;
#[path = "ingest_office.rs"]
mod ingest_office;
#[path = "ingest_pdf.rs"]
mod ingest_pdf;
#[path = "text_decode.rs"]
mod text_decode;
#[path = "visual_preview.rs"]
mod visual_preview;

pub use ingest_deps::{
    check_dependencies, install_dependencies, system_tools, DependencyCheckItem, SystemTools,
};
pub use visual_preview::{
    base64_encode, image_file_to_data_uri, libreoffice_to_inline_html, ocr_image_for_kb,
    office_to_png_data_uris, pdf_to_png_data_uris,
};

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 单文件硬上限 20 MB —— 超大文件就算转 md 后 token 数也炸上下文。
/// `pub` 供 remote_control 上传链路对齐硬上限(PR #213 审查 #5:mobile upload cap 必须
/// ≤ 此值,否则 20-64MiB 文件会被 mobile 接受但 file_ingest 静默降级为 oversize 兜底)。
pub const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;

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

impl IngestResult {
    /// 缺工具/不支持时的占位结果（warning 文案，无 markdown）。
    ///
    /// 字段默认与既有字面量逐字段一致：`markdown: None`、`token_estimate: 0`、
    /// `warning: Some(warning)`，`path` 由 `path.to_string_lossy()` 还原（与各
    /// ingest 函数顶部预计算的 `path_str` 等价）。
    pub fn warning(
        kind: &str,
        basename: &str,
        path: &Path,
        byte_size: u64,
        warning: impl Into<String>,
    ) -> Self {
        IngestResult {
            kind: kind.into(),
            basename: basename.into(),
            path: path.to_string_lossy().into(),
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: Some(warning.into()),
        }
    }

    /// 已生成 markdown 的正常结果。
    ///
    /// `token_estimate` 由 `estimate_tokens(&markdown)` 推导，`warning: None`。
    /// 仅适用于「成功提取正文且无任何告警」的结果；同时带有正文和告警（如编码转换、
    /// OCR 误差提示）的结果无法用本构造器表达，仍需保留字面量。
    pub fn with_markdown(
        kind: &str,
        basename: &str,
        path: &Path,
        byte_size: u64,
        markdown: String,
    ) -> Self {
        let token_estimate = estimate_tokens(&markdown);
        IngestResult {
            kind: kind.into(),
            basename: basename.into(),
            path: path.to_string_lossy().into(),
            markdown: Some(markdown),
            token_estimate,
            byte_size,
            warning: None,
        }
    }

    /// 仅占位、无内容无警告（极少用）。`markdown: None`、`token_estimate: 0`、
    /// `warning: None`。典型用例：图片附件登记元数据（kind="image"）。
    pub fn placeholder(kind: &str, basename: &str, path: &Path, byte_size: u64) -> Self {
        IngestResult {
            kind: kind.into(),
            basename: basename.into(),
            path: path.to_string_lossy().into(),
            markdown: None,
            token_estimate: 0,
            byte_size,
            warning: None,
        }
    }
}

/// 粗算 token：中英混合按 `chars / 1.6` —— 比较保守，偏向高估避免炸上下文。
/// 实测 cl100k_base 中文 1.0-1.5 char/token、英文 3-4 char/token。
fn estimate_tokens(text: &str) -> u32 {
    let chars = text.chars().count();
    (chars as f64 / 1.6).ceil() as u32
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
            return IngestResult::warning(
                "missing",
                &basename,
                path,
                0,
                format!("文件不存在: {e}"),
            );
        }
    };
    let byte_size = meta.len();
    if byte_size > MAX_FILE_BYTES {
        return IngestResult::warning(
            "oversize",
            &basename,
            path,
            byte_size,
            format!(
                "文件 {:.1} MB 超过 20 MB 上限,请拆分或裁剪",
                byte_size as f64 / 1024.0 / 1024.0
            ),
        );
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    let kind = classify(&ext);

    match kind {
        "text" => text_decode::ingest_text(path, basename, path_str, byte_size),
        "pdf" => ingest_pdf::ingest_pdf(path, basename, path_str, byte_size),
        // 文字文档：pandoc 原生支持 docx/odt。
        "doc_pandoc" => ingest_office::ingest_pandoc(path, basename, byte_size, &ext),
        // 文字文档：pandoc 吃不下，LibreOffice 转纯文本（doc/rtf/wps）。
        "doc_office" => ingest_office::ingest_office_text(path, basename, byte_size, &ext),
        // 演示：LibreOffice 转 PDF 再 pdftotext（pptx/ppt/odp/dps）。
        "presentation" => ingest_office::ingest_presentation(path, basename, byte_size, &ext),
        // 表格：LibreOffice 转 CSV（xlsx/ods/xls/et）—— pandoc 不支持表格输入。
        "spreadsheet" => {
            ingest_office::ingest_spreadsheet(path, basename, path_str, byte_size, &ext)
        }
        "image" => visual_preview::ingest_image(path, basename, path_str, byte_size),
        "archive" => ingest_archive::ingest_archive(path, basename, path_str, byte_size),
        "email" => ingest_email::ingest_email(path, basename, path_str, byte_size, &ext),
        "media" => media_placeholder(basename, path_str, byte_size),
        // 私钥 / 密钥库:绝不读正文(防泄露给 LLM)。见 classify 的 "secret" 分支。
        "secret" => secret_placeholder(basename, path_str, byte_size),
        // 未知扩展名 / 无扩展名:不再一刀切 binary,先内容嗅探。是文本就当文本读
        // (让模型看到内容 —— 这是「接受任何文件」的根本解),真二进制才降级。
        _ => text_decode::sniff_text_or_binary(path, basename, path_str, byte_size),
    }
}

/// 按「文档类型」而非新旧分流：pandoc 只吃 docx/odt，pptx/ppt/xlsx/ods/xls 一律
/// 走 LibreOffice（演示→PDF→pdftotext，表格→CSV）。WPS 三件套按用途归类：
/// .wps→文字、.et→表格、.dps→演示。
fn classify(ext: &str) -> &'static str {
    match ext {
        // 安全硬墙(优先级最高):私钥 / 密钥库类扩展名一律不读正文 —— 防止私钥被读进
        // markdown、内联进 prompt、进而发给(可能的云端)LLM。validate_path 只挡 5 个
        // 敏感目录,挡不住 ~/certs/server.key 这类文件,故在此补文件级拒绝。
        // 走 "secret" 分类 → secret_placeholder(markdown 恒为 None),不进 sniff。
        // 注意:公钥证书(crt/cer/csr)本身可公开,仍归 text,方便用户问「证书 CN/到期日」。
        "key" | "pem" | "p12" | "pfx" | "keystore" | "jks" | "kdbx" | "gpg" | "pgp" => "secret",
        // 纯文本:文档 + 结构化数据 + 源码 + Web + 配置 + 公钥证书(均为 UTF-8/文本可读)。
        // 未列出的扩展名由 ingest 末尾 sniff_text_or_binary 内容嗅探兜底,故这里只列
        //「确定是文本」的常见格式,不追求穷举 —— 嗅探会接住遗漏(含用户自定义扩展名、
        // 以及无扩展名的 Makefile/Dockerfile/README —— 这些文件 Path::extension() 返回
        // None,根本不会进 classify,故不在此列「扩展名」)。
        "txt" | "md" | "markdown" | "mdx" | "rst" | "org" | "adoc" | "asciidoc" | "tex"
        | "latex" | "bib" | "text" | "log" | "json" | "jsonl" | "ndjson" | "geojson" | "csv"
        | "tsv" | "yaml" | "yml" | "toml" | "xml" | "proto" | "graphql" | "gql" | "sql" | "ini"
        | "conf" | "cfg" | "properties" | "props" | "env" | "editorconfig" | "gradle" | "cmake"
        | "bazel" | "bzl" | "mk" | "rs" | "py" | "pyi" | "js" | "mjs" | "cjs" | "ts" | "jsx"
        | "tsx" | "go" | "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" | "java"
        | "kt" | "kts" | "scala" | "groovy" | "clj" | "cljs" | "edn" | "el" | "lisp" | "scm"
        | "rkt" | "r" | "rb" | "php" | "pl" | "pm" | "lua" | "tcl" | "m" | "mm" | "swift"
        | "dart" | "hs" | "lhs" | "ml" | "mli" | "fs" | "fsx" | "fsi" | "cs" | "vb" | "pas"
        | "d" | "nim" | "zig" | "v" | "jl" | "ex" | "exs" | "erl" | "hrl" | "f" | "f90" | "f95"
        | "f03" | "asm" | "s" | "vhdl" | "sv" | "sh" | "bash" | "zsh" | "fish" | "ps1" | "bat"
        | "cmd" | "html" | "htm" | "xhtml" | "css" | "scss" | "sass" | "less" | "styl" | "vue"
        | "svelte" | "pug" | "hbs" | "ejs" | "twig" | "crt" | "cer" | "csr" | "wsgi" | "rake" => {
            "text"
        }
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
        // svg(矢量)/tiff 不在支持列表 → 不归 image(避免被当图暂存后 image_analyze 报
        // "Unsupported image format")。它们经 `_ =>` 落到 sniff 兜底:SVG 是文本 XML,
        // 会被读成 text(模型能看懂结构,无害且有用);TIFF 头含 NUL(II*\0 / MM\0*),
        // sniff 判二进制 → binary_placeholder。
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => "image",
        // 压缩包：解压后递归识别（7z 统一处理 zip/rar/7z）
        "zip" | "rar" | "7z" => "archive",
        // 邮件：eml 走 python email 标准库；msg 按 OS 策略解析
        "eml" | "msg" => "email",
        // 音视频：本地语音转录(whisper)尚未部署，先优雅降级标「未处理」
        "mp4" | "avi" | "mov" | "mkv" | "webm" | "flv" | "wmv" | "m4v" | "mp3" | "wav" | "m4a"
        | "aac" | "flac" | "ogg" | "wma" => "media",
        _ => "binary",
    }
}

/// 音视频：本地语音转录（whisper 等）尚未部署到 GB10，先优雅降级，明确告知用户
/// 「未处理」而非臆测内容。真正转录留作未来独立能力（见 process.md）。
fn media_placeholder(basename: String, path_str: String, byte_size: u64) -> IngestResult {
    IngestResult::warning(
        "media",
        &basename,
        Path::new(&path_str),
        byte_size,
        "检测到音视频文件，当前暂不支持本地语音转录。\
             可改为提供文字稿，或口述其中要点。",
    )
}

fn binary_placeholder(basename: String, path_str: String, byte_size: u64) -> IngestResult {
    IngestResult::warning(
        "binary",
        &basename,
        Path::new(&path_str),
        byte_size,
        "不支持的文件类型(二进制)",
    )
}

/// 私钥 / 密钥库文件的占位:与 binary_placeholder 同为 markdown=None,但文案明确说明
/// 「为防泄露而拒绝读取」,便于用户理解为什么内容没被读入。kind 用 "binary" 以复用
/// 前端既有分类展示(无需新增前端分支),靠 warning 区分原因。
fn secret_placeholder(basename: String, path_str: String, byte_size: u64) -> IngestResult {
    IngestResult::warning(
        "binary",
        &basename,
        Path::new(&path_str),
        byte_size,
        "检测到密钥/私钥文件,为防止泄露给 LLM 已拒绝读取内容",
    )
}

/// 把剪贴板粘贴的图片 bytes 写到 `~/.pinvou3/pastes/<timestamp>-<sanitized_name>`。
/// 只用于「Ctrl+V 粘贴图片」——磁盘上没有原 path 的场景。
/// 文件选择器直接拿原 path；HTML5 拖拽使用独立的有界分块摄入，不调用这个。
pub fn save_paste_image(filename: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!(
            "图片 {:.1} MB 超过 20 MB 上限",
            bytes.len() as f64 / 1024.0 / 1024.0
        ));
    }
    let pastes = crate::platform::os::user_home_dir()
        .join(".pinvou3")
        .join("pastes");
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

/// 供远程上传落盘使用的文件名清洗：在 `sanitize_filename` 基础上兜底拒绝
/// `.` / `..`，保证 join 后仍留在上传暂存目录内。
pub(crate) fn sanitize_upload_filename(raw: &str) -> String {
    let cleaned = sanitize_filename(raw);
    if cleaned == "." || cleaned == ".." {
        "file".into()
    } else {
        cleaned
    }
}

/// 把文件名做 sanitize：去掉路径分隔符、控制字符；保留中英文 + 常见标点。
fn sanitize_filename(raw: &str) -> String {
    let trimmed = raw.rsplit(['/', '\\']).next().unwrap_or("file");
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

/// 校验上传路径：必须绝对 + 指向普通文件 + 满足当前系统的上传位置策略 + 不在敏感目录。
/// 跟 commands::validate_user_path 同语义，单独抽出供前端 ingest 入口调用。
pub fn validate_path(raw: &str) -> Result<PathBuf, String> {
    let canon = validate_browsable_path(raw)?;
    if !canon.is_file() {
        return Err(format!("path {} is not a file", canon.display()));
    }
    Ok(canon)
}

/// 校验可由桌面端文件浏览器展示的现有路径。与 `validate_path` 共享相同的
/// 位置和敏感组件限制，但允许普通文件和目录；真正摄入附件时仍由
/// `validate_path` 强制要求普通文件。
///
/// Wave 3 收紧：原先只挡 5 个敏感目录（.ssh/.gnupg/.aws/.docker/.kube），
/// 现委托 `path_policy::check_sensitive_components` 挡完整的凭据组件/前缀
/// 黑名单（含 id_rsa/.env/credentials.json/.password-store 等文件级凭据）。
/// 上传位置约束（$HOME / 非 system-root）仍由 `validate_upload_location` 保留。
pub(crate) fn validate_browsable_path(raw: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(raw);
    if !p.is_absolute() {
        return Err(format!("path must be absolute: {raw}"));
    }
    let canon = normalize_validated_path(&std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone()));
    std::fs::metadata(&canon)
        .map_err(|e| format!("path {} is not readable: {e}", canon.display()))?;
    crate::platform::os::validate_upload_location(&canon)?;
    crate::platform::path_policy::check_sensitive_components(&canon)?;
    Ok(canon)
}

fn normalize_validated_path(path: &Path) -> PathBuf {
    crate::platform::os::platform_compat_path(&path.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// RAII 守卫:在 `$HOME` 下创建 PID + 唯一后缀的专属目录,Drop 时仅清理该目录。
    ///
    /// `validate_upload_location` 要求路径位于 `$HOME` 下,而 `check_sensitive_components`
    /// 按路径组件(如 `id_rsa` / `.env`)判定,与目录位置无关。因此把测试文件放进
    /// `$HOME/.pinvou3-file-ingest-test-<pid>-<rand>/` 即可同时满足两者,且绝不触碰
    /// 开发者 `$HOME` 下可能存在的真实 `~/keys/id_rsa`、`~/project/.env`。
    /// Drop 时整目录删除(含 panic 路径),不会遗留文件。
    struct ScopedHomeDir {
        dir: std::path::PathBuf,
    }

    impl ScopedHomeDir {
        /// 在 `$HOME` 下创建 `subdir` 子目录(如 `keys`),返回其完整路径。
        fn subdir(&self, subdir: &str) -> std::path::PathBuf {
            let p = self.dir.join(subdir);
            std::fs::create_dir_all(&p).unwrap();
            p
        }
    }

    impl Drop for ScopedHomeDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn scoped_home_dir(label: &str) -> ScopedHomeDir {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nonce = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = crate::platform::os::user_home_dir().join(format!(
            ".pinvou3-file-ingest-test-{label}-{}-{}",
            std::process::id(),
            nonce
        ));
        std::fs::create_dir_all(&dir).unwrap();
        ScopedHomeDir { dir }
    }

    #[test]
    fn ingest_result_constructors_match_equivalent_literals() {
        // 构造器产出必须与等价字面量逐字段相等（含 None 默认值）。
        // 任一字段默认值被构造器静默改变即视为 bug。
        let tmp = std::env::temp_dir().join("pinvou3-ctor-equiv-test.txt");
        std::fs::write(&tmp, "hello").unwrap();
        let basename = "ctor-equiv-test.txt".to_string();
        let path_str = tmp.to_string_lossy().to_string();

        // warning()：markdown None、token_estimate 0、warning Some。
        let via_ctor = IngestResult::warning("text", &basename, &tmp, 5, "缺工具");
        let literal = IngestResult {
            kind: "text".into(),
            basename: basename.clone(),
            path: path_str.clone(),
            markdown: None,
            token_estimate: 0,
            byte_size: 5,
            warning: Some("缺工具".into()),
        };
        assert_eq!(via_ctor.kind, literal.kind);
        assert_eq!(via_ctor.basename, literal.basename);
        assert_eq!(
            via_ctor.path, literal.path,
            "path 必须由 to_string_lossy 还原"
        );
        assert_eq!(via_ctor.markdown, literal.markdown);
        assert_eq!(via_ctor.token_estimate, literal.token_estimate);
        assert_eq!(via_ctor.byte_size, literal.byte_size);
        assert_eq!(via_ctor.warning, literal.warning);

        // with_markdown()：markdown Some、token_estimate=estimate_tokens、warning None。
        let md = "# 标题\n正文内容".to_string();
        let via_ctor = IngestResult::with_markdown("pdf", &basename, &tmp, 5, md.clone());
        let literal = IngestResult {
            kind: "pdf".into(),
            basename: basename.clone(),
            path: path_str.clone(),
            markdown: Some(md.clone()),
            token_estimate: estimate_tokens(&md),
            byte_size: 5,
            warning: None,
        };
        assert_eq!(via_ctor.kind, literal.kind);
        assert_eq!(via_ctor.path, literal.path);
        assert_eq!(via_ctor.markdown, literal.markdown);
        assert_eq!(via_ctor.token_estimate, literal.token_estimate);
        assert_eq!(via_ctor.byte_size, literal.byte_size);
        assert_eq!(via_ctor.warning, literal.warning);

        // placeholder()：markdown None、token_estimate 0、warning None。
        let via_ctor = IngestResult::placeholder("image", &basename, &tmp, 5);
        assert_eq!(via_ctor.kind, "image");
        assert_eq!(via_ctor.basename, basename);
        assert_eq!(via_ctor.path, path_str);
        assert!(via_ctor.markdown.is_none());
        assert_eq!(via_ctor.token_estimate, 0);
        assert_eq!(via_ctor.byte_size, 5);
        assert!(via_ctor.warning.is_none());

        std::fs::remove_file(&tmp).ok();
    }

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
        // Fix B:扩充的纯文本格式(源码 / Web / 配置 / 数据 / 公钥证书)必须归 text。
        // 未列出扩展名由 sniff_text_or_binary 兜底,但列出的必须直接走 text(不嗅探,
        // 因为扩展名已明确是文本)。注意:Dockerfile/Makefile/.gitignore 是**文件名**而非
        // 扩展名(Path::extension 对它们返回 None),不在此列 —— 它们走 sniff 兜底,
        // 见 ingest_no_extension_text_is_read_via_sniff。
        assert_eq!(classify("java"), "text");
        assert_eq!(classify("kt"), "text");
        assert_eq!(classify("rb"), "text");
        assert_eq!(classify("php"), "text");
        assert_eq!(classify("swift"), "text");
        assert_eq!(classify("sql"), "text");
        assert_eq!(classify("html"), "text");
        assert_eq!(classify("css"), "text");
        assert_eq!(classify("scss"), "text");
        assert_eq!(classify("vue"), "text");
        assert_eq!(classify("svelte"), "text");
        assert_eq!(classify("jsx"), "text");
        assert_eq!(classify("tsx"), "text");
        assert_eq!(classify("proto"), "text");
        assert_eq!(classify("graphql"), "text");
        assert_eq!(classify("gradle"), "text");
        assert_eq!(classify("cmake"), "text");
        assert_eq!(classify("jsonl"), "text");
        assert_eq!(classify("mdx"), "text");
        assert_eq!(classify("tex"), "text");
        // 公钥证书可公开 → text;私钥 / 密钥库 → secret(绝不读正文)。
        assert_eq!(classify("crt"), "text");
        assert_eq!(classify("cer"), "text");
        assert_eq!(classify("csr"), "text");
        assert_eq!(classify("pem"), "secret");
        assert_eq!(classify("key"), "secret");
        assert_eq!(classify("p12"), "secret");
        assert_eq!(classify("gpg"), "secret");
        // 仍然未知 → binary 分类(但 ingest 会再嗅探,见 sniff 测试)。
        assert_eq!(classify("customxyz"), "binary");
        assert_eq!(classify("webp"), "image");
    }

    #[test]
    fn estimate_tokens_grows_with_content() {
        let small = estimate_tokens("hi");
        let big = estimate_tokens(&"x".repeat(1000));
        assert!(big > small);
        assert!(big < 1000); // 不应大于字符数
    }

    // ── Fix C:内容嗅探(接受任何文件)回归 ──
    // 旧实现:未知 / 无扩展名 → binary_placeholder → markdown:None,模型完全读不到。
    // 新实现:先嗅探,文本就当文本读,真二进制才降级。这满足「用户应能上传任何文件」。
    fn write_tmp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("pinvou3-sniff-{}-{name}", std::process::id()));
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn ingest_unknown_extension_text_falls_back_to_text_via_sniff() {
        // 自定义扩展名 .myfmt:不在 classify 表 → binary 分类 → 嗅探后当文本读。
        let marker = "PINVOU3-SNIFF-UNKNOWN-EXT-a1b2";
        let p = write_tmp("notes.myfmt", format!("{marker}\n第二行中文").as_bytes());
        let r = ingest(&p);
        std::fs::remove_file(&p).ok();
        assert_eq!(
            r.kind, "text",
            "未知扩展名的纯文本应被嗅探成 text,而非 binary"
        );
        assert!(
            r.markdown.as_deref().unwrap_or("").contains(marker),
            "嗅探成文本后 markdown 必须含正文 marker"
        );
        assert!(
            r.warning.is_none(),
            "合法 UTF-8 文本不应有 warning,实际={:?}",
            r.warning
        );
    }

    #[test]
    fn ingest_no_extension_text_is_read_via_sniff() {
        // 无扩展名(Makefile / Dockerfile / README 风格):file_name 无 ext。
        let marker = "PINVOU3-NOEXT-MARKER-c3d4";
        let p = write_tmp("Makefile", format!("build:\n\t{marker}\n").as_bytes());
        let r = ingest(&p);
        std::fs::remove_file(&p).ok();
        assert_eq!(r.kind, "text", "无扩展名文本应被嗅探成 text");
        assert!(r.markdown.as_deref().unwrap_or("").contains(marker));
    }

    #[test]
    fn ingest_real_binary_with_unknown_ext_falls_to_binary() {
        // 真二进制内容(含 NUL)+ 未知扩展名:必须降级 binary,不强行 lossy。
        let mut bin = vec![0u8; 512];
        for (i, b) in bin.iter_mut().enumerate() {
            *b = (i % 256) as u8; // 含 NUL + 全字节范围
        }
        let p = write_tmp("blob.dat", &bin);
        let r = ingest(&p);
        std::fs::remove_file(&p).ok();
        assert_eq!(r.kind, "binary", "含 NUL 的二进制文件应降级 binary");
        assert!(r.markdown.is_none(), "二进制不得强行 lossy 还原成文本");
    }

    #[test]
    fn ingest_gbk_text_is_decoded_to_real_chinese_with_warning() {
        // GBK 编码中文必须真正转成 Unicode，不能用 lossy 把正文变成 U+FFFD。
        let mut bytes = b"GBK-tail: ".to_vec();
        // "中文测试" 的 GBK 编码:D6 D0 CE C4 B2 E2 CA D4(均非合法 UTF-8 续位)
        bytes.extend_from_slice(&[0xD6, 0xD0, 0xCE, 0xC4, 0xB2, 0xE2, 0xCA, 0xD4]);
        for name in ["gbk_notes.txt", "gbk_notes.custom"] {
            let p = write_tmp(name, &bytes);
            let r = ingest(&p);
            std::fs::remove_file(&p).ok();
            assert_eq!(r.kind, "text", "{name} 应解码成 text");
            let md = r.markdown.expect("GBK 文本应解码出正文");
            assert!(
                md.contains("GBK-tail: 中文测试"),
                "{name} 的 GBK 中文必须完整还原,实际={md:?}"
            );
            assert!(!md.contains('\u{fffd}'), "GBK 解码不应产生替换符");
            assert!(
                r.warning.as_deref().unwrap_or("").contains("GB18030/GBK"),
                "{name} 应标编码转换 warning,实际={:?}",
                r.warning
            );
        }
    }

    // ── 私钥安全硬墙回归 ──
    // 私钥扩展名走 secret 分类(绝不读正文),改名/无扩展名的私钥由内容侧
    // looks_like_secret_material 拦截;公钥不被误伤。
    #[test]
    fn ingest_secret_extension_key_is_blocked() {
        let body = b"-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANB\n-----END PRIVATE KEY-----\n";
        let p = write_tmp("server.key", body);
        let r = ingest(&p);
        std::fs::remove_file(&p).ok();
        assert_eq!(
            r.kind, "binary",
            ".key 必须走 secret(=binary)分类,不得读正文"
        );
        assert!(
            r.markdown.is_none(),
            "私钥正文绝不能进 markdown,实际={:?}",
            r.markdown
        );
        assert!(
            r.warning.as_deref().unwrap_or("").contains("私钥"),
            "应给出「拒绝读取私钥」类提示,实际={:?}",
            r.warning
        );
    }

    #[test]
    fn ingest_renamed_private_key_blocked_by_content() {
        // 无扩展名 + OpenSSH 私钥头:走 sniff → 内容侧拦截。
        let openssh = b"-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEA\n-----END OPENSSH PRIVATE KEY-----\n";
        let p1 = write_tmp("id_rsa_test", openssh);
        let r1 = ingest(&p1);
        std::fs::remove_file(&p1).ok();
        assert_eq!(r1.kind, "binary", "无扩展名私钥应由内容嗅探拦截");
        assert!(r1.markdown.is_none(), "私钥正文不得进 markdown");

        // 套了 .txt 外壳的 RSA 私钥:走 ingest_text → 内容侧拦截。
        let rsa =
            b"-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n-----END RSA PRIVATE KEY-----\n";
        let p2 = write_tmp("backup.txt", rsa);
        let r2 = ingest(&p2);
        std::fs::remove_file(&p2).ok();
        assert_eq!(r2.kind, "binary", ".txt 套壳私钥应由内容嗅探拦截");
        assert!(r2.markdown.is_none(), "私钥正文不得进 markdown");

        // 公钥(id_rsa.pub 内容 / 纯 ssh-ed25519 ASCII)不得被误伤 —— 仍可读。
        let pub_key = b"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFakePublicKey user@host\n";
        let p3 = write_tmp("id_ed25519.pub", pub_key);
        let r3 = ingest(&p3);
        std::fs::remove_file(&p3).ok();
        assert_eq!(r3.kind, "text", "公钥不是私钥,应正常读取");
        assert!(
            r3.markdown.as_deref().unwrap_or("").contains("ssh-ed25519"),
            "公钥正文应被读出"
        );
    }

    // ── Fix E:编码兜底(BOM / UTF-16)回归 ──
    #[test]
    fn decode_strips_utf8_bom_and_reads_content() {
        // UTF-8 BOM(EF BB BF → U+FEFF)不应污染正文开头。
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"BOM-should-be-stripped");
        let p = write_tmp("with_bom.txt", &bytes);
        let r = ingest(&p);
        std::fs::remove_file(&p).ok();
        assert_eq!(r.kind, "text");
        let md = r.markdown.expect("BOM 文件应读出文本");
        assert!(
            !md.starts_with('\u{feff}'),
            "开头 BOM 必须去掉,实际首字符={:?}",
            md.chars().next()
        );
        assert!(md.contains("BOM-should-be-stripped"));
    }

    #[test]
    fn decode_utf16_bom_converts_content_instead_of_nul_garbage() {
        // UTF-16 LE 的 ASCII「hi」= 68 00 69 00:每字节 <0x80 恰为合法 UTF-8,
        // from_utf8 会成功产出 "h\0i\0"(含 NUL 的垃圾)。修复后见 BOM 转成 UTF-8。
        let bytes = [0xFFu8, 0xFE, 0x68, 0x00, 0x69, 0x00];
        let p = write_tmp("utf16le.txt", &bytes);
        let r = ingest(&p);
        std::fs::remove_file(&p).ok();
        assert_eq!(r.kind, "text", "UTF-16 文件仍归 text 分类");
        assert_eq!(r.markdown.as_deref(), Some("hi"));
        assert!(
            r.warning.as_deref().unwrap_or("").contains("UTF-16"),
            "应明确提示发生 UTF-16 转换,实际={:?}",
            r.warning
        );
    }

    #[test]
    fn decode_utf16_without_bom_uses_strong_nul_signal_for_both_endians() {
        let cases: [(&str, &[u8]); 2] = [
            (
                "utf16le-no-bom.txt",
                &[0x68, 0x00, 0x69, 0x00, 0x2D, 0x4E, 0x87, 0x65],
            ),
            // 未知扩展名也必须在 binary 嗅探前识别 UTF-16 BE。
            (
                "utf16be-no-bom.custom",
                &[0x00, 0x68, 0x00, 0x69, 0x4E, 0x2D, 0x65, 0x87],
            ),
        ];
        for (name, bytes) in cases {
            let p = write_tmp(name, bytes);
            let r = ingest(&p);
            std::fs::remove_file(&p).ok();
            assert_eq!(r.kind, "text", "{name} 应识别为 UTF-16 文本");
            assert_eq!(
                r.markdown.as_deref(),
                Some("hi中文"),
                "{name} 应完整解码中英文"
            );
            assert!(
                r.warning.as_deref().unwrap_or("").contains("无 BOM UTF-16"),
                "{name} 应提示无 BOM UTF-16 转换,实际={:?}",
                r.warning
            );
        }
    }

    #[test]
    fn sniff_reads_svg_xml_as_text() {
        // SVG 是文本 XML:经 `_ =>` sniff → 非二进制 → 读成 text(模型能看懂结构)。
        // 这是 PR 通用化的有意行为(classify 仍返回 binary,但 dispatch 末尾 sniff 兜底)。
        // 锁定该行为,防回归。
        let svg =
            b"<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\"><rect/></svg>\n";
        let p = write_tmp("icon.svg", svg);
        let r = ingest(&p);
        std::fs::remove_file(&p).ok();
        assert_eq!(r.kind, "text", "SVG(XML)应被嗅探成 text");
        assert!(r.markdown.as_deref().unwrap_or("").contains("<svg"));
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
    fn validate_browsable_path_rejects_credential_file() {
        // Wave 3 收紧：id_rsa 不在任何敏感目录里，但文件名本身是凭据。
        // 必须先创建文件，使路径通过 metadata 检查，从而真正命中的是
        // check_sensitive_components 的组件黑名单（而非 "not readable"）。
        //
        // 用 ScopedHomeDir 在 $HOME 下开 PID+nonce 专属目录，绝不触碰开发者
        // 真实的 ~/keys/id_rsa；Drop 时（含 panic）整目录清理。
        let scope = scoped_home_dir("cred");
        let dir = scope.subdir("keys");
        let id_rsa = dir.join("id_rsa");
        std::fs::write(&id_rsa, b"dummy").unwrap();
        let result = validate_browsable_path(id_rsa.to_str().unwrap());
        let err = result.expect_err("id_rsa outside .ssh should be rejected");
        assert!(
            err.contains("sensitive"),
            "should fail on sensitive component, got: {err}"
        );
    }

    #[test]
    fn validate_browsable_path_rejects_env_file() {
        // 同上：先创建 .env 文件，确保拒绝原因来自组件黑名单。
        let scope = scoped_home_dir("env");
        let dir = scope.subdir("project");
        let env_file = dir.join(".env");
        std::fs::write(&env_file, b"dummy").unwrap();
        let result = validate_browsable_path(env_file.to_str().unwrap());
        let err = result.expect_err(".env file should be rejected");
        assert!(
            err.contains("sensitive"),
            "should fail on sensitive component, got: {err}"
        );
    }

    #[test]
    fn validate_browsable_path_accepts_adjacent_normal_file() {
        // 正向用例：同一隔离目录下的普通文件（非凭据组件）必须通过校验，
        // 证明上面两个拒绝用例确实命中的是组件黑名单，而非「任何文件都失败」。
        let scope = scoped_home_dir("ok");
        let dir = scope.subdir("project");
        let normal = dir.join("notes.md");
        std::fs::write(&normal, b"# notes").unwrap();
        let result = validate_browsable_path(normal.to_str().unwrap());
        assert!(
            result.is_ok(),
            "adjacent normal file should be accepted: {:?}",
            result.err()
        );
    }

    #[test]
    fn validate_path_accepts_windows_canonicalized_home_file() {
        if !crate::platform::capabilities::is_windows() {
            return;
        }
        let home = crate::platform::os::user_home_dir();
        let file = home.join(format!("pinvou3-validate-path-{}.txt", std::process::id()));
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
    fn windows_invalid_msg_returns_warning_without_msgconvert_dependency() {
        if !crate::platform::capabilities::is_windows() {
            return;
        }
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

    /// 端到端 OCR 实测：依赖本机装了 tesseract + chi_sim，且 /tmp 下有用
    /// PIL 造的中文测试图/扫描件 PDF（见 PR 说明的造图脚本）。常规 CI 无这些
    /// 前置，故 `#[ignore]`；手动 `cargo test -- --ignored ocr_extracts_chinese`。
    /// 验证两条真实代码路径：图片直 OCR + 扫描件 PDF（pdftotext 空→pdftoppm→OCR）。
    #[test]
    #[ignore = "需要本机 tesseract+chi_sim 与 /tmp 测试文件"]
    fn ocr_extracts_chinese_from_image_and_scanned_pdf() {
        if !ingest_deps::system_tools().tesseract {
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
    /// 中文正文。依赖 Python（标准库 email），故 `#[ignore]`。
    #[test]
    #[ignore = "需要 Python"]
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
            "\n{:<14} {:<12} {:<5} {:>6}  warning / 内容预览",
            "文件", "kind", "md", "tokens",
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
mod multi_type_dispatch_e2e {
    use super::*;

    // PR #213 审查 #1 端到端回归:验证 file_ingest::ingest 按磁盘路径扩展名把
    // txt/pdf/docx/xlsx/png 分别派发到正确处理路径,而不是全部落到 binary 兜底。
    // 旧实现把上传文件统一落盘成 data.bin(扩展名 bin),所有类型都进 binary →
    // "不支持的文件类型";Fix #1 保留原始扩展名后,ingest 必须对每个类型识别成功。
    //
    // 每种类型用唯一标记字符串,断言它出现在 ingest 结果里(text/markdown 正文、
    // pdf 的 markdown、docx 的 pandoc markdown、xlsx 的 calamine 表格、png 的
    // image data-URI kind)。工具缺失时跳过(不 FAIL),与 visual_preview_smoke 同策略。
    // 这是评审要求的「真实文件类型不被丢失」等价证据 —— 不经过浏览器,直接证明
    // ingest 分派层对真实扩展名生效(small.txt 全链路经 pinvou3-app 的 WebUI v2
    // smoke 上传场景覆盖 → "已就绪")。
    #[test]
    #[ignore = "需要 pandoc + pdftotext + libreoffice + tesseract(本机齐全时跑)"]
    fn ingest_dispatches_each_real_type_by_extension_not_binary() {
        let tools = ingest_deps::system_tools();
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-multi-type-e2e-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = "PINVOU3-MULTITYPE-MARKER-9f3a";

        // ── txt:text 分派,正文含 marker ──
        let txt = dir.join("note.txt");
        std::fs::write(&txt, format!("{marker} 文本正文")).unwrap();
        let ir = ingest(&txt);
        assert_eq!(ir.kind, "text", "txt 必须走 text 分派");
        assert!(
            ir.markdown.as_deref().unwrap_or("").contains(marker),
            "txt 正文必须含 marker"
        );

        // ── docx:pandoc 分派,正文含 marker(需 pandoc) ──
        if tools.pandoc {
            let md = dir.join("src.md");
            std::fs::write(&md, format!("# {marker}\n\ndocx 正文段落。")).unwrap();
            let docx = dir.join("src.docx");
            let ok = std::process::Command::new("pandoc")
                .arg(&md)
                .arg("-o")
                .arg(&docx)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok && docx.exists() {
                let ir = ingest(&docx);
                // doc_pandoc 分派把 kind 设为扩展名标签(ingest_pandoc 的 label=ext),
                // 所以 kind 应是 "docx"(不是 binary 也不是 "doc_pandoc" 类别名)。
                assert_ne!(
                    ir.kind, "binary",
                    "docx 不得落到 binary 兜底(扩展名保留生效的证明)"
                );
                assert!(
                    ir.markdown.as_deref().unwrap_or("").contains(marker),
                    "docx markdown 必须含 marker,实际 kind={} warning={:?}",
                    ir.kind,
                    ir.warning
                );
            }
        }

        // ── xlsx:spreadsheet(calamine)分派,单元格含 marker ──
        // 用 csv → soffice 转 xlsx 造真实表格文件。
        if tools.libreoffice {
            let csv = dir.join("sheet.csv");
            std::fs::write(&csv, format!("A,B\n{marker},2\n")).unwrap();
            let _ = std::process::Command::new("soffice")
                .arg(format!("-env:UserInstallation=file://{}/lo", dir.display()))
                .args(["--headless", "--convert-to", "xlsx", "--outdir"])
                .arg(&dir)
                .arg(&csv)
                .status();
            let xlsx = dir.join("sheet.xlsx");
            if xlsx.exists() {
                let ir = ingest(&xlsx);
                // spreadsheet 分派把 kind 设为扩展名标签(ingest_spreadsheet 的 kind=ext)。
                assert_ne!(
                    ir.kind, "binary",
                    "xlsx 不得落到 binary 兜底(扩展名保留生效的证明)"
                );
                assert!(
                    ir.markdown.as_deref().unwrap_or("").contains(marker),
                    "xlsx 表格抽取必须含 marker,实际 kind={} warning={:?}",
                    ir.kind,
                    ir.warning
                );
            }
        }

        // ── pdf:pdf 分派,正文含 marker(用 soffice 把含 marker 的文档转 pdf) ──
        if tools.pdftotext && tools.libreoffice {
            let md = dir.join("pdfsrc.md");
            std::fs::write(&md, format!("# {marker}\n\nPDF 正文。")).unwrap();
            // md → docx → pdf
            let docx = dir.join("pdfsrc.docx");
            let _ = std::process::Command::new("pandoc")
                .arg(&md)
                .arg("-o")
                .arg(&docx)
                .status();
            if docx.exists() {
                let _ = std::process::Command::new("soffice")
                    .arg(format!(
                        "-env:UserInstallation=file://{}/lo2",
                        dir.display()
                    ))
                    .args(["--headless", "--convert-to", "pdf", "--outdir"])
                    .arg(&dir)
                    .arg(&docx)
                    .status();
                let pdf = dir.join("pdfsrc.pdf");
                if pdf.exists() {
                    let ir = ingest(&pdf);
                    assert_eq!(ir.kind, "pdf", "pdf 必须走 pdf 分派");
                    assert!(
                        ir.markdown.as_deref().unwrap_or("").contains(marker),
                        "pdf 抽取文本必须含 marker,实际 kind={} warning={:?}",
                        ir.kind,
                        ir.warning
                    );
                }
            }
        }

        // ── png:image 分派 ──
        // 对话附件图片走视觉(image_analyze),ingest_image 只登记元数据(kind="image",
        // markdown=None),不产 data-URI(见 ingest_image 注释)。所以这里只断言
        // kind=="image" —— 关键是它**没**被降级成 binary 兜底(旧 data.bin 的结果)。
        // PNG 分派不依赖任何外部工具,无条件运行(故不走 `if tools.xxx` 守卫)。
        // 1x1 灰度 PNG(像素值 0xAA),67 字节:签名 + IHDR + IDAT + IEND,
        // CRC/IDAT 均由 zlib 正确生成,PIL 实测可解码。
        let min_png: Vec<u8> = [
            0x89u8, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x00, 0x00, 0x00,
            0x00, 0x3A, 0x7E, 0x9B, 0x55, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78,
            0xDA, 0x63, 0x58, 0x05, 0x00, 0x00, 0xAC, 0x00, 0xAB, 0xCB, 0x83, 0x9E, 0xE6, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ]
        .to_vec();
        let png = dir.join("pixel.png");
        std::fs::write(&png, &min_png).unwrap();
        let ir = ingest(&png);
        assert_eq!(ir.kind, "image", "png 必须走 image 分派(未被降级成 binary)");

        let _ = std::fs::remove_dir_all(&dir);
    }
}

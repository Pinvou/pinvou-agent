//! `files` family: attachment ingest — converts a user-selected file into the
//! markdown text the model would see, mirroring
//! `pinvou3-app/src-tauri/src/app/commands/files.rs::ingest_file`.
//!
//! The GUI command is a thin wrapper over two public feature functions, and
//! this module calls exactly those:
//! - `validate_path` (`features::files::file_ingest::validate_path`) — the
//!   upload path policy (absolute, existing regular file, under `$HOME`,
//!   outside credential components). A missing or policy-rejected file is a
//!   runtime failure (exit 1), matching the CLI contract. The *absolute*
//!   requirement is free in the GUI (a file dialog only ever yields absolute
//!   paths) but would reject the dominant CLI input form, so the positional is
//!   resolved against the current directory first — the same thing every other
//!   path input in this crate does (`knowledge --root`, `agent --workspace`,
//!   `code`'s workspace roots). The `$HOME` confinement and the credential
//!   component blacklist are NOT relaxed: they are the reason this policy
//!   exists, and dropping them would make the CLI a way around a rule the GUI
//!   enforces.
//! - `ingest_attachment` (`features::files::file_ingest::ingest_attachment`) —
//!   hard size/archive limits surface as stable wire codes
//!   (`attachment_file_too_large`, …) which the CLI reports verbatim; format
//!   degradation (missing pandoc/poppler, image without vision, binary) stays
//!   a *successful* ingest whose `warning` field carries the chip text, so the
//!   CLI also exits 0 and prints the warning line.
//!
//! Both the policy errors and the `warning` chips are GUI i18n copy (Chinese);
//! `pinvou-cli` is an English tool, so they are translated at this boundary by
//! [`translate_ingest_error`] / [`translate_ingest_warning`], exactly like
//! `feedback::translate_feedback_text` and `deps::translate_deps_error`.
//!
//! The CLI never reads pixels or runs OCR itself; all conversion logic lives
//! in the feature layer. No Tauri host is booted.

use std::path::PathBuf;

use crate::support::{render, success};
use crate::{CliError, CliOutcome, OutputMode};
use pinvou3_lib::features::files::file_ingest::{self, IngestResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilesCommand {
    Ingest {
        path: PathBuf,
        output: Option<PathBuf>,
    },
}

const USAGE: &str = "usage: pinvou files ingest <PATH> [--output PATH]";

pub fn parse(values: &[String]) -> Result<FilesCommand, CliError> {
    let subcommand = values.get(1).ok_or_else(|| CliError::usage(USAGE))?;
    match subcommand.as_str() {
        "ingest" => {
            let path = values
                .get(2)
                .map(PathBuf::from)
                .ok_or_else(|| CliError::usage("files ingest requires a PATH"))?;
            let rest = &values[3..];
            let mut output: Option<PathBuf> = None;
            let mut index = 0;
            while index < rest.len() {
                let token = rest[index].as_str();
                if token != "--output" {
                    return Err(CliError::usage(format!(
                        "unsupported files option: {token}"
                    )));
                }
                if output.is_some() {
                    return Err(CliError::usage("duplicate files option --output"));
                }
                let value = rest
                    .get(index + 1)
                    .ok_or_else(|| CliError::usage("files option --output requires a value"))?;
                if value.is_empty() || value.starts_with("--") {
                    return Err(CliError::usage("files option --output requires a value"));
                }
                output = Some(PathBuf::from(value));
                index += 2;
            }
            Ok(FilesCommand::Ingest { path, output })
        }
        _ => Err(CliError::usage(USAGE)),
    }
}

pub fn execute(command: FilesCommand, output: OutputMode) -> Result<CliOutcome, CliError> {
    let FilesCommand::Ingest {
        path,
        output: destination,
    } = command;
    // `validate_path` rejects a relative path outright, which is free in the
    // GUI (its file dialog only produces absolute paths) but would reject
    // `pinvou files ingest report.docx` — the ordinary way a terminal names a
    // file. Resolve against the cwd first, like every sibling path input in
    // this crate. Nothing else about the policy changes: the resolved path
    // still has to be an existing regular file under `$HOME` and outside the
    // credential component blacklist.
    let absolute = if path.is_absolute() {
        path.clone()
    } else {
        std::env::current_dir()
            .map_err(|error| {
                CliError::failed(format!(
                    "files ingest: cannot resolve {} against the current directory: {error}",
                    path.display()
                ))
            })?
            .join(&path)
    };
    let raw = absolute.to_string_lossy().into_owned();
    // Same two-step entry as the GUI `ingest_file` command: path policy first,
    // then the attachment ingest with hard limits as wire codes.
    let validated = file_ingest::validate_path(&raw).map_err(|error| {
        CliError::failed(format!("files ingest: {}", translate_ingest_error(&error)))
    })?;
    let result = file_ingest::ingest_attachment(&validated)
        .map_err(|code| CliError::failed(format!("files ingest: {code}")))?;
    match destination {
        Some(destination) => write_file(&result, &destination, output),
        None => print_result(&result, output),
    }
}

/// `--output PATH`: the extracted markdown (or an empty file when the ingest
/// produced only a placeholder, mirroring the GUI which would send no text)
/// is written to the file; stdout carries the summary fields.
fn write_file(
    result: &IngestResult,
    destination: &std::path::Path,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    use std::io::Write;
    let markdown = result.markdown.clone().unwrap_or_default();
    // Same no-overwrite policy as `sessions export`: a plain write could
    // destroy an unrelated file the user pointed at with exit 0, and
    // `create_new` makes the check and the write one atomic step.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                CliError::failed(format!(
                    "files ingest: refusing to overwrite {}; choose a destination that does \
                     not exist yet",
                    destination.display()
                ))
            } else {
                CliError::failed(format!(
                    "files ingest: cannot write {}: {error}",
                    destination.display()
                ))
            }
        })?;
    file.write_all(markdown.as_bytes()).map_err(|error| {
        CliError::failed(format!(
            "files ingest: cannot write {}: {error}",
            destination.display()
        ))
    })?;
    let mut value = ingest_json(result);
    if let Some(map) = value.as_object_mut() {
        map.insert(
            "output".to_owned(),
            serde_json::json!(destination.display().to_string()),
        );
    }
    let mut human = summary_lines(result);
    human.push(format!("Output: {}", destination.display()));
    Ok(success(render(output, human.join("\n"), &value)))
}

fn print_result(result: &IngestResult, output: OutputMode) -> Result<CliOutcome, CliError> {
    let value = ingest_json(result);
    let mut human = summary_lines(result);
    if let Some(markdown) = &result.markdown {
        human.push(markdown.clone());
    }
    Ok(success(render(output, human.join("\n"), &value)))
}

fn summary_lines(result: &IngestResult) -> Vec<String> {
    let mut lines = vec![
        format!("File: {}", result.basename),
        format!("Kind: {}", result.kind),
        format!("Tokens: {}", result.token_estimate),
        format!("Bytes: {}", result.byte_size),
    ];
    if let Some(warning) = &result.warning {
        lines.push(format!("Warning: {}", translate_ingest_warning(warning)));
    }
    lines
}

fn ingest_json(result: &IngestResult) -> serde_json::Value {
    serde_json::json!({
        "kind": result.kind,
        "basename": result.basename,
        "path": result.path,
        "markdown": result.markdown,
        "token_estimate": result.token_estimate,
        "byte_size": result.byte_size,
        "warning": result.warning.as_deref().map(translate_ingest_warning),
    })
}

/// Translates the path-policy refusals `validate_path` can return.
///
/// Same boundary technique as `feedback::translate_feedback_text` and
/// `deps::translate_deps_error`: known messages get English copy, anything
/// unrecognized passes through unchanged rather than being dropped.
///
/// The `$HOME` refusal gets more than a literal translation. `not under $HOME`
/// is a constraint a CLI user hits routinely (`/tmp/report.docx`, a mounted
/// share, `/var/…`) and the upstream wording never says what to do about it,
/// so the English copy states the rule plainly.
fn translate_ingest_error(message: &str) -> String {
    // `platform::os::validate_upload_location` (posix + the non-unix
    // fallback both use this exact wording).
    if let Some(rest) = message.strip_suffix(" not under $HOME") {
        let path = rest.strip_prefix("path ").unwrap_or(rest);
        return format!(
            "{path} is outside the user's home directory; the file to ingest must live under \
             your home directory — copy or move it there first"
        );
    }
    // `platform::path_policy::check_sensitive_components`.
    if let Some(rest) = message.strip_prefix("path ") {
        if let Some((path, component)) = rest.split_once(" crosses sensitive component ") {
            return format!(
                "{path} crosses the credential path component `{component}`; files under \
                 credential locations are never read into a model context"
            );
        }
        if let Some(path) = rest.strip_suffix(" is in system-sensitive area") {
            return format!("{path} is in a system-sensitive area and is never read");
        }
        if let Some((path, detail)) = rest.split_once(" is not readable: ") {
            return format!("{path} is not readable: {detail}");
        }
        if let Some(path) = rest.strip_suffix(" is not a file") {
            return format!("{path} is not a regular file");
        }
    }
    if let Some(path) = message.strip_prefix("path must be absolute: ") {
        return format!("{path} could not be resolved to an absolute path");
    }
    message.to_owned()
}

/// Translates the `IngestResult::warning` chip text.
///
/// These strings are GUI i18n copy rendered as a chip next to the attachment;
/// printed verbatim by an English tool they are unreadable. The list is
/// enumerated from the feature sources that can produce a warning on the
/// `ingest_attachment` path — `features/files/{file_ingest, text_decode,
/// ingest_pdf, ingest_office, ingest_email, ingest_archive}.rs` — and must be
/// revisited when those add a case. Unknown text passes through unchanged
/// (same rule as the two sibling translators): a warning the CLI cannot name
/// is still worth more to the user than no warning at all.
///
/// Not listed here: the hard size/archive limits. `ingest_attachment` turns
/// those into stable wire codes (`attachment_file_too_large`, …) on the `Err`
/// side, so they never reach this field — only `ingest()`, which the CLI does
/// not call, renders them as warnings.
fn translate_ingest_warning(warning: &str) -> String {
    // Prefixed diagnostics run FIRST. Their tail is the underlying OS/tool
    // error (already English, or a path) and is kept verbatim; matching them
    // before the tool-name table below also stops `pdftotext 失败: …` from
    // being mistaken for the "pdftotext is not installed" copy.
    for (prefix, english) in [
        ("文件不存在: ", "file does not exist: "),
        (
            "读取失败(可能不是文本): ",
            "read failed (may not be text): ",
        ),
        ("文件读取失败: ", "read failed: "),
        (
            "创建临时目录失败: ",
            "failed to create a temporary directory: ",
        ),
        (
            "压缩包内容读取失败: ",
            "failed to read the archive contents: ",
        ),
        (
            "解析临时目录失败: ",
            "failed to scan the temporary directory: ",
        ),
        ("pandoc 调用失败: ", "pandoc could not be started: "),
        ("pandoc 失败: ", "pandoc failed: "),
        ("pdftotext 调用失败: ", "pdftotext could not be started: "),
        ("pdftotext 失败: ", "pdftotext failed: "),
        ("pdftoppm 调用失败: ", "pdftoppm could not be started: "),
        ("pdftoppm 失败: ", "pdftoppm failed: "),
        ("tesseract 调用失败: ", "tesseract could not be started: "),
        ("tesseract 退出码 ", "tesseract exited with code "),
        ("7z 调用失败: ", "7z could not be started: "),
        ("7z 解压失败: ", "7z extraction failed: "),
        (
            "LibreOffice 转换后读取失败: ",
            "the LibreOffice output could not be read: ",
        ),
        ("LibreOffice 转换失败: ", "LibreOffice conversion failed: "),
    ] {
        if let Some(rest) = warning.strip_prefix(prefix) {
            return format!("{english}{rest}");
        }
    }
    // Fixed copy, most specific first: several of the missing-tool strings
    // mention more than one tool, and the first arm that matches is the one
    // that names the action the user actually has to take.
    for (needle, english) in [
        (
            "检测到密钥/私钥文件",
            "a key or private-key file was detected; its contents were not read, so they cannot \
             reach a model",
        ),
        ("不支持的文件类型", "unsupported file type (binary)"),
        (
            "检测到音视频文件",
            "audio/video files are not transcribed locally yet; supply a transcript, or describe \
             the key points instead",
        ),
        (
            "演示文稿未提取到文字",
            "no text could be extracted from the presentation",
        ),
        (
            "内容由 OCR 提取",
            "scanned PDF: the text was recovered by OCR and may contain recognition errors",
        ),
        (
            "扫描件 OCR 未识别到文字",
            "OCR found no text in this scanned document",
        ),
        (
            "pdftoppm 未产出可识别页",
            "the PDF has no text layer and pdftoppm produced no readable page",
        ),
        (
            "PDF 无文字层",
            "the PDF has no text layer (likely a scan); the OCR fallback needs poppler-utils and \
             tesseract",
        ),
        // Ahead of the generic PDF-component arms: the Windows OCR copy names
        // Poppler too, but Tesseract is the part the user has to install.
        (
            "Tesseract",
            "running OCR needs tesseract; install it and retry",
        ),
        (
            "tesseract",
            "running OCR needs tesseract; install it and retry",
        ),
        (
            "邮件解析需要可用的 Python 运行时",
            "parsing email requires a usable Python runtime",
        ),
        (
            "libemail-outlook-message-perl",
            "parsing .msg requires the Perl Email::Outlook::Message module \
             (Debian/Ubuntu: sudo apt install libemail-outlook-message-perl)",
        ),
        (
            "压缩包为空或无可识别文件",
            "the archive is empty or holds no readable file",
        ),
        (
            "内置压缩包解析组件",
            "the bundled archive parser is missing or unusable; repair or reinstall Pinvou",
        ),
        (
            "已按 GB18030/GBK 转换为 UTF-8",
            "the file is not UTF-8; it was decoded as GB18030/GBK and converted to UTF-8",
        ),
        (
            "已尽力还原",
            "the file is not UTF-8; it was recovered as best as possible and some characters may \
             be replacement markers",
        ),
        (
            "但文件字节数不完整",
            "a UTF-16 encoding was detected but the byte count is odd; the content was not read",
        ),
        (
            "但内容损坏",
            "a UTF-16 encoding was detected but the content is corrupt; it was not read",
        ),
        (
            "已转换为 UTF-8",
            "a UTF-16 encoding was detected; the content was converted to UTF-8",
        ),
        // Missing-tool copy from `platform::os::*_missing_message`. Its
        // wording differs per platform (an apt line on Linux, a brew line on
        // macOS, a "repair the bundled copy" line on Windows), so the match is
        // on the tool name the user has to act on rather than on the sentence.
        (
            "演示文稿",
            "converting this presentation needs LibreOffice and pdftotext; install them and retry",
        ),
        (
            "Office 文档预览",
            "converting this document needs LibreOffice; install it and retry",
        ),
        (
            "LibreOffice",
            "converting this document needs LibreOffice; install it and retry",
        ),
        (
            "pandoc",
            "converting this document needs pandoc; install it and retry",
        ),
        (
            "Pandoc",
            "converting this document needs pandoc; install it and retry",
        ),
        (
            "pdftotext",
            "reading this PDF needs pdftotext (poppler); install it and retry",
        ),
        (
            "PDF 文本解析",
            "reading this PDF needs pdftotext (poppler); install it and retry",
        ),
        (
            "PDF 渲染",
            "rendering this PDF needs pdftoppm (poppler); install it and retry",
        ),
        (
            "Poppler",
            "this step needs poppler (pdftotext/pdftoppm); install it and retry",
        ),
        ("OCR", "running OCR needs tesseract; install it and retry"),
        ("7z", "reading this archive needs 7z; install it and retry"),
        (
            "当前平台缺少可用的文档解析组件",
            "this platform has no usable document parser",
        ),
    ] {
        if warning.contains(needle) {
            return english.to_owned();
        }
    }
    warning.to_owned()
}

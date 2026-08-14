//! 服务端可移植文档解析器。它只依赖本机可发现的通用命令，不依赖 Tauri。

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use calamine::{open_workbook_auto, Data, Reader};
use encoding_rs::{GB18030, UTF_16BE, UTF_16LE};
use wait_timeout::ChildExt;

const MAX_PARSE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EXTRACTED_TEXT_BYTES: usize = 64 * 1024 * 1024;
const EXTERNAL_PARSER_TIMEOUT: Duration = Duration::from_secs(120);

pub fn parse_document(path: &Path) -> Result<String, String> {
    let metadata = std::fs::metadata(path).map_err(|error| format!("读取文件失败: {error}"))?;
    if metadata.len() > MAX_PARSE_BYTES {
        return Err(format!(
            "文件超过 {} MiB 解析上限",
            MAX_PARSE_BYTES / 1024 / 1024
        ));
    }
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "txt" | "md" | "markdown" | "csv" | "tsv" | "json" | "jsonl" | "yaml" | "yml" | "toml"
        | "xml" | "html" | "htm" | "log" | "ini" | "conf" | "rs" | "py" | "js" | "jsx" | "ts"
        | "tsx" | "java" | "c" | "h" | "cpp" | "hpp" | "go" | "sh" | "ps1" | "sql" => {
            read_text(path)
        }
        "xlsx" | "xls" | "xlsb" | "ods" => parse_spreadsheet(path),
        "pdf" => run_text_command("pdftotext", &[path.as_os_str(), "-".as_ref()]),
        "doc" | "docx" | "odt" | "rtf" | "ppt" | "pptx" | "odp" | "epub" => {
            parse_office(path, &ext)
        }
        "png" | "jpg" | "jpeg" | "bmp" | "tif" | "tiff" | "webp" => parse_image_ocr(path),
        _ => Err(format!("暂不支持解析 .{ext} 文件")),
    }
}

fn read_text(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    if crate::looks_like_secret_material(&bytes) {
        return Err("检测到私钥内容，已拒绝读取和索引".to_string());
    }
    decode_text_bytes(bytes)
}

fn decode_text_bytes(bytes: Vec<u8>) -> Result<String, String> {
    if let Some(payload) = bytes.strip_prefix(&[0xff, 0xfe]) {
        return decode_utf16(UTF_16LE, payload);
    }
    if let Some(payload) = bytes.strip_prefix(&[0xfe, 0xff]) {
        return decode_utf16(UTF_16BE, payload);
    }
    if bytes.len() >= 4 && bytes.len().is_multiple_of(2) {
        let pairs = bytes.len() / 2;
        let even_zeroes = bytes.iter().step_by(2).filter(|byte| **byte == 0).count();
        let odd_zeroes = bytes
            .iter()
            .skip(1)
            .step_by(2)
            .filter(|byte| **byte == 0)
            .count();
        if odd_zeroes * 4 >= pairs && even_zeroes * 16 <= pairs {
            return decode_utf16(UTF_16LE, &bytes);
        }
        if even_zeroes * 4 >= pairs && odd_zeroes * 16 <= pairs {
            return decode_utf16(UTF_16BE, &bytes);
        }
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok(text),
        Err(error) => {
            let bytes = error.into_bytes();
            let (decoded, _, had_errors) = GB18030.decode(&bytes);
            if had_errors {
                Err("文件既不是 UTF-8，也无法按 GB18030 解码".to_string())
            } else {
                Ok(decoded.into_owned())
            }
        }
    }
}

fn decode_utf16(encoding: &'static encoding_rs::Encoding, bytes: &[u8]) -> Result<String, String> {
    let (decoded, _, had_errors) = encoding.decode(bytes);
    if had_errors {
        Err("UTF-16 文本编码无效".to_string())
    } else {
        Ok(decoded.into_owned())
    }
}

fn parse_office(path: &Path, extension: &str) -> Result<String, String> {
    if matches!(extension, "ppt" | "pptx" | "odp") {
        return libreoffice_presentation_text(path);
    }
    let pandoc = || {
        run_text_command(
            "pandoc",
            &[path.as_os_str(), "-t".as_ref(), "plain".as_ref()],
        )
    };
    if extension == "epub" {
        return pandoc();
    }
    pandoc().or_else(|pandoc_error| {
        libreoffice_text(path)
            .map_err(|office_error| format!("pandoc: {pandoc_error}; LibreOffice: {office_error}"))
    })
}

fn libreoffice_text(path: &Path) -> Result<String, String> {
    let (_staging, output) = libreoffice_convert(path, "txt:Text (encoded):UTF8", "txt")?;
    read_text(&output)
}

fn libreoffice_presentation_text(path: &Path) -> Result<String, String> {
    let (_staging, output) = libreoffice_convert(path, "pdf", "pdf")?;
    run_text_command("pdftotext", &[output.as_os_str(), "-".as_ref()])
}

fn libreoffice_convert(
    path: &Path,
    target: &str,
    extension: &str,
) -> Result<(tempfile::TempDir, std::path::PathBuf), String> {
    let staging = tempfile::tempdir().map_err(|error| error.to_string())?;
    let output = staging
        .path()
        .join(
            path.file_stem()
                .ok_or_else(|| "Office 文档文件名无效".to_string())?,
        )
        .with_extension(extension);
    run_bounded_command(
        "soffice",
        &[
            "--headless".as_ref(),
            "--convert-to".as_ref(),
            target.as_ref(),
            "--outdir".as_ref(),
            staging.path().as_os_str(),
            path.as_os_str(),
        ],
    )?;
    if !output.is_file() {
        return Err(format!("LibreOffice 未生成 .{extension} 转换结果"));
    }
    Ok((staging, output))
}

fn parse_spreadsheet(path: &Path) -> Result<String, String> {
    let mut workbook = open_workbook_auto(path).map_err(|error| error.to_string())?;
    let sheets = workbook.sheet_names().to_vec();
    let mut output = String::new();
    for sheet in sheets {
        let range = workbook
            .worksheet_range(&sheet)
            .map_err(|error| format!("工作表 {sheet}: {error}"))?;
        output.push_str("\n## ");
        output.push_str(&sheet);
        output.push('\n');
        for row in range.rows() {
            let cells = row.iter().map(cell_text).collect::<Vec<_>>();
            output.push_str(&cells.join("\t"));
            output.push('\n');
            if output.len() > MAX_EXTRACTED_TEXT_BYTES {
                return Err(format!(
                    "解析文本超过 {} MiB 上限",
                    MAX_EXTRACTED_TEXT_BYTES / 1024 / 1024
                ));
            }
        }
    }
    Ok(output)
}

fn cell_text(value: &Data) -> String {
    match value {
        Data::Empty => String::new(),
        _ => value.to_string(),
    }
}

fn parse_image_ocr(path: &Path) -> Result<String, String> {
    let first = run_text_command(
        "tesseract",
        &[
            path.as_os_str(),
            "stdout".as_ref(),
            "-l".as_ref(),
            "chi_sim+eng".as_ref(),
        ],
    );
    first.or_else(|_| run_text_command("tesseract", &[path.as_os_str(), "stdout".as_ref()]))
}

fn run_text_command(program: &str, args: &[&std::ffi::OsStr]) -> Result<String, String> {
    let stdout = run_bounded_command(program, args)?;
    let text = String::from_utf8_lossy(&stdout).trim().to_string();
    if text.is_empty() {
        Err(format!("{program} 未提取到文本"))
    } else {
        Ok(text)
    }
}

fn run_bounded_command(program: &str, args: &[&std::ffi::OsStr]) -> Result<Vec<u8>, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("无法运行 {program}: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("无法读取 {program} 输出"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("无法读取 {program} 错误输出"))?;
    let stdout_reader = std::thread::spawn(move || read_bounded(stdout));
    let stderr_reader = std::thread::spawn(move || read_bounded(stderr));
    let status = match child
        .wait_timeout(EXTERNAL_PARSER_TIMEOUT)
        .map_err(|error| format!("等待 {program} 失败: {error}"))?
    {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(format!(
                "{program} 解析超过 {} 秒，已终止",
                EXTERNAL_PARSER_TIMEOUT.as_secs()
            ));
        }
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| format!("读取 {program} 输出的线程异常结束"))?
        .map_err(|error| format!("读取 {program} 输出失败: {error}"))?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| format!("读取 {program} 错误输出的线程异常结束"))?
        .map_err(|error| format!("读取 {program} 错误输出失败: {error}"))?;
    if stdout.len() > MAX_EXTRACTED_TEXT_BYTES {
        return Err(format!(
            "{program} 提取文本超过 {} MiB 上限",
            MAX_EXTRACTED_TEXT_BYTES / 1024 / 1024
        ));
    }
    if !status.success() {
        return Err(format!(
            "{program} 解析失败: {}",
            String::from_utf8_lossy(&stderr).trim()
        ));
    }
    Ok(stdout)
}

fn read_bounded(mut reader: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(MAX_EXTRACTED_TEXT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::decode_text_bytes;

    #[test]
    fn decodes_utf16_with_bom_and_without_bom_when_byte_order_is_clear() {
        assert_eq!(
            decode_text_bytes(vec![0xff, 0xfe, b'A', 0, b'B', 0]).unwrap(),
            "AB"
        );
        assert_eq!(decode_text_bytes(vec![0, b'A', 0, b'B']).unwrap(), "AB");
    }
}

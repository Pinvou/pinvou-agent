//! 邮件摄入（.eml / .msg）。
//!
//! - `.eml`：python 标准库 email 模块解出收发件人/主题/日期/正文/附件名；
//! - `.msg`：Windows 走 Rust 原生（msg_parser）解析，非 Windows 保留 msgconvert 转 .eml。
//!
//! .msg 的 HTML 正文与 hex 编码 body 在此做 UTF-16 / HTML-entity 解码。

use std::path::Path;
use std::process::Command;

use super::estimate_tokens;
use super::ingest_deps::system_tools;
use super::IngestResult;

/// 邮件（.eml / .msg）：.eml 直接用 python 标准库 email 模块解出收发件人/主题/
/// 日期/正文/附件名；.msg 在 Windows 走 Rust 原生解析，非 Windows 保留 msgconvert 转 .eml。
pub(super) fn ingest_email(
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

    if kind == "msg" && crate::platform::os::msg_native_supported() {
        return match parse_msg_via_msg_parser(path) {
            Ok(text) => mk(Some(text), None),
            Err(e) => mk(None, Some(e)),
        };
    }

    if !tools.python {
        return mk(None, Some("邮件解析需要可用的 Python 运行时。".into()));
    }

    let parsed = if kind == "msg" {
        if !tools.msgconvert {
            return mk(
                None,
                Some(".msg 解析需要: sudo apt install libemail-outlook-message-perl".into()),
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
    value
        .chars()
        .filter(|ch| *ch != '\0')
        .collect::<String>()
        .trim()
        .to_string()
}

fn decode_msg_html_payload(value: &str) -> String {
    let value = clean_msg_text(value);
    if value.len() < 8
        || !value.len().is_multiple_of(2)
        || !value.bytes().all(|b| b.is_ascii_hexdigit())
    {
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
            u32::from_str_radix(&entity[2..], 16)
                .ok()
                .and_then(char::from_u32)
        }
        _ if entity.starts_with('#') => entity[1..].parse::<u32>().ok().and_then(char::from_u32),
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
    let program = crate::platform::paths::python_command();
    let out = crate::platform::process::HiddenCommand::new(&program)
        .arg("-c")
        .arg(SCRIPT)
        .arg(path)
        .output()
        .map_err(|e| format!("Python 调用失败({program}): {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    } else {
        Err(format!(
            "邮件解析失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(
            clean_msg_text("你的临时 OpenAI 登录代码\0"),
            "你的临时 OpenAI 登录代码"
        );
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
    fn eml_regression_parses_headers_body_and_attachment_when_python_available() {
        if !crate::platform::os::command_exists(&crate::platform::paths::python_command()) {
            eprintln!("skip: Python is not available");
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
}

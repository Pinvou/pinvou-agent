pub(crate) fn has_usable_asr_text(text: &str) -> bool {
    text.chars().any(|ch| ch.is_alphanumeric())
}

pub(crate) fn parse_asr_transcript(stdout: &str, stderr: &str) -> Option<String> {
    parse_asr_stream(stdout, true).or_else(|| parse_asr_stream(stderr, false))
}

fn parse_asr_stream(stream: &str, allow_plain_text: bool) -> Option<String> {
    for line in stream.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(text) = parse_protocol_line(line) {
            return Some(text);
        }
        if allow_plain_text && !looks_like_log_line(line) && has_usable_asr_text(line) {
            let text = clean_asr_text(line);
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

fn parse_protocol_line(line: &str) -> Option<String> {
    const RESULT_PREFIXES: [&str; 7] = [
        "result:",
        "asr result:",
        "recognition result:",
        "transcription:",
        "transcript:",
        "text:",
        "output:",
    ];

    let lower = line.to_ascii_lowercase();
    for prefix in RESULT_PREFIXES {
        if lower.starts_with(prefix) {
            let text = clean_asr_text(line[prefix.len()..].trim());
            return (!text.is_empty()).then_some(text);
        }
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
        for key in ["text", "result", "sentence", "transcript"] {
            if let Some(text) = value.get(key).and_then(serde_json::Value::as_str) {
                let text = clean_asr_text(text);
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
    }

    if (line.starts_with("['") && line.ends_with("']"))
        || (line.starts_with("[\"") && line.ends_with("\"]"))
    {
        let text = line
            .trim_start_matches('[')
            .trim_end_matches(']')
            .trim_matches('\'')
            .trim_matches('"')
            .trim();
        let text = clean_asr_text(text);
        if !text.is_empty() {
            return Some(text);
        }
    }
    None
}

fn looks_like_log_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("error")
        || lower.contains("warning")
        || lower.contains("paddlespeech")
        || lower.contains("sensevoice")
        || lower.contains("funasr")
        || lower.contains("gguf")
        || lower.contains("python")
        || lower.contains("download")
        || lower.starts_with('[')
}

fn clean_asr_text(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text.trim();
    loop {
        let Some(start) = rest.find("<|") else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let after_start = &rest[start + 2..];
        let Some(end) = after_start.find("|>") else {
            out.push_str(&rest[start..]);
            break;
        };
        rest = &after_start[end + 2..];
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_numeric_only_transcripts_without_accepting_punctuation_noise() {
        assert!(has_usable_asr_text("123"));
        assert!(has_usable_asr_text("１２３"));
        assert!(has_usable_asr_text("你好"));
        assert!(!has_usable_asr_text("...，。!?"));
        assert!(!has_usable_asr_text(" \t\r\n"));
    }

    #[test]
    fn stdout_plain_text_wins_over_stderr_progress_noise() {
        assert_eq!(
            parse_asr_transcript("123\n", "100/100%\n12:34:56\n"),
            Some("123".to_string())
        );
    }

    #[test]
    fn stderr_plain_progress_and_counts_are_not_transcripts() {
        assert_eq!(parse_asr_transcript("", "100/100%\n100\n12:34:56\n"), None);
    }

    #[test]
    fn accepts_unicode_digits_and_structured_numeric_strings() {
        assert_eq!(
            parse_asr_transcript("１２３\n", ""),
            Some("１２３".to_string())
        );
        assert_eq!(
            parse_asr_transcript(r#"{"text":"123"}"#, ""),
            Some("123".to_string())
        );
        assert_eq!(
            parse_asr_transcript("", "result: 123\n"),
            Some("123".to_string()),
            "stderr is accepted only when it uses an explicit result protocol"
        );
    }
}

pub(crate) fn has_usable_asr_text(text: &str) -> bool {
    text.chars().any(|ch| ch.is_alphanumeric())
}

pub(crate) fn parse_asr_transcript(stdout: &str, stderr: &str) -> Option<String> {
    parse_explicit_asr_result(stdout)
        .or_else(|| parse_explicit_asr_result(stderr))
        .or_else(|| parse_stdout_fallback(stdout))
}

fn parse_explicit_asr_result(stream: &str) -> Option<String> {
    stream
        .lines()
        .rev()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .find_map(parse_protocol_line)
}

fn parse_stdout_fallback(stream: &str) -> Option<String> {
    let lines = stream
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();

    let timed_segments = lines
        .iter()
        .filter_map(|line| parse_timed_transcript_line(line))
        .collect::<Vec<_>>();
    let timed_segments = join_timed_transcript_segments(&timed_segments);
    if has_usable_asr_text(&timed_segments) {
        return Some(timed_segments);
    }

    for line in lines.iter().rev().copied() {
        let line = line.trim();
        let text = clean_asr_text(line);
        if !text.is_empty()
            && !looks_like_log_line(line)
            && !looks_like_plain_status(&text)
            && has_usable_asr_text(&text)
        {
            return Some(text);
        }
    }
    None
}

struct TimedTranscriptSegment {
    text: String,
    has_leading_whitespace: bool,
    has_trailing_whitespace: bool,
}

fn parse_timed_transcript_line(line: &str) -> Option<TimedTranscriptSegment> {
    let line = line.trim_start();
    let rest = line.strip_prefix('[')?;
    let end = rest.find(']')?;
    let range = &rest[..end];
    let (start, finish) = range.split_once("-->").or_else(|| range.split_once('-'))?;
    if !looks_like_time_value(start.trim()) || !looks_like_time_value(finish.trim()) {
        return None;
    }
    let raw_text = rest[end + 1..]
        .strip_prefix(char::is_whitespace)
        .unwrap_or(&rest[end + 1..]);
    let cleaned = strip_asr_control_markers(raw_text);
    let text = cleaned.trim().to_string();
    (!text.is_empty() && !looks_like_plain_status(&text)).then(|| TimedTranscriptSegment {
        text,
        has_leading_whitespace: cleaned.starts_with(char::is_whitespace),
        has_trailing_whitespace: cleaned.ends_with(char::is_whitespace),
    })
}

fn join_timed_transcript_segments(segments: &[TimedTranscriptSegment]) -> String {
    let mut transcript = String::new();
    let mut previous_had_trailing_whitespace = false;
    for segment in segments {
        let needs_word_separator = needs_timed_segment_separator(&transcript, &segment.text);
        if !transcript.is_empty()
            && (previous_had_trailing_whitespace
                || segment.has_leading_whitespace
                || needs_word_separator)
        {
            transcript.push(' ');
        }
        transcript.push_str(&segment.text);
        previous_had_trailing_whitespace = segment.has_trailing_whitespace;
    }
    transcript
}

fn needs_timed_segment_separator(transcript: &str, next_segment: &str) -> bool {
    let Some(next) = next_segment.chars().next() else {
        return false;
    };
    if !is_latin_word_char(next) {
        return false;
    }

    let mut preceding = transcript.chars().rev();
    let Some(mut previous) = preceding.next() else {
        return false;
    };
    while matches!(previous, '"' | ')' | ']' | '}') {
        let Some(before_closer) = preceding.next() else {
            return false;
        };
        previous = before_closer;
    }
    is_latin_word_char(previous) || matches!(previous, ',' | '.' | '!' | '?' | ':' | ';')
}

fn is_latin_word_char(ch: char) -> bool {
    if ch.is_ascii_alphanumeric() {
        return true;
    }
    ch.is_alphabetic()
        && (('\u{00C0}'..='\u{024F}').contains(&ch)
            || ('\u{1E00}'..='\u{1EFF}').contains(&ch)
            || ('\u{2C60}'..='\u{2C7F}').contains(&ch)
            || ('\u{A720}'..='\u{A7FF}').contains(&ch)
            || ('\u{AB30}'..='\u{AB6F}').contains(&ch))
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

fn looks_like_plain_status(line: &str) -> bool {
    let trimmed = line.trim().trim_matches(['[', ']', '(', ')']);
    let lower = trimmed.to_ascii_lowercase();
    if [
        "progress:",
        "progress ",
        "loading:",
        "loading ",
        "processed:",
        "processed ",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
    {
        return true;
    }

    if let Some(value) = trimmed
        .strip_suffix('%')
        .or_else(|| trimmed.strip_suffix('％'))
    {
        return looks_like_number(value.trim());
    }

    for separator in ['/', '／'] {
        if let Some((current, total)) = trimmed.split_once(separator) {
            let total = total
                .trim()
                .strip_suffix('%')
                .or_else(|| total.trim().strip_suffix('％'))
                .unwrap_or(total.trim());
            if looks_like_number(current.trim()) && looks_like_number(total) {
                return true;
            }
        }
    }
    if let Some((current, total)) = lower.split_once(" of ") {
        if looks_like_number(current.trim()) && looks_like_number(total.trim()) {
            return true;
        }
    }

    looks_like_clock_value(trimmed)
}

fn looks_like_number(value: &str) -> bool {
    let mut saw_digit = false;
    for ch in value.chars() {
        if ch.is_numeric() {
            saw_digit = true;
        } else if !matches!(ch, '.' | '．' | ',') {
            return false;
        }
    }
    saw_digit
}

fn looks_like_clock_value(value: &str) -> bool {
    let parts = value.split([':', '：']).collect::<Vec<_>>();
    (parts.len() == 2 || parts.len() == 3)
        && parts
            .iter()
            .all(|part| !part.trim().is_empty() && looks_like_number(part.trim()))
}

fn looks_like_time_value(value: &str) -> bool {
    looks_like_number(value) || looks_like_clock_value(value)
}

fn clean_asr_text(text: &str) -> String {
    strip_asr_control_markers(text).trim().to_string()
}

fn strip_asr_control_markers(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
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
    out
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
            parse_asr_transcript("123\n100%\n", "100/100%\n12:34:56\n"),
            Some("123".to_string())
        );
        assert_eq!(parse_asr_transcript("100%\n", ""), None);
    }

    #[test]
    fn stderr_explicit_result_wins_over_stdout_fallback_text() {
        assert_eq!(
            parse_asr_transcript("Done\n", "result: 123\n"),
            Some("123".to_string())
        );
        assert_eq!(
            parse_asr_transcript("Using 4 threads\n", r#"{"text":"final transcript"}"#),
            Some("final transcript".to_string())
        );
        assert_eq!(
            parse_asr_transcript("[0-.5] preliminary transcript\n", r#"{"result":"final"}"#),
            Some("final".to_string()),
            "stdout timestamp fallback must not mask an explicit stderr result"
        );
    }

    #[test]
    fn stderr_plain_progress_and_counts_are_not_transcripts() {
        assert_eq!(parse_asr_transcript("", "100/100%\n100\n12:34:56\n"), None);
        assert_eq!(parse_asr_transcript("", "ordinary stderr text\n"), None);
        assert_eq!(parse_asr_transcript("", "[0-.5] timed stderr text\n"), None);
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
        assert_eq!(
            parse_asr_transcript("[0.00-0.50] <|zh|><|NEUTRAL|>１２\n[0.50-1.00] ３\n", ""),
            Some("１２３".to_string()),
            "SenseVoice timestamped segments are its explicit backend protocol"
        );
    }

    #[test]
    fn joins_timed_segments_without_corrupting_word_boundaries() {
        assert_eq!(
            parse_asr_transcript("[0-.5] hello there\n[.5-1] wide world\n", ""),
            Some("hello there wide world".to_string())
        );
        assert_eq!(
            parse_asr_transcript("[0-.5] 你好\n[.5-1] ，\n[1-1.5] 世界\n[1.5-2] ！\n", ""),
            Some("你好，世界！".to_string())
        );
        assert_eq!(
            parse_asr_transcript("[0-.5] 你好 \n[.5-1] 世界\n", ""),
            Some("你好 世界".to_string()),
            "explicit backend whitespace is retained at a CJK segment boundary"
        );
        assert_eq!(
            parse_asr_transcript(
                "[0-.5] 中文\n[.5-1] Rust\n[1-1.5] 123\n[1.5-2] ，\n[2-2.5] 版本\n",
                ""
            ),
            Some("中文Rust 123，版本".to_string())
        );
        assert_eq!(
            parse_asr_transcript("[0-.5] １２\n[.5-1] ３\n", ""),
            Some("１２３".to_string())
        );
        assert_eq!(
            parse_asr_transcript("[0-.5] hello,\n[.5-1] world\n", ""),
            Some("hello, world".to_string())
        );
        assert_eq!(
            parse_asr_transcript("[0-.5] Hello\n[.5-1] .\n[1-1.5] World\n", ""),
            Some("Hello. World".to_string())
        );
        assert_eq!(
            parse_asr_transcript("[0-.5] (Hello\n[.5-1] )\n[1-1.5] World\n", ""),
            Some("(Hello) World".to_string())
        );
        assert_eq!(
            parse_asr_transcript("[0-.5] \"Hello\n[.5-1] .\n[1-1.5] \"\n[1.5-2] World\n", ""),
            Some("\"Hello.\" World".to_string())
        );
        assert_eq!(
            parse_asr_transcript("[0-.5] can\n[.5-1] 't\n", ""),
            Some("can't".to_string())
        );
        assert_eq!(
            parse_asr_transcript("[0-.5] state-\n[.5-1] of-the-art\n", ""),
            Some("state-of-the-art".to_string())
        );
        assert_eq!(
            parse_asr_transcript("[0-.5] 你好\n[.5-1] 。\n[1-1.5] 世界\n", ""),
            Some("你好。世界".to_string())
        );
        assert_eq!(
            parse_asr_transcript("[0-.5] 你好，\n[.5-1] world\n", ""),
            Some("你好，world".to_string())
        );
        assert_eq!(
            parse_asr_transcript("[0-.5] ,\n[.5-1] !\n", ""),
            None,
            "a timestamp protocol does not make punctuation-only output usable"
        );
    }
}

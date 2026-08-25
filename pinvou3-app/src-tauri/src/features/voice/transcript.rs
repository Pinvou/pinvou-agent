fn has_usable_asr_text(text: &str) -> bool {
    text.chars().any(|ch| ch.is_alphanumeric())
}

pub(crate) fn parse_asr_transcript(stdout: &str, stderr: &str) -> Option<String> {
    parse_explicit_asr_result(stdout)
        .or_else(|| parse_explicit_asr_result(stderr))
        .or_else(|| parse_timed_fallback(stdout))
        // Custom engines configured through PINVOU3_ASR_CMD and the legacy
        // command variables may emit their final plain-text result on stderr.
        // The bundled runtime uses an explicit/timed stdout protocol, so keep
        // those higher-confidence forms ahead of this compatibility fallback.
        .or_else(|| parse_plain_fallback(stderr, true))
        .or_else(|| parse_plain_fallback(stdout, false))
}

fn parse_explicit_asr_result(stream: &str) -> Option<String> {
    let mut later_fallback_result = false;
    let mut later_json_segment = false;
    let mut json_segments = Vec::new();
    for line in stream
        .lines()
        .rev()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if let Some(segment) = parse_timestamped_json_transcript_line(line) {
            if !later_fallback_result {
                json_segments.push(segment);
                later_json_segment = true;
            }
            continue;
        }
        if let Some(result) = parse_protocol_line(line) {
            // A later transcript-like line is the final result. Do not let an
            // earlier JSON log object with a generic `text` field mask it.
            if !later_fallback_result && !later_json_segment {
                return Some(result);
            }
            continue;
        }
        if parse_plain_transcript_line(line).is_some()
            || parse_timed_transcript_line(line)
                .is_some_and(|segment| has_usable_asr_text(&segment.text))
        {
            later_fallback_result = true;
        }
    }
    json_segments.reverse();
    let transcript = join_transcript_segments(&json_segments);
    has_usable_asr_text(&transcript).then_some(transcript)
}

fn parse_timed_fallback(stream: &str) -> Option<String> {
    let lines = stream
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();

    let timed_segments = lines
        .iter()
        .filter_map(|line| parse_timed_transcript_line(line))
        .collect::<Vec<_>>();
    let timed_segments = join_transcript_segments(&timed_segments);
    if has_usable_asr_text(&timed_segments) {
        return Some(timed_segments);
    }
    None
}

fn parse_plain_fallback(stream: &str, reject_numeric_progress: bool) -> Option<String> {
    let lines = stream
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    let has_progress_context = lines
        .iter()
        .any(|line| looks_like_plain_status(&clean_asr_text(line)));
    let has_subtitle_timing = lines.iter().any(|line| looks_like_subtitle_timing(line));
    for line in lines.iter().rev().copied() {
        if let Some(text) = parse_plain_transcript_line(line) {
            if (has_subtitle_timing || (reject_numeric_progress && has_progress_context))
                && looks_like_number(&text)
            {
                continue;
            }
            return Some(text);
        }
    }
    None
}

fn parse_plain_transcript_line(line: &str) -> Option<String> {
    let line = line.trim();
    let structured_json = serde_json::from_str::<serde_json::Value>(line)
        .is_ok_and(|value| value.is_object() || value.is_array());
    if line.is_empty() || protocol_payload(line).is_some() || structured_json {
        return None;
    }
    let text = clean_asr_text(line);
    (!text.is_empty()
        && !looks_like_log_line(line)
        && !looks_like_plain_status(&text)
        && has_usable_asr_text(&text))
    .then_some(text)
}

struct TranscriptSegment {
    text: String,
    has_leading_whitespace: bool,
    has_trailing_whitespace: bool,
}

fn transcript_segment(raw_text: &str) -> Option<TranscriptSegment> {
    let cleaned = strip_asr_control_markers(raw_text);
    let text = cleaned.trim().to_string();
    (!text.is_empty()).then(|| TranscriptSegment {
        text,
        has_leading_whitespace: cleaned.starts_with(char::is_whitespace),
        has_trailing_whitespace: cleaned.ends_with(char::is_whitespace),
    })
}

fn parse_timed_transcript_line(line: &str) -> Option<TranscriptSegment> {
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
    // A valid timestamp is the SenseVoice protocol credential. Values such as
    // `12:30`, `50%`, or `3/4` are legitimate recognized speech inside a timed
    // segment; progress filtering is only safe for unstructured loose output.
    transcript_segment(raw_text)
}

fn join_transcript_segments(segments: &[TranscriptSegment]) -> String {
    let mut transcript = String::new();
    let mut previous_had_trailing_whitespace = false;
    for segment in segments {
        let needs_word_separator = needs_segment_separator(&transcript, &segment.text);
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

fn needs_segment_separator(transcript: &str, next_segment: &str) -> bool {
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
    if previous.is_numeric() && next.is_numeric() {
        return false;
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
    if let Some(payload) = protocol_payload(line) {
        return explicit_result_text(payload);
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
        if let Some(object) = value.as_object() {
            if ["level", "severity", "logger"]
                .iter()
                .any(|key| object.contains_key(*key))
                || object
                    .get("timestamp")
                    .is_some_and(serde_json::Value::is_string)
            {
                return None;
            }
            for key in ["text", "result", "sentence", "transcript"] {
                if let Some(text) = object.get(key).and_then(serde_json::Value::as_str) {
                    if let Some(text) = explicit_result_text(text) {
                        return Some(text);
                    }
                }
            }
            return None;
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
        return explicit_result_text(text);
    }
    None
}

fn parse_timestamped_json_transcript_line(line: &str) -> Option<TranscriptSegment> {
    let value = serde_json::from_str::<serde_json::Value>(line).ok()?;
    let object = value.as_object()?;
    if ["level", "severity", "logger"]
        .iter()
        .any(|key| object.contains_key(*key))
        || !object
            .get("timestamp")
            .is_some_and(serde_json::Value::is_number)
    {
        return None;
    }
    ["text", "result", "sentence", "transcript"]
        .iter()
        .find_map(|key| object.get(*key).and_then(serde_json::Value::as_str))
        .and_then(transcript_segment)
}

fn protocol_payload(line: &str) -> Option<&str> {
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
            return Some(line[prefix.len()..].trim());
        }
    }
    None
}

fn explicit_result_text(text: &str) -> Option<String> {
    let text = clean_asr_text(text);
    (!text.is_empty() && has_usable_asr_text(&text)).then_some(text)
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
        || lower == "done"
        || lower.starts_with("done in ")
        || (lower.starts_with("using ") && lower.contains("thread"))
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

    if looks_like_subtitle_timing(trimmed) {
        return true;
    }

    if let Some(value) = trimmed
        .strip_suffix('%')
        .or_else(|| trimmed.strip_suffix('％'))
    {
        if looks_like_number(value.trim()) {
            return true;
        }
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

fn looks_like_subtitle_timing(line: &str) -> bool {
    line.split_once("-->").is_some_and(|(start, finish)| {
        looks_like_clock_value(start.trim()) && looks_like_clock_value(finish.trim())
    })
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
            // A truncated control marker is metadata, not recognized speech.
            // Discard the unterminated marker and its tail instead of leaking
            // `<|...` into the user transcript.
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
    fn stderr_plain_fallback_preserves_custom_engine_compatibility() {
        assert_eq!(parse_asr_transcript("", "100/100%\n12:34:56\n"), None);
        assert_eq!(
            parse_asr_transcript("", "100\n"),
            Some("100".to_string()),
            "numeric-only speech remains valid for custom stderr engines"
        );
        assert_eq!(
            parse_asr_transcript("", "100/100%\n100\n12:34:56\n"),
            None,
            "a bare number among progress-shaped stderr lines is progress output"
        );
        assert_eq!(
            parse_asr_transcript("", "ordinary stderr text\n"),
            Some("ordinary stderr text".to_string())
        );
        assert_eq!(
            parse_asr_transcript("noise banner text\n", "final spoken sentence here\n"),
            Some("final spoken sentence here".to_string()),
            "legacy custom engines may emit their final plain result on stderr"
        );
        assert_eq!(
            parse_asr_transcript("final spoken sentence here\n", "Using 4 threads\n"),
            Some("final spoken sentence here".to_string()),
            "stderr diagnostics must not mask a usable stdout fallback"
        );
        assert_eq!(parse_asr_transcript("", "[0-.5] timed stderr text\n"), None);
    }

    #[test]
    fn structured_logs_and_non_string_json_do_not_mask_the_final_result() {
        assert_eq!(
            parse_asr_transcript("{\"text\":\"loading vocab\"}\n你好世界\n", ""),
            Some("你好世界".to_string())
        );
        assert_eq!(
            parse_asr_transcript(
                "{\"level\":\"info\",\"text\":\"done in 1.2s\"}\nfinal result\n",
                ""
            ),
            Some("final result".to_string())
        );
        assert_eq!(
            parse_asr_transcript(
                "final stdout result\n",
                "{\"level\":\"info\",\"text\":\"loading model\"}\n"
            ),
            Some("final stdout result".to_string()),
            "a JSON log on stderr must not mask a plain stdout result"
        );
        assert_eq!(parse_asr_transcript("{\"text\":123}\n", ""), None);
        assert_eq!(
            parse_asr_transcript("{\"text\":123}\n", "result: fallback\n"),
            Some("fallback".to_string())
        );
        assert_eq!(parse_asr_transcript("result: ...\n", ""), None);
    }

    #[test]
    fn joins_numeric_timestamp_jsonl_segments_without_accepting_json_logs() {
        assert_eq!(
            parse_asr_transcript(
                "{\"timestamp\":0,\"text\":\"hello\"}\n{\"timestamp\":1,\"text\":\"world\"}\n",
                ""
            ),
            Some("hello world".to_string())
        );
        assert_eq!(
            parse_asr_transcript(
                "{\"timestamp\":0,\"level\":\"info\",\"text\":\"loading\"}\n",
                ""
            ),
            None
        );
        assert_eq!(
            parse_asr_transcript(
                "{\"timestamp\":\"2026-08-25T00:00:00Z\",\"text\":\"loading\"}\n",
                ""
            ),
            None
        );
        assert_eq!(
            parse_asr_transcript("{\"timestamp\":null,\"text\":\"final result\"}\n", ""),
            Some("final result".to_string()),
            "only string timestamps are diagnostic metadata by themselves"
        );
    }

    #[test]
    fn subtitle_timing_and_plain_progress_phrases_are_not_transcripts() {
        assert_eq!(
            parse_asr_transcript("1\n00:00:03,000 --> 00:00:04,000\n", ""),
            None
        );
        assert_eq!(parse_asr_transcript("3 of 4\n", ""), None);
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
            "stderr explicit protocol results remain highest-confidence output"
        );
        assert_eq!(
            parse_asr_transcript("result: 50%\n", ""),
            Some("50%".to_string()),
            "an explicit protocol credential makes percentage speech unambiguous"
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
            parse_asr_transcript("[0-.5] 12\n[.5-1] 34\n", ""),
            Some("1234".to_string()),
            "ASCII and full-width digits use the same segment-boundary rule"
        );
        assert_eq!(
            parse_asr_transcript("[0-.5] 现在是\n[.5-1] 12:30\n", ""),
            Some("现在是12:30".to_string()),
            "clock text inside the timed protocol is recognized speech, not progress"
        );
        assert_eq!(
            parse_asr_transcript("[0-.5] 完成度\n[.5-1] 50%\n", ""),
            Some("完成度50%".to_string())
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
        assert_eq!(
            parse_asr_transcript("[0-.5] timed result\nplain footer\n", ""),
            Some("timed result".to_string()),
            "a mixed stream keeps the higher-confidence timed protocol"
        );
    }

    #[test]
    fn control_markers_never_leak_into_transcripts() {
        assert_eq!(
            parse_asr_transcript("<|zh|><|NEUTRAL|>你好 <|en\n", ""),
            Some("你好".to_string())
        );
        assert_eq!(
            parse_asr_transcript("[0-.5] <|zh|><|NEUTRAL|>你好 <|en\n", ""),
            Some("你好".to_string())
        );
    }
}

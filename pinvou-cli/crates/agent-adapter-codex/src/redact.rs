const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;

/// Redacts credential-shaped text before it can enter diagnostics or events.
pub fn redact_diagnostic(input: &str) -> String {
    let mut output = input.to_owned();
    for marker in [
        "api_key=",
        "apikey=",
        "access_token=",
        "refresh_token=",
        "authorization:",
        "authorization=",
        "bearer ",
        "basic ",
        "cookie:",
        "cookie=",
        "token=",
        "key=",
        "secret=",
    ] {
        output = redact_after_marker(&output, marker);
    }
    output = redact_prefixed_tokens(&output, "sk-");
    if output.len() > MAX_DIAGNOSTIC_BYTES {
        output.truncate(floor_char_boundary(&output, MAX_DIAGNOSTIC_BYTES));
        output.push_str("…[TRUNCATED]");
    }
    output
}

fn redact_after_marker(input: &str, marker: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let mut cursor = 0;
    let mut output = String::with_capacity(input.len());
    while let Some(relative) = lower[cursor..].find(marker) {
        let start = cursor + relative;
        let value_start = start + marker.len();
        output.push_str(&input[cursor..value_start]);
        output.push_str("[REDACTED]");
        let end = input[value_start..]
            .find(|character: char| {
                character.is_whitespace() || matches!(character, ';' | ',' | '"' | '\'' | '}')
            })
            .map_or(input.len(), |relative| value_start + relative);
        cursor = end;
    }
    output.push_str(&input[cursor..]);
    output
}

fn redact_prefixed_tokens(input: &str, prefix: &str) -> String {
    let mut cursor = 0;
    let mut output = String::with_capacity(input.len());
    while let Some(relative) = input[cursor..].find(prefix) {
        let start = cursor + relative;
        output.push_str(&input[cursor..start]);
        output.push_str("[REDACTED]");
        let end = input[start..]
            .find(|character: char| {
                character.is_whitespace() || matches!(character, ';' | ',' | '"' | '\'' | '}')
            })
            .map_or(input.len(), |relative| start + relative);
        cursor = end;
    }
    output.push_str(&input[cursor..]);
    output
}

fn floor_char_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

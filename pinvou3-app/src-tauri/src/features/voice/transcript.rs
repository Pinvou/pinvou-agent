pub(crate) fn has_usable_asr_text(text: &str) -> bool {
    text.chars().any(|ch| ch.is_alphanumeric())
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
}

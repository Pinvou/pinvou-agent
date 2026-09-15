fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn language_preference(value: Option<&str>) -> Option<&str> {
    value?
        .split(':')
        .map(str::trim)
        .find(|value| !value.is_empty())
}

fn is_portable_locale(value: &str) -> bool {
    let bytes = value.as_bytes();
    value.eq_ignore_ascii_case("C")
        || value.eq_ignore_ascii_case("POSIX")
        || (bytes.len() > 2 && bytes[0].eq_ignore_ascii_case(&b'c') && bytes[1] == b'.')
}

/// Selects the locale used for GNU-style translated messages.
///
/// `LANGUAGE` overrides an explicitly configured category locale, except when
/// the effective base locale is the untranslated `C`/`POSIX` locale. A missing
/// base locale also means the portable default and therefore ignores
/// `LANGUAGE`. Keeping this logic pure makes the environment precedence
/// testable without mutating process-global state.
fn select_message_locale(
    language: Option<&str>,
    lc_all: Option<&str>,
    lc_messages: Option<&str>,
    lang: Option<&str>,
) -> Option<String> {
    let base_locale = [lc_all, lc_messages, lang]
        .into_iter()
        .find_map(non_empty)?;

    if is_portable_locale(base_locale) {
        return Some(base_locale.to_owned());
    }

    language_preference(language)
        .or(Some(base_locale))
        .map(str::to_owned)
}

pub(crate) fn current_system_locale() -> Option<String> {
    let language = std::env::var("LANGUAGE").ok();
    let lc_all = std::env::var("LC_ALL").ok();
    let lc_messages = std::env::var("LC_MESSAGES").ok();
    let lang = std::env::var("LANG").ok();

    select_message_locale(
        language.as_deref(),
        lc_all.as_deref(),
        lc_messages.as_deref(),
        lang.as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_precedes_non_portable_base_locale() {
        assert_eq!(
            select_message_locale(
                Some("ja_JP"),
                Some("en_US.UTF-8"),
                Some("zh_CN.UTF-8"),
                Some("de_DE.UTF-8"),
            ),
            Some("ja_JP".to_string())
        );
    }

    #[test]
    fn base_locale_uses_category_precedence_without_language() {
        assert_eq!(
            select_message_locale(
                Some("  "),
                Some("en_US.UTF-8"),
                Some("ja_JP.UTF-8"),
                Some("zh_CN.UTF-8"),
            ),
            Some("en_US.UTF-8".to_string())
        );
        assert_eq!(
            select_message_locale(None, Some(""), Some("ja_JP.UTF-8"), Some("zh_CN.UTF-8")),
            Some("ja_JP.UTF-8".to_string())
        );
        assert_eq!(
            select_message_locale(None, None, None, Some("zh_CN.UTF-8")),
            Some("zh_CN.UTF-8".to_string())
        );
    }

    #[test]
    fn language_uses_first_non_empty_colon_list_entry() {
        assert_eq!(
            select_message_locale(Some(" : ja_JP : zh_CN "), None, None, Some("en_US.UTF-8")),
            Some("ja_JP".to_string())
        );
    }

    #[test]
    fn c_base_locale_ignores_language() {
        assert_eq!(
            select_message_locale(Some("ja_JP"), Some(" c "), Some("zh_CN"), Some("en_US")),
            Some("c".to_string())
        );
    }

    #[test]
    fn c_encoding_base_locales_ignore_language() {
        assert_eq!(
            select_message_locale(Some("ja_JP"), Some("C.UTF-8"), None, None),
            Some("C.UTF-8".to_string())
        );
        assert_eq!(
            select_message_locale(Some("ja_JP"), None, Some("C.utf8"), None),
            Some("C.utf8".to_string())
        );
        assert_eq!(
            select_message_locale(Some("ja_JP"), None, None, Some("c.UtF-8")),
            Some("c.UtF-8".to_string())
        );
    }

    #[test]
    fn locale_starting_with_c_is_not_automatically_portable() {
        assert_eq!(
            select_message_locale(Some("ja_JP"), Some("ca_ES.UTF-8"), None, None),
            Some("ja_JP".to_string())
        );
    }

    #[test]
    fn posix_base_locale_ignores_language() {
        assert_eq!(
            select_message_locale(Some("ja_JP"), None, Some("POSIX"), Some("zh_CN")),
            Some("POSIX".to_string())
        );
    }

    #[test]
    fn empty_values_do_not_select_a_locale() {
        assert_eq!(
            select_message_locale(Some(" :  : "), Some(" "), Some(""), None),
            None
        );
    }

    #[test]
    fn language_without_an_explicit_base_locale_is_ignored() {
        assert_eq!(select_message_locale(Some("ja_JP"), None, None, None), None);
        assert_eq!(
            select_message_locale(Some("ja_JP"), Some(" "), Some(""), Some("  ")),
            None
        );
    }
}

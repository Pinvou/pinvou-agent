//! App composition barrier, invoked as the first statement in lib::run.
//! Failure presentation uses immutable shared UI copy and read-only OS locale;
//! it must never load/migrate preferences, initialize logs or create a WebView.
use crate::platform::prefs::Language;
use serde::Deserialize;
use std::sync::OnceLock;

#[must_use = "retain the successful startup barrier for the full App runtime"]
pub(crate) enum StartupWriterAdmission {
    #[cfg(not(any(feature = "upgrade-host-lab", feature = "upgrade-host-production")))]
    DisabledBuild,
    #[cfg(any(feature = "upgrade-host-lab", feature = "upgrade-host-production"))]
    Host {
        _admission: pinvou_upgrade_host::AppWriterStartup,
    },
}

pub(crate) fn before_any_writer() -> Result<StartupWriterAdmission, &'static str> {
    #[cfg(any(feature = "upgrade-host-lab", feature = "upgrade-host-production"))]
    {
        pinvou_upgrade_host::admit_current_app_writer()
            .map(|admission| StartupWriterAdmission::Host {
                _admission: admission,
            })
            .map_err(pinvou_upgrade_host::Error::code)
    }
    #[cfg(not(any(feature = "upgrade-host-lab", feature = "upgrade-host-production")))]
    {
        // Existing explicit community configuration: no host dependency/profile
        // is compiled. This is distinct from a configured host that failed.
        Ok(StartupWriterAdmission::DisabledBuild)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Copy {
    title: String,
    message: String,
    code: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    zh: Copy,
    en: Copy,
    ja: Copy,
}

#[allow(clippy::expect_used)]
fn catalog() -> &'static Catalog {
    static COPY: OnceLock<Catalog> = OnceLock::new();
    COPY.get_or_init(|| {
        // Trusted compiled source, also imported by all three existing JS
        // dictionaries. Missing/invalid languages are a source defect tested
        // below; runtime settings or caller data cannot replace this resource.
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../src/shared/i18n/startup-admission.js"
        ));
        // This shared module contains one JSON-valued constant and its export.
        // Keeping a
        // plain ES2021 module also supports direct Node dictionary imports.
        // Trim preserves the same grammar for LF and Windows CRLF checkouts.
        let source = source.replace("\r\n", "\n");
        let json = source
            .trim()
            .strip_prefix("const startupAdmission = ")
            .and_then(|body| body.strip_suffix(";\n\nexport default startupAdmission;"))
            .expect("embedded startup catalog must be a JSON-valued constant");
        serde_json::from_str(json)
            .expect("embedded startup admission translations must be complete")
    })
}
fn localized(language: Language, code: &str) -> (&'static str, String) {
    let copy = match language {
        Language::ZhHans => &catalog().zh,
        Language::En => &catalog().en,
        Language::Ja => &catalog().ja,
    };
    (
        &copy.title,
        format!("{}\n\n{}", copy.message, copy.code.replace("{code}", code)),
    )
}

/// No best-effort continuation: the main entry terminates after this returns.
/// Error codes are the host's fixed nonsecret codes, not native paths or IDs.
pub(crate) fn show_failure(code: &'static str) {
    let locale = crate::platform::os::current_system_locale();
    let language = Language::from_system_locale(locale.as_deref());
    let (title, message) = localized(language, code);
    super::platform::show_startup_admission_failure(title, &message);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_startup_copy_has_each_supported_language_without_dictionary_fallback() {
        for language in [Language::ZhHans, Language::En, Language::Ja] {
            let (title, message) = localized(language, "HOST_BUSY");
            assert!(!title.is_empty());
            assert!(message.contains("HOST_BUSY"));
            assert!(!message.contains("{code}"));
        }
        assert_ne!(catalog().zh.message, catalog().en.message);
        assert_ne!(catalog().ja.message, catalog().en.message);
    }
}

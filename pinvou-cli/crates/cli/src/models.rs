//! `models` + `settings` families (GUI parity). The `settings` top-level token
//! is an alias routed here by the dispatcher (values[0] == "settings"); parse
//! enforces that the alias only accepts settings subcommands while the
//! `models` token only accepts models subcommands.
//!
//! Pure-storage operations (everything except the connection probes) call the
//! same feature-layer building blocks the GUI commands in
//! `pinvou3-app/src-tauri/src/app/commands/settings.rs` use:
//! `pinvou3_lib::platform::prefs::UserPrefs` load -> mutate -> save via
//! `update_transaction` (process lock + atomic write + migrations +
//! memory-locale policy) and `pinvou3_lib::platform::credential_store` for
//! API keys. No Tauri host is booted.
//!
//! The connection probes (`models test`, `models probe-local`,
//! `settings search test`) mirror the standalone reqwest probes in
//! app/commands/settings.rs (`probe_model_connection`, `probe_local_server_kind`,
//! `test_search_provider`). Those functions live in the app-private `app` /
//! `core` modules and are not exported through `pinvou3_lib`, so the exact
//! semantics are reimplemented here on a blocking reqwest client (no async
//! runtime). They are the only paths that touch the network and are covered
//! by `#[ignore]` tests only.

use std::collections::BTreeMap;
use std::time::Duration;

use pinvou3_lib::features::sessions::SerializableMode;
use pinvou3_lib::platform::credential_store::{
    CredentialState, CredentialStore, SystemCredentialStore, redact_secret,
};
use pinvou3_lib::platform::prefs::{
    ColorScheme, Language, ModelPreset, SavedModel, SearchProvider, Theme, UserPrefs,
};

use crate::support::{render, require_yes, resolve_secret, success};
use crate::{CliError, CliOutcome, ExitCode, OutputMode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelsCommand {
    List,
    Add {
        preset: ModelPreset,
        name: String,
        model: String,
        base_url: String,
        api_key_env: Option<String>,
        api_key_stdin: bool,
        context_window: Option<u32>,
        max_output: Option<u32>,
        reasoning_effort: Option<String>,
        set_active: bool,
    },
    Remove {
        id: String,
        yes: bool,
    },
    Use {
        id: String,
    },
    Show {
        id: String,
        reveal_key: bool,
    },
    Test {
        id: String,
    },
    ProbeLocal {
        url: Option<String>,
    },
    SettingsGet {
        key: Option<SettingsKey>,
    },
    SettingsSet {
        key: SettingsKey,
        value: SettingsValue,
    },
    SearchList,
    SearchSet {
        provider: SearchProvider,
        api_key_env: Option<String>,
        clear: bool,
    },
    SearchTest {
        provider: SearchProvider,
    },
}

/// A settings key accepted by `settings get` / `settings set`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsKey {
    Theme,
    ColorScheme,
    Language,
    MemoryEnabled,
    NotificationsEnabled,
    NotificationsTaskCompleted,
    SidebarDateGrouping,
    ModeDefaultsWork,
    CodePermissionLastMode,
    AdvancedAllowShell,
    VoiceShortcutEnabled,
    PetEnabled,
}

impl SettingsKey {
    const ALL: &'static [(&'static str, SettingsKey)] = &[
        ("theme", SettingsKey::Theme),
        ("color_scheme", SettingsKey::ColorScheme),
        ("language", SettingsKey::Language),
        ("memory_enabled", SettingsKey::MemoryEnabled),
        ("notifications.enabled", SettingsKey::NotificationsEnabled),
        (
            "notifications.task_completed",
            SettingsKey::NotificationsTaskCompleted,
        ),
        ("sidebar.date_grouping", SettingsKey::SidebarDateGrouping),
        ("mode_defaults.work", SettingsKey::ModeDefaultsWork),
        (
            "code_permission.last_mode",
            SettingsKey::CodePermissionLastMode,
        ),
        ("advanced.allow_shell", SettingsKey::AdvancedAllowShell),
        ("voice_shortcut_enabled", SettingsKey::VoiceShortcutEnabled),
        ("pet.enabled", SettingsKey::PetEnabled),
    ];

    fn parse(raw: &str) -> Result<Self, CliError> {
        Self::ALL
            .iter()
            .find(|(name, _)| *name == raw)
            .map(|(_, key)| *key)
            .ok_or_else(|| {
                CliError::usage(format!(
                    "unknown settings key {raw:?}; valid keys: {}",
                    Self::ALL
                        .iter()
                        .map(|(name, _)| *name)
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })
    }
}

/// A typed `settings set` value, validated at parse time so bad types and
/// values exit with code 2 naming the accepted values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsValue {
    Theme(Theme),
    ColorScheme(ColorScheme),
    Language(Language),
    Bool(bool),
    /// `plan | yolo | none` (`none` clears the Option).
    Mode(Option<SerializableMode>),
    /// `true | false | none` (only `advanced.allow_shell`).
    OptionalBool(Option<bool>),
}

const MODEL_PRESETS: &[&str] = &[
    "local_vllm",
    "deepseek",
    "kimi",
    "openai_compatible",
    "qwen",
    "doubao",
    "minimax",
    "glm",
    "mimo",
    "openai",
    "anthropic",
    "gemini",
    "xai",
];

fn parse_preset(raw: &str) -> Result<ModelPreset, CliError> {
    let preset = match raw {
        "local_vllm" => ModelPreset::LocalVllm,
        "deepseek" => ModelPreset::Deepseek,
        "kimi" => ModelPreset::Kimi,
        "openai_compatible" => ModelPreset::OpenaiCompatible,
        "qwen" => ModelPreset::Qwen,
        "doubao" => ModelPreset::Doubao,
        "minimax" => ModelPreset::Minimax,
        "glm" => ModelPreset::Glm,
        "mimo" => ModelPreset::Mimo,
        "openai" => ModelPreset::Openai,
        "anthropic" => ModelPreset::Anthropic,
        "gemini" => ModelPreset::Gemini,
        "xai" => ModelPreset::Xai,
        other => {
            return Err(CliError::usage(format!(
                "unknown preset {other:?}; valid presets: {}",
                MODEL_PRESETS.join(", ")
            )));
        }
    };
    Ok(preset)
}

fn parse_reasoning_effort(raw: &str) -> Result<String, CliError> {
    match raw {
        "off" | "low" | "medium" | "high" | "max" => Ok(raw.to_owned()),
        other => Err(CliError::usage(format!(
            "invalid reasoning effort {other:?}; valid values: off, low, medium, high, max"
        ))),
    }
}

fn parse_positive_u32(flag: &str, raw: &str) -> Result<u32, CliError> {
    raw.parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| CliError::usage(format!("{flag} must be a positive integer")))
}

fn parse_bool(raw: &str) -> Result<bool, CliError> {
    match raw {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(CliError::usage(format!(
            "expected true or false, got {other:?}"
        ))),
    }
}

fn parse_mode(raw: &str) -> Result<Option<SerializableMode>, CliError> {
    match raw {
        "plan" => Ok(Some(SerializableMode::Plan)),
        "yolo" => Ok(Some(SerializableMode::Yolo)),
        "none" => Ok(None),
        other => Err(CliError::usage(format!(
            "expected plan, yolo or none, got {other:?}"
        ))),
    }
}

fn parse_theme(raw: &str) -> Result<Theme, CliError> {
    match raw {
        "genesis" => Ok(Theme::Genesis),
        // Accept the spec's snake_case spelling as an alias of the canonical
        // kebab-case settings.json value.
        "liquid-light" | "liquid_light" => Ok(Theme::LiquidLight),
        "liquid-dark" | "liquid_dark" => Ok(Theme::LiquidDark),
        other => Err(CliError::usage(format!(
            "invalid theme {other:?}; valid values: genesis, liquid-light, liquid-dark"
        ))),
    }
}

fn parse_color_scheme(raw: &str) -> Result<ColorScheme, CliError> {
    match raw {
        "light" => Ok(ColorScheme::Light),
        "dark" => Ok(ColorScheme::Dark),
        "system" => Ok(ColorScheme::System),
        other => Err(CliError::usage(format!(
            "invalid color scheme {other:?}; valid values: light, dark, system"
        ))),
    }
}

fn parse_language(raw: &str) -> Result<Language, CliError> {
    match raw {
        "zh-Hans" => Ok(Language::ZhHans),
        "en" => Ok(Language::En),
        "ja" => Ok(Language::Ja),
        other => Err(CliError::usage(format!(
            "invalid language {other:?}; valid values: zh-Hans, en, ja"
        ))),
    }
}

fn parse_search_provider(raw: &str) -> Result<SearchProvider, CliError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "bing" => Ok(SearchProvider::Bing),
        "metaso" => Ok(SearchProvider::Metaso),
        "bocha" => Ok(SearchProvider::Bocha),
        "baidu" => Ok(SearchProvider::Baidu),
        "tavily" => Ok(SearchProvider::Tavily),
        other => Err(CliError::usage(format!(
            "unsupported search provider {other:?}; valid providers: bing, metaso, bocha, baidu, tavily"
        ))),
    }
}

/// Typed value for one settings key; rejects type/value mismatches with a
/// usage error naming the accepted values.
fn parse_settings_value(key: SettingsKey, raw: &str) -> Result<SettingsValue, CliError> {
    let usage = |expected: &str| {
        CliError::usage(format!(
            "invalid value {raw:?} for {}; expected {expected}",
            SettingsKey::ALL
                .iter()
                .find(|(_, candidate)| *candidate == key)
                .map(|(name, _)| *name)
                .unwrap_or_default(),
        ))
    };
    match key {
        SettingsKey::Theme => parse_theme(raw).map(SettingsValue::Theme),
        SettingsKey::ColorScheme => parse_color_scheme(raw).map(SettingsValue::ColorScheme),
        SettingsKey::Language => parse_language(raw).map(SettingsValue::Language),
        SettingsKey::MemoryEnabled
        | SettingsKey::NotificationsEnabled
        | SettingsKey::NotificationsTaskCompleted
        | SettingsKey::SidebarDateGrouping
        | SettingsKey::VoiceShortcutEnabled
        | SettingsKey::PetEnabled => parse_bool(raw)
            .map(SettingsValue::Bool)
            .map_err(|_| usage("true | false")),
        SettingsKey::ModeDefaultsWork | SettingsKey::CodePermissionLastMode => parse_mode(raw)
            .map(SettingsValue::Mode)
            .map_err(|_| usage("plan | yolo | none")),
        SettingsKey::AdvancedAllowShell => parse_optional_bool(raw)
            .map(SettingsValue::OptionalBool)
            .map_err(|_| usage("true | false | none")),
    }
}

fn parse_optional_bool(raw: &str) -> Result<Option<bool>, CliError> {
    match raw {
        "none" => Ok(None),
        other => parse_bool(other).map(Some),
    }
}

/// Parsed `--flag value` / boolean `--flag` options for one subcommand.
struct Options {
    values: BTreeMap<String, String>,
    flags: Vec<String>,
    positionals: Vec<String>,
}

fn parse_options(
    family: &str,
    subcommand: &str,
    rest: &[String],
    value_flags: &[&str],
    flag_flags: &[&str],
) -> Result<Options, CliError> {
    let mut values = BTreeMap::new();
    let mut flags = Vec::new();
    let mut positionals = Vec::new();
    let mut index = 0;
    while index < rest.len() {
        let argument = &rest[index];
        if let Some(name) = argument.strip_prefix("--") {
            if value_flags.contains(&name) {
                let value = rest
                    .get(index + 1)
                    .ok_or_else(|| CliError::usage(format!("--{name} requires a value")))?;
                if values.insert(name.to_owned(), value.clone()).is_some() {
                    return Err(CliError::usage(format!(
                        "--{name} was given more than once"
                    )));
                }
                index += 2;
            } else if flag_flags.contains(&name) {
                flags.push(name.to_owned());
                index += 1;
            } else {
                return Err(CliError::usage(format!(
                    "unknown option --{name} for pinvou {family} {subcommand}"
                )));
            }
        } else {
            positionals.push(argument.clone());
            index += 1;
        }
    }
    Ok(Options {
        values,
        flags,
        positionals,
    })
}

impl Options {
    fn has(&self, name: &str) -> bool {
        self.flags.iter().any(|candidate| candidate == name)
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    fn required(&self, name: &str) -> Result<&str, CliError> {
        self.value(name)
            .ok_or_else(|| CliError::usage(format!("--{name} is required")))
    }

    fn exactly_one_positional(&self, what: &str) -> Result<String, CliError> {
        match self.positionals.len() {
            1 => Ok(self.positionals[0].clone()),
            0 => Err(CliError::usage(format!(
                "pinvou models {what} requires an id"
            ))),
            n => Err(CliError::usage(format!(
                "pinvou models {what} takes one id, got {n}"
            ))),
        }
    }
}

const MODELS_USAGE: &str = "usage: pinvou models <list|add|remove|use|show|test|probe-local>";
const SETTINGS_USAGE: &str =
    "usage: pinvou settings <get|set|search>; search subcommands: list|set|test";

pub fn parse(values: &[String]) -> Result<ModelsCommand, CliError> {
    let family = values[0].as_str();
    let subcommand = values.get(1).cloned().ok_or_else(|| {
        CliError::usage(if family == "settings" {
            SETTINGS_USAGE.to_owned()
        } else {
            MODELS_USAGE.to_owned()
        })
    })?;
    let rest = values.get(2..).unwrap_or_default();
    match (family, subcommand.as_str()) {
        ("models", "list") => {
            let options = parse_options("models", "list", rest, &[], &[])?;
            ensure_no_positionals(&options, "models list")?;
            Ok(ModelsCommand::List)
        }
        ("models", "add") => parse_add(rest),
        ("models", "remove") => {
            let options = parse_options("models", "remove", rest, &[], &["yes"])?;
            Ok(ModelsCommand::Remove {
                id: options.exactly_one_positional("remove")?,
                yes: options.has("yes"),
            })
        }
        ("models", "use") => {
            let options = parse_options("models", "use", rest, &[], &[])?;
            Ok(ModelsCommand::Use {
                id: options.exactly_one_positional("use")?,
            })
        }
        ("models", "show") => {
            let options = parse_options("models", "show", rest, &[], &["reveal-key"])?;
            Ok(ModelsCommand::Show {
                id: options.exactly_one_positional("show")?,
                reveal_key: options.has("reveal-key"),
            })
        }
        ("models", "test") => {
            let options = parse_options("models", "test", rest, &[], &[])?;
            Ok(ModelsCommand::Test {
                id: options.exactly_one_positional("test")?,
            })
        }
        ("models", "probe-local") => {
            let options = parse_options("models", "probe-local", rest, &["url"], &[])?;
            ensure_no_positionals(&options, "models probe-local")?;
            Ok(ModelsCommand::ProbeLocal {
                url: options.value("url").map(str::to_owned),
            })
        }
        ("settings", "get") => {
            let options = parse_options("settings", "get", rest, &[], &[])?;
            let key = match options.positionals.len() {
                0 => None,
                1 => Some(SettingsKey::parse(&options.positionals[0])?),
                n => {
                    return Err(CliError::usage(format!(
                        "pinvou settings get takes at most one key, got {n}"
                    )));
                }
            };
            Ok(ModelsCommand::SettingsGet { key })
        }
        ("settings", "set") => {
            let options = parse_options("settings", "set", rest, &[], &[])?;
            if options.positionals.len() != 2 {
                return Err(CliError::usage("usage: pinvou settings set <key> <value>"));
            }
            let key = SettingsKey::parse(&options.positionals[0])?;
            let value = parse_settings_value(key, &options.positionals[1])?;
            Ok(ModelsCommand::SettingsSet { key, value })
        }
        ("settings", "search") => parse_search(rest),
        ("models", _) => Err(CliError::usage(MODELS_USAGE)),
        ("settings", _) => Err(CliError::usage(SETTINGS_USAGE)),
        _ => unreachable!("dispatcher only routes models/settings tokens here"),
    }
}

fn ensure_no_positionals(options: &Options, what: &str) -> Result<(), CliError> {
    if options.positionals.is_empty() {
        Ok(())
    } else {
        Err(CliError::usage(format!("pinvou {what} takes no arguments")))
    }
}

fn parse_add(rest: &[String]) -> Result<ModelsCommand, CliError> {
    let options = parse_options(
        "models",
        "add",
        rest,
        &[
            "preset",
            "name",
            "model",
            "base-url",
            "api-key-env",
            "context-window",
            "max-output",
            "reasoning-effort",
        ],
        &["api-key-stdin", "set-active"],
    )?;
    if options.value("api-key-env").is_some() && options.has("api-key-stdin") {
        return Err(CliError::usage(
            "use only one of --api-key-env or --api-key-stdin",
        ));
    }
    let context_window = match options.value("context-window") {
        Some(raw) => Some(parse_positive_u32("--context-window", raw)?),
        None => None,
    };
    let max_output = match options.value("max-output") {
        Some(raw) => Some(parse_positive_u32("--max-output", raw)?),
        None => None,
    };
    let reasoning_effort = match options.value("reasoning-effort") {
        Some(raw) => Some(parse_reasoning_effort(raw)?),
        None => None,
    };
    Ok(ModelsCommand::Add {
        preset: parse_preset(options.required("preset")?)?,
        name: options.required("name")?.trim().to_owned(),
        model: options.required("model")?.trim().to_owned(),
        base_url: options.required("base-url")?.trim().to_owned(),
        api_key_env: options.value("api-key-env").map(str::to_owned),
        api_key_stdin: options.has("api-key-stdin"),
        context_window,
        max_output,
        reasoning_effort,
        set_active: options.has("set-active"),
    })
}

fn parse_search(rest: &[String]) -> Result<ModelsCommand, CliError> {
    let search_subcommand = rest
        .first()
        .cloned()
        .ok_or_else(|| CliError::usage("usage: pinvou settings search <list|set|test>"))?;
    let rest = rest.get(1..).unwrap_or_default();
    match search_subcommand.as_str() {
        "list" => {
            let options = parse_options("settings", "search list", rest, &[], &[])?;
            ensure_no_positionals(&options, "settings search list")?;
            Ok(ModelsCommand::SearchList)
        }
        "set" => {
            let options = parse_options(
                "settings",
                "search set",
                rest,
                &["provider", "api-key-env"],
                &["clear"],
            )?;
            if options.has("clear") && options.value("api-key-env").is_some() {
                return Err(CliError::usage("use only one of --api-key-env or --clear"));
            }
            Ok(ModelsCommand::SearchSet {
                provider: parse_search_provider(options.required("provider")?)?,
                api_key_env: options.value("api-key-env").map(str::to_owned),
                clear: options.has("clear"),
            })
        }
        "test" => {
            let options = parse_options("settings", "search test", rest, &[], &[])?;
            let provider = options.exactly_one_positional("search test <provider>")?;
            Ok(ModelsCommand::SearchTest {
                provider: parse_search_provider(&provider)?,
            })
        }
        _ => Err(CliError::usage(
            "usage: pinvou settings search <list|set|test>",
        )),
    }
}

pub fn execute(command: ModelsCommand, output: OutputMode) -> Result<CliOutcome, CliError> {
    match command {
        ModelsCommand::List => list(output),
        ModelsCommand::Add {
            preset,
            name,
            model,
            base_url,
            api_key_env,
            api_key_stdin,
            context_window,
            max_output,
            reasoning_effort,
            set_active,
        } => add(
            preset,
            &name,
            &model,
            &base_url,
            &api_key_env,
            api_key_stdin,
            context_window,
            max_output,
            reasoning_effort,
            set_active,
            output,
        ),
        ModelsCommand::Remove { id, yes } => remove(&id, yes, output),
        ModelsCommand::Use { id } => use_model(&id, output),
        ModelsCommand::Show { id, reveal_key } => show(&id, reveal_key, output),
        ModelsCommand::Test { id } => test_connection(&id, output),
        ModelsCommand::ProbeLocal { url } => probe_local(url.as_deref(), output),
        ModelsCommand::SettingsGet { key } => settings_get(key, output),
        ModelsCommand::SettingsSet { key, value } => settings_set(key, value, output),
        ModelsCommand::SearchList => search_list(output),
        ModelsCommand::SearchSet {
            provider,
            api_key_env,
            clear,
        } => search_set(provider, &api_key_env, clear, output),
        ModelsCommand::SearchTest { provider } => search_test(provider, output),
    }
}

/// Read-latest-prefs the same way the GUI's `refresh_safe_prefs` does:
/// normalize metadata, re-read credential states from the credential store
/// and strip plaintext keys before anything is rendered. Models without a
/// `credential_ref` never touch the credential backend.
fn safe_prefs() -> UserPrefs {
    let mut prefs = UserPrefs::load();
    prefs.normalize_saved_model_metadata();
    prefs.refresh_credential_states_with_store(&SystemCredentialStore::new());
    prefs.sanitize_plaintext_api_keys();
    prefs
}

fn model_entry_json(model: &SavedModel, active_id: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "id": model.id,
        "name": model.name,
        "preset": model.preset.as_str(),
        "model": model.model,
        "base_url": model.base_url,
        "context_window": model.context_window_tokens,
        "max_output": model.max_output_tokens,
        "reasoning_effort": model.reasoning_effort,
        "active": active_id == Some(model.id.as_str()),
        "has_secret": model.has_secret,
    })
}

fn optional_u32(value: Option<u32>) -> String {
    value.map_or_else(|| "none".to_owned(), |tokens| tokens.to_string())
}

fn list(output: OutputMode) -> Result<CliOutcome, CliError> {
    let prefs = safe_prefs();
    let active_id = prefs.active_model().map(|model| model.id.as_str());
    let models = &prefs.advanced.saved_models;
    let entries: Vec<serde_json::Value> = models
        .iter()
        .map(|model| model_entry_json(model, active_id))
        .collect();
    let human = models
        .iter()
        .map(|model| {
            let marker = if active_id == Some(model.id.as_str()) {
                "*"
            } else {
                "-"
            };
            format!(
                "{marker}{}\t{}\t{}\t{}\t{}\tcontext_window={}\tmax_output={}\treasoning_effort={}\thas_secret={}",
                model.id,
                model.name,
                model.preset.as_str(),
                model.model,
                model.base_url,
                optional_u32(model.context_window_tokens),
                optional_u32(model.max_output_tokens),
                model.reasoning_effort.as_deref().unwrap_or("default"),
                model.has_secret,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let text = render(output, human, &serde_json::json!({ "models": entries }));
    Ok(success(text))
}

/// New model id using the GUI's scheme (`m_` + epoch-millis base36 + a short
/// random suffix); the random suffix is derived from sub-second nanos plus a
/// process-local counter to avoid a new dependency.
fn new_model_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let millis = elapsed.as_millis() as u64;
    let salt =
        ((elapsed.subsec_nanos() as u64) << 8) | (SEQUENCE.fetch_add(1, Ordering::Relaxed) & 0xff);
    format!("m_{}{}", to_base36(millis), to_base36(salt))
}

fn to_base36(mut value: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(DIGITS[(value % 36) as usize]);
        value /= 36;
        if value == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// Credential bookkeeping identical to the GUI's `apply_model_credential`
/// with `old = None` (a fresh model): an empty key means "no secret"
/// (`mark_missing`), a non-empty key is stored under the model's credential
/// reference in the platform credential store and marked configured. The
/// plaintext key never reaches settings.json (`clear_plaintext_key`).
fn apply_new_model_credential(mut model: SavedModel) -> Result<SavedModel, String> {
    if model.api_key.trim().is_empty() {
        model.mark_missing();
    } else {
        let reference = model.credential_reference();
        SystemCredentialStore::new()
            .set(&reference, model.api_key.trim())
            .map_err(|error| error.user_message())?;
        model.mark_configured(reference);
    }
    Ok(model)
}

#[allow(clippy::too_many_arguments)]
fn add(
    preset: ModelPreset,
    name: &str,
    model: &str,
    base_url: &str,
    api_key_env: &Option<String>,
    api_key_stdin: bool,
    context_window: Option<u32>,
    max_output: Option<u32>,
    reasoning_effort: Option<String>,
    set_active: bool,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    if name.is_empty() || model.is_empty() || base_url.is_empty() {
        return Err(CliError::usage(
            "--name, --model and --base-url must not be empty",
        ));
    }
    // Resolved before any prefs mutation so a missing environment variable is
    // reported without side effects.
    let secret = resolve_secret(api_key_env, api_key_stdin)?;
    let stores_secret = secret.is_some();
    let id = new_model_id();
    let saved = SavedModel {
        id: id.clone(),
        name: name.to_owned(),
        alias: None,
        preset,
        context_window_tokens: context_window,
        max_output_tokens: max_output,
        reasoning_effort,
        model: model.to_owned(),
        base_url: base_url.to_owned(),
        provider_kind: None,
        vendor: None,
        endpoint_mode: None,
        image_capability_override: Default::default(),
        vision_model_id: None,
        api_key: secret.unwrap_or_default(),
        credential_ref: None,
        credential_state: CredentialState::Missing,
        has_secret: false,
        credential_action: None,
    };
    let active_id = id.clone();
    let transaction = UserPrefs::update_transaction(|prefs| {
        let saved = apply_new_model_credential(saved.clone())
            .map_err(|error| format!("credential store unavailable: {error}"))?;
        prefs.upsert_model(saved);
        if set_active {
            prefs.advanced.active_model_id = Some(active_id.clone());
        }
        Ok(())
    });
    if let Err(error) = transaction {
        // The closure may have stored the keyring secret before the save
        // failed; roll it back so no orphaned entry outlives the model.
        if stores_secret {
            let reference = saved.credential_reference();
            let _ = SystemCredentialStore::new().delete(&reference);
        }
        return Err(prefs_error(error));
    }
    let text = render(
        output,
        format!("id: {id}"),
        &serde_json::json!({ "id": id }),
    );
    Ok(success(text))
}

/// Classifies transaction failures: model-not-found and the min-1 rule are
/// argument-level problems (exit 2); everything else (I/O, credential store)
/// is a host failure (exit 1). Messages are already redacted by the
/// credential layer.
fn prefs_error(error: String) -> CliError {
    if error.starts_with("model not found") || error.starts_with("cannot remove the last") {
        CliError::usage(error)
    } else {
        CliError::failed(error)
    }
}

fn remove(id: &str, yes: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    require_yes(yes)?;
    UserPrefs::update_transaction(|prefs| {
        if prefs.model_by_id(id).is_none() {
            return Err(format!("model not found: {id}"));
        }
        if prefs.advanced.saved_models.len() <= 1 {
            return Err(
                "cannot remove the last remaining model; add another model first".to_owned(),
            );
        }
        if let Some(reference) = prefs
            .model_by_id(id)
            .and_then(|model| model.credential_ref.clone())
        {
            SystemCredentialStore::new()
                .delete(&reference)
                .map_err(|error| error.user_message())?;
        }
        prefs.remove_model(id);
        Ok(())
    })
    .map_err(prefs_error)?;
    let text = render(
        output,
        format!("removed: {id}"),
        &serde_json::json!({ "removed": id }),
    );
    Ok(success(text))
}

fn use_model(id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    UserPrefs::update_transaction(|prefs| {
        if prefs.model_by_id(id).is_none() {
            return Err(format!("model not found: {id}"));
        }
        prefs.advanced.active_model_id = Some(id.to_owned());
        Ok(())
    })
    .map_err(prefs_error)?;
    let text = render(
        output,
        format!("active: {id}"),
        &serde_json::json!({ "active": id }),
    );
    Ok(success(text))
}

fn find_model(prefs: &UserPrefs, id: &str) -> Result<SavedModel, CliError> {
    prefs
        .model_by_id(id)
        .cloned()
        .ok_or_else(|| CliError::usage(format!("model not found: {id}")))
}

/// Resolves a stored credential exactly like the GUI's
/// `resolve_saved_model_key`: read the reference out of the saved model and
/// fetch it from the credential store. A model without `credential_ref` has
/// no stored secret.
fn resolve_saved_model_key(model: &SavedModel) -> Result<Option<String>, String> {
    let Some(reference) = &model.credential_ref else {
        return Ok(None);
    };
    SystemCredentialStore::new()
        .get(reference)
        .map_err(|error| error.user_message())
}

fn show(id: &str, reveal_key: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    let prefs = safe_prefs();
    let model = find_model(&prefs, id)?;
    let active_id = prefs.active_model().map(|active| active.id.as_str());
    // The only path where a secret value is ever printed; --reveal-key mirrors
    // the GUI "reveal key" action (environment-overridden credentials are not
    // echoed, matching reveal_model_api_key).
    let revealed =
        if reveal_key {
            if model.credential_state == CredentialState::EnvOverride {
                None
            } else {
                Some(resolve_saved_model_key(&model).map_err(|error| {
                    CliError::failed(format!("credential_unavailable: {error}"))
                })?)
            }
        } else {
            None
        };
    let mut json = model_entry_json(&model, active_id);
    json["credential_state"] = serde_json::json!(model.credential_state);
    if reveal_key {
        json["api_key"] = serde_json::json!(revealed);
    }
    let mut human = format!(
        "id: {}\nname: {}\npreset: {}\nmodel: {}\nbase_url: {}\ncontext_window: {}\nmax_output: {}\nreasoning_effort: {}\nactive: {}\ncredential_state: {}\nhas_secret: {}",
        model.id,
        model.name,
        model.preset.as_str(),
        model.model,
        model.base_url,
        optional_u32(model.context_window_tokens),
        optional_u32(model.max_output_tokens),
        model.reasoning_effort.as_deref().unwrap_or("default"),
        active_id == Some(model.id.as_str()),
        credential_state_str(model.credential_state),
        model.has_secret,
    );
    if reveal_key {
        let value = revealed
            .flatten()
            .unwrap_or_else(|| "(not stored)".to_owned());
        human.push_str(&format!("\napi_key: {value}"));
    }
    Ok(success(render(output, human, &json)))
}

fn credential_state_str(state: CredentialState) -> &'static str {
    match state {
        CredentialState::Missing => "missing",
        CredentialState::Configured => "configured",
        CredentialState::EnvOverride => "env_override",
        CredentialState::NeedsMigration => "needs_migration",
        CredentialState::Unavailable => "unavailable",
    }
}

// ---------------------------------------------------------------------------
// Connection probes (network). Mirrors app/commands/settings.rs +
// core/model_endpoint.rs; not covered by default tests.
// ---------------------------------------------------------------------------

/// `GET {base}/models` result classification, mirroring
/// `ModelConnectionTestResult` with the GUI's stable snake_case codes.
struct ConnectionProbe {
    ok: bool,
    code: &'static str,
    detail: Option<String>,
    http_status: Option<u16>,
}

fn models_probe_url(base_url: &str) -> String {
    format!("{}/models", base_url.trim_end_matches('/'))
}

fn is_anthropic_host(url: &reqwest::Url) -> bool {
    url.host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("api.anthropic.com"))
}

fn connection_result(
    ok: bool,
    code: &'static str,
    detail: Option<String>,
    http_status: Option<u16>,
) -> ConnectionProbe {
    ConnectionProbe {
        ok,
        code,
        detail,
        http_status,
    }
}

fn connection_http_result(status: reqwest::StatusCode) -> ConnectionProbe {
    let status_code = status.as_u16();
    let detail = Some(format!("HTTP {status_code}"));
    if status.is_success() {
        return connection_result(true, "ok", detail, Some(status_code));
    }
    if status.is_redirection() {
        return connection_result(false, "redirect", detail, Some(status_code));
    }
    let code = match status_code {
        400 | 422 => "request_invalid",
        401 => "auth_invalid",
        403 => "auth_forbidden",
        404 => "endpoint_not_found",
        405 => "method_not_allowed",
        402 => "billing",
        408 => "timeout",
        429 => "rate_limited",
        500..=599 => "server_unavailable",
        _ => "http_error",
    };
    connection_result(false, code, detail, Some(status_code))
}

fn connection_error_result(error: &reqwest::Error) -> ConnectionProbe {
    let raw = redact_secret(&error.to_string());
    let lower = raw.to_lowercase();
    let code = if error.is_timeout() {
        "timeout"
    } else if lower.contains("certificate") || lower.contains("tls") || lower.contains("ssl") {
        "tls_error"
    } else if lower.contains("dns")
        || lower.contains("lookup")
        || lower.contains("name or service not known")
    {
        "dns_failed"
    } else if lower.contains("connection refused")
        || lower.contains("os error 10061")
        || lower.contains("actively refused")
    {
        "connection_refused"
    } else {
        "network_error"
    };
    connection_result(false, code, Some(raw), None)
}

/// `models test <id>`: GET `{base_url}/models` with the saved credential,
/// exactly like the GUI "test connection" button (8s timeout; Anthropic
/// official endpoints authenticate with x-api-key + anthropic-version).
fn test_connection(id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let prefs = safe_prefs();
    let model = find_model(&prefs, id)?;
    let key = resolve_saved_model_key(&model)
        .map_err(|error| CliError::failed(format!("credential_unavailable: {error}")))?
        .unwrap_or_default();
    let probe = run_connection_probe(&model.base_url, &key);
    let json = serde_json::json!({
        "ok": probe.ok,
        "code": probe.code,
        "detail": probe.detail,
        "http_status": probe.http_status,
    });
    let mut human = format!("ok: {}\ncode: {}", probe.ok, probe.code);
    if let Some(detail) = &probe.detail {
        human.push_str(&format!("\ndetail: {detail}"));
    }
    // A completed probe reports its result; the exit code reflects whether
    // the endpoint accepted the request (scripts branch on it).
    let outcome = CliOutcome {
        exit_code: if probe.ok {
            ExitCode::Success
        } else {
            ExitCode::Failed
        },
        stdout: render(output, human, &json),
    };
    Ok(outcome)
}

fn run_connection_probe(base_url: &str, key: &str) -> ConnectionProbe {
    let parsed_url = match reqwest::Url::parse(&models_probe_url(base_url)) {
        Ok(url) => url,
        Err(error) => {
            return connection_result(false, "invalid_url", Some(error.to_string()), None);
        }
    };
    let Ok(client) = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
    else {
        return connection_result(false, "client_error", None, None);
    };
    let mut request = client.get(parsed_url.clone());
    if !key.trim().is_empty() {
        request = if is_anthropic_host(&parsed_url) {
            request
                .header("x-api-key", key.trim())
                .header("anthropic-version", "2023-06-01")
        } else {
            request.bearer_auth(key.trim())
        };
    }
    match request.send() {
        Ok(response) => connection_http_result(response.status()),
        Err(error) => connection_error_result(&error),
    }
}

// --- local server kind probe (models probe-local) ---

/// Loopback-only guard for `probe-local`: the CLI refuses non-loopback hosts
/// with a usage error instead of the GUI's silent "generic" fallback, so a
/// typo can never send a probe request to a remote endpoint.
fn is_loopback_url(raw: &str) -> Result<bool, CliError> {
    let url = reqwest::Url::parse(raw.trim())
        .map_err(|error| CliError::usage(format!("probe-local url is not a valid url: {error}")))?;
    let Some(host) = url.host_str() else {
        return Err(CliError::usage("probe-local url has no host"));
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") {
        return Ok(true);
    }
    // Parse as an IP instead of string-prefix matching: "127.evil.com"
    // starts with "127." but resolves to a remote host.
    let Ok(address) = host.parse::<std::net::IpAddr>() else {
        return Ok(false);
    };
    Ok(address.is_loopback())
}

/// Strips a trailing `/v1` so native endpoints (`/api/tags`, `/props`, ...)
/// are reached at the API root; mirrors `strip_v1_suffix`.
fn strip_v1_suffix(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    trimmed
        .strip_suffix("/v1")
        .map_or_else(|| trimmed.to_owned(), str::to_owned)
}

fn probe_client() -> Option<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .ok()
}

fn apply_bearer(
    request: reqwest::blocking::RequestBuilder,
    bearer: Option<&str>,
) -> reqwest::blocking::RequestBuilder {
    match bearer.map(str::trim).filter(|key| !key.is_empty()) {
        Some(key) => request.bearer_auth(key),
        None => request,
    }
}

fn get_json(url: &str, bearer: Option<&str>) -> Option<serde_json::Value> {
    let response = apply_bearer(probe_client()?.get(url), bearer)
        .send()
        .ok()?
        .error_for_status()
        .ok()?;
    response.json::<serde_json::Value>().ok()
}

fn probe_ollama_tags(base_url: &str, bearer: Option<&str>) -> bool {
    get_json(&format!("{}/api/tags", strip_v1_suffix(base_url)), bearer).is_some_and(|value| {
        value
            .get("models")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|models| {
                models.iter().any(|item| {
                    item.get("name")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|name| !name.trim().is_empty())
                })
            })
    })
}

fn probe_lmstudio_v0(base_url: &str, bearer: Option<&str>) -> bool {
    get_json(
        &format!("{}/api/v0/models", strip_v1_suffix(base_url)),
        bearer,
    )
    .is_some_and(|value| {
        value
            .get("data")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|models| {
                models.iter().any(|item| {
                    item.get("id")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|id| !id.trim().is_empty())
                })
            })
    })
}

fn probe_koboldcpp(base_url: &str, bearer: Option<&str>) -> bool {
    get_json(
        &format!("{}/api/extra/version", strip_v1_suffix(base_url)),
        bearer,
    )
    .and_then(|value| {
        value
            .get("result")
            .and_then(serde_json::Value::as_str)
            .map(|result| result.to_ascii_lowercase().contains("koboldcpp"))
    })
    .unwrap_or(false)
}

fn probe_llamacpp(base_url: &str, bearer: Option<&str>) -> bool {
    get_json(&format!("{}/props", strip_v1_suffix(base_url)), bearer)
        .and_then(|value| value.as_object().cloned())
        .is_some_and(|object| {
            object.contains_key("default_generation_settings") && object.contains_key("total_slots")
        })
}

fn probe_sglang(base_url: &str, bearer: Option<&str>) -> bool {
    get_json(
        &format!("{}/get_server_info", strip_v1_suffix(base_url)),
        bearer,
    )
    .and_then(|value| {
        value
            .get("version")
            .and_then(serde_json::Value::as_str)
            .map(|_| true)
    })
    .unwrap_or(false)
}

fn is_docker_model_runner_port(base_url: &str) -> bool {
    reqwest::Url::parse(base_url.trim())
        .ok()
        .and_then(|url| url.port_or_known_default())
        == Some(12434)
}

/// Docker Model Runner management API lives at the host root; documented
/// addresses carry `/engines/v1`, `/engines` or `/v1` suffixes.
fn strip_docker_model_runner_root(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    for suffix in ["/engines/v1", "/engines", "/v1"] {
        if let Some(root) = trimmed.strip_suffix(suffix) {
            return root.to_owned();
        }
    }
    trimmed.to_owned()
}

fn probe_docker_model_runner(base_url: &str, bearer: Option<&str>) -> bool {
    if !is_docker_model_runner_port(base_url) {
        return false;
    }
    get_json(
        &format!("{}/models", strip_docker_model_runner_root(base_url)),
        bearer,
    )
    .is_some_and(|value| value.is_array())
}

/// OpenAI-compatible `/v1/models` body shared by the LMDeploy and vLLM
/// `owned_by` decisions; upstreams already ending in `/v1` get `/models`
/// appended, others get `/v1/models`.
fn fetch_v1_models(base_url: &str, bearer: Option<&str>) -> Option<serde_json::Value> {
    let trimmed = base_url.trim_end_matches('/');
    let url = if trimmed.ends_with("/v1") {
        format!("{trimmed}/models")
    } else {
        format!("{trimmed}/v1/models")
    };
    get_json(&url, bearer)
}

fn v1_models_owned_by_matches(value: &serde_json::Value, expected: &str) -> bool {
    value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("owned_by")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|owned| owned.eq_ignore_ascii_case(expected))
            })
        })
}

/// Local server kind selection in the same signature-exclusivity priority as
/// `select_local_server_kind`: DMR port gate > Ollama > LM Studio > KoboldCpp
/// > llama.cpp > SGLang > LMDeploy > vLLM > generic. Sequential with early
/// return instead of `tokio::join!` (the CLI has no async runtime); each
/// candidate's hit is independent, so the selected kind is identical.
fn select_local_server_kind(base_url: &str, bearer: Option<&str>) -> &'static str {
    if probe_docker_model_runner(base_url, bearer) {
        return "dockermodelrunner";
    }
    if probe_ollama_tags(base_url, bearer) {
        return "ollama";
    }
    if probe_lmstudio_v0(base_url, bearer) {
        return "lmstudio";
    }
    if probe_koboldcpp(base_url, bearer) {
        return "koboldcpp";
    }
    if probe_llamacpp(base_url, bearer) {
        return "llamacpp";
    }
    if probe_sglang(base_url, bearer) {
        return "sglang";
    }
    if let Some(v1_models) = fetch_v1_models(base_url, bearer) {
        if v1_models_owned_by_matches(&v1_models, "lmdeploy") {
            return "lmdeploy";
        }
        if v1_models_owned_by_matches(&v1_models, "vllm") {
            return "vllm";
        }
    }
    "generic"
}

/// `models probe-local [--url URL]`: identify the local inference server
/// kind. Defaults to the active model's base_url and reuses its stored
/// credential (mirroring the GUI's credential resolution). Refuses
/// non-loopback hosts with a usage error before any request is sent.
fn probe_local(url: Option<&str>, output: OutputMode) -> Result<CliOutcome, CliError> {
    let (target, bearer) = match url {
        Some(url) => {
            if !is_loopback_url(url)? {
                return Err(CliError::usage(
                    "probe-local refuses non-loopback urls; pass a 127.0.0.1, ::1 or localhost endpoint",
                ));
            }
            (url.to_owned(), None)
        }
        None => {
            let prefs = safe_prefs();
            let model = prefs
                .active_model()
                .cloned()
                .ok_or_else(|| CliError::failed("no active model to probe"))?;
            if !is_loopback_url(&model.base_url)? {
                return Err(CliError::usage(format!(
                    "the active model base_url {} is not a loopback endpoint; pass --url",
                    model.base_url
                )));
            }
            let bearer = resolve_saved_model_key(&model)
                .map_err(|error| CliError::failed(format!("credential_unavailable: {error}")))?
                .filter(|key| !key.trim().is_empty());
            (model.base_url, bearer)
        }
    };
    let kind = select_local_server_kind(&target, bearer.as_deref());
    let text = render(
        output,
        format!("url: {target}\nkind: {kind}"),
        &serde_json::json!({ "url": target, "kind": kind }),
    );
    Ok(success(text))
}

// ---------------------------------------------------------------------------
// settings family
// ---------------------------------------------------------------------------

fn theme_str(theme: Theme) -> &'static str {
    match theme {
        Theme::Genesis => "genesis",
        Theme::LiquidLight => "liquid-light",
        Theme::LiquidDark => "liquid-dark",
    }
}

fn color_scheme_str(scheme: ColorScheme) -> &'static str {
    match scheme {
        ColorScheme::Light => "light",
        ColorScheme::Dark => "dark",
        ColorScheme::System => "system",
    }
}

fn mode_str(mode: Option<SerializableMode>) -> String {
    mode.map_or_else(
        || "none".to_owned(),
        |mode| match mode {
            SerializableMode::Plan => "plan".to_owned(),
            SerializableMode::Yolo => "yolo".to_owned(),
        },
    )
}

fn optional_bool_str(value: Option<bool>) -> String {
    value.map_or_else(|| "none".to_owned(), |value| value.to_string())
}

/// `settings get [key]`: with a key prints `key = value` (human) or a single
/// JSON object; without a key prints all settings as JSON (the same payload
/// the GUI `get_settings` command returns, minus plaintext keys).
fn settings_get(key: Option<SettingsKey>, output: OutputMode) -> Result<CliOutcome, CliError> {
    let Some(key) = key else {
        let prefs = safe_prefs();
        let value = serde_json::to_value(&prefs)
            .map_err(|error| CliError::failed(format!("settings serialization failed: {error}")))?;
        return Ok(success(serde_json::to_string(&value).unwrap_or_default()));
    };
    let prefs = safe_prefs();
    let (human_value, json_value) = match key {
        SettingsKey::Theme => {
            let value = theme_str(prefs.theme);
            (value.to_owned(), serde_json::json!(value))
        }
        SettingsKey::ColorScheme => {
            let value = color_scheme_str(prefs.color_scheme);
            (value.to_owned(), serde_json::json!(value))
        }
        SettingsKey::Language => (
            prefs.language.locale_tag().to_owned(),
            serde_json::json!(prefs.language.locale_tag()),
        ),
        SettingsKey::MemoryEnabled => (
            prefs.memory_enabled.to_string(),
            serde_json::json!(prefs.memory_enabled),
        ),
        SettingsKey::NotificationsEnabled => (
            prefs.notifications.enabled.to_string(),
            serde_json::json!(prefs.notifications.enabled),
        ),
        SettingsKey::NotificationsTaskCompleted => (
            prefs.notifications.task_completed.to_string(),
            serde_json::json!(prefs.notifications.task_completed),
        ),
        SettingsKey::SidebarDateGrouping => (
            prefs.sidebar.date_grouping.to_string(),
            serde_json::json!(prefs.sidebar.date_grouping),
        ),
        SettingsKey::ModeDefaultsWork => {
            let value = mode_str(prefs.mode_defaults.work);
            (value.clone(), serde_json::json!(value))
        }
        SettingsKey::CodePermissionLastMode => {
            let value = mode_str(prefs.code_permission.last_mode);
            (value.clone(), serde_json::json!(value))
        }
        SettingsKey::AdvancedAllowShell => (
            optional_bool_str(prefs.advanced.allow_shell),
            serde_json::json!(prefs.advanced.allow_shell),
        ),
        SettingsKey::VoiceShortcutEnabled => (
            prefs.voice_shortcut_enabled.to_string(),
            serde_json::json!(prefs.voice_shortcut_enabled),
        ),
        SettingsKey::PetEnabled => (
            prefs.pet.enabled.to_string(),
            serde_json::json!(prefs.pet.enabled),
        ),
    };
    let key_name = SettingsKey::ALL
        .iter()
        .find(|(_, candidate)| *candidate == key)
        .map(|(name, _)| *name)
        .unwrap_or_default();
    let text = render(
        output,
        format!("{key_name} = {human_value}"),
        &serde_json::json!({ key_name: json_value }),
    );
    Ok(success(text))
}

/// `settings set <key> <value>`: mutates through `UserPrefs::update_transaction`
/// so the memory-locale policy, normalization and the atomic write all run
/// exactly as for GUI saves (e.g. `memory_enabled true` under a non-zh-Hans
/// language is normalized back to false by the prefs layer).
fn settings_set(
    key: SettingsKey,
    value: SettingsValue,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    UserPrefs::update_transaction(|prefs| {
        match (key, value.clone()) {
            (SettingsKey::Theme, SettingsValue::Theme(v)) => prefs.theme = v,
            (SettingsKey::ColorScheme, SettingsValue::ColorScheme(v)) => prefs.color_scheme = v,
            (SettingsKey::Language, SettingsValue::Language(v)) => prefs.language = v,
            (SettingsKey::MemoryEnabled, SettingsValue::Bool(v)) => prefs.memory_enabled = v,
            (SettingsKey::NotificationsEnabled, SettingsValue::Bool(v)) => {
                prefs.notifications.enabled = v;
            }
            (SettingsKey::NotificationsTaskCompleted, SettingsValue::Bool(v)) => {
                prefs.notifications.task_completed = v;
            }
            (SettingsKey::SidebarDateGrouping, SettingsValue::Bool(v)) => {
                prefs.sidebar.date_grouping = v;
            }
            (SettingsKey::ModeDefaultsWork, SettingsValue::Mode(v)) => {
                prefs.mode_defaults.work = v;
            }
            (SettingsKey::CodePermissionLastMode, SettingsValue::Mode(v)) => {
                prefs.code_permission.last_mode = v;
            }
            (SettingsKey::AdvancedAllowShell, SettingsValue::OptionalBool(v)) => {
                prefs.advanced.allow_shell = v;
            }
            (SettingsKey::VoiceShortcutEnabled, SettingsValue::Bool(v)) => {
                prefs.voice_shortcut_enabled = v;
            }
            (SettingsKey::PetEnabled, SettingsValue::Bool(v)) => prefs.pet.enabled = v,
            _ => return Err("settings key/value type mismatch".to_owned()),
        }
        Ok(())
    })
    .map_err(prefs_error)?;
    let key_name = SettingsKey::ALL
        .iter()
        .find(|(_, candidate)| *candidate == key)
        .map(|(name, _)| *name)
        .unwrap_or_default();
    let text = render(
        output,
        format!("{key_name} updated"),
        &serde_json::json!({ "updated": key_name }),
    );
    Ok(success(text))
}

fn search_prefs_json(prefs: &UserPrefs) -> serde_json::Value {
    let credentials: serde_json::Map<String, serde_json::Value> = prefs
        .search
        .credentials
        .iter()
        .map(|(provider, credential)| {
            (
                provider.as_str().to_owned(),
                serde_json::json!({
                    "credential_state": credential_state_str(credential.credential_state),
                    "has_secret": credential.has_secret,
                }),
            )
        })
        .collect();
    serde_json::json!({
        "provider": prefs.search.provider.as_str(),
        "enabled_providers": prefs
            .search
            .enabled_providers
            .iter()
            .map(|provider| provider.as_str())
            .collect::<Vec<_>>(),
        "credentials": credentials,
    })
}

fn search_list(output: OutputMode) -> Result<CliOutcome, CliError> {
    let prefs = safe_prefs();
    let json = search_prefs_json(&prefs);
    let credentials = prefs
        .search
        .credentials
        .iter()
        .map(|(provider, credential)| {
            format!(
                "{}={}",
                provider.as_str(),
                credential_state_str(credential.credential_state)
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    let human = format!(
        "provider: {}\nenabled: {}\ncredentials: {}",
        prefs.search.provider.as_str(),
        prefs
            .search
            .enabled_providers
            .iter()
            .map(|provider| provider.as_str())
            .collect::<Vec<_>>()
            .join(","),
        if credentials.is_empty() {
            "none"
        } else {
            &credentials
        },
    );
    Ok(success(render(output, human, &json)))
}

/// `settings search set --provider P [--api-key-env V | --clear]`: switches
/// the active search provider and stores/clears its credential with the same
/// bookkeeping (`mark_configured` / `mark_missing`) as the GUI's search
/// settings save path.
fn search_set(
    provider: SearchProvider,
    api_key_env: &Option<String>,
    clear: bool,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let secret = resolve_secret(api_key_env, false)?;
    if provider == SearchProvider::Bing && secret.is_some() {
        return Err(CliError::usage(
            "provider bing does not use an api key; it needs no configuration",
        ));
    }
    let stored = if clear { None } else { secret };
    UserPrefs::update_transaction(|prefs| {
        prefs.search.provider = provider;
        if let Some(key) = &stored {
            let reference = provider.credential_reference();
            SystemCredentialStore::new()
                .set(&reference, key)
                .map_err(|error| error.user_message())?;
            prefs
                .search
                .credentials
                .entry(provider)
                .or_default()
                .mark_configured(reference);
        }
        if clear {
            if let Some(credential) = prefs.search.credentials.get_mut(&provider) {
                credential.mark_missing();
            }
        }
        Ok(())
    })
    .map_err(prefs_error)?;
    if clear {
        // Only after the prefs save succeeded — deleting first would leave
        // the prefs entry pointing at a credential that no longer exists if
        // the save fails (a leftover keyring entry is the benign direction).
        SystemCredentialStore::new()
            .delete(&provider.credential_reference())
            .map_err(|error| CliError::failed(error.user_message()))?;
    }
    let action = if clear {
        "cleared"
    } else if stored.is_some() {
        "configured"
    } else {
        "unchanged"
    };
    let text = render(
        output,
        format!("provider: {}\ncredential: {action}", provider.as_str()),
        &serde_json::json!({ "provider": provider.as_str(), "credential": action }),
    );
    Ok(success(text))
}

/// `settings search test <provider>`: Bing runs a real HTTP probe (same
/// request as the GUI); API-key providers resolve the credential from the
/// environment names first, then the stored credential, and only report
/// configuration status (no network call) — mirroring `test_search_provider`.
fn search_test(provider: SearchProvider, output: OutputMode) -> Result<CliOutcome, CliError> {
    let probe = if provider == SearchProvider::Bing {
        run_bing_probe()
    } else {
        let key = resolve_search_key(provider);
        match key {
            Some(key) if !key.trim().is_empty() => SearchProbe {
                ok: true,
                code: "configured",
                detail: Some("credential configured".to_owned()),
            },
            _ => SearchProbe {
                ok: false,
                code: "no_api_key",
                detail: Some(format!(
                    "no api key configured for provider {}; set one with settings search set",
                    provider.as_str()
                )),
            },
        }
    };
    let json = serde_json::json!({
        "provider": provider.as_str(),
        "ok": probe.ok,
        "code": probe.code,
        "detail": probe.detail,
    });
    let mut human = format!(
        "provider: {}\nok: {}\ncode: {}",
        provider.as_str(),
        probe.ok,
        probe.code
    );
    if let Some(detail) = &probe.detail {
        human.push_str(&format!("\ndetail: {detail}"));
    }
    let outcome = CliOutcome {
        exit_code: if probe.ok {
            ExitCode::Success
        } else {
            ExitCode::Failed
        },
        stdout: render(output, human, &json),
    };
    Ok(outcome)
}

struct SearchProbe {
    ok: bool,
    code: &'static str,
    detail: Option<String>,
}

fn run_bing_probe() -> SearchProbe {
    let Ok(client) = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
    else {
        return SearchProbe {
            ok: false,
            code: "client_error",
            detail: None,
        };
    };
    match client
        .get("https://www.bing.com/search")
        .query(&[("q", "pinvou")])
        .send()
    {
        Ok(response) => {
            let status = response.status();
            if status.is_success() {
                SearchProbe {
                    ok: true,
                    code: "ok",
                    detail: Some(format!("HTTP {}", status.as_u16())),
                }
            } else {
                SearchProbe {
                    ok: false,
                    code: "http_error",
                    detail: Some(format!("HTTP {}", status.as_u16())),
                }
            }
        }
        Err(error) => {
            let probe = connection_error_result(&error);
            SearchProbe {
                ok: false,
                code: probe.code,
                detail: probe.detail,
            }
        }
    }
}

/// Credential resolution order of the GUI's `resolve_saved_search_key`:
/// provider-specific environment variables first, then the stored credential.
fn resolve_search_key(provider: SearchProvider) -> Option<String> {
    for name in provider.env_key_names() {
        if let Ok(value) = std::env::var(name) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
    }
    let mut prefs = UserPrefs::load();
    prefs.refresh_credential_states_with_store(&SystemCredentialStore::new());
    let credential = prefs.search.credentials.get(&provider)?;
    let reference = credential.credential_ref.clone()?;
    SystemCredentialStore::new()
        .get(&reference)
        .ok()
        .flatten()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_guard_parses_ips_instead_of_prefix_matching() {
        for url in [
            "http://localhost:11434",
            "http://127.0.0.1:8080/v1",
            "http://[::1]:11434",
            "http://127.0.0.2:8000",
        ] {
            assert!(is_loopback_url(url).unwrap_or(false), "{url} is loopback");
        }
        // A hostname starting with "127." resolves remotely and must be
        // refused — the guard exists so a typo can never probe off-host.
        for url in [
            "http://127.evil.com:8000/v1",
            "http://example.com",
            "http://0.0.0.0:8000",
            "not a url",
        ] {
            assert!(
                !is_loopback_url(url).unwrap_or(false),
                "{url} is not loopback"
            );
        }
    }
}

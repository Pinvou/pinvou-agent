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
//! Two of the three network paths mirror a real GUI counterpart, and the
//! third has none — the distinction matters because it decides what the
//! command is allowed to claim:
//!
//! - `models test` mirrors `probe_model_connection`
//!   (app/commands/settings.rs), a real `GET {base}/models`.
//! - `models probe-local` mirrors `probe_local_server_kind`
//!   (app/commands/settings.rs) over `core::model_endpoint`, a real set of
//!   signature probes.
//! - `settings search test` has NO GUI counterpart: the desktop app ships no
//!   search-provider test at all. It is a CLI-only command, so its contract
//!   is anchored on two in-repo facts instead of on a mirrored function —
//!   the provider endpoints and auth schemes in
//!   `CodeWhale/crates/tui/src/tools/web_search.rs` (what a real search
//!   request actually sends), and the credential resolution order of
//!   `features/assistant/platform/bridge.rs::search_api_key` (what a real
//!   search request resolves its key from).
//!
//! Those app functions live in the app-private `app` / `core` modules and are
//! not exported through `pinvou3_lib`, so the exact semantics are
//! reimplemented here on a blocking reqwest client (no async runtime). They
//! are the only paths that touch the network and are covered by `#[ignore]`
//! tests only.

use std::collections::BTreeMap;
use std::time::Duration;

use pinvou3_lib::features::sessions::SerializableMode;
use pinvou3_lib::platform::credential_store::{
    CredentialReference, CredentialState, CredentialStore, SystemCredentialStore, redact_secret,
};
use pinvou3_lib::platform::prefs::{
    ColorScheme, CredentialStateOps, Language, MODEL_PROVIDER_KIND_CODING_PLAN,
    MODEL_PROVIDER_KIND_CUSTOM, MODEL_PROVIDER_KIND_OFFICIAL_API, ModelPreset, SavedModel,
    SearchProvider, Theme, UserPrefs,
};

use crate::support::{collapse_control_characters, render, require_yes, resolve_secret, success};
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
        metadata: ModelMetadata,
        set_active: bool,
    },
    Edit {
        id: String,
        changes: Box<ModelEdit>,
        api_key_env: Option<String>,
        api_key_stdin: bool,
        clear_api_key: bool,
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
        api_key_env: Option<String>,
        model_id: Option<String>,
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

/// The optional model metadata the GUI's model form sends on every save
/// (`pinvou3-app/src/features/settings/SettingsView.jsx` `onSave`: `alias`,
/// `provider_kind`, `vendor`, `endpoint_mode`, `vision_model_id`) but that
/// `models add` used to hardcode to `None`. `provider_kind` is repaired on
/// save by `normalize_provider_metadata`, so leaving it out was survivable;
/// `vendor` is NOT — it stays `None` and is read for reasoning-protocol
/// routing (`features/assistant/platform/bridge.rs`), so a CLI-created model
/// routed differently from the same model created in the GUI.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelMetadata {
    pub alias: Option<String>,
    pub provider_kind: Option<String>,
    pub vendor: Option<String>,
    pub endpoint_mode: Option<String>,
    pub vision_model_id: Option<String>,
}

/// A `models edit` patch. The double `Option` is the whole point of the type:
/// the outer layer is "was this flag given at all" (`None` = leave the stored
/// value untouched) and the inner layer is the new value, where `None` means
/// the caller asked to clear the field with the literal `none`. Collapsing
/// the two would make `models edit` unable to express "clear the alias"
/// without also being unable to express "leave the alias alone".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelEdit {
    pub preset: Option<ModelPreset>,
    pub name: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub context_window: Option<Option<u32>>,
    pub max_output: Option<Option<u32>>,
    pub reasoning_effort: Option<Option<String>>,
    pub alias: Option<Option<String>>,
    pub provider_kind: Option<Option<String>>,
    pub vendor: Option<Option<String>>,
    pub endpoint_mode: Option<Option<String>>,
    pub vision_model_id: Option<Option<String>>,
}

impl ModelEdit {
    /// True when no field flag was given. An edit that changes nothing is a
    /// mistyped command, not a successful no-op write: reporting success for
    /// it would tell a script the rotation it asked for had happened.
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
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
        // `auto` is a value the GUI can legitimately hold (the prefs
        // normalizer maps automatic/auto to "auto"); rejecting it would make
        // GUI-created configurations unrepresentable from the CLI.
        "off" | "low" | "medium" | "high" | "max" | "auto" | "automatic" => Ok(raw.to_owned()),
        other => Err(CliError::usage(format!(
            "invalid reasoning effort {other:?}; valid values: off, low, medium, high, max, \
             auto, automatic"
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
    label: String,
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
                if value.starts_with("--") {
                    // `--api-key-env --set-active` would otherwise consume
                    // the flag name as its value and fail later with a
                    // confusing host error.
                    return Err(CliError::usage(format!(
                        "--{name} requires a value (got the flag {value})"
                    )));
                }
                // An empty value is a missing value, not a value that happens
                // to be empty: `--name "$UNSET"` is how the shell spells
                // "this argument was never computed". `support::parse_family_flags`
                // has always refused it; `models` did not, so `--name ""`
                // reached the store and `--api-key-env ""` reached
                // `std::env::var("")` — each failing later with a message
                // about the store or the environment rather than about the
                // command line. Refusing here keeps all three parsers
                // (support.rs, models.rs, memory.rs) on one contract.
                if value.is_empty() {
                    return Err(CliError::usage(format!("--{name} requires a value")));
                }
                if values.insert(name.to_owned(), value.clone()).is_some() {
                    return Err(CliError::usage(format!(
                        "--{name} was given more than once"
                    )));
                }
                index += 2;
            } else if flag_flags.contains(&name) {
                // Duplicate boolean flags are usage errors, like duplicate
                // value flags above and every `support::parse_family_flags`
                // family (`models remove X --yes --yes` must exit 2).
                if flags.iter().any(|candidate| candidate == name) {
                    return Err(CliError::usage(format!(
                        "--{name} was given more than once"
                    )));
                }
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
        label: format!("pinvou {family} {subcommand}"),
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

    fn exactly_one_positional(&self) -> Result<String, CliError> {
        match self.positionals.len() {
            // `label` already names the subcommand ("pinvou models remove"),
            // so the message needs no extra noun.
            1 => Ok(self.positionals[0].clone()),
            0 => Err(CliError::usage(format!("{} requires an id", self.label))),
            n => Err(CliError::usage(format!(
                "{} takes one id, got {n}",
                self.label
            ))),
        }
    }

    /// Option-only subcommands must not silently drop stray tokens
    /// (`settings search set junk --provider metaso` is a typo, not a
    /// value).
    fn reject_stray_positionals(&self) -> Result<(), CliError> {
        if self.positionals.is_empty() {
            Ok(())
        } else {
            Err(CliError::usage(format!(
                "{} takes no positional arguments (got {})",
                self.label,
                self.positionals.join(" ")
            )))
        }
    }
}

const MODELS_USAGE: &str = "usage: pinvou models <list|add|edit|remove|use|show|test|probe-local>";
/// `settings get` without a key is documented as always-JSON right in the
/// usage text. `--output` is a global flag, so it is accepted on every
/// subcommand and a reader could reasonably expect `--output human` to change
/// the whole-settings dump; it does not, because that payload is the nested
/// `UserPrefs` document and there is no meaningful `key = value` rendering of
/// it. Saying so here turns a flag that looks ignored into a stated contract.
const SETTINGS_USAGE: &str = "usage: pinvou settings <get|set|search>; search subcommands: \
list|set|test; note: `settings get` without a key always prints JSON regardless of --output";

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
        ("models", "edit") => parse_edit(rest),
        ("models", "remove") => {
            let options = parse_options("models", "remove", rest, &[], &["yes"])?;
            Ok(ModelsCommand::Remove {
                id: options.exactly_one_positional()?,
                yes: options.has("yes"),
            })
        }
        ("models", "use") => {
            let options = parse_options("models", "use", rest, &[], &[])?;
            Ok(ModelsCommand::Use {
                id: options.exactly_one_positional()?,
            })
        }
        ("models", "show") => {
            let options = parse_options("models", "show", rest, &[], &["reveal-key"])?;
            Ok(ModelsCommand::Show {
                id: options.exactly_one_positional()?,
                reveal_key: options.has("reveal-key"),
            })
        }
        ("models", "test") => {
            let options = parse_options("models", "test", rest, &[], &[])?;
            Ok(ModelsCommand::Test {
                id: options.exactly_one_positional()?,
            })
        }
        ("models", "probe-local") => {
            let options = parse_options(
                "models",
                "probe-local",
                rest,
                &["url", "api-key-env", "model"],
                &[],
            )?;
            ensure_no_positionals(&options, "models probe-local")?;
            let url = options.value("url").map(str::to_owned);
            let model_id = options.value("model").map(str::to_owned);
            if model_id.is_some() && options.value("api-key-env").is_some() {
                return Err(CliError::usage("use only one of --model or --api-key-env"));
            }
            // Without `--url` the target IS the active model's own endpoint
            // and its own stored credential is already used; naming a
            // different model there would send that model's key to another
            // model's endpoint — the leak the GUI's own `saved_model_for_probe`
            // comment refuses to allow.
            if model_id.is_some() && url.is_none() {
                return Err(CliError::usage(
                    "--model only applies with --url; without --url the active model's own \
                     credential is used",
                ));
            }
            Ok(ModelsCommand::ProbeLocal {
                url,
                api_key_env: options.value("api-key-env").map(str::to_owned),
                model_id,
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

/// The metadata value flags shared by `models add` and `models edit`, so the
/// two commands can never accept different spellings of the same field.
const MODEL_METADATA_FLAGS: &[&str] = &[
    "alias",
    "provider-kind",
    "vendor",
    "endpoint-mode",
    "vision-model-id",
];

/// `provider_kind` is not free-form: the prefs layer only recognizes these
/// three (`MODEL_PROVIDER_KIND_*`), and `normalize_provider_metadata` rewrites
/// anything else. Rejecting an unrecognized spelling at parse time reports the
/// typo instead of silently storing a value the next save discards.
fn parse_provider_kind(raw: &str) -> Result<String, CliError> {
    let trimmed = raw.trim();
    if trimmed == MODEL_PROVIDER_KIND_OFFICIAL_API
        || trimmed == MODEL_PROVIDER_KIND_CUSTOM
        || trimmed == MODEL_PROVIDER_KIND_CODING_PLAN
    {
        return Ok(trimmed.to_owned());
    }
    Err(CliError::usage(format!(
        "invalid provider kind {raw:?}; valid values: {MODEL_PROVIDER_KIND_OFFICIAL_API}, \
         {MODEL_PROVIDER_KIND_CUSTOM}, {MODEL_PROVIDER_KIND_CODING_PLAN}"
    )))
}

/// A free-form metadata value that must not be stored as whitespace: the
/// prefs normalizer maps a blank `vendor` / `endpoint_mode` back to `None`,
/// so accepting one here would report a write that the next save undoes.
fn parse_metadata_value(flag: &str, raw: &str) -> Result<String, CliError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CliError::usage(format!("--{flag} must not be blank")));
    }
    Ok(trimmed.to_owned())
}

/// Reads the five optional GUI-form metadata fields off a parsed option set.
fn parse_metadata(options: &Options) -> Result<ModelMetadata, CliError> {
    Ok(ModelMetadata {
        alias: match options.value("alias") {
            Some(raw) => Some(parse_metadata_value("alias", raw)?),
            None => None,
        },
        provider_kind: match options.value("provider-kind") {
            Some(raw) => Some(parse_provider_kind(raw)?),
            None => None,
        },
        vendor: match options.value("vendor") {
            Some(raw) => Some(parse_metadata_value("vendor", raw)?),
            None => None,
        },
        endpoint_mode: match options.value("endpoint-mode") {
            Some(raw) => Some(parse_metadata_value("endpoint-mode", raw)?),
            None => None,
        },
        vision_model_id: match options.value("vision-model-id") {
            Some(raw) => Some(parse_metadata_value("vision-model-id", raw)?),
            None => None,
        },
    })
}

fn parse_add(rest: &[String]) -> Result<ModelsCommand, CliError> {
    let mut value_flags = vec![
        "preset",
        "name",
        "model",
        "base-url",
        "api-key-env",
        "context-window",
        "max-output",
        "reasoning-effort",
    ];
    value_flags.extend_from_slice(MODEL_METADATA_FLAGS);
    let options = parse_options(
        "models",
        "add",
        rest,
        &value_flags,
        &["api-key-stdin", "set-active"],
    )?;
    ensure_no_positionals(&options, "models add")?;
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
        metadata: parse_metadata(&options)?,
        set_active: options.has("set-active"),
    })
}

/// The literal that clears an optional field on `models edit`. Reusing the
/// spelling `settings set` already uses for its clearable keys
/// (`mode_defaults.work none`, `advanced.allow_shell none`) keeps one word
/// meaning one thing across the two families. The consequence is stated in
/// the usage text: a field whose intended value is literally `none` cannot be
/// set from `models edit` — none of these fields (a vendor id, an endpoint
/// mode, an `m_`-prefixed model id) has such a value, and an alias that needs
/// it can be written from the GUI form.
const EDIT_CLEAR_LITERAL: &str = "none";

/// Reads one clearable value flag into the [`ModelEdit`] double-`Option`:
/// absent -> `None`, the clear literal -> `Some(None)`, otherwise
/// `Some(Some(parsed))`.
fn parse_clearable<T>(
    options: &Options,
    flag: &str,
    parse: impl Fn(&str) -> Result<T, CliError>,
) -> Result<Option<Option<T>>, CliError> {
    match options.value(flag) {
        None => Ok(None),
        Some(raw) if raw.trim() == EDIT_CLEAR_LITERAL => Ok(Some(None)),
        Some(raw) => parse(raw).map(|value| Some(Some(value))),
    }
}

fn parse_edit(rest: &[String]) -> Result<ModelsCommand, CliError> {
    let mut value_flags = vec![
        "preset",
        "name",
        "model",
        "base-url",
        "api-key-env",
        "context-window",
        "max-output",
        "reasoning-effort",
    ];
    value_flags.extend_from_slice(MODEL_METADATA_FLAGS);
    let options = parse_options(
        "models",
        "edit",
        rest,
        &value_flags,
        &["api-key-stdin", "clear-api-key", "set-active"],
    )?;
    let id = options.exactly_one_positional()?;
    let api_key_env = options.value("api-key-env").map(str::to_owned);
    let api_key_stdin = options.has("api-key-stdin");
    let clear_api_key = options.has("clear-api-key");
    // Three mutually exclusive credential intents; accepting two would leave
    // the stored secret's fate decided by evaluation order.
    if [api_key_env.is_some(), api_key_stdin, clear_api_key]
        .iter()
        .filter(|given| **given)
        .count()
        > 1
    {
        return Err(CliError::usage(
            "use only one of --api-key-env, --api-key-stdin or --clear-api-key",
        ));
    }
    let changes = ModelEdit {
        preset: match options.value("preset") {
            Some(raw) => Some(parse_preset(raw)?),
            None => None,
        },
        // The three identity fields are not clearable: a model with no name,
        // wire model or base_url is not a model. `--name none` therefore
        // stores the literal, like `models add` would.
        name: match options.value("name") {
            Some(raw) => Some(parse_required_text("name", raw)?),
            None => None,
        },
        model: match options.value("model") {
            Some(raw) => Some(parse_required_text("model", raw)?),
            None => None,
        },
        base_url: match options.value("base-url") {
            Some(raw) => Some(parse_required_text("base-url", raw)?),
            None => None,
        },
        context_window: parse_clearable(&options, "context-window", |raw: &str| {
            parse_positive_u32("--context-window", raw)
        })?,
        max_output: parse_clearable(&options, "max-output", |raw: &str| {
            parse_positive_u32("--max-output", raw)
        })?,
        reasoning_effort: parse_clearable(&options, "reasoning-effort", parse_reasoning_effort)?,
        alias: parse_clearable(&options, "alias", |raw: &str| {
            parse_metadata_value("alias", raw)
        })?,
        provider_kind: parse_clearable(&options, "provider-kind", parse_provider_kind)?,
        vendor: parse_clearable(&options, "vendor", |raw: &str| {
            parse_metadata_value("vendor", raw)
        })?,
        endpoint_mode: parse_clearable(&options, "endpoint-mode", |raw: &str| {
            parse_metadata_value("endpoint-mode", raw)
        })?,
        vision_model_id: parse_clearable(&options, "vision-model-id", |raw: &str| {
            parse_metadata_value("vision-model-id", raw)
        })?,
    };
    if changes.is_empty()
        && api_key_env.is_none()
        && !api_key_stdin
        && !clear_api_key
        && !options.has("set-active")
    {
        return Err(CliError::usage(
            "pinvou models edit <id> requires at least one of --preset, --name, --model, \
             --base-url, --context-window, --max-output, --reasoning-effort, --alias, \
             --provider-kind, --vendor, --endpoint-mode, --vision-model-id, --api-key-env, \
             --api-key-stdin, --clear-api-key or --set-active",
        ));
    }
    // A model cannot be its own vision fallback: the routing lookup would
    // resolve straight back to the model that could not see the image. This is
    // decidable from argv alone (both values are right here), so it is a usage
    // error rather than the Failed-class check `require_known_vision_model`
    // applies to an id that merely does not exist.
    if changes.vision_model_id.as_ref().and_then(Option::as_ref) == Some(&id) {
        return Err(CliError::usage(
            "pinvou models edit: --vision-model-id must name a different model \
             than the one being edited",
        ));
    }
    Ok(ModelsCommand::Edit {
        id,
        changes: Box::new(changes),
        api_key_env,
        api_key_stdin,
        clear_api_key,
        set_active: options.has("set-active"),
    })
}

/// Trims an identity field and refuses a whitespace-only value, matching what
/// `models add` rejects in `add()` ("--name, --model and --base-url must not
/// be empty") — reported at parse time so it exits 2 with the flag named.
fn parse_required_text(flag: &str, raw: &str) -> Result<String, CliError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CliError::usage(format!("--{flag} must not be empty")));
    }
    Ok(trimmed.to_owned())
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
            options.reject_stray_positionals()?;
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
            let provider = options.exactly_one_positional()?;
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
    // Every subcommand reads or writes the settings store / credential store
    // under the product data root; a relative PINVOU3_HOME would silently
    // resolve against the cwd.
    crate::support::sandbox_home()?;
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
            metadata,
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
            metadata,
            set_active,
            output,
        ),
        ModelsCommand::Edit {
            id,
            changes,
            api_key_env,
            api_key_stdin,
            clear_api_key,
            set_active,
        } => edit(
            &id,
            &changes,
            &api_key_env,
            api_key_stdin,
            clear_api_key,
            set_active,
            output,
        ),
        ModelsCommand::Remove { id, yes } => remove(&id, yes, output),
        ModelsCommand::Use { id } => use_model(&id, output),
        ModelsCommand::Show { id, reveal_key } => show(&id, reveal_key, output),
        ModelsCommand::Test { id } => test_connection(&id, output),
        ModelsCommand::ProbeLocal {
            url,
            api_key_env,
            model_id,
        } => probe_local(
            url.as_deref(),
            api_key_env.as_deref(),
            model_id.as_deref(),
            output,
        ),
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

/// One `models list` JSON row, mirroring the GUI's `ModelListItem` DTO
/// (`credential_state` included, same semantics as `models show`) minus the
/// GUI-only presentation fields.
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
        "credential_state": model.credential_state,
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
            // Every untrusted cell goes through `collapse_control_characters`:
            // `models add --name $'ok\n*m_fake\tEvil'` would otherwise inject
            // a line indistinguishable from a real active-model row (the
            // `.trim()` in `parse_add` only strips the edges). Same hygiene
            // the `sessions`, `projects` and `code` rows apply; JSON output
            // still carries the original untouched.
            format!(
                "{marker}{}\t{}\t{}\t{}\t{}\tcontext_window={}\tmax_output={}\treasoning_effort={}\thas_secret={}\tcredential_state={}",
                collapse_control_characters(&model.id),
                collapse_control_characters(&model.name),
                model.preset.as_str(),
                collapse_control_characters(&model.model),
                collapse_control_characters(&model.base_url),
                optional_u32(model.context_window_tokens),
                optional_u32(model.max_output_tokens),
                collapse_control_characters(
                    model.reasoning_effort.as_deref().unwrap_or("default")
                ),
                model.has_secret,
                credential_state_str(model.credential_state),
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

/// The exact bytes a secret is written to the credential store under — the
/// single normalization every credential write in this module goes through.
///
/// `support::resolve_secret` returns an `--api-key-env` value VERBATIM; only
/// its stdin lane trims. Environment secrets routinely carry a trailing
/// newline (a file mounted by a CI secret store, `read KEY < key.txt`, most
/// `.env` loaders — `$(cat key.txt)` is the exception, not the rule), and
/// storing that newline makes every signed request 401.
///
/// The failure is self-masking, which is why it must be fixed on the WRITE
/// side: every read path trims (`settings search test`, `apply_bearer`), so
/// the CLI keeps reporting the provider as `configured` while the stored
/// value is unusable. Routing both lanes through one function is the point —
/// they had already drifted, `models add` trimming and `settings search set`
/// not, which is exactly the shape of bug a shared helper prevents.
fn secret_for_storage(raw: &str) -> &str {
    raw.trim()
}

/// Credential bookkeeping identical to the GUI's `apply_model_credential`
/// with `old = None` (a fresh model): an empty key means "no secret"
/// (`mark_missing`), a non-empty key is stored under the model's credential
/// reference in the platform credential store and marked configured. The
/// plaintext key never reaches settings.json (`clear_plaintext_key`).
fn apply_new_model_credential(mut model: SavedModel) -> Result<SavedModel, String> {
    let key = secret_for_storage(&model.api_key).to_owned();
    if key.is_empty() {
        model.mark_missing();
    } else {
        let reference = model.credential_reference();
        SystemCredentialStore::new()
            .set(&reference, &key)
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
    metadata: ModelMetadata,
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
        alias: metadata.alias,
        preset,
        context_window_tokens: context_window,
        max_output_tokens: max_output,
        reasoning_effort,
        model: model.to_owned(),
        base_url: base_url.to_owned(),
        provider_kind: metadata.provider_kind,
        vendor: metadata.vendor,
        endpoint_mode: metadata.endpoint_mode,
        image_capability_override: Default::default(),
        vision_model_id: metadata.vision_model_id,
        api_key: secret.unwrap_or_default(),
        credential_ref: None,
        credential_state: CredentialState::Missing,
        has_secret: false,
        credential_action: None,
    };
    let active_id = id.clone();
    let transaction = UserPrefs::update_transaction(|prefs| {
        require_known_vision_model(prefs, saved.vision_model_id.as_deref())?;
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

/// `vision_model_id` points at ANOTHER `SavedModel`'s id and its endpoint and
/// credential are reused from there (`prefs::SavedModel::vision_model_id`).
/// A dangling id is not a stored preference, it is a vision fallback that
/// silently never fires, so the reference is resolved against the same
/// in-transaction prefs snapshot the write lands in.
fn require_known_vision_model(
    prefs: &UserPrefs,
    vision_model_id: Option<&str>,
) -> Result<(), String> {
    match vision_model_id {
        Some(target) if prefs.model_by_id(target).is_none() => {
            Err(format!("vision model not found: {target}"))
        }
        _ => Ok(()),
    }
}

/// Applies a [`ModelEdit`] patch onto the stored model. Only the fields whose
/// flag was given are touched; the id is never among them, which is the whole
/// reason `models edit` exists (a remove+add mints a NEW id and orphans every
/// per-session model binding and scheduled-task model pin that referenced the
/// old one).
fn apply_model_edit(model: &mut SavedModel, changes: &ModelEdit) {
    if let Some(preset) = changes.preset {
        model.preset = preset;
    }
    if let Some(name) = &changes.name {
        model.name = name.clone();
    }
    if let Some(wire_model) = &changes.model {
        model.model = wire_model.clone();
    }
    if let Some(base_url) = &changes.base_url {
        model.base_url = base_url.clone();
    }
    if let Some(value) = changes.context_window {
        model.context_window_tokens = value;
    }
    if let Some(value) = changes.max_output {
        model.max_output_tokens = value;
    }
    if let Some(value) = &changes.reasoning_effort {
        model.reasoning_effort = value.clone();
    }
    if let Some(value) = &changes.alias {
        model.alias = value.clone();
    }
    if let Some(value) = &changes.provider_kind {
        model.provider_kind = value.clone();
    }
    if let Some(value) = &changes.vendor {
        model.vendor = value.clone();
    }
    if let Some(value) = &changes.endpoint_mode {
        model.endpoint_mode = value.clone();
    }
    if let Some(value) = &changes.vision_model_id {
        model.vision_model_id = value.clone();
    }
}

/// `models edit <id>`: mutate an existing model IN PLACE. Rotating a key or
/// fixing a `base_url` used to require `remove` + `add`, which mints a new id
/// (`new_model_id`) and therefore orphans per-session model bindings and
/// scheduled-task model pins — the edit that silently breaks other features.
///
/// The credential lanes mirror the GUI's `apply_model_credential`:
/// - no credential flag  -> `KeepExisting`: `credential_ref`,
///   `credential_state` and `has_secret` are carried over untouched;
/// - `--api-key-env` / `--api-key-stdin` -> `Replace`: `.set()` runs INSIDE
///   the transaction closure, so a save failure never leaves a model marked
///   configured against a secret that was never written;
/// - `--clear-api-key` -> `Delete`: the record is marked missing inside the
///   closure and the keyring `.delete()` is DEFERRED to after the commit
///   (`save_model_inner`/`delete_model_inner`'s ordering contract — deleting
///   first would leave a configured-but-secretless model if the save failed;
///   an orphaned entry is the benign direction).
#[allow(clippy::too_many_arguments)]
fn edit(
    id: &str,
    changes: &ModelEdit,
    api_key_env: &Option<String>,
    api_key_stdin: bool,
    clear_api_key: bool,
    set_active: bool,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    // A model cannot be its own vision fallback (the GUI form drops
    // `visionModelId === id` before saving); saying so beats storing a
    // self-reference the app then ignores.
    if matches!(&changes.vision_model_id, Some(Some(target)) if target == id) {
        return Err(CliError::usage(
            "--vision-model-id must name a different model",
        ));
    }
    // Resolved before any prefs mutation so a missing environment variable is
    // reported without side effects (same ordering as `add`).
    let secret = resolve_secret(api_key_env, api_key_stdin)?;
    let replacement = secret.map(|raw| secret_for_storage(&raw).to_owned());
    let mut reference_to_delete: Option<CredentialReference> = None;
    // What the closure actually wrote, and what was under that reference
    // before it did. Both are captured INSIDE the transaction rather than
    // read up front: `credential_reference()` prefers the model's stored
    // `credential_ref`, so only the in-transaction snapshot is guaranteed to
    // name the reference the rotation overwrote. A rotation writes over the
    // old secret under that same reference, so deleting on rollback would
    // destroy a secret prefs still points at — a READ ERROR therefore stays
    // distinct from "no previous secret", the same three-way rollback
    // `search_set` documents.
    let mut written: Option<(CredentialReference, Result<Option<String>, String>)> = None;
    let transaction = UserPrefs::update_transaction(|prefs| {
        let Some(existing) = prefs.model_by_id(id) else {
            return Err(format!("model not found: {id}"));
        };
        let mut updated = existing.clone();
        apply_model_edit(&mut updated, changes);
        require_known_vision_model(prefs, updated.vision_model_id.as_deref())?;
        match (&replacement, clear_api_key) {
            // Replace: store first, then mark configured, all inside the
            // closure so the save that follows either commits both or neither.
            (Some(key), _) if !key.is_empty() => {
                let reference = updated.credential_reference();
                let store = SystemCredentialStore::new();
                let previous = store.get(&reference).map_err(|error| error.user_message());
                written = Some((reference.clone(), previous));
                store
                    .set(&reference, key)
                    .map_err(|error| format!("credential store unavailable: {error}"))?;
                updated.mark_configured(reference);
            }
            // Delete: the record is cleared now, the keyring entry after the
            // commit.
            (_, true) => {
                reference_to_delete = updated
                    .credential_ref
                    .clone()
                    .or_else(|| Some(updated.credential_reference()));
                updated.mark_missing();
            }
            // KeepExisting: the clone already carries the stored credential
            // bookkeeping, so there is nothing to do.
            _ => {}
        }
        // The plaintext key never reaches settings.json.
        updated.api_key = String::new();
        prefs.upsert_model(updated);
        if set_active {
            prefs.advanced.active_model_id = Some(id.to_owned());
        }
        Ok(())
    });
    if let Err(error) = transaction {
        if let Some((reference, previous)) = written {
            let store = SystemCredentialStore::new();
            match previous {
                // The rotation destroyed the old secret: put it back.
                Ok(Some(old)) => {
                    let _ = store.set(&reference, old.as_str());
                }
                // Nothing was there before: remove what was just stored.
                Ok(None) => {
                    let _ = store.delete(&reference);
                }
                // Pre-transaction state unknown (keychain read failed): leave
                // it alone rather than delete a secret prefs may still
                // reference.
                Err(_) => {}
            }
        }
        return Err(prefs_error(error));
    }
    if let Some(reference) = reference_to_delete
        && let Err(error) = SystemCredentialStore::new().delete(&reference)
    {
        note!(
            "pinvou: warning: model {id} api key cleared from settings, but its keyring \
             entry could not be deleted: {}",
            error.user_message()
        );
    }
    let credential = if replacement.as_ref().is_some_and(|key| !key.is_empty()) {
        "replaced"
    } else if clear_api_key {
        "cleared"
    } else {
        "unchanged"
    };
    let text = render(
        output,
        format!("id: {id}\nupdated: true\ncredential: {credential}"),
        &serde_json::json!({ "id": id, "updated": true, "credential": credential }),
    );
    Ok(success(text))
}

/// Classifies transaction failures: the min-1 rule is an argument-level
/// problem (exit 2); everything else — I/O, credential store, and unknown
/// ids — is a host failure (exit 1). `remove` rejects the min-1 rule
/// structurally before the transaction, so the prefix match here is only a
/// backstop (e.g. a concurrent removal shrinking the list mid-run). Unknown
/// ids exit 1 like every other family (`sessions`, `knowledge`,
/// `scheduled`): a lookup miss against the live store is a runtime failure,
/// not argv misuse, and scripts branch on the same code across families.
/// Messages are already redacted by the credential layer.
fn prefs_error(error: String) -> CliError {
    if error.starts_with("cannot remove the last") {
        CliError::usage(error)
    } else {
        CliError::failed(error)
    }
}

/// The GUI's min-1-model rule message, shared by the up-front usage check in
/// `remove` and the in-transaction backstop so the wording cannot drift.
const REMOVE_LAST_MODEL_MESSAGE: &str =
    "cannot remove the last remaining model; add another model first";

fn remove(id: &str, yes: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    require_yes(yes)?;
    // Structural min-1 classification: load the prefs through the same
    // `UserPrefs::load` path the transaction uses and reject up front, so
    // the usage exit code does not depend on string-matching the
    // transaction error below (which stays as a backstop). A plain load —
    // not `safe_prefs` — keeps the check free of credential-store refreshes.
    let prefs = UserPrefs::load();
    if prefs.model_by_id(id).is_some() && prefs.advanced.saved_models.len() <= 1 {
        return Err(CliError::usage(REMOVE_LAST_MODEL_MESSAGE));
    }
    // The keyring delete moves AFTER the prefs save (the ordering this
    // file's own `search set --clear` comment states): deleting first left
    // a save failure with a model that is still configured but secretless.
    // A secret left behind by a failed post-save delete is the benign
    // direction — the prefs record no longer references it.
    let mut reference_to_delete: Option<CredentialReference> = None;
    UserPrefs::update_transaction(|prefs| {
        if prefs.model_by_id(id).is_none() {
            return Err(format!("model not found: {id}"));
        }
        if prefs.advanced.saved_models.len() <= 1 {
            return Err(REMOVE_LAST_MODEL_MESSAGE.to_owned());
        }
        if let Some(reference) = prefs
            .model_by_id(id)
            .and_then(|model| model.credential_ref.clone())
        {
            reference_to_delete = Some(reference);
        }
        prefs.remove_model(id);
        Ok(())
    })
    .map_err(prefs_error)?;
    if let Some(reference) = reference_to_delete {
        if let Err(error) = SystemCredentialStore::new().delete(&reference) {
            note!(
                "pinvou: warning: model {id} removed, but its keyring secret could not be \
                 deleted: {}",
                error.user_message()
            );
        }
    }
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
        // Unknown ids exit 1 like every other family — see `prefs_error`.
        .ok_or_else(|| CliError::failed(format!("model not found: {id}")))
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
    // Where the key the model actually signs with comes from. `(not stored)`
    // used to be printed for an `EnvOverride` model too, which is simply
    // false: that model HAS a key, supplied by the environment — the CLI just
    // refuses to echo a value it does not own (mirroring
    // `reveal_model_api_key`). Naming the source keeps the three outcomes
    // distinguishable in both renderings instead of collapsing them to one
    // `null` / one `(not stored)`.
    let key_source = if model.credential_state == CredentialState::EnvOverride {
        "environment"
    } else if revealed.as_ref().is_some_and(|inner| inner.is_some()) {
        "credential_store"
    } else {
        "none"
    };
    let mut json = model_entry_json(&model, active_id);
    if reveal_key {
        json["api_key"] = serde_json::json!(revealed.clone().flatten());
        json["api_key_source"] = serde_json::json!(key_source);
    }
    // Every untrusted cell is collapsed for the same reason as the `models
    // list` rows: a name carrying `\n` would forge extra `key: value` lines
    // in this block.
    let mut human = format!(
        "id: {}\nname: {}\npreset: {}\nmodel: {}\nbase_url: {}\ncontext_window: {}\nmax_output: {}\nreasoning_effort: {}\nactive: {}\ncredential_state: {}\nhas_secret: {}",
        collapse_control_characters(&model.id),
        collapse_control_characters(&model.name),
        model.preset.as_str(),
        collapse_control_characters(&model.model),
        collapse_control_characters(&model.base_url),
        optional_u32(model.context_window_tokens),
        optional_u32(model.max_output_tokens),
        collapse_control_characters(model.reasoning_effort.as_deref().unwrap_or("default")),
        active_id == Some(model.id.as_str()),
        credential_state_str(model.credential_state),
        model.has_secret,
    );
    if reveal_key {
        let value = match revealed.flatten() {
            Some(key) => key,
            // Not collapsed: this is a literal placeholder, not stored data.
            None if key_source == "environment" => {
                "(supplied by the DEEPSEEK_API_KEY environment override; not echoed)".to_owned()
            }
            None => "(not stored)".to_owned(),
        };
        // `api_key_source` goes FIRST so the revealed secret — the one cell
        // that must be rendered verbatim and therefore cannot be collapsed —
        // stays the last line and cannot forge a field after itself.
        human.push_str(&format!("\napi_key_source: {key_source}\napi_key: {value}"));
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
    // A keychain failure is a probe RESULT, not a crash: like every other
    // `models test` outcome it renders a single-line JSON row on stdout
    // (with `credential_unavailable`), so scripts can branch on it instead
    // of parsing stderr text. Parity by design: the GUI's connection test
    // resolves the stored key identically, so env-overridden keys (e.g.
    // DEEPSEEK_API_KEY) are not honored here either.
    let key = match resolve_saved_model_key(&model) {
        Ok(key) => key.unwrap_or_default(),
        Err(error) => {
            let probe = connection_result(
                false,
                "credential_unavailable",
                Some(error.to_string()),
                None,
            );
            return Ok(render_probe_outcome(&probe, output));
        }
    };
    let probe = run_connection_probe(&model.base_url, &key);
    Ok(render_probe_outcome(&probe, output))
}

fn render_probe_outcome(probe: &ConnectionProbe, output: OutputMode) -> CliOutcome {
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
    CliOutcome {
        exit_code: if probe.ok {
            ExitCode::Success
        } else {
            ExitCode::Failed
        },
        stdout: render(output, human, &json),
    }
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

/// Where a URL handed to [`is_loopback_url`] came from, which decides how a
/// malformed one is classified.
///
/// A URL typed on the command line is the caller's mistake: usage, exit 2,
/// and re-running with a corrected argument fixes it. A `base_url` read back
/// out of `settings.json` is not something argv can be blamed for — the
/// invocation was well-formed and the host's own state is broken — so it is a
/// host failure, exit 1. `models test` already classifies exactly this
/// condition that way (`{"code":"invalid_url"}`, exit 1, `run_connection_probe`),
/// and probe-local reporting the same corrupt stored value as a usage error
/// made the two commands disagree about the same fact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UrlOrigin {
    /// Supplied by the user as `--url`.
    Argument,
    /// Read out of the persisted model configuration.
    StoredConfig,
}

impl UrlOrigin {
    /// Malformed-URL error carrying this origin's exit-code classification.
    fn malformed(self, detail: impl std::fmt::Display) -> CliError {
        match self {
            Self::Argument => CliError::usage(format!("probe-local url {detail}")),
            Self::StoredConfig => {
                CliError::failed(format!("invalid_url: the active model base_url {detail}"))
            }
        }
    }
}

/// Loopback-only guard for `probe-local`: the CLI refuses non-loopback hosts
/// with a usage error instead of the GUI's silent "generic" fallback, so a
/// typo can never send a probe request to a remote endpoint. `origin` decides
/// only how a MALFORMED url is classified (see [`UrlOrigin`]); a well-formed
/// non-loopback url returns `Ok(false)` for the caller to refuse.
fn is_loopback_url(raw: &str, origin: UrlOrigin) -> Result<bool, CliError> {
    let url = reqwest::Url::parse(raw.trim())
        .map_err(|error| origin.malformed(format!("is not a valid url: {error}")))?;
    let Some(host) = url.host_str() else {
        return Err(origin.malformed("has no host"));
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

/// The process-wide probe client, built once and reused — the CLI's
/// equivalent of `core/model_endpoint.rs`'s `shared_probe_client`.
///
/// This is called from inside `get_json`, i.e. once per candidate request.
/// Building a fresh `reqwest::blocking::Client` there threw away the
/// connection pool AND spun up a new internal runtime thread for every one of
/// the eight probe requests `probe-local` issues, so the mirror had strictly
/// worse connection behaviour than the GUI it mirrors. A `OnceLock` keeps the
/// same client (and therefore the same keep-alive pool) for the whole
/// process; `reqwest::blocking::Client` is `Send + Sync`, so the parallel
/// candidates in [`probe_candidates`] share it safely.
fn probe_client() -> Option<&'static reqwest::blocking::Client> {
    static CLIENT: std::sync::OnceLock<Option<reqwest::blocking::Client>> =
        std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(3))
                // Local model servers have no business redirecting; following
                // a 302 would let a loopback service turn the probe into an
                // arbitrary remote request.
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .ok()
        })
        .as_ref()
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

/// Generous cap on one probe/model-list response body: the timeout bounds
/// time, but a hostile loopback endpoint can stream within it — the same
/// bounded-IO rule every other CLI read follows.
const PROBE_BODY_CAP_BYTES: usize = 4 * 1024 * 1024;

/// Reads and parses a probe response body under [`PROBE_BODY_CAP_BYTES`];
/// an over-cap or non-JSON body degrades to `None` like any other probe
/// failure.
fn read_json_capped(response: reqwest::blocking::Response) -> Option<serde_json::Value> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    response
        .take(PROBE_BODY_CAP_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > PROBE_BODY_CAP_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

fn get_json(url: &str, bearer: Option<&str>) -> Option<serde_json::Value> {
    let response = apply_bearer(probe_client()?.get(url), bearer)
        .send()
        .ok()?
        .error_for_status()
        .ok()?;
    read_json_capped(response)
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

/// Fetches an OpenAI-compatible model-list response body. URL convention:
/// a configured root already ending in `/v1` appends `/models` directly
/// (same convention as the GUI's `fetch_v1_models`); the bare host form
/// (common for local vLLM) gets `/v1/models` appended first. For non-`/v1`
/// version roots (glm `/api/paas/v4`, Volcengine Ark `/api/v3`, etc. — their
/// model list lives at `{base}/models`, and appending `/v1` would 404),
/// retry `{base}/models` once only when the primary candidate clearly
/// reports "path not found" (404/405); auth failures (401/403) are not
/// helped by switching paths, and timeouts/connection refusals mean the
/// host is unreachable — neither is retried, conservatively falling back
/// to no facts. Mirrors the GUI's `fetch_v1_models` fallback exactly.
fn fetch_v1_models(base_url: &str, bearer: Option<&str>) -> V1ModelsProbe {
    let Some(client) = probe_client() else {
        return V1ModelsProbe::Miss;
    };
    let trimmed = base_url.trim_end_matches('/');
    let (primary, fallback) = if trimmed.ends_with("/v1") {
        (format!("{trimmed}/models"), None)
    } else {
        (
            format!("{trimmed}/v1/models"),
            Some(format!("{trimmed}/models")),
        )
    };
    let Ok(response) = apply_bearer(client.get(&primary), bearer).send() else {
        return V1ModelsProbe::Miss;
    };
    // Retry once only when the primary candidate clearly reports "path not
    // found" (404/405) and a fallback candidate exists; auth failures
    // (401/403) are not helped by switching paths, and timeouts/connection
    // refusals mean the host is unreachable — neither is retried,
    // conservatively falling back to no facts.
    let response = match (response.status().as_u16(), fallback) {
        (200..=299, _) => response,
        (404 | 405, Some(url)) => {
            let Ok(response) = apply_bearer(client.get(&url), bearer).send() else {
                return V1ModelsProbe::Miss;
            };
            match response.status().as_u16() {
                200..=299 => response,
                401 | 403 => return V1ModelsProbe::AuthRequired,
                _ => return V1ModelsProbe::Miss,
            }
        }
        (401 | 403, _) => return V1ModelsProbe::AuthRequired,
        _ => return V1ModelsProbe::Miss,
    };
    read_json_capped(response).map_or(V1ModelsProbe::Miss, V1ModelsProbe::Body)
}

/// What the OpenAI-compatible model list answered, kept as three cases rather
/// than `Option` because "the endpoint refused to talk to me" is a DIFFERENT
/// fact from "the endpoint had nothing to say", and `probe-local` must not
/// report the first as the second. `/v1/models` is the one endpoint every
/// OpenAI-compatible local server exposes, so its 401/403 is the reliable
/// signal that the host is authenticated and every signature probe was
/// answered with a challenge rather than a signature.
#[derive(Debug, Default, PartialEq, Eq)]
enum V1ModelsProbe {
    /// A parsed JSON body.
    Body(serde_json::Value),
    /// 401/403: the endpoint exists and is authenticated, but this probe was
    /// not allowed to see its signature.
    AuthRequired,
    /// Unreachable, non-JSON, over the body cap, or any other status.
    #[default]
    Miss,
}

impl V1ModelsProbe {
    fn body(&self) -> Option<&serde_json::Value> {
        match self {
            Self::Body(value) => Some(value),
            _ => None,
        }
    }
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

/// The kind reported when every signature probe was answered with an
/// authentication challenge. It is deliberately NOT one of the GUI's kind
/// strings: those all assert something about the server's identity, and this
/// case asserts the opposite — that the probe learned nothing. Reporting
/// `generic` here (what the sequential mirror did) is the silently wrong
/// answer the classification must never give.
const LOCAL_KIND_UNKNOWN_AUTHENTICATED: &str = "unknown_authenticated";

/// One round of candidate probes, as facts rather than as a decision.
/// Mirrors `core/model_endpoint.rs`'s `ProbeCandidateHits` so the priority
/// rule below can be exercised without a network.
#[derive(Debug, Default)]
struct ProbeCandidateHits {
    docker_mgmt_shape: bool,
    ollama: bool,
    lmstudio_v0: bool,
    koboldcpp: bool,
    llamacpp: bool,
    sglang: bool,
    v1_models: V1ModelsProbe,
}

/// Issues all seven candidate probes CONCURRENTLY, the way the GUI issues
/// them with `tokio::join!` (`core/model_endpoint.rs`).
///
/// The sequential mirror ran them one after another, so a hung loopback
/// endpoint cost up to 8 x 3 s ~= 24 s instead of the GUI's ~3 s: every probe
/// shares one client-level 3 s timeout, and run in parallel the whole round
/// is bounded by the slowest single request (plus at most one extra window
/// for `fetch_v1_models`'s 404/405 root fallback). `std::thread::scope`
/// borrows `base_url` and `bearer` directly, so this needs no new dependency
/// and no async runtime.
///
/// Running every candidate always — instead of returning early on the first
/// hit — is exactly what the GUI does and cannot change the selected kind:
/// each candidate probes a DIFFERENT endpoint and its hit is independent of
/// the others. It does mean a matched server still receives the remaining
/// probes, which is the price of the ~8x latency win.
fn probe_candidates(base_url: &str, bearer: Option<&str>) -> ProbeCandidateHits {
    std::thread::scope(|scope| {
        let docker = scope.spawn(|| probe_docker_model_runner(base_url, bearer));
        let ollama = scope.spawn(|| probe_ollama_tags(base_url, bearer));
        let lmstudio = scope.spawn(|| probe_lmstudio_v0(base_url, bearer));
        let koboldcpp = scope.spawn(|| probe_koboldcpp(base_url, bearer));
        let llamacpp = scope.spawn(|| probe_llamacpp(base_url, bearer));
        let sglang = scope.spawn(|| probe_sglang(base_url, bearer));
        let v1_models = scope.spawn(|| fetch_v1_models(base_url, bearer));
        // A panicking probe thread degrades to "no hit" rather than taking
        // the command down: the probes are best-effort facts, and one
        // candidate's failure must not lose the other six.
        ProbeCandidateHits {
            docker_mgmt_shape: docker.join().unwrap_or(false),
            ollama: ollama.join().unwrap_or(false),
            lmstudio_v0: lmstudio.join().unwrap_or(false),
            koboldcpp: koboldcpp.join().unwrap_or(false),
            llamacpp: llamacpp.join().unwrap_or(false),
            sglang: sglang.join().unwrap_or(false),
            v1_models: v1_models.join().unwrap_or_default(),
        }
    })
}

/// The signature-exclusivity priority of the GUI's `select_local_server_kind`:
/// DMR port gate > Ollama > LM Studio > KoboldCpp > llama.cpp > SGLang >
/// LMDeploy > vLLM > generic. Pure, so the order is pinned by a test instead
/// of by a live server.
fn select_local_server_kind_from_hits(hits: &ProbeCandidateHits) -> &'static str {
    if hits.docker_mgmt_shape {
        return "dockermodelrunner";
    }
    if hits.ollama {
        return "ollama";
    }
    if hits.lmstudio_v0 {
        return "lmstudio";
    }
    if hits.koboldcpp {
        return "koboldcpp";
    }
    if hits.llamacpp {
        return "llamacpp";
    }
    if hits.sglang {
        return "sglang";
    }
    if let Some(v1_models) = hits.v1_models.body() {
        if v1_models_owned_by_matches(v1_models, "lmdeploy") {
            return "lmdeploy";
        }
        if v1_models_owned_by_matches(v1_models, "vllm") {
            return "vllm";
        }
    }
    // Only reached when nothing matched. An authenticated endpoint answers
    // every signature probe with a challenge, so `generic` here would be a
    // classification the probe never earned.
    if hits.v1_models == V1ModelsProbe::AuthRequired {
        return LOCAL_KIND_UNKNOWN_AUTHENTICATED;
    }
    "generic"
}

fn select_local_server_kind(base_url: &str, bearer: Option<&str>) -> &'static str {
    select_local_server_kind_from_hits(&probe_candidates(base_url, bearer))
}

/// `models probe-local [--url URL [--model ID]] [--api-key-env VAR]`:
/// identify the local inference server kind. Without `--url` the target is
/// the active model's base_url and its stored credential, mirroring the GUI's
/// credential resolution. Refuses non-loopback hosts with a usage error
/// before any request is sent.
///
/// `--model ID` is the CLI's spelling of the GUI command's `model_id`
/// parameter (`probe_local_server_kind(base_url, api_key, model_id)`): it
/// names WHICH saved model's stored key the probe should present to the
/// `--url` endpoint. Without it, `--url` alone probes anonymously, an
/// api-key-protected vLLM answers every signature probe with a 401, and the
/// result used to come back as `{"kind":"generic"}` with exit 0 — a silently
/// wrong answer rather than an error.
///
/// The GUI's implicit fallback is deliberately NOT mirrored: it resolves
/// `model_id: None` to `prefs.active_model()`, and `saved_model_for_probe`'s
/// own comment warns that resolving a credential the caller did not name
/// sends one model's key to an arbitrary endpoint. Naming the model is
/// therefore required, and no credential stays the default.
fn probe_local(
    url: Option<&str>,
    api_key_env: Option<&str>,
    model_id: Option<&str>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    // Phase 1 — resolve and validate the target endpoint, for BOTH branches,
    // before anything else. Usage validation must precede env resolution so
    // scripts keying on the exit-code contract see the usage error (2) rather
    // than the --api-key-env failure (1) when both are wrong. The `--url`
    // branch already did this; the stored-base_url branch resolved
    // `--api-key-env` first and only checked loopback afterwards, so
    // `probe-local --api-key-env MISSING` exited 1 where the contract says 2.
    // Resolving the target first for both branches makes the ordering a
    // property of the function rather than of one branch.
    let active = match url {
        Some(url) => {
            if !is_loopback_url(url, UrlOrigin::Argument)? {
                return Err(CliError::usage(
                    "probe-local refuses non-loopback urls; pass a 127.0.0.1, ::1 or localhost endpoint",
                ));
            }
            None
        }
        None => {
            let prefs = safe_prefs();
            let model = prefs
                .active_model()
                .cloned()
                .ok_or_else(|| CliError::failed("no active model to probe"))?;
            if !is_loopback_url(&model.base_url, UrlOrigin::StoredConfig)? {
                return Err(CliError::usage(format!(
                    "the active model base_url {} is not a loopback endpoint; pass --url",
                    model.base_url
                )));
            }
            Some(model)
        }
    };
    // Phase 2 — only now resolve the explicit key override, whose failures
    // are host failures (exit 1).
    let explicit_key = match api_key_env {
        Some(var) => {
            let value = std::env::var(var).map_err(|_| {
                CliError::failed(format!(
                    "probe-local: api key environment variable {var} is not set"
                ))
            })?;
            // A set-but-empty variable would be filtered out by apply_bearer
            // and silently downgrade to an unauthenticated probe, which then
            // misclassifies an authenticated local server as `generic` —
            // exactly what the explicit-key lane exists to prevent.
            if value.trim().is_empty() {
                return Err(CliError::failed(format!(
                    "probe-local: api key environment variable {var} is set but empty"
                )));
            }
            Some(value)
        }
        None => None,
    };
    // Phase 3 — the `--model ID` lane: the GUI's `model_id` parameter, read
    // through the same `credential_ref` -> credential store resolution as
    // `resolve_saved_model_key(Some(id))`. An unknown id FAILS here rather
    // than degrading to an anonymous probe (which the GUI's `.ok().flatten()`
    // would do): probing without the credential the caller named is how the
    // misclassification this flag exists to prevent comes back.
    let named_model_key = match model_id {
        Some(id) => {
            let prefs = safe_prefs();
            let model = find_model(&prefs, id)?;
            let key = resolve_saved_model_key(&model)
                .map_err(|error| CliError::failed(format!("credential_unavailable: {error}")))?
                .filter(|key| !key.trim().is_empty());
            if key.is_none() {
                return Err(CliError::failed(format!(
                    "probe-local: model {id} has no stored api key to present"
                )));
            }
            key
        }
        None => None,
    };
    let (target, bearer) = match active {
        // An authenticated local endpoint 401s every signature probe and
        // misclassifies as generic without an explicit credential (the GUI
        // form-key lane, or `--model` for a saved one).
        None => (
            url.expect("the explicit-url branch sets active to None")
                .to_owned(),
            explicit_key.or(named_model_key),
        ),
        Some(model) => {
            let bearer = match explicit_key {
                Some(key) => Some(key),
                None => {
                    // Swallowing a keychain failure here would turn every
                    // signed request into a 401 and classify a working
                    // server as `generic` — surface it instead.
                    resolve_saved_model_key(&model)
                        .map_err(|error| {
                            CliError::failed(format!("credential_unavailable: {error}"))
                        })?
                        .filter(|key| !key.trim().is_empty())
                }
            };
            (model.base_url, bearer)
        }
    };
    let authenticated = bearer.is_some();
    let kind = select_local_server_kind(&target, bearer.as_deref());
    // `unknown_authenticated` is not a classification, it is the absence of
    // one: the endpoint answered every signature probe with 401/403. Saying
    // so with exit 1 keeps a script from reading a kind the probe never
    // earned, and the detail names the flag that would let the next run
    // actually classify it.
    if kind == LOCAL_KIND_UNKNOWN_AUTHENTICATED {
        let detail = if authenticated {
            "the endpoint rejected the credential that was presented (401/403), so no server \
             signature could be read"
        } else {
            "the endpoint requires authentication (401/403); rerun with --model <id> or \
             --api-key-env <var> so the probe can present a credential"
        };
        let text = render(
            output,
            format!("url: {target}\nkind: {kind}\ndetail: {detail}"),
            &serde_json::json!({
                "url": target,
                "kind": kind,
                "authenticated": authenticated,
                "detail": detail,
            }),
        );
        return Ok(CliOutcome {
            exit_code: ExitCode::Failed,
            stdout: text,
        });
    }
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
///
/// The keyless dump deliberately ignores `--output`: its payload is the whole
/// nested `UserPrefs` document, which has no `key = value` rendering, so it is
/// always JSON. That is stated in `SETTINGS_USAGE` rather than left for a
/// caller to discover from output that did not change.
fn settings_get(key: Option<SettingsKey>, output: OutputMode) -> Result<CliOutcome, CliError> {
    let Some(key) = key else {
        let prefs = safe_prefs();
        // Serialized straight to the output string. The previous shape went
        // through `to_value` and then `to_string(...).unwrap_or_default()`,
        // which converted a serialization failure into an EMPTY stdout with
        // exit 0 — a caller piping this into `jq` saw a successful run that
        // produced no settings, indistinguishable from a successful run of a
        // command that has no output. A failure to serialize the settings is
        // a host failure and must exit 1 saying so.
        let text = serde_json::to_string(&prefs)
            .map_err(|error| CliError::failed(format!("settings serialization failed: {error}")))?;
        return Ok(success(text));
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
    // `update_transaction` already returns the re-parsed post-save prefs
    // (`platform/prefs/mod.rs`: `Ok(Self::load_unlocked(false))`), so the note
    // below reads THIS write's effective state. The discarded value plus a
    // fresh `UserPrefs::load()` used to be a TOCTOU — a concurrent writer
    // between the commit and the load produced a spurious or missing
    // locale-policy note — and `load()` is additionally the PERSISTING
    // variant (`load_unlocked(true)`), which re-runs `migrate_models` and
    // `migrate_plaintext_api_keys_with_store` (a keyring write path) and can
    // rewrite settings.json as a side effect of reporting one boolean.
    let saved = UserPrefs::update_transaction(|prefs| {
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
    // The prefs layer may normalize a request back (the memory-locale policy
    // reverts `memory_enabled true` under a non-zh-Hans UI language); say so
    // instead of printing a plain success for a no-op.
    let note = if let (SettingsKey::MemoryEnabled, SettingsValue::Bool(requested)) = (&key, &value)
    {
        let effective = saved.memory_enabled;
        (effective != *requested).then(|| {
            format!(
                "note: the memory locale policy kept memory_enabled = {effective} (memory \
                 features require the zh-Hans UI language)"
            )
        })
    } else {
        None
    };
    let key_name = SettingsKey::ALL
        .iter()
        .find(|(_, candidate)| *candidate == key)
        .map(|(name, _)| *name)
        .unwrap_or_default();
    let human = match &note {
        Some(note) => format!("{key_name} updated\n{note}"),
        None => format!("{key_name} updated"),
    };
    let value = match note {
        Some(note) => serde_json::json!({ "updated": key_name, "note": note }),
        None => serde_json::json!({ "updated": key_name }),
    };
    let text = render(output, human, &value);
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

/// `settings search set --provider P [--api-key-env V | --clear]`: stores or
/// clears provider P's credential with the same bookkeeping
/// (`mark_configured` / `mark_missing`) as the GUI's search settings save
/// path.
///
/// P becomes the ACTIVE search provider only when the caller is selecting it
/// — that is, on every form except `--clear`. Clearing is a credential
/// operation on a named provider and must not move the user's search
/// backend; `--provider tavily --clear` means "forget the tavily key", not
/// "search with tavily from now on".
fn search_set(
    provider: SearchProvider,
    api_key_env: &Option<String>,
    clear: bool,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    if provider == SearchProvider::Bing && api_key_env.is_some() {
        return Err(CliError::usage(
            "provider bing does not use an api key; it needs no configuration",
        ));
    }
    let secret = resolve_secret(api_key_env, false)?;
    let stored = if clear { None } else { secret };
    let stored_reference = stored.as_ref().map(|_| provider.credential_reference());
    // Gate the clear-path keyring deletion on the provider actually holding
    // a credential reference: a never-configured provider has no keyring
    // entry, and deleting anyway errors on most keyrings — a spurious
    // warning for a no-op (the same gate `models remove` applies to its
    // credential_ref). The lookup happens INSIDE the transaction closure
    // below rather than through an extra `UserPrefs::load()` here: that load
    // is the PERSISTING variant (it re-runs the migrations, including the
    // keyring write path) and reading it before the critical section made the
    // reference a TOCTOU snapshot of a different prefs state than the one the
    // clear commits against. Captured by `&mut` exactly like `models remove`.
    let mut reference_to_delete: Option<CredentialReference> = None;
    // Replacing an existing key OVERWRITES it in the keyring, so the
    // rollback below must restore the previous value — deleting would
    // destroy the old secret while prefs still references it (strictly
    // worse than not rolling back). Snapshot it before the transaction,
    // keeping a snapshot READ ERROR distinct from "no previous secret":
    // rolling back on unknown state must not delete a key that may still
    // exist and still be referenced by prefs.
    let previous_secret = stored_reference
        .as_ref()
        .map(|reference| SystemCredentialStore::new().get(reference));
    let transaction = UserPrefs::update_transaction(|prefs| {
        // Only a caller actually SELECTING a provider switches the active
        // one. `--clear` is a credential operation: clearing a non-active
        // provider's key used to activate that provider as a side effect, so
        // `settings search set --provider tavily --clear` silently moved
        // search off whatever the user had chosen.
        if !clear {
            prefs.search.provider = provider;
        }
        if let Some(key) = &stored {
            let reference = provider.credential_reference();
            // Normalized exactly like `models add` and the GUI
            // (`platform/prefs` `apply_model_credential`). This call site
            // used to pass `key` verbatim, so a `--api-key-env` secret with
            // a trailing newline was stored with it and every search request
            // 401'd while `settings search test` still reported
            // `configured` — see `secret_for_storage`.
            SystemCredentialStore::new()
                .set(&reference, secret_for_storage(key))
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
                reference_to_delete = credential.credential_ref.clone();
                credential.mark_missing();
            }
        }
        Ok(())
    });
    // The transaction's own return value is the post-save prefs, so the
    // active provider reported below is read from the state that committed
    // rather than from a second `UserPrefs::load()`.
    let saved = match transaction {
        Ok(saved) => saved,
        Err(error) => {
            // The closure may have stored the keyring secret before the save
            // failed; restore the pre-transaction state so no orphaned entry
            // outlives the prefs record (same standard as models add). An
            // overwrite restores the previous secret; a fresh store deletes.
            if let Some(reference) = stored_reference.as_ref() {
                let store = SystemCredentialStore::new();
                match previous_secret {
                    // A previous secret existed: the overwrite destroyed it,
                    // so the rollback must put it back.
                    Some(Ok(Some(old))) => {
                        let _ = store.set(reference, old.as_str());
                    }
                    // No previous secret existed: remove the just-stored one.
                    Some(Ok(None)) => {
                        let _ = store.delete(reference);
                    }
                    // The pre-transaction state is unknown (keychain read
                    // failed): leave the keyring untouched. Deleting could
                    // destroy a secret prefs still references; a stale
                    // orphaned entry is the benign direction.
                    Some(Err(_)) => {}
                    // No write happened (nothing to store), so no rollback.
                    None => {}
                }
            }
            return Err(prefs_error(error));
        }
    };
    if clear {
        // Only after the prefs save succeeded — deleting first would leave
        // the prefs entry pointing at a credential that no longer exists if
        // the save fails (a leftover keyring entry is the benign direction).
        // The same benign direction applies on the way out: the prefs entry
        // is already cleared, so a keyring deletion failure warns and
        // succeeds like `models remove`, instead of reporting a failure
        // whose only remedy (rerun) has nothing left to do.
        if let Some(reference) = reference_to_delete {
            if let Err(error) = SystemCredentialStore::new().delete(&reference) {
                note!(
                    "pinvou: warning: credential cleared from settings, but the keyring entry \
                     could not be deleted: {}",
                    error.user_message()
                );
            }
        }
    }
    let action = if clear {
        "cleared"
    } else if stored.is_some() {
        "configured"
    } else {
        "unchanged"
    };
    // `provider` names the credential that was touched; `active_provider`
    // names what search actually runs on afterwards. They differ exactly when
    // `--clear` targets a non-active provider, which is precisely the case a
    // single `provider:` line used to render ambiguously.
    let active = saved.search.provider.as_str();
    let text = render(
        output,
        format!(
            "provider: {}\ncredential: {action}\nactive_provider: {active}",
            provider.as_str()
        ),
        &serde_json::json!({
            "provider": provider.as_str(),
            "credential": action,
            "active_provider": active,
        }),
    );
    Ok(success(text))
}

/// `settings search test <provider>`: every provider now runs a REAL request.
///
/// It did not used to. Bing ran a live probe and the other four — 4 of the 5
/// `SearchProvider` variants — returned `{"ok":true,"code":"configured"}` the
/// moment a non-empty credential existed, without contacting anything, so a
/// revoked, expired or garbage key passed a command called `test`. The
/// contract now is: `ok: true` means the provider answered, and the
/// `verified` field says which of the two was actually established.
///
/// The four API lanes are built from the request shapes the product's own
/// search tool sends (`CodeWhale/crates/tui/src/tools/web_search.rs`), not
/// from an invented API contract — see [`search_api_request`].
fn search_test(provider: SearchProvider, output: OutputMode) -> Result<CliOutcome, CliError> {
    let probe = if provider == SearchProvider::Bing {
        run_bing_probe()
    } else {
        match resolve_search_key(provider) {
            Ok(Some(key)) if !key.trim().is_empty() => run_search_api_probe(provider, key.trim()),
            Ok(_) => SearchProbe {
                ok: false,
                code: "no_api_key",
                detail: Some(format!(
                    "no api key configured for provider {}; set one with settings search set",
                    provider.as_str()
                )),
                verified: VERIFIED_CREDENTIAL_PRESENCE,
            },
            Err(error) => SearchProbe {
                ok: false,
                code: "credential_unavailable",
                detail: Some(error),
                verified: VERIFIED_CREDENTIAL_PRESENCE,
            },
        }
    };
    let json = serde_json::json!({
        "provider": provider.as_str(),
        "ok": probe.ok,
        "code": probe.code,
        "verified": probe.verified,
        "detail": probe.detail,
    });
    let mut human = format!(
        "provider: {}\nok: {}\ncode: {}\nverified: {}",
        provider.as_str(),
        probe.ok,
        probe.code,
        probe.verified,
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

/// A request to the provider was attempted and its outcome — a classified
/// response status, or the transport error that prevented one — is what the
/// row reports.
const VERIFIED_LIVE_PROBE: &str = "live_probe";
/// NOTHING was sent. Only the configured credential was inspected, so the row
/// says something about the CLI's own state and nothing about the provider.
const VERIFIED_CREDENTIAL_PRESENCE: &str = "credential_presence";
/// Neither: the probe could not run at all (the HTTP client would not build).
const VERIFIED_NOTHING: &str = "nothing";

struct SearchProbe {
    ok: bool,
    code: &'static str,
    detail: Option<String>,
    /// What the row's `ok` is a statement ABOUT. Without it a caller reads
    /// `ok: true` as "this key works", which is a claim only a live probe
    /// earns — see [`VERIFIED_LIVE_PROBE`] and friends.
    verified: &'static str,
}

/// The search endpoints the product actually calls, copied from
/// `CodeWhale/crates/tui/src/tools/web_search.rs` (`TAVILY_ENDPOINT`,
/// `BOCHA_ENDPOINT`, `METASO_ENDPOINT`, `BAIDU_ENDPOINT`). They are duplicated
/// rather than imported because the CLI does not depend on the TUI crate;
/// they must be changed together with that file.
const TAVILY_SEARCH_ENDPOINT: &str = "https://api.tavily.com/search";
const BOCHA_SEARCH_ENDPOINT: &str = "https://api.bochaai.com/v1/web-search";
const METASO_SEARCH_ENDPOINT: &str = "https://metaso.cn/api/v1/search";
const BAIDU_SEARCH_ENDPOINT: &str = "https://qianfan.baidubce.com/v2/ai_search/web_search";

/// The query a probe sends. A `test` that validates a key necessarily spends
/// one search against the provider's quota — that is the cost of the command
/// meaning what its name says — so it asks for the smallest possible result
/// set.
const SEARCH_PROBE_QUERY: &str = "pinvou";
const SEARCH_PROBE_RESULTS: u32 = 1;

/// The request timeout for a search probe, matching the product's own
/// `DEFAULT_SEARCH_TIMEOUT_MS` (15 s, `CodeWhale/crates/tui/src/tools/web/
/// contract.rs`). The 8 s used by the model-connection probe is too tight for
/// these endpoints — Baidu's AI Search in particular is model-backed — and a
/// premature `timeout` on a perfectly good key would be a false negative.
const SEARCH_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Builds one provider's real search request. Every shape here — endpoint,
/// auth scheme and body — is transcribed from the corresponding builder in
/// `CodeWhale/crates/tui/src/tools/web_search.rs`: Tavily carries the key in
/// the JSON body (`api_key`), Bocha, Metaso and Baidu carry it as
/// `Authorization: Bearer <key>`. Bing has no API-key form and never reaches
/// this function.
fn search_api_request(
    client: &reqwest::blocking::Client,
    provider: SearchProvider,
    key: &str,
) -> Option<reqwest::blocking::RequestBuilder> {
    let request = match provider {
        SearchProvider::Bing => return None,
        SearchProvider::Tavily => client
            .post(TAVILY_SEARCH_ENDPOINT)
            .json(&serde_json::json!({
                "api_key": key,
                "query": SEARCH_PROBE_QUERY,
                "search_depth": "basic",
                "max_results": SEARCH_PROBE_RESULTS,
            })),
        SearchProvider::Bocha => {
            client
                .post(BOCHA_SEARCH_ENDPOINT)
                .bearer_auth(key)
                .json(&serde_json::json!({
                    "query": SEARCH_PROBE_QUERY,
                    "freshness": "noLimit",
                    "count": SEARCH_PROBE_RESULTS,
                }))
        }
        SearchProvider::Metaso => {
            client
                .post(METASO_SEARCH_ENDPOINT)
                .bearer_auth(key)
                .json(&serde_json::json!({
                    "q": SEARCH_PROBE_QUERY,
                    "scope": "webpage",
                    "size": SEARCH_PROBE_RESULTS,
                }))
        }
        SearchProvider::Baidu => {
            client
                .post(BAIDU_SEARCH_ENDPOINT)
                .bearer_auth(key)
                .json(&serde_json::json!({
                    "messages": [{ "role": "user", "content": SEARCH_PROBE_QUERY }],
                    "search_source": "baidu_search_v2",
                    "resource_type_filter": [{ "type": "web", "top_k": SEARCH_PROBE_RESULTS }],
                }))
        }
    };
    Some(request)
}

/// Runs one API provider's live probe and classifies the answer with the
/// SAME status vocabulary `models test` uses (`connection_http_result`:
/// `auth_invalid` for 401, `auth_forbidden` for 403, `rate_limited` for 429,
/// ...), so one exit contract covers both commands. Transport failures go
/// through `connection_error_result`, which redacts the message.
fn run_search_api_probe(provider: SearchProvider, key: &str) -> SearchProbe {
    let Ok(client) = reqwest::blocking::Client::builder()
        .timeout(SEARCH_PROBE_TIMEOUT)
        .build()
    else {
        return SearchProbe {
            ok: false,
            code: "client_error",
            detail: None,
            verified: VERIFIED_NOTHING,
        };
    };
    let Some(request) = search_api_request(&client, provider, key) else {
        // Bing is routed to `run_bing_probe` before this function is reached.
        return SearchProbe {
            ok: false,
            code: "unsupported_provider",
            detail: Some(format!(
                "provider {} has no api-key search request",
                provider.as_str()
            )),
            verified: VERIFIED_NOTHING,
        };
    };
    match request.send() {
        Ok(response) => {
            let probe = connection_http_result(response.status());
            SearchProbe {
                ok: probe.ok,
                code: probe.code,
                detail: probe.detail,
                verified: VERIFIED_LIVE_PROBE,
            }
        }
        Err(error) => {
            let probe = connection_error_result(&error);
            SearchProbe {
                ok: false,
                code: probe.code,
                detail: probe.detail,
                verified: VERIFIED_LIVE_PROBE,
            }
        }
    }
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
            verified: VERIFIED_NOTHING,
        };
    };
    match client
        .get("https://www.bing.com/search")
        .query(&[("q", SEARCH_PROBE_QUERY)])
        .send()
    {
        Ok(response) => {
            let status = response.status();
            if status.is_success() {
                SearchProbe {
                    ok: true,
                    code: "ok",
                    detail: Some(format!("HTTP {}", status.as_u16())),
                    verified: VERIFIED_LIVE_PROBE,
                }
            } else {
                SearchProbe {
                    ok: false,
                    code: "http_error",
                    detail: Some(format!("HTTP {}", status.as_u16())),
                    verified: VERIFIED_LIVE_PROBE,
                }
            }
        }
        Err(error) => {
            let probe = connection_error_result(&error);
            SearchProbe {
                ok: false,
                code: probe.code,
                detail: probe.detail,
                verified: VERIFIED_LIVE_PROBE,
            }
        }
    }
}

/// Resolves the key a real search request would sign with, in the order
/// `features/assistant/platform/bridge.rs::search_api_key` uses: the
/// provider's environment variable names first (`env_key_names`), then the
/// credential store entry behind `credentials[provider].credential_ref`.
///
/// `search_api_key` has a THIRD tier the CLI deliberately does not mirror:
/// `self.prefs.search.normalized_api_key()`, the legacy plaintext
/// `search.api_key` field. That field is `#[serde(default, skip_serializing)]`
/// (`platform/prefs/search.rs`), `SearchPrefs::normalize` sets it to `None`
/// on every save, and `UserPrefs::load` ends with
/// `sanitize_plaintext_api_keys()`, which nulls it again — so on any prefs the
/// CLI can load it is unconditionally `None`. Mirroring it would add a tier
/// that can never fire and imply the CLI reads a plaintext key out of
/// settings.json, which it must never do.
fn resolve_search_key(provider: SearchProvider) -> Result<Option<String>, String> {
    for name in provider.env_key_names() {
        if let Ok(value) = std::env::var(name) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Ok(Some(trimmed.to_owned()));
            }
        }
    }
    let mut prefs = UserPrefs::load();
    prefs.refresh_credential_states_with_store(&SystemCredentialStore::new());
    let Some(credential) = prefs.search.credentials.get(&provider) else {
        return Ok(None);
    };
    let Some(reference) = credential.credential_ref.clone() else {
        return Ok(None);
    };
    // A keychain failure is a credential-store problem, not "no key":
    // swallowing it here would degrade every signed request into a
    // misleading no_api_key report.
    let value = SystemCredentialStore::new()
        .get(&reference)
        .map_err(|error| error.user_message())?
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The single normalization both credential lanes write through.
    ///
    /// This is a unit test rather than a contract test on purpose: observing
    /// the stored bytes end to end would mean writing to the real OS keychain
    /// (`SystemCredentialStore` has no test backdoor — it only falls back to
    /// file storage when the OS keyring `probe()` fails), which no test in
    /// this crate is allowed to do. So the normalization is pinned here and
    /// the wiring is enforced by every `.set(` call site in this module
    /// (`models add`, `models edit`, `settings search set`) passing a value
    /// that went through `secret_for_storage(...)`.
    ///
    /// The trailing newline is the case that mattered: `--api-key-env`
    /// values come back from `support::resolve_secret` verbatim (only its
    /// stdin lane trims), a CI secret file carries a newline, and storing it
    /// 401s every request while `settings search test` — which trims on read
    /// — still reports the provider as `configured`.
    #[test]
    fn secret_for_storage_strips_the_whitespace_ci_secrets_carry() {
        assert_eq!(secret_for_storage("sk-abc123\n"), "sk-abc123");
        assert_eq!(secret_for_storage("sk-abc123\r\n"), "sk-abc123");
        assert_eq!(secret_for_storage("  sk-abc123  "), "sk-abc123");
        // Interior characters are never touched: only the edges are noise.
        assert_eq!(secret_for_storage("sk-a b\tc"), "sk-a b\tc");
        assert_eq!(secret_for_storage("sk-abc123"), "sk-abc123");
        // A whitespace-only value normalizes to empty, which is what makes
        // `apply_new_model_credential` mark the model `missing` instead of
        // storing a blank secret and reporting it as configured.
        assert_eq!(secret_for_storage(" \n\t "), "");
    }

    /// Minimal loopback HTTP mock (std-only, same idea as the GUI's
    /// `models_mock`): each route is `(path, status, body)`; hit counts and
    /// the last Authorization header per path are recorded for assertions.
    struct ProbeMock {
        base_url: String,
        hits: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>>,
        auth: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
    }

    impl ProbeMock {
        fn hits_for(&self, path: &str) -> usize {
            self.hits
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(path)
                .copied()
                .unwrap_or(0)
        }

        fn auth_for(&self, path: &str) -> Option<String> {
            self.auth
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(path)
                .cloned()
        }
    }

    fn spawn_probe_mock(routes: &[(&str, u16, &str)]) -> ProbeMock {
        use std::io::Read;
        use std::sync::{Arc, Mutex};

        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind probe mock listener");
        let addr = listener.local_addr().expect("probe mock listener addr");
        let routes: Vec<(String, u16, String)> = routes
            .iter()
            .map(|(path, status, body)| ((*path).to_owned(), *status, (*body).to_owned()))
            .collect();
        let hits: Arc<Mutex<std::collections::HashMap<String, usize>>> =
            Arc::new(Mutex::new(std::collections::HashMap::new()));
        let hits_thread = hits.clone();
        let auth: Arc<Mutex<std::collections::HashMap<String, String>>> =
            Arc::new(Mutex::new(std::collections::HashMap::new()));
        let auth_thread = auth.clone();
        std::thread::spawn(move || {
            // Each test sends at most two requests; this also bounds the
            // leaked thread.
            for stream in listener.incoming().take(8) {
                let Ok(mut stream) = stream else { continue };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                loop {
                    let Ok(n) = stream.read(&mut chunk) else {
                        break;
                    };
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&buf);
                let path = request
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .split('?')
                    .next()
                    .unwrap_or("")
                    .to_owned();
                *hits_thread
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .entry(path.clone())
                    .or_insert(0) += 1;
                let authorization = request.lines().skip(1).find_map(|line| {
                    line.split_once(':')
                        .filter(|(name, _)| name.trim().eq_ignore_ascii_case("authorization"))
                        .map(|(_, value)| value)
                });
                if let Some(value) = authorization {
                    auth_thread
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .insert(path.clone(), value.trim().to_owned());
                }
                let (status, body) = routes
                    .iter()
                    .find(|(route, _, _)| route == &path)
                    .map_or((404, "{}".to_owned()), |(_, status, body)| {
                        (*status, body.clone())
                    });
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
            }
        });
        ProbeMock {
            base_url: format!("http://{addr}"),
            hits,
            auth,
        }
    }

    #[test]
    fn fetch_v1_models_falls_back_to_root_models_on_404() {
        // Non-v1 version roots such as glm /api/paas/v4 or Ark /api/v3:
        // appending /v1 always 404s and the model list lives at
        // {base}/models — the single fallback must hit, mirroring the GUI's
        // `fetch_v1_models_falls_back_to_root_models_on_404`.
        let mock = spawn_probe_mock(&[
            ("/v1/models", 404, "{}"),
            ("/models", 200, r#"{"data":[{"id":"glm-4.7"}]}"#),
        ]);
        let probe = fetch_v1_models(&mock.base_url, Some("route-key"));
        let value = probe
            .body()
            .expect("after 404 the fallback to {base}/models must happen");
        assert_eq!(value["data"][0]["id"], "glm-4.7");
        assert_eq!(mock.hits_for("/v1/models"), 1);
        assert_eq!(mock.hits_for("/models"), 1);
        assert_eq!(
            mock.auth_for("/models").as_deref(),
            Some("Bearer route-key"),
            "the fallback request must carry the same-origin credentials, never silently degrade to anonymous"
        );
    }

    #[test]
    fn fetch_v1_models_auth_failure_does_not_retry_alt_path() {
        let mock = spawn_probe_mock(&[
            ("/v1/models", 401, "{}"),
            ("/models", 200, r#"{"data":[{"id":"x"}]}"#),
        ]);
        // A 401 is reported as `AuthRequired`, not as "no facts": it is the
        // signal `probe-local` needs in order to refuse to answer `generic`.
        assert_eq!(
            fetch_v1_models(&mock.base_url, None),
            V1ModelsProbe::AuthRequired
        );
        assert_eq!(
            mock.hits_for("/models"),
            0,
            "401 is an auth problem; switching paths cannot help, no retry"
        );
    }

    /// The 401 carried by the FALLBACK candidate must classify the same way
    /// as one carried by the primary: a non-`/v1` root (glm `/api/paas/v4`,
    /// Ark `/api/v3`) reaches its real model list only on the second
    /// candidate, so losing the signal there would put exactly those
    /// endpoints back on the silent `generic` answer.
    #[test]
    fn fetch_v1_models_reports_auth_required_from_the_fallback_candidate() {
        let mock = spawn_probe_mock(&[("/v1/models", 404, "{}"), ("/models", 403, "{}")]);
        assert_eq!(
            fetch_v1_models(&mock.base_url, None),
            V1ModelsProbe::AuthRequired
        );
        assert_eq!(mock.hits_for("/v1/models"), 1);
        assert_eq!(mock.hits_for("/models"), 1);
    }

    #[test]
    fn fetch_v1_models_v1_shaped_base_has_no_alt_path() {
        // A configured root ending in /v1: the primary candidate is
        // {base}/models == the mock's "/v1/models"; on 404 no second path
        // may ever appear (no fallback beyond /v1/models).
        let mock = spawn_probe_mock(&[("/v1/models", 404, "{}")]);
        let base = format!("{}/v1", mock.base_url);
        assert_eq!(fetch_v1_models(&base, None), V1ModelsProbe::Miss);
        assert_eq!(mock.hits_for("/v1/models"), 1);
        assert_eq!(
            mock.hits_for("/models"),
            0,
            "a /v1-shaped configured root has a single candidate, no fallback path"
        );
    }

    /// Pins the signature-exclusivity priority the GUI's
    /// `select_local_server_kind` implements. Parallelizing the candidate
    /// probes ([`probe_candidates`]) means every candidate now runs even when
    /// an earlier one hit, so the ORDER is the only thing left deciding the
    /// answer — if it drifts, a host that matches two signatures silently
    /// changes kind. Each case below sets one hit plus every LOWER-priority
    /// hit, so it fails if the entry is demoted below any of them.
    #[test]
    fn local_server_kind_priority_order_is_pinned() {
        let vllm_body = || {
            V1ModelsProbe::Body(serde_json::json!({
                "data": [{ "id": "m", "owned_by": "vllm" }]
            }))
        };
        let lmdeploy_body = || {
            V1ModelsProbe::Body(serde_json::json!({
                "data": [{ "id": "m", "owned_by": "lmdeploy" }]
            }))
        };
        // Everything hits at once: the highest-priority candidate wins.
        let all = ProbeCandidateHits {
            docker_mgmt_shape: true,
            ollama: true,
            lmstudio_v0: true,
            koboldcpp: true,
            llamacpp: true,
            sglang: true,
            v1_models: vllm_body(),
        };
        assert_eq!(
            select_local_server_kind_from_hits(&all),
            "dockermodelrunner"
        );
        let cases: &[(&str, ProbeCandidateHits)] = &[
            (
                "ollama",
                ProbeCandidateHits {
                    ollama: true,
                    lmstudio_v0: true,
                    koboldcpp: true,
                    llamacpp: true,
                    sglang: true,
                    v1_models: vllm_body(),
                    ..Default::default()
                },
            ),
            (
                "lmstudio",
                ProbeCandidateHits {
                    lmstudio_v0: true,
                    koboldcpp: true,
                    llamacpp: true,
                    sglang: true,
                    v1_models: vllm_body(),
                    ..Default::default()
                },
            ),
            (
                "koboldcpp",
                ProbeCandidateHits {
                    koboldcpp: true,
                    llamacpp: true,
                    sglang: true,
                    v1_models: vllm_body(),
                    ..Default::default()
                },
            ),
            (
                "llamacpp",
                ProbeCandidateHits {
                    llamacpp: true,
                    sglang: true,
                    v1_models: vllm_body(),
                    ..Default::default()
                },
            ),
            (
                "sglang",
                ProbeCandidateHits {
                    sglang: true,
                    v1_models: vllm_body(),
                    ..Default::default()
                },
            ),
            // LMDeploy outranks vLLM, and both are read off the SAME shared
            // /v1/models body (fetched once, like the GUI).
            (
                "lmdeploy",
                ProbeCandidateHits {
                    v1_models: V1ModelsProbe::Body(serde_json::json!({
                        "data": [
                            { "id": "a", "owned_by": "lmdeploy" },
                            { "id": "b", "owned_by": "vllm" },
                        ]
                    })),
                    ..Default::default()
                },
            ),
            (
                "vllm",
                ProbeCandidateHits {
                    v1_models: vllm_body(),
                    ..Default::default()
                },
            ),
            ("generic", ProbeCandidateHits::default()),
        ];
        for (expected, hits) in cases {
            assert_eq!(
                select_local_server_kind_from_hits(hits),
                *expected,
                "priority order changed around {expected}"
            );
        }
        assert_eq!(
            select_local_server_kind_from_hits(&ProbeCandidateHits {
                v1_models: lmdeploy_body(),
                ..Default::default()
            }),
            "lmdeploy"
        );
    }

    /// The honesty rule behind the priority table: when no signature matched
    /// AND the one universal endpoint answered with an auth challenge, the
    /// probe learned nothing and must not report `generic`.
    #[test]
    fn local_server_kind_reports_unknown_authenticated_instead_of_generic() {
        assert_eq!(
            select_local_server_kind_from_hits(&ProbeCandidateHits {
                v1_models: V1ModelsProbe::AuthRequired,
                ..Default::default()
            }),
            LOCAL_KIND_UNKNOWN_AUTHENTICATED,
        );
        // An auth challenge never overrides a signature that DID match: the
        // server identified itself on its native endpoint, which is a fact
        // the 401 does not take away.
        assert_eq!(
            select_local_server_kind_from_hits(&ProbeCandidateHits {
                ollama: true,
                v1_models: V1ModelsProbe::AuthRequired,
                ..Default::default()
            }),
            "ollama",
        );
    }

    /// End to end for the misclassification in the finding: an
    /// api-key-protected local server (every endpoint 401) probed with no
    /// credential used to answer `{"kind":"generic"}` with exit 0 — a
    /// silently wrong classification rather than an error. It must now report
    /// that it could not classify, fail, and name the flags that would let
    /// the next run succeed.
    ///
    /// Loopback-only and driven by the same std-only mock the other probe
    /// tests use, so it needs no external network.
    #[test]
    fn probe_local_refuses_to_classify_an_endpoint_that_401s_everything() {
        let mock = spawn_probe_mock(&[("/v1/models", 401, "{}")]);
        let outcome = probe_local(Some(&mock.base_url), None, None, OutputMode::Json)
            .expect("a completed probe is a result, not a command failure");
        assert_eq!(
            outcome.exit_code,
            ExitCode::Failed,
            "an unclassifiable endpoint must not exit 0: {}",
            outcome.stdout
        );
        let value: serde_json::Value =
            serde_json::from_str(&outcome.stdout).expect("single-line json");
        assert_eq!(value["kind"], LOCAL_KIND_UNKNOWN_AUTHENTICATED);
        assert_eq!(value["authenticated"], false);
        assert!(
            value["detail"].as_str().is_some_and(
                |detail| detail.contains("--model") && detail.contains("--api-key-env")
            ),
            "the detail must name the flags that supply a credential: {}",
            outcome.stdout
        );
    }

    /// One client for the whole process: the GUI memoizes `shared_probe_client`
    /// and the mirror rebuilt a `reqwest::blocking::Client` — pool, internal
    /// runtime thread and all — inside `get_json`, i.e. once per candidate
    /// request.
    #[test]
    fn probe_client_is_a_process_wide_singleton() {
        let first = probe_client().expect("probe client builds");
        let second = probe_client().expect("probe client builds");
        assert!(
            std::ptr::eq(first, second),
            "every caller must share one client, not rebuild one per request"
        );
    }

    /// The four API providers send a REAL request built from the product's
    /// own search endpoints and auth schemes
    /// (`CodeWhale/crates/tui/src/tools/web_search.rs`). Pinning the shapes
    /// here is what stops the lane from quietly becoming presence-only again:
    /// a probe that does not carry the key cannot validate it.
    #[test]
    fn search_api_requests_carry_the_key_for_every_api_provider() {
        let client = reqwest::blocking::Client::builder()
            .build()
            .expect("client builds");
        // Bing has no api-key form; it is routed to the scrape probe instead.
        assert!(search_api_request(&client, SearchProvider::Bing, "k").is_none());
        for (provider, host) in [
            (SearchProvider::Tavily, "api.tavily.com"),
            (SearchProvider::Bocha, "api.bochaai.com"),
            (SearchProvider::Metaso, "metaso.cn"),
            (SearchProvider::Baidu, "qianfan.baidubce.com"),
        ] {
            let request = search_api_request(&client, provider, "probe-key")
                .unwrap_or_else(|| panic!("{} must build a request", provider.as_str()))
                .build()
                .unwrap_or_else(|_| panic!("{} request must build", provider.as_str()));
            assert_eq!(request.method(), reqwest::Method::POST);
            assert_eq!(
                request.url().host_str(),
                Some(host),
                "{} must probe its documented endpoint",
                provider.as_str()
            );
            let body = request
                .body()
                .and_then(reqwest::blocking::Body::as_bytes)
                .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                .unwrap_or_default();
            let authorization = request
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            assert!(
                authorization.contains("probe-key") || body.contains("probe-key"),
                "{} probe must present the key (Tavily in the body, the rest as a bearer)",
                provider.as_str()
            );
            // Never both: duplicating a secret across header and body is how
            // one of the two ends up in a log.
            assert!(
                !(authorization.contains("probe-key") && body.contains("probe-key")),
                "{} must carry the key in exactly one place",
                provider.as_str()
            );
        }
    }

    #[test]
    fn loopback_guard_parses_ips_instead_of_prefix_matching() {
        for url in [
            "http://localhost:11434",
            "http://127.0.0.1:8080/v1",
            "http://[::1]:11434",
            "http://127.0.0.2:8000",
        ] {
            assert!(
                is_loopback_url(url, UrlOrigin::Argument).unwrap_or(false),
                "{url} is loopback"
            );
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
                !is_loopback_url(url, UrlOrigin::Argument).unwrap_or(false),
                "{url} is not loopback"
            );
        }
    }
}

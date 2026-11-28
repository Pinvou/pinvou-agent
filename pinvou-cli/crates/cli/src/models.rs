//! `models` + `settings` families (GUI parity). The `settings` top-level token
//! is an alias routed here by the dispatcher (values[0] == "settings"); parse
//! enforces that the alias only accepts settings subcommands while the
//! `models` token only accepts models subcommands.
//!
//! Pure-storage operations (everything except the connection probes) call the
//! same feature-layer building blocks the GUI commands in
//! `pinvou3-app/src-tauri/src/app/commands/settings.rs` use:
//! `pinvou3_lib::platform::prefs::UserPrefs` load -> mutate -> save via
//! `update_transaction` (cross-process flock + in-process lock + atomic
//! write + migrations + memory-locale policy — round-40 review M7 gave the
//! transaction an OS-level lock because this family made the CLI the second
//! writer process of settings.json) and
//! `pinvou3_lib::platform::credential_store` for API keys. No Tauri host is
//! booted.
//!
//! Three of the four network paths mirror a real GUI counterpart, and the
//! fourth has none — the distinction matters because it decides what the
//! command is allowed to claim:
//!
//! - `models test` mirrors `probe_model_connection`
//!   (app/commands/settings.rs), a real `GET {base}/models`.
//! - `models probe-local` mirrors `probe_local_server_kind`
//!   (app/commands/settings.rs) over `core::model_endpoint`, a real set of
//!   signature probes.
//! - `models discover` mirrors the GUI's endpoint model discovery
//!   (`probe_endpoint_models`, the frozen ladder over
//!   `core::endpoint_discovery`): the same per-protocol probing, listing and
//!   error classification, reimplemented on the blocking client. The GUI also
//!   back-fills missing lengths from its model-length catalog; the CLI has
//!   no catalog access, so every fact source stays `endpoint`.
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
//! reimplemented here on a blocking reqwest client (no async runtime). Live
//! endpoints are exercised by exactly three `#[ignore]` opt-in tests
//! (tests/models_contract.rs), and none of them cover this module's two
//! mirrored network commands: the Bing probe and the live-key search probe
//! exercise `settings search test`, and the third runs `models probe-local`
//! against a local inference server. `models test` and `models discover`
//! have NO `#[ignore]` live test — their network behavior is covered only by
//! the loopback HTTP mocks of the default tests, alongside the offline
//! parsing, classification and formatting units.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::time::Duration;

use pinvou3_lib::features::sessions::SerializableMode;
use pinvou3_lib::platform::credential_store::{
    CredentialReference, CredentialState, CredentialStore, SystemCredentialStore, redact_secret,
};
use pinvou3_lib::platform::prefs::{
    ColorScheme, CredentialStateOps, Language, MODEL_API_KEY_ENV_OVERRIDE,
    MODEL_PROVIDER_KIND_CODING_PLAN, MODEL_PROVIDER_KIND_CUSTOM, MODEL_PROVIDER_KIND_OFFICIAL_API,
    ModelPreset, SavedModel, SearchProvider, Theme, UserPrefs,
};

use crate::support::{
    collapse_control_characters, ensure_env_var_name, render, require_yes, resolve_secret, success,
};
use crate::{CliError, CliOutcome, ExitCode, OutputMode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelsCommand {
    List,
    Add(AddModel),
    Edit(EditModel),
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
    Discover {
        base_url: String,
        api_key_env: Option<String>,
        preset: Option<ModelPreset>,
        wire: Option<DiscoveryProtocol>,
        json: bool,
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
        yes: bool,
    },
    SearchTest {
        provider: SearchProvider,
    },
}

/// The `models add` payload: every flag value named once here, so the
/// command variant, the execute arm and the `add` worker pass one struct
/// instead of re-listing eleven fields positionally.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddModel {
    pub preset: ModelPreset,
    pub name: String,
    pub model: String,
    pub base_url: String,
    pub api_key_env: Option<String>,
    pub api_key_stdin: bool,
    pub context_window: Option<u32>,
    pub max_output: Option<u32>,
    pub reasoning_effort: Option<String>,
    pub metadata: ModelMetadata,
    pub set_active: bool,
}

/// The `models edit` payload: the target id, the parsed [`ModelEdit`] patch
/// and the credential / destructive-consent / activation flags, named once
/// instead of flowing through a nine-parameter worker signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditModel {
    pub id: String,
    pub changes: Box<ModelEdit>,
    pub api_key_env: Option<String>,
    pub api_key_stdin: bool,
    pub clear_api_key: bool,
    pub yes: bool,
    pub set_active: bool,
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

    /// The canonical settings.json spelling of this key, read off [`ALL`]
    /// (every variant is listed exactly once, so the lookup always hits).
    fn name(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(_, candidate)| *candidate == self)
            .map(|(name, _)| *name)
            .unwrap_or_default()
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
    /// `plan | yolo | auto | none` (`none` clears the Option; `auto` stores
    /// as the `*_auto` companion flag, never the legacy literal).
    Mode(Option<SerializableMode>),
    /// `true | false | none` (only `advanced.allow_shell`).
    OptionalBool(Option<bool>),
}

/// The CLI's accepted `models add/edit --preset` names. Deliberately
/// narrower than `ModelPreset::as_str`'s full set (`openai_responses` is a
/// GUI-only protocol value, not a `--preset`).
const MODEL_PRESETS: &[(&str, ModelPreset)] = &[
    ("local_vllm", ModelPreset::LocalVllm),
    ("deepseek", ModelPreset::Deepseek),
    ("kimi", ModelPreset::Kimi),
    ("openai_compatible", ModelPreset::OpenaiCompatible),
    ("qwen", ModelPreset::Qwen),
    ("doubao", ModelPreset::Doubao),
    ("minimax", ModelPreset::Minimax),
    ("glm", ModelPreset::Glm),
    ("mimo", ModelPreset::Mimo),
    ("openai", ModelPreset::Openai),
    ("anthropic", ModelPreset::Anthropic),
    ("gemini", ModelPreset::Gemini),
    ("xai", ModelPreset::Xai),
];

/// Round-44 review: every `--api-key-env` value is an environment-variable
/// NAME; the shared shape gate refuses a pasted literal secret at parse
/// time (usage, exit 2) instead of echoing it back from a later not-set
/// error.
fn gate_env_var(value: Option<&str>) -> Result<Option<String>, CliError> {
    match value {
        Some(value) => {
            ensure_env_var_name("--api-key-env", value)?;
            Ok(Some(value.to_owned()))
        }
        None => Ok(None),
    }
}

fn parse_preset(raw: &str) -> Result<ModelPreset, CliError> {
    match MODEL_PRESETS.iter().find(|(name, _)| *name == raw) {
        Some((_, preset)) => Ok(*preset),
        None => Err(CliError::usage(format!(
            "unknown preset {raw:?}; valid presets: {}",
            MODEL_PRESETS
                .iter()
                .map(|(name, _)| *name)
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
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
        // `auto` is a value the desktop legitimately holds (tri-mode). It is
        // only a SETTER marker here: the settings.json lane vocabulary never
        // stores the `auto` literal - the setter below stores the explicit
        // Auto choice as the `*_auto` companion flag with a None legacy
        // value, mirroring the desktop's `set_mode_default` pair contract.
        "auto" => Ok(Some(SerializableMode::Auto)),
        "yolo" => Ok(Some(SerializableMode::Yolo)),
        "none" => Ok(None),
        other => Err(CliError::usage(format!(
            "expected plan, yolo, auto or none, got {other:?}"
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

/// The wire protocol a `models discover` run speaks. Named after the GUI
/// command's `DiscoveryProtocol` (`core::endpoint_discovery`); the app-private
/// module is not exported through `pinvou3_lib`, so the CLI carries its own
/// equivalent — the variants and their preset/wire mapping are the shared
/// contract, pinned by tests on both sides.
/// `pub` for the same reason as `SettingsKey`: the payload type of a `pub`
/// command variant must not be more private than the variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiscoveryProtocol {
    /// OpenAI-compatible `GET {base}/models` (the default; also the ladder
    /// every loopback/private endpoint walks).
    OpenAiCompatible,
    /// Anthropic `/v1/models` with `x-api-key` + `anthropic-version`.
    Anthropic,
    /// Gemini native `GET {root}/v1beta/models` with `x-goog-api-key`.
    Gemini,
}

/// The frozen preset → protocol mapping (spec §2.1/§2.2): only the two
/// native-protocol presets divert; every OpenAI-family preset — including
/// `local_vllm` and the vendor endpoints (`glm` `/paas/v4`, Ark `/api/v3`,
/// gemini's OpenAI-compatible surface) — walks the OpenAI-compatible ladder
/// through `models_probe_url`, which never invents a `/v1` segment.
fn protocol_for_preset(preset: ModelPreset) -> DiscoveryProtocol {
    match preset {
        ModelPreset::Anthropic => DiscoveryProtocol::Anthropic,
        ModelPreset::Gemini => DiscoveryProtocol::Gemini,
        // OpenaiResponses exists only in the GUI's full preset set, not in
        // the CLI's `--preset` table; the catch-all keeps the mapping total
        // over the shared enum either way.
        _ => DiscoveryProtocol::OpenAiCompatible,
    }
}

/// The frozen wire → protocol mapping (the ACP provider wires). `responses`
/// joins the OpenAI-compatible family: the GUI's wire mapping treats the
/// responses wire (and everything unknown) as OpenAI-compatible, and an ACP
/// provider configured on that wire must reach the same ladder here. The CLI
/// stays STRICTER than the GUI about unknown wires on purpose — a typo on the
/// command line should be a usage error, not a silently different protocol.
fn protocol_for_wire(wire: &str) -> Option<DiscoveryProtocol> {
    match wire.trim().to_ascii_lowercase().as_str() {
        "anthropic" => Some(DiscoveryProtocol::Anthropic),
        "openai" | "kimi" | "responses" => Some(DiscoveryProtocol::OpenAiCompatible),
        _ => None,
    }
}

/// The `--wire` spellings `models discover` accepts, with their mapping.
fn parse_discover_wire(raw: &str) -> Result<DiscoveryProtocol, CliError> {
    protocol_for_wire(raw).ok_or_else(|| {
        CliError::usage(format!(
            "invalid wire {raw:?}; valid wires: anthropic, openai, kimi, responses"
        ))
    })
}

/// Which protocol one `models discover` run speaks: the preset wins when both
/// flags are given (the GUI command's own precedence — a preset names a full
/// provider configuration, a wire only its protocol family), and neither
/// means the OpenAI-compatible default.
fn resolve_discovery_protocol(
    preset: Option<ModelPreset>,
    wire: Option<DiscoveryProtocol>,
) -> DiscoveryProtocol {
    preset
        .map(protocol_for_preset)
        .or(wire)
        .unwrap_or(DiscoveryProtocol::OpenAiCompatible)
}

/// Typed value for one settings key; rejects type/value mismatches with a
/// usage error naming the accepted values.
fn parse_settings_value(key: SettingsKey, raw: &str) -> Result<SettingsValue, CliError> {
    let usage = |expected: &str| {
        CliError::usage(format!(
            "invalid value {raw:?} for {}; expected {expected}",
            key.name()
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
            .map_err(|_| usage("plan | yolo | auto | none")),
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
    let label = format!("pinvou {family} {subcommand}");
    let mut values = BTreeMap::new();
    let mut flags = Vec::new();
    let mut positionals = Vec::new();
    let mut index = 0;
    while index < rest.len() {
        let argument = &rest[index];
        if let Some(name) = argument.strip_prefix("--") {
            if value_flags.contains(&name) {
                // An empty value is a missing value, not a value that happens
                // to be empty: `--name "$UNSET"` is how the shell spells
                // "this argument was never computed". `support::parse_family_flags`
                // has always refused it; `models` did not, so `--name ""`
                // reached the store and `--api-key-env ""` reached
                // `std::env::var("")` — each failing later with a message
                // about the store or the environment rather than about the
                // command line. Refusing here keeps all three parsers
                // (support.rs, models.rs, memory.rs) on one contract.
                let value = rest
                    .get(index + 1)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| CliError::usage(format!("--{name} requires a value")))?;
                if value.starts_with("--") {
                    // `--api-key-env --set-active` would otherwise consume
                    // the flag name as its value and fail later with a
                    // confusing host error. The pasted token is NOT echoed
                    // (round-48 review): a user who pasted a literal secret
                    // into a value flag must not have it echoed to stderr
                    // by the module's own never-echo doctrine.
                    return Err(CliError::usage(format!(
                        "--{name} requires a value (got another flag, not a value)"
                    )));
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
                // An unknown `--flag=value` token (`--api-key=sk-...`, the
                // way a pasted secret arrives) must be reported by its FLAG
                // name only: everything from the `=` on is the caller's own
                // text, and echoing it back would write a secret into the
                // terminal — the same never-echo doctrine the value-flag and
                // stray-positional refusals above follow.
                let spelled = name.split_once('=').map(|(flag, _)| flag).unwrap_or(name);
                return Err(CliError::usage(format!(
                    "unknown option --{spelled} for {label}"
                )));
            }
        } else {
            positionals.push(argument.clone());
            index += 1;
        }
    }
    Ok(Options {
        label,
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
        self.exactly_one_positional_named("an id")
    }

    /// Same gate with an explicit noun phrase (article included): the
    /// search-test lane's positional is a provider name, and "takes one id"
    /// read as self-contradicting there.
    fn exactly_one_positional_named(&self, noun: &str) -> Result<String, CliError> {
        match self.positionals.len() {
            // `label` already names the subcommand ("pinvou models remove"),
            // so the message needs no extra noun.
            1 => Ok(self.positionals[0].clone()),
            0 => Err(CliError::usage(format!("{} requires {noun}", self.label))),
            n => Err(CliError::usage(format!(
                "{} takes just {noun}, got {n}",
                self.label
            ))),
        }
    }

    /// Option-only subcommands must not silently drop stray tokens
    /// (`settings search set junk --provider metaso` is a typo, not a
    /// value). The message reports the COUNT only, never the tokens: a
    /// positional here is usually a pasted secret meant for an interactive
    /// prompt, and echoing it back would write it into the terminal.
    fn reject_stray_positionals(&self) -> Result<(), CliError> {
        if self.positionals.is_empty() {
            Ok(())
        } else {
            Err(CliError::usage(format!(
                "{} takes no positional arguments (got {})",
                self.label,
                self.positionals.len()
            )))
        }
    }

    /// The count-free companion of [`Self::reject_stray_positionals`] for
    /// subcommands that take nothing but options.
    fn ensure_no_positionals(&self) -> Result<(), CliError> {
        if self.positionals.is_empty() {
            Ok(())
        } else {
            Err(CliError::usage(format!(
                "{} takes no arguments",
                self.label
            )))
        }
    }
}

const MODELS_USAGE: &str =
    "usage: pinvou models <list|add|edit|remove|use|show|test|probe-local|discover>";
/// `settings get` without a key is documented as always-JSON right in the
/// usage text. `--output` is a global flag, so it is accepted on every
/// subcommand and a reader could reasonably expect `--output human` to change
/// the whole-settings dump; it does not, because that payload is the nested
/// `UserPrefs` document and there is no meaningful `key = value` rendering of
/// it. Saying so here turns a flag that looks ignored into a stated contract.
const SETTINGS_USAGE: &str = "usage: pinvou settings <get|set|search>; search subcommands: \
list|set|test; note: `settings get` without a key always prints JSON regardless of --output";

/// The `models discover` flag surface and exit-code contract. It is embedded
/// ONLY in the two `--base-url` refusals (missing or malformed); the
/// `--wire`/`--preset` value errors carry just their own message naming the
/// valid values. The exit codes stay inside the CLI's documented 0/1/2
/// contract (`support::emit_report`): the three failure statuses share
/// exit 1 and are distinguished by the `status` field the command always
/// prints — a fourth exit code would break every script that branches on
/// the family-wide 0/1/2 table for a distinction the JSON row already
/// carries. The usage text also spells out the one exit-1 shape WITHOUT a
/// status field: a failed `--api-key-env` (unset / non-UTF-8 / empty) is an
/// `Err` the runner prints to stderr, leaving stdout empty, so a script
/// that only parses stdout knows there is nothing to parse.
const DISCOVER_USAGE: &str = "usage: pinvou models discover --base-url <url> [--api-key-env V] \
[--preset <name>] [--wire <anthropic|openai|kimi|responses>] [--json]\nexit codes: 0 listed (an \
empty list is still listed), 1 auth_required | unsupported | error (see the status field) or a \
failed --api-key-env (message on stderr, stdout empty — no status field to parse), 2 usage";

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
            options.ensure_no_positionals()?;
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
            options.ensure_no_positionals()?;
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
            let api_key_env = gate_env_var(options.value("api-key-env"))?;
            Ok(ModelsCommand::ProbeLocal {
                url,
                api_key_env,
                model_id,
            })
        }
        ("models", "discover") => {
            let options = parse_options(
                "models",
                "discover",
                rest,
                &["base-url", "api-key-env", "preset", "wire"],
                &["json"],
            )?;
            // Option-only subcommand: a stray positional here is usually a
            // pasted secret meant for a prompt, so the refusal reports the
            // COUNT only (the same rule `settings search set` applies) and
            // never echoes the tokens.
            options.reject_stray_positionals()?;
            // The one required flag gets the full usage block, like the
            // benchmark family's unknown-command error: everything else the
            // command accepts (and the exit-code table) is one line away.
            let base_url = match options.value("base-url") {
                Some(raw) => parse_text_value("base-url", raw, "empty")?,
                None => {
                    return Err(CliError::usage(format!(
                        "pinvou models discover requires --base-url <url>\n{DISCOVER_USAGE}"
                    )));
                }
            };
            let preset = opt_flag(&options, "preset", parse_preset)?;
            let wire = opt_flag(&options, "wire", parse_discover_wire)?;
            Ok(ModelsCommand::Discover {
                base_url,
                // Same credential entry as probe-local: the flag names an
                // environment VARIABLE, and `gate_env_var` refuses a pasted
                // literal secret at parse time (usage, exit 2, the value
                // never echoed) — a literal on argv rides the shell history
                // and the process list. The variable is read only when the
                // command runs; the keyless (empty-key) case is spelled by
                // omitting the flag, so `Some` here is always a non-empty
                // NAME (`parse_options` refuses an empty or `--` value).
                api_key_env: gate_env_var(options.value("api-key-env"))?,
                preset,
                wire,
                json: options.has("json"),
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

/// Trims a text flag value and refuses a whitespace-only one, reported at
/// parse time so it exits 2 with the flag named. `word` carries the noun the
/// message uses (`blank`, `empty`) so each call site keeps its exact former
/// wording: a free-form metadata value must not be stored as whitespace (the
/// prefs normalizer maps a blank `vendor` / `endpoint_mode` back to `None`,
/// so accepting one would report a write that the next save undoes), and an
/// identity field matches what `models add` rejects in `add()` ("--name,
/// --model and --base-url must not be empty").
fn parse_text_value(flag: &str, raw: &str, word: &str) -> Result<String, CliError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CliError::usage(format!("--{flag} must not be {word}")));
    }
    Ok(trimmed.to_owned())
}

/// Reads the five optional GUI-form metadata fields off a parsed option set.
fn parse_metadata(options: &Options) -> Result<ModelMetadata, CliError> {
    Ok(ModelMetadata {
        alias: opt_flag_text(options, "alias", "blank")?,
        provider_kind: opt_flag(options, "provider-kind", parse_provider_kind)?,
        vendor: opt_flag_text(options, "vendor", "blank")?,
        endpoint_mode: opt_flag_text(options, "endpoint-mode", "blank")?,
        vision_model_id: opt_flag_text(options, "vision-model-id", "blank")?,
    })
}

/// The value flags `models add` and `models edit` accept — the eight
/// identity/credential/sizing flags followed by the five GUI-form metadata
/// flags (`alias`, `provider-kind`, `vendor`, `endpoint-mode`,
/// `vision-model-id`) — so the two commands can never accept different
/// spellings of the same field.
const MODEL_VALUE_FLAGS: &[&str] = &[
    "preset",
    "name",
    "model",
    "base-url",
    "api-key-env",
    "context-window",
    "max-output",
    "reasoning-effort",
    "alias",
    "provider-kind",
    "vendor",
    "endpoint-mode",
    "vision-model-id",
];

fn parse_add(rest: &[String]) -> Result<ModelsCommand, CliError> {
    let options = parse_options(
        "models",
        "add",
        rest,
        MODEL_VALUE_FLAGS,
        &["api-key-stdin", "set-active"],
    )?;
    options.ensure_no_positionals()?;
    if options.value("api-key-env").is_some() && options.has("api-key-stdin") {
        return Err(CliError::usage(
            "use only one of --api-key-env or --api-key-stdin",
        ));
    }
    let context_window = opt_flag(&options, "context-window", |raw: &str| {
        parse_positive_u32("--context-window", raw)
    })?;
    let max_output = opt_flag(&options, "max-output", |raw: &str| {
        parse_positive_u32("--max-output", raw)
    })?;
    let reasoning_effort = opt_flag(&options, "reasoning-effort", parse_reasoning_effort)?;
    Ok(ModelsCommand::Add(AddModel {
        preset: parse_preset(options.required("preset")?)?,
        name: options.required("name")?.trim().to_owned(),
        model: options.required("model")?.trim().to_owned(),
        base_url: options.required("base-url")?.trim().to_owned(),
        api_key_env: gate_env_var(options.value("api-key-env"))?,
        api_key_stdin: options.has("api-key-stdin"),
        context_window,
        max_output,
        reasoning_effort,
        metadata: parse_metadata(&options)?,
        set_active: options.has("set-active"),
    }))
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

/// Reads one single-`Option` value flag: absent -> `None`, present ->
/// `Some(parse(raw)?)`. The non-clearable companion of [`parse_clearable`],
/// shared by `models add`, `parse_metadata` and the four identity fields of
/// `models edit`.
fn opt_flag<T>(
    options: &Options,
    flag: &str,
    parse: impl Fn(&str) -> Result<T, CliError>,
) -> Result<Option<T>, CliError> {
    match options.value(flag) {
        Some(raw) => parse(raw).map(Some),
        None => Ok(None),
    }
}

/// [`opt_flag`] over [`parse_text_value`]: the caller names the flag and the
/// noun its refusal message uses; building that closure here keeps every call
/// site to one line.
fn opt_flag_text(options: &Options, flag: &str, word: &str) -> Result<Option<String>, CliError> {
    opt_flag(options, flag, |raw: &str| parse_text_value(flag, raw, word))
}

/// The clearable companion of [`opt_flag_text`], for `models edit` fields the
/// literal `none` clears.
fn clearable_flag_text(
    options: &Options,
    flag: &str,
    word: &str,
) -> Result<Option<Option<String>>, CliError> {
    parse_clearable(options, flag, |raw: &str| parse_text_value(flag, raw, word))
}

fn parse_edit(rest: &[String]) -> Result<ModelsCommand, CliError> {
    let options = parse_options(
        "models",
        "edit",
        rest,
        MODEL_VALUE_FLAGS,
        &["api-key-stdin", "clear-api-key", "set-active", "yes"],
    )?;
    let id = options.exactly_one_positional()?;
    let api_key_env = gate_env_var(options.value("api-key-env"))?;
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
        preset: opt_flag(&options, "preset", parse_preset)?,
        // The three identity fields are not clearable: a model with no name,
        // wire model or base_url is not a model. `--name none` therefore
        // stores the literal, like `models add` would.
        name: opt_flag_text(&options, "name", "empty")?,
        model: opt_flag_text(&options, "model", "empty")?,
        base_url: opt_flag_text(&options, "base-url", "empty")?,
        context_window: parse_clearable(&options, "context-window", |raw: &str| {
            parse_positive_u32("--context-window", raw)
        })?,
        max_output: parse_clearable(&options, "max-output", |raw: &str| {
            parse_positive_u32("--max-output", raw)
        })?,
        reasoning_effort: parse_clearable(&options, "reasoning-effort", parse_reasoning_effort)?,
        alias: clearable_flag_text(&options, "alias", "blank")?,
        provider_kind: parse_clearable(&options, "provider-kind", parse_provider_kind)?,
        vendor: clearable_flag_text(&options, "vendor", "blank")?,
        endpoint_mode: clearable_flag_text(&options, "endpoint-mode", "blank")?,
        vision_model_id: clearable_flag_text(&options, "vision-model-id", "blank")?,
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
    // A model cannot be its own vision fallback (the GUI form drops
    // `visionModelId === id` before saving): the routing lookup would
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
    Ok(ModelsCommand::Edit(EditModel {
        id,
        changes: Box::new(changes),
        api_key_env,
        api_key_stdin,
        clear_api_key,
        yes: options.has("yes"),
        set_active: options.has("set-active"),
    }))
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
            options.ensure_no_positionals()?;
            Ok(ModelsCommand::SearchList)
        }
        "set" => {
            let options = parse_options(
                "settings",
                "search set",
                rest,
                &["provider", "api-key-env"],
                &["clear", "yes"],
            )?;
            options.reject_stray_positionals()?;
            if options.has("clear") && options.value("api-key-env").is_some() {
                return Err(CliError::usage("use only one of --api-key-env or --clear"));
            }
            Ok(ModelsCommand::SearchSet {
                provider: parse_search_provider(options.required("provider")?)?,
                api_key_env: gate_env_var(options.value("api-key-env"))?,
                clear: options.has("clear"),
                yes: options.has("yes"),
            })
        }
        "test" => {
            let options = parse_options("settings", "search test", rest, &[], &[])?;
            let provider = options.exactly_one_positional_named("a provider id")?;
            Ok(ModelsCommand::SearchTest {
                provider: parse_search_provider(&provider)?,
            })
        }
        _ => Err(CliError::usage(
            "usage: pinvou settings search <list|set|test>",
        )),
    }
}

/// The production entry point for `ModelsCommand`; testability lives on the
/// private generic command fns (`list`, `add`, `edit`, ...), which take the
/// credential store as a parameter, not on this entry point.
pub fn execute(command: ModelsCommand, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store = SystemCredentialStore::new();
    // Every subcommand reads or writes the settings store / credential store
    // under the product data root; a relative PINVOU3_HOME would silently
    // resolve against the cwd.
    crate::support::sandbox_home()?;
    let outcome = match command {
        ModelsCommand::List => list(&store, output),
        ModelsCommand::Add(spec) => add(&store, spec, output),
        ModelsCommand::Edit(spec) => edit(&store, spec, output),
        ModelsCommand::Remove { id, yes } => remove(&store, &id, yes, output),
        ModelsCommand::Use { id } => use_model(&id, output),
        ModelsCommand::Show { id, reveal_key } => show(&store, &id, reveal_key, output),
        ModelsCommand::Test { id } => test_connection(&store, &id, output),
        ModelsCommand::ProbeLocal {
            url,
            api_key_env,
            model_id,
        } => probe_local(
            &store,
            url.as_deref(),
            api_key_env.as_deref(),
            model_id.as_deref(),
            output,
        ),
        ModelsCommand::Discover {
            base_url,
            api_key_env,
            preset,
            wire,
            json,
        } => discover(
            &base_url,
            api_key_env.as_deref(),
            preset,
            wire,
            json,
            output,
        ),
        ModelsCommand::SettingsGet { key } => settings_get(&store, key, output),
        ModelsCommand::SettingsSet { key, value } => settings_set(key, value, output),
        ModelsCommand::SearchList => search_list(&store, output),
        ModelsCommand::SearchSet {
            provider,
            api_key_env,
            clear,
            yes,
        } => search_set(&store, provider, &api_key_env, clear, yes, output),
        ModelsCommand::SearchTest { provider } => search_test(&store, provider, output),
    };
    // Round-47 review: the store's keyring-failure fallback reports through
    // the `log` facade, which no CLI binary installs — so on a headless box
    // an API key could land in the plaintext file fallback with no terminal
    // notice at all. Surface it once per process, after the command (the
    // note writes stderr; stdout JSON stays parseable).
    let fallbacks = store.os_keyring_fallback_services();
    if !fallbacks.is_empty() {
        crate::note!(
            "pinvou: warning: the OS keyring was unreachable for {}; credential \
             storage used the plaintext file fallback on disk",
            fallbacks.join(", ")
        );
    }
    outcome
}

/// Read-latest-prefs the same way the GUI's `refresh_safe_prefs` does:
/// normalize metadata, re-read credential states from the credential store
/// and strip plaintext keys before anything is rendered. Models without a
/// `credential_ref` never touch the credential backend.
fn safe_prefs<S: CredentialStore>(store: &S) -> UserPrefs {
    let mut prefs = UserPrefs::load();
    prefs.normalize_saved_model_metadata();
    prefs.refresh_credential_states_with_store(store);
    prefs.sanitize_plaintext_api_keys();
    prefs
}

/// Plain prefs load for the probe lanes (`models test`, `probe-local`): their
/// rows render only ok/code/detail/http_status — no credential_state — so the
/// all-models keychain refresh `safe_prefs` performs (one `get` per stored
/// credential) buys nothing here; the one key they need is resolved
/// explicitly afterwards, exactly like the GUI twin's plain
/// `UserPrefs::load()` in `resolve_saved_model_key_honest` (round-39 review).
fn plain_prefs() -> UserPrefs {
    let mut prefs = UserPrefs::load();
    prefs.normalize_saved_model_metadata();
    prefs.sanitize_plaintext_api_keys();
    prefs
}

/// One `models list`/`show` JSON row. Field names are the GUI's
/// `ModelListItem` DTO (a flatten of `SavedModel`), so a script can read
/// back every field `models add`/`edit` accepts without maintaining a
/// second key map: `alias`/`provider_kind`/`vendor`/`endpoint_mode`/
/// `vision_model_id` are CLI-writable, and the token fields keep the
/// GUI's `*_tokens` names. Secret material stays out (`has_secret`/
/// `credential_state` only), and the GUI-only presentation fields
/// (`readonly`/`system`/`kind`, `image_capability_override`) stay
/// GUI-side. `auth_mode` is a posture flag (round-5 review), not secret
/// material: without it a declared record's credential-free probe
/// behavior has no visible cause.
fn model_entry_json(model: &SavedModel, active_id: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "id": model.id,
        "name": model.name,
        "alias": model.alias,
        "preset": model.preset.as_str(),
        "model": model.model,
        "base_url": model.base_url,
        "provider_kind": model.provider_kind,
        "vendor": model.vendor,
        "endpoint_mode": model.endpoint_mode,
        "auth_mode": model.auth_mode,
        "vision_model_id": model.vision_model_id,
        "context_window_tokens": model.context_window_tokens,
        "max_output_tokens": model.max_output_tokens,
        "reasoning_effort": model.reasoning_effort,
        "active": active_id == Some(model.id.as_str()),
        "has_secret": model.has_secret,
        "credential_state": model.credential_state,
    })
}

fn optional_u32(value: Option<u32>) -> String {
    value.map_or_else(|| "none".to_owned(), |tokens| tokens.to_string())
}

fn list<S: CredentialStore>(store: &S, output: OutputMode) -> Result<CliOutcome, CliError> {
    let prefs = safe_prefs(store);
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
/// `support::resolve_secret` trims both lanes (env and stdin), and this
/// write-side normalization is the belt-and-braces half: environment
/// secrets routinely carry a trailing newline into the process (a file
/// mounted by a CI secret store, `read KEY < key.txt`, most `.env`
/// loaders), and any lane that ever skips one of the two trims again
/// would make every signed request 401.
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
fn apply_new_model_credential<S: CredentialStore>(
    store: &S,
    mut model: SavedModel,
) -> Result<SavedModel, String> {
    let key = secret_for_storage(&model.api_key).to_owned();
    if key.is_empty() {
        model.mark_missing();
    } else {
        let reference = model.credential_reference();
        store
            .set(&reference, &key)
            .map_err(|error| error.user_message())?;
        model.mark_configured(reference);
    }
    Ok(model)
}

fn add<S: CredentialStore>(
    store: &S,
    spec: AddModel,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let AddModel {
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
    } = spec;
    if name.is_empty() || model.is_empty() || base_url.is_empty() {
        return Err(CliError::usage(
            "--name, --model and --base-url must not be empty",
        ));
    }
    // Resolved before any prefs mutation so a missing environment variable is
    // reported without side effects.
    let secret = resolve_secret(&api_key_env, api_key_stdin)?;
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
        // CLI-created entries use the default auth posture; keyless
        // declarations (`auth_mode = "none"`) are a settings-page concept.
        auth_mode: None,
        image_capability_override: Default::default(),
        vision_model_id: metadata.vision_model_id,
        api_key: secret.unwrap_or_default(),
        credential_ref: None,
        credential_state: CredentialState::Missing,
        has_secret: false,
        credential_action: None,
    };
    let active_id = id.clone();
    // Whether the keyring write actually happened inside the closure: a
    // failure BEFORE it (the vision-model gate below) must not delete a
    // reference nothing was stored under — a never-written reference errors
    // on most keyrings, and the module's other rollback lanes gate on
    // "written" for exactly that reason.
    let credential_written = std::sync::atomic::AtomicBool::new(false);
    let transaction = UserPrefs::update_transaction(|prefs| {
        require_known_vision_model(prefs, saved.vision_model_id.as_deref())?;
        let stored = apply_new_model_credential(store, saved.clone())
            .map_err(|error| format!("credential store unavailable: {error}"))?;
        credential_written.store(true, std::sync::atomic::Ordering::Release);
        prefs.upsert_model(stored);
        if set_active {
            prefs.advanced.active_model_id = Some(active_id.clone());
        }
        Ok(())
    });
    if let Err(error) = transaction {
        // The closure may have stored the keyring secret before the save
        // failed; roll it back so no orphaned entry outlives the model.
        if stores_secret && credential_written.load(std::sync::atomic::Ordering::Acquire) {
            let reference = saved.credential_reference();
            let _ = store.delete(&reference);
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

/// Single home for the failed-write keyring rollback `edit` and
/// `settings search set` share. The restore is narrowed to the value THIS
/// call wrote (`wrote`) — a peer rotation landing in the post-flock window
/// must not be rolled back to a stale secret, so if the reference no longer
/// holds what this call wrote (or the check cannot be done) the keyring is
/// left untouched. A pre-transaction read error means the old state is
/// unknown, and unknown leaves the keyring alone rather than destroy a
/// secret prefs may still reference (a stale orphaned entry is the benign
/// direction). `previous` being `None` means this call wrote nothing and
/// there is nothing to roll back. Residual (round-49 review): a peer write
/// landing between this post-flock re-read and the restoring set/delete is
/// the last unclosable sub-window without a keyring compare-and-swap.
fn rollback_keyring_write<S: CredentialStore>(
    store: &S,
    reference: &CredentialReference,
    wrote: Option<String>,
    previous: Option<Result<Option<String>, ()>>,
) {
    let peer_moved = match (
        wrote.as_deref(),
        store.get(reference).map_err(|e| e.user_message()),
    ) {
        (Some(written_value), Ok(current)) => current.as_deref() != Some(written_value),
        _ => true,
    };
    match previous {
        // A previous secret existed and the failed write destroyed it: put
        // it back — unless a peer moved the reference meanwhile.
        Some(Ok(Some(old))) if !peer_moved => {
            let _ = store.set(reference, old.as_str());
        }
        Some(Ok(Some(_))) => {}
        // Nothing was there before: remove what was just stored — unless a
        // peer re-pointed/rewrote the reference meanwhile (deleting would
        // destroy the peer's secret).
        Some(Ok(None)) if !peer_moved => {
            let _ = store.delete(reference);
        }
        Some(Ok(None)) => {}
        // Pre-transaction state unknown (keychain read failed): leave it
        // alone rather than delete a secret prefs may still reference.
        Some(Err(_)) => {}
        // No write happened (nothing to store), so no rollback.
        None => {}
    }
}

/// The post-commit hook body shared by the three deferred-delete lanes
/// (`models edit --clear-api-key`, `models remove`,
/// `settings search set --clear`): consume the reference the transaction
/// captured and delete its keyring entry, warning — without failing, since
/// the prefs record is already committed and a leftover keyring entry is the
/// benign direction — when the delete errors.
fn delete_after_commit<S: CredentialStore>(
    store: &S,
    reference: Option<CredentialReference>,
    warning: &str,
) {
    if let Some(reference) = reference
        && let Err(error) = store.delete(&reference)
    {
        note!("pinvou: warning: {warning}: {}", error.user_message());
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
///   an orphaned entry is the benign direction). The delete targets only the
///   model's stored `credential_ref`: a never-configured model has no keyring
///   entry, so there is nothing to delete (the same gate `models remove` and
///   `search set --clear` apply).
///   Round-50 review: the post-flock rollback is single-homed in
///   [`rollback_keyring_write`] — the two write lanes used to carry
///   line-for-line copies of the module's subtlest invariant, where a
///   one-line divergence would be invisible.
fn edit<S: CredentialStore>(
    store: &S,
    spec: EditModel,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let EditModel {
        id,
        changes,
        api_key_env,
        api_key_stdin,
        clear_api_key,
        yes,
        set_active,
    } = spec;
    // `--clear-api-key` deletes the stored credential (keyring entry and
    // configured bookkeeping), so it opts in explicitly exactly like
    // `models remove` (`require_yes`): destructive actions must not be
    // runnable by a mistyped command.
    if clear_api_key {
        require_yes(yes)?;
    }
    // The id is checked before any secret resolution: `--api-key-stdin`
    // BLOCKS on the pipe, and a mistyped id would consume the pasted key and
    // only then fail — the same pre-check ordering `code providers update`
    // applies to its unknown-id refusal. An unreadable prefs store defers to
    // the transaction below, which owns the real error surface for that.
    // `UserPrefs::load` persists normalization migrations, so this
    // pre-check can rewrite settings.json before the command fails — the
    // same disclosed trade-off the `remove` lane carries (idempotent
    // migrations only; round-40 review names it here too).
    if UserPrefs::load().model_by_id(&id).is_none() {
        return Err(CliError::failed(format!("model not found: {id}")));
    }
    // Resolved before any prefs mutation so a missing environment variable is
    // reported without side effects (same ordering as `add`).
    let secret = resolve_secret(&api_key_env, api_key_stdin)?;
    let replacement = secret.map(|raw| secret_for_storage(&raw).to_owned());
    // The deferred keyring delete travels from the mutate closure (which
    // discovers the reference) to the post-commit hook (which issues it)
    // through a `RefCell`: both closures are `FnOnce` over the same captured
    // environment. Round-49 review: the hook runs via
    // `update_transaction_with_post_commit` right after the save succeeds and
    // BEFORE the settings flock releases, so the delete is totally ordered
    // against a concurrent replacement of the same deterministic reference —
    // the same barrier the GUI's `save_model_inner` applies. A delete that
    // ran after the lock released could destroy a secret that replacement
    // had just committed. `cleared_stored_reference` keeps the
    // "was there actually something to clear" bit the `cleared`/`unchanged`
    // receipt below reports (the hook consumes the reference itself).
    let deferred_delete = std::cell::RefCell::new(Option::<CredentialReference>::None);
    let cleared_stored_reference = std::cell::Cell::new(false);
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
    let transaction = UserPrefs::update_transaction_with_post_commit(
        |prefs| {
            let Some(existing) = prefs.model_by_id(&id) else {
                return Err(format!("model not found: {id}"));
            };
            let mut updated = existing.clone();
            apply_model_edit(&mut updated, &changes);
            // Validate only the reference this edit WRITES: a pre-existing
            // dangling `vision_model_id` (the referenced model was removed —
            // `remove_model` does not cascade) must not fail an unrelated edit,
            // where the GUI's `save_model_inner` (this module's parity anchor)
            // has no such gate. A carried-through reference keeps the GUI's
            // behavior: the dead fallback silently never fires.
            if changes.vision_model_id.is_some() {
                require_known_vision_model(prefs, updated.vision_model_id.as_deref())?;
            }
            match (&replacement, clear_api_key) {
                // Replace: store first, then mark configured, all inside the
                // closure so the save that follows either commits both or neither.
                // The resolver's contract is a returned key is already trimmed
                // and non-empty, so Some is exactly "there is a key to store".
                (Some(key), _) => {
                    // An explicit replacement is the caller deliberately
                    // re-authenticating the endpoint, so a stale keyless
                    // declaration yields — the same explicit-key-outranks-
                    // declaration precedence the probe lanes document.
                    // Without this, `edit` is the one CLI writer that could
                    // persist the declared+configured dual state the GUI's
                    // `apply_model_credential` intercept exists to prevent
                    // (`add` hardcodes `auth_mode: None`; a hand-edited
                    // settings.json can carry the declaration). KeepExisting
                    // deliberately does NOT clear: an unrelated field edit
                    // must not silently mutate the credential posture, and
                    // that residual dual state stays inert under the
                    // read-side guards.
                    if updated.declares_keyless_auth() {
                        updated.auth_mode = None;
                    }
                    let reference = updated.credential_reference();
                    let previous = store.get(&reference).map_err(|error| error.user_message());
                    written = Some((reference.clone(), previous));
                    store
                        .set(&reference, key)
                        .map_err(|error| format!("credential store unavailable: {error}"))?;
                    updated.mark_configured(reference);
                }
                // Delete: the record is cleared now, the keyring entry in the
                // post-commit hook — gated on the model actually holding a
                // stored `credential_ref`. A never-configured model has no
                // keyring entry, and deleting the `model:{id}` reference
                // `credential_reference()` would synthesize anyway errors on
                // most keyrings — a spurious warning for a no-op. The same gate
                // `models remove` (its `credential_ref` lookup with no fallback)
                // and `search set --clear` (a never-configured provider has no
                // keyring entry) already apply; all three deletion lanes share
                // one contract.
                (_, true) => {
                    let reference = updated.credential_ref.clone();
                    cleared_stored_reference.set(reference.is_some());
                    *deferred_delete.borrow_mut() = reference;
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
        },
        // Runs only after `save_unlocked` succeeded, still under the prefs
        // lock; a failed commit never reaches it, so the secret survives a
        // failed clear (the ordering the unit lane pins).
        || {
            delete_after_commit(
                store,
                deferred_delete.borrow_mut().take(),
                &format!(
                    "model {id} api key cleared from settings, but its keyring \
                     entry could not be deleted"
                ),
            );
        },
    );
    if let Err(error) = transaction {
        if let Some((reference, previous)) = written {
            // (The Replace arm only writes non-empty values, so `written`
            // being Some implies `replacement` is Some and holds exactly
            // what was stored.)
            rollback_keyring_write(
                store,
                &reference,
                replacement.clone(),
                Some(previous.map_err(|_| ())),
            );
        }
        return Err(prefs_error(error));
    }
    let had_stored_reference = cleared_stored_reference.get();
    let credential = if replacement.is_some() {
        "replaced"
    } else if clear_api_key {
        // Round-42 review: mirror `settings search set --clear`'s doctrine —
        // a model with no stored credential reference has nothing to clear,
        // and reporting "cleared" was a success-shaped no-op. `cleared` now
        // means the stored reference was actually removed.
        if had_stored_reference {
            "cleared"
        } else {
            "unchanged"
        }
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

fn remove<S: CredentialStore>(
    store: &S,
    id: &str,
    yes: bool,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    require_yes(yes)?;
    // Structural min-1 classification: load the prefs through the same
    // `UserPrefs::load` path the transaction uses and reject up front, so
    // the usage exit code does not depend on string-matching the
    // transaction error below (which stays as a backstop). A plain load —
    // not `safe_prefs` — keeps the check free of credential-store refreshes.
    // Disclosed side effect (same one the `settings set` comment at
    // `settings_set` documents): `load()` is the persisting variant, so this
    // read-classified classification can run the (idempotent) migrations and
    // rewrite settings.json before its verdict. The prefs layer exposes no
    // read-only load that skips the migrations, only one that skips the
    // save — so the honest state is this disclosure, not a pretense.
    let prefs = UserPrefs::load();
    if prefs.model_by_id(id).is_some() && prefs.advanced.saved_models.len() <= 1 {
        return Err(CliError::usage(REMOVE_LAST_MODEL_MESSAGE));
    }
    // The keyring delete runs in the post-commit hook, still inside the
    // settings-flock critical section (`update_transaction_with_post_commit`):
    // deleting first left a save failure with a model that is still
    // configured but secretless, and deleting after the lock released would
    // let a concurrent replacement of the same deterministic reference commit
    // a fresh secret that the stale delete then destroys — the peer-clobber
    // window the GUI's `delete_model_inner` already closed. A secret left
    // behind by a failed delete is the benign direction: the prefs record no
    // longer references it. The reference travels from the mutate closure to
    // the hook through a `RefCell` because both closures are `FnOnce` over
    // the same captured environment.
    let deferred_delete = std::cell::RefCell::new(Option::<CredentialReference>::None);
    UserPrefs::update_transaction_with_post_commit(
        |prefs| {
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
                *deferred_delete.borrow_mut() = Some(reference);
            }
            prefs.remove_model(id);
            Ok(())
        },
        || {
            delete_after_commit(
                store,
                deferred_delete.borrow_mut().take(),
                &format!("model {id} removed, but its keyring secret could not be deleted"),
            );
        },
    )
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
        // Unknown ids exit 1 like every other family — see `prefs_error`.
        .ok_or_else(|| CliError::failed(format!("model not found: {id}")))
}

/// Resolves a stored credential exactly like the GUI's
/// `resolve_saved_model_key`: read the reference out of the saved model and
/// fetch it from the credential store. A model without `credential_ref` has
/// no stored secret.
///
/// Round-48 review: under the file-fallback valves, a `get` miss cannot
/// distinguish "never stored" from "stored in the OS keyring this process
/// cannot reach" — the `CredentialStore` trait doc prescribes that
/// secret-classification callers consult [`CredentialStore::os_keyring_unreachable`]
/// for exactly this. Returns `Err("credential_unavailable: …")` when the
/// miss is actually an unreachable-keyring situation, so a caller can avoid
/// reporting a false `credential_state: missing` / `api_key: null` a script
/// might act on (e.g. conclude the key is gone and overwrite it).
fn resolve_saved_model_key_honest<S: CredentialStore>(
    store: &S,
    model: &SavedModel,
) -> Result<Option<String>, String> {
    let Some(reference) = &model.credential_ref else {
        return Ok(None);
    };
    let resolved = store.get(reference).map_err(|error| error.user_message())?;
    honest_reference_get(store, reference, resolved, || {
        format!(
            "the OS keyring holding {} is unreachable from this process; the stored key cannot be revealed or verified here",
            reference.account
        )
    })
}

/// The honest-miss rule [`resolve_saved_model_key_honest`] owns and the
/// search lane shares (round-50 review): under the file-fallback valves a
/// `get` miss cannot distinguish "never stored" from "stored in an OS
/// keyring this process cannot reach", so a miss on a reference the keyring
/// holds but cannot serve must ERROR with the lane's own wording instead of
/// answering a false `None` a script might act on. `value` is the lane's
/// already-fetched (and, on the search lane, trimmed) `get` result — the
/// rule fires only on a miss, and each lane supplies its exact
/// unreachable-keyring message (the model key can still be revealed from a
/// process that reaches the keyring; the search key only verified).
fn honest_reference_get<S: CredentialStore>(
    store: &S,
    reference: &CredentialReference,
    value: Option<String>,
    unreachable_message: impl FnOnce() -> String,
) -> Result<Option<String>, String> {
    if value.is_none() && store.os_keyring_unreachable(reference) {
        return Err(unreachable_message());
    }
    Ok(value)
}

/// The one `credential_unavailable` mapping every honest-resolver caller in
/// this module applies: an unreachable OS keyring is a host failure (exit 1)
/// naming the credential, never a silent "no stored key" the caller would
/// then misreport or the endpoint would 401 into a misleading
/// classification.
fn resolve_honest_saved_key<S: CredentialStore>(
    store: &S,
    model: &SavedModel,
) -> Result<Option<String>, CliError> {
    resolve_saved_model_key_honest(store, model)
        .map_err(|error| CliError::failed(format!("credential_unavailable: {error}")))
}

fn show<S: CredentialStore>(
    store: &S,
    id: &str,
    reveal_key: bool,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let prefs = safe_prefs(store);
    let model = find_model(&prefs, id)?;
    let active_id = prefs.active_model().map(|active| active.id.as_str());
    // The only path where a secret value is ever printed; --reveal-key mirrors
    // the GUI "reveal key" action (environment-overridden credentials are not
    // echoed, matching reveal_model_api_key).
    let revealed = if reveal_key {
        if model.credential_state == CredentialState::EnvOverride {
            None
        } else {
            // Round-48: the honest resolver refuses to answer `None`
            // when the OS keyring is merely unreachable — a false
            // "not stored" here is three false statements in one block
            // (`credential_state: missing`, `api_key: null`,
            // `api_key_source: none`).
            Some(resolve_honest_saved_key(store, &model)?)
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
    // Round-40 review: the five writable metadata fields `model_entry_json`
    // carries and `models add`/`edit` accept (`vendor` drives
    // reasoning-protocol routing) were absent from the human block, so a
    // `models edit --vendor glm` gave the user no way to confirm the write
    // without `--output json` — against the same-facts claim. Rendered
    // only when set, after the fixed fields and before the reveal block,
    // so `api_key_source`/`api_key` stay the last lines.
    for (field, value) in [
        ("alias", model.alias.as_deref()),
        ("provider_kind", model.provider_kind.as_deref()),
        ("vendor", model.vendor.as_deref()),
        ("endpoint_mode", model.endpoint_mode.as_deref()),
        ("auth_mode", model.auth_mode.as_deref()),
        ("vision_model_id", model.vision_model_id.as_deref()),
    ] {
        if let Some(value) = value {
            human.push_str(&format!(
                "\n{field}: {}",
                collapse_control_characters(value)
            ));
        }
    }
    if reveal_key {
        let value = match revealed.flatten() {
            Some(key) => key,
            // Not collapsed: this is a literal placeholder, not stored data.
            // The variable name comes from the same const the env-override
            // check reads (`refresh_credential_states_with_store`), so the
            // wording cannot drift from what actually classifies the model.
            None if key_source == "environment" => format!(
                "(supplied by the {MODEL_API_KEY_ENV_OVERRIDE} environment override; not echoed)"
            ),
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

fn connection_http_result(status: reqwest::StatusCode) -> ConnectionProbe {
    // The status ladder is the app's shared one (`model_probe`
    // facade): both surfaces classify from the same table.
    let code = pinvou3_lib::model_probe::model_probe_http_code(status);
    ConnectionProbe {
        ok: status.is_success(),
        code,
        detail: Some(format!("HTTP {}", status.as_u16())),
        http_status: Some(status.as_u16()),
    }
}

fn connection_error_result(error: &reqwest::Error) -> ConnectionProbe {
    // Round-43 review posture, now enforced in one place: reqwest's Display
    // embeds the full URL, userinfo included, and `redact_secret`'s generic
    // heuristic needs a digit — the shared ladder
    // (`model_probe::model_probe_transport_code`) applies the two-pass scrub
    // before the keyword match, so the CLI lane and the GUI lane redact
    // identically by construction.
    let (code, raw) = pinvou3_lib::model_probe::model_probe_transport_code(error);
    ConnectionProbe {
        ok: false,
        code,
        detail: Some(raw),
        http_status: None,
    }
}

/// `models test <id>`: GET `{base_url}/models` with the saved credential
/// (8s timeout; Anthropic official endpoints authenticate with x-api-key +
/// anthropic-version). Request shape and auth split match the GUI "test
/// connection" button exactly; the GUI-only part NOT reproduced is the
/// success-path FACT ENRICHMENT (the GUI attaches the matched entry's
/// context/output facts + fact_model to the result so its form can
/// prefill — round-5 review m1: previously disclosed nowhere while the
/// wording claimed "exactly").
fn test_connection<S: CredentialStore>(
    store: &S,
    id: &str,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let prefs = plain_prefs();
    let model = find_model(&prefs, id)?;
    // A keychain failure is a probe RESULT, not a crash: like every other
    // `models test` outcome it renders a single-line JSON row on stdout
    // (with `credential_unavailable`), so scripts can branch on it instead
    // of parsing stderr text. Parity by design: the GUI's connection test
    // resolves the stored key identically, so env-overridden keys (the
    // `MODEL_API_KEY_ENV_OVERRIDE` variable) are not honored here either.
    // Round-49 review: the resolution is the HONEST variant — under the
    // file-fallback valves a plain miss cannot distinguish "never stored"
    // from "stored in the OS keyring this process cannot reach", and the
    // latter must surface as `credential_unavailable`, not masquerade as an
    // unauthenticated probe that the endpoint then 401s into `auth_invalid`
    // (the same mapping `show --reveal-key` applies).
    // Keyless parity (GUI probe ladder): a record that itself declares the
    // endpoint keyless must probe credential-free even with a residual
    // stored credential (hand-edited settings.json can carry both) — the
    // CLI's test must exercise the posture the engine will reproduce.
    let key = if model.declares_keyless_auth() {
        String::new()
    } else {
        match resolve_saved_model_key_honest(store, &model) {
            Ok(key) => key.unwrap_or_default(),
            Err(error) => {
                let probe = ConnectionProbe {
                    ok: false,
                    code: "credential_unavailable",
                    detail: Some(error.to_string()),
                    http_status: None,
                };
                return Ok(render_probe_outcome(&probe, output));
            }
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
    let parsed_url =
        match reqwest::Url::parse(&pinvou3_lib::model_probe::models_probe_url(base_url)) {
            Ok(url) => url,
            Err(error) => {
                return ConnectionProbe {
                    ok: false,
                    code: "invalid_url",
                    detail: Some(error.to_string()),
                    http_status: None,
                };
            }
        };
    let Some(client) = connection_client() else {
        return ConnectionProbe {
            ok: false,
            code: "client_error",
            detail: None,
            http_status: None,
        };
    };
    match connection_probe_request(client, &parsed_url, base_url, key).send() {
        Ok(response) => connection_http_result(response.status()),
        Err(error) => connection_error_result(&error),
    }
}

/// The `models test` request, factored out of [`run_connection_probe`] so the
/// header contract is inspectable without a network (the unit test builds the
/// request and reads its headers; nothing is sent).
///
/// The Go gateway (`/zen/go/v1`) has rejected auxiliary requests without the
/// `x-opencode-session` affinity header with 400 `MissingSessionID` since
/// 2026-09; the GUI probe this lane mirrors attaches it
/// (`with_opencode_session_header(req, base_url, "connection-test")`), so the
/// CLI probe must too — with the SAME conversation key and the same
/// deterministic derivation, so both surfaces send the same value for the
/// same endpoint, not merely a well-formed one. The app helper itself takes
/// the async `reqwest::RequestBuilder`, which the blocking client here
/// cannot consume; the gate + id functions are the shared surface (re-exported
/// through `pinvou3_lib::model_probe`).
fn connection_probe_request(
    client: &'static reqwest::blocking::Client,
    parsed_url: &reqwest::Url,
    base_url: &str,
    key: &str,
) -> reqwest::blocking::RequestBuilder {
    let mut request = client.get(parsed_url.clone());
    if !key.trim().is_empty() {
        request = if pinvou3_lib::model_probe::is_anthropic_api_url(parsed_url) {
            request
                .header("x-api-key", key.trim())
                .header("anthropic-version", DISCOVER_ANTHROPIC_VERSION)
        } else {
            request.bearer_auth(key.trim())
        };
    }
    if pinvou3_lib::model_probe::is_opencode_gateway_base_url(base_url) {
        request = request.header(
            "x-opencode-session",
            pinvou3_lib::model_probe::opencode_session_id_for("connection-test"),
        );
    }
    request
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
/// are reached at the API root. The rule lives in the app's
/// `model_probe::strip_v1_suffix` (re-exported through the round-41 facade
/// widening); this wrapper only flattens its `Option<String>` (always
/// `Some` by construction) so the local call sites keep their `String`
/// shape.
fn strip_v1_suffix(url: &str) -> String {
    pinvou3_lib::model_probe::strip_v1_suffix(url).unwrap_or_default()
}

/// One process-wide blocking client per probe lane, mirroring how the GUI
/// reuses its `shared_probe_client`. Every network lane in this module runs
/// through one of these ([`probe_client`], [`connection_client`],
/// [`search_probe_client`]); constructing a `reqwest::blocking::Client` per
/// call — as the sequential mirror did on `models test` and `settings search
/// test` — threw away the connection pool AND spun up a new internal runtime
/// thread for every request. The lanes differ only in timeout (and, for the
/// local-server probes, redirects); a `OnceLock` per lane keeps each client
/// (and its keep-alive pool) for the whole process, so the concurrent
/// candidates in [`probe_candidates`] share one client safely
/// (`reqwest::blocking::Client` is `Send + Sync`).
fn singleton_client(
    cell: &'static std::sync::OnceLock<Option<reqwest::blocking::Client>>,
    configure: impl FnOnce(reqwest::blocking::ClientBuilder) -> reqwest::blocking::ClientBuilder,
) -> Option<&'static reqwest::blocking::Client> {
    cell.get_or_init(|| configure(reqwest::blocking::Client::builder()).build().ok())
        .as_ref()
}

/// The local-server probe client (`models probe-local`), used from inside
/// `get_json` by up to eight concurrent candidate requests.
fn probe_client() -> Option<&'static reqwest::blocking::Client> {
    static CLIENT: std::sync::OnceLock<Option<reqwest::blocking::Client>> =
        std::sync::OnceLock::new();
    singleton_client(&CLIENT, |builder| {
        builder
            .timeout(Duration::from_secs(3))
            // Local model servers have no business redirecting; following
            // a 302 would let a loopback service turn the probe into an
            // arbitrary remote request.
            .redirect(reqwest::redirect::Policy::none())
    })
}

/// The model-connection client (`models test`): 8 s timeout, matching the
/// GUI's connection-test button, and redirects DISABLED — the same policy
/// the GUI's `probe_model_connection` pins (this PR's round-3 review).
/// The lane carries `x-api-key` + `anthropic-version` for anthropic
/// official hosts, and reqwest strips only the Authorization/Cookie family
/// on a cross-origin redirect, so a followed 302 would re-send those
/// custom headers to a host the user never named. With redirects off, a
/// 3xx arrives as the final response and classifies through the shared
/// status ladder ([`connection_http_result`] → `model_probe_http_code`,
/// code `redirect`) — status handling unchanged, one hop fewer.
fn connection_client() -> Option<&'static reqwest::blocking::Client> {
    static CLIENT: std::sync::OnceLock<Option<reqwest::blocking::Client>> =
        std::sync::OnceLock::new();
    singleton_client(&CLIENT, |builder| {
        builder
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
    })
}

/// The `models discover` client: the connection client's 8 s timeout PLUS
/// redirects disabled. Discovery sends carry user credentials in custom
/// headers (`x-api-key`, `x-goog-api-key`), and reqwest strips only the
/// Authorization/Cookie family on a cross-origin redirect — following one
/// would re-send those headers to a host the user never named (the reason
/// [`probe_client`] pins `Policy::none()` for the local probes and
/// [`connection_client`] for `models test`; the GUI pins the same policy
/// on both its counterparts). A one-shot listing has no business following
/// a 302 anyway: with redirects off, a 3xx response arrives as-is and
/// classifies as an ordinary `error` row with its status kept
/// ([`discovery_status_listing`]). Deliberate divergence, round-5 review
/// m1 disclosure: the GUI's LOCAL lanes ride its shared probe client,
/// which FOLLOWS redirects on a 3s deadline — this surface is stricter
/// everywhere (no-redirect, 3s kind probes / 8s native listings), so a
/// redirecting loopback shim classifies differently per surface by
/// design (a 302 lists via the follow on the GUI and falls back here).
fn discovery_client() -> Option<&'static reqwest::blocking::Client> {
    static CLIENT: std::sync::OnceLock<Option<reqwest::blocking::Client>> =
        std::sync::OnceLock::new();
    singleton_client(&CLIENT, |builder| {
        builder
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
    })
}

/// The `settings search test` client: [`SEARCH_PROBE_TIMEOUT`] (15 s, the
/// product's own search timeout) — the 8 s of `models test` is too tight for
/// these endpoints, and a premature `timeout` on a perfectly good key would
/// be a false negative.
fn search_probe_client() -> Option<&'static reqwest::blocking::Client> {
    static CLIENT: std::sync::OnceLock<Option<reqwest::blocking::Client>> =
        std::sync::OnceLock::new();
    singleton_client(&CLIENT, |builder| builder.timeout(SEARCH_PROBE_TIMEOUT))
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

/// Generous cap on one probe/model-list/search response body: the timeout
/// bounds time, but a hostile loopback endpoint can stream within it — the
/// same bounded-IO rule every other CLI read follows.
const PROBE_BODY_CAP_BYTES: usize = 4 * 1024 * 1024;

/// The outcome of one capped body read: the bytes, or WHICH bounded-IO rule
/// failed — the search lane reports a read error (`response_unreadable`)
/// differently from an over-cap body (`response_too_large`), while the JSON
/// reader and the Bing shape check degrade both to "no usable body".
/// `UnreadableTimeout` (round-3 review minor) splits the timeout case out of
/// `Unreadable`: the discovery ladder reports a mid-body STALL as transport
/// `timeout` (the GUI's `BodyReadFail::MidStreamTimeout`), a distinct
/// retry-hint voice from a plain proxy cut (`network`); every other lane
/// folds the two back together.
enum BodyRead {
    Bytes(Vec<u8>),
    Unreadable,
    UnreadableTimeout,
    TooLarge,
}

/// Whether one blocking-read io error carries a reqwest TIMEOUT in its
/// source chain (the blocking `Read` impl wraps the reqwest error as the
/// io error's source). URL-free: only structured flags are read.
fn body_read_is_timeout(error: &std::io::Error) -> bool {
    use std::error::Error as _;
    let mut source = error.source();
    while let Some(err) = source {
        if let Some(reqwest_error) = err.downcast_ref::<reqwest::Error>() {
            return reqwest_error.is_timeout();
        }
        // The blocking reader's client deadline surfaces as an OPAQUE
        // hyper-util timeout: kind Other, not downcast-able to
        // reqwest::Error or io::Error, its Display exactly the canonical
        // std TimedOut phrase. Exact-equality is the only honest signal
        // here (found by the round-5 stall test: the walk above never
        // matched a real mid-body stall, so the timeout split was
        // unreachable in practice and every stall read as a plain
        // network cut) — a substring scan would repeat the URL-bias
        // mistake the TLS arm fixed, exact equality cannot fire on
        // arbitrary embedded text.
        if err.to_string() == "operation timed out" {
            return true;
        }
        source = err.source();
    }
    false
}

/// Reads one probe response body under `cap` — the bounded-IO rule every
/// network read in this module follows: the timeout bounds time, but a
/// hostile endpoint can stream within it. The cap is a parameter because
/// the lanes legitimately differ: probes and search ride
/// [`PROBE_BODY_CAP_BYTES`], while the discovery listing read uses
/// [`DISCOVER_BODY_CAP_BYTES`], mirroring the GUI's listing cap.
fn read_body_capped(response: reqwest::blocking::Response, cap: usize) -> BodyRead {
    let mut bytes = Vec::new();
    if let Err(error) = response.take(cap as u64 + 1).read_to_end(&mut bytes) {
        // Timeout split (round-3 review minor): a mid-body stall is a
        // different retry hint than a proxy cut; see `BodyRead`.
        if body_read_is_timeout(&error) {
            return BodyRead::UnreadableTimeout;
        }
        return BodyRead::Unreadable;
    }
    if bytes.len() > cap {
        return BodyRead::TooLarge;
    }
    BodyRead::Bytes(bytes)
}

/// Reads and parses a probe response body under [`PROBE_BODY_CAP_BYTES`];
/// an over-cap or non-JSON body degrades to `None` like any other probe
/// failure.
fn read_json_capped(response: reqwest::blocking::Response) -> Option<serde_json::Value> {
    match read_body_capped(response, PROBE_BODY_CAP_BYTES) {
        BodyRead::Bytes(bytes) => serde_json::from_slice(&bytes).ok(),
        BodyRead::Unreadable | BodyRead::UnreadableTimeout | BodyRead::TooLarge => None,
    }
}

fn get_json(url: &str, bearer: Option<&str>) -> Option<serde_json::Value> {
    let response = apply_bearer(probe_client()?.get(url), bearer)
        .send()
        .ok()?
        .error_for_status()
        .ok()?;
    read_json_capped(response)
}

/// Shared shape of the Ollama and LM Studio signature probes: `GET
/// {base}{path}` (after the `/v1` strip) must answer with a JSON array under
/// `key` holding at least one entry whose `field` is a non-blank string.
fn probe_non_empty_str_field(
    base_url: &str,
    bearer: Option<&str>,
    path: &str,
    key: &str,
    field: &str,
) -> bool {
    get_json(&format!("{}{path}", strip_v1_suffix(base_url)), bearer).is_some_and(|value| {
        value
            .get(key)
            .and_then(serde_json::Value::as_array)
            .is_some_and(|models| {
                models.iter().any(|item| {
                    item.get(field)
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|value| !value.trim().is_empty())
                })
            })
    })
}

fn probe_ollama_tags(base_url: &str, bearer: Option<&str>) -> bool {
    probe_non_empty_str_field(base_url, bearer, "/api/tags", "models", "name")
}

fn probe_lmstudio_v0(base_url: &str, bearer: Option<&str>) -> bool {
    probe_non_empty_str_field(base_url, bearer, "/api/v0/models", "data", "id")
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
    // The gateway affinity header rides both legs (round-5 review m1: the
    // GUI's fetch_v1_models attaches it and this mirror's doc claimed
    // "exactly" — the header was missing, so a gateway-owned loopback base
    // lost the owned_by evidence and degraded tachyon/vllm/lmdeploy kinds
    // to generic where the GUI classified them).
    let Ok(response) =
        with_discovery_affinity(apply_bearer(client.get(&primary), bearer), base_url).send()
    else {
        return V1ModelsProbe::Miss;
    };
    let response = match (response.status().as_u16(), fallback) {
        (200..=299, _) => response,
        (404 | 405, Some(url)) => {
            let Ok(response) =
                with_discovery_affinity(apply_bearer(client.get(&url), bearer), base_url).send()
            else {
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

/// The companion absence-of-a-classification: a probe thread PANICKED, so
/// the round is incomplete and `generic` would be an answer the probe never
/// earned (the auth sentinel's doctrine, one fault class over).
const LOCAL_KIND_PROBE_FAILED: &str = "probe_failed";

/// One round of candidate probes, as facts rather than as a decision.
/// Mirrors `core/model_endpoint.rs`'s `ProbeCandidateHits` so the priority
/// rule below can be exercised without a network.
#[derive(Debug, Default, PartialEq, Eq)]
struct ProbeCandidateHits {
    docker_mgmt_shape: bool,
    ollama: bool,
    lmstudio_v0: bool,
    koboldcpp: bool,
    llamacpp: bool,
    sglang: bool,
    v1_models: V1ModelsProbe,
    /// A probe thread panicked: the round did not complete, so a no-hit
    /// decision is not a `generic` classification, it is an incomplete one.
    panicked: bool,
}

/// Joins one probe thread from [`probe_candidates`]'s fan-out: `Ok` yields
/// the candidate's hit; a panicked thread degrades to "no hit" (the
/// candidate type's `Default`) and records the panic, so the round stays
/// best-effort and an all-miss round with a dead probe can still report
/// `probe_failed` instead of `generic`. Only the identical join arms are
/// shared — the spawn topology is untouched.
fn join_probe<T: Default>(handle: std::thread::ScopedJoinHandle<'_, T>, panicked: &mut bool) -> T {
    match handle.join() {
        Ok(hit) => hit,
        Err(_) => {
            *panicked = true;
            T::default()
        }
    }
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
        // candidate's failure must not lose the other six. The panic itself
        // is still a fact the decision owes the caller (`panicked`), so an
        // all-miss round with a dead probe reports `probe_failed`, not
        // `generic`.
        let mut panicked = false;
        let docker_mgmt_shape = join_probe(docker, &mut panicked);
        let ollama = join_probe(ollama, &mut panicked);
        let lmstudio_v0 = join_probe(lmstudio, &mut panicked);
        let koboldcpp = join_probe(koboldcpp, &mut panicked);
        let llamacpp = join_probe(llamacpp, &mut panicked);
        let sglang = join_probe(sglang, &mut panicked);
        let v1_models = join_probe(v1_models, &mut panicked);
        ProbeCandidateHits {
            docker_mgmt_shape,
            ollama,
            lmstudio_v0,
            koboldcpp,
            llamacpp,
            sglang,
            v1_models,
            panicked,
        }
    })
}

/// The signature-exclusivity priority of the GUI's `select_local_server_kind`
/// (demoted-Ollama order, GUI #686): DMR port gate > LM Studio > KoboldCpp >
/// llama.cpp > Tachyon > SGLang > LMDeploy > vLLM > Ollama > generic. Pure, so
/// the order is pinned by a test instead of by a live server. Tachyon sits
/// BEFORE SGLang (the GUI's own placement): new Tachyon packages also expose
/// `/get_server_info` with a version field, which satisfies SGLang's loose
/// signature — `owned_by == "tachyon"` is the stronger identity evidence.
///
/// Ollama is deliberately LAST among the specific signatures: `/api/tags` is
/// the weakest signature — wrapped deployments (model-store containers, DMR)
/// expose an Ollama-compatible shim on top of the real engine, and a
/// first-place Ollama branch would shadow every one of them (a sglang/vLLM
/// deployment behind such a shim would be misrouted onto the ollama wire). A
/// genuine Ollama server answers none of the stronger signatures, so the
/// demotion does not change its classification.
fn select_local_server_kind_from_hits(hits: &ProbeCandidateHits) -> &'static str {
    if hits.docker_mgmt_shape {
        return "dockermodelrunner";
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
    // Tachyon BEFORE SGLang (the GUI's ordering): a Tachyon server's
    // /get_server_info satisfies SGLang's loose signature, so deciding
    // sglang first would mislabel every new Tachyon package. The same
    // shared /v1/models body the lmdeploy/vllm arms read below is fetched
    // once by the candidate round.
    if let Some(v1_models) = hits.v1_models.body()
        && v1_models_owned_by_matches(v1_models, "tachyon")
    {
        return "tachyon";
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
    if hits.ollama {
        return "ollama";
    }
    // Only reached when nothing matched. An authenticated endpoint answers
    // every signature probe with a challenge, so `generic` here would be a
    // classification the probe never earned.
    if hits.v1_models == V1ModelsProbe::AuthRequired {
        return LOCAL_KIND_UNKNOWN_AUTHENTICATED;
    }
    // An incomplete round must not read as a classification: with a probe
    // thread dead and no signature matched, `generic` is an answer the
    // probe never earned.
    if hits.panicked {
        return LOCAL_KIND_PROBE_FAILED;
    }
    "generic"
}

/// `models probe-local [--url URL [--model ID]] [--api-key-env VAR]`:
/// identify the local inference server kind. Without `--url` the target is
/// the active model's base_url and its stored credential, mirroring the GUI's
/// credential resolution — except that a keyless-declared record (see
/// `SavedModel::declares_keyless_auth`) probes credential-free on both the
/// `--model` and active-model lanes, like `models test` and the GUI ladder.
/// Refuses non-loopback hosts with a usage error before any request is sent.
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
fn probe_local<S: CredentialStore>(
    store: &S,
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
            let prefs = plain_prefs();
            let model = prefs
                .active_model()
                .cloned()
                .ok_or_else(|| CliError::failed("no active model to probe"))?;
            if !is_loopback_url(&model.base_url, UrlOrigin::StoredConfig)? {
                // Round-37 review: "pass --url" sent the user in a circle —
                // the --url value hits the identical loopback guard. Name
                // the rule instead of the dead-end action.
                return Err(CliError::usage(format!(
                    "the active model base_url {} is not a loopback endpoint; probe-local \
                     probes loopback endpoints only — pass --url with a loopback address",
                    // Same collapse discipline the human rows for the same
                    // value use: the stored base_url is user-supplied and a
                    // control character here would forge the stderr message
                    // (round-44 review).
                    collapse_control_characters(&model.base_url)
                )));
            }
            Some(model)
        }
    };
    // Phase 2 — only now resolve the explicit key override, whose failures
    // are host failures (exit 1).
    let explicit_key = match api_key_env {
        Some(var) => {
            // Same distinction `resolve_secret` draws (and its comment
            // justifies): a set-but-non-UTF-8 variable reported as "not
            // set" sends the user hunting for a missing export when the
            // variable exists but carries undecodable bytes.
            let value = std::env::var(var).map_err(|error| match error {
                std::env::VarError::NotUnicode(_) => CliError::failed(format!(
                    "probe-local: api key environment variable {var} is set but not valid UTF-8"
                )),
                _ => CliError::failed(format!(
                    "probe-local: api key environment variable {var} is not set"
                )),
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
            // Round-40 review: this row renders ok/code/detail/http_status
            // only, so it loads prefs WITHOUT the all-models keychain
            // refresh (`safe_prefs`) its rows never render — the same
            // conversion the round-39 review applied to `test_connection`
            // and the active-model arm; the key resolves for exactly this
            // one model via the honest resolver below (round-49 review:
            // an unreachable OS keyring must report
            // `credential_unavailable`, not "no stored api key" — the key
            // may well exist where this process cannot read it).
            let prefs = plain_prefs();
            let model = find_model(&prefs, id)?;
            // Keyless parity (round-5 review): a record that declares the
            // endpoint keyless probes credential-free — the declaration, not
            // the keyring, decides the posture, exactly like `models test`
            // and the GUI probe ladder (a residual stored credential from a
            // hand-edited settings.json is never presented, and the clean
            // declared state must proceed anonymously instead of tripping
            // the missing-key error below).
            if model.declares_keyless_auth() {
                None
            } else {
                let key =
                    resolve_honest_saved_key(store, &model)?.filter(|key| !key.trim().is_empty());
                if key.is_none() {
                    return Err(CliError::failed(format!(
                        "probe-local: model {id} has no stored api key to present"
                    )));
                }
                key
            }
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
                // A probe-time `--api-key-env` is the caller's explicit
                // instruction to present one key, so it outranks even the
                // declaration (the record posture governs stored
                // credentials, not deliberate probe overrides).
                Some(key) => Some(key),
                // Keyless parity (round-5 review): the declaration
                // short-circuits BEFORE the stored-credential read —
                // mirroring `models test` above and the GUI probe ladder.
                None if model.declares_keyless_auth() => None,
                None => {
                    // Swallowing a keychain failure here would turn every
                    // signed request into a 401 and classify a working
                    // server as `generic` — surface it instead. Round-49
                    // review: the honest resolver keeps an unreachable OS
                    // keyring from degrading a miss into an anonymous probe
                    // (the endpoint's 401 would misclassify it) — it fails
                    // as `credential_unavailable` like every other lane.
                    resolve_honest_saved_key(store, &model)?.filter(|key| !key.trim().is_empty())
                }
            };
            (model.base_url, bearer)
        }
    };
    let authenticated = bearer.is_some();
    let kind = select_local_server_kind_from_hits(&probe_candidates(&target, bearer.as_deref()));
    // The human lines collapse the target like every other untrusted cell
    // (`models list`/`show`): WHATWG URL parsing strips interior control
    // characters before the loopback gate, so a stored
    // `http://127.0.0.1:8000/\nkind: fake` would otherwise forge human output
    // lines. JSON keeps the verbatim value (round-39 review).
    let human_url = collapse_control_characters(&target);
    // `unknown_authenticated` is not a classification, it is the absence of
    // one: the endpoint answered every signature probe with 401/403. Saying
    // so with exit 1 keeps a script from reading a kind the probe never
    // earned, and the detail names the flag that would let the next run
    // actually classify it.
    // The two failure sentinels share one rendering — the kind, the verbatim
    // url in JSON / collapsed url in human, and a per-lane detail — and exit
    // 1 so a script never reads a kind the probe did not earn. Only the
    // unknown-authenticated lane carries the `authenticated` fact; the key
    // stays absent from the probe-failed row.
    if kind == LOCAL_KIND_PROBE_FAILED || kind == LOCAL_KIND_UNKNOWN_AUTHENTICATED {
        let authenticated_fact =
            (kind == LOCAL_KIND_UNKNOWN_AUTHENTICATED).then_some(authenticated);
        let detail = match authenticated_fact {
            None => {
                "a local-server probe failed unexpectedly, so the round was \
                 incomplete; rerun the probe (if it repeats, the server's behavior on one \
                 of the probed endpoints is crashing the probe)"
            }
            Some(true) => {
                "the endpoint rejected the credential that was presented (401/403), \
                 so no server signature could be read"
            }
            Some(false) => {
                "the endpoint requires authentication (401/403); rerun with \
                 --model <id> or --api-key-env <var> so the probe can present a credential"
            }
        };
        let mut json = serde_json::json!({ "url": target, "kind": kind });
        if let Some(authenticated_fact) = authenticated_fact {
            json["authenticated"] = serde_json::Value::Bool(authenticated_fact);
        }
        json["detail"] = serde_json::Value::from(detail);
        let text = render(
            output,
            format!("url: {human_url}\nkind: {kind}\ndetail: {detail}"),
            &json,
        );
        return Ok(CliOutcome {
            exit_code: ExitCode::Failed,
            stdout: text,
        });
    }
    let text = render(
        output,
        format!("url: {human_url}\nkind: {kind}"),
        &serde_json::json!({ "url": target, "kind": kind }),
    );
    Ok(success(text))
}

// ---------------------------------------------------------------------------
// Endpoint model discovery (models discover). Mirrors the GUI's
// core::endpoint_discovery ladder; not covered by live tests.
// ---------------------------------------------------------------------------

/// The three facts a listing row can carry about WHERE a length came from.
/// The GUI also back-fills from its model-length catalog; the CLI never does
/// (the catalog lives in the app-private `core` module and is not reachable
/// from this crate), so every value serialized here is `endpoint`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModelFactSource {
    Endpoint,
    /// Kept for vocabulary parity with the GUI's `ModelFactSource` (its
    /// catalog back-fill serializes this string); the CLI never fabricates a
    /// catalog fact, so nothing constructs it today.
    #[allow(dead_code)]
    Catalog,
}

impl ModelFactSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Endpoint => "endpoint",
            Self::Catalog => "catalog",
        }
    }
}

/// The stable status vocabulary of the GUI's `EndpointModelsListing`
/// (snake_case on the wire), consumed by scripts through `--json`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListingStatus {
    Listed,
    AuthRequired,
    Unsupported,
    Error,
}

impl ListingStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Listed => "listed",
            Self::AuthRequired => "auth_required",
            Self::Unsupported => "unsupported",
            Self::Error => "error",
        }
    }
}

/// One discovered model. Field names are the GUI's `DiscoveredModel` DTO so
/// the `--json` rows parse identically on both surfaces; `context_from` /
/// `output_from` are present exactly when the corresponding length is (the
/// CLI never fabricates a `Catalog` source).
#[derive(Clone, Debug, PartialEq, Eq)]
struct DiscoveredModel {
    id: String,
    context_window_tokens: Option<u32>,
    context_from: Option<ModelFactSource>,
    max_output_tokens: Option<u32>,
    output_from: Option<ModelFactSource>,
    /// Only meaningful for the local Ollama / LM Studio lanes (`/api/ps`
    /// membership, `/api/v0/models` `state`); remote listings leave it
    /// `None` like the GUI does for unknown-state endpoints.
    loaded: Option<bool>,
    /// anthropic `display_name` / gemini `displayName`; the OpenAI shapes
    /// have no display title.
    title: Option<String>,
}

impl DiscoveredModel {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "context_window_tokens": self.context_window_tokens,
            "context_from": self.context_from.map(ModelFactSource::as_str),
            "max_output_tokens": self.max_output_tokens,
            "output_from": self.output_from.map(ModelFactSource::as_str),
            "loaded": self.loaded,
            "title": self.title,
        })
    }
}

/// The CLI's `EndpointModelsListing` twin: same field names, same snake_case
/// status/source vocabulary, same `Option`-as-null JSON shape a plain serde
/// derive would produce on the GUI side. Serialized by hand because this
/// crate depends on `serde_json` only (no serde derive), the same way every
/// other JSON payload in this module is built.
#[derive(Clone, Debug, PartialEq, Eq)]
struct EndpointModelsListing {
    status: ListingStatus,
    http_status: Option<u16>,
    /// Transport class of a failed request ("timeout" | "dns" | "connect" |
    /// "network", the GUI discovery lane's own vocabulary, classified in its
    /// is_timeout → is_dns → is_connect order); `None` unless the failure
    /// happened below HTTP.
    transport: Option<&'static str>,
    /// The local server kind when the loopback/private short-circuit fired
    /// (the same strings `models probe-local` reports). The two probe-local
    /// honesty sentinels are NOT server kinds themselves, but the short-
    /// circuit carries the DETECTED kind on every outcome — `generic` for
    /// the all-miss sentinel (round-5 review minor: this comment used to
    /// claim `None`, which never happens on the local branch; the wire
    /// always carries `"server_kind":"generic"` there).
    server_kind: Option<&'static str>,
    models: Vec<DiscoveredModel>,
    truncated: bool,
}

impl EndpointModelsListing {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "status": self.status.as_str(),
            "http_status": self.http_status,
            "transport": self.transport,
            "server_kind": self.server_kind,
            "models": self.models.iter().map(DiscoveredModel::to_json).collect::<Vec<_>>(),
            "truncated": self.truncated,
        })
    }
}

fn listed_listing(models: Vec<DiscoveredModel>, truncated: bool) -> EndpointModelsListing {
    EndpointModelsListing {
        // A listed answer carries no http_status: the GUI's listing assembly
        // clears it (the status code is redundant once the body parsed), and
        // the two surfaces must serialize identically.
        status: ListingStatus::Listed,
        http_status: None,
        transport: None,
        server_kind: None,
        models,
        truncated,
    }
}

fn auth_listing(http_status: u16) -> EndpointModelsListing {
    EndpointModelsListing {
        status: ListingStatus::AuthRequired,
        http_status: Some(http_status),
        transport: None,
        server_kind: None,
        models: Vec::new(),
        truncated: false,
    }
}

fn unsupported_listing(http_status: Option<u16>) -> EndpointModelsListing {
    EndpointModelsListing {
        status: ListingStatus::Unsupported,
        http_status,
        transport: None,
        server_kind: None,
        models: Vec::new(),
        truncated: false,
    }
}

fn error_listing(
    http_status: Option<u16>,
    transport: Option<&'static str>,
) -> EndpointModelsListing {
    EndpointModelsListing {
        status: ListingStatus::Error,
        http_status,
        transport,
        server_kind: None,
        models: Vec::new(),
        truncated: false,
    }
}

/// Positive-u32 gate for self-reported endpoint values: outside the u32
/// range, zero, negative and non-numeric values all read as "undeclared"
/// (`as u32` truncation would turn a corrupted 2^32 into a real-looking 0).
/// Same rule as the GUI probes' `parse_positive_u32`.
fn discovery_positive_u32(value: &serde_json::Value) -> Option<u32> {
    u32::try_from(value.as_u64()?)
        .ok()
        .filter(|limit| *limit > 0)
}

/// Whether the discovery ladder may take the local short-circuit for this
/// base_url. Mirrors the GUI's `base_url_uses_local_or_private` exactly:
/// loopback (localhost / 127.0.0.0/8 / ::1), RFC1918 private ranges
/// (10/8, 172.16/12, 192.168/16), and the Docker Desktop host aliases.
/// A public base_url saved with a local-looking prefix must not receive
/// native probes here either — the parse is on the URL's HOST, not on any
/// textual prefix.
fn is_local_or_private_base_url(raw: &str) -> bool {
    reqwest::Url::parse(raw)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .is_some_and(|host| {
            let host = host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim_end_matches('.');
            if host.eq_ignore_ascii_case("localhost") {
                return true;
            }
            if host.eq_ignore_ascii_case("host.docker.internal")
                || host.eq_ignore_ascii_case("host.lima.internal")
                || host.eq_ignore_ascii_case("host.orbstack.internal")
                || host.ends_with(".docker.internal")
            {
                return true;
            }
            let Ok(address) = host.parse::<std::net::IpAddr>() else {
                return false;
            };
            if address.is_loopback() {
                return true;
            }
            match address {
                std::net::IpAddr::V4(v4) => v4.is_private(),
                std::net::IpAddr::V6(_) => false,
            }
        })
}

/// Transport class of a failed discovery request, from reqwest's structured
/// flags plus the error SOURCE CHAIN (round-3 review minor) rather than its
/// top-level `Display` — the display embeds the full URL (userinfo included)
/// and this lane's error rows must carry NEITHER the raw text NOR the URL NOR
/// the key, so the classification never reads the top-level text at all. The
/// `tls` arm (the GUI's `transport_class` gained it in round 2) walks the
/// error's `source()` chain via the shared
/// `pinvou3_lib::model_probe::error_chain_looks_like_tls`: certificate facts
/// live in the inner rustls/hyper error texts, never in the top-level
/// Display, so a host whose URL merely CONTAINS "ssl" cannot bias the class.
/// Vocabulary is the discovery lane's own short table
/// ("timeout" | "dns" | "connect" | "tls" | "network", shared with the GUI's
/// `core::endpoint_discovery` transport_class — NOT the monitor facade's
/// longer `dns_failed`/`connection_refused` codes, which feed different
/// consumers). Order mirrors the GUI ladder (timeout → dns → tls →
/// connect → network; round-4 review M1 moved tls AHEAD of connect):
/// `is_dns` MUST precede the connect flag — hyper-util files resolver
/// failures under its Connect kind — and the tls chain scan must precede
/// `is_connect` for the same underlying reason: reqwest's `is_connect()`
/// walks the source chain too, and hyper-util files the TLS handshake under
/// its Connect kind, so a real certificate failure answers
/// `is_connect() == true` and would never reach the tls arm.
fn discovery_transport_class(error: &reqwest::Error) -> &'static str {
    discovery_transport_class_from_flags(
        error.is_timeout(),
        error.is_dns(),
        error.is_connect(),
        error,
    )
}

/// The flag+chain core of [`discovery_transport_class`], split out for the
/// same reason the app's `transport_code_for` takes booleans:
/// `reqwest::Error` has no public constructor, so the ARM ORDER — the
/// contract this lane shares with the GUI — is pinned by feeding the core
/// synthetic error chains (the same construction the app-side
/// `tls_class_walks_source_chain_not_top_level_display` test uses); the
/// wrapper above is only the four flag reads.
fn discovery_transport_class_from_flags(
    is_timeout: bool,
    is_dns: bool,
    is_connect: bool,
    error: &(dyn std::error::Error + '_),
) -> &'static str {
    if is_timeout {
        "timeout"
    } else if is_dns {
        "dns"
    } else if pinvou3_lib::model_probe::error_chain_looks_like_tls(error) {
        "tls"
    } else if is_connect {
        "connect"
    } else {
        "network"
    }
}

/// The anthropic-version value the GUI pins for its native-protocol probes
/// (`apply_anthropic_auth_headers`); the same literal `models test` sends.
const DISCOVER_ANTHROPIC_VERSION: &str = "2023-06-01";

/// The discovery listing body cap: 8 MiB, the GUI discovery ladder's own
/// `LISTING_BODY_CAP_BYTES` — the two surfaces must truncate the same
/// oversized answer the same way. Wider than the probe lanes'
/// [`PROBE_BODY_CAP_BYTES`] (4 MiB) because a gateway listing thousands of
/// models with extension fields is a real shape here; the timeout still
/// bounds time, this only bounds a hostile within-timeout stream.
const DISCOVER_BODY_CAP_BYTES: usize = 8 * 1024 * 1024;

/// The native anthropic/gemini ladders' TOTAL accumulated-entry cap, the
/// same-wave GUI bound (`NATIVE_LISTING_CAP`): three pages of 1000 entries
/// each is already 3000 rows a human picker must chew, so the accumulated
/// list truncates at 2000 and flags — the exact-boundary rule the
/// OpenAI-compatible lane's own 2000-entry cap applies (exactly 2000
/// entries stay unflagged, entry 2001 truncates). The native Ollama / LM
/// Studio listings have NO row cap: their `truncated` flag discloses
/// partial context facts (the show sweep), not dropped rows.
const DISCOVER_NATIVE_LISTING_CAP: usize = 2000;

/// The Ollama per-model `/api/show` sweep's overall wall-clock budget: the
/// count cap (50) bounds the LOOKUPS, but each runs on the 8s discovery
/// client, so a stalled-but-accepting server could stack the sweep into
/// minutes. 20s — the GUI discovery ladder's `OLLAMA_SHOW_BUDGET` — bounds
/// the whole sweep instead; past it the remaining entries keep their
/// undeclared context and the run reports `truncated`. (The CLI has no
/// shared show cache to consult — that part of the GUI lane is GUI-side —
/// so the budget is the only bound the two surfaces share verbatim.)
const DISCOVER_OLLAMA_SHOW_BUDGET: Duration = Duration::from_secs(20);

/// What one discovery HTTP round produced. The failure variants carry only
/// structured facts (a status number or a transport class) — never the
/// reqwest error text, so no redaction pass can regress into leaking a URL
/// or a key through this lane.
enum DiscoveryAnswer {
    /// 2xx with a parsed JSON body and its status code.
    Json(serde_json::Value, u16),
    /// 2xx whose body is not JSON or is over the listing body cap (a captive
    /// portal, a Cloudflare interstitial): the endpoint exists but did not
    /// speak the protocol.
    Unusable(u16),
    /// 2xx whose body broke mid-stream (a proxy/VPN cut the connection): a
    /// TRANSPORT fact, not a shape verdict — the read failed after the
    /// endpoint had already answered. The GUI ladder files this under
    /// `error` with transport `network` (its `BodyReadFail::MidStream` →
    /// `ProbeOutcome::BrokenBody`) and never under `unsupported`, which
    /// would pin a retryable failure as "the endpoint does not speak
    /// discovery".
    BrokenBody(u16),
    /// Any non-2xx FINAL status. The discovery client does not follow
    /// redirects ([`discovery_client`]), so a 3xx arrives here as-is and
    /// [`discovery_status_listing`] files it under `error` — a redirect is
    /// never silently re-sent with the caller's credentials attached.
    Status(u16),
    /// Request-level failure, classified WITHOUT the error text.
    Transport(&'static str),
}

/// Sends one discovery request and reduces it to a [`DiscoveryAnswer`]. The
/// body read goes through [`DISCOVER_BODY_CAP_BYTES`], the GUI listing
/// ladder's own cap — wider than the probe lanes' 4 MiB because a gateway
/// listing thousands of models with OpenRouter-style extension fields is a
/// real shape, and truncating a legitimate listing into "not JSON" would
/// misclassify a working endpoint as unsupported.
fn discovery_send(request: reqwest::blocking::RequestBuilder) -> DiscoveryAnswer {
    let response = match request.send() {
        Ok(response) => response,
        Err(error) => return DiscoveryAnswer::Transport(discovery_transport_class(&error)),
    };
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return DiscoveryAnswer::Status(status);
    }
    match read_body_capped(response, DISCOVER_BODY_CAP_BYTES) {
        BodyRead::Bytes(bytes) => serde_json::from_slice(&bytes)
            .map(|value| DiscoveryAnswer::Json(value, status))
            .unwrap_or(DiscoveryAnswer::Unusable(status)),
        // A read that FAILED mid-stream is a transport break (the GUI's
        // `BodyReadFail::MidStream`), while an over-cap body is a shape the
        // endpoint chose — the two must not share a classification, or a
        // retryable proxy cut gets pinned as "endpoint does not speak
        // discovery". Truncation at the cap ends with a SHORT body that
        // simply fails the JSON parse, so it lands in `Unusable` above —
        // the same fold the GUI applies (`read_json_body_bounded`'s cap
        // stop never returns its MidStream arm).
        //
        // Round-3 review minor: a mid-body TIMEOUT splits out as transport
        // `timeout` (the GUI's `BodyReadFail::MidStreamTimeout` →
        // `ProbeOutcome::Transport("timeout")`, http None) — the retry hint
        // names the real cause instead of the generic network cut.
        BodyRead::Unreadable => DiscoveryAnswer::BrokenBody(status),
        BodyRead::UnreadableTimeout => DiscoveryAnswer::Transport("timeout"),
        BodyRead::TooLarge => DiscoveryAnswer::Unusable(status),
    }
}

/// The status-only half of the classification ladder: `Some` for every
/// non-2xx status the ladder decides on (401/403 → auth, 404/405 →
/// unsupported, everything else → error with the status kept), `None` for
/// 2xx (the caller proceeds to parse the body).
fn discovery_status_listing(status: u16) -> Option<EndpointModelsListing> {
    match status {
        401 | 403 => Some(auth_listing(status)),
        404 | 405 => Some(unsupported_listing(Some(status))),
        200..=299 => None,
        _ => Some(error_listing(Some(status), None)),
    }
}

/// Context spellings the OpenAI-compatible ladder normalizes over. When an
/// entry reports several, the TIGHTEST wins — the same only-tighten doctrine
/// the output limit applies: preferring one shape by name could adopt a cap
/// larger than another cap the same entry reported, and an over-reported
/// window lets upstream silently truncate (the conservative direction is
/// always down).
///
/// `max_context_length` is deliberately ABSENT. It is LM Studio's capability
/// ceiling — the maximum the model supports — not a window the server will
/// serve; an entry reporting only that name has NO declared context. The
/// connection-test lane refuses it for the same reason
/// (`parse_models_response_list_refuses_lmstudio_cap_alias`), the GUI's
/// native LM Studio lane reads only the loaded window, and the compat lane
/// must match: a remote entry that carried the ceiling into `discover` would
/// ride the empty-form prefill / bulk-add path into a deployment fact.
const DISCOVER_CONTEXT_FIELDS: &[&str] = &["max_model_len", "context_length", "context_window"];

/// Per-entry output limit, mirroring the GUI probes'
/// `parse_entry_output_limit`: direct `max_output_tokens` /
/// `max_completion_tokens` plus the OpenRouter gateway's
/// `top_provider.max_completion_tokens` (unlimited is null → undeclared),
/// tightest wins.
fn discovery_output_limit(item: &serde_json::Value) -> Option<u32> {
    let direct = ["max_output_tokens", "max_completion_tokens"]
        .into_iter()
        .filter_map(|field| item.get(field).and_then(discovery_positive_u32));
    let top_provider = item
        .get("top_provider")
        .and_then(|provider| provider.get("max_completion_tokens"))
        .and_then(discovery_positive_u32);
    direct.chain(top_provider).min()
}

/// One OpenAI-compatible `/models` entry → a [`DiscoveredModel`]. `loaded` /
/// `title` stay `None` (these shapes carry neither fact).
fn discovery_openai_entry(item: &serde_json::Value) -> Option<DiscoveredModel> {
    let id = item.get("id").and_then(|value| value.as_str())?.trim();
    if id.is_empty() {
        return None;
    }
    let context = DISCOVER_CONTEXT_FIELDS
        .iter()
        .filter_map(|field| item.get(*field).and_then(discovery_positive_u32))
        .min();
    let output = discovery_output_limit(item);
    Some(DiscoveredModel {
        id: id.to_owned(),
        context_window_tokens: context,
        context_from: context.map(|_| ModelFactSource::Endpoint),
        max_output_tokens: output,
        output_from: output.map(|_| ModelFactSource::Endpoint),
        loaded: None,
        title: None,
    })
}

/// Accepts `{data:[...]}` (OpenAI) AND the bare-array shape (Together): the
/// envelope key is preferred, but an array at the top level is a model list
/// all the same. `None` = neither shape (an object without `data`, a scalar,
/// ...) → the endpoint did not speak the protocol.
fn discovery_openai_entries(value: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .or_else(|| value.as_array())
}

/// Parses + caps an OpenAI-compatible body: `None` when the shape is not a
/// model list, otherwise the parsed entries truncated to
/// [`DISCOVER_ENTRY_CAP`] and whether the cap cut anything off (an empty
/// array is a legal listed answer). Factored out of the network lane so the
/// cap is testable without a server. The cap stops PARSING at the bound
/// (round-5 review m1 — the app lane's per-entry break, previously the CLI
/// collected every valid row and `truncate`d after): a hostile endpoint
/// packing minimal entries into the 8 MiB body cap no longer materializes
/// ~640k rows (~130-190 MB transient) before the cut — the row phase now
/// costs the same ~2001-row bound as the app.
fn discovery_openai_models(value: &serde_json::Value) -> Option<(Vec<DiscoveredModel>, bool)> {
    const DISCOVER_ENTRY_CAP: usize = 2000;
    let entries = discovery_openai_entries(value)?;
    let mut models: Vec<DiscoveredModel> = Vec::new();
    let mut truncated = false;
    for entry in entries {
        if let Some(model) = discovery_openai_entry(entry) {
            if models.len() >= DISCOVER_ENTRY_CAP {
                truncated = true;
                break;
            }
            models.push(model);
        }
    }
    Some((models, truncated))
}

/// Builds the request for one OpenAI-compatible listing round: bearer when a
/// key was given (blank/absent stays anonymous — keyless endpoints such as
/// OpenRouter or a local server must stay reachable), plus the OpenCode
/// gateway affinity header the GUI's own model probe attaches (the Go
/// gateway rejects hand-rolled auxiliary clients without it; same gate and
/// same derivation key as `probe_openai_models`).
fn discovery_openai_request(
    client: &'static reqwest::blocking::Client,
    url: &str,
    key: Option<&str>,
    base_url: &str,
) -> reqwest::blocking::RequestBuilder {
    let mut request = apply_bearer(client.get(url), key);
    request = with_discovery_affinity(request, base_url);
    request
}

/// The OpenCode gateway session-affinity header on a raw request builder
/// (round-3 review minor: the GUI attaches it on all THREE ladders —
/// `with_opencode_session_header` at the compat, anthropic and gemini steps
/// — while the CLI only sent it on the compat lane; a gateway discovery of
/// `--wire anthropic` 400'd with MissingSessionID in the CLI where the GUI
/// was accepted). No-op off-gateway.
fn with_discovery_affinity(
    mut request: reqwest::blocking::RequestBuilder,
    base_url: &str,
) -> reqwest::blocking::RequestBuilder {
    if pinvou3_lib::model_probe::is_opencode_gateway_base_url(base_url) {
        request = request.header(
            "x-opencode-session",
            pinvou3_lib::model_probe::opencode_session_id_for("models-probe"),
        );
    }
    request
}

/// The OpenAI-compatible ladder: a single `GET {base}/models`
/// (`models_probe_url` — no `/v1` is invented; glm `/paas/v4`, Ark `/api/v3`
/// and gemini's OpenAI-compatible surface keep working because their real
/// version segment is preserved), `{data}` or bare array accepted, entry cap
/// 2000.
fn discovery_openai_listing(base_url: &str, key: Option<&str>) -> EndpointModelsListing {
    discovery_openai_listing_at(
        &pinvou3_lib::model_probe::models_probe_url(base_url.trim()),
        base_url,
        key,
    )
}

/// URL-qualified variant (the GUI ladder's `ladder_openai_url` shape): the
/// local `/v1` retry re-enters with the assembled `{base}/v1/models` while
/// the gateway-affinity decision still keys on the ORIGINAL base_url.
fn discovery_openai_listing_at(
    url: &str,
    base_url: &str,
    key: Option<&str>,
) -> EndpointModelsListing {
    let Some(client) = discovery_client() else {
        return error_listing(None, None);
    };
    let request = discovery_openai_request(client, url, key, base_url);
    match discovery_send(request) {
        DiscoveryAnswer::Transport(class) => error_listing(None, Some(class)),
        DiscoveryAnswer::Status(status) => {
            discovery_status_listing(status).unwrap_or_else(|| error_listing(Some(status), None))
        }
        DiscoveryAnswer::Unusable(status) => unsupported_listing(Some(status)),
        // The GUI's BrokenBody terminal, field for field: error with the
        // status kept AND transport `network` — a mid-stream body break is
        // retryable, never "unsupported". (A mid-body TIMEOUT now carries
        // transport `timeout` via [`DiscoveryAnswer::Transport`].)
        DiscoveryAnswer::BrokenBody(status) => error_listing(Some(status), Some("network")),
        DiscoveryAnswer::Json(value, status) => match discovery_openai_models(&value) {
            Some((models, truncated)) => listed_listing(models, truncated),
            None => unsupported_listing(Some(status)),
        },
    }
}

/// The local short-circuit's OpenAI-compatible ladder + the bare-host `/v1`
/// retry (round-3 review M4b, the GUI `ladder_openai_local` mirror): healthy
/// local vLLM / Ollama-compat servers are commonly configured as a bare
/// address where `{base}/models` 404s while the service answers at
/// `{base}/v1/models`. Only the LOCAL lanes retry (this fn is called from
/// the short-circuit branch only), only on Unsupported(404/405), and only
/// when the base does not already end `/v1`; remote roots vary (glm
/// `/paas/v4` etc.) and a blind `/v1` append would launder a real
/// "path not found" into another 404.
fn discovery_openai_listing_local(base_url: &str, key: Option<&str>) -> EndpointModelsListing {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        return discovery_openai_listing(base_url, key);
    }
    let first = discovery_openai_listing(base_url, key);
    match (&first.status, first.http_status) {
        (ListingStatus::Unsupported, Some(404 | 405)) => {
            discovery_openai_listing_at(&format!("{trimmed}/v1/models"), base_url, key)
        }
        _ => first,
    }
}

/// One anthropic `/models` page → its entries, the `has_more` flag and the
/// `after_id` cursor (the page's last entry id). Shape mismatch (`data` not
/// an array) is `None` → unsupported.
fn discovery_anthropic_page(
    value: &serde_json::Value,
) -> Option<(Vec<DiscoveredModel>, Option<bool>, Option<String>)> {
    let data = value.get("data").and_then(serde_json::Value::as_array)?;
    let models = data
        .iter()
        .filter_map(|item| {
            let id = item.get("id").and_then(|value| value.as_str())?.trim();
            if id.is_empty() {
                return None;
            }
            let context = item
                .get("max_input_tokens")
                .and_then(discovery_positive_u32);
            let output = item.get("max_tokens").and_then(discovery_positive_u32);
            // Same string handling as the GUI page parser: the display name
            // is taken verbatim (no trim / empty filter) so both surfaces
            // serialize identical rows for the same endpoint.
            let title = item
                .get("display_name")
                .and_then(|value| value.as_str())
                .map(str::to_owned);
            Some(DiscoveredModel {
                id: id.to_owned(),
                context_window_tokens: context,
                context_from: context.map(|_| ModelFactSource::Endpoint),
                max_output_tokens: output,
                output_from: output.map(|_| ModelFactSource::Endpoint),
                loaded: None,
                title,
            })
        })
        .collect::<Vec<_>>();
    let has_more = value.get("has_more").and_then(|value| value.as_bool());
    let after_id = data
        .last()
        .and_then(|item| item.get("id"))
        .and_then(|value| value.as_str())
        .map(str::to_owned);
    // Malformed/missing `has_more` is `None`, NOT `false` (round-3 review
    // minor, the GUI parser's same amendment): a malformed answer must not
    // collapse into "complete, untruncated" — the ladder ends it TRUNCATED.
    Some((models, has_more, after_id))
}

/// Whether `base_url` points at the official anthropic API host, the GUI's
/// `is_anthropic_api_url` semantics (host equals `api.anthropic.com`,
/// ASCII case-insensitive; the shared helper comes through the
/// `pinvou3_lib::model_probe` facade). A parse failure reads as
/// non-official — the same wrapper rule the GUI's string-level
/// `is_anthropic_endpoint` applies — so the caller falls to the standard
/// bearer arm instead of failing the ladder.
fn discovery_anthropic_official_host(base_url: &str) -> bool {
    reqwest::Url::parse(base_url.trim())
        .map(|url| pinvou3_lib::model_probe::is_anthropic_api_url(&url))
        .unwrap_or(false)
}

/// One anthropic ladder request: auth headers + gateway affinity, the exact
/// composition the GUI's ladder builds
/// (`with_opencode_session_header(apply_anthropic_auth_headers(...))` over
/// its host split). Auth follows the host: ONLY the official
/// api.anthropic.com host authenticates with `x-api-key` +
/// `anthropic-version` (its API rejects Bearer); ANY other host — a
/// LiteLLM-style proxy, a corporate relay, a gateway — takes the key as a
/// standard bearer credential (round-4 review M6: the unconditional
/// `x-api-key` made every non-official anthropic-protocol endpoint look
/// unauthenticated unless it also happened to read the custom header);
/// keyless sends no auth header in either arm. Factored out of
/// [`discovery_anthropic_listing`] so the header contract is inspectable
/// without a network (the same seam `connection_probe_request` uses).
/// Pagination queries are applied by the caller on the returned builder.
fn discovery_anthropic_request(
    client: &'static reqwest::blocking::Client,
    url: String,
    key: Option<&str>,
    base_url: &str,
) -> reqwest::blocking::RequestBuilder {
    let mut request = client.get(url);
    match key.map(str::trim).filter(|key| !key.is_empty()) {
        Some(key) if discovery_anthropic_official_host(base_url) => {
            request = request
                .header("x-api-key", key)
                .header("anthropic-version", DISCOVER_ANTHROPIC_VERSION);
        }
        Some(key) => request = request.bearer_auth(key),
        None => {}
    }
    with_discovery_affinity(request, base_url)
}

/// The anthropic ladder: `GET {normalized base}/v1/models?limit=1000`
/// (a configured root already ending in `/v1` appends directly), with the
/// host-split auth headers of [`discovery_anthropic_request`],
/// paginating with `has_more` + `after_id` up to THREE pages and a
/// [`DISCOVER_NATIVE_LISTING_CAP`]-entry total cap; either bound ending the
/// run early is the `truncated` flag, never a silent loss.
fn discovery_anthropic_listing(base_url: &str, key: Option<&str>) -> EndpointModelsListing {
    const DISCOVER_ANTHROPIC_PAGE_CAP: usize = 3;
    let Some(client) = discovery_client() else {
        return error_listing(None, None);
    };
    // The messages-endpoint normalization rule (`anthropic_messages_url`):
    // a configured root already ending in `/v1` appends directly, anything
    // else gets `/v1` — the official preset upstream
    // (`https://api.anthropic.com`) has its list at `/v1/models`.
    let trimmed = base_url.trim().trim_end_matches('/');
    let base = if trimmed.ends_with("/v1") {
        trimmed.to_owned()
    } else {
        format!("{trimmed}/v1")
    };
    let mut models = Vec::new();
    let mut after_id: Option<String> = None;
    for page in 1..=DISCOVER_ANTHROPIC_PAGE_CAP {
        let mut request =
            discovery_anthropic_request(client, format!("{base}/models"), key, base_url)
                .query(&[("limit", "1000")]);
        if let Some(after) = after_id.as_deref() {
            request = request.query(&[("after_id", after)]);
        }
        match discovery_send(request) {
            DiscoveryAnswer::Transport(class) => return error_listing(None, Some(class)),
            DiscoveryAnswer::Status(status) => {
                return discovery_status_listing(status)
                    .unwrap_or_else(|| error_listing(Some(status), None));
            }
            DiscoveryAnswer::Unusable(status) => return unsupported_listing(Some(status)),
            DiscoveryAnswer::BrokenBody(status) => {
                return error_listing(Some(status), Some("network"));
            }
            DiscoveryAnswer::Json(value, status) => {
                let Some((page_models, has_more, after)) = discovery_anthropic_page(&value) else {
                    return unsupported_listing(Some(status));
                };
                models.extend(page_models);
                // The total-entry cap: past it the accumulated list is
                // truncated and flagged (the GUI's NATIVE_LISTING_CAP),
                // never silently extended.
                if models.len() > DISCOVER_NATIVE_LISTING_CAP {
                    models.truncate(DISCOVER_NATIVE_LISTING_CAP);
                    return listed_listing(models, true);
                }
                // Round-3 review minor, the GUI ladder's same three-way
                // split: `Some(false)` completes the listing; a malformed
                // or missing `has_more` (`None`) ends TRUNCATED without
                // blindly probing another page; `Some(true)` continues on
                // the cursor, or truncates when the page carries none.
                match has_more {
                    Some(false) => return listed_listing(models, false),
                    None => return listed_listing(models, true),
                    Some(true) => match after {
                        Some(next_after) if page < DISCOVER_ANTHROPIC_PAGE_CAP => {
                            after_id = Some(next_after);
                        }
                        _ => return listed_listing(models, true),
                    },
                }
            }
        }
    }
    listed_listing(models, true)
}

/// One gemini `/v1beta/models` page → its generateContent-capable entries and
/// the `nextPageToken` cursor. Only entries whose
/// `supportedGenerationMethods` include `generateContent` are listings-worthy
/// (embedding / aio entries cannot serve a chat model slot; the method match
/// is ASCII case-insensitive, the GUI parser's tolerance); `models/` is a
/// resource prefix, not part of the id.
fn discovery_gemini_page(
    value: &serde_json::Value,
) -> Option<(Vec<DiscoveredModel>, Option<String>)> {
    let entries = value.get("models").and_then(serde_json::Value::as_array)?;
    let models = entries
        .iter()
        .filter_map(|item| {
            // Round-3 review minor, the GUI parser's same amendment: an
            // entry WITHOUT `supportedGenerationMethods` is kept — the
            // official API always sends the field, a gateway omitting it
            // must not silently filter the whole listing down to zero rows
            // ("the endpoint has no models" for a demonstrably listing
            // endpoint). Only an explicit list WITHOUT generateContent
            // (embedding etc.) filters.
            let generates = match item
                .get("supportedGenerationMethods")
                .and_then(serde_json::Value::as_array)
            {
                Some(methods) => methods.iter().any(|method| {
                    method
                        .as_str()
                        .is_some_and(|name| name.eq_ignore_ascii_case("generateContent"))
                }),
                None => true,
            };
            if !generates {
                return None;
            }
            let name = item.get("name").and_then(|value| value.as_str())?.trim();
            let id = name.strip_prefix("models/").unwrap_or(name);
            if id.is_empty() {
                return None;
            }
            let context = item.get("inputTokenLimit").and_then(discovery_positive_u32);
            let output = item
                .get("outputTokenLimit")
                .and_then(discovery_positive_u32);
            // Verbatim like the GUI page parser (no trim / empty filter).
            let title = item
                .get("displayName")
                .and_then(|value| value.as_str())
                .map(str::to_owned);
            Some(DiscoveredModel {
                id: id.to_owned(),
                context_window_tokens: context,
                context_from: context.map(|_| ModelFactSource::Endpoint),
                max_output_tokens: output,
                output_from: output.map(|_| ModelFactSource::Endpoint),
                loaded: None,
                title,
            })
        })
        .collect::<Vec<_>>();
    let next_page_token = value
        .get("nextPageToken")
        .and_then(|value| value.as_str())
        .filter(|token| !token.is_empty())
        .map(str::to_owned);
    Some((models, next_page_token))
}

/// One gemini ladder request: `x-goog-api-key` + gateway affinity. Unlike
/// the anthropic ladder there is NO host split — the gemini API
/// authenticates with `x-goog-api-key` on every host (mirror proxies
/// included), and a keyless run sends no auth header. Factored out of
/// [`discovery_gemini_listing`] for the same inspect-without-a-network
/// seam as [`discovery_anthropic_request`]; pagination queries are applied
/// by the caller on the returned builder.
fn discovery_gemini_request(
    client: &'static reqwest::blocking::Client,
    url: String,
    key: Option<&str>,
    base_url: &str,
) -> reqwest::blocking::RequestBuilder {
    let mut request = client.get(url);
    if let Some(key) = key.map(str::trim).filter(|key| !key.is_empty()) {
        request = request.header("x-goog-api-key", key);
    }
    with_discovery_affinity(request, base_url)
}

/// The gemini ladder: a configured OpenAI-compatibility surface (`/openai`
/// suffix) is stripped to the native root, then
/// `GET {root}/v1beta/models?pageSize=1000` with the
/// [`discovery_gemini_request`] headers, paginating with `nextPageToken` up
/// to THREE pages and a [`DISCOVER_NATIVE_LISTING_CAP`]-entry total cap.
fn discovery_gemini_listing(base_url: &str, key: Option<&str>) -> EndpointModelsListing {
    const DISCOVER_GEMINI_PAGE_CAP: usize = 3;
    let Some(client) = discovery_client() else {
        return error_listing(None, None);
    };
    // The native root: a configured OpenAI-compatibility surface (`/openai`
    // suffix) is stripped, then the version segment `/v1beta` (users often
    // paste the OpenAI-compatible address `/v1beta/openai` wholesale) — the
    // GUI's `gemini_native_root` rule, in its same two-step order.
    let mut root = base_url.trim().trim_end_matches('/').to_owned();
    for suffix in ["/openai", "/v1beta"] {
        if let Some(stripped) = root.strip_suffix(suffix) {
            root = stripped.to_owned();
        }
    }
    let mut models = Vec::new();
    let mut page_token: Option<String> = None;
    for _page in 1..=DISCOVER_GEMINI_PAGE_CAP {
        let mut request =
            discovery_gemini_request(client, format!("{root}/v1beta/models"), key, base_url)
                .query(&[("pageSize", "1000")]);
        if let Some(token) = page_token.as_deref() {
            request = request.query(&[("pageToken", token)]);
        }
        match discovery_send(request) {
            DiscoveryAnswer::Transport(class) => return error_listing(None, Some(class)),
            DiscoveryAnswer::Status(status) => {
                return discovery_status_listing(status)
                    .unwrap_or_else(|| error_listing(Some(status), None));
            }
            DiscoveryAnswer::Unusable(status) => return unsupported_listing(Some(status)),
            DiscoveryAnswer::BrokenBody(status) => {
                return error_listing(Some(status), Some("network"));
            }
            DiscoveryAnswer::Json(value, status) => {
                let Some((page_models, next)) = discovery_gemini_page(&value) else {
                    return unsupported_listing(Some(status));
                };
                models.extend(page_models);
                // The total-entry cap, same exact-boundary rule as the
                // anthropic lane (the GUI's NATIVE_LISTING_CAP).
                if models.len() > DISCOVER_NATIVE_LISTING_CAP {
                    models.truncate(DISCOVER_NATIVE_LISTING_CAP);
                    return listed_listing(models, true);
                }
                match next {
                    Some(token) => page_token = Some(token),
                    None => return listed_listing(models, false),
                }
            }
        }
    }
    // The page cap ran out while a next token kept arriving: truncated.
    listed_listing(models, true)
}

/// Ollama `/api/ps` → the loaded-name set (failure or odd shape reads as
/// empty: prefer marking everything unloaded over a fabricated `true`).
fn discovery_ollama_ps_names(value: &serde_json::Value) -> std::collections::HashSet<String> {
    value
        .get("models")
        .and_then(serde_json::Value::as_array)
        .map(|models| {
            models
                .iter()
                .filter_map(|item| {
                    item.get("name")
                        .or_else(|| item.get("model"))
                        .and_then(|value| value.as_str())
                        .map(|name| name.trim().to_owned())
                })
                .filter(|name| !name.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Ollama `/api/ps` → loaded model → effective (`num_ctx`-applied) context.
/// Both `name` and `model` register (explicit digest pulls differ between
/// the two); non-positive / out-of-range values stay undeclared.
fn discovery_ollama_ps_contexts(
    value: &serde_json::Value,
) -> std::collections::HashMap<String, u32> {
    let mut contexts = std::collections::HashMap::new();
    if let Some(models) = value.get("models").and_then(serde_json::Value::as_array) {
        for item in models {
            let Some(context) = item.get("context_length").and_then(discovery_positive_u32) else {
                continue;
            };
            for key in ["name", "model"] {
                if let Some(name) = item
                    .get(key)
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                {
                    contexts.insert(name.to_owned(), context);
                }
            }
        }
    }
    contexts
}

/// Ollama `/api/tags` → the downloaded-model names (ordered, deduped).
/// `None` = no `models` array (shape mismatch → unsupported), so an empty
/// downloaded list stays distinguishable from a non-Ollama answer.
fn discovery_ollama_tag_names(value: &serde_json::Value) -> Option<Vec<String>> {
    let mut names: Vec<String> = Vec::new();
    for item in value.get("models").and_then(serde_json::Value::as_array)? {
        let Some(name) = item.get("name").and_then(|value| value.as_str()) else {
            continue;
        };
        let name = name.trim();
        if !name.is_empty() && !names.iter().any(|existing| existing == name) {
            names.push(name.to_owned());
        }
    }
    Some(names)
}

/// Ollama `/api/show` → the Modelfile's `num_ctx` declaration only. The
/// `model_info.<arch>.context_length` GGUF TRAINING window is deliberately
/// NOT adopted (the GUI probes refuse it for the same reason): without an
/// explicit `num_ctx` Ollama runs the server default, which is
/// unknowable for an unloaded model — adopting the training cap would
/// over-report the window and let upstream silently truncate. The first
/// `num_ctx` line decides; a malformed value reads as undeclared (later
/// lines cannot overrule it).
fn discovery_ollama_show_context(value: &serde_json::Value) -> Option<u32> {
    let parameters = value.get("parameters").and_then(|value| value.as_str())?;
    for line in parameters.lines() {
        let mut parts = line.split_whitespace();
        if parts.next() == Some("num_ctx") {
            return parts
                .next()
                .and_then(|count| count.parse::<u64>().ok())
                .and_then(|count| u32::try_from(count).ok())
                .filter(|count| *count > 0);
        }
    }
    None
}

/// One `/api/show` follow-up for a model whose context did not come from
/// `/api/ps` (read-only metadata query; never a inference request). Any
/// failure degrades to "undeclared" — the listing must not fail because one
/// model's metadata query did.
fn discovery_ollama_query_show(
    client: &'static reqwest::blocking::Client,
    host: &str,
    key: Option<&str>,
    name: &str,
) -> Option<u32> {
    let request = apply_bearer(client.post(format!("{host}/api/show")), key)
        .json(&serde_json::json!({ "model": name }));
    match discovery_send(request) {
        DiscoveryAnswer::Json(value, _) => discovery_ollama_show_context(&value),
        _ => None,
    }
}

/// The ollama branch's three outcomes (round-3 review M4a restructure):
/// `Listed` for a usable native listing, `NotOllamaShaped` for a well-formed
/// tags round trip with no usable name list (empty list / shape drift — the
/// caller falls back to the OpenAI-compatible ladder, the historical
/// "no longer Ollama" semantics), and `Failed` for a TRANSIENT tags failure
/// re-checked and confirmed (busy 5xx / transport / auth) — reported at its
/// REAL classification, never laundered into `unsupported(404)` by a
/// compat-ladder rerun (the GUI's `ollama_native_failure` invariant:绝不把
/// 可重试的「服务端正忙」钉成「端点不支持发现」).
enum DiscoveryOllamaListing {
    Listed(Vec<DiscoveredModel>, bool),
    NotOllamaShaped,
    Failed(EndpointModelsListing),
}

/// The transient-failure recheck: one more `/api/tags` round. A 404/405 or
/// an unusable body reconfirms the fall-back semantics (404 = the kind cache
/// went stale, the server is no longer Ollama-shaped); a well-formed EMPTY
/// roster does too (an empty list is not a failure); every OTHER outcome
/// branches: the SAME transient failure twice → `Failed` at its real
/// classification, a well-formed NON-EMPTY roster → `HealthyNonEmpty` — the
/// service is still a healthy, listing Ollama and the listing probe just
/// hit a transient blip, so the native listing is retried once (round-5
/// GUI parity for the round-3 m1-6 retry arm: the CLI used to collapse
/// every well-formed recheck into the compat fallback, the round-4 M5 bug
/// class one lane over — on an `/api`-only deployment the GUI listed
/// native facts while the CLI reported compat rows or `unsupported(404)`).
enum DiscoveryOllamaRecheck {
    NotOllamaShaped,
    Failed(EndpointModelsListing),
    HealthyNonEmpty,
}

fn discovery_ollama_recheck_tags(base_url: &str, key: Option<&str>) -> DiscoveryOllamaRecheck {
    let Some(client) = discovery_client() else {
        return DiscoveryOllamaRecheck::NotOllamaShaped;
    };
    let host = strip_v1_suffix(base_url);
    let request = apply_bearer(client.get(format!("{host}/api/tags")), key);
    match discovery_send(request) {
        DiscoveryAnswer::Json(value, _) => {
            if matches!(discovery_ollama_tag_names(&value), Some(names) if !names.is_empty()) {
                DiscoveryOllamaRecheck::HealthyNonEmpty
            } else {
                DiscoveryOllamaRecheck::NotOllamaShaped
            }
        }
        DiscoveryAnswer::Unusable(_) | DiscoveryAnswer::Status(404 | 405) => {
            DiscoveryOllamaRecheck::NotOllamaShaped
        }
        DiscoveryAnswer::Status(status) => DiscoveryOllamaRecheck::Failed(
            discovery_status_listing(status).unwrap_or_else(|| error_listing(Some(status), None)),
        ),
        DiscoveryAnswer::Transport(class) => {
            DiscoveryOllamaRecheck::Failed(error_listing(None, Some(class)))
        }
        DiscoveryAnswer::BrokenBody(status) => {
            DiscoveryOllamaRecheck::Failed(error_listing(Some(status), Some("network")))
        }
    }
}

/// The ollama branch of the short-circuit: `/api/ps` (loaded state and
/// effective contexts) + `/api/tags` (the downloaded list) + per-model
/// `/api/show` `num_ctx` lookups for entries WITHOUT a context fact
/// (unloaded ones — loaded entries already carry the `/api/ps` truth). The
/// lookups are bounded at [`DISCOVER_OLLAMA_SHOW_CAP`] (taken from the FRONT
/// of the missing list) AND by the [`DISCOVER_OLLAMA_SHOW_BUDGET`]
/// wall-clock budget; either bound ending the sweep early sets `truncated`
/// (every downloaded model is still listed — the flag says the context facts
/// are partial, not that rows were dropped).
///
/// A tags round trip that fails at the transport/status level is re-checked
/// once ([`discovery_ollama_recheck_tags`], the GUI's `ollama_native_failure`
/// mirror — round-3 review M4a): a confirmed transient failure reports its
/// REAL classification instead of rerunning the compat ladder, which would
/// pin a busy Ollama as `unsupported(404)`; a healthy NON-EMPTY recheck
/// retries the native listing once (round-5 GUI parity for the round-3
/// m1-6 retry arm) — a retry that still fails falls back to the compat
/// ladder, and the retry never rechecks, so the loop is bounded at one.
fn discovery_ollama_models(base_url: &str, key: Option<&str>) -> DiscoveryOllamaListing {
    discovery_ollama_models_inner(base_url, key, true)
}

/// [`discovery_ollama_models`] minus the recheck arm: the retry path calls
/// this with `allow_recheck = false` so a still-failing retry falls back to
/// the compat ladder exactly like the GUI's "重试仍失败:回落阶梯" and the
/// failure/recheck cycle cannot loop.
fn discovery_ollama_models_inner(
    base_url: &str,
    key: Option<&str>,
    allow_recheck: bool,
) -> DiscoveryOllamaListing {
    const DISCOVER_OLLAMA_SHOW_CAP: usize = 50;
    let Some(client) = discovery_client() else {
        return DiscoveryOllamaListing::Failed(error_listing(None, None));
    };
    let host = strip_v1_suffix(base_url);
    // Loaded set and effective contexts: any failure reads as empty (all
    // unloaded, no window facts) without losing the downloaded list.
    let (loaded_names, ps_contexts) =
        match discovery_send(apply_bearer(client.get(format!("{host}/api/ps")), key)) {
            DiscoveryAnswer::Json(value, _) => (
                discovery_ollama_ps_names(&value),
                discovery_ollama_ps_contexts(&value),
            ),
            _ => Default::default(),
        };
    let value = match discovery_send(apply_bearer(client.get(format!("{host}/api/tags")), key)) {
        DiscoveryAnswer::Json(value, _) => value,
        // Round-3 review M4a: not "re-enter the compat ladder" — re-check
        // once and classify honestly (the GUI's ollama_native_failure).
        // Round-5 GUI parity: a healthy NON-EMPTY recheck retries the
        // native listing once; the retry runs without a second recheck, so
        // a still-failing retry falls back exactly like the GUI and the
        // fail/recheck cycle is bounded at one.
        _ if allow_recheck => {
            return match discovery_ollama_recheck_tags(base_url, key) {
                DiscoveryOllamaRecheck::Failed(listing) => DiscoveryOllamaListing::Failed(listing),
                DiscoveryOllamaRecheck::NotOllamaShaped => DiscoveryOllamaListing::NotOllamaShaped,
                DiscoveryOllamaRecheck::HealthyNonEmpty => {
                    match discovery_ollama_models_inner(base_url, key, false) {
                        DiscoveryOllamaListing::Listed(models, truncated) => {
                            DiscoveryOllamaListing::Listed(models, truncated)
                        }
                        // 重试仍失败:回落阶梯,与旧行为一致(瞬态不可硬钉)。
                        _ => DiscoveryOllamaListing::NotOllamaShaped,
                    }
                }
            };
        }
        _ => return DiscoveryOllamaListing::NotOllamaShaped,
    };
    let names = match discovery_ollama_tag_names(&value) {
        Some(names) if !names.is_empty() => names,
        _ => return DiscoveryOllamaListing::NotOllamaShaped,
    };
    let mut models: Vec<DiscoveredModel> = names
        .into_iter()
        .map(|name| {
            let loaded = loaded_names.contains(&name);
            let context = ps_contexts.get(&name).copied();
            DiscoveredModel {
                context_window_tokens: context,
                context_from: context.map(|_| ModelFactSource::Endpoint),
                id: name,
                max_output_tokens: None,
                output_from: None,
                loaded: Some(loaded),
                title: None,
            }
        })
        .collect();
    let missing: Vec<usize> = (0..models.len())
        .filter(|&index| models[index].context_window_tokens.is_none())
        .collect();
    let truncated = missing.len() > DISCOVER_OLLAMA_SHOW_CAP;
    // The sweep is bounded by the count cap AND a wall-clock budget: 50
    // serial 8s-timeout requests could legitimately stack into minutes for a
    // stalled-but-not-dead server, and a one-shot click must not wait that
    // out. Past the budget the sweep stops and the remainder is disclosed
    // through `truncated` — the same partial-facts semantics as the count
    // cap (the GUI's `OLLAMA_SHOW_BUDGET` check, mirrored per iteration).
    let started = std::time::Instant::now();
    for &index in missing.iter().take(DISCOVER_OLLAMA_SHOW_CAP) {
        if started.elapsed() >= DISCOVER_OLLAMA_SHOW_BUDGET {
            return DiscoveryOllamaListing::Listed(models, true);
        }
        let name = models[index].id.clone();
        if let Some(context) = discovery_ollama_query_show(client, &host, key, &name) {
            models[index].context_window_tokens = Some(context);
            models[index].context_from = Some(ModelFactSource::Endpoint);
        }
    }
    DiscoveryOllamaListing::Listed(models, truncated)
}

/// LM Studio's native REST listing → entries with `state`-derived loaded
/// facts. The only served-window fact read here is `loaded_context_length`
/// on a LOADED entry; `max_context_length` is the model's capability ("the
/// maximum the model supports"), the same fact class refused for Ollama's
/// GGUF training cap — adopting it would over-report whenever the load
/// override sits below the cap. `None` = shape not recognized (the caller
/// falls back to the OpenAI-compatible shape).
fn discovery_lmstudio_v0_models(value: &serde_json::Value) -> Option<Vec<DiscoveredModel>> {
    let data = value.get("data").and_then(serde_json::Value::as_array)?;
    let models = data
        .iter()
        .filter_map(|item| {
            let id = item.get("id").and_then(|value| value.as_str())?.trim();
            if id.is_empty() {
                return None;
            }
            let loaded = item
                .get("state")
                .and_then(|value| value.as_str())
                .map(|state| state == "loaded");
            let context = if loaded == Some(true) {
                item.get("loaded_context_length")
                    .and_then(discovery_positive_u32)
            } else {
                None
            };
            Some(DiscoveredModel {
                id: id.to_owned(),
                context_window_tokens: context,
                context_from: context.map(|_| ModelFactSource::Endpoint),
                max_output_tokens: None,
                output_from: None,
                loaded,
                title: None,
            })
        })
        .collect::<Vec<_>>();
    (!models.is_empty()).then_some(models)
}

/// The lmstudio ladder (short-circuit branch): the keyless native
/// `/api/v0/models` round first (loaded states — the GUI's
/// `probe_lmstudio_v0_only` sends no form credential either), and ANY
/// non-listing v0 outcome — a transport failure, ANY decided status
/// (404/405 legacy server, 401/403 authenticated, anything else), an
/// unusable or mid-stream-broken body, or a 2xx body that is not a usable
/// v0 shape — re-enters the KEYED local OpenAI-compatible ladder
/// ([`discovery_openai_listing_local`], the `{base}/models` +
/// `/v1/models`-on-404/405 mirror of the GUI's `ladder_openai_local`).
/// That is exactly the GUI's composition (round-4 review M5): its
/// `lmstudio_discovered` returns `None` on any v0 failure and its caller
/// falls through to the keyed compat ladder, so an authenticated LM Studio
/// LISTS there where this lane's old decided-status terminal reported
/// `auth_required` from the keyless v0 round.
fn discovery_lmstudio_listing(base_url: &str, key: Option<&str>) -> EndpointModelsListing {
    let Some(client) = discovery_client() else {
        return error_listing(None, None);
    };
    let host = strip_v1_suffix(base_url);
    let v0 = discovery_send(client.get(format!("{host}/api/v0/models")));
    if let DiscoveryAnswer::Json(value, _) = &v0
        && let Some(models) = discovery_lmstudio_v0_models(value)
    {
        return listed_listing(models, false);
    }
    // Every non-listing v0 outcome lands here (see the doc): the keyed
    // compat ladder decides, at ITS outcome's real classification — a
    // transport failure of the compat legs still surfaces as transport,
    // never laundered through the v0 round's facts.
    discovery_openai_listing_local(base_url, key)
}

/// The `server_kind` string `models discover` reports, in the GUI's kind
/// vocabulary (`endpoint_discovery::local_server_kind_str`, the same table
/// `monitor.rs` speaks): `lm_studio` with the underscore — not the
/// detector-internal `lmstudio` spelling the CLI's own kind probe computes
/// with — plus `tachyon`, which the CLI's `/v1/models` `owned_by` probe
/// detects just like the GUI's. Discover therefore reports the GUI's
/// vocabulary, not `models probe-local`'s detector-internal table (which
/// spells `lmstudio` without the underscore); the two probe-local honesty
/// sentinels are absence-of-a-classification, and the GUI's kind table
/// answers exactly that case with `generic`.
fn discovery_server_kind_str(kind: &'static str) -> &'static str {
    match kind {
        "lmstudio" => "lm_studio",
        "vllm" | "ollama" | "lm_studio" | "sglang" | "llamacpp" | "koboldcpp" | "lmdeploy"
        | "tachyon" | "dockermodelrunner" | "generic" => kind,
        LOCAL_KIND_UNKNOWN_AUTHENTICATED | LOCAL_KIND_PROBE_FAILED => "generic",
        _ => "generic",
    }
}

/// The full discovery ladder for one endpoint, mirroring the GUI's
/// `probe_endpoint_models`: the loopback/private short-circuit (native
/// ollama / lmstudio lanes, otherwise the OpenAI-compatible ladder) serves
/// the OpenAI-compatible protocol; an explicitly requested anthropic or
/// gemini protocol speaks its native ladder regardless of locality — a local
/// LiteLLM-style proxy exposing the anthropic wire must not be re-routed
/// onto an OpenAI-shaped answer just because it listens on 127.0.0.1.
///
/// Every error row carries only structured facts: a status number or a
/// transport class. The reqwest error text (URL and userinfo included) and
/// the api key never reach a `String` on this path at all.
fn run_endpoint_discovery(
    base_url: &str,
    api_key: Option<&str>,
    protocol: DiscoveryProtocol,
) -> EndpointModelsListing {
    let key = api_key.map(str::trim).filter(|key| !key.is_empty());
    match protocol {
        DiscoveryProtocol::Anthropic => discovery_anthropic_listing(base_url, key),
        DiscoveryProtocol::Gemini => discovery_gemini_listing(base_url, key),
        DiscoveryProtocol::OpenAiCompatible => {
            if !is_local_or_private_base_url(base_url) {
                return discovery_openai_listing(base_url, key);
            }
            // The local short-circuit belongs to the OpenAI-compatible
            // protocol only (the explicit native protocols above keep their
            // own ladders even on loopback). Every outcome of this branch —
            // including a fallback and the sentinels — carries the detected
            // server kind, translated into the GUI's kind vocabulary by
            // [`discovery_server_kind_str`].
            let kind = select_local_server_kind_from_hits(&probe_candidates(base_url.trim(), key));
            let mut listing = match kind {
                "ollama" => match discovery_ollama_models(base_url, key) {
                    DiscoveryOllamaListing::Listed(models, truncated) => {
                        listed_listing(models, truncated)
                    }
                    DiscoveryOllamaListing::Failed(listing) => listing,
                    DiscoveryOllamaListing::NotOllamaShaped => {
                        discovery_openai_listing_local(base_url, key)
                    }
                },
                "lmstudio" => discovery_lmstudio_listing(base_url, key),
                _ => discovery_openai_listing_local(base_url, key),
            };
            listing.server_kind = Some(discovery_server_kind_str(kind));
            listing
        }
    }
}

/// `models discover --base-url <url> [--api-key-env V] [--preset <name>]
/// [--wire <anthropic|openai|kimi|responses>] [--json]`: list the models an endpoint
/// reports, with the lengths the entries self-report. The GUI counterpart
/// (`probe_endpoint_models`) additionally back-fills missing lengths from
/// its model-length catalog; the CLI has no catalog, so `SOURCE` is always
/// `endpoint` and absent lengths simply print as `-`.
///
/// `--api-key-env` names an environment VARIABLE holding the credential,
/// the same discipline as probe-local — the keyless case is the flag's
/// omission, and the variable is read only here, at execution time.
///
/// A malformed `--base-url` is a usage error (argv is the caller's mistake,
/// the same classification `probe-local --url` applies). The protocol comes
/// from `--preset` (wins over `--wire`), else `--wire`, else the
/// OpenAI-compatible default. Exit codes: listed 0 (an empty list is still
/// a successful answer), the three failure statuses 1 with the status in
/// the output row, usage 2.
fn discover(
    base_url: &str,
    api_key_env: Option<&str>,
    preset: Option<ModelPreset>,
    wire: Option<DiscoveryProtocol>,
    json: bool,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    if reqwest::Url::parse(base_url.trim()).is_err() {
        return Err(CliError::usage(format!(
            "pinvou models discover: --base-url is not a valid url\n{DISCOVER_USAGE}"
        )));
    }
    // Usage validation stays ahead of credential resolution (the ordering
    // `probe_local` documents): a script keying on the exit-code contract
    // must see the usage error (2) even when the named variable is also
    // missing (which would be exit 1).
    let api_key = match api_key_env {
        Some(var) => Some(resolve_discover_api_key(var)?),
        None => None,
    };
    let protocol = resolve_discovery_protocol(preset, wire);
    let listing = run_endpoint_discovery(base_url, api_key.as_deref(), protocol);
    let output = if json { OutputMode::Json } else { output };
    Ok(render_discovery_outcome(&listing, output))
}

/// The name→value step of discover's `--api-key-env`, the same contract
/// `probe_local` Phase 2 enforces (its comment justifies each branch): a
/// set-but-non-UTF-8 variable is distinct from an unset one, and a
/// set-but-empty variable fails the command — `run_endpoint_discovery`
/// filters an empty key into an anonymous round, an authenticated endpoint
/// would answer 401, and the caller would get `auth_required` with no hint
/// that the credential they named never reached the request. The failures
/// are host failures (exit 1), never usage: the flag was spelled
/// correctly, the ENVIRONMENT is wrong. Only the variable NAME may appear
/// in a message — the value never does. The value itself is trimmed, the
/// same contract [`support::resolve_secret`] documents: environment secrets
/// routinely carry a trailing newline (`read KEY < key.txt`, a file mounted
/// by a CI secret store, most `.env` loaders), and handing it on verbatim
/// sends that `\n` inside the Authorization/x-api-key header — the ladder's
/// own downstream trim hides the bug for discover, but the returned value
/// must already BE the credential, like the resolver it mirrors.
fn resolve_discover_api_key(var: &str) -> Result<String, CliError> {
    let value = std::env::var(var).map_err(|error| match error {
        std::env::VarError::NotUnicode(_) => CliError::failed(format!(
            "discover: api key environment variable {var} is set but not valid UTF-8"
        )),
        _ => CliError::failed(format!(
            "discover: api key environment variable {var} is not set"
        )),
    })?;
    // The empty check stays on the raw value (same order as
    // `resolve_secret`): a whitespace-only variable is named as empty, not
    // trimmed into a credential.
    if value.trim().is_empty() {
        return Err(CliError::failed(format!(
            "discover: api key environment variable {var} is set but empty"
        )));
    }
    Ok(value.trim().to_owned())
}

/// One human table cell for an untrusted endpoint-provided value: control
/// characters collapsed (the same hygiene every other human row applies)
/// and the column separator neutralized so a hostile model id cannot forge
/// extra columns. JSON keeps the verbatim value.
fn discovery_table_cell(value: &str) -> String {
    collapse_control_characters(value).replace('|', "/")
}

/// K/M formatting for the CONTEXT / OUTPUT table cells (decimal, like the
/// modern token-window convention: 128K = 128000): values under 1000 print
/// as-is, larger values one decimal with the trailing `.0` trimmed
/// (`8192` → `8.2K`, `128000` → `128K`, `1000000` → `1M`). A rounding carry
/// at a unit boundary steps up (`999950` → `1M`).
fn format_tokens_km(value: u32) -> String {
    let (divisor, suffix) = if value >= 1_000_000_000 {
        (1_000_000_000.0_f64, "G")
    } else if value >= 1_000_000 {
        (1_000_000.0_f64, "M")
    } else if value >= 1_000 {
        (1_000.0_f64, "K")
    } else {
        return value.to_string();
    };
    let rounded = (value as f64 / divisor * 10.0).round() / 10.0;
    if rounded >= 1000.0 {
        return match suffix {
            "K" => "1M".to_owned(),
            "M" => "1G".to_owned(),
            _ => format!("{rounded:.0}{suffix}"),
        };
    }
    let text = format!("{rounded:.1}");
    let text = text.strip_suffix(".0").map(str::to_owned).unwrap_or(text);
    format!("{text}{suffix}")
}

fn format_tokens_cell(value: Option<u32>) -> String {
    value.map_or_else(|| "-".to_owned(), format_tokens_km)
}

/// The human rendering of a listing: the table for a listed endpoint, the
/// status row + hint for every failure (the same shape the probe outcomes
/// print — the JSON row is the parseable source of truth either way).
fn render_discovery_human(listing: &EndpointModelsListing) -> String {
    if listing.status != ListingStatus::Listed {
        let mut lines = vec![format!("status: {}", listing.status.as_str())];
        if let Some(http_status) = listing.http_status {
            lines.push(format!("http_status: {http_status}"));
        }
        if let Some(transport) = listing.transport {
            lines.push(format!("transport: {transport}"));
        }
        if let Some(server_kind) = listing.server_kind {
            lines.push(format!("server_kind: {server_kind}"));
        }
        lines.push(match listing.status {
            ListingStatus::AuthRequired => {
                "hint: the endpoint rejected the request as unauthenticated; rerun with \
                 --api-key-env V"
                    .to_owned()
            }
            ListingStatus::Unsupported => {
                "hint: the endpoint did not answer with a recognized model-list protocol".to_owned()
            }
            _ => "hint: the endpoint could not be reached or failed; see transport or \
                  http_status above"
                .to_owned(),
        });
        return lines.join("\n");
    }
    let mut lines = vec!["ID | CONTEXT | OUTPUT | SOURCE".to_owned()];
    if listing.models.is_empty() {
        lines.push("the endpoint returned no models".to_owned());
    }
    for model in &listing.models {
        lines.push(format!(
            "{} | {} | {} | {}",
            discovery_table_cell(&model.id),
            format_tokens_cell(model.context_window_tokens),
            format_tokens_cell(model.max_output_tokens),
            ModelFactSource::Endpoint.as_str(),
        ));
    }
    // Cause-neutral on purpose: `truncated` covers several bounds (the
    // OpenAI-compatible entry cap, the native lanes' 2000-entry/3-page
    // caps, the Ollama show-lookup budget) — naming one cause would lie
    // about the others. The JSON row's `truncated: true` is the contract;
    // the note only discloses that it fired.
    if listing.truncated {
        lines.push("note: the listing is truncated".to_owned());
    }
    lines.join("\n")
}

/// One rendered `models discover` outcome: listed exits 0, every failure
/// status still prints its structured row but exits 1 — the exact style the
/// probe outcomes render in.
fn render_discovery_outcome(listing: &EndpointModelsListing, output: OutputMode) -> CliOutcome {
    let stdout = render(output, render_discovery_human(listing), &listing.to_json());
    CliOutcome {
        exit_code: if listing.status == ListingStatus::Listed {
            ExitCode::Success
        } else {
            ExitCode::Failed
        },
        stdout,
    }
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
            // Reachable through the settings getters: a settings.json holding
            // the legacy-field `"auto"` literal (a hand edit, or a file
            // written by this PR's own dev builds) parses leniently to
            // `Some(Auto)`, and reporting "auto" is the truth. First-party
            // writers never store Auto in the legacy field (the effective
            // getter reports the companion flag instead).
            SerializableMode::Auto => "auto".to_owned(),
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
fn settings_get<S: CredentialStore>(
    store: &S,
    key: Option<SettingsKey>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let Some(key) = key else {
        let prefs = safe_prefs(store);
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
    // Round-48 review: the keyed arm renders only the scalar — it never
    // prints `credential_state`/`has_secret` — so the all-models keychain
    // refresh `safe_prefs` performs (one `get` per stored credential, plus
    // one per search credential) buys nothing here and can pop one keychain
    // consent dialog per configured model on an ad-hoc macOS build, to print
    // one boolean. The module's own `plain_prefs` doctrine (round-39/40)
    // names exactly this call shape. The keyless dump arm above keeps
    // `safe_prefs`: it does render `credential_state`.
    let prefs = plain_prefs();
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
        // Effective-mode contract: the companion `*_auto` flag wins over the
        // legacy field (the desktop resolves the same pair the same way), so
        // an explicit Auto choice reports `auto` instead of a stale legacy
        // spelling.
        SettingsKey::ModeDefaultsWork => {
            let value = if prefs.mode_defaults.work_auto {
                "auto".to_owned()
            } else {
                mode_str(prefs.mode_defaults.work)
            };
            (value.clone(), serde_json::json!(value))
        }
        SettingsKey::CodePermissionLastMode => {
            let value = if prefs.code_permission.last_mode_auto {
                "auto".to_owned()
            } else {
                mode_str(prefs.code_permission.last_mode)
            };
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
    let key_name = key.name();
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
    // The language-arm note below needs the PRE-save memory_enabled (the
    // locale policy may revert it during this very save). It's captured at
    // the top of the transaction closure — reading it with a second
    // `UserPrefs::load()` would be the persisting variant, the exact
    // keyring-write side effect this command's round-37 note rework removed.
    let mut memory_enabled_before = false;
    let saved = UserPrefs::update_transaction(|prefs| {
        memory_enabled_before = prefs.memory_enabled;
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
            // Lane-default writes follow the desktop `set_mode_default` pair
            // contract: the settings.json lane vocabulary never stores the
            // `auto` literal. An explicit Auto choice stores the `*_auto`
            // companion flag with a None legacy value; every other choice -
            // including the `none` clear - stores the legacy value and
            // CLEARS the companion, so a retained stale flag can no longer
            // mask this write when the desktop resolves the effective mode.
            (SettingsKey::ModeDefaultsWork, SettingsValue::Mode(v)) => {
                let is_auto = v == Some(SerializableMode::Auto);
                prefs.mode_defaults.work = if is_auto { None } else { v };
                prefs.mode_defaults.work_auto = is_auto;
            }
            (SettingsKey::CodePermissionLastMode, SettingsValue::Mode(v)) => {
                let is_auto = v == Some(SerializableMode::Auto);
                prefs.code_permission.last_mode = if is_auto { None } else { v };
                prefs.code_permission.last_mode_auto = is_auto;
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
    let note = match (&key, &value) {
        (SettingsKey::MemoryEnabled, SettingsValue::Bool(requested)) => {
            let effective = saved.memory_enabled;
            (effective != *requested).then(|| {
                // No "note: " prefix here: the human renderer adds it for
                // stderr-adjacent display, and in JSON the sentence lands in a
                // field already NAMED `note` — the old shape serialized
                // `"note": "note: ..."` (round-37 review).
                format!(
                    "the memory locale policy kept memory_enabled = {effective} (memory \
                     features require the zh-Hans UI language)"
                )
            })
        }
        // Round-41 review: the same policy silently reverts memory_enabled
        // when THIS write sets a non-zh-Hans language — disclose it exactly
        // like the direct `memory_enabled` key instead of a plain success.
        (SettingsKey::Language, _) => (memory_enabled_before && !saved.memory_enabled).then(|| {
            "the memory locale policy kept memory_enabled = false (memory \
                 features require the zh-Hans UI language)"
                .to_owned()
        }),
        _ => None,
    };
    let key_name = key.name();
    let human = match &note {
        Some(note) => format!("{key_name} updated\nnote: {note}"),
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

fn search_list<S: CredentialStore>(store: &S, output: OutputMode) -> Result<CliOutcome, CliError> {
    let prefs = safe_prefs(store);
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

/// `settings search set --provider P [--api-key-env V | --clear [--yes]]`:
/// stores or clears provider P's credential with the same bookkeeping
/// (`mark_configured` / `mark_missing`) as the GUI's search settings save
/// path.
///
/// P becomes the ACTIVE search provider only when the caller is selecting it
/// — that is, on every form except `--clear`. Clearing is a credential
/// operation on a named provider and must not move the user's search
/// backend; `--provider tavily --clear` means "forget the tavily key", not
/// "search with tavily from now on".
///
/// `--clear` deletes the stored credential, so it requires `--yes` exactly
/// like the model family's credential deletions (`models edit
/// --clear-api-key`) and `models remove` (`require_yes`).
fn search_set<S: CredentialStore>(
    store: &S,
    provider: SearchProvider,
    api_key_env: &Option<String>,
    clear: bool,
    yes: bool,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    if provider == SearchProvider::Bing && api_key_env.is_some() {
        return Err(CliError::usage(
            "provider bing does not use an api key; it needs no configuration",
        ));
    }
    if clear {
        require_yes(yes)?;
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
    // clear commits against. The reference travels to the post-commit hook
    // through a `RefCell` (both closures are `FnOnce` over the same captured
    // environment, exactly like `models remove`): round-49 review moved the
    // delete inside `update_transaction_with_post_commit`'s hook, so it runs
    // right after the save and BEFORE the settings flock releases — a
    // concurrent replacement of the same deterministic reference is then
    // totally ordered against it instead of racing a post-lock delete.
    // `cleared_stored_reference` keeps the "was there actually something to
    // clear" bit the receipt below reports (the hook consumes the reference).
    let deferred_delete = std::cell::RefCell::new(Option::<CredentialReference>::None);
    let cleared_stored_reference = std::cell::Cell::new(false);
    // Replacing an existing key OVERWRITES it in the keyring, so the
    // rollback below must restore the previous value — deleting would
    // destroy the old secret while prefs still references it (strictly
    // worse than not rolling back). Snapshot the previous value INSIDE the
    // transaction closure, immediately before the overwrite and exactly
    // like `models edit`: only the in-transaction snapshot is guaranteed to
    // name the value this command is about to overwrite — a snapshot taken
    // before the critical section could roll a concurrent writer's rotated
    // secret back after a failed save. A snapshot READ ERROR stays distinct
    // from "no previous secret": rolling back on unknown state must not
    // delete a key that may still exist and still be referenced by prefs.
    let mut previous_secret = None;
    let transaction = UserPrefs::update_transaction_with_post_commit(
        |prefs| {
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
                previous_secret = Some(store.get(&reference));
                // Normalized exactly like `models add` and the GUI
                // (`platform/prefs` `apply_model_credential`). This call site
                // used to pass `key` verbatim, so a `--api-key-env` secret with
                // a trailing newline was stored with it and every search request
                // 401'd while `settings search test` still reported
                // `configured` — see `secret_for_storage`.
                store
                    .set(&reference, secret_for_storage(key))
                    .map_err(|error| error.user_message())?;
                prefs
                    .search
                    .credentials
                    .entry(provider)
                    .or_default()
                    .mark_configured(reference);
            }
            if clear && let Some(credential) = prefs.search.credentials.get_mut(&provider) {
                let reference = credential.credential_ref.clone();
                cleared_stored_reference.set(reference.is_some());
                *deferred_delete.borrow_mut() = reference;
                credential.mark_missing();
            }
            Ok(())
        },
        // Runs only after `save_unlocked` succeeded, still under the prefs
        // lock — deleting first would leave the prefs entry pointing at a
        // credential that no longer exists if the save fails (a leftover
        // keyring entry is the benign direction). The same benign direction
        // applies on the way out: the prefs entry is already cleared, so a
        // keyring deletion failure warns and succeeds like `models remove`,
        // instead of reporting a failure whose only remedy (rerun) has
        // nothing left to do.
        || {
            delete_after_commit(
                store,
                deferred_delete.borrow_mut().take(),
                "credential cleared from settings, but the keyring entry could not be deleted",
            );
        },
    );
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
            // (`stored` is storage-normalized exactly like the write above;
            // `previous_secret` is None when nothing was stored.)
            if let Some(reference) = stored_reference.as_ref() {
                rollback_keyring_write(
                    store,
                    reference,
                    stored
                        .as_ref()
                        .map(|key| secret_for_storage(key).to_owned()),
                    previous_secret.map(|previous| previous.map_err(|_| ())),
                );
            }
            return Err(prefs_error(error));
        }
    };
    let had_reference = cleared_stored_reference.get();
    let action = if clear {
        if had_reference {
            "cleared"
        } else {
            // Round-41 review: a keyless provider (bing) or a never-configured
            // one has no stored credential — reporting "cleared" was a
            // success-shaped no-op, against the same doctrine that refuses
            // empty edits elsewhere in this file.
            "unchanged"
        }
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
/// contract now is: `ok: true` means the provider answered with a body its
/// own search path accepts — the HTTP status AND, for the providers that
/// report business errors in-band on a 200 (Metaso, Bocha, Baidu), the
/// body's error code ([`search_body_error`], replicated from the
/// web-search tool's per-provider checks); Bing additionally requires a
/// result-shaped body (a bare 2xx consent/captcha page stays `ok: false`)
/// — and the `verified` field says which of the two was actually
/// established.
///
/// The four API lanes are built from the request shapes the product's own
/// search tool sends (`CodeWhale/crates/tui/src/tools/web_search.rs`), not
/// from an invented API contract — see [`search_api_request_at`].
fn search_test<S: CredentialStore>(
    store: &S,
    provider: SearchProvider,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let probe = if provider == SearchProvider::Bing {
        run_bing_probe()
    } else {
        match resolve_search_key(store, provider) {
            // The resolver's contract: a returned key is already trimmed and
            // non-empty.
            Ok(Some(key)) => {
                let Some(endpoint) = search_api_endpoint(provider) else {
                    unreachable!("Bing is routed to `run_bing_probe` before the api-key lane")
                };
                run_search_api_probe_at(provider, &key, endpoint)
            }
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
    Ok(CliOutcome {
        exit_code: if probe.ok {
            ExitCode::Success
        } else {
            ExitCode::Failed
        },
        stdout: render(output, human, &json),
    })
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

impl SearchProbe {
    /// Adapts one answered request's `models test`-style connection
    /// classification to the search lane: the row copies the connection
    /// probe's `ok`/`code`/`detail` verbatim (the same status vocabulary, so
    /// one exit contract covers both commands) and records that a request
    /// WAS attempted and answered.
    fn from_live(probe: ConnectionProbe) -> Self {
        Self {
            ok: probe.ok,
            code: probe.code,
            detail: probe.detail,
            verified: VERIFIED_LIVE_PROBE,
        }
    }

    /// The row for a probe that could not run at all: the HTTP client would
    /// not build, so no request was sent and nothing about the provider was
    /// learned.
    fn client_error() -> Self {
        Self {
            ok: false,
            code: "client_error",
            detail: None,
            verified: VERIFIED_NOTHING,
        }
    }
}

/// The search endpoints the product actually calls, copied from
/// `CodeWhale/crates/tui/src/tools/web_search.rs` (`TAVILY_ENDPOINT`,
/// `BOCHA_ENDPOINT`, `BAIDU_ENDPOINT`) and from `run_metaso_search`'s
/// `{METASO_ENDPOINT}/search` composition. They are duplicated
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

/// The pinned endpoint each provider's live probe targets, mirroring the
/// request builders in `CodeWhale/crates/tui/src/tools/web_search.rs`. `None`
/// for Bing: it has no api-key form and is routed to the scrape probe instead.
fn search_api_endpoint(provider: SearchProvider) -> Option<&'static str> {
    match provider {
        SearchProvider::Tavily => Some(TAVILY_SEARCH_ENDPOINT),
        SearchProvider::Bocha => Some(BOCHA_SEARCH_ENDPOINT),
        SearchProvider::Metaso => Some(METASO_SEARCH_ENDPOINT),
        SearchProvider::Baidu => Some(BAIDU_SEARCH_ENDPOINT),
        SearchProvider::Bing => None,
    }
}

/// Builds one provider's real search request. Every shape here — endpoint,
/// auth scheme and body — is transcribed from the corresponding builder in
/// `CodeWhale/crates/tui/src/tools/web_search.rs`: Tavily carries the key in
/// the JSON body (`api_key`), Bocha, Metaso and Baidu carry it as
/// `Authorization: Bearer <key>`. Bing has no API-key form and never reaches
/// this function.
///
/// The endpoint is a parameter (rather than read from the pinned consts via
/// [`search_api_endpoint`]) so the loopback unit tests in this module can
/// drive the same request lane without network access.
fn search_api_request_at(
    client: &reqwest::blocking::Client,
    provider: SearchProvider,
    key: &str,
    endpoint: &str,
) -> Option<reqwest::blocking::RequestBuilder> {
    let request = match provider {
        SearchProvider::Bing => return None,
        SearchProvider::Tavily => client.post(endpoint).json(&serde_json::json!({
            "api_key": key,
            "query": SEARCH_PROBE_QUERY,
            "search_depth": "basic",
            "max_results": SEARCH_PROBE_RESULTS,
        })),
        SearchProvider::Bocha => client
            .post(endpoint)
            .bearer_auth(key)
            .json(&serde_json::json!({
                "query": SEARCH_PROBE_QUERY,
                "freshness": "noLimit",
                "count": SEARCH_PROBE_RESULTS,
            })),
        SearchProvider::Metaso => client
            .post(endpoint)
            .bearer_auth(key)
            .json(&serde_json::json!({
                "q": SEARCH_PROBE_QUERY,
                "scope": "webpage",
                "size": SEARCH_PROBE_RESULTS,
            })),
        SearchProvider::Baidu => client
            .post(endpoint)
            .bearer_auth(key)
            .json(&serde_json::json!({
                "messages": [{ "role": "user", "content": SEARCH_PROBE_QUERY }],
                "search_source": "baidu_search_v2",
                "resource_type_filter": [{ "type": "web", "top_k": SEARCH_PROBE_RESULTS }],
            })),
    };
    Some(request)
}

/// Sends one API provider's search probe against the given endpoint and
/// classifies the answer with the SAME status vocabulary `models test` uses
/// (`connection_http_result`: `auth_invalid` for 401, `auth_forbidden` for
/// 403, `rate_limited` for 429, ...), so one exit contract covers both
/// commands. Transport failures go through `connection_error_result`, which
/// redacts the message.
///
/// The endpoint is a parameter (rather than read from the pinned consts via
/// [`search_api_endpoint`]) so the loopback unit tests in this module can
/// drive the whole request→classification lane through `spawn_probe_mock`
/// without network access — including the canned-body cases that cannot be
/// pointed at a real provider's 200-with-error-code answer.
fn run_search_api_probe_at(provider: SearchProvider, key: &str, endpoint: &str) -> SearchProbe {
    let Some(client) = search_probe_client() else {
        return SearchProbe::client_error();
    };
    let Some(request) = search_api_request_at(client, provider, key, endpoint) else {
        unreachable!("Bing is routed to `run_bing_probe` before the api-key lane")
    };
    match request.send() {
        Ok(response) => search_response_probe(provider, response),
        Err(error) => SearchProbe::from_live(connection_error_result(&error)),
    }
}

/// Classifies one answered search probe: the HTTP status first (same
/// vocabulary as `models test`), then — on a 2xx — the response BODY, because
/// Metaso, Bocha and Baidu answer `200 OK` with a business error code in the
/// body when the key is rejected or the quota is gone. See
/// [`search_body_error`] for the source of those checks.
fn search_response_probe(
    provider: SearchProvider,
    response: reqwest::blocking::Response,
) -> SearchProbe {
    let status = response.status();
    let probe = connection_http_result(status);
    if !probe.ok {
        return SearchProbe::from_live(probe);
    }
    // The bounded-IO rule every other probe read follows (`read_json_capped`):
    // the timeout bounds time, but a hostile endpoint can stream within it, so
    // the body is read through [`PROBE_BODY_CAP_BYTES`] instead of an
    // unbounded `text()`.
    let body = match read_body_capped(response, PROBE_BODY_CAP_BYTES) {
        // The status says success but the body never arrived: the probe
        // learned the endpoint is reachable, not that it answered cleanly.
        // (The timeout split is a discovery-lane distinction; the search
        // probe reports both as `response_unreadable`.)
        BodyRead::Unreadable | BodyRead::UnreadableTimeout => {
            return SearchProbe {
                ok: false,
                code: "response_unreadable",
                detail: Some(format!(
                    "failed to read the {} response body",
                    provider.as_str()
                )),
                verified: VERIFIED_LIVE_PROBE,
            };
        }
        // Over-cap is its own honest failure, not a parse of the truncation.
        BodyRead::TooLarge => {
            return SearchProbe {
                ok: false,
                code: "response_too_large",
                detail: Some(format!(
                    "the {} response body exceeded the probe cap ({PROBE_BODY_CAP_BYTES} bytes)",
                    provider.as_str()
                )),
                verified: VERIFIED_LIVE_PROBE,
            };
        }
        // Same decoding `text()` applies: invalid UTF-8 degrades to the
        // replacement character instead of failing the probe.
        BodyRead::Bytes(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
    };
    match search_body_error(provider, &body) {
        Some((code, detail)) => SearchProbe {
            ok: false,
            code,
            detail: Some(detail),
            verified: VERIFIED_LIVE_PROBE,
        },
        None => SearchProbe::from_live(probe),
    }
}

/// Classifies a 2xx search response body by the provider's own business
/// error conventions.
///
/// SOURCE OF TRUTH: these are the minimal per-provider body checks the
/// product's web-search path applies to the SAME bodies, in
/// `CodeWhale/crates/tui/src/tools/web_search.rs` — Metaso's inline
/// `code != 0` check in `run_metaso_search` (2005 = key rejected, 3003 =
/// daily limit), `bocha_error_message` (fail on any code other than 0/200),
/// and `baidu_error_message` (fail on `error_code`/`code` other than 0).
/// They are replicated here rather than imported because those items are
/// private to the TUI crate the CLI does not depend on; the probes and the
/// search tool must classify these bodies identically or `settings search
/// test` passes keys the product's own search rejects. Tavily carries no
/// business error code on 2xx, matching `run_tavily_search`, which parses
/// the body without an error-code check.
///
/// Returns `(probe code, truthful detail)` for a body that means failure.
fn search_body_error(provider: SearchProvider, body: &str) -> Option<(&'static str, String)> {
    // A 2xx body that is not JSON at all (a captive portal or an HTML error
    // interstitial served with 200) must not pass as "no business error":
    // for the three providers with declared JSON conventions an
    // unparseable body means the probe cannot have reached the real API —
    // the same failure class the unreadable-body arm above reports. Tavily
    // carries no error-code convention, so its body stays unjudged here.
    let parsed: serde_json::Value = match serde_json::from_str(body) {
        Ok(value) => value,
        Err(_) if provider == SearchProvider::Tavily => return None,
        Err(_) => {
            return Some((
                "response_unreadable",
                "the 2xx body is not JSON (expected the provider's JSON envelope)".to_owned(),
            ));
        }
    };
    match provider {
        SearchProvider::Tavily => None,
        SearchProvider::Metaso => {
            let code = parsed.get("code").and_then(|value| value.as_i64())?;
            if code == 0 {
                return None;
            }
            // The provider controls this text, so it can echo the presented
            // key back — redact like every other remote-text render in this
            // module (round-38 review).
            let message = parsed
                .get("message")
                .and_then(|value| value.as_str())
                .map(redact_secret)
                .unwrap_or_else(|| "unknown error".to_owned());
            let message = message.as_str();
            Some(match code {
                2005 => (
                    "key_rejected",
                    format!("Metaso API key rejected (code {code}: {message})"),
                ),
                3003 => (
                    "quota_exhausted",
                    format!("Metaso daily search limit reached (code {code}: {message})"),
                ),
                _ => (
                    "provider_error",
                    format!("Metaso API error (code {code}: {message})"),
                ),
            })
        }
        SearchProvider::Bocha => {
            let code = parsed.get("code").and_then(|value| value.as_i64())?;
            if code == 0 || code == 200 {
                return None;
            }
            let message = parsed
                .get("msg")
                .or_else(|| parsed.get("message"))
                .and_then(|value| value.as_str())
                .map(redact_secret)
                .unwrap_or_else(|| "unknown error".to_owned());
            Some((
                "provider_error",
                format!("Bocha search API error (code {code}: {})", message),
            ))
        }
        SearchProvider::Baidu => {
            let code = parsed
                .get("error_code")
                .or_else(|| parsed.get("code"))
                .and_then(|value| value.as_i64())?;
            if code == 0 {
                return None;
            }
            let message = parsed
                .get("error_msg")
                .or_else(|| parsed.get("message"))
                .and_then(|value| value.as_str())
                .map(redact_secret)
                .unwrap_or_else(|| "unknown error".to_owned());
            Some((
                "provider_error",
                format!("Baidu search API error (code {code}: {})", message),
            ))
        }
        SearchProvider::Bing => None,
    }
}

fn run_bing_probe() -> SearchProbe {
    // Same lane client as the API providers: `settings search test` is one
    // command, so its Bing and API lanes share one (15 s) timeout and one
    // connection pool instead of the 8 s per-request client this lane used
    // to build on every call.
    let Some(client) = search_probe_client() else {
        return SearchProbe::client_error();
    };
    match client
        .get("https://www.bing.com/search")
        .query(&[("q", SEARCH_PROBE_QUERY)])
        .send()
    {
        Ok(response) => {
            let status = response.status();
            // Scope note for the module's "body its own search path accepts"
            // contract: Bing needs no key and serves consent/captcha pages
            // with 2xx, so the HTTP status alone cannot prove searchability.
            // The body must at least carry a result-shaped element; a bare
            // 2xx page without one stays unverified.
            // Bounded like every other probe body (`read_json_capped`): the
            // 15s client timeout bounds time, not bytes, so a hostile or
            // hijacked endpoint must not stream unbounded into memory before
            // the shape check runs. Over-cap degrades to unshaped, same as a
            // read failure.
            let body_is_result_shaped = status.is_success()
                && match read_body_capped(response, PROBE_BODY_CAP_BYTES) {
                    BodyRead::Bytes(bytes) => body_carries_result_element(&bytes),
                    BodyRead::Unreadable | BodyRead::UnreadableTimeout | BodyRead::TooLarge => {
                        false
                    }
                };
            let (ok, code) = if body_is_result_shaped {
                (true, "ok")
            } else {
                (false, "http_error")
            };
            SearchProbe {
                ok,
                code,
                detail: Some(format!("HTTP {}", status.as_u16())),
                verified: VERIFIED_LIVE_PROBE,
            }
        }
        Err(error) => SearchProbe::from_live(connection_error_result(&error)),
    }
}

/// The Bing result-page shape check, extracted so the needle stays pinned by
/// a unit test: the marker is the 17-byte fragment `<li class="b_algo`
/// (element opener + class attribute, NO closing quote), while the shipped
/// code compared 17-byte windows against the 18-byte quote-terminated
/// literal — a length-mismatched slice comparison is constant-false, which
/// classified every genuine result page as `http_error`. The fragment ends
/// at the class value on purpose: real pages append further classes
/// (`"b_algo h-..."`) or the bare quote (`"b_algo"`), so only the shared
/// prefix is a stable needle.
fn body_carries_result_element(bytes: &[u8]) -> bool {
    const RESULT_MARKER: &[u8] = b"<li class=\"b_algo";
    bytes
        .windows(RESULT_MARKER.len())
        .any(|window| window == RESULT_MARKER)
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
fn resolve_search_key<S: CredentialStore>(
    store: &S,
    provider: SearchProvider,
) -> Result<Option<String>, String> {
    for name in provider.env_key_names() {
        if let Ok(value) = std::env::var(name) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Ok(Some(trimmed.to_owned()));
            }
        }
    }
    // Disclosed side effect (same one the `settings set` comment at
    // `settings_set` documents): `load()` is the persisting variant, so this
    // read lane can run the (idempotent) migrations and rewrite
    // settings.json on the way to a verdict. The prefs layer exposes no
    // read-only load that skips the migrations, only one that skips the
    // save — so the honest state is this disclosure, not a pretense.
    let prefs = UserPrefs::load();
    // Deliberately NO refresh_credential_states_with_store here: it performs
    // a keychain `get` per configured provider (double keychain I/O on the
    // target, N pointless reads on the others) and this function re-reads
    // the reference's value itself anyway — every refresh outcome yields the
    // identical verdict on the path below (cleared ref -> None; kept ref ->
    // the explicit get decides), so the extra round trips bought nothing.
    let Some(credential) = prefs.search.credentials.get(&provider) else {
        return Ok(None);
    };
    let Some(reference) = credential.credential_ref.clone() else {
        return Ok(None);
    };
    // A keychain failure is a credential-store problem, not "no key":
    // swallowing it here would degrade every signed request into a
    // misleading no_api_key report.
    let value = store
        .get(&reference)
        .map_err(|error| error.user_message())?
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    // Round-50 review: the same honest-miss rule the model lanes apply via
    // `resolve_saved_model_key_honest` — under the file-fallback valves a
    // `get` miss cannot distinguish "never stored" from "stored in an OS
    // keyring this process cannot reach", and reporting the false
    // `no_api_key` verdict here would hand a script a state it might act on
    // (this lane's own suggested repair, `settings search set`, overwrites
    // the same deterministic reference). The caller maps this error to the
    // `credential_unavailable` probe row like every other lane.
    honest_reference_get(store, &reference, value, || {
        format!(
            "the OS keyring holding the {} search key is unreachable from this process; the stored key cannot be verified here",
            provider.as_str()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinvou3_lib::platform::credential_store::CredentialError;

    // The credential-lane tests all steer the product through the
    // process-wide environment (`PINVOU3_HOME`, the `--api-key-env`
    // variable), so they run one at a time under this lock — the same rule
    // the integration suites' `ENV_LOCK` enforces. Each test takes the lock
    // itself and declares the guard BEFORE its `TempHome`/`TempEnv` guards,
    // so the fixture restores the environment while the lock is still held.
    //
    // This is the lib test binary's SHARED env lock (the gaia test in
    // `lib.rs` holds the same one through its `TempHome` guard), not a
    // models-local static: a per-module lock would serialize inside this
    // module and still race the gaia fixture's `PINVOU3_HOME` swaps.
    use crate::support::{ENV_LOCK, test_support::TempHome};

    #[test]
    fn transport_error_scrubs_url_userinfo_before_the_report() {
        // Round-44 review pin: reqwest's Display embeds the full URL,
        // userinfo included, and the scrubber must run before the probe
        // report is built. A free-form stored base_url with userinfo that
        // fails on the transport layer would otherwise echo `user:pass`
        // into the report (and, on the scheduled/memory organize lanes,
        // into the durable error strings). Port 1 is refused locally, so
        // the test stays hermetic; the timeout bounds any platform whose
        // refusal is slower.
        let error = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .expect("client")
            .get("http://user:pass@127.0.0.1:1/v1")
            .send()
            .expect_err("connecting to port 1 must fail");
        let probe = connection_error_result(&error);
        let detail = probe.detail.unwrap_or_default();
        assert!(
            !detail.contains("user:pass"),
            "the transport detail must not carry the userinfo: {detail}"
        );
    }

    #[test]
    fn non_json_2xx_body_fails_the_search_probe_for_json_convention_providers() {
        // Round-36 review minor: a captive portal or an HTML interstitial
        // served with 200 used to parse as "no business error" and pass the
        // probe; for the three providers with declared JSON conventions it
        // must fail as response_unreadable, while Tavily (no error-code
        // convention) stays unjudged.
        let portal = "<html>please log in to the wifi</html>";
        for provider in [
            SearchProvider::Metaso,
            SearchProvider::Bocha,
            SearchProvider::Baidu,
        ] {
            let (code, _) = search_body_error(provider, portal)
                .unwrap_or_else(|| panic!("{provider:?}: a non-JSON body must fail the probe"));
            assert_eq!(code, "response_unreadable");
        }
        assert!(search_body_error(SearchProvider::Tavily, portal).is_none());
        // A well-formed envelope still classifies by its business code.
        assert!(search_body_error(SearchProvider::Metaso, r#"{"code":0}"#).is_none());
    }

    /// The bing shape needle is pinned at its real length: the historical
    /// `windows(17)` against the 18-byte `<li class="b_algo"` marker made
    /// the comparison constant-false, so every genuine result page was
    /// reported as `http_error` with exit 1. The non-ignored unit test keeps
    /// the live-lane check honest (the endpoint test itself stays
    /// `#[ignore]` — no network in CI).
    #[test]
    fn bing_shape_check_matches_a_real_result_page() {
        let page = b"<html><body><ul><li class=\"b_algo\"><h2>Result</h2></li></ul></body></html>";
        assert!(body_carries_result_element(page));
        // Attribute-bearing variants share the leading fragment.
        assert!(body_carries_result_element(b"<li class=\"b_algo h-4\">"));
        // Consent/captcha pages (2xx, no result element) stay unshaped, as
        // do hostile near-misses.
        assert!(!body_carries_result_element(
            b"<html>please accept cookies</html>"
        ));
        assert!(!body_carries_result_element(b"<li class=\"b_alg\">"));
        assert!(!body_carries_result_element(b""));
        // The plain quote-terminated form (the original needle's shape) and
        // a needle-sized body, the boundary the old windows(17) missed.
        assert!(body_carries_result_element(b"<li class=\"b_algo\""));
        assert!(body_carries_result_element(b"<li class=\"b_algo"));
    }

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
    /// The trailing newline is the case that mattered: `support::resolve_secret`
    /// trims BOTH of its lanes (`--api-key-env` and stdin — environment
    /// secrets routinely carry a trailing newline from `read KEY < key.txt`, a
    /// CI secret store mount, or a `.env` loader), and a value that dodged
    /// that trim 401s every request while `settings search test` — which
    /// trims on read — still reports the provider as `configured`.
    /// The `models test` probe must carry the `x-opencode-session` header on
    /// OpenCode gateway base URLs (the Go gateway 400s `MissingSessionID`
    /// without it — the GUI probe this lane mirrors attaches it) and must
    /// NOT carry it anywhere else. Header value parity with the GUI comes
    /// from the shared deterministic derivation (`opencode_session_id_for`
    /// over the same `connection-test` key), so this pin is shape + presence;
    /// the value derivation is pinned app-side. Building the request never
    /// sends it.
    #[test]
    fn connection_probe_carries_the_opencode_session_header_on_gateway_urls() {
        let client = connection_client().expect("probe client singleton");
        for (base_url, on_gateway) in [
            ("https://opencode.ai/zen/go/v1", true),
            ("https://gateway.opencode.ai/zen/v1", true),
            ("https://api.example.com/v1", false),
            ("https://opencode.ai/docs", false),
        ] {
            let parsed =
                reqwest::Url::parse(&pinvou3_lib::model_probe::models_probe_url(base_url)).unwrap();
            let request = connection_probe_request(client, &parsed, base_url, "sk-test")
                .build()
                .unwrap();
            assert_eq!(
                request.headers().get("x-opencode-session").is_some(),
                on_gateway,
                "{base_url}"
            );
        }
    }

    /// `models test` does NOT follow redirects (round-4 review, the GUI
    /// `probe_model_connection` mirror — it switched to `Policy::none()`
    /// this PR): the lane carries `x-api-key` for anthropic official hosts,
    /// and reqwest strips only the Authorization/Cookie family on a
    /// cross-origin redirect, so a followed 302 would re-send those custom
    /// headers to a host the user never named. With redirects off the 3xx
    /// arrives as the final response and the shared status ladder
    /// classifies it as `redirect` — status handling unchanged, one hop
    /// fewer (a second hop would answer 404 here and read
    /// `endpoint_not_found`).
    /// A `/models` 302 WITH a `location` header pointing at a second in-mock
    /// path (everything else answers 404): the queue mock cannot emit headers,
    /// and a bare 302 without `location` is terminal under reqwest's default
    /// follow policy too - a no-follow pin fed by that shape never bites
    /// (round-4 mutation: deleting `Policy::none()` stayed green). This mock
    /// makes the second hop real.
    fn spawn_connection_redirect_mock() -> (String, std::sync::Arc<DiscoverMockLog>) {
        use std::io::Read as _;
        use std::sync::Arc;

        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind redirect mock listener");
        let addr = listener.local_addr().expect("redirect mock listener addr");
        let log = Arc::new(DiscoverMockLog::default());
        let log_thread = log.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(20) {
                let Ok(mut stream) = stream else { continue };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                loop {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                    if buf.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&buf).into_owned();
                let target = request
                    .split("\r\n")
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .to_owned();
                let path = target.split('?').next().unwrap_or("").to_owned();
                {
                    let mut hits = log_thread.hits.lock().unwrap_or_else(|p| p.into_inner());
                    *hits.entry(path.clone()).or_insert(0) += 1;
                }
                let response = if path == "/models" {
                    format!(
                        "HTTP/1.1 302 Found\r\nlocation: http://{}/elsewhere\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                        addr
                    )
                } else {
                    "HTTP/1.1 404 Not Found\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}".to_owned()
                };
                let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
                let _ = std::io::Write::flush(&mut stream);
            }
        });
        (format!("http://{addr}"), log)
    }

    /// The discover-lane twin of [`spawn_connection_redirect_mock`]: a
    /// `/models` 302 WITH a `location` header pointing at a second in-mock
    /// path (everything else answers 404). The queue mock cannot emit
    /// headers, and a bare 302 without `location` is terminal under
    /// reqwest's default follow policy too — the discover-lane no-follow
    /// pin fed by that shape never bit (round-5 mutation: deleting
    /// `Policy::none()` from `discovery_client` stayed green). Discover is
    /// the lane that carries `x-api-key` / `x-goog-api-key` custom headers,
    /// which reqwest does NOT strip on a redirect — the lane where a
    /// followed hop is the credential-leak hazard this policy exists for.
    fn spawn_discover_redirect_mock() -> (String, std::sync::Arc<DiscoverMockLog>) {
        use std::io::Read as _;
        use std::sync::Arc;

        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("bind discover redirect mock listener");
        let addr = listener
            .local_addr()
            .expect("discover redirect mock listener addr");
        let log = Arc::new(DiscoverMockLog::default());
        let log_thread = log.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(20) {
                let Ok(mut stream) = stream else { continue };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                loop {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                    if buf.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&buf).into_owned();
                let target = request
                    .split("\r\n")
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .to_owned();
                let path = target.split('?').next().unwrap_or("").to_owned();
                {
                    let mut hits = log_thread.hits.lock().unwrap_or_else(|p| p.into_inner());
                    *hits.entry(path.clone()).or_insert(0) += 1;
                }
                let response = if path == "/models" {
                    format!(
                        "HTTP/1.1 302 Found\r\nlocation: http://{}/elsewhere\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                        addr
                    )
                } else {
                    "HTTP/1.1 404 Not Found\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}".to_owned()
                };
                let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
                let _ = std::io::Write::flush(&mut stream);
            }
        });
        (format!("http://{addr}"), log)
    }

    #[test]
    fn models_test_redirect_is_not_followed_and_classifies_as_redirect() {
        let (base, log) = spawn_connection_redirect_mock();
        let probe = run_connection_probe(&base, "");
        assert!(!probe.ok, "a redirect is not a successful probe");
        assert_eq!(probe.code, "redirect");
        assert_eq!(probe.http_status, Some(302));
        assert_eq!(log.hits_for("/models"), 1, "no second hop was followed");
        assert_eq!(
            log.hits_for("/elsewhere"),
            0,
            "the location target must never be requested"
        );
    }

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
                while let Ok(n) = stream.read(&mut chunk) {
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

    /// The 4 MiB probe body cap ([`PROBE_BODY_CAP_BYTES`]) had no test: a
    /// regression to an unbounded read would have passed CI. Over-cap is a
    /// `Miss` like any other unusable answer, never a parse of the truncation.
    #[test]
    fn fetch_v1_models_over_the_body_cap_is_a_miss() {
        let oversized = format!("{{\"data\":\"{}\"}}", "A".repeat(PROBE_BODY_CAP_BYTES + 1));
        let mock = spawn_probe_mock(&[("/v1/models", 200, &oversized)]);
        assert_eq!(
            fetch_v1_models(&mock.base_url, None),
            V1ModelsProbe::Miss,
            "a body over PROBE_BODY_CAP_BYTES must be a Miss"
        );
        assert_eq!(
            mock.hits_for("/v1/models"),
            1,
            "the request itself was fine; only the body was over the cap"
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
    /// `select_local_server_kind` implements (demoted-Ollama order, GUI #686).
    /// Parallelizing the candidate probes ([`probe_candidates`]) means every
    /// candidate now runs even when an earlier one hit, so the ORDER is the
    /// only thing left deciding the answer — if it drifts, a host that matches
    /// two signatures silently changes kind. Each case below sets one hit plus
    /// every LOWER-priority hit, so it fails if the entry is demoted below any
    /// of them.
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
        let tachyon_body = || {
            V1ModelsProbe::Body(serde_json::json!({
                "data": [{ "id": "m", "owned_by": "tachyon" }]
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
            panicked: false,
        };
        assert_eq!(
            select_local_server_kind_from_hits(&all),
            "dockermodelrunner"
        );
        let cases: &[(&str, ProbeCandidateHits)] = &[
            (
                "lmstudio",
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
                "koboldcpp",
                ProbeCandidateHits {
                    ollama: true,
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
                    ollama: true,
                    llamacpp: true,
                    sglang: true,
                    v1_models: vllm_body(),
                    ..Default::default()
                },
            ),
            (
                // Tachyon outranks SGLang (the GUI's ordering): the case
                // sets every LOWER-priority hit, including the sglang
                // signature a new Tachyon package coincidentally satisfies.
                "tachyon",
                ProbeCandidateHits {
                    ollama: true,
                    sglang: true,
                    v1_models: tachyon_body(),
                    ..Default::default()
                },
            ),
            (
                "sglang",
                ProbeCandidateHits {
                    ollama: true,
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
                    ollama: true,
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
                    ollama: true,
                    v1_models: vllm_body(),
                    ..Default::default()
                },
            ),
            // Ollama is deliberately LAST among the specific signatures: the
            // /api/tags shim of a wrapped deployment must not shadow a stronger
            // engine, and a bare /api/tags hit only wins when it is alone.
            (
                "ollama",
                ProbeCandidateHits {
                    ollama: true,
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

    /// GUI #686 parity pin: model-store containers wrap the real engine behind
    /// an Ollama-compatible `/api/tags` shim, so a stronger signature (SGLang's
    /// `/get_server_info`, vLLM's owned_by) must win over the shim — otherwise
    /// the deployment is silently routed onto the ollama wire (boolean-only
    /// wire, reasoning_content inlined into answer text). A bare `/api/tags`
    /// hit with no stronger signature stays Ollama.
    #[test]
    fn local_server_kind_stronger_signatures_win_over_ollama_shim() {
        let shimmed = || ProbeCandidateHits {
            ollama: true,
            ..Default::default()
        };
        assert_eq!(
            select_local_server_kind_from_hits(&ProbeCandidateHits {
                sglang: true,
                ..shimmed()
            }),
            "sglang",
            "an /api/tags shim must not shadow the sglang /get_server_info signature"
        );
        assert_eq!(
            select_local_server_kind_from_hits(&ProbeCandidateHits {
                v1_models: V1ModelsProbe::Body(serde_json::json!({
                    "data": [{ "id": "m", "owned_by": "vllm" }]
                })),
                ..shimmed()
            }),
            "vllm",
            "an /api/tags shim must not shadow the vLLM owned_by signature"
        );
        assert_eq!(
            select_local_server_kind_from_hits(&shimmed()),
            "ollama",
            "a bare /api/tags hit with no stronger signature stays Ollama"
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

    /// A round whose probe thread panicked is incomplete: with no signature
    /// matched it must answer the probe-failed sentinel, not `generic`, and
    /// a signature that DID match still wins over the panic (the identified
    /// server is a fact the dead probe does not take away).
    #[test]
    fn local_server_kind_reports_probe_failed_when_the_round_is_incomplete() {
        assert_eq!(
            select_local_server_kind_from_hits(&ProbeCandidateHits {
                panicked: true,
                ..Default::default()
            }),
            LOCAL_KIND_PROBE_FAILED,
        );
        assert_eq!(
            select_local_server_kind_from_hits(&ProbeCandidateHits {
                ollama: true,
                panicked: true,
                ..Default::default()
            }),
            "ollama",
        );
        assert_eq!(
            select_local_server_kind_from_hits(&ProbeCandidateHits::default()),
            "generic",
            "a complete all-miss round is still a legitimate generic"
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
        let outcome = probe_local(
            &SystemCredentialStore::new(),
            Some(&mock.base_url),
            None,
            None,
            OutputMode::Json,
        )
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

    /// Every probe lane now reuses a process-wide singleton, not per-request
    /// clients: the GUI memoizes `shared_probe_client`, while the mirror
    /// rebuilt a `reqwest::blocking::Client` — pool, internal runtime thread
    /// and all — inside `get_json`, i.e. once per candidate request, for
    /// `models test` and the api-key lanes of `settings search test` alike.
    #[test]
    fn every_probe_lane_reuses_one_client() {
        assert!(std::ptr::eq(
            connection_client().expect("connection client builds"),
            connection_client().expect("connection client rebuilds")
        ));
        assert!(std::ptr::eq(
            search_probe_client().expect("search client builds"),
            search_probe_client().expect("search client rebuilds")
        ));
        assert!(std::ptr::eq(
            probe_client().expect("probe client builds"),
            probe_client().expect("probe client rebuilds")
        ));
        // Different lanes have different timeouts, so they must be distinct
        // clients; if two lanes shared one, one of the two timeouts would
        // silently win.
        assert!(!std::ptr::eq(
            connection_client().expect("connection client"),
            probe_client().expect("local probe client")
        ));
    }

    /// Bounds the wall time of one `probe-local` round against HUNG local
    /// listeners (bind, never accept, never answer): a probe request hangs
    /// until the per-probe 3 s timeout fires. The seven candidates are
    /// independent requests, so the concurrent fan-out in
    /// `probe_candidates` is bounded by ONE timeout window (plus at most one
    /// extra for `fetch_v1_models`'s path fallback); the sequential loop the
    /// module had before cost 7 windows. The assertions therefore fail if
    /// the probes regress to running one after another.
    ///
    /// Uses only loopback TCP listeners the test process itself binds — no
    /// external network (the same rule the other probe tests follow).
    #[test]
    fn probe_candidates_round_is_bounded_by_one_timeout_window() {
        /// One listener that accepts nothing: the request connects (kernel
        /// backlog) and then waits for a response that never arrives, which
        /// is exactly what a hung endpoint looks like to the client.
        struct HungListener(std::net::TcpListener);
        impl HungListener {
            fn spawn() -> Self {
                let listener =
                    std::net::TcpListener::bind("127.0.0.1:0").expect("bind hung listener");
                Self(listener)
            }
            fn base_url(&self) -> String {
                format!(
                    "http://{}",
                    self.0.local_addr().expect("hung listener addr")
                )
            }
        }
        // Dropping the listener closes the backlog sockets, releasing the
        // probe threads.

        let hung = HungListener::spawn();
        // The DMR probe fires only on port 12434; it will miss (refused) and
        // return false, which is fine — every other candidate hangs its full
        // timeout window against the listener.
        let url = hung.base_url();
        let started = std::time::Instant::now();
        let hits = probe_candidates(&url, None);
        let elapsed = started.elapsed();
        // One timeout window is ~3 s. The fan-out must stay comfortably
        // under TWO windows even on a loaded CI machine (an extra window is
        // the floor for `fetch_v1_models`'s 404 fallback), while the
        // sequential lower bound is 7 windows. Asserting < 5 s fails only
        // when probes actually serialize (7 x 3 s = 21 s).
        assert!(
            // Two windows + margin: the sequential regression this guards
            // against costs 21s, so precision is unnecessary — and a tight
            // ceiling flaked on loaded runners where 7 threads each sleep
            // out a 3s timeout (round-37 review).
            elapsed < Duration::from_secs(8),
            "one probe round took {elapsed:?} against a hung endpoint; the candidates \
             must share one timeout window, not run one after another"
        );
        // And the round must still have waited for the slowest single probe:
        // a result that came back instantly would mean the probes failed for
        // the wrong reason (e.g. an immediate error), not a timeout run.
        assert!(
            elapsed >= Duration::from_secs(3),
            "the round returned in {elapsed:?}; the hung endpoint means nothing can \
             answer before the timeout window"
        );
        assert_eq!(hits, ProbeCandidateHits::default());
        drop(hung);
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
        // Bing has no api-key form (no documented endpoint); it is routed to
        // the scrape probe instead.
        assert!(search_api_endpoint(SearchProvider::Bing).is_none());
        for (provider, host) in [
            (SearchProvider::Tavily, "api.tavily.com"),
            (SearchProvider::Bocha, "api.bochaai.com"),
            (SearchProvider::Metaso, "metaso.cn"),
            (SearchProvider::Baidu, "qianfan.baidubce.com"),
        ] {
            let request = search_api_request_at(
                &client,
                provider,
                "probe-key",
                search_api_endpoint(provider).expect("documented endpoint"),
            )
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

    /// Metaso, Bocha and Baidu answer `HTTP 200` with a business error code
    /// in the body when the key is rejected (Metaso `code: 2005`) or the
    /// quota is gone — the product's own web-search path treats those bodies
    /// as failures (`CodeWhale/crates/tui/src/tools/web_search.rs`
    /// `run_metaso_search` / `bocha_error_message` / `baidu_error_message`).
    /// A probe that classifies only the HTTP status reports a rejected key as
    /// `ok: true` from a command named `test`. Each case below drives the
    /// full request→classification lane against canned bodies on loopback.
    #[test]
    fn search_probe_classifies_error_bodies_as_not_ok() {
        let cases: &[(SearchProvider, &str, &str)] = &[
            // Metaso `2005`: the API key was rejected (invalid/expired).
            (
                SearchProvider::Metaso,
                r#"{"code":2005,"message":"api key rejected"}"#,
                "key_rejected",
            ),
            // Metaso daily-quota body (upstream `3003` arm).
            (
                SearchProvider::Metaso,
                r#"{"code":3003,"message":"daily limit"}"#,
                "quota_exhausted",
            ),
            // Metaso error body with a code the probe does not special-case:
            // still not ok, with the code and message carried into the detail.
            (
                SearchProvider::Metaso,
                r#"{"code":42,"message":"other error"}"#,
                "provider_error",
            ),
            // Bocha answers 200 with a non-zero/non-200 business code.
            (
                SearchProvider::Bocha,
                r#"{"code":401,"msg":"Unauthorized"}"#,
                "provider_error",
            ),
            // Baidu puts the business code in `error_code`/`error_msg`.
            (
                SearchProvider::Baidu,
                r#"{"error_code":17,"error_msg":"Open api daily reached"}"#,
                "provider_error",
            ),
        ];
        for (provider, body, expected_code) in cases {
            let name = provider.as_str();
            let mock = spawn_probe_mock(&[("/", 200u16, body)]);
            let probe = run_search_api_probe_at(*provider, "probe-key", &mock.base_url);
            assert!(
                !probe.ok,
                "{name}: a 200 body carrying a business error code must not pass the test: {:?}",
                probe.detail
            );
            assert_eq!(
                probe.code, *expected_code,
                "{name}: {body} must classify as {expected_code}"
            );
            // The refusal must be truthful: the provider's own code/message
            // text is the evidence behind the classification.
            let detail = probe.detail.as_deref().unwrap_or("");
            if *expected_code == "provider_error" {
                assert!(!detail.is_empty(), "{name}: detail must not be empty");
            }
            if *expected_code == "key_rejected" {
                assert!(
                    detail.contains("2005"),
                    "the detail must carry the provider's code: {detail}"
                );
            }
            assert_eq!(probe.verified, VERIFIED_LIVE_PROBE);
            assert_eq!(
                mock.auth_for("/").as_deref(),
                Some("Bearer probe-key"),
                "{name}: the probe must actually sign with the key"
            );
        }
    }

    /// The happy path the classification must not over-reject: a 2xx body
    /// without a business error code stays `ok: true`, with the per-provider
    /// success codes (Bocha's literal `200`, Baidu's and Metaso's `0`) in the
    /// same shapes the product accepts.
    #[test]
    fn search_probe_accepts_clean_success_bodies() {
        let cases: &[(SearchProvider, &str)] = &[
            (SearchProvider::Tavily, r#"{"results":[]}"#),
            (SearchProvider::Metaso, r#"{"code":0,"webpages":[]}"#),
            (
                SearchProvider::Bocha,
                r#"{"code":200,"data":{"webPages":[]}}"#,
            ),
            (SearchProvider::Baidu, r#"{"error_code":0,"references":[]}"#),
        ];
        for (provider, body) in cases {
            let name = provider.as_str();
            let mock = spawn_probe_mock(&[("/", 200u16, body)]);
            let probe = run_search_api_probe_at(*provider, "probe-key", &mock.base_url);
            assert!(
                probe.ok,
                "{name}: a clean success body must pass the probe: {:?}",
                probe.detail
            );
            assert_eq!(probe.code, "ok");
            assert_eq!(probe.verified, VERIFIED_LIVE_PROBE);
        }
    }

    /// The 4 MiB cap ([`PROBE_BODY_CAP_BYTES`]) had no teeth on the search
    /// lane: a regression to an unbounded `response.text()` would have passed
    /// CI. Over-cap is its own honest failure — never `ok: true` from parsing
    /// the truncation (Tavily is the sharpest canary here: with no business
    /// error codes, a truncated parse would pass the probe).
    #[test]
    fn search_probe_over_the_body_cap_fails_the_probe() {
        let oversized = format!(
            "{{\"results\":\"{}\"}}",
            "A".repeat(PROBE_BODY_CAP_BYTES + 1)
        );
        let mock = spawn_probe_mock(&[("/", 200u16, &oversized)]);
        let probe = run_search_api_probe_at(SearchProvider::Tavily, "probe-key", &mock.base_url);
        assert!(
            !probe.ok,
            "a body over PROBE_BODY_CAP_BYTES must fail the probe: {:?}",
            probe.detail
        );
        assert_eq!(probe.code, "response_too_large");
        assert_eq!(probe.verified, VERIFIED_LIVE_PROBE);
        assert_eq!(
            mock.hits_for("/"),
            1,
            "the request itself was fine; only the body was over the cap"
        );
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

    // -------------------------------------------------------------------
    // `models discover` — protocol mapping, parsing, formatting, the
    // classification ladder and the per-protocol shapes. The HTTP lanes run
    // against a std-only loopback mock modeled on `spawn_probe_mock`, with
    // the additions the discovery lanes need: full header capture (the
    // anthropic / gemini auth headers), query recording (the pagination
    // cursors), per-path SEQUENCED responses (the two-page ladders) and a
    // content-length body drain (the Ollama /api/show POSTs).
    // -------------------------------------------------------------------

    /// A `models discover` wire protocol resolves the same way on both
    /// surfaces: the preset table maps only anthropic / gemini to their
    /// native ladders, every wire spelling except `anthropic` stays on the
    /// OpenAI-compatible ladder, and a preset outranks a wire when both are
    /// given (the GUI command's own precedence).
    #[test]
    fn discover_protocol_mapping_is_pinned() {
        assert_eq!(
            protocol_for_preset(ModelPreset::Anthropic),
            DiscoveryProtocol::Anthropic
        );
        assert_eq!(
            protocol_for_preset(ModelPreset::Gemini),
            DiscoveryProtocol::Gemini
        );
        for preset in [
            ModelPreset::LocalVllm,
            ModelPreset::Deepseek,
            ModelPreset::Kimi,
            ModelPreset::OpenaiCompatible,
            ModelPreset::Qwen,
            ModelPreset::Doubao,
            ModelPreset::Minimax,
            ModelPreset::Glm,
            ModelPreset::Mimo,
            ModelPreset::Openai,
            ModelPreset::Xai,
        ] {
            assert_eq!(
                protocol_for_preset(preset),
                DiscoveryProtocol::OpenAiCompatible,
                "{preset:?} walks the OpenAI-compatible ladder"
            );
        }
        assert_eq!(
            protocol_for_wire("anthropic"),
            Some(DiscoveryProtocol::Anthropic)
        );
        assert_eq!(
            protocol_for_wire("openai"),
            Some(DiscoveryProtocol::OpenAiCompatible)
        );
        assert_eq!(
            protocol_for_wire("kimi"),
            Some(DiscoveryProtocol::OpenAiCompatible)
        );
        assert_eq!(
            protocol_for_wire("responses"),
            Some(DiscoveryProtocol::OpenAiCompatible),
            "the GUI's responses wire is OpenAI-compatible on both surfaces"
        );
        assert_eq!(protocol_for_wire(" claude "), None);
        assert_eq!(
            resolve_discovery_protocol(
                Some(ModelPreset::Anthropic),
                Some(DiscoveryProtocol::OpenAiCompatible)
            ),
            DiscoveryProtocol::Anthropic,
            "the preset wins when both flags are given"
        );
        assert_eq!(
            resolve_discovery_protocol(None, Some(DiscoveryProtocol::Anthropic)),
            DiscoveryProtocol::Anthropic
        );
        assert_eq!(
            resolve_discovery_protocol(Some(ModelPreset::LocalVllm), None),
            DiscoveryProtocol::OpenAiCompatible
        );
        assert_eq!(
            resolve_discovery_protocol(None, None),
            DiscoveryProtocol::OpenAiCompatible,
            "neither flag means the OpenAI-compatible default"
        );
    }

    #[test]
    fn discover_parse_accepts_the_documented_flag_surface() {
        let arguments = [
            "models".to_owned(),
            "discover".to_owned(),
            "--base-url".to_owned(),
            " http://127.0.0.1:11434 ".to_owned(),
            "--api-key-env".to_owned(),
            "PINVOU_TEST_DISCOVER_PARSE_KEY".to_owned(),
            "--preset".to_owned(),
            "anthropic".to_owned(),
            "--json".to_owned(),
        ];
        let ModelsCommand::Discover {
            base_url,
            api_key_env,
            preset,
            wire,
            json,
        } = parse(&arguments).expect("the documented flag surface parses")
        else {
            panic!("parse must produce a Discover command");
        };
        assert_eq!(base_url, "http://127.0.0.1:11434", "the value is trimmed");
        // The flag carries an environment VARIABLE NAME, stored verbatim;
        // the variable is read only when the command runs.
        assert_eq!(
            api_key_env.as_deref(),
            Some("PINVOU_TEST_DISCOVER_PARSE_KEY")
        );
        assert_eq!(preset, Some(ModelPreset::Anthropic));
        assert_eq!(wire, None);
        assert!(json);
    }

    #[test]
    fn discover_parse_maps_the_wire_flag() {
        let arguments = [
            "models".to_owned(),
            "discover".to_owned(),
            "--base-url".to_owned(),
            "http://127.0.0.1:8080".to_owned(),
            "--wire".to_owned(),
            "Kimi".to_owned(),
        ];
        let ModelsCommand::Discover { wire, .. } =
            parse(&arguments).expect("wire parsing is case-insensitive")
        else {
            panic!("parse must produce a Discover command");
        };
        assert_eq!(wire, Some(DiscoveryProtocol::OpenAiCompatible));
    }

    #[test]
    fn discover_parse_rejects_bad_usage() {
        // Missing --base-url: the usage block (flags + exit-code table)
        // rides along on the error, like the benchmark family's catch-all.
        let missing = parse(&["models".to_owned(), "discover".to_owned()])
            .expect_err("discover without --base-url is a usage error");
        assert_eq!(missing.exit_code(), ExitCode::Usage);
        assert!(
            missing.to_string().contains("--base-url")
                && missing.to_string().contains("exit codes: 0 listed"),
            "the error must carry the flag surface and the exit-code table: {missing}"
        );
        let unknown_wire = parse(&[
            "models".to_owned(),
            "discover".to_owned(),
            "--base-url".to_owned(),
            "http://127.0.0.1:11434".to_owned(),
            "--wire".to_owned(),
            "grpc".to_owned(),
        ])
        .expect_err("an unknown wire is a usage error naming the valid values");
        assert!(
            unknown_wire
                .to_string()
                .contains("anthropic, openai, kimi, responses")
        );
        let unknown_preset = parse(&[
            "models".to_owned(),
            "discover".to_owned(),
            "--base-url".to_owned(),
            "http://127.0.0.1:11434".to_owned(),
            "--preset".to_owned(),
            "mistral".to_owned(),
        ])
        .expect_err("an unknown preset reuses the shared preset table error");
        assert!(unknown_preset.to_string().contains("valid presets"));
        let stray = parse(&[
            "models".to_owned(),
            "discover".to_owned(),
            "--base-url".to_owned(),
            "http://127.0.0.1:11434".to_owned(),
            "sk-pasted-by-accident".to_owned(),
        ])
        .expect_err("a stray positional is refused without echoing it");
        assert!(
            !stray.to_string().contains("sk-pasted-by-accident"),
            "the refusal reports the count only, never the token: {stray}"
        );
        let flag_as_value = parse(&[
            "models".to_owned(),
            "discover".to_owned(),
            "--base-url".to_owned(),
            "http://127.0.0.1:11434".to_owned(),
            "--api-key-env".to_owned(),
            "--preset".to_owned(),
            "openai".to_owned(),
        ])
        .expect_err("a value flag cannot consume another flag");
        assert_eq!(flag_as_value.exit_code(), ExitCode::Usage);
    }

    /// The secret-discipline pin: the literal-key flag this subcommand
    /// shipped with for one round (`--api-key <key>`) must stay refused. A
    /// pasted secret on argv rides the shell history and the process list —
    /// exactly what the family-wide `--api-key-env` entry exists to prevent
    /// — so the literal flag is now an unknown option (usage, exit 2), and
    /// the refusal must not echo the pasted value. The single-token `=` form
    /// (`--api-key=sk-...`) carries the secret INSIDE the flag token, so the
    /// unknown-option refusal reports the flag name only.
    #[test]
    fn discover_parse_refuses_the_literal_api_key_flag() {
        let error = parse(&[
            "models".to_owned(),
            "discover".to_owned(),
            "--base-url".to_owned(),
            "http://127.0.0.1:11434".to_owned(),
            "--api-key".to_owned(),
            "sk-pasted-literal".to_owned(),
        ])
        .expect_err("the literal --api-key flag is no longer part of the surface");
        assert_eq!(error.exit_code(), ExitCode::Usage);
        let message = error.to_string();
        assert!(
            message.contains("unknown option --api-key"),
            "the refusal names the unknown flag: {message}"
        );
        assert!(
            !message.contains("sk-pasted-literal"),
            "the pasted value is never echoed: {message}"
        );
        // The `=` spelling lands on the same refusal with the same redaction:
        // the message carries the flag name, never the value after the `=`.
        let joined = parse(&[
            "models".to_owned(),
            "discover".to_owned(),
            "--base-url".to_owned(),
            "http://127.0.0.1:11434".to_owned(),
            "--api-key=sk-pasted-literal".to_owned(),
        ])
        .expect_err("the joined --api-key=<secret> form is the same unknown flag");
        assert_eq!(joined.exit_code(), ExitCode::Usage);
        let message = joined.to_string();
        assert!(
            message.contains("unknown option --api-key"),
            "the refusal names the flag without the = value: {message}"
        );
        assert!(
            !message.contains("sk-pasted-literal"),
            "the pasted value after the = is never echoed: {message}"
        );
    }

    /// The malformed-URL gate is argv-origin (usage, exit 2), the same
    /// classification `probe-local --url` applies; the error carries the
    /// discover usage block so the fix is one line away.
    #[test]
    fn discover_refuses_a_malformed_base_url_as_usage() {
        let error = discover(
            "not a url at all",
            None,
            None,
            None,
            false,
            OutputMode::Human,
        )
        .expect_err("a malformed base_url is a usage error");
        assert_eq!(error.exit_code(), ExitCode::Usage);
        assert!(error.to_string().contains("is not a valid url"));
        assert!(error.to_string().contains("exit codes: 0 listed"));
    }

    #[test]
    fn discover_tokens_format_as_decimal_k_and_m() {
        assert_eq!(format_tokens_km(0), "0");
        assert_eq!(format_tokens_km(999), "999");
        assert_eq!(format_tokens_km(1000), "1K");
        assert_eq!(format_tokens_km(1001), "1K");
        assert_eq!(format_tokens_km(8192), "8.2K");
        assert_eq!(format_tokens_km(128000), "128K");
        assert_eq!(format_tokens_km(131072), "131.1K");
        assert_eq!(format_tokens_km(999950), "1M", "a rounding carry steps up");
        assert_eq!(format_tokens_km(1000000), "1M");
        assert_eq!(format_tokens_km(1048576), "1M");
        assert_eq!(format_tokens_km(1234567), "1.2M");
        assert_eq!(format_tokens_km(999999999), "1G");
        assert_eq!(format_tokens_km(1000000000), "1G");
        assert_eq!(format_tokens_km(4000000000), "4G");
        assert_eq!(
            format_tokens_cell(None),
            "-",
            "an absent length prints as a dash"
        );
    }

    /// The classification ladder is the error contract scripts read: 401/403
    /// (Groq uses 403) → auth_required, 404/405 → unsupported, every other
    /// ≥400 → error with the status kept, 2xx → the caller keeps parsing.
    /// 3xx rows are in the SAME error bucket: the discovery client does not
    /// follow redirects, so a 302/307/308 arrives as a decided final status
    /// and files as plain `error` — never listed, never auth/unsupported.
    #[test]
    fn discover_status_classification_is_pinned() {
        use ListingStatus::*;
        for status in [401u16, 403] {
            let listing = discovery_status_listing(status).expect("401/403 decide");
            assert_eq!(listing.status, AuthRequired);
            assert_eq!(listing.http_status, Some(status));
            assert!(listing.models.is_empty());
        }
        for status in [404u16, 405] {
            let listing = discovery_status_listing(status).expect("404/405 decide");
            assert_eq!(listing.status, Unsupported);
            assert_eq!(listing.http_status, Some(status));
        }
        for status in [302u16, 307, 308, 400, 402, 408, 429, 500, 503] {
            let listing = discovery_status_listing(status).expect("other statuses decide");
            assert_eq!(listing.status, Error);
            assert_eq!(listing.http_status, Some(status));
            assert!(listing.models.is_empty(), "{status} carries no rows");
        }
        assert_eq!(discovery_status_listing(200), None, "2xx keeps parsing");
    }

    /// The transport class's ARM ORDER (round-4 review M1, the GUI
    /// `transport_class` mirror): timeout → dns → tls → connect → network.
    /// The tls chain scan must sit BEFORE `is_connect` because reqwest's
    /// `is_connect()` walks the source chain too and hyper-util files the
    /// TLS handshake under its Connect kind — a real certificate failure
    /// answers `is_connect() == true` and would never reach the tls arm if
    /// connect were consulted first. `reqwest::Error` has no public
    /// constructor, so the order is pinned through the flags+chain core
    /// ([`discovery_transport_class_from_flags`], the same testability seam
    /// the app's `transport_code_for` exists for) with synthetic chains —
    /// the same construction the app-side
    /// `tls_class_walks_source_chain_not_top_level_display` test uses.
    #[test]
    fn discovery_transport_class_orders_tls_before_connect() {
        #[derive(Debug)]
        struct Wrapper(&'static str);
        impl std::fmt::Display for Wrapper {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.0)
            }
        }
        impl std::error::Error for Wrapper {}

        #[derive(Debug)]
        struct Chained {
            display: &'static str,
            source: Wrapper,
        }
        impl std::fmt::Display for Chained {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.display)
            }
        }
        impl std::error::Error for Chained {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.source)
            }
        }

        // Real shape: the top level is reqwest's url-bearing Display (the
        // URL happens to carry "ssl"), the certificate fact lives in the
        // inner rustls text.
        let tls_chain = || Chained {
            display: "error sending request for url (https://ba-ssl-gw.example.com/v1/models)",
            source: Wrapper("certificate verify failed: self-signed certificate"),
        };
        assert_eq!(
            discovery_transport_class_from_flags(false, false, true, &tls_chain()),
            "tls",
            "a TLS-handshake failure that also answers is_connect() must classify tls"
        );
        // The structured flags still dominate the chain scan.
        assert_eq!(
            discovery_transport_class_from_flags(true, true, true, &tls_chain()),
            "timeout"
        );
        assert_eq!(
            discovery_transport_class_from_flags(false, true, false, &tls_chain()),
            "dns"
        );
        // A plain connect failure whose chain carries no certificate fact.
        assert_eq!(
            discovery_transport_class_from_flags(
                false,
                false,
                true,
                &Chained {
                    display: "error sending request for url (http://127.0.0.1:9/v1/models)",
                    source: Wrapper("connection refused"),
                }
            ),
            "connect"
        );
        // The top-level Display (URL text included) is never read: a URL
        // biased with tls/ssl words over a CLEAN chain (no certificate text
        // in any source level - the tls_chain() fixture's inner text would
        // legitimately answer tls) classifies by the flags alone.
        assert_eq!(
            discovery_transport_class_from_flags(
                false,
                false,
                false,
                &Chained {
                    display: "error sending request for url (https://ba-ssl-gw.example.com/v1/models)",
                    source: Wrapper("connection refused"),
                }
            ),
            "network",
            "no flags and no chain fact: the ssl-looking URL must not bias the class"
        );
    }

    /// The `server_kind` strings discover emits speak the GUI's kind
    /// vocabulary (`local_server_kind_str`, the same table monitor speaks),
    /// not the detector's internal spellings: LM Studio reports as
    /// `lm_studio`, the probe-local honesty sentinels read as `generic`,
    /// and the kinds that already match — Tachyon included, which the CLI's
    /// `/v1/models` owned_by probe detects exactly like the GUI's — pass
    /// through unchanged.
    #[test]
    fn discover_server_kind_speaks_the_gui_vocabulary() {
        assert_eq!(discovery_server_kind_str("lmstudio"), "lm_studio");
        assert_eq!(discovery_server_kind_str("tachyon"), "tachyon");
        for passthrough in [
            "vllm",
            "ollama",
            "sglang",
            "llamacpp",
            "koboldcpp",
            "lmdeploy",
            "dockermodelrunner",
            "generic",
        ] {
            assert_eq!(
                discovery_server_kind_str(passthrough),
                passthrough,
                "{passthrough} is already the GUI spelling"
            );
        }
        assert_eq!(
            discovery_server_kind_str(LOCAL_KIND_UNKNOWN_AUTHENTICATED),
            "generic",
            "absence-of-a-classification reads as generic, the GUI table's answer"
        );
        assert_eq!(
            discovery_server_kind_str(LOCAL_KIND_PROBE_FAILED),
            "generic"
        );
    }

    /// Constant pins mirroring the GUI discovery ladder's own pins: the 8
    /// MiB listing body cap (its `LISTING_BODY_CAP_BYTES`), the 20s Ollama
    /// show sweep budget (its `OLLAMA_SHOW_BUDGET`) and the native lanes'
    /// 2000-entry total cap (its same-wave `NATIVE_LISTING_CAP`) must not
    /// drift silently out of parity.
    #[test]
    fn discover_bounds_match_the_gui_ladder() {
        assert_eq!(DISCOVER_BODY_CAP_BYTES, 8 * 1024 * 1024);
        assert_eq!(DISCOVER_OLLAMA_SHOW_BUDGET, Duration::from_secs(20));
        assert_eq!(DISCOVER_NATIVE_LISTING_CAP, 2000);
    }

    /// Entry parsing normalizes the THREE context spellings (tightest wins —
    /// the same only-tighten doctrine as the output side) and the three
    /// output shapes including OpenRouter's `top_provider`; non-positive,
    /// out-of-range and non-numeric values read as undeclared, and an entry
    /// without an id is not a model. `max_context_length` is not one of the
    /// spellings: LM Studio's capability ceiling is refused, alone (no
    /// declared context) and against a tighter effective window alike.
    #[test]
    fn discover_openai_entry_parses_lengths_tightest_first() {
        let entry = serde_json::json!({
            "id": " m1 ",
            "max_model_len": 4096,
            "context_length": 8192,
            "context_window": 9999999999u64,
            "max_context_length": 16384,
            "max_output_tokens": 4096,
            "max_completion_tokens": 8192,
            "top_provider": { "max_completion_tokens": 2048 },
        });
        let model = discovery_openai_entry(&entry).expect("a well-formed entry parses");
        assert_eq!(model.id, "m1", "the id is trimmed");
        assert_eq!(
            model.context_window_tokens,
            Some(4096),
            "the tightest context spelling wins; out-of-range reads undeclared"
        );
        assert_eq!(model.context_from, Some(ModelFactSource::Endpoint));
        assert_eq!(
            model.max_output_tokens,
            Some(2048),
            "the tightest output shape (OpenRouter top_provider) wins"
        );
        assert_eq!(model.output_from, Some(ModelFactSource::Endpoint));
        assert_eq!(model.loaded, None);
        assert_eq!(model.title, None);
        // The capability ceiling never speaks for the entry: alone it is an
        // undeclared context (the compat lane's counterpart of the
        // connection-test lane's cap-alias refusal), and beside an effective
        // window it is never adopted — not even as the looser alternative.
        let cap_only = discovery_openai_entry(&serde_json::json!({
            "id": "m2",
            "max_context_length": 16384,
        }))
        .expect("an id-only entry parses");
        assert_eq!(
            cap_only.context_window_tokens, None,
            "a capability ceiling alone is not a served window"
        );
        assert_eq!(cap_only.context_from, None, "no length, no source");
        let mixed = discovery_openai_entry(&serde_json::json!({
            "id": "m3",
            "max_model_len": 8192,
            "max_context_length": 131072,
        }))
        .expect("an effective-window entry parses");
        assert_eq!(
            mixed.context_window_tokens,
            Some(8192),
            "the effective window wins; the capability ceiling is never adopted"
        );
        // Zero / negative / non-numeric values never fabricate a length.
        let lying = serde_json::json!({
            "id": "m4",
            "max_model_len": 0,
            "context_length": -8192,
            "context_window": "huge",
            "max_output_tokens": 1.5,
        });
        let model = discovery_openai_entry(&lying).expect("an id-only entry parses");
        assert_eq!(model.context_window_tokens, None);
        assert_eq!(model.max_output_tokens, None);
        assert_eq!(model.context_from, None, "no length, no source");
        // A bare array entry with an empty id is not a model.
        assert_eq!(
            discovery_openai_entry(&serde_json::json!({"id": "   "})),
            None
        );
    }

    /// Spec §2.10-11 mirror: the 2000-entry cap truncates and flags; exactly
    /// 2000 entries stay unflagged; the bare-array (Together) shape and the
    /// `{data}` shape parse identically; an unrecognized shape is None
    /// (→ unsupported), an EMPTY array is a legal listed answer.
    #[test]
    fn discover_openai_body_cap_and_shapes_are_pinned() {
        let bare = serde_json::json!([{ "id": "a" }, { "id": "b" }]);
        let (models, truncated) = discovery_openai_models(&bare).expect("a bare array parses");
        assert_eq!(models.len(), 2);
        assert!(!truncated);
        let envelope = serde_json::json!({ "data": [{ "id": "a" }, { "id": "b" }] });
        let (models, _) = discovery_openai_models(&envelope).expect("the data envelope parses");
        assert_eq!(models.len(), 2);
        let empty = serde_json::json!({ "data": [] });
        let (models, truncated) = discovery_openai_models(&empty).expect("empty parses");
        assert!(
            models.is_empty() && !truncated,
            "an empty list is a legal listed answer"
        );
        for shape in [serde_json::json!({"models": []}), serde_json::json!(42)] {
            assert_eq!(
                discovery_openai_models(&shape),
                None,
                "a shape that is neither the envelope nor a bare array is unrecognized"
            );
        }
        let cap = serde_json::json!({
            "data": (0..2001).map(|index| serde_json::json!({"id": format!("m{index}")})).collect::<Vec<_>>()
        });
        let (models, truncated) = discovery_openai_models(&cap).expect("the capped body parses");
        assert_eq!(models.len(), 2000);
        assert!(truncated, "2001 entries must flag the truncation");
        let exact = serde_json::json!({
            "data": (0..2000).map(|index| serde_json::json!({"id": format!("m{index}")})).collect::<Vec<_>>()
        });
        let (models, truncated) = discovery_openai_models(&exact).expect("the exact body parses");
        assert_eq!(models.len(), 2000);
        assert!(!truncated, "exactly 2000 entries do not flag");
    }

    #[test]
    fn discover_anthropic_page_parses_facts_and_cursor() {
        let page = serde_json::json!({
            "data": [
                {
                    "id": "claude-a",
                    "display_name": "Claude A",
                    "max_input_tokens": 200000,
                    "max_tokens": 8192,
                },
                { "id": "claude-b", "max_input_tokens": 0 },
            ],
            "has_more": true,
        });
        let (models, has_more, after_id) =
            discovery_anthropic_page(&page).expect("a well-formed page parses");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "claude-a");
        assert_eq!(models[0].context_window_tokens, Some(200000));
        assert_eq!(models[0].max_output_tokens, Some(8192));
        assert_eq!(models[0].title.as_deref(), Some("Claude A"));
        assert_eq!(
            models[1].context_window_tokens, None,
            "a non-positive length reads undeclared"
        );
        assert_eq!(has_more, Some(true));
        assert_eq!(
            after_id.as_deref(),
            Some("claude-b"),
            "the cursor is the page's last id"
        );
        let missing_cursor = serde_json::json!({ "data": [], "has_more": true });
        let (_, has_more, after_id) =
            discovery_anthropic_page(&missing_cursor).expect("an empty page parses");
        assert_eq!(has_more, Some(true));
        assert!(after_id.is_none(), "no entries means no cursor");
        assert_eq!(discovery_anthropic_page(&serde_json::json!({})), None);
    }

    #[test]
    fn discover_gemini_page_filters_generation_methods() {
        let page = serde_json::json!({
            "models": [
                {
                    "name": "models/gemini-x",
                    "supportedGenerationMethods": ["generateContent", "bidi"],
                    "inputTokenLimit": 1000000,
                    "outputTokenLimit": 8192,
                    "displayName": "Gemini X",
                },
                {
                    "name": "models/embed-thing",
                    "supportedGenerationMethods": ["embedContent"],
                },
                { "name": "models/no-methods" },
                {
                    "name": "models/case-variant",
                    "supportedGenerationMethods": ["GenerateContent"],
                },
            ],
            "nextPageToken": "tok-1",
        });
        let (models, next) = discovery_gemini_page(&page).expect("a well-formed page parses");
        let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
        // Round-3 review minor: an entry WITHOUT the field is KEPT (a
        // gateway omitting it must not filter the listing to zero rows);
        // only an explicit list without generateContent filters.
        assert_eq!(
            ids,
            ["gemini-x", "no-methods", "case-variant"],
            "generateContent-capable and missing-field entries stay (case-insensitive), with the models/ prefix stripped"
        );
        assert_eq!(models[0].context_window_tokens, Some(1000000));
        assert_eq!(models[0].max_output_tokens, Some(8192));
        assert_eq!(models[0].title.as_deref(), Some("Gemini X"));
        assert_eq!(next.as_deref(), Some("tok-1"));
        let last = serde_json::json!({ "models": [] });
        let (models, next) = discovery_gemini_page(&last).expect("an empty page parses");
        assert!(models.is_empty() && next.is_none());
        assert_eq!(discovery_gemini_page(&serde_json::json!({})), None);
    }

    /// LM Studio's capability cap (`max_context_length`) is the same fact
    /// class refused for Ollama's GGUF training window: only a LOADED
    /// entry's `loaded_context_length` is the served window.
    #[test]
    fn discover_lmstudio_v0_reads_loaded_context_only() {
        let value = serde_json::json!({
            "data": [
                {
                    "id": "loaded-model",
                    "state": "loaded",
                    "loaded_context_length": 12918,
                    "max_context_length": 131072,
                },
                {
                    "id": "cold-model",
                    "state": "not-loaded",
                    "max_context_length": 4096,
                },
                { "id": "legacy", "state": "weird-state" },
            ]
        });
        let models = discovery_lmstudio_v0_models(&value).expect("a well-formed v0 body parses");
        assert_eq!(models.len(), 3);
        assert_eq!(
            models[0].context_window_tokens,
            Some(12918),
            "the served window of a loaded entry"
        );
        assert_eq!(models[0].loaded, Some(true));
        assert_eq!(
            models[1].context_window_tokens, None,
            "the capability cap of a cold entry must NOT be adopted"
        );
        assert_eq!(models[1].loaded, Some(false));
        assert_eq!(
            models[2].loaded,
            Some(false),
            "any recognized-but-not-loaded state reads not-loaded, mirroring the GUI parser"
        );
        assert_eq!(
            discovery_lmstudio_v0_models(&serde_json::json!({"data": []})),
            None
        );
        assert_eq!(discovery_lmstudio_v0_models(&serde_json::json!({})), None);
    }

    /// Ollama facts: `/api/ps` carries the loaded set and effective contexts
    /// under both name spellings; `/api/show` contributes ONLY the Modelfile
    /// `num_ctx` — the GGUF training window (`model_info.<arch>.
    /// context_length`) is deliberately not adopted (the GUI probes refuse
    /// it for the same over-reporting reason).
    #[test]
    fn discover_ollama_fact_parsers_are_pinned() {
        let ps = serde_json::json!({
            "models": [
                { "name": "llama3:latest", "context_length": 8192 },
                { "model": "qwen2:7b", "context_length": 0 },
            ]
        });
        let names = discovery_ollama_ps_names(&ps);
        assert!(names.contains("llama3:latest") && names.contains("qwen2:7b"));
        let contexts = discovery_ollama_ps_contexts(&ps);
        assert_eq!(contexts.get("llama3:latest"), Some(&8192));
        assert_eq!(
            contexts.get("qwen2:7b"),
            None,
            "a non-positive context reads undeclared"
        );
        assert!(discovery_ollama_ps_names(&serde_json::json!({})).is_empty());
        let tags = serde_json::json!({
            "models": [ { "name": "b" }, { "name": "a" }, { "name": "b" }, { "name": "  " } ]
        });
        assert_eq!(
            discovery_ollama_tag_names(&tags).as_deref(),
            Some(&["b".to_owned(), "a".to_owned()][..]),
            "ordered, deduped, blank names dropped"
        );
        assert_eq!(discovery_ollama_tag_names(&serde_json::json!({})), None);
        let show = serde_json::json!({
            "parameters": "temperature 0.7\nnum_ctx 4096\nstop <|eot|>",
            "model_info": { "llama.context_length": 131072 },
        });
        assert_eq!(
            discovery_ollama_show_context(&show),
            Some(4096),
            "the declared num_ctx wins; the GGUF training cap is refused"
        );
        let undeclared = serde_json::json!({
            "parameters": "temperature 0.7",
            "model_info": { "llama.context_length": 131072 },
        });
        assert_eq!(
            discovery_ollama_show_context(&undeclared),
            None,
            "no num_ctx declaration means no window fact"
        );
        let malformed = serde_json::json!({ "parameters": "num_ctx zero\nnum_ctx 8192" });
        assert_eq!(
            discovery_ollama_show_context(&malformed),
            None,
            "a malformed declaration cannot be overruled by a later line"
        );
    }

    // --- discovery HTTP lanes (loopback mock) ---

    /// One captured request's facts for assertions.
    #[derive(Default)]
    struct DiscoverMockLog {
        hits: std::sync::Mutex<std::collections::HashMap<String, usize>>,
        headers: std::sync::Mutex<std::collections::HashMap<String, Vec<(String, String)>>>,
        queries: std::sync::Mutex<std::collections::HashMap<String, Vec<String>>>,
    }

    impl DiscoverMockLog {
        fn hits_for(&self, path: &str) -> usize {
            self.hits
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(path)
                .copied()
                .unwrap_or(0)
        }

        fn header_values(&self, path: &str, name: &str) -> Vec<String> {
            self.headers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(path)
                .map(|pairs| {
                    pairs
                        .iter()
                        .filter(|(header, _)| header.eq_ignore_ascii_case(name))
                        .map(|(_, value)| value.clone())
                        .collect()
                })
                .unwrap_or_default()
        }

        fn queries_for(&self, path: &str) -> Vec<String> {
            self.queries
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(path)
                .cloned()
                .unwrap_or_default()
        }
    }

    /// The discovery lanes' std-only loopback mock. `routes` maps a request
    /// PATH (query stripped for matching) to a SEQUENCE of `(status, body)`
    /// answers, consumed in order — a two-page ladder feeds the same path
    /// twice. Unrouted paths answer `404 {}`. All request headers and query
    /// strings are recorded for assertions.
    fn spawn_discover_mock(
        routes: &[(&str, Vec<(u16, &str)>)],
    ) -> (String, std::sync::Arc<DiscoverMockLog>) {
        use std::io::Read as _;
        use std::sync::Arc;

        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind discover mock listener");
        let addr = listener.local_addr().expect("discover mock listener addr");
        let mut queues: std::collections::HashMap<
            String,
            std::collections::VecDeque<(u16, String)>,
        > = std::collections::HashMap::new();
        for (path, answers) in routes {
            queues.insert(
                (*path).to_owned(),
                answers.iter().map(|(s, b)| (*s, (*b).to_owned())).collect(),
            );
        }
        let log = Arc::new(DiscoverMockLog::default());
        let log_thread = log.clone();
        std::thread::spawn(move || {
            // Generous bound: the local short-circuit fans out the candidate
            // probes before the ladder itself runs, and the ollama show-cap
            // test issues ~60 requests on purpose.
            for stream in listener.incoming().take(200) {
                let Ok(mut stream) = stream else { continue };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                loop {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                    if buf.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&buf).into_owned();
                let mut lines = request.split("\r\n");
                let target = lines
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .to_owned();
                let (path, query) = match target.split_once('?') {
                    Some((path, query)) => (path.to_owned(), Some(query.to_owned())),
                    None => (target.clone(), None),
                };
                // Drain the declared body (the Ollama /api/show POSTs):
                // answering before the client wrote its body would make the
                // client's write fail mid-flight and poison the request.
                let header_end = request
                    .find("\r\n\r\n")
                    .map(|index| index + 4)
                    .unwrap_or(buf.len());
                let content_length = request.split("\r\n").skip(1).find_map(|line| {
                    line.split_once(':')
                        .filter(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
                });
                if let Some((_, value)) = content_length
                    && let Ok(length) = value.trim().parse::<usize>()
                {
                    let mut have = buf.len().saturating_sub(header_end);
                    while have < length {
                        match stream.read(&mut chunk) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                buf.extend_from_slice(&chunk[..n]);
                                have += n;
                            }
                        }
                    }
                }
                {
                    let mut hits = log_thread.hits.lock().unwrap_or_else(|p| p.into_inner());
                    *hits.entry(path.clone()).or_insert(0) += 1;
                }
                {
                    let mut headers = log_thread.headers.lock().unwrap_or_else(|p| p.into_inner());
                    headers.entry(path.clone()).or_default().extend(
                        request
                            .split("\r\n")
                            .skip(1)
                            .take_while(|line| !line.is_empty())
                            .filter_map(|line| line.split_once(':'))
                            .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned())),
                    );
                }
                if let Some(query) = query {
                    let mut queries = log_thread.queries.lock().unwrap_or_else(|p| p.into_inner());
                    queries.entry(path.clone()).or_default().push(query);
                }
                let (status, body) = queues
                    .get_mut(&path)
                    .and_then(|queue| queue.pop_front())
                    .unwrap_or((404, "{}".to_owned()));
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
            }
        });
        (format!("http://{addr}"), log)
    }

    /// The truncated-body mock: ONE route (everything else 404s, so the
    /// local short-circuit's candidate probes all miss) answered with a 200
    /// whose Content-Length promises far more bytes than are actually
    /// written, then the connection closes — the mid-stream transport break
    /// a proxy/VPN cut produces, which the ladder must classify as a
    /// retryable transport error, never as "unsupported". Every request to
    /// the route gets the same truncated answer (the candidate round and
    /// the ladder each hit it). Same log type as [`spawn_discover_mock`].
    fn spawn_discover_truncated_mock(path: &str) -> (String, std::sync::Arc<DiscoverMockLog>) {
        use std::io::Read as _;
        use std::sync::Arc;

        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind truncated mock listener");
        let addr = listener.local_addr().expect("truncated mock listener addr");
        let route = path.to_owned();
        let log = Arc::new(DiscoverMockLog::default());
        let log_thread = log.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(50) {
                let Ok(mut stream) = stream else { continue };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                loop {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                    if buf.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&buf).into_owned();
                let target = request
                    .split("\r\n")
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .to_owned();
                let path = target.split('?').next().unwrap_or("").to_owned();
                {
                    let mut hits = log_thread.hits.lock().unwrap_or_else(|p| p.into_inner());
                    *hits.entry(path.clone()).or_insert(0) += 1;
                }
                if path != route {
                    let _ = std::io::Write::write_all(
                        &mut stream,
                        b"HTTP/1.1 404\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}",
                    );
                    continue;
                }
                // The promised body is an order of magnitude larger than the
                // written prefix: the client sees headers for a 200 JSON
                // answer, then the stream dies under it.
                let body = r#"{"data":[{"id":"partial-model"}]}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 4096\r\nconnection: close\r\n\r\n{body}"
                );
                let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
                let _ = std::io::Write::flush(&mut stream);
                // Drop closes the socket with the promised bytes unsent: the
                // mid-stream break under test.
            }
        });
        (format!("http://{addr}"), log)
    }

    /// Every mock-driven ladder test walks the local short-circuit (a
    /// loopback address cannot help being local), so the candidates' seven
    /// signature probes must MISS cleanly: a convenience builder producing
    /// the all-miss route set plus the interesting paths.
    fn discover_routes(
        extra: &[(&'static str, Vec<(u16, &'static str)>)],
    ) -> Vec<(&'static str, Vec<(u16, &'static str)>)> {
        let mut routes: Vec<(&'static str, Vec<(u16, &'static str)>)> = [
            "/api/tags",
            "/api/v0/models",
            "/api/extra/version",
            "/props",
            "/get_server_info",
            "/v1/models",
            "/models",
        ]
        .map(|path| (path, vec![(404u16, "{}")]))
        .into_iter()
        .collect();
        for (path, answers) in extra {
            if let Some(route) = routes.iter_mut().find(|(candidate, _)| candidate == path) {
                route.1 = answers.clone();
            } else {
                routes.push((path, answers.clone()));
            }
        }
        routes
    }

    /// The OpenAI-compatible ladder over a `{data}` envelope: the candidate
    /// round consumes one body (the `/v1/models` 404 fallback), the listing
    /// ladder the second; lengths land endpoint-sourced and the short-circuit
    /// reports the all-miss kind as `generic`. The credential enters through
    /// `--api-key-env` — the mock lanes set the variable and call the real
    /// `discover()` lane, so the env-var entry is exercised end to end (the
    /// LM Studio ladder tests below are the exception: they hand a fixture
    /// key directly to `run_endpoint_discovery` because their subject is
    /// the keyed-ladder route, not the credential entry).
    #[test]
    fn discover_openai_envelope_lists_endpoint_sourced_facts() {
        let body = r#"{"data":[
            {"id":"glm-4.7","max_model_len":131072,"max_completion_tokens":8192},
            {"id":"no-facts"}
        ]}"#;
        let (base, log) = spawn_discover_mock(&discover_routes(&[(
            "/models",
            vec![(200u16, body), (200, body)],
        )]));
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _key = TempEnv::set("PINVOU_TEST_DISCOVER_ROUTE_KEY", " route-key ");
        let outcome = discover(
            &base,
            Some("PINVOU_TEST_DISCOVER_ROUTE_KEY"),
            None,
            Some(DiscoveryProtocol::OpenAiCompatible),
            true,
            OutputMode::Human,
        )
        .expect("a completed discovery is a result, not a command failure");
        assert_eq!(outcome.exit_code, ExitCode::Success);
        let listing: serde_json::Value =
            serde_json::from_str(&outcome.stdout).expect("single-line json");
        assert_eq!(listing["status"], "listed");
        assert_eq!(
            listing["server_kind"], "generic",
            "an all-miss short-circuit round is generic"
        );
        assert_eq!(listing["truncated"], false);
        assert_eq!(
            listing["http_status"],
            serde_json::Value::Null,
            "a listed answer carries no http_status, exactly like the GUI's assembly"
        );
        assert_eq!(listing["models"].as_array().expect("models array").len(), 2);
        assert_eq!(listing["models"][0]["id"], "glm-4.7");
        assert_eq!(listing["models"][0]["context_window_tokens"], 131072);
        assert_eq!(listing["models"][0]["max_output_tokens"], 8192);
        assert_eq!(listing["models"][0]["context_from"], "endpoint");
        assert_eq!(
            listing["models"][1]["context_window_tokens"],
            serde_json::Value::Null,
            "no fabricated lengths"
        );
        // The ladder request must carry the same-origin bearer credential.
        // The path was requested twice (candidate fallback + ladder); every
        // request must carry the same-origin trimmed bearer — the padding on
        // the environment value is trimmed by the ladder, not read verbatim.
        let bearers = log.header_values("/models", "authorization");
        assert!(
            bearers.len() == 2 && bearers.iter().all(|value| value == "Bearer route-key"),
            "the trimmed key goes out as the bearer on every request: {bearers:?}"
        );
    }

    /// The Together bare-array shape: identical listing through the same
    /// ladder, no envelope required.
    #[test]
    fn discover_openai_accepts_the_bare_array_shape() {
        let body = r#"[{"id":"together-model","context_length":32768,"max_output_tokens":4096}]"#;
        let (base, _log) = spawn_discover_mock(&discover_routes(&[(
            "/models",
            vec![(200u16, body), (200, body)],
        )]));
        let listing = run_endpoint_discovery(&base, None, DiscoveryProtocol::OpenAiCompatible);
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(listing.models.len(), 1);
        assert_eq!(listing.models[0].context_window_tokens, Some(32768));
    }

    /// The error ladder, end to end through the mock: 401 → auth_required
    /// (and no retry on other paths), 404 → unsupported, 500 → error, and a
    /// 200 HTML interstitial → unsupported with the status kept.
    #[test]
    fn discover_openai_error_ladder_is_pinned() {
        for (status, expected, name) in [
            (401u16, ListingStatus::AuthRequired, "auth"),
            (403, ListingStatus::AuthRequired, "auth-403"),
            (404, ListingStatus::Unsupported, "unsupported-404"),
            (405, ListingStatus::Unsupported, "unsupported-405"),
            (500, ListingStatus::Error, "error-500"),
        ] {
            let body = "{}";
            // Round-3 review M4b: a bare LOCAL base now retries
            // `{base}/v1/models` on 404/405 (the GUI `ladder_openai_local`
            // mirror), so the raw status rows drive a `/v1`-suffixed base
            // where the ladder answers `/v1/models` directly and no retry
            // applies. The retry itself is pinned by
            // `discover_bare_local_base_retries_v1_models`.
            let v1_base = matches!(status, 404 | 405);
            let (base, _log) = spawn_discover_mock(&discover_routes(&[(
                if v1_base { "/v1/models" } else { "/models" },
                vec![(status, body), (status, body)],
            )]));
            let driven = if v1_base { format!("{base}/v1") } else { base };
            let listing =
                run_endpoint_discovery(&driven, None, DiscoveryProtocol::OpenAiCompatible);
            assert_eq!(listing.status, expected, "{name}");
            assert_eq!(listing.http_status, Some(status), "{name}");
            assert!(
                listing.models.is_empty(),
                "{name}: failures carry no half data"
            );
        }
        let (base, _log) = spawn_discover_mock(&discover_routes(&[(
            "/models",
            vec![
                (200u16, "<html>please log in to the wifi</html>"),
                (200, "<html>"),
            ],
        )]));
        let listing = run_endpoint_discovery(&base, None, DiscoveryProtocol::OpenAiCompatible);
        assert_eq!(listing.status, ListingStatus::Unsupported);
        assert_eq!(
            listing.http_status,
            Some(200),
            "a 2xx non-JSON body keeps its status in the unsupported row"
        );
    }

    /// Redirects are NOT followed: the discovery client pins
    /// `Policy::none()` because reqwest strips only the Authorization/Cookie
    /// family on a host-or-port change, so a followed 302 would carry
    /// `x-api-key` / `x-goog-api-key` to a host the user never named. The
    /// 3xx response therefore arrives as the final status and files under
    /// plain `error` — never a second hop, never a listed/auth/unsupported
    /// reading of whatever the redirect target would have answered. The
    /// 302 carries a real `location` header: a bare 302 without one is
    /// terminal under reqwest's default follow policy too, so a no-follow
    /// pin fed by that shape never bites (round-5 mutation: deleting
    /// `Policy::none()` kept the old queue-mock test green while the same
    /// mutation turned the `models test` pin red). This mock makes the
    /// second hop real — under the mutation the follow lands on
    /// `/elsewhere` and the final row reads `unsupported(404)`; with the
    /// policy in place neither hop happens.
    #[test]
    fn discover_redirect_is_not_followed_and_classifies_as_error() {
        let (base, log) = spawn_discover_redirect_mock();
        let listing = run_endpoint_discovery(&base, None, DiscoveryProtocol::OpenAiCompatible);
        assert_eq!(listing.status, ListingStatus::Error);
        assert_eq!(listing.http_status, Some(302), "the 3xx status is kept");
        assert_eq!(listing.transport, None, "a decided status is not transport");
        assert!(listing.models.is_empty(), "failures carry no half data");
        assert_eq!(
            log.hits_for("/models"),
            2,
            "candidate fallback + ladder, each answered once — no third hop"
        );
        assert_eq!(
            log.hits_for("/elsewhere"),
            0,
            "the location target must never be requested"
        );
    }

    /// The mid-stream classification, end to end on the real `discover()`
    /// lane: a 200 whose headers promise a body the connection then cuts
    /// short (proxy/VPN kill) is a TRANSPORT fact — the row is `error` with
    /// transport `network` and the status kept, exit 1, exactly the GUI
    /// ladder's BrokenBody terminal. Classifying it `unsupported` would pin
    /// a retryable failure as "the endpoint does not speak discovery" — the
    /// misclassification this lane used to make.
    #[test]
    fn discover_midstream_body_break_classifies_as_transport_error() {
        let (base, log) = spawn_discover_truncated_mock("/models");
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _key = TempEnv::set("PINVOU_TEST_DISCOVER_MIDSTREAM_KEY", "midstream-key");
        let outcome = discover(
            &base,
            Some("PINVOU_TEST_DISCOVER_MIDSTREAM_KEY"),
            None,
            Some(DiscoveryProtocol::OpenAiCompatible),
            true,
            OutputMode::Human,
        )
        .expect("a completed discovery is a result, not a command failure");
        assert_eq!(
            outcome.exit_code,
            ExitCode::Failed,
            "a broken body is an error, not a listing: {}",
            outcome.stdout
        );
        let listing: serde_json::Value =
            serde_json::from_str(&outcome.stdout).expect("single-line json");
        assert_eq!(listing["status"], "error");
        assert_eq!(
            listing["transport"], "network",
            "a mid-stream body break is a transport fact, the GUI's own spelling"
        );
        assert_eq!(
            listing["http_status"], 200,
            "the endpoint DID answer; its status stays on the row"
        );
        assert!(
            listing["models"]
                .as_array()
                .expect("models array")
                .is_empty(),
            "failures carry no half data"
        );
        assert_eq!(
            log.hits_for("/models"),
            2,
            "candidate fallback + ladder, each cut short once"
        );
    }

    /// The mid-body TIMEOUT split, end to end (round-3 review minor's CLI
    /// half, still unpinned as of round 5): headers arrive, the body then
    /// STALLS — the client deadline fires mid-read and the row is `error`
    /// with transport `timeout` (retryable), never the fallback `network`.
    /// No status survives (the response never completed), unlike the cut
    /// short above. Writing this test EXPOSED a real bug: the split was
    /// unreachable in practice — the blocking reader's deadline error is
    /// an opaque hyper-util timeout (kind Other, downcast-free) that the
    /// reqwest::Error-only classifier never matched, so every real stall
    /// read as a plain network cut and the round-3 arm existed only for
    /// synthetic errors; `body_read_is_timeout` now matches the canonical
    /// timeout phrase by exact Display equality. Mutation: reverting the
    /// classifier arm turns this test red (network vs timeout). Costs one
    /// real client-timeout window (~8s): the discovery client is a fixed
    /// 8s singleton with no per-test timeout seam, so the wiring can only
    /// be pinned against the real deadline.
    #[test]
    fn discover_mid_body_stall_classifies_as_timeout() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind stalled listener");
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept probe connection");
            let mut buf = [0u8; 1024];
            let _ = std::io::Read::read(&mut stream, &mut buf);
            // Half a 200 answer: content-length promises far more than the
            // bytes written, then the stream HOLDS (no close) — the body
            // read can only end at the client timeout.
            let _ = std::io::Write::write_all(
                &mut stream,
                b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 512\r\nconnection: close\r\n\r\n",
            );
            let _ = std::io::Write::write_all(&mut stream, br#"{"data":[{"id":"partial""#);
            // Hold past the client timeout; the drop afterwards closes the
            // socket and the thread without leaking either.
            std::thread::sleep(Duration::from_secs(12));
        });
        // The anthropic protocol takes no local short-circuit: the ladder
        // dials the bare listener directly, one request.
        let listing = run_endpoint_discovery(
            &format!("http://{addr}/"),
            None,
            DiscoveryProtocol::Anthropic,
        );
        assert_eq!(listing.status, ListingStatus::Error);
        assert_eq!(
            listing.http_status, None,
            "a timeout is not an HTTP-status failure — the response never completed"
        );
        assert_eq!(listing.transport.as_deref(), Some("timeout"));
        assert!(listing.models.is_empty(), "failures carry no half data");
        server.join().expect("stall thread joins");
    }

    /// The Tachyon arm, end to end on the discover lane: a server whose
    /// `/get_server_info` carries the version field SGLang's loose signature
    /// matches (the shape every new Tachyon package exposes) but whose
    /// `/v1/models` entries self-report `owned_by: "tachyon"` must classify
    /// `tachyon` — owned_by is the stronger identity evidence, so the arm
    /// sits BEFORE the sglang arm, the GUI's own ordering. Without that arm
    /// this exact server reads `sglang`, which is the misclassification the
    /// GUI fixed and this pin mirrors.
    #[test]
    fn discover_detects_tachyon_owned_by_before_the_sglang_signature() {
        let models = r#"{"object":"list","data":[{"id":"qwen3.8","owned_by":"tachyon"}]}"#;
        let (base, log) = spawn_discover_mock(&discover_routes(&[
            (
                "/get_server_info",
                vec![(200u16, r#"{"version":"88251e38","model_path":"qwen3.8"}"#)],
            ),
            ("/v1/models", vec![(200u16, models)]),
            ("/models", vec![(200u16, models)]),
        ]));
        let listing = run_endpoint_discovery(&base, None, DiscoveryProtocol::OpenAiCompatible);
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(
            listing.server_kind,
            Some("tachyon"),
            "owned_by tachyon beats the /get_server_info shape sglang's loose signature also \
             matches"
        );
        assert_eq!(listing.models.len(), 1);
        assert_eq!(listing.models[0].id, "qwen3.8");
        assert_eq!(
            log.hits_for("/get_server_info"),
            1,
            "the candidate round's sglang probe"
        );
        assert_eq!(
            log.hits_for("/v1/models"),
            1,
            "the candidate round's owned_by fetch"
        );
        assert_eq!(log.hits_for("/models"), 1, "the ladder's compat fetch");
    }

    /// The anthropic ladder's pagination and its round-4 (M6) host-split
    /// auth, plus the same facts the original test pinned: the two-page
    /// `has_more` + `after_id` pagination and `max_input_tokens` /
    /// `max_tokens` / `display_name` per entry (the protocol ladder runs
    /// even against loopback — the local short-circuit belongs to the
    /// OpenAI-compatible protocol only; the credential enters through
    /// `--api-key-env`, env var set, real `discover()` lane, like every
    /// mock lane that presents a key). The wire half runs against the
    /// loopback mock — a NON-official host — so the ladder must present the
    /// caller's key as a standard BEARER there, with no
    /// `x-api-key`/`anthropic-version` (the old unconditional `x-api-key`
    /// made every non-official anthropic-protocol endpoint look
    /// unauthenticated). The OFFICIAL-host arm (`x-api-key` +
    /// `anthropic-version`, no bearer) cannot reach a loopback mock — the
    /// host gate keys on `api.anthropic.com` — so it is pinned by building
    /// the ladder's own factored request builder and reading its headers,
    /// the same without-a-network seam the connection lane's gateway-header
    /// test uses; that section also pins the gateway affinity header
    /// (`x-opencode-session`, derived from "models-probe") that is a no-op
    /// on the mock's loopback host.
    #[test]
    fn discover_anthropic_pagination_and_the_official_host_auth_split() {
        let page1 = r#"{"data":[{"id":"claude-a","display_name":"Claude A","max_input_tokens":200000,"max_tokens":8192}],"has_more":true}"#;
        let page2 = r#"{"data":[{"id":"claude-b","max_input_tokens":1000000}],"has_more":false}"#;
        let (base, log) =
            spawn_discover_mock(&[("/v1/models", vec![(200u16, page1), (200, page2)])]);
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _key = TempEnv::set("PINVOU_TEST_DISCOVER_ANTHROPIC_KEY", "probe-key");
        let outcome = discover(
            &base,
            Some("PINVOU_TEST_DISCOVER_ANTHROPIC_KEY"),
            None,
            Some(DiscoveryProtocol::Anthropic),
            true,
            OutputMode::Human,
        )
        .expect("a completed discovery is a result, not a command failure");
        assert_eq!(outcome.exit_code, ExitCode::Success);
        let listing: serde_json::Value =
            serde_json::from_str(&outcome.stdout).expect("single-line json");
        assert_eq!(listing["status"], "listed");
        assert_eq!(listing["models"].as_array().expect("models array").len(), 2);
        assert_eq!(listing["models"][0]["context_window_tokens"], 200000);
        assert_eq!(listing["models"][0]["max_output_tokens"], 8192);
        assert_eq!(listing["models"][0]["title"], "Claude A");
        assert_eq!(listing["models"][1]["context_window_tokens"], 1000000);
        assert_eq!(listing["truncated"], false);
        assert_eq!(
            listing["server_kind"],
            serde_json::Value::Null,
            "no short-circuit on the native protocol"
        );
        assert_eq!(log.hits_for("/v1/models"), 2, "two pages, one path");
        // Non-official host: the key goes out as the standard bearer.
        let bearers = log.header_values("/v1/models", "authorization");
        assert!(
            bearers.len() == 2 && bearers.iter().all(|value| value == "Bearer probe-key"),
            "a non-official host takes the bearer credential: {bearers:?}"
        );
        assert!(
            log.header_values("/v1/models", "x-api-key").is_empty()
                && log
                    .header_values("/v1/models", "anthropic-version")
                    .is_empty(),
            "the native headers belong to the official host only"
        );
        assert!(
            log.header_values("/v1/models", "x-opencode-session")
                .is_empty(),
            "off-gateway the affinity header is a no-op"
        );
        let queries = log.queries_for("/v1/models");
        assert_eq!(queries.len(), 2);
        assert_eq!(queries[0], "limit=1000");
        assert_eq!(
            queries[1], "limit=1000&after_id=claude-a",
            "the second page resumes from the first page's last id"
        );

        // The official-host arm, pinned on the ladder's own request builder
        // (nothing is sent): x-api-key + anthropic-version, no bearer, and
        // ASCII case-insensitivity of the host gate.
        let client = discovery_client().expect("discovery client singleton");
        for official_base in ["https://api.anthropic.com", "https://Api.Anthropic.COM/v1"] {
            let request = discovery_anthropic_request(
                client,
                format!("{official_base}/v1/models"),
                Some(" probe-key "),
                official_base,
            )
            .build()
            .unwrap();
            assert_eq!(
                request
                    .headers()
                    .get("x-api-key")
                    .map(|value| value.to_str().expect("header ascii")),
                Some("probe-key"),
                "{official_base}: the key is trimmed into x-api-key"
            );
            assert_eq!(
                request
                    .headers()
                    .get("anthropic-version")
                    .map(|value| value.to_str().expect("header ascii")),
                Some("2023-06-01")
            );
            assert!(
                request.headers().get("authorization").is_none(),
                "{official_base}: the official host never sends a bearer"
            );
        }
        // Keyless sends no auth header in either arm.
        for keyless_base in ["https://api.anthropic.com", "http://127.0.0.1:8000"] {
            let request = discovery_anthropic_request(
                client,
                format!("{keyless_base}/v1/models"),
                None,
                keyless_base,
            )
            .build()
            .unwrap();
            assert!(request.headers().get("x-api-key").is_none());
            assert!(request.headers().get("authorization").is_none());
            assert!(request.headers().get("anthropic-version").is_none());
        }
        // The gateway affinity header rides this ladder too (the GUI
        // attaches it on all three), with the shared "models-probe"
        // derivation — present on a gateway base, keyed on the ORIGINAL
        // base_url, not the request URL.
        let gateway = "https://opencode.ai/zen/go/v1";
        let request = discovery_anthropic_request(
            client,
            format!("{gateway}/v1/models"),
            Some("probe-key"),
            gateway,
        )
        .build()
        .unwrap();
        assert_eq!(
            request
                .headers()
                .get("x-opencode-session")
                .map(|value| value.to_str().expect("header ascii").to_owned()),
            Some(pinvou3_lib::model_probe::opencode_session_id_for(
                "models-probe"
            )),
            "the affinity value matches the shared models-probe derivation"
        );
    }

    /// The COMPAT ladder's gateway affinity header, pinned on its own
    /// request builder (round-5 review m1: the anthropic and gemini
    /// builders were pinned, but the compat lane — the lane the round-3
    /// finding was originally about — had no presence assertion, so
    /// deleting its `with_discovery_affinity` call kept every suite green
    /// while a gateway discovery 400'd with `MissingSessionID`). Same
    /// shared derivation, no-op off the gateway.
    #[test]
    fn discover_compat_request_carries_the_gateway_affinity_header() {
        let client = discovery_client().expect("discovery client singleton");
        let gateway = "https://opencode.ai/zen/go/v1";
        let request = discovery_openai_request(
            client,
            &format!("{gateway}/models"),
            Some("probe-key"),
            gateway,
        )
        .build()
        .unwrap();
        assert_eq!(
            request
                .headers()
                .get("x-opencode-session")
                .map(|value| value.to_str().expect("header ascii").to_owned()),
            Some(pinvou3_lib::model_probe::opencode_session_id_for(
                "models-probe"
            )),
            "the affinity value matches the shared models-probe derivation"
        );
        let request = discovery_openai_request(
            client,
            "https://api.example.com/v1/models",
            Some("probe-key"),
            "https://api.example.com/v1",
        )
        .build()
        .unwrap();
        assert!(
            request.headers().get("x-opencode-session").is_none(),
            "the affinity header is a gateway-only no-op off the gateway"
        );
    }

    /// A still-has_more run after the three-page cap ends TRUNCATED — the
    /// remainder is disclosed, never silently lost.
    #[test]
    fn discover_anthropic_page_cap_truncates() {
        let page = r#"{"data":[{"id":"m"}],"has_more":true}"#;
        let (base, log) =
            spawn_discover_mock(&[("/v1/models", vec![(200u16, page), (200, page), (200, page)])]);
        let listing = run_endpoint_discovery(&base, None, DiscoveryProtocol::Anthropic);
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(listing.models.len(), 3, "one entry per fetched page");
        assert!(listing.truncated);
        assert_eq!(log.hits_for("/v1/models"), 3, "the cap is three pages");
    }

    /// The native lanes' total-entry cap (the GUI's same-wave
    /// `NATIVE_LISTING_CAP`): three still-more pages of 700 entries each
    /// accumulate 2100 rows, and both ladders end at exactly 2000 with the
    /// `truncated` flag — entry 2001 is disclosed, never silently kept or
    /// lost. (Exactly 2000 entries would stay unflagged, the same
    /// exact-boundary rule the OpenAI-compatible lane pins.)
    #[test]
    fn discover_native_listing_cap_truncates_at_2000_entries() {
        let anthropic_page = {
            let entries: Vec<serde_json::Value> = (0..700)
                .map(|index| {
                    serde_json::json!({ "id": format!("a{index}"), "max_input_tokens": 4096 })
                })
                .collect();
            serde_json::json!({ "data": entries, "has_more": true }).to_string()
        };
        // The route table is &'static; dynamically built bodies leak their
        // bytes for the test's lifetime, the same pattern the ollama
        // show-cap test uses for its 52-entry roster.
        let anthropic_page: &'static str = Box::leak(anthropic_page.into_boxed_str());
        let (base, log) = spawn_discover_mock(&[(
            "/v1/models",
            vec![
                (200u16, anthropic_page),
                (200, anthropic_page),
                (200, anthropic_page),
            ],
        )]);
        let listing = run_endpoint_discovery(&base, None, DiscoveryProtocol::Anthropic);
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(
            listing.models.len(),
            2000,
            "2100 accumulated entries end at the cap"
        );
        assert!(listing.truncated, "the cap cut 100 entries off");
        assert_eq!(log.hits_for("/v1/models"), 3, "all three pages fetched");
        assert_eq!(listing.models[699].id, "a699", "page one intact");
        assert_eq!(listing.models[1399].id, "a699", "page two intact");
        assert_eq!(
            listing.models[1999].id, "a599",
            "the 2001st entry is the first casualty"
        );

        // Gemini: the same bound on the nextPageToken ladder.
        let gemini_page = {
            let entries: Vec<serde_json::Value> = (0..700)
                .map(|index| {
                    serde_json::json!({
                        "name": format!("models/g{index}"),
                        "supportedGenerationMethods": ["generateContent"],
                    })
                })
                .collect();
            serde_json::json!({ "models": entries, "nextPageToken": "next" }).to_string()
        };
        let gemini_page: &'static str = Box::leak(gemini_page.into_boxed_str());
        let (base, log) = spawn_discover_mock(&[(
            "/v1beta/models",
            vec![
                (200u16, gemini_page),
                (200, gemini_page),
                (200, gemini_page),
            ],
        )]);
        let listing = run_endpoint_discovery(&base, None, DiscoveryProtocol::Gemini);
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(listing.models.len(), 2000);
        assert!(listing.truncated);
        assert_eq!(log.hits_for("/v1beta/models"), 3);
    }

    /// A configured root already ending in `/v1` appends `/models`
    /// directly (the `anthropic_messages_url` normalization rule) — no
    /// doubled version segment.
    #[test]
    fn discover_anthropic_v1_shaped_base_appends_directly() {
        let (base, log) = spawn_discover_mock(&[(
            "/v1/models",
            vec![(200u16, r#"{"data":[],"has_more":false}"#)],
        )]);
        let base = format!("{base}/v1");
        let listing = run_endpoint_discovery(&base, None, DiscoveryProtocol::Anthropic);
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(log.hits_for("/v1/models"), 1);
        assert_eq!(log.hits_for("/v1/v1/models"), 0, "no doubled /v1");
    }

    /// Gemini: `/openai` (and `/v1beta`) suffixes stripped to the native
    /// root, `x-goog-api-key` auth, generateContent-only filtering and the
    /// `nextPageToken` pagination. The credential enters through
    /// `--api-key-env` (env var set, real `discover()` lane), like every
    /// mock lane that presents a key.
    #[test]
    fn discover_gemini_strips_suffixes_filters_and_paginates() {
        let page1 = r#"{"models":[
            {"name":"models/gemini-x","supportedGenerationMethods":["generateContent"],"inputTokenLimit":1000000,"outputTokenLimit":8192,"displayName":"Gemini X"},
            {"name":"models/embed-thing","supportedGenerationMethods":["embedContent"]}
        ],"nextPageToken":"tok-1"}"#;
        let page2 = r#"{"models":[{"name":"models/gemini-y","supportedGenerationMethods":["generateContent"]}]}"#;
        let (base, log) =
            spawn_discover_mock(&[("/v1beta/models", vec![(200u16, page1), (200, page2)])]);
        let base = format!("{base}/openai");
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _key = TempEnv::set("PINVOU_TEST_DISCOVER_GEMINI_KEY", "goog-key");
        let outcome = discover(
            &base,
            Some("PINVOU_TEST_DISCOVER_GEMINI_KEY"),
            None,
            Some(DiscoveryProtocol::Gemini),
            true,
            OutputMode::Human,
        )
        .expect("a completed discovery is a result, not a command failure");
        assert_eq!(outcome.exit_code, ExitCode::Success);
        let listing: serde_json::Value =
            serde_json::from_str(&outcome.stdout).expect("single-line json");
        assert_eq!(listing["status"], "listed");
        let ids: Vec<&str> = listing["models"]
            .as_array()
            .expect("models array")
            .iter()
            .map(|model| model["id"].as_str().expect("every row carries an id"))
            .collect();
        assert_eq!(
            ids,
            ["gemini-x", "gemini-y"],
            "the embed entry is filtered out"
        );
        assert_eq!(listing["models"][0]["context_window_tokens"], 1000000);
        assert_eq!(listing["models"][0]["title"], "Gemini X");
        assert_eq!(listing["truncated"], false);
        assert_eq!(log.hits_for("/v1beta/models"), 2);
        let keys = log.header_values("/v1beta/models", "x-goog-api-key");
        assert!(
            keys.len() == 2 && keys.iter().all(|value| value == "goog-key"),
            "every page presents x-goog-api-key: {keys:?}"
        );
        assert!(
            log.header_values("/v1beta/models", "x-opencode-session")
                .is_empty(),
            "off-gateway the affinity header is a no-op"
        );
        let queries = log.queries_for("/v1beta/models");
        assert_eq!(queries[0], "pageSize=1000");
        assert_eq!(queries[1], "pageSize=1000&pageToken=tok-1");
        // The gateway session-affinity header rides this ladder too (the
        // GUI attaches it on all three; round-3 review minor made the CLI
        // match). A loopback mock can never be a gateway, so the presence
        // half is pinned on the ladder's own factored request builder —
        // the same without-a-network seam the connection lane's gateway
        // header test uses — including the shared "models-probe" derivation
        // and the host-agnostic `x-goog-api-key` (no anthropic-style host
        // split on the gemini ladder).
        let client = discovery_client().expect("discovery client singleton");
        let gateway = "https://opencode.ai/zen/go/v1";
        let request = discovery_gemini_request(
            client,
            format!("{gateway}/v1beta/models"),
            Some("goog-key"),
            gateway,
        )
        .build()
        .unwrap();
        assert_eq!(
            request
                .headers()
                .get("x-opencode-session")
                .map(|value| value.to_str().expect("header ascii").to_owned()),
            Some(pinvou3_lib::model_probe::opencode_session_id_for(
                "models-probe"
            )),
            "the affinity value matches the shared models-probe derivation"
        );
        assert_eq!(
            request
                .headers()
                .get("x-goog-api-key")
                .map(|value| value.to_str().expect("header ascii").to_owned()),
            Some("goog-key".to_owned()),
            "the gemini ladder authenticates with x-goog-api-key on every host"
        );
    }

    /// The ollama chain end to end: `/api/tags` for the downloaded list,
    /// `/api/ps` for the loaded set + effective context, `/api/show` only
    /// for the entry the ps table could not answer — and the GGUF training
    /// window never adopted.
    #[test]
    fn discover_ollama_walks_ps_tags_show_chain() {
        let tags = r#"{"models":[{"name":"llama3:latest"},{"name":"qwen2:7b"}]}"#;
        let ps = r#"{"models":[{"name":"llama3:latest","context_length":8192}]}"#;
        // No num_ctx declaration: only the GGUF training cap is present, and
        // that is deliberately not adopted.
        let show =
            r#"{"parameters":"temperature 0.7","model_info":{"qwen2.context_length":32768}}"#;
        let (base, log) = spawn_discover_mock(&discover_routes(&[
            ("/api/tags", vec![(200u16, tags), (200, tags)]),
            ("/api/ps", vec![(200u16, ps)]),
            ("/api/show", vec![(200u16, show)]),
        ]));
        let listing = run_endpoint_discovery(&base, None, DiscoveryProtocol::OpenAiCompatible);
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(listing.server_kind, Some("ollama"));
        assert!(!listing.truncated);
        assert_eq!(listing.models.len(), 2);
        assert_eq!(listing.models[0].id, "llama3:latest");
        assert_eq!(
            listing.models[0].context_window_tokens,
            Some(8192),
            "the /api/ps effective value"
        );
        assert_eq!(listing.models[0].loaded, Some(true));
        assert_eq!(
            listing.models[1].context_window_tokens, None,
            "the /api/show GGUF training cap must NOT fill the window"
        );
        assert_eq!(listing.models[1].loaded, Some(false));
        assert_eq!(
            log.hits_for("/api/tags"),
            2,
            "the candidate probe plus the listing fetch"
        );
        assert_eq!(log.hits_for("/api/ps"), 1);
        assert_eq!(
            log.hits_for("/api/show"),
            1,
            "only the entry without a ps fact is queried"
        );
    }

    /// The show-lookup budget: at most the first 50 context-MISSING entries
    /// get a `/api/show` round trip; more missing entries than the cap flag
    /// `truncated` (every model still listed).
    #[test]
    fn discover_ollama_show_lookups_cap_at_fifty() {
        // 52 tags, the ps table answers one: 51 missing > 50 → truncated,
        // 50 show queries issued.
        let tags_json = serde_json::json!({
            "models": (0..52).map(|index| serde_json::json!({ "name": format!("m{index:02}") })).collect::<Vec<_>>()
        })
        .to_string();
        // The route table is &'static; the one dynamically built body here
        // leaks its bytes for the test's lifetime (a 52-entry roster).
        let tags: &'static str = Box::leak(tags_json.into_boxed_str());
        let ps = r#"{"models":[{"name":"m00","context_length":4096}]}"#;
        let show = r#"{"parameters":"num_ctx 2048"}"#;
        let (base, log) = spawn_discover_mock(&discover_routes(&[
            ("/api/tags", vec![(200u16, tags), (200, tags)]),
            ("/api/ps", vec![(200u16, ps)]),
            ("/api/show", vec![(200u16, show); 50]),
        ]));
        let listing = run_endpoint_discovery(&base, None, DiscoveryProtocol::OpenAiCompatible);
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(listing.models.len(), 52, "every downloaded model is listed");
        assert!(
            listing.truncated,
            "51 missing facts over the 50 cap flag truncation"
        );
        assert_eq!(log.hits_for("/api/show"), 50, "the budget is 50 lookups");
        assert_eq!(
            listing.models[1].context_window_tokens,
            Some(2048),
            "the looked-up num_ctx lands endpoint-sourced"
        );
        assert_eq!(
            listing.models[51].context_window_tokens, None,
            "entries past the budget stay undeclared"
        );
    }

    /// Round-3 review M4b: a bare local base (no `/v1` segment) whose
    /// `{base}/models` 404s retries `{base}/v1/models` once — the GUI's
    /// `ladder_openai_local` mirror. The GUI listed here; the CLI previously
    /// pinned the same healthy vLLM as `unsupported(404)`, contradicting the
    /// mirror claim. The kind probe (vLLM owned_by via `/v1/models`) and the
    /// listing retry are the two `/v1/models` hits.
    #[test]
    fn discover_bare_local_base_retries_v1_models() {
        // Queue shape (the candidate round runs first): the kind probe's
        // primary `/v1/models` 404s and its `/models` fallback 404s; the
        // ladder's primary `/models` 404s again; the bare-base listing
        // retry finally hits `/v1/models` and lists.
        let (base, log) = spawn_discover_mock(&discover_routes(&[
            ("/models", vec![(404u16, "{}"), (404, "{}")]),
            (
                "/v1/models",
                vec![
                    (404u16, "{}"),
                    (
                        200u16,
                        r#"{"data":[{"id":"bare-vllm","max_model_len":4096}]}"#,
                    ),
                ],
            ),
        ]));
        let listing = run_endpoint_discovery(&base, None, DiscoveryProtocol::OpenAiCompatible);
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(listing.server_kind, Some("generic"));
        assert_eq!(listing.models.len(), 1);
        assert_eq!(listing.models[0].id, "bare-vllm");
        assert_eq!(listing.models[0].context_window_tokens, Some(4096));
        assert_eq!(
            log.hits_for("/models"),
            2,
            "the kind probe fallback plus the ladder primary"
        );
        assert_eq!(
            log.hits_for("/v1/models"),
            2,
            "the kind probe primary plus the bare-base listing retry"
        );
    }

    /// Round-3 review M4a: a detected Ollama whose `/api/tags` fails
    /// TRANSIENTLY (busy 503) reports the REAL classification (error, 503)
    /// after the re-check — never `unsupported(404)` from a compat-ladder
    /// rerun. The GUI's `ollama_native_failure` invariant, mirrored: hit 1
    /// is the kind probe, hit 2 the ladder fetch, hit 3 the re-check; the
    /// same 503 twice is a confirmed transient, not a shape verdict.
    #[test]
    fn discover_busy_ollama_reports_real_failure_not_unsupported() {
        let tags = r#"{"models":[{"name":"llama3:latest"}]}"#;
        let (base, log) = spawn_discover_mock(&discover_routes(&[
            ("/api/tags", vec![(200u16, tags), (503, "{}"), (503, "{}")]),
            ("/api/ps", vec![(503u16, "{}")]),
        ]));
        let listing = run_endpoint_discovery(&base, None, DiscoveryProtocol::OpenAiCompatible);
        assert_eq!(
            listing.status,
            ListingStatus::Error,
            "a busy Ollama is a retryable error, not \"endpoint has no list interface\""
        );
        assert_eq!(listing.http_status, Some(503));
        assert_eq!(listing.server_kind, Some("ollama"));
        assert_eq!(
            log.hits_for("/api/tags"),
            3,
            "kind probe + ladder fetch + the transient re-check"
        );
    }

    /// The re-check's recovery arm: the re-run `/api/tags` answers
    /// well-formed (here: an empty roster), so the historical fall-back
    /// semantics stand — the compat ladder decides (unsupported on this
    /// mock, via the local `/v1` retry).
    #[test]
    fn discover_ollama_transient_recheck_ok_falls_back_to_compat() {
        let tags = r#"{"models":[{"name":"llama3:latest"}]}"#;
        let (base, log) = spawn_discover_mock(&discover_routes(&[
            (
                "/api/tags",
                vec![(200u16, tags), (503, "{}"), (200u16, r#"{"models":[]}"#)],
            ),
            ("/api/ps", vec![(503u16, "{}")]),
            ("/models", vec![(404u16, "{}")]),
            ("/v1/models", vec![(404u16, "{}"), (404u16, "{}")]),
        ]));
        let listing = run_endpoint_discovery(&base, None, DiscoveryProtocol::OpenAiCompatible);
        assert_eq!(listing.status, ListingStatus::Unsupported);
        assert_eq!(listing.server_kind, Some("ollama"));
        assert_eq!(log.hits_for("/api/tags"), 3);
    }

    /// The recheck's recovery arm (round-5 GUI parity for the round-3 m1-6
    /// retry): a recheck that comes back healthy AND NON-EMPTY retries the
    /// native listing once — the blip passed, so the CLI must report the
    /// native facts (`/api/ps` loaded state + effective windows) instead of
    /// collapsing to the compat ladder, which on an `/api`-only deployment
    /// would report `unsupported(404)` where the GUI lists. Mirrors the
    /// GUI's `ollama_transient_blip_retries_native_after_healthy_recheck`
    /// fixture: tags hit 4× (kind + ladder 503 + recheck + retry), ps 2×
    /// (ladder + retry), show 0× (ps already carried the effective window),
    /// and the compat ladder is never entered. The retry cannot loop: it
    /// runs without a second recheck.
    #[test]
    fn discover_ollama_transient_recheck_retries_native_when_healthy() {
        let tags = r#"{"models":[{"name":"llama3:latest"}]}"#;
        let ps = r#"{"models":[{"name":"llama3:latest","context_length":8192}]}"#;
        let (base, log) = spawn_discover_mock(&discover_routes(&[
            (
                "/api/tags",
                vec![(200u16, tags), (503, "{}"), (200u16, tags), (200u16, tags)],
            ),
            ("/api/ps", vec![(200u16, ps), (200u16, ps)]),
        ]));
        let listing = run_endpoint_discovery(&base, None, DiscoveryProtocol::OpenAiCompatible);
        assert_eq!(
            listing.status,
            ListingStatus::Listed,
            "a healthy non-empty recheck retries the native listing instead of the compat ladder"
        );
        assert_eq!(listing.server_kind, Some("ollama"));
        assert!(!listing.truncated);
        assert_eq!(listing.models.len(), 1);
        assert_eq!(listing.models[0].id, "llama3:latest");
        assert_eq!(
            listing.models[0].context_window_tokens,
            Some(8_192),
            "the retried listing carries the /api/ps effective window"
        );
        assert_eq!(
            log.hits_for("/api/tags"),
            4,
            "kind probe + ladder fetch + recheck + the single native retry"
        );
        assert_eq!(log.hits_for("/api/ps"), 2, "ladder + retry");
        assert_eq!(
            log.hits_for("/api/show"),
            0,
            "ps already carried the window"
        );
    }

    /// Round-3 review minor: a malformed `has_more` ("yes" as a string)
    /// ends the run TRUNCATED after one page — never silently complete, and
    /// no blind extra page. (The old `unwrap_or(false)` collapsed it to an
    /// untruncated complete listing.)
    #[test]
    fn discover_anthropic_malformed_has_more_ends_truncated() {
        let (base, log) = spawn_discover_mock(&discover_routes(&[(
            "/v1/models",
            vec![(
                200u16,
                r#"{"data":[{"id":"claude-a","max_input_tokens":200000}],"has_more":"yes"}"#,
            )],
        )]));
        let listing = run_endpoint_discovery(&base, None, DiscoveryProtocol::Anthropic);
        assert_eq!(listing.status, ListingStatus::Listed);
        assert!(
            listing.truncated,
            "malformed has_more must not read complete"
        );
        assert_eq!(listing.models.len(), 1);
        assert_eq!(log.hits_for("/v1/models"), 1, "no blind extra page");
    }

    /// Round-3 review minor: an entry WITHOUT `supportedGenerationMethods`
    /// is kept (a gateway omitting the field must not filter the listing to
    /// zero rows); only an explicit list without generateContent filters.
    #[test]
    fn discover_gemini_entry_without_generation_methods_is_kept() {
        let (base, log) = spawn_discover_mock(&discover_routes(&[(
            "/v1beta/models",
            vec![(
                200u16,
                r#"{"models":[
                    {"name":"models/gateway-pro","inputTokenLimit":960000},
                    {"name":"models/gateway-embed",
                     "supportedGenerationMethods":["embeddingContent"]}
                ]}"#,
            )],
        )]));
        let listing = run_endpoint_discovery(&base, None, DiscoveryProtocol::Gemini);
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(
            listing.models.len(),
            1,
            "missing-field kept, embedding filtered"
        );
        assert_eq!(listing.models[0].id, "gateway-pro");
        assert_eq!(listing.models[0].context_window_tokens, Some(960_000));
        assert_eq!(log.hits_for("/v1beta/models"), 1);
    }

    /// The lmstudio ladder's non-listing-v0 recovery (round-4 review M5,
    /// subsuming the round-3 keyed-fallback minor): a v0 round that stops
    /// yielding a usable listing between the candidate round and the ladder
    /// fetch (here: empty `data`) falls to the KEYED local
    /// OpenAI-compatible ladder — `{base}/models` first, the
    /// `/v1/models` retry on 404/405 — the GUI's `ladder_openai_local`
    /// mirror, not a narrower direct `/v1/models` hop. Every leg of the
    /// compat ladder rides the caller's key.
    #[test]
    fn discover_lmstudio_v0_failure_rides_the_keyed_compat_ladder() {
        let (base, log) = spawn_discover_mock(&discover_routes(&[
            (
                "/api/v0/models",
                vec![
                    (
                        200u16,
                        r#"{"data":[{"id":"warmup","state":"loaded","loaded_context_length":8}]}"#,
                    ),
                    (200u16, r#"{"data":[]}"#),
                ],
            ),
            (
                "/v1/models",
                vec![
                    (
                        200u16,
                        r#"{"data":[{"id":"authed-x","max_model_len":4096}]}"#,
                    ),
                    (200, r#"{"data":[{"id":"authed-x","max_model_len":4096}]}"#),
                ],
            ),
        ]));
        let listing = run_endpoint_discovery(
            &base,
            Some("sk-live-123"),
            DiscoveryProtocol::OpenAiCompatible,
        );
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(listing.server_kind, Some("lm_studio"));
        assert_eq!(listing.models[0].id, "authed-x");
        // Both `/v1/models` rounds (the kind probe and the compat retry)
        // ride the caller's key, and so does the `{base}/models` first leg
        // the compat ladder tries before its 404-driven retry.
        assert_eq!(
            log.header_values("/v1/models", "authorization"),
            vec![
                "Bearer sk-live-123".to_string(),
                "Bearer sk-live-123".to_string()
            ],
            "the compat fallback must send the caller's key"
        );
        assert_eq!(
            log.header_values("/models", "authorization"),
            vec!["Bearer sk-live-123".to_string()],
            "the compat ladder's first leg is keyed too"
        );
        assert_eq!(log.hits_for("/api/v0/models"), 2);
        assert_eq!(log.hits_for("/models"), 1, "the compat ladder's first leg");
        assert_eq!(log.hits_for("/v1/models"), 2);
    }

    /// Round-4 review M5: an authenticated LM Studio answers the keyless
    /// native v0 round with 401 — a DECIDED status the old ladder reported
    /// terminally as `auth_required`, where the GUI (whose
    /// `lmstudio_discovered` returns `None` on ANY v0 failure) falls through
    /// to the keyed compat ladder and LISTS. The queue order is
    /// deterministic because the candidate round completes before the
    /// ladder starts: the candidate sees the usable v0 shape (kind =
    /// lmstudio), the ladder's own v0 fetch eats the 401.
    #[test]
    fn discover_lmstudio_v0_auth_failure_lists_through_the_keyed_ladder() {
        let (base, log) = spawn_discover_mock(&discover_routes(&[
            (
                "/api/v0/models",
                vec![
                    (
                        200u16,
                        r#"{"data":[{"id":"warmup","state":"loaded","loaded_context_length":8}]}"#,
                    ),
                    (401u16, r#"{"error":"unauthorized"}"#),
                ],
            ),
            (
                "/v1/models",
                vec![
                    (
                        200u16,
                        r#"{"data":[{"id":"authed-x","max_model_len":4096}]}"#,
                    ),
                    (200, r#"{"data":[{"id":"authed-x","max_model_len":4096}]}"#),
                ],
            ),
        ]));
        let listing = run_endpoint_discovery(
            &base,
            Some("sk-live-123"),
            DiscoveryProtocol::OpenAiCompatible,
        );
        assert_eq!(
            listing.status,
            ListingStatus::Listed,
            "a v0 401 must not terminate the ladder where the GUI lists"
        );
        assert_eq!(listing.server_kind, Some("lm_studio"));
        assert_eq!(listing.models[0].id, "authed-x");
        assert_eq!(listing.models[0].context_window_tokens, Some(4096));
        assert_eq!(log.hits_for("/api/v0/models"), 2, "candidate plus ladder");
        assert_eq!(log.hits_for("/models"), 1, "the compat ladder's first leg");
        assert_eq!(
            log.hits_for("/v1/models"),
            2,
            "the kind probe plus the compat retry"
        );
        assert_eq!(
            log.header_values("/v1/models", "authorization"),
            vec![
                "Bearer sk-live-123".to_string(),
                "Bearer sk-live-123".to_string()
            ],
            "the keyed retry carries the credential the v0 round lacked"
        );
    }

    /// The v0 round failing at the TRANSPORT layer (the mock serves exactly
    /// one usable v0 answer — the candidate's — and closes every later
    /// connection without an HTTP response): the ladder re-enters the keyed
    /// compat ladder, whose own transport failure surfaces at its REAL
    /// class — `error` with transport `network` and no http status — never
    /// laundered into `unsupported` by the 404 default of a live-but-broken
    /// endpoint.
    #[test]
    fn discover_lmstudio_v0_transport_failure_surfaces_the_real_class() {
        let (base, log) = spawn_discover_v0_then_close_mock();
        let listing = run_endpoint_discovery(
            &base,
            Some("sk-live-123"),
            DiscoveryProtocol::OpenAiCompatible,
        );
        assert_eq!(listing.status, ListingStatus::Error);
        assert_eq!(listing.http_status, None);
        assert_eq!(
            listing.transport,
            Some("network"),
            "a connection closed before any response is a transport break"
        );
        assert_eq!(
            listing.server_kind,
            Some("lm_studio"),
            "the candidate's v0 answer selected the lmstudio branch"
        );
        assert!(listing.models.is_empty(), "failures carry no half data");
        assert_eq!(log.hits_for("/api/v0/models"), 2, "candidate plus ladder");
        assert_eq!(
            log.hits_for("/models"),
            1,
            "the compat ladder's first leg fails at transport and does not retry"
        );
        assert_eq!(
            log.hits_for("/v1/models"),
            1,
            "only the candidate kind probe; a transport failure never retries"
        );
    }

    /// The transport-failure mock for
    /// [`discover_lmstudio_v0_transport_failure_surfaces_the_real_class`]:
    /// the FIRST `/api/v0/models` request (the candidate round's signature
    /// probe) is answered with a usable v0 listing — that is what selects
    /// the `lmstudio` kind — and every LATER request on any path is served
    /// by accepting the connection and closing it WITHOUT an HTTP response,
    /// the transport break a crashed or killed server produces (distinct
    /// from [`spawn_discover_truncated_mock`]'s mid-BODY cut: here no
    /// status line is ever sent, so the request fails at the transport
    /// layer). Same log type as [`spawn_discover_mock`].
    fn spawn_discover_v0_then_close_mock() -> (String, std::sync::Arc<DiscoverMockLog>) {
        use std::io::Read as _;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind v0-then-close mock listener");
        let addr = listener
            .local_addr()
            .expect("v0-then-close mock listener addr");
        let served_v0 = AtomicBool::new(false);
        let log = Arc::new(DiscoverMockLog::default());
        let log_thread = log.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(50) {
                let Ok(mut stream) = stream else { continue };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                loop {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                    if buf.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&buf).into_owned();
                let target = request
                    .split("\r\n")
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .to_owned();
                let path = target.split('?').next().unwrap_or("").to_owned();
                {
                    let mut hits = log_thread.hits.lock().unwrap_or_else(|p| p.into_inner());
                    *hits.entry(path.clone()).or_insert(0) += 1;
                }
                let first_v0 = path == "/api/v0/models" && !served_v0.swap(true, Ordering::SeqCst);
                if first_v0 {
                    let body =
                        r#"{"data":[{"id":"warmup","state":"loaded","loaded_context_length":8}]}"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
                }
                // Otherwise drop the stream with nothing written: the client
                // sees a connection closed before any response byte — the
                // transport failure under test.
            }
        });
        (format!("http://{addr}"), log)
    }

    /// Round-3 test-gap pin: the CLI has NO catalog, so `SOURCE` is always
    /// `endpoint` and a catalog-known id with no self-reported lengths stays
    /// UNDECLARED. The GUI's local-lane catalog skip (round-3 review M1)
    /// must never grow a CLI twin by accident.
    #[test]
    fn discover_never_fills_lengths_from_a_catalog() {
        let (base, _log) = spawn_discover_mock(&discover_routes(&[(
            "/models",
            vec![
                (
                    200u16,
                    r#"{"data":[{"id":"kimi-k3"},{"id":"deepseek-r1-distill-qwen-7b"}]}"#,
                ),
                (
                    200,
                    r#"{"data":[{"id":"kimi-k3"},{"id":"deepseek-r1-distill-qwen-7b"}]}"#,
                ),
            ],
        )]));
        let listing = run_endpoint_discovery(&base, None, DiscoveryProtocol::OpenAiCompatible);
        assert_eq!(listing.status, ListingStatus::Listed);
        for model in &listing.models {
            assert_eq!(
                model.context_window_tokens, None,
                "{}: the CLI lane has no catalog back-fill",
                model.id
            );
            assert_eq!(model.context_from, None);
            assert_eq!(model.max_output_tokens, None);
        }
    }

    /// LM Studio: the v0 listing is served twice (candidate + ladder) and
    /// carries loaded states; a v0 answer that stops yielding a usable
    /// listing between the candidate round and the ladder fetch (the race
    /// the fallback exists for) falls back to `/v1/models` in the
    /// OpenAI-compatible shape.
    #[test]
    fn discover_lmstudio_uses_v0_and_falls_back_to_v1() {
        let v0 = r#"{"data":[
            {"id":"loaded","state":"loaded","loaded_context_length":12918,"max_context_length":131072},
            {"id":"cold","state":"not-loaded","max_context_length":4096}
        ]}"#;
        let (base, log) = spawn_discover_mock(&discover_routes(&[(
            "/api/v0/models",
            vec![(200u16, v0), (200, v0)],
        )]));
        let listing = run_endpoint_discovery(&base, None, DiscoveryProtocol::OpenAiCompatible);
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(
            listing.server_kind,
            Some("lm_studio"),
            "discover reports the GUI vocabulary, not the detector's internal spelling"
        );
        assert_eq!(listing.models.len(), 2);
        assert_eq!(
            listing.models[0].context_window_tokens,
            Some(12918),
            "the served window; the capability cap stays refused"
        );
        assert_eq!(listing.models[1].loaded, Some(false));
        assert_eq!(log.hits_for("/api/v0/models"), 2);
        // The candidate signature probe fetched /v1/models once (all-miss
        // 404); the ladder never fell back.
        assert_eq!(log.hits_for("/v1/models"), 1);

        // The candidate round sees a usable v0 listing (kind = lmstudio),
        // but the ladder's own v0 fetch comes back EMPTY: the fallback
        // serves the listing from `/v1/models` (loaded unknown). The queue
        // order is deterministic because the candidate round completes
        // before the ladder starts.
        let (base, log) = spawn_discover_mock(&discover_routes(&[
            (
                "/api/v0/models",
                vec![
                    (
                        200u16,
                        r#"{"data":[{"id":"warmup","state":"loaded","loaded_context_length":8}]}"#,
                    ),
                    (200, r#"{"data":[]}"#),
                ],
            ),
            (
                "/v1/models",
                vec![(200u16, r#"{"data":[{"id":"x","max_model_len":4096}]}"#); 2],
            ),
        ]));
        let listing = run_endpoint_discovery(&base, None, DiscoveryProtocol::OpenAiCompatible);
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(listing.server_kind, Some("lm_studio"));
        assert_eq!(listing.models.len(), 1);
        assert_eq!(listing.models[0].id, "x");
        assert_eq!(listing.models[0].context_window_tokens, Some(4096));
        assert_eq!(
            listing.models[0].loaded, None,
            "the fallback shape has no state fact"
        );
        assert_eq!(
            log.hits_for("/api/v0/models"),
            2,
            "candidate round plus the ladder fetch"
        );
        assert_eq!(
            log.hits_for("/v1/models"),
            2,
            "the candidate probe plus the ladder's fallback"
        );
    }

    /// The redaction doctrine: transport failures classify from reqwest's
    /// structured flags, so the error row carries NEITHER the raw error text
    /// (URL and userinfo included) NOR the api key. The key now enters
    /// through `--api-key-env`, so the whole lane runs end to end: env var
    /// set, real `discover()` call, both output modes. Port 1 is refused
    /// locally, so the test stays hermetic.
    #[test]
    fn discover_transport_failure_leaks_neither_url_nor_key() {
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _key = TempEnv::set("PINVOU_TEST_DISCOVER_LEAK_KEY", "super-secret-key");
        for (json, output) in [(false, OutputMode::Human), (true, OutputMode::Json)] {
            let outcome = discover(
                "http://user:pass@127.0.0.1:1",
                Some("PINVOU_TEST_DISCOVER_LEAK_KEY"),
                None,
                Some(DiscoveryProtocol::OpenAiCompatible),
                json,
                output,
            )
            .expect("a completed discovery is a result, not a command failure");
            assert_eq!(outcome.exit_code, ExitCode::Failed);
            if json {
                let value: serde_json::Value =
                    serde_json::from_str(&outcome.stdout).expect("single-line json");
                assert_eq!(value["status"], "error");
                assert_eq!(value["transport"], "connect");
                assert_eq!(value["http_status"], serde_json::Value::Null);
            }
            for secret in ["user:pass", "127.0.0.1:1", "super-secret-key"] {
                assert!(
                    !outcome.stdout.contains(secret),
                    "{output:?} output must not carry {secret}: {}",
                    outcome.stdout
                );
            }
        }
    }

    /// The env→value step inherits probe-local's contract: a named-but-unset
    /// or set-but-empty variable fails the command as a host failure
    /// (exit 1, after the usage gates), the message names the VARIABLE and
    /// never echoes any value.
    #[test]
    fn discover_key_env_var_failures_name_the_variable_not_the_value() {
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        assert!(
            std::env::var("PINVOU_TEST_DISCOVER_UNSET_KEY").is_err(),
            "precondition: this test's missing-variable name must start unset"
        );
        let missing = discover(
            "http://127.0.0.1:1",
            Some("PINVOU_TEST_DISCOVER_UNSET_KEY"),
            None,
            Some(DiscoveryProtocol::Anthropic),
            true,
            OutputMode::Human,
        )
        .expect_err("a named-but-unset key variable fails the command");
        assert_eq!(missing.exit_code(), ExitCode::Failed);
        let message = missing.to_string();
        assert!(
            message.contains("PINVOU_TEST_DISCOVER_UNSET_KEY") && message.contains("is not set"),
            "the error names the variable: {message}"
        );
        let _empty = TempEnv::set("PINVOU_TEST_DISCOVER_EMPTY_KEY", "   ");
        let empty = discover(
            "http://127.0.0.1:1",
            Some("PINVOU_TEST_DISCOVER_EMPTY_KEY"),
            None,
            Some(DiscoveryProtocol::Anthropic),
            true,
            OutputMode::Human,
        )
        .expect_err("a set-but-empty key variable fails the command");
        assert_eq!(empty.exit_code(), ExitCode::Failed);
        let message = empty.to_string();
        assert!(
            message.contains("PINVOU_TEST_DISCOVER_EMPTY_KEY")
                && message.contains("is set but empty"),
            "the error names the variable: {message}"
        );
    }

    /// The trim half of `resolve_discover_api_key`'s contract (the one
    /// `support::resolve_secret` documents): environment secrets routinely
    /// carry a trailing newline (`read KEY < key.txt`, a CI secret store, a
    /// `.env` loader), and the resolver must return the credential, not the
    /// raw variable — a "\n" pasted into the Authorization header is a
    /// silent 401 generator. A variable that is blank after trimming still
    /// hits the set-but-empty refusal, never an anonymous round.
    #[test]
    fn discover_env_key_is_trimmed_and_blank_after_trim_is_refused() {
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _key = TempEnv::set("PINVOU_TEST_DISCOVER_TRIM_KEY", "sk-x\n");
        assert_eq!(
            resolve_discover_api_key("PINVOU_TEST_DISCOVER_TRIM_KEY").expect("a usable value"),
            "sk-x",
            "the resolver returns the credential without the trailing newline"
        );

        // End to end: the value that reaches the wire carries no newline.
        let body = r#"{"data":[{"id":"m"}]}"#;
        let (base, log) = spawn_discover_mock(&discover_routes(&[(
            "/models",
            vec![(200u16, body), (200, body)],
        )]));
        let outcome = discover(
            &base,
            Some("PINVOU_TEST_DISCOVER_TRIM_KEY"),
            None,
            Some(DiscoveryProtocol::OpenAiCompatible),
            true,
            OutputMode::Json,
        )
        .expect("a completed discovery is a result, not a command failure");
        assert_eq!(outcome.exit_code, ExitCode::Success);
        let bearers = log.header_values("/models", "authorization");
        assert!(
            bearers.len() == 2 && bearers.iter().all(|value| value == "Bearer sk-x"),
            "every request presents the trimmed credential: {bearers:?}"
        );

        // Blank after trim: the set-but-empty refusal, not a keyless round.
        let _blank = TempEnv::set("PINVOU_TEST_DISCOVER_TRIM_KEY", " \n\t");
        let error = discover(
            "http://127.0.0.1:1",
            Some("PINVOU_TEST_DISCOVER_TRIM_KEY"),
            None,
            Some(DiscoveryProtocol::OpenAiCompatible),
            true,
            OutputMode::Human,
        )
        .expect_err("a blank-after-trim variable fails the command");
        assert_eq!(error.exit_code(), ExitCode::Failed);
        let message = error.to_string();
        assert!(
            message.contains("PINVOU_TEST_DISCOVER_TRIM_KEY")
                && message.contains("is set but empty"),
            "the refusal names the variable, never the value: {message}"
        );
    }

    /// The rendering contract: the table for a listed run (exit 0), the
    /// status row + hint for failures (exit 1), and the `--json` payload
    /// byte-identical in field names, snake_case vocabulary and null shape
    /// to the GUI's `EndpointModelsListing` serialization.
    #[test]
    fn discover_rendering_and_exit_codes_are_pinned() {
        let listed = EndpointModelsListing {
            status: ListingStatus::Listed,
            http_status: None,
            transport: None,
            server_kind: Some("ollama"),
            models: vec![
                DiscoveredModel {
                    id: "big-model".to_owned(),
                    context_window_tokens: Some(131072),
                    context_from: Some(ModelFactSource::Endpoint),
                    max_output_tokens: Some(8192),
                    output_from: Some(ModelFactSource::Endpoint),
                    loaded: Some(true),
                    title: None,
                },
                DiscoveredModel {
                    id: "cold|model".to_owned(),
                    context_window_tokens: None,
                    context_from: None,
                    max_output_tokens: None,
                    output_from: None,
                    loaded: Some(false),
                    title: None,
                },
            ],
            truncated: true,
        };
        let outcome = render_discovery_outcome(&listed, OutputMode::Human);
        assert_eq!(outcome.exit_code, ExitCode::Success);
        let human = outcome.stdout;
        assert!(human.starts_with("ID | CONTEXT | OUTPUT | SOURCE"));
        assert!(human.contains("big-model | 131.1K | 8.2K | endpoint"));
        // A hostile id cannot forge extra table columns in human output.
        assert!(human.contains("cold/model | - | - | endpoint"));
        // Cause-neutral: `truncated` fires for the OpenAI-compatible entry
        // cap, the native lanes' page/entry caps and the Ollama show budget
        // alike, so the note names no single cause.
        assert!(human.contains("note: the listing is truncated"));
        assert!(
            !human.contains("entry cap reached"),
            "the note must not name one cause among several: {human}"
        );
        let outcome = render_discovery_outcome(&listed, OutputMode::Json);
        let value: serde_json::Value =
            serde_json::from_str(&outcome.stdout).expect("single-line json");
        assert_eq!(value["status"], "listed");
        assert_eq!(value["server_kind"], "ollama");
        assert_eq!(value["http_status"], serde_json::Value::Null);
        assert_eq!(value["models"][0]["context_from"], "endpoint");
        assert_eq!(value["models"][0]["loaded"], true);
        assert_eq!(
            value["models"][1]["context_window_tokens"],
            serde_json::Value::Null
        );
        assert_eq!(value["truncated"], true);

        // An empty listed run says so, and stays exit 0.
        let empty = EndpointModelsListing {
            models: Vec::new(),
            truncated: false,
            server_kind: None,
            ..listed
        };
        let outcome = render_discovery_outcome(&empty, OutputMode::Human);
        assert_eq!(outcome.exit_code, ExitCode::Success);
        assert!(outcome.stdout.contains("the endpoint returned no models"));

        // Failures keep the structured row but exit 1.
        let auth = auth_listing(401);
        let outcome = render_discovery_outcome(&auth, OutputMode::Human);
        assert_eq!(outcome.exit_code, ExitCode::Failed);
        assert!(outcome.stdout.contains("status: auth_required"));
        assert!(outcome.stdout.contains("http_status: 401"));
        assert!(outcome.stdout.contains("--api-key-env V"));
        let transport_error = error_listing(None, Some("timeout"));
        let outcome = render_discovery_outcome(&transport_error, OutputMode::Human);
        assert_eq!(outcome.exit_code, ExitCode::Failed);
        assert!(outcome.stdout.contains("status: error"));
        assert!(outcome.stdout.contains("transport: timeout"));
        // The failure row's JSON shape matches the GUI's serialization.
        let value: serde_json::Value =
            serde_json::from_str(&render_discovery_outcome(&auth, OutputMode::Json).stdout)
                .expect("single-line json");
        assert_eq!(value["status"], "auth_required");
        assert_eq!(value["models"], serde_json::json!([]));
        assert_eq!(value["truncated"], false);
    }

    /// The `--json` flag overrides the global human mode for one invocation.
    #[test]
    fn discover_json_flag_forces_the_json_payload() {
        let outcome = discover(
            "http://user:pass@127.0.0.1:1",
            None,
            None,
            None,
            true,
            OutputMode::Human,
        )
        .expect("a completed discovery is a result, not a command failure");
        let value: serde_json::Value =
            serde_json::from_str(&outcome.stdout).expect("the --json payload parses");
        assert_eq!(value["status"], "error");
    }

    // -------------------------------------------------------------------
    // The injectable credential store. The app passes `&dyn
    // CredentialStore` into `save_model_inner`/`delete_model_inner` so the
    // lane is testable against something other than the OS keyring
    // (`RecordingCredentialStore` in the app's credential_store tests is the
    // same idea); the private generic command fns (`list`, `add`, `show`,
    // ...) are this module's spelling of that seam. These tests exercise
    // them with a recording store so the whole
    // credential lane (store, roll back, reveal, clear, delete) finally has
    // behavioural coverage — previously every call site inlined
    // `SystemCredentialStore`, so nothing could observe any of it without
    // touching the real keyring, which no test in this crate does.
    // -------------------------------------------------------------------

    /// A `CredentialStore` that answers `get` from an in-memory map, accepts
    /// every write, and records each operation in a log a test can assert on.
    /// The `fail_sets` / `keyring_unreachable` constructors below shape it
    /// into the two failure doubles the credential-lane tests need.
    struct RecordingStore {
        state: std::sync::Mutex<std::collections::HashMap<(String, String), String>>,
        log: std::sync::Mutex<Vec<String>>,
        fail_sets: bool,
        keyring_unreachable: bool,
    }

    impl RecordingStore {
        fn new() -> Self {
            Self {
                state: std::sync::Mutex::new(std::collections::HashMap::new()),
                log: std::sync::Mutex::new(Vec::new()),
                fail_sets: false,
                keyring_unreachable: false,
            }
        }

        /// A `RecordingStore` whose `set` fails from the first `set` on. Used to
        /// pin the rollback arms for failures that happen AFTER the old secret
        /// was destroyed by an overwrite — the only reachable "saved failed"
        /// ordering in `add`/`edit`/`search_set`, since their in-closure
        /// validations all run before any store write.
        fn failing_sets() -> Self {
            Self {
                fail_sets: true,
                ..RecordingStore::new()
            }
        }

        /// A store in the state an unreachable OS keyring produces: reads MISS
        /// (the file fallback answers "absent" — the secret itself sits in the
        /// keyring this process cannot reach) while
        /// [`CredentialStore::os_keyring_unreachable`] reports that ambiguity.
        /// `SystemCredentialStore` reaches exactly this state after its keyring
        /// probe fails; the double makes it hermetic for the honest-resolver
        /// pins below.
        fn unreachable_keyring() -> Self {
            Self {
                keyring_unreachable: true,
                ..RecordingStore::new()
            }
        }

        fn entries(&self) -> Vec<String> {
            let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state.values().cloned().collect()
        }

        fn operations(&self) -> Vec<String> {
            self.log.lock().unwrap_or_else(|p| p.into_inner()).clone()
        }
    }

    impl CredentialStore for RecordingStore {
        fn get(&self, reference: &CredentialReference) -> Result<Option<String>, CredentialError> {
            self.log
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(format!("get:{}/{}", reference.service, reference.account));
            Ok(self
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(&(reference.service.clone(), reference.account.clone()))
                .cloned())
        }

        fn set(&self, reference: &CredentialReference, value: &str) -> Result<(), CredentialError> {
            if self.fail_sets {
                return Err(CredentialError::new("keyring unavailable".to_owned()));
            }
            self.log
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(format!(
                    "set:{}/{}={}",
                    reference.service, reference.account, value
                ));
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state.insert(
                (reference.service.clone(), reference.account.clone()),
                value.to_owned(),
            );
            Ok(())
        }

        fn delete(&self, reference: &CredentialReference) -> Result<(), CredentialError> {
            self.log
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(format!(
                    "delete:{}/{}",
                    reference.service, reference.account
                ));
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state.remove(&(reference.service.clone(), reference.account.clone()));
            Ok(())
        }

        fn os_keyring_unreachable(&self, _reference: &CredentialReference) -> bool {
            self.keyring_unreachable
        }
    }

    /// Sets one environment variable for one test and restores it on drop.
    /// Assumes the caller already holds `ENV_LOCK` (each env-steering test
    /// takes it itself), so constructing one `TempHome` THEN a `TempEnv`
    /// (the common order) works with a single lock acquisition.
    struct TempEnv(&'static str, Option<std::ffi::OsString>);

    impl TempEnv {
        fn set(name: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(name);
            unsafe { std::env::set_var(name, value) };
            Self(name, previous)
        }
    }

    impl Drop for TempEnv {
        fn drop(&mut self) {
            match self.1.take() {
                Some(value) => unsafe { std::env::set_var(self.0, value) },
                None => unsafe { std::env::remove_var(self.0) },
            }
        }
    }

    /// Adds one model through the real `add` lane with these tests' common
    /// defaults (`ModelPreset::Deepseek`, no sizing fields or metadata,
    /// human output) and returns the new id printed on stdout. `model` stays
    /// a parameter because two fixtures pin the wire model their site
    /// actually passes.
    fn add_model(
        store: &RecordingStore,
        name: &str,
        model: &str,
        base_url: &str,
        key: Option<&str>,
    ) -> String {
        let outcome = add(
            store,
            AddModel {
                preset: ModelPreset::Deepseek,
                name: name.to_owned(),
                model: model.to_owned(),
                base_url: base_url.to_owned(),
                api_key_env: key.map(str::to_owned),
                api_key_stdin: false,
                context_window: None,
                max_output: None,
                reasoning_effort: None,
                metadata: ModelMetadata::default(),
                set_active: false,
            },
            OutputMode::Human,
        )
        .expect("add succeeds");
        outcome
            .stdout
            .strip_prefix("id: ")
            .expect("prints the new id")
            .trim()
            .to_owned()
    }

    /// Edits one model's credential through the real `edit` lane with these
    /// tests' common defaults (no field changes, no stdin, no activation,
    /// human output). `clear` doubles as the destructive `--yes`
    /// acknowledgment, exactly as every site pairs them.
    fn edit_credential(
        store: &RecordingStore,
        id: &str,
        api_key_env: Option<&str>,
        clear: bool,
    ) -> Result<CliOutcome, CliError> {
        edit(
            store,
            EditModel {
                id: id.to_owned(),
                changes: Box::new(ModelEdit::default()),
                api_key_env: api_key_env.map(str::to_owned),
                api_key_stdin: false,
                clear_api_key: clear,
                yes: clear,
                set_active: false,
            },
            OutputMode::Human,
        )
    }

    /// Sets (or clears) one search provider's key through the real
    /// `search_set` lane with these tests' common defaults (Metaso, human
    /// output). `clear` doubles as the destructive `--yes` acknowledgment,
    /// exactly as every site pairs them.
    fn search_set_key(
        store: &RecordingStore,
        api_key_env: Option<&str>,
        clear: bool,
    ) -> Result<CliOutcome, CliError> {
        search_set(
            store,
            SearchProvider::Metaso,
            &api_key_env.map(str::to_owned),
            clear,
            clear,
            OutputMode::Human,
        )
    }

    /// `add` stores the secret trimmed, `show --reveal-key` reads it back
    /// through the same seam, `edit --clear-api-key` deletes it after the
    /// commit, and the persisted model is marked missing, not configured.
    #[test]
    fn credential_lane_stores_reveals_and_clears_through_the_injected_store() {
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _home = TempHome::new("lane");
        let _key = TempEnv::set("MODELS_UNIT_TEST_KEY", "sk-lane\n");
        let store = RecordingStore::new();

        let id = add_model(
            &store,
            "Lane",
            "deepseek-v4-pro",
            "https://api.deepseek.com",
            Some("MODELS_UNIT_TEST_KEY"),
        );
        // Stored trimmed (the trailing newline must not survive
        // `secret_for_storage`), exactly one entry.
        assert_eq!(store.entries(), vec!["sk-lane".to_owned()]);
        assert_eq!(
            UserPrefs::load()
                .model_by_id(&id)
                .expect("model persisted")
                .credential_state,
            CredentialState::Configured,
            "a stored secret must mark the model configured"
        );

        // show --reveal-key reads back through the same injected store.
        let outcome = show(&store, &id, true, OutputMode::Human).expect("show succeeds");
        assert!(
            outcome.stdout.contains("api_key: sk-lane"),
            "reveal must read through the injected store: {}",
            outcome.stdout
        );
        assert!(
            outcome.stdout.contains("api_key_source: credential_store"),
            "and name the store as its source: {}",
            outcome.stdout
        );

        // clear: the keyring delete runs after the commit, through the seam.
        let outcome = edit_credential(&store, &id, None, true).expect("edit succeeds");
        assert_eq!(outcome.exit_code, ExitCode::Success);
        assert!(
            store
                .operations()
                .iter()
                .any(|op| op.starts_with("delete:")),
            "the clear path must delete through the injected store: {:?}",
            store.operations()
        );
        assert!(store.entries().is_empty(), "the key must be gone");
        assert_eq!(
            UserPrefs::load()
                .model_by_id(&id)
                .expect("model kept")
                .credential_state,
            CredentialState::Missing,
            "the cleared model must be marked missing, not configured"
        );
    }

    /// Effective-mode contract (review P2): the desktop stores an explicit
    /// Auto lane default as the `*_auto` companion flag with a None legacy
    /// value. The CLI getter must report the effective value (not the stale
    /// legacy spelling), and EVERY legacy write - including the `none` clear
    /// - must clear the companion, so a retained stale flag can no longer
    /// mask the CLI's choice when the desktop reloads and resolves the pair.
    #[test]
    fn mode_default_reads_and_writes_follow_the_effective_contract() {
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _home = TempHome::new("mode-contract");

        // Desktop-selected Auto on both lanes: the exact settings.json shape
        // the desktop's `set_mode_default` pair contract produces.
        UserPrefs::update_transaction(|prefs| {
            prefs.code_permission.last_mode = None;
            prefs.code_permission.last_mode_auto = true;
            prefs.mode_defaults.work = None;
            prefs.mode_defaults.work_auto = true;
            Ok(())
        })
        .expect("seed desktop auto flags");

        // Getter reports the effective value, not the stale legacy None.
        let store = RecordingStore::new();
        for key in [
            SettingsKey::CodePermissionLastMode,
            SettingsKey::ModeDefaultsWork,
        ] {
            let outcome =
                settings_get(&store, Some(key), OutputMode::Human).expect("auto get succeeds");
            assert!(
                outcome.stdout.contains("auto"),
                "{key:?} must report the companion Auto, got: {}",
                outcome.stdout
            );
        }

        // Desktop Auto -> CLI choice -> reload: a legacy write stores the
        // value AND clears the companion, so the reloaded effective mode is
        // the CLI's choice, not the seeded Auto.
        settings_set(
            SettingsKey::CodePermissionLastMode,
            SettingsValue::Mode(Some(SerializableMode::Yolo)),
            OutputMode::Human,
        )
        .expect("yolo set succeeds");
        let prefs = UserPrefs::load();
        assert_eq!(
            prefs.code_permission.last_mode,
            Some(SerializableMode::Yolo),
            "the legacy value must carry the CLI choice"
        );
        assert!(
            !prefs.code_permission.last_mode_auto,
            "the companion flag must clear on a legacy choice - a retained flag would mask \
             this write on the desktop"
        );
        let outcome = settings_get(
            &store,
            Some(SettingsKey::CodePermissionLastMode),
            OutputMode::Human,
        )
        .expect("post-set get succeeds");
        assert!(
            outcome.stdout.contains("yolo") && !outcome.stdout.contains("auto"),
            "the getter must report the CLI's choice after the write: {}",
            outcome.stdout
        );

        // The `none` clear follows the same contract.
        settings_set(
            SettingsKey::ModeDefaultsWork,
            SettingsValue::Mode(None),
            OutputMode::Human,
        )
        .expect("none set succeeds");
        let prefs = UserPrefs::load();
        assert_eq!(prefs.mode_defaults.work, None);
        assert!(
            !prefs.mode_defaults.work_auto,
            "the work companion must clear on the none clear too"
        );

        // `auto` itself stores as the pair (None legacy + companion), never
        // the literal in the legacy field - the settings.json lane vocabulary
        // must stay readable by pre-tri-mode binaries.
        settings_set(
            SettingsKey::ModeDefaultsWork,
            SettingsValue::Mode(Some(SerializableMode::Auto)),
            OutputMode::Human,
        )
        .expect("auto set succeeds");
        let prefs = UserPrefs::load();
        assert_eq!(prefs.mode_defaults.work, None, "legacy field stays None");
        assert!(prefs.mode_defaults.work_auto, "the companion carries Auto");
        let outcome = settings_get(
            &store,
            Some(SettingsKey::ModeDefaultsWork),
            OutputMode::Human,
        )
        .expect("auto re-get succeeds");
        assert!(
            outcome.stdout.contains("auto"),
            "the getter must report auto after an auto write: {}",
            outcome.stdout
        );
    }

    /// Round-20 review finding (M1): the `--clear-api-key` deletion lane used
    /// to synthesize a `model:{id}` reference when the model had no stored
    /// `credential_ref` (`or_else(|| Some(updated.credential_reference()))`),
    /// then called the keyring's `delete` on it after the commit. A
    /// never-configured model has no keyring entry, and deleting a
    /// never-written reference errors on most keyrings — a spurious warning
    /// for a no-op — and through `execute` that delete reached the REAL OS
    /// keyring. The lane now gates exactly like `models remove` and
    /// `search set --clear` (stored `credential_ref` only, no fallback).
    /// Red-style: against the pre-fix code this test fails, because the
    /// injected store records a `delete:` for a model that never stored
    /// anything under any reference.
    #[test]
    fn edit_clear_without_a_stored_reference_never_deletes_from_the_keyring() {
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _home = TempHome::new("edit-clear-no-ref");
        let store = RecordingStore::new();

        // A model added without a key: nothing is ever written, so
        // `credential_ref` stays None (`apply_new_model_credential` runs
        // `mark_missing` without touching the store).
        let id = add_model(
            &store,
            "NoKey",
            "deepseek-v4-pro",
            "https://api.deepseek.com",
            None,
        );
        assert!(
            UserPrefs::load()
                .model_by_id(&id)
                .expect("model persisted")
                .credential_ref
                .is_none(),
            "the fixture must be a model with no stored credential reference"
        );
        assert!(
            store.operations().is_empty(),
            "a keyless add must not touch the store: {:?}",
            store.operations()
        );

        // clear with --yes through the same seam: the command succeeds, the
        // prefs record flips to missing, and the store sees NO operation —
        // in particular no `delete:` against a synthesized reference.
        let outcome = edit_credential(&store, &id, None, true).expect("edit succeeds");
        assert_eq!(outcome.exit_code, ExitCode::Success);
        // Round-42 review: no stored reference → nothing was cleared, so the
        // receipt says `unchanged` (the search lane's doctrine) instead of a
        // success-shaped no-op `cleared`.
        assert!(
            outcome.stdout.contains("credential: unchanged"),
            "a keyless model's clear must report unchanged, not cleared: {}",
            outcome.stdout
        );
        assert!(
            store.operations().is_empty(),
            "a model without a stored credential_ref must never issue a keyring delete: {:?}",
            store.operations()
        );
        assert_eq!(
            UserPrefs::load()
                .model_by_id(&id)
                .expect("model kept")
                .credential_state,
            CredentialState::Missing,
            "the cleared model must be marked missing, not configured"
        );
    }

    /// The rollback `add` documents: a save that fails AFTER the secret was
    /// stored must delete the just-stored secret. The in-closure validations
    /// (`require_known_vision_model`, the store write itself) all run BEFORE
    /// `apply_new_model_credential` stores, so the only reachable
    /// "stored, then failed" ordering is a COMMIT failure in
    /// `UserPrefs::update_transaction`'s `save_unlocked`. Forced
    /// deterministically here: a directory named `settings.json` makes the
    /// atomic write's rename fail (EISDIR/enotempty) while the closure —
    /// which loads from a missing file into defaults — still succeeds and
    /// stores the secret.
    #[test]
    fn add_rollback_deletes_the_orphaned_secret() {
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let home = TempHome::new("add-rollback");
        let _key = TempEnv::set("MODELS_UNIT_TEST_KEY", "sk-rollback");
        let store = RecordingStore::new();
        // settings.json as a DIRECTORY: reads fall back to defaults (the path
        // holds no file), but the commit's tmp->rename cannot replace a
        // non-empty directory target, so `save_unlocked` fails after the
        // closure already stored the secret.
        std::fs::create_dir_all(home.join("settings.json")).expect("place the commit blocker");

        let error = add(
            &store,
            AddModel {
                preset: ModelPreset::Deepseek,
                name: "Roll".to_owned(),
                model: "M".to_owned(),
                base_url: "https://api.deepseek.com".to_owned(),
                api_key_env: Some("MODELS_UNIT_TEST_KEY".to_owned()),
                api_key_stdin: false,
                context_window: None,
                max_output: None,
                reasoning_effort: None,
                metadata: ModelMetadata::default(),
                set_active: false,
            },
            OutputMode::Human,
        )
        .expect_err("must fail: the settings commit fails");
        assert_eq!(error.exit_code(), ExitCode::Failed);
        let operations = store.operations();
        let set_at = operations
            .iter()
            .position(|op| op.starts_with("set:"))
            .expect("the key was stored before the failure");
        assert!(
            operations[set_at..]
                .iter()
                .any(|op| op.starts_with("delete:")),
            "a failed save must roll the stored secret back: {operations:?}"
        );
        assert!(
            store.entries().is_empty(),
            "no orphaned credential may outlive the failed add"
        );
    }

    /// A `store.set` that fails mid-rotation must fail the command (exit 1)
    /// without rolling prefs back to a state that no longer matches the
    /// keyring, and without the rollback being able to write either — the
    /// failure stays honest instead of half-applied.
    #[test]
    fn edit_rotation_store_failure_fails_the_save_cleanly() {
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _home = TempHome::new("edit-store-failure");
        let _old = TempEnv::set("MODELS_UNIT_TEST_KEY", "sk-old");
        let store = RecordingStore::new();
        let id = add_model(
            &store,
            "Fragile",
            "M",
            "https://api.deepseek.com",
            Some("MODELS_UNIT_TEST_KEY"),
        );
        let _new = TempEnv::set("MODELS_UNIT_TEST_KEY", "sk-new");

        // The rotation's store write fails: the failure surfaces as a save
        // error, the prefs record is untouched, and the old secret survives.
        let failing = RecordingStore::failing_sets();
        let error = edit_credential(&failing, &id, Some("MODELS_UNIT_TEST_KEY"), false)
            .expect_err("must fail: the keyring write fails");
        assert_eq!(error.exit_code(), ExitCode::Failed);
        assert!(
            error.to_string().contains("credential store unavailable"),
            "the rotation failure is reported as a keyring failure: {error}"
        );
        // The old secret is intact and the model still points at it.
        assert_eq!(store.entries(), vec!["sk-old".to_owned()]);
        assert_eq!(
            UserPrefs::load()
                .model_by_id(&id)
                .expect("model kept")
                .credential_state,
            CredentialState::Configured
        );
    }

    /// The rotate-on-edit rollback: `--api-key-env` writes over the reference
    /// that held the OLD secret, so a failed save must RESTORE the previous
    /// value — deleting would destroy a secret prefs still references. The
    /// rotation happens inside the closure and succeeds; the failure that
    /// leaves the overwrite stranded is the COMMIT (`save_unlocked`), forced
    /// here by making the product data dir non-writable: the closure still
    /// reads settings.json fine, while the commit's `create_new` tmp write
    /// fails. Unix-only (chmod), the same pattern as the code family's
    /// undiffable-file test; the portable variant of commit failure is pinned
    /// by `add_rollback_deletes_the_orphaned_secret` (EISDIR needs no
    /// permissions).
    #[cfg(unix)]
    #[test]
    fn edit_rotation_rollback_restores_the_previous_secret() {
        use std::os::unix::fs::PermissionsExt as _;

        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _home = TempHome::new("edit-rollback");
        let _old = TempEnv::set("MODELS_UNIT_TEST_KEY", "sk-old");
        let store = RecordingStore::new();

        let id = add_model(
            &store,
            "Rot",
            "M",
            "https://api.deepseek.com",
            Some("MODELS_UNIT_TEST_KEY"),
        );
        let _new = TempEnv::set("MODELS_UNIT_TEST_KEY", "sk-new");
        // The commit must fail while the load still succeeds: read-only on
        // the product data dir blocks the atomic write's staging (`create_new`
        // tmp file) without touching settings.json's readability.
        std::fs::set_permissions(&*_home, std::fs::Permissions::from_mode(0o555))
            .expect("make the data dir read-only");

        // The rotation wrote the new key over the old one; the commit then
        // fails, so the rollback must put the OLD value back.
        let result = edit_credential(&store, &id, Some("MODELS_UNIT_TEST_KEY"), false);
        // Restore write permission first so TempHome's cleanup can delete.
        std::fs::set_permissions(&*_home, std::fs::Permissions::from_mode(0o755))
            .expect("restore the data dir");
        let error = result.expect_err("must fail: the settings commit fails");
        assert_eq!(error.exit_code(), ExitCode::Failed);
        assert_eq!(
            store.entries(),
            vec!["sk-old".to_owned()],
            "rollback must RESTORE the previous secret, not delete it"
        );
    }

    /// `remove --yes` deletes the model's key through the same seam after the
    /// prefs save — the ordering the module documents and the app's
    /// `delete_model_inner` implements identically.
    #[test]
    fn remove_deletes_the_key_through_the_injected_store() {
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _home = TempHome::new("remove-key");
        let _key = TempEnv::set("MODELS_UNIT_TEST_KEY", "sk-remove");
        let store = RecordingStore::new();

        let id = add_model(
            &store,
            "Bye",
            "M",
            "https://api.deepseek.com",
            Some("MODELS_UNIT_TEST_KEY"),
        );

        let outcome = remove(&store, &id, true, OutputMode::Human).expect("remove succeeds");
        assert_eq!(outcome.exit_code, ExitCode::Success);
        assert!(
            store
                .operations()
                .iter()
                .any(|op| op.starts_with("delete:")),
            "remove must delete the key through the injected store"
        );
        assert!(store.entries().is_empty(), "the key must be gone");
        assert!(
            UserPrefs::load().model_by_id(&id).is_none(),
            "the model must be removed from prefs before the keyring delete"
        );
    }

    /// Pins the ordering `remove` documents: the settings commit FIRST, the
    /// keyring delete only AFTER it.
    /// `remove_deletes_the_key_through_the_injected_store` cannot tell a
    /// delete-after-commit from one hoisted above the transaction (the happy
    /// path orders are observationally identical), so this forces the commit
    /// to fail AFTER the closure found the model and captured its reference:
    /// a read-only product data dir blocks the atomic write's staging tmp
    /// file while every read (the up-front min-1 check, the transaction's
    /// load) still succeeds. A hoisted delete would log a `delete:` before
    /// that failure; the deferral must not.
    ///
    /// Unix-only (chmod), the same pattern as
    /// `edit_rotation_rollback_restores_the_previous_secret`: the portable
    /// settings.json-as-a-directory blocker cannot reach the commit here —
    /// with a directory, `load` falls back to defaults and the model lookup
    /// fails before any save happens.
    #[cfg(unix)]
    #[test]
    fn remove_defers_the_keyring_delete_until_after_the_commit() {
        use std::os::unix::fs::PermissionsExt as _;

        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _home = TempHome::new("remove-order");
        let _first = TempEnv::set("MODELS_UNIT_TEST_KEY", "sk-keep");
        let store = RecordingStore::new();

        // Two models: `remove`'s min-1 rule would reject a lone one before
        // the transaction could ever reach its save.
        let id = add_model(
            &store,
            "Keep",
            "M",
            "https://api.deepseek.com",
            Some("MODELS_UNIT_TEST_KEY"),
        );
        let _second = TempEnv::set("MODELS_UNIT_TEST_KEY", "sk-bye");
        add_model(
            &store,
            "Bye",
            "M",
            "https://api.deepseek.com",
            Some("MODELS_UNIT_TEST_KEY"),
        );

        // The commit must fail while the loads still succeed: read-only on
        // the product data dir blocks the atomic write's staging (`create_new`
        // tmp file) without touching settings.json's readability.
        std::fs::set_permissions(&*_home, std::fs::Permissions::from_mode(0o555))
            .expect("make the data dir read-only");
        let result = remove(&store, &id, true, OutputMode::Human);
        // Restore write permission first so TempHome's cleanup can delete.
        std::fs::set_permissions(&*_home, std::fs::Permissions::from_mode(0o755))
            .expect("restore the data dir");
        let error = result.expect_err("must fail: the settings commit fails");
        assert_eq!(error.exit_code(), ExitCode::Failed);
        let operations = store.operations();
        assert!(
            !operations.iter().any(|op| op.starts_with("delete:")),
            "the keyring delete is deferred behind the commit; a failed save must not delete: {operations:?}"
        );
        assert!(
            store.entries().contains(&"sk-keep".to_owned()),
            "the secret must survive the failed remove: {:?}",
            store.entries()
        );
    }

    /// The same pin for `edit --clear-api-key`: the record is marked missing
    /// inside the transaction and the keyring delete waits until after the
    /// commit (models.rs documents both). A commit failure must leave the
    /// secret in place with no `delete:` logged — deleting first would strip
    /// a secret the still-configured record references.
    #[cfg(unix)]
    #[test]
    fn edit_clear_defers_the_keyring_delete_until_after_the_commit() {
        use std::os::unix::fs::PermissionsExt as _;

        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _home = TempHome::new("clear-order");
        let _key = TempEnv::set("MODELS_UNIT_TEST_KEY", "sk-clear");
        let store = RecordingStore::new();

        let id = add_model(
            &store,
            "Clear",
            "M",
            "https://api.deepseek.com",
            Some("MODELS_UNIT_TEST_KEY"),
        );

        // Read-only on the product data dir blocks the commit's staging tmp
        // file while the transaction's load still succeeds (the same pattern
        // as `remove_defers_the_keyring_delete_until_after_the_commit`).
        std::fs::set_permissions(&*_home, std::fs::Permissions::from_mode(0o555))
            .expect("make the data dir read-only");
        let result = edit_credential(&store, &id, None, true);
        // Restore write permission first so TempHome's cleanup can delete.
        std::fs::set_permissions(&*_home, std::fs::Permissions::from_mode(0o755))
            .expect("restore the data dir");
        let error = result.expect_err("must fail: the settings commit fails");
        assert_eq!(error.exit_code, ExitCode::Failed);
        let operations = store.operations();
        assert!(
            !operations.iter().any(|op| op.starts_with("delete:")),
            "the keyring delete is deferred behind the commit; a failed clear must not delete: {operations:?}"
        );
        assert_eq!(
            store.entries(),
            vec!["sk-clear".to_owned()],
            "the secret must survive the failed clear"
        );
    }

    /// `settings search set` stores through the seam with the same
    /// normalization as `models add` (the round-16 finding stored the raw env
    /// value, trailing newline and all), and `--clear` deletes afterwards.
    #[test]
    fn search_set_stores_and_clears_through_the_injected_store() {
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _home = TempHome::new("search-key");
        let _key = TempEnv::set("METASO_API_KEY", "sk-search\n");
        let store = RecordingStore::new();

        let outcome = search_set_key(&store, Some("METASO_API_KEY"), false).expect("set succeeds");
        assert!(
            outcome.stdout.contains("credential: configured"),
            "{}",
            outcome.stdout
        );
        assert_eq!(
            store.entries(),
            vec!["sk-search".to_owned()],
            "the stored key must be trimmed like every other credential write"
        );

        let outcome = search_set_key(&store, None, true).expect("clear succeeds");
        assert_eq!(outcome.exit_code, ExitCode::Success);
        assert!(
            store
                .operations()
                .iter()
                .any(|op| op.starts_with("delete:")),
            "clear must delete through the injected store"
        );
        assert!(store.entries().is_empty(), "the search key must be gone");
    }

    /// The same pin for `settings search set --clear` (round-38 review: the
    /// two model lanes had commit-failure defer tests; the search lane, same
    /// shape, had none): the keyring delete waits until after the prefs
    /// commit — a failed save must leave the secret in place with no
    /// `delete:` logged, since the still-present prefs entry references it.
    #[cfg(unix)]
    #[test]
    fn search_clear_defers_the_keyring_delete_until_after_the_commit() {
        use std::os::unix::fs::PermissionsExt as _;

        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _home = TempHome::new("search-clear-order");
        let _key = TempEnv::set("METASO_API_KEY", "sk-search-defer");
        let store = RecordingStore::new();

        let outcome = search_set_key(&store, Some("METASO_API_KEY"), false).expect("set succeeds");
        assert!(outcome.stdout.contains("credential: configured"));

        // Same failure mechanism as the remove lane: a read-only data dir
        // blocks the atomic write's staging while reads keep working.
        std::fs::set_permissions(&*_home, std::fs::Permissions::from_mode(0o555))
            .expect("make the data dir read-only");
        let result = search_set_key(&store, None, true);
        // Restore write permission first so TempHome's cleanup can delete.
        std::fs::set_permissions(&*_home, std::fs::Permissions::from_mode(0o755))
            .expect("restore the data dir");
        let error = result.expect_err("must fail: the settings commit fails");
        assert_eq!(error.exit_code(), ExitCode::Failed);
        let operations = store.operations();
        assert!(
            !operations.iter().any(|op| op.starts_with("delete:")),
            "the clear's keyring delete is deferred behind the commit; a failed save \
             must not delete: {operations:?}"
        );
        assert!(
            store.entries().contains(&"sk-search-defer".to_owned()),
            "the secret must survive the failed clear: {:?}",
            store.entries()
        );
    }

    /// Round-49 review: with the OS keyring unreachable, a `get` MISS is not
    /// "no key" — the honest resolver must fail the probe lanes as
    /// `credential_unavailable` instead of letting the miss masquerade as an
    /// absent credential. `models test` used to probe with an empty key (the
    /// endpoint's 401 then reported `auth_invalid`); the probe-local lanes
    /// used to degrade to an anonymous probe or claim "no stored api key".
    /// The store is forced into exactly the ambiguous state through the
    /// injectable seam (a stored `credential_ref` whose value reads back
    /// absent while `os_keyring_unreachable` is true); no network is
    /// touched, because the verdict short-circuits before any request.
    #[test]
    fn probe_lanes_report_credential_unavailable_when_the_keyring_is_unreachable() {
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _home = TempHome::new("probe-unreachable");
        let _key = TempEnv::set("MODELS_UNIT_TEST_KEY", "sk-unreachable");
        let store = RecordingStore::unreachable_keyring();

        // A loopback base_url so the probe-local lane's loopback gate passes
        // and the failure is attributable to the resolver alone.
        let id = add_model(
            &store,
            "Unreachable",
            "M",
            "http://127.0.0.1:8000/v1",
            Some("MODELS_UNIT_TEST_KEY"),
        );
        assert!(
            UserPrefs::load()
                .model_by_id(&id)
                .expect("model persisted")
                .credential_ref
                .is_some(),
            "the fixture must hold a stored credential reference"
        );
        // Empty THIS process's view of the reference: under an unreachable
        // keyring the file fallback answers the miss, which is exactly the
        // "absent vs unreadable" ambiguity the honest resolver exists for.
        store
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();

        // `models test`: a probe RESULT, not a crash — single-line stdout
        // row with `credential_unavailable`, exit 1, and no `auth_invalid`.
        let outcome =
            test_connection(&store, &id, OutputMode::Human).expect("a completed probe outcome");
        assert_eq!(outcome.exit_code, ExitCode::Failed);
        assert!(
            outcome.stdout.contains("credential_unavailable"),
            "the verdict must name the unreachable keyring: {}",
            outcome.stdout
        );
        assert!(
            !outcome.stdout.contains("auth_invalid"),
            "an unreachable keyring must not masquerade as a rejected key: {}",
            outcome.stdout
        );

        // probe-local's active-model arm fails the same way instead of
        // silently probing anonymously (which would misclassify the server).
        use_model(&id, OutputMode::Human).expect("use succeeds");
        let error = probe_local(&store, None, None, None, OutputMode::Human)
            .expect_err("must fail: the keyring holding the key is unreachable");
        assert_eq!(error.exit_code(), ExitCode::Failed);
        assert!(
            error.to_string().contains("credential_unavailable"),
            "the refusal must name the credential: {error}"
        );

        // probe-local's named-model arm (`--model ID`) rides the same honest
        // resolver: the failure says the credential cannot be verified, not
        // that no key was ever stored.
        let error = probe_local(
            &store,
            Some("http://127.0.0.1:8000/v1"),
            None,
            Some(&id),
            OutputMode::Human,
        )
        .expect_err("must fail: the keyring holding the key is unreachable");
        assert_eq!(error.exit_code(), ExitCode::Failed);
        assert!(
            error.to_string().contains("credential_unavailable"),
            "the refusal must name the credential: {error}"
        );

        // Round-50 review: the search-test lane rides the same honest-miss
        // rule — a stored-but-unreachable search key used to report the
        // false `no_api_key` verdict (the one lane the round-49 rollout
        // skipped); it must fail as `credential_unavailable` like every
        // other lane.
        let outcome = search_set_key(&store, Some("MODELS_UNIT_TEST_KEY"), false)
            .expect("search set succeeds");
        assert!(
            outcome.stdout.contains("credential: configured"),
            "{}",
            outcome.stdout
        );
        store
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        let outcome = search_test(&store, SearchProvider::Metaso, OutputMode::Human)
            .expect("a completed probe outcome");
        assert_eq!(outcome.exit_code, ExitCode::Failed);
        assert!(
            outcome.stdout.contains("credential_unavailable"),
            "the verdict must name the unreachable keyring: {}",
            outcome.stdout
        );
        assert!(
            !outcome.stdout.contains("no_api_key"),
            "an unreachable keyring must not masquerade as a missing key: {}",
            outcome.stdout
        );

        // Round-51 review: `show --reveal-key` rides the same honest
        // resolver — with the keyring unreachable it must fail naming the
        // credential instead of printing the false `api_key: (not stored)`
        // / `credential_state: missing` block.
        let error = show(&store, &id, true, OutputMode::Human)
            .expect_err("must fail: the keyring holding the key is unreachable");
        assert_eq!(error.exit_code(), ExitCode::Failed);
        assert!(
            error.to_string().contains("credential_unavailable"),
            "the refusal must name the credential: {error}"
        );
    }

    /// Keyless parity with the GUI probe ladder: a record that declares the
    /// endpoint keyless (`auth_mode = "none"`) must probe credential-free
    /// even when a residual stored credential exists (hand-edited
    /// settings.json can carry both) — `models test` must validate the
    /// posture the engine will reproduce, not attach the stale secret.
    #[test]
    fn models_test_probes_credential_free_under_keyless_declaration() {
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _home = TempHome::new("probe-keyless-declaration");
        let _key = TempEnv::set("MODELS_UNIT_TEST_KEY", "sk-residual-stored");
        let store = RecordingStore::new();

        // Port 9 (discard) refuses fast: the probe completes offline either
        // way, so the verdict difference is attributable to the guard alone.
        let id = add_model(
            &store,
            "Keyless",
            "M",
            "http://127.0.0.1:9/v1",
            Some("MODELS_UNIT_TEST_KEY"),
        );
        assert!(
            UserPrefs::load()
                .model_by_id(&id)
                .expect("model persisted")
                .credential_ref
                .is_some(),
            "the fixture must hold a stored credential reference"
        );
        // Declare the endpoint keyless behind the saved record's back
        // (hand-edited settings.json shape; the CLI has no toggle).
        UserPrefs::update_transaction(|prefs| {
            if let Some(record) = prefs
                .advanced
                .saved_models
                .iter_mut()
                .find(|model| model.id == id)
            {
                record.auth_mode = Some("none".to_string());
            }
            Ok(())
        })
        .expect("keyless declaration should save");

        let outcome =
            test_connection(&store, &id, OutputMode::Human).expect("a completed probe outcome");
        let operations = store.operations();
        assert!(
            !operations.iter().any(|op| op.starts_with("get:")),
            "the probe must not read the stored credential for a keyless-declared record: {operations:?}"
        );
        assert!(
            !outcome.stdout.contains("credential_unavailable"),
            "the declaration must bypass credential resolution entirely: {}",
            outcome.stdout
        );

        // Reverse leg: the same record without the declaration resolves and
        // uses the stored credential as before (no over-suppression).
        UserPrefs::update_transaction(|prefs| {
            if let Some(record) = prefs
                .advanced
                .saved_models
                .iter_mut()
                .find(|model| model.id == id)
            {
                record.auth_mode = None;
            }
            Ok(())
        })
        .expect("declaration clear should save");
        let _ = test_connection(&store, &id, OutputMode::Human).expect("a completed probe outcome");
        let operations = store.operations();
        assert!(
            operations.iter().any(|op| op.starts_with("get:")),
            "an undeclared record keeps the stored-credential resolution: {operations:?}"
        );
    }

    /// Round-5 review: `models probe-local` resolves the stored credential
    /// with NO keyless consult on both its lanes — a declared record with a
    /// residual credential (hand-edited settings.json) presented the secret
    /// to the endpoint, and the `--model` lane hard-failed "no stored api
    /// key" on a CLEAN declared record (the exact state a keyless user is
    /// in). Both lanes must now short-circuit to an anonymous probe before
    /// any store read, while undeclared records keep the honest-resolver
    /// behavior (including the missing-key error).
    #[test]
    fn models_probe_local_honors_keyless_declaration() {
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _home = TempHome::new("probe-local-keyless-declaration");
        let _key = TempEnv::set("MODELS_UNIT_TEST_KEY", "sk-residual-stored");
        let store = RecordingStore::new();

        // Port 9 (discard) refuses fast: both lanes complete offline either
        // way, so the verdict difference is attributable to the guard alone.
        let id = add_model(
            &store,
            "Keyless",
            "M",
            "http://127.0.0.1:9/v1",
            Some("MODELS_UNIT_TEST_KEY"),
        );
        UserPrefs::update_transaction(|prefs| {
            if let Some(record) = prefs
                .advanced
                .saved_models
                .iter_mut()
                .find(|model| model.id == id)
            {
                record.auth_mode = Some("none".to_string());
            }
            Ok(())
        })
        .expect("keyless declaration should save");

        // `--model` lane: credential-free, and NO missing-key hard-fail —
        // a declared record proceeds anonymously like the active lane.
        let before = store.operations().len();
        let outcome = probe_local(
            &store,
            Some("http://127.0.0.1:9/v1"),
            None,
            Some(&id),
            OutputMode::Json,
        )
        .expect("a declared record probes anonymously, not a missing-key error");
        let operations = &store.operations()[before..];
        assert!(
            !operations.iter().any(|op| op.starts_with("get:")),
            "the --model lane must not read the stored credential under a declaration: {operations:?}"
        );
        assert!(
            !outcome.stdout.contains("has no stored api key"),
            "the declaration must bypass the missing-key error: {}",
            outcome.stdout
        );

        // Active-model lane: credential-free resolution as well.
        use_model(&id, OutputMode::Human).expect("use succeeds");
        let before = store.operations().len();
        probe_local(&store, None, None, None, OutputMode::Json)
            .expect("the active lane probes anonymously under a declaration");
        let operations = &store.operations()[before..];
        assert!(
            !operations.iter().any(|op| op.starts_with("get:")),
            "the active lane must not read the stored credential under a declaration: {operations:?}"
        );

        // Round-7 review: the documented precedence — an explicit probe-time
        // key outranks even the declaration (the record posture governs
        // stored credentials, not deliberate probe overrides) — had no pin.
        // Wire-level against the mock: the bearer must reach the endpoint,
        // and the store must stay untouched (the override arm reads the env
        // var, not the keyring).
        let mock = spawn_probe_mock(&[("/v1/models", 404, "{}"), ("/models", 404, "{}")]);
        UserPrefs::update_transaction(|prefs| {
            if let Some(record) = prefs
                .advanced
                .saved_models
                .iter_mut()
                .find(|model| model.id == id)
            {
                record.base_url = mock.base_url.clone();
            }
            Ok(())
        })
        .expect("repointing the record at the mock should save");
        let before = store.operations().len();
        probe_local(
            &store,
            None,
            Some("MODELS_UNIT_TEST_KEY"),
            None,
            OutputMode::Json,
        )
        .expect("the explicit-key probe completes against the mock");
        let operations = &store.operations()[before..];
        assert!(
            !operations.iter().any(|op| op.starts_with("get:")),
            "the explicit key rides the override arm, not a store read: {operations:?}"
        );
        assert!(
            mock.auth
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .values()
                .any(|value| value == "Bearer sk-residual-stored"),
            "an explicit probe-time key outranks the keyless declaration on the wire"
        );

        // Reverse leg: clear the declaration and both lanes resolve (and
        // use) the stored credential again — no over-suppression.
        UserPrefs::update_transaction(|prefs| {
            if let Some(record) = prefs
                .advanced
                .saved_models
                .iter_mut()
                .find(|model| model.id == id)
            {
                record.auth_mode = None;
            }
            Ok(())
        })
        .expect("declaration clear should save");
        let before = store.operations().len();
        probe_local(&store, None, None, None, OutputMode::Json).expect("a completed probe outcome");
        let operations = &store.operations()[before..];
        assert!(
            operations.iter().any(|op| op.starts_with("get:")),
            "an undeclared record keeps the stored-credential resolution: {operations:?}"
        );
    }

    /// Round-5 review: `model_entry_json` claimed field parity with the
    /// GUI's flattened `ModelListItem`, but omitted `auth_mode` — a
    /// declared record's credential-free probe behavior had no visible
    /// cause in `models list`/`show`. The posture flag (not secret
    /// material) now rides the JSON rows and the `show` human block
    /// (rendered only when set, like the round-40 metadata fields).
    #[test]
    fn models_list_and_show_expose_the_keyless_declaration() {
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _home = TempHome::new("models-json-auth-mode");
        let store = RecordingStore::new();
        let id = add_model(&store, "Keyless", "M", "http://127.0.0.1:9/v1", None);
        UserPrefs::update_transaction(|prefs| {
            if let Some(record) = prefs
                .advanced
                .saved_models
                .iter_mut()
                .find(|model| model.id == id)
            {
                record.auth_mode = Some("none".to_string());
            }
            Ok(())
        })
        .expect("keyless declaration should save");

        let outcome = list(&store, OutputMode::Json).expect("list succeeds");
        let value: serde_json::Value = serde_json::from_str(&outcome.stdout).expect("json");
        let row = value["models"]
            .as_array()
            .expect("rows")
            .iter()
            .find(|row| row["id"] == serde_json::json!(id))
            .expect("the added row renders");
        assert_eq!(
            row["auth_mode"],
            serde_json::json!("none"),
            "the JSON row must carry the declaration (GUI DTO parity)"
        );

        let outcome = show(&store, &id, false, OutputMode::Json).expect("show succeeds");
        let value: serde_json::Value = serde_json::from_str(&outcome.stdout).expect("json");
        assert_eq!(value["auth_mode"], serde_json::json!("none"));

        // Human block: rendered when set, absent for undeclared rows.
        let outcome = show(&store, &id, false, OutputMode::Human).expect("show succeeds");
        assert!(
            outcome.stdout.contains("auth_mode: none"),
            "the human block must surface the declaration: {}",
            outcome.stdout
        );
        let other = add_model(&store, "Plain", "M", "http://127.0.0.1:9/v1", None);
        let outcome = show(&store, &other, false, OutputMode::Human).expect("show succeeds");
        assert!(
            !outcome.stdout.contains("auth_mode"),
            "an undeclared record renders no auth_mode line: {}",
            outcome.stdout
        );
    }

    /// Round-7 review: `edit` was the one CLI writer that could persist the
    /// declared+configured dual state the GUI's `apply_model_credential`
    /// intercept exists to prevent — an explicit key replacement onto a
    /// hand-declared record stored the secret while `auth_mode: "none"`
    /// stayed (inert under the read-side guards, but a self-contradictory
    /// record shape the GUI can never produce). An explicit replacement is
    /// the caller deliberately re-authenticating the endpoint, so the stale
    /// declaration yields (the same explicit-key-outranks-declaration
    /// precedence the probe lanes document); keep-existing edits must not
    /// silently mutate the posture.
    #[test]
    fn models_edit_yields_keyless_declaration_to_explicit_key() {
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _home = TempHome::new("edit-keyless-yield");
        let _key = TempEnv::set("MODELS_UNIT_TEST_KEY", "sk-explicit-replacement");
        let store = RecordingStore::new();

        let id = add_model(
            &store,
            "Keyless",
            "M",
            "http://127.0.0.1:9/v1",
            Some("MODELS_UNIT_TEST_KEY"),
        );
        // Hand-edited settings.json shape: declaration + residual credential.
        UserPrefs::update_transaction(|prefs| {
            if let Some(record) = prefs
                .advanced
                .saved_models
                .iter_mut()
                .find(|model| model.id == id)
            {
                record.auth_mode = Some("none".to_string());
            }
            Ok(())
        })
        .expect("keyless declaration should save");

        // Explicit replacement: the declaration clears, the credential lands.
        let outcome = edit_credential(&store, &id, Some("MODELS_UNIT_TEST_KEY"), false)
            .expect("edit with an explicit key succeeds");
        assert!(
            outcome.stdout.contains("credential: replaced"),
            "{}",
            outcome.stdout
        );
        let record = UserPrefs::load()
            .model_by_id(&id)
            .expect("model persisted")
            .clone();
        assert_eq!(
            record.auth_mode, None,
            "an explicit key replacement must clear the stale declaration"
        );
        assert!(
            record.credential_ref.is_some() && record.has_secret,
            "the replacement credential must be stored: {:?}",
            record.credential_state
        );

        // Keep-existing edit (no key flags): the posture is untouched — an
        // unrelated field edit must not silently rewrite the declaration or
        // the bookkeeping. (The residual dual state stays inert under the
        // read-side guards, exactly as the probe lanes neutralize it.)
        let outcome = edit_credential(&store, &id, None, false).expect("keep-existing edit");
        assert!(
            outcome.stdout.contains("credential: unchanged"),
            "{}",
            outcome.stdout
        );
        let record = UserPrefs::load()
            .model_by_id(&id)
            .expect("model persisted")
            .clone();
        assert_eq!(record.auth_mode, None);
        assert!(record.credential_ref.is_some());

        // `--clear-api-key` keeps the GUI Delete posture: the record goes
        // missing (and the declaration, if any, stays a coherent keyless
        // statement about the endpoint).
        let id2 = add_model(
            &store,
            "Keyless2",
            "M",
            "http://127.0.0.1:9/v1",
            Some("MODELS_UNIT_TEST_KEY"),
        );
        UserPrefs::update_transaction(|prefs| {
            if let Some(record) = prefs
                .advanced
                .saved_models
                .iter_mut()
                .find(|model| model.id == id2)
            {
                record.auth_mode = Some("none".to_string());
            }
            Ok(())
        })
        .expect("keyless declaration should save");
        let outcome = edit_credential(&store, &id2, None, true).expect("clear succeeds");
        assert!(
            outcome.stdout.contains("credential: cleared"),
            "{}",
            outcome.stdout
        );
        let record = UserPrefs::load()
            .model_by_id(&id2)
            .expect("model persisted")
            .clone();
        assert_eq!(
            record.credential_state,
            CredentialState::Missing,
            "the clear must mark the record missing (GUI Delete parity)"
        );
        assert_eq!(
            record.auth_mode,
            Some("none".to_string()),
            "clearing the key keeps the endpoint's keyless statement"
        );
    }
}

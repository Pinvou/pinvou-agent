pub mod assistant;
pub(crate) mod browser;
// `pub` (not `pub(crate)`) only because of the `count_user_turns_in_json`
// re-export below: the stacked CLI families PR (#507) counts user turns for
// `code checkpoints rewind` through that exact predicate, and a crate-private
// module would make the re-export unreachable. No in-tree caller exists yet —
// an ahead-of-consumer surface, disclosed in PR #602.
pub mod code_checkpoints;
// The other `pub` modules below (`codex_acp`, `dependencies`, `knowledge`,
// `monitor`, `projects`, `remote_knowledge`, `shared_knowledge_host`,
// `voice`) are opened for the stacked headless CLI (#507 family), which
// calls them across the `pinvou3_lib` path dependency; like
// `code_checkpoints`, some of that surface is ahead of its first in-tree
// consumer.
pub mod codex_acp;
pub(crate) mod computer_use;
pub(crate) mod connectors;
// Crate-boundary re-exports for the headless CLI's `connectors ensure-cli`
// lane (same shape as the `count_user_turns_in_json` re-export below: the
// module stays crate-private, two items cross): the install-log appender, so
// the CLI's npm attempts append to — never truncate — the shared log and
// mark the mirror retry; and the acceleration-prefix env name, so the CLI's
// download chain consumes the app's constant instead of a drifting copy.
pub use connectors::{GITHUB_ASSET_MIRROR_PREFIX_ENV, append_cli_install_log};
// `pub` for the headless CLI (`pinvou artifacts list`): the deliverable
// extension whitelist and the category mapping must be ONE table shared
// with the GUI surface — a copy there drifts silently (the CLI's forced
// mirror predated the widening).
pub mod deliverables;
pub mod dependencies;
pub mod feedback;
pub mod files;
pub mod knowledge;
pub mod marketplace;
pub mod memory;
pub mod monitor;
pub mod multiagent;
pub mod personas;
pub(crate) mod pet;
pub mod projects;
pub(crate) mod remote_control;
pub mod remote_knowledge;
pub(crate) mod retirement;
pub(crate) mod review;
pub(crate) mod runtime_bundle;
pub(crate) mod scheduled;
pub mod sessions;
pub mod shared_knowledge_host;
pub(crate) mod updater;
pub mod voice;
pub(crate) mod voice_shortcut;

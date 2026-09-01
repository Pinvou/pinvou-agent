use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use adapter_gaia::{
    GAIA_DATASET_REVISION, GAIA_LEVEL, GAIA_SPLIT, GaiaAdapter, GaiaDataset, GaiaSnapshotManager,
    GaiaSource, HfSnapshotDownloader,
};
use benchmark_core::{
    BenchmarkAdapter, OfficialScoreReport, RunStore, SafeFailureCategory, SafeFailureReason,
    TaskOutcome, TaskStatus, publish_markdown_report, publish_score_json,
};

#[cfg(any(test, feature = "product-backend"))]
use adapter_smoke::{
    SmokeAnalysisMaterial, SmokeRecord, SmokeToolEvent, analyze_rules, calculate_product_score,
    not_configured_judge, render_smoke_markdown, smoke_cases,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputMode {
    Human,
    Json,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitCode {
    Success,
    Failed,
    Usage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BenchmarkAvailability {
    Available,
    Unavailable,
    Planned,
}

impl BenchmarkAvailability {
    fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Unavailable => "unavailable",
            Self::Planned => "planned",
        }
    }

    fn is_available(self) -> bool {
        self == Self::Available
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BenchmarkCommandSpec {
    id: &'static str,
    availability: BenchmarkAvailability,
    score_kind: &'static str,
    description: &'static str,
    command_error: &'static str,
    dispatch: BenchmarkDispatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BenchmarkDispatch {
    Smoke,
    Gaia,
    NotAvailable,
}

impl BenchmarkCommandSpec {
    pub fn id(&self) -> &'static str {
        self.id
    }
    pub fn availability(&self) -> BenchmarkAvailability {
        self.availability
    }
    pub fn score_kind(&self) -> &'static str {
        self.score_kind
    }
    pub fn command_error(&self) -> &'static str {
        self.command_error
    }
}

pub fn benchmark_registry() -> &'static [BenchmarkCommandSpec] {
    static REGISTRY: [BenchmarkCommandSpec; 4] = [
        BenchmarkCommandSpec {
            id: "smoke",
            availability: if cfg!(feature = "product-backend") {
                BenchmarkAvailability::Available
            } else {
                BenchmarkAvailability::Unavailable
            },
            score_kind: "internal_health",
            description: "内部健康检查（不是官方 benchmark 分数）",
            command_error: "product_backend_not_enabled",
            dispatch: BenchmarkDispatch::Smoke,
        },
        BenchmarkCommandSpec {
            id: "gaia",
            availability: BenchmarkAvailability::Available,
            score_kind: "official_compatible_local",
            description: "GAIA validation Level 1（官方兼容本地评分）",
            command_error: "",
            dispatch: BenchmarkDispatch::Gaia,
        },
        BenchmarkCommandSpec {
            id: "bfcl",
            availability: BenchmarkAvailability::Planned,
            score_kind: "official",
            description: "BFCL adapter（planned/not_available）",
            command_error: "benchmark_not_available",
            dispatch: BenchmarkDispatch::NotAvailable,
        },
        BenchmarkCommandSpec {
            id: "workbuddy",
            availability: BenchmarkAvailability::Planned,
            score_kind: "official",
            description: "WorkBuddy adapter（planned/not_available）",
            command_error: "benchmark_not_available",
            dispatch: BenchmarkDispatch::NotAvailable,
        },
    ];
    &REGISTRY
}

impl ExitCode {
    pub fn as_i32(self) -> i32 {
        match self {
            Self::Success => 0,
            Self::Failed => 1,
            Self::Usage => 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BenchmarkCommand {
    List,
    RunSmoke,
    FetchGaia {
        token_env: Option<String>,
        source: Option<PathBuf>,
    },
    VerifyGaia {
        source: PathBuf,
    },
    RunGaia {
        split: String,
        level: u8,
    },
    ScoreGaia {
        run_id: String,
    },
    SubmissionGaia {
        run_id: String,
        output: PathBuf,
    },
    Status(String),
    Resume(String),
    Report(String),
    RunNotAvailable(String),
    NotAvailable(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliCommand {
    Benchmark(BenchmarkCommand),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedCli {
    command: CliCommand,
    output: OutputMode,
}

impl ParsedCli {
    pub fn command(&self) -> &CliCommand {
        &self.command
    }

    pub fn output(&self) -> OutputMode {
        self.output
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliError {
    message: String,
    exit_code: ExitCode,
}

impl CliError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            exit_code: ExitCode::Usage,
        }
    }

    fn failed(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            exit_code: ExitCode::Failed,
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        self.exit_code
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

pub fn parse_args<I, S>(arguments: I) -> Result<ParsedCli, CliError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut values = arguments
        .into_iter()
        .map(|value| value.as_ref().to_owned())
        .collect::<Vec<_>>();
    if !values.is_empty() {
        values.remove(0);
    }
    let mut output = OutputMode::Human;
    let mut index = 0;
    while index < values.len() {
        if values[index] == "--output" {
            let value = values
                .get(index + 1)
                .ok_or_else(|| CliError::usage("--output requires human or json"))?;
            match value.as_str() {
                "human" => {
                    output = OutputMode::Human;
                    values.drain(index..=index + 1);
                }
                "json" => {
                    output = OutputMode::Json;
                    values.drain(index..=index + 1);
                }
                _ => index += 1,
            }
        } else {
            index += 1;
        }
    }
    if values.first().map(String::as_str) != Some("benchmark") {
        return Err(CliError::usage("usage: pinvou benchmark <command>"));
    }
    let command = match values.get(1).map(String::as_str) {
        Some("list") if values.len() == 2 => BenchmarkCommand::List,
        Some("run") if values.len() >= 3 => {
            let benchmark_id = values[2].as_str();
            let spec = benchmark_registry()
                .iter()
                .find(|spec| spec.id() == benchmark_id)
                .ok_or_else(|| CliError::usage("unknown benchmark"))?;
            match spec.dispatch {
                BenchmarkDispatch::Smoke if values.len() == 3 => BenchmarkCommand::RunSmoke,
                BenchmarkDispatch::Smoke => {
                    return Err(CliError::usage("benchmark run smoke accepts no options"));
                }
                BenchmarkDispatch::Gaia => parse_gaia_run(&values)?,
                BenchmarkDispatch::NotAvailable => {
                    if values.len() != 3 {
                        return Err(CliError::usage("planned benchmark accepts no options"));
                    }
                    BenchmarkCommand::RunNotAvailable(spec.command_error().to_owned())
                }
            }
        }
        Some("run") => return Err(CliError::usage("benchmark run requires one benchmark id")),
        Some("status") => BenchmarkCommand::Status(required_value(&values, "status")?),
        Some("resume") => BenchmarkCommand::Resume(required_value(&values, "resume")?),
        Some("report") => BenchmarkCommand::Report(required_value(&values, "report")?),
        Some("fetch") => parse_gaia_fetch(&values)?,
        Some("verify") => parse_gaia_verify(&values)?,
        Some("score") => parse_gaia_score(&values)?,
        Some("submission") => parse_gaia_submission(&values)?,
        _ => return Err(CliError::usage("unknown benchmark command")),
    };
    Ok(ParsedCli {
        command: CliCommand::Benchmark(command),
        output,
    })
}

fn parse_gaia_fetch(values: &[String]) -> Result<BenchmarkCommand, CliError> {
    require_gaia(values, "fetch")?;
    let options = named_options(&values[3..], &["--token-env", "--source"])?;
    let token_env = option(&options, "--token-env").map(str::to_owned);
    let source = option(&options, "--source").map(PathBuf::from);
    if token_env.is_some() == source.is_some() {
        return Err(CliError::usage(
            "benchmark fetch gaia requires exactly one of --token-env or --source",
        ));
    }
    Ok(BenchmarkCommand::FetchGaia { token_env, source })
}

fn parse_gaia_verify(values: &[String]) -> Result<BenchmarkCommand, CliError> {
    require_gaia(values, "verify")?;
    let options = named_options(&values[3..], &["--source"])?;
    let source = option(&options, "--source")
        .map(PathBuf::from)
        .ok_or_else(|| CliError::usage("benchmark verify gaia requires --source"))?;
    Ok(BenchmarkCommand::VerifyGaia { source })
}

fn parse_gaia_run(values: &[String]) -> Result<BenchmarkCommand, CliError> {
    let options = named_options(&values[3..], &["--split", "--level"])?;
    let split = option(&options, "--split")
        .ok_or_else(|| CliError::usage("benchmark run gaia requires --split validation"))?;
    let level = option(&options, "--level")
        .and_then(|value| value.parse::<u8>().ok())
        .ok_or_else(|| CliError::usage("benchmark run gaia requires --level 1"))?;
    if split != GAIA_SPLIT || level != GAIA_LEVEL {
        return Err(CliError::usage("only GAIA validation Level 1 is available"));
    }
    Ok(BenchmarkCommand::RunGaia {
        split: split.to_owned(),
        level,
    })
}

fn parse_gaia_score(values: &[String]) -> Result<BenchmarkCommand, CliError> {
    require_gaia(values, "score")?;
    let options = named_options(&values[3..], &["--run-id"])?;
    let run_id = option(&options, "--run-id")
        .map(str::to_owned)
        .ok_or_else(|| CliError::usage("benchmark score gaia requires --run-id"))?;
    Ok(BenchmarkCommand::ScoreGaia { run_id })
}

fn parse_gaia_submission(values: &[String]) -> Result<BenchmarkCommand, CliError> {
    require_gaia(values, "submission")?;
    let options = named_options(&values[3..], &["--run-id", "--destination", "--output"])?;
    let run_id = option(&options, "--run-id")
        .map(str::to_owned)
        .ok_or_else(|| CliError::usage("benchmark submission gaia requires --run-id"))?;
    let destination = option(&options, "--destination");
    let legacy_output = option(&options, "--output");
    if destination.is_some() && legacy_output.is_some() {
        return Err(CliError::usage(
            "benchmark submission gaia accepts one destination",
        ));
    }
    let output = destination
        .or(legacy_output)
        .map(PathBuf::from)
        .ok_or_else(|| CliError::usage("benchmark submission gaia requires --destination"))?;
    Ok(BenchmarkCommand::SubmissionGaia { run_id, output })
}

fn require_gaia(values: &[String], command: &str) -> Result<(), CliError> {
    if values.get(2).map(String::as_str) != Some("gaia") {
        return Err(CliError::usage(format!(
            "benchmark {command} requires benchmark id gaia"
        )));
    }
    Ok(())
}

fn named_options<'a>(
    values: &'a [String],
    allowed: &[&str],
) -> Result<Vec<(&'a str, &'a str)>, CliError> {
    if values.len() % 2 != 0 {
        return Err(CliError::usage("benchmark option requires a value"));
    }
    let mut parsed = Vec::new();
    for pair in values.chunks_exact(2) {
        let name = pair[0].as_str();
        let value = pair[1].as_str();
        if !allowed.contains(&name)
            || value.is_empty()
            || value.starts_with("--")
            || parsed.iter().any(|(existing, _)| *existing == name)
        {
            return Err(CliError::usage("unsupported or duplicate benchmark option"));
        }
        parsed.push((name, value));
    }
    Ok(parsed)
}

fn option<'a>(options: &'a [(&str, &'a str)], name: &str) -> Option<&'a str> {
    options
        .iter()
        .find_map(|(candidate, value)| (*candidate == name).then_some(*value))
}

fn required_value(values: &[String], command: &str) -> Result<String, CliError> {
    if values.len() != 3 {
        return Err(CliError::usage(format!(
            "benchmark {command} requires one run-id"
        )));
    }
    Ok(values[2].clone())
}

pub fn render_list(output: OutputMode) -> String {
    match output {
        OutputMode::Human => benchmark_registry()
            .iter()
            .map(|spec| {
                format!(
                    "{}\t{}\t{}",
                    spec.id,
                    spec.availability.as_str(),
                    spec.description
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        OutputMode::Json => format!(
            "{{\"benchmarks\":[{}]}}",
            benchmark_registry()
                .iter()
                .map(|spec| format!(
                    "{{\"id\":\"{}\",\"availability\":\"{}\",\"available\":{},\"score_kind\":\"{}\",\"command_error\":\"{}\"}}",
                    spec.id,
                    spec.availability.as_str(),
                    spec.availability.is_available(),
                    spec.score_kind,
                    spec.command_error,
                ))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliOutcome {
    pub exit_code: ExitCode,
    pub stdout: String,
}

pub fn execute(parsed: ParsedCli) -> Result<CliOutcome, CliError> {
    let output = parsed.output;
    match parsed.command {
        CliCommand::Benchmark(BenchmarkCommand::List) => Ok(success(render_list(output))),
        CliCommand::Benchmark(BenchmarkCommand::Status(run_id)) => status(&run_id, output),
        CliCommand::Benchmark(BenchmarkCommand::Report(run_id)) => report(&run_id, output),
        CliCommand::Benchmark(BenchmarkCommand::RunSmoke) => run_smoke(output),
        CliCommand::Benchmark(BenchmarkCommand::Resume(run_id)) => resume_smoke(&run_id, output),
        CliCommand::Benchmark(BenchmarkCommand::FetchGaia { token_env, source }) => {
            fetch_gaia(token_env, source, output)
        }
        CliCommand::Benchmark(BenchmarkCommand::VerifyGaia { source }) => {
            verify_gaia(&source, output)
        }
        CliCommand::Benchmark(BenchmarkCommand::RunGaia { split, level }) => {
            debug_assert_eq!(split, GAIA_SPLIT);
            debug_assert_eq!(level, GAIA_LEVEL);
            run_gaia(output)
        }
        CliCommand::Benchmark(BenchmarkCommand::ScoreGaia { run_id }) => {
            score_gaia(&run_id, output)
        }
        CliCommand::Benchmark(BenchmarkCommand::SubmissionGaia {
            run_id,
            output: path,
        }) => submission_gaia(&run_id, &path, output),
        CliCommand::Benchmark(BenchmarkCommand::RunNotAvailable(error)) => {
            Err(CliError::usage(error))
        }
        CliCommand::Benchmark(BenchmarkCommand::NotAvailable(command)) => Err(CliError::usage(
            format!("benchmark command '{command}' is not_available"),
        )),
    }
}

fn success(stdout: String) -> CliOutcome {
    CliOutcome {
        exit_code: ExitCode::Success,
        stdout,
    }
}

#[cfg(any(test, feature = "product-backend"))]
fn finalized_outcome(stdout: String, failed: usize) -> CliOutcome {
    CliOutcome {
        exit_code: if failed == 0 {
            ExitCode::Success
        } else {
            ExitCode::Failed
        },
        stdout,
    }
}

#[cfg(any(test, feature = "product-backend"))]
fn finalize_smoke_outcomes(
    base: &Path,
    run_id: &str,
    completed: usize,
    outcomes: &[TaskOutcome],
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let records = outcomes
        .iter()
        .cloned()
        .map(|outcome| {
            let tools = outcome
                .tool_observations()
                .iter()
                .map(|tool| SmokeToolEvent::new(tool.canonical_name.clone(), tool.failed))
                .collect();
            let usage = outcome.usage().map(|usage| {
                adapter_smoke::SmokeUsage::new(
                    usage.input_tokens,
                    usage.cache_hit_tokens,
                    usage.cache_miss_tokens,
                )
            });
            SmokeRecord::new(outcome, SmokeAnalysisMaterial::with_details(tools, usage))
        })
        .collect::<Vec<_>>();
    let analysis = analyze_rules(&smoke_cases(), &records);
    let score = calculate_product_score(&records, analysis.findings())
        .map_err(|_| CliError::failed("smoke_report_failed"))?;
    let markdown = render_smoke_markdown(&records, &analysis, &score, &not_configured_judge())
        .map_err(|_| CliError::failed("smoke_report_failed"))?;
    let store = RunStore::open(base, run_id).map_err(core_error)?;
    let report_path = if store.run_dir().join("report.md").exists() {
        let existing = std::fs::read_to_string(store.run_dir().join("report.md"))
            .map_err(|_| CliError::failed("smoke_report_failed"))?;
        if existing != markdown {
            return Err(CliError::failed("smoke_report_failed"));
        }
        store.run_dir().join("report.md")
    } else {
        publish_markdown_report(&store, &markdown)
            .map_err(core_error)?
            .path()
            .to_owned()
    };
    let report_path = report_path
        .canonicalize()
        .map_err(|_| CliError::failed("smoke_report_failed"))?;
    let failed = outcomes.len().saturating_sub(completed);
    let score_value = score
        .total()
        .map_or("null".to_owned(), |value| value.to_string());
    let text = match output {
        OutputMode::Human => format!(
            "Run: {run_id}\nCompleted: {completed}\nFailed: {failed}\nSmoke Health Score: {}\nReport: {}",
            score
                .total()
                .map_or_else(|| "unavailable".to_owned(), |value| format!("{value}/100")),
            report_path.display(),
        ),
        OutputMode::Json => format!(
            "{{\"run_id\":\"{}\",\"completed\":{completed},\"failed\":{failed},\"score\":{score_value},\"report_path\":\"{}\"}}",
            json_escape(run_id),
            json_escape(&report_path.display().to_string()),
        ),
    };
    Ok(finalized_outcome(text, failed))
}

fn status(run_id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store = RunStore::open(&benchmark_base()?, run_id).map_err(core_error)?;
    let recovered = store.recover().map_err(core_error)?;
    let terminal = store.read_outcomes().map_err(core_error)?;
    let failed = terminal
        .iter()
        .filter(|outcome| outcome.status() != benchmark_core::TaskStatus::Completed)
        .count();
    let diagnostics = GaiaDiagnostics::from_outcomes(&terminal);
    let text = match output {
        OutputMode::Human => format!(
            "Run: {run_id}\nCompleted: {}\nFailed: {failed}\nRemaining: {}\n{}",
            recovered.completed_task_ids().len(),
            recovered.runnable_task_ids().len(),
            diagnostics.render_human(),
        ),
        OutputMode::Json => serde_json::json!({
            "run_id": run_id,
            "completed": recovered.completed_task_ids().len(),
            "failed": failed,
            "remaining": recovered.runnable_task_ids().len(),
            "diagnostics": diagnostics.to_json(),
        })
        .to_string(),
    };
    Ok(success(text))
}

fn report(run_id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store = RunStore::open(&benchmark_base()?, run_id).map_err(core_error)?;
    let path = store.run_dir().join("report.md");
    let markdown =
        std::fs::read_to_string(&path).map_err(|_| CliError::failed("report_not_available"))?;
    let text = match output {
        OutputMode::Human => markdown,
        OutputMode::Json => format!(
            "{{\"run_id\":\"{}\",\"report_path\":\"{}\"}}",
            json_escape(run_id),
            json_escape(&path.display().to_string())
        ),
    };
    Ok(success(text))
}

fn benchmark_base() -> Result<PathBuf, CliError> {
    if let Some(value) = std::env::var_os("PINVOU3_HOME") {
        return absolute_path(PathBuf::from(value));
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or_else(|| CliError::failed("home_directory_not_available"))?;
    absolute_path(PathBuf::from(home).join(".pinvou3"))
}

fn absolute_path(path: PathBuf) -> Result<PathBuf, CliError> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(CliError::failed("benchmark base must be absolute"))
    }
}

fn core_error(error: benchmark_core::BenchmarkError) -> CliError {
    CliError::failed(error.code())
}

fn gaia_acquisition_root() -> Result<PathBuf, CliError> {
    let root = benchmark_base()?
        .join("benchmarks")
        .join("gaia")
        .join("datasets");
    std::fs::create_dir_all(&root).map_err(|_| CliError::failed("gaia_storage_unavailable"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| CliError::failed("gaia_storage_unavailable"))?;
    }
    Ok(root)
}

fn gaia_snapshot_root() -> Result<PathBuf, CliError> {
    Ok(gaia_acquisition_root()?.join(format!(
        "gaia-2023-validation-level1-{}",
        &GAIA_DATASET_REVISION[..12]
    )))
}

fn gaia_manager() -> Result<GaiaSnapshotManager<HfSnapshotDownloader>, CliError> {
    let current = std::env::current_dir()
        .and_then(|path| path.canonicalize())
        .map_err(|_| CliError::failed("gaia_worktree_unavailable"))?;
    let worktree = current
        .ancestors()
        .find(|ancestor| std::fs::symlink_metadata(ancestor.join(".git")).is_ok())
        .map(Path::to_path_buf);
    GaiaSnapshotManager::new_with_optional_worktree(
        gaia_acquisition_root()?,
        worktree.as_deref(),
        HfSnapshotDownloader,
    )
    .map_err(|error| CliError::failed(error.code()))
}

fn fetch_gaia(
    token_env: Option<String>,
    source: Option<PathBuf>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let source = match (token_env, source) {
        (Some(name), None) => GaiaSource::TokenEnvironment(name),
        (None, Some(path)) => GaiaSource::ExistingSnapshot(path),
        _ => return Err(CliError::usage("invalid GAIA source")),
    };
    let acquisition = gaia_manager()?
        .acquire(source)
        .map_err(|error| CliError::failed(error.code()))?;
    let text = match output {
        OutputMode::Human => format!("GAIA snapshot ready\nRevision: {}", acquisition.revision()),
        OutputMode::Json => format!(
            "{{\"status\":\"ready\",\"dataset_revision\":\"{}\"}}",
            acquisition.revision()
        ),
    };
    Ok(success(text))
}

fn open_official_gaia_dataset(snapshot_root: &Path) -> Result<GaiaDataset, CliError> {
    let acquisition = gaia_manager()?
        .verify_offline(snapshot_root)
        .map_err(|error| CliError::failed(error.code()))?;
    Ok(acquisition.into_dataset())
}

fn verify_gaia(source: &Path, output: OutputMode) -> Result<CliOutcome, CliError> {
    verify_gaia_with(source, output, |source| {
        gaia_manager()?
            .verify_source(source)
            .map(|_| ())
            .map_err(|error| CliError::failed(error.code()))
    })
}

fn verify_gaia_with(
    source: &Path,
    output: OutputMode,
    verify: impl FnOnce(&Path) -> Result<(), CliError>,
) -> Result<CliOutcome, CliError> {
    verify(source)?;
    let text = match output {
        OutputMode::Human => format!("GAIA dataset verified\nRevision: {GAIA_DATASET_REVISION}"),
        OutputMode::Json => {
            format!("{{\"status\":\"verified\",\"dataset_revision\":\"{GAIA_DATASET_REVISION}\"}}")
        }
    };
    Ok(success(text))
}

fn open_gaia_run(run_id: &str) -> Result<(GaiaAdapter, benchmark_core::CompletedRun), CliError> {
    let store = RunStore::open(&benchmark_base()?, run_id).map_err(core_error)?;
    let unbound = GaiaAdapter::new();
    let manifest = store
        .read_manifest()
        .map_err(|_| CliError::failed("gaia_run_manifest_mismatch"))?;
    if manifest.run_id() != run_id
        || !manifest.matches_contract(
            unbound.descriptor(),
            GAIA_SPLIT,
            "pinvou-gaia-public-web/v1",
            1,
        )
    {
        return Err(CliError::failed("gaia_run_manifest_mismatch"));
    }
    let dataset = Arc::new(open_official_gaia_dataset(&gaia_snapshot_root()?)?);
    let run = store.completed_run().map_err(core_error)?;
    Ok((GaiaAdapter::with_dataset(dataset), run))
}

fn score_gaia(run_id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let (adapter, run) = open_gaia_run(run_id)?;
    let report = adapter.score(&run).map_err(core_error)?;
    let store = RunStore::open(&benchmark_base()?, run_id).map_err(core_error)?;
    if report.is_complete() {
        publish_gaia_score_artifacts(&store, run_id, &report)?;
    }
    let comparable = report.comparable_accuracy();
    let text = match (output, comparable) {
        (OutputMode::Human, Some(accuracy)) => format!(
            "Run: {run_id}\nStatus: official_compatible_local\nSplit: {}\nLevel: {}\nEvaluated: {}\nCorrect: {}\nComparable accuracy: {:.6}",
            report.split(),
            report.level(),
            report.evaluated(),
            report.correct(),
            accuracy,
        ),
        (OutputMode::Human, None) => format!(
            "Run: {run_id}\nStatus: unofficial_partial\nSplit: {}\nLevel: {}\nEvaluated: {}\nCorrect: {}",
            report.split(),
            report.level(),
            report.evaluated(),
            report.correct(),
        ),
        (OutputMode::Json, Some(accuracy)) => format!(
            "{{\"run_id\":\"{}\",\"status\":\"official_compatible_local\",\"split\":\"{}\",\"level\":\"{}\",\"evaluated\":{},\"correct\":{},\"comparable_accuracy\":{accuracy:.6}}}",
            json_escape(run_id),
            json_escape(report.split()),
            json_escape(report.level()),
            report.evaluated(),
            report.correct(),
        ),
        (OutputMode::Json, None) => format!(
            "{{\"run_id\":\"{}\",\"status\":\"unofficial_partial\",\"split\":\"{}\",\"level\":\"{}\",\"evaluated\":{},\"correct\":{}}}",
            json_escape(run_id),
            json_escape(report.split()),
            json_escape(report.level()),
            report.evaluated(),
            report.correct(),
        ),
    };
    Ok(success(text))
}

fn publish_gaia_score_artifacts(
    store: &RunStore,
    run_id: &str,
    report: &OfficialScoreReport,
) -> Result<(), CliError> {
    if !report.is_complete() {
        return Err(CliError::failed("gaia_score_incomplete"));
    }
    let (status, comparable_accuracy) = match report.comparable_accuracy() {
        Some(accuracy) => ("official_compatible_local", Some(accuracy)),
        None => ("unofficial_partial", None),
    };
    let outcomes = store.read_outcomes().map_err(core_error)?;
    let diagnostics = GaiaDiagnostics::from_outcomes(&outcomes);
    let agent_evaluation_eligible = diagnostics.agent_evaluation_eligible();
    let mut score = serde_json::json!({
        "run_id": run_id,
        "status": status,
        "split": report.split(),
        "level": report.level(),
        "evaluated": report.evaluated(),
        "correct": report.correct(),
        "complete": report.is_complete(),
        "official_dataset_compatible": report.is_official_dataset_compatible(),
        "agent_evaluation_eligible": agent_evaluation_eligible,
        "diagnostics": diagnostics.to_json(),
    });
    if let Some(accuracy) = comparable_accuracy {
        score["comparable_accuracy"] = serde_json::json!(accuracy);
    }
    let mut score_bytes =
        serde_json::to_vec(&score).map_err(|_| CliError::failed("gaia_score_artifact_failed"))?;
    score_bytes.push(b'\n');
    publish_or_verify_bytes(store, "score.json", &score_bytes)?;

    let accuracy = comparable_accuracy
        .map(|value| format!("{value:.6}"))
        .unwrap_or_else(|| "不可比较".to_owned());
    let markdown = format!(
        "# GAIA Level 1 评测报告\n\n- Run ID: `{run_id}`\n- 状态: `{status}`\n- Complete: {}\n- Official dataset compatible: {}\n- 可用于 Agent 趋势比较: {}\n- Split: `{}`\n- Level: `{}`\n- 完成评分: {}\n- 正确: {} / {}\n- Comparable accuracy: {accuracy}\n\n## 接入诊断\n\n{}\n",
        report.is_complete(),
        report.is_official_dataset_compatible(),
        agent_evaluation_eligible,
        report.split(),
        report.level(),
        report.evaluated(),
        report.correct(),
        report.evaluated(),
        diagnostics.render_markdown(),
    );
    let report_path = store.run_dir().join("report.md");
    if report_path.exists() {
        let existing = std::fs::read(&report_path)
            .map_err(|_| CliError::failed("gaia_score_artifact_failed"))?;
        if existing != markdown.as_bytes() {
            return Err(CliError::failed("gaia_score_artifact_conflict"));
        }
    } else {
        publish_markdown_report(store, &markdown).map_err(core_error)?;
    }
    Ok(())
}

#[derive(Default)]
struct GaiaDiagnostics {
    statuses: BTreeMap<&'static str, u64>,
    failure_categories: BTreeMap<&'static str, u64>,
    failure_reasons: BTreeMap<&'static str, u64>,
    integration_layers: BTreeMap<&'static str, u64>,
    observed_issue_layers: BTreeMap<&'static str, u64>,
    tool_proposals: u64,
    tool_calls: u64,
    tool_failures: u64,
    tool_failure_reasons: BTreeMap<&'static str, u64>,
    agent_control_events: BTreeMap<&'static str, u64>,
    elapsed_ms: u64,
    model_request_durations_ms: Vec<u64>,
    model_request_ttft_ms: Vec<u64>,
    effective_decode_tps: Vec<f64>,
    tool_elapsed_ms: Vec<u64>,
}

impl GaiaDiagnostics {
    fn from_outcomes(outcomes: &[TaskOutcome]) -> Self {
        let mut diagnostics = Self::default();
        for outcome in outcomes {
            *diagnostics
                .statuses
                .entry(status_key(outcome.status()))
                .or_default() += 1;
            diagnostics.elapsed_ms = diagnostics.elapsed_ms.saturating_add(outcome.elapsed_ms());
            diagnostics.tool_proposals = diagnostics
                .tool_proposals
                .saturating_add(outcome.tool_observations().len() as u64);
            let executed_tools = outcome
                .tool_observations()
                .iter()
                .filter(|tool| !is_agent_control_event(tool.failure_code.as_deref()));
            diagnostics.tool_calls = diagnostics
                .tool_calls
                .saturating_add(executed_tools.clone().count() as u64);
            diagnostics.tool_failures = diagnostics
                .tool_failures
                .saturating_add(executed_tools.clone().filter(|tool| tool.failed).count() as u64);
            diagnostics.tool_elapsed_ms.extend(
                executed_tools
                    .map(|tool| tool.elapsed_ms)
                    .filter(|elapsed| *elapsed > 0),
            );
            for request in outcome.model_request_observations() {
                diagnostics
                    .model_request_durations_ms
                    .push(request.request_duration_ms);
                if let Some(ttft_ms) = request.ttft_ms {
                    diagnostics.model_request_ttft_ms.push(ttft_ms);
                    let decode_ms = request.request_duration_ms.saturating_sub(ttft_ms);
                    if decode_ms > 0 && request.output_tokens > 0 {
                        diagnostics
                            .effective_decode_tps
                            .push(request.output_tokens as f64 * 1_000.0 / decode_ms as f64);
                    }
                }
            }
            for tool in outcome
                .tool_observations()
                .iter()
                .filter(|tool| tool.failed)
            {
                let reason = tool_failure_reason_key(tool.failure_code.as_deref());
                *diagnostics
                    .observed_issue_layers
                    .entry(tool_observation_issue_layer(tool.failure_code.as_deref()))
                    .or_default() += 1;
                if is_agent_control_event(tool.failure_code.as_deref()) {
                    *diagnostics.agent_control_events.entry(reason).or_default() += 1;
                } else {
                    *diagnostics.tool_failure_reasons.entry(reason).or_default() += 1;
                }
            }
            if let Some(category) = outcome.failure_category() {
                *diagnostics
                    .failure_categories
                    .entry(failure_category_key(category))
                    .or_default() += 1;
            }
            if let Some(reason) = outcome.failure_reason() {
                *diagnostics
                    .failure_reasons
                    .entry(failure_reason_key(reason))
                    .or_default() += 1;
            }
            if outcome.status() != TaskStatus::Completed {
                *diagnostics
                    .integration_layers
                    .entry(integration_layer(outcome))
                    .or_default() += 1;
            }
        }
        diagnostics
    }

    fn to_json(&self) -> serde_json::Value {
        let blockers = self.integration_blockers();
        let eligible = blockers.is_empty();
        serde_json::json!({
            "statuses": self.statuses,
            "failure_categories": self.failure_categories,
            "failure_reasons": self.failure_reasons,
            "integration_layers": self.integration_layers,
            "observed_issue_layers": self.observed_issue_layers,
            "tool_proposals": self.tool_proposals,
            "tool_calls": self.tool_calls,
            "tool_failures": self.tool_failures,
            "tool_failure_reasons": self.tool_failure_reasons,
            "agent_control_events": self.agent_control_events,
            "integration_stability": {
                "status": if eligible { "pass" } else { "fail" },
                "agent_evaluation_eligible": eligible,
                "blockers": blockers,
                "policy": "no model/bridge/GAIA lifecycle failures and no ambiguous tool failures",
            },
            "elapsed_ms": self.elapsed_ms,
            "performance": self.performance_json(),
        })
    }

    fn integration_blockers(&self) -> BTreeMap<&'static str, u64> {
        let mut blockers = BTreeMap::new();
        for key in ["model", "gaia_integration", "agent_or_model_bridge"] {
            if let Some(count) = self
                .integration_layers
                .get(key)
                .copied()
                .filter(|count| *count > 0)
            {
                blockers.insert(key, count);
            }
        }
        for key in [
            "search_provider_config",
            "tool_execution_failed",
            "unclassified",
        ] {
            if let Some(count) = self
                .tool_failure_reasons
                .get(key)
                .copied()
                .filter(|count| *count > 0)
            {
                blockers.insert(key, count);
            }
        }
        blockers
    }

    fn agent_evaluation_eligible(&self) -> bool {
        self.integration_blockers().is_empty()
    }

    fn performance_json(&self) -> serde_json::Value {
        let model_count = self.model_request_durations_ms.len();
        let ttft_count = self.model_request_ttft_ms.len();
        serde_json::json!({
            "model_requests": {
                "count": model_count,
                "request_duration_ms": distribution_json_u64(&self.model_request_durations_ms),
                "ttft_ms": {
                    "samples": ttft_count,
                    "coverage_ratio": ratio(ttft_count, model_count),
                    "distribution": distribution_json_u64(&self.model_request_ttft_ms),
                },
                "effective_decode_tokens_per_second": {
                    "samples": self.effective_decode_tps.len(),
                    "distribution": distribution_json_f64(&self.effective_decode_tps),
                    "definition": "output_tokens / (request_duration - ttft)",
                },
            },
            "tools": {
                "calls": self.tool_calls,
                "elapsed_samples": self.tool_elapsed_ms.len(),
                "elapsed_coverage_ratio": ratio(self.tool_elapsed_ms.len(), self.tool_calls as usize),
                "elapsed_ms": distribution_json_u64(&self.tool_elapsed_ms),
            },
            "scope": "benchmark-only successful model requests; not a server-side hardware throughput benchmark",
        })
    }

    fn render_human(&self) -> String {
        let blockers = self.integration_blockers();
        format!(
            "Integration stability: {} (Agent comparison eligible: {})\nIntegration blockers: {}\nTask-level integration failures: {}\nObserved issue attribution: {}\nFailure reasons: {}\nTool proposals: {}\nExecuted tool calls: {} (failed: {})\nTool failure reasons: {}\nAgent control events: {}\nRecorded elapsed: {} ms\nPerformance: {}",
            if blockers.is_empty() { "PASS" } else { "FAIL" },
            self.agent_evaluation_eligible(),
            render_counts(&blockers),
            render_counts(&self.integration_layers),
            render_counts(&self.observed_issue_layers),
            render_counts(&self.failure_reasons),
            self.tool_proposals,
            self.tool_calls,
            self.tool_failures,
            render_counts(&self.tool_failure_reasons),
            render_counts(&self.agent_control_events),
            self.elapsed_ms,
            self.performance_json(),
        )
    }

    fn render_markdown(&self) -> String {
        let performance = serde_json::to_string_pretty(&self.performance_json())
            .unwrap_or_else(|_| "{}".to_owned());
        let blockers = self.integration_blockers();
        format!(
            "### 接入稳定性门禁\n\n- 状态: **{}**\n- 可用于 Agent 能力趋势比较: **{}**\n- 阻断项: {}\n- 规则: 模型/桥接/GAIA 生命周期必须零失败，且不能残留 `tool_execution_failed` 或未知工具错误。\n\n- 任务级接入失败归属: {}\n- 观测问题归属: {}\n- 安全失败原因: {}\n- 失败类别: {}\n- 模型工具提议: {}\n- 实际执行工具: {}（失败 {}）\n- 工具执行失败原因: {}\n- Agent 控制事件: {}\n- 已记录总耗时: {} ms\n\n### Benchmark 性能观测\n\n```json\n{}\n```\n\n- `agent`：Agent 循环或最终输出契约问题。\n- `model`：模型请求或协议问题。\n- `gaia_integration`：GAIA 输入、附件、backend 生命周期或私有输出接入问题。\n- `agent_or_model_bridge`：任务级超时（模型与 Agent 循环无法安全区分）或旧记录只有通用 backend 失败，现有安全数据不足以继续细分。\n- `model_tool_call`：模型选择了禁止动作、生成错误参数或请求不存在的资源。\n- `agent_recovery_policy`：工具失败后重复调用直至预算拒绝，属于 Agent/模型恢复策略问题。\n- `external_network`：目标网络连接或超时，不等同于桥接失败。\n- `external_web_source`：目标站点拒绝、动态页面或正文提取失败，不等同于 Desktop/CLI 接入失败。\n- `gaia_integration_or_unknown`：评测配置失败，或错误码仍过于宽泛；存在时本轮禁止用于 Agent 趋势比较。\n- `unclassified_tool_observation`：工具失败错误码超出固定白名单（含未知码），按未知工具错误处理并阻断本轮 Agent 趋势比较。\n- 性能范围：仅覆盖 benchmark 中成功返回 usage 的模型请求。\n- 有效解码吞吐：`output_tokens / (请求耗时 - TTFT)`，不能替代推理服务器侧的纯硬件吞吐测试。\n- 工具诊断：预算耗尽是未执行提议的 Agent 控制事件，不计入工具调用或工具执行失败；失败原因只记录固定白名单错误码，不包含参数或返回内容。",
            if blockers.is_empty() { "PASS" } else { "FAIL" },
            self.agent_evaluation_eligible(),
            render_counts(&blockers),
            render_counts(&self.integration_layers),
            render_counts(&self.observed_issue_layers),
            render_counts(&self.failure_reasons),
            render_counts(&self.failure_categories),
            self.tool_proposals,
            self.tool_calls,
            self.tool_failures,
            render_counts(&self.tool_failure_reasons),
            render_counts(&self.agent_control_events),
            self.elapsed_ms,
            performance,
        )
    }
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn distribution_json_u64(values: &[u64]) -> serde_json::Value {
    let values = values.iter().map(|value| *value as f64).collect::<Vec<_>>();
    distribution_json_f64(&values)
}

fn distribution_json_f64(values: &[f64]) -> serde_json::Value {
    if values.is_empty() {
        return serde_json::json!({"p50": null, "p95": null, "max": null});
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let percentile = |fraction: f64| {
        let index = ((sorted.len() - 1) as f64 * fraction).ceil() as usize;
        sorted[index]
    };
    serde_json::json!({
        "p50": percentile(0.50),
        "p95": percentile(0.95),
        "max": sorted[sorted.len() - 1],
    })
}

fn tool_failure_reason_key(code: Option<&str>) -> &'static str {
    match code {
        Some("tool_call_budget_exhausted") => "tool_call_budget_exhausted",
        Some("missing_action") => "missing_action",
        Some("invalid_arguments") => "invalid_arguments",
        Some("tool_execution_failed") => "tool_execution_failed",
        Some("network_failed") => "network_failed",
        Some("network_timeout") => "network_timeout",
        Some("dynamic_page_unreadable") => "dynamic_page_unreadable",
        Some("content_extraction_failed") => "content_extraction_failed",
        Some("remote_access_denied") => "remote_access_denied",
        Some("http_status_failed") => "http_status_failed",
        Some("resource_not_found") => "resource_not_found",
        Some("network_policy_blocked") => "network_policy_blocked",
        Some("restricted_address") => "restricted_address",
        Some("policy_denied") => "policy_denied",
        Some("search_provider_config") => "search_provider_config",
        Some("host_read_only_blocked") => "host_read_only_blocked",
        Some("host_tool_blocked") => "host_tool_blocked",
        Some("approval_required") => "approval_required",
        _ => "unclassified",
    }
}

fn is_agent_control_event(code: Option<&str>) -> bool {
    code == Some("tool_call_budget_exhausted")
}

fn tool_observation_issue_layer(code: Option<&str>) -> &'static str {
    match code {
        Some("tool_call_budget_exhausted") => "agent_recovery_policy",
        Some(
            "missing_action"
            | "invalid_arguments"
            | "host_read_only_blocked"
            | "host_tool_blocked"
            | "network_policy_blocked"
            | "restricted_address"
            | "policy_denied",
        ) => "model_tool_call",
        Some("search_provider_config" | "tool_execution_failed") => "gaia_integration_or_unknown",
        Some("network_failed" | "network_timeout") => "external_network",
        Some(
            "dynamic_page_unreadable"
            | "content_extraction_failed"
            | "remote_access_denied"
            | "http_status_failed",
        ) => "external_web_source",
        Some("resource_not_found") => "model_tool_call",
        _ => "unclassified_tool_observation",
    }
}

fn render_counts(counts: &BTreeMap<&'static str, u64>) -> String {
    if counts.is_empty() {
        return "none".to_owned();
    }
    counts
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn status_key(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Planned => "planned",
        TaskStatus::Running => "running",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
        TaskStatus::Timeout => "timeout",
        TaskStatus::Cancelled => "cancelled",
    }
}

fn failure_category_key(category: &SafeFailureCategory) -> &'static str {
    match category {
        SafeFailureCategory::Backend => "backend",
        SafeFailureCategory::Timeout => "timeout",
        SafeFailureCategory::InvalidOutput => "invalid_output",
        SafeFailureCategory::Infrastructure => "infrastructure",
        SafeFailureCategory::Cancelled => "cancelled",
    }
}

fn failure_reason_key(reason: SafeFailureReason) -> &'static str {
    match reason {
        SafeFailureReason::TaskTimeout => "task_timeout",
        SafeFailureReason::MissingFinalAnswer => "missing_final_answer",
        SafeFailureReason::AgentTurnFailed => "agent_turn_failed",
        SafeFailureReason::AgentToolFailed => "agent_tool_failed",
        SafeFailureReason::ModelContextLimit => "model_context_limit",
        SafeFailureReason::ModelRateLimited => "model_rate_limited",
        SafeFailureReason::ModelRequestTimeout => "model_request_timeout",
        SafeFailureReason::ModelTransportFailed => "model_transport_failed",
        SafeFailureReason::ModelProtocolFailed => "model_protocol_failed",
        SafeFailureReason::AttachmentResolutionFailed => "attachment_resolution_failed",
        SafeFailureReason::AttachmentStagingFailed => "attachment_staging_failed",
        SafeFailureReason::BackendPrepareFailed => "backend_prepare_failed",
        SafeFailureReason::BackendCloseFailed => "backend_close_failed",
        SafeFailureReason::PrivateOutputResolutionFailed => "private_output_resolution_failed",
        SafeFailureReason::IntegrationLifecycleFailed => "integration_lifecycle_failed",
    }
}

fn integration_layer(outcome: &TaskOutcome) -> &'static str {
    match outcome.failure_reason() {
        Some(SafeFailureReason::TaskTimeout) => "agent_or_model_bridge",
        Some(
            SafeFailureReason::ModelContextLimit
            | SafeFailureReason::ModelRateLimited
            | SafeFailureReason::ModelRequestTimeout
            | SafeFailureReason::ModelTransportFailed
            | SafeFailureReason::ModelProtocolFailed,
        ) => "model",
        Some(
            SafeFailureReason::AgentTurnFailed
            | SafeFailureReason::AgentToolFailed
            | SafeFailureReason::MissingFinalAnswer,
        ) => "agent",
        Some(
            SafeFailureReason::AttachmentResolutionFailed
            | SafeFailureReason::AttachmentStagingFailed
            | SafeFailureReason::BackendPrepareFailed
            | SafeFailureReason::BackendCloseFailed
            | SafeFailureReason::PrivateOutputResolutionFailed
            | SafeFailureReason::IntegrationLifecycleFailed,
        ) => "gaia_integration",
        None => match outcome.failure_category() {
            Some(SafeFailureCategory::Infrastructure) => "gaia_integration",
            _ => "agent_or_model_bridge",
        },
    }
}

fn publish_or_verify_bytes(store: &RunStore, name: &str, expected: &[u8]) -> Result<(), CliError> {
    let path = store.run_dir().join(name);
    if path.exists() {
        let existing =
            std::fs::read(path).map_err(|_| CliError::failed("gaia_score_artifact_failed"))?;
        if existing != expected {
            return Err(CliError::failed("gaia_score_artifact_conflict"));
        }
        return Ok(());
    }
    if name != "score.json" {
        return Err(CliError::failed("gaia_score_artifact_failed"));
    }
    publish_score_json(store, expected)
        .map_err(core_error)
        .map(|_| ())
}

fn submission_gaia(
    run_id: &str,
    destination: &Path,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let (adapter, run) = open_gaia_run(run_id)?;
    adapter
        .write_submission(&run, destination)
        .map_err(core_error)?;
    let text = match output {
        OutputMode::Human => "GAIA submission written".to_owned(),
        OutputMode::Json => "{\"status\":\"written\"}".to_owned(),
    };
    Ok(success(text))
}

/// 生成 JSON 字符串字面量的内部内容(不含引号)。委托 serde_json,覆盖
/// 控制字符等全部需要转义的码点;手写 replace 会漏掉 \t、\u0000-\u001F。
fn json_escape(value: &str) -> String {
    let encoded = serde_json::to_string(value).unwrap_or_default();
    encoded[1..encoded.len().saturating_sub(1)].to_owned()
}

#[cfg(not(feature = "product-backend"))]
fn run_smoke(_output: OutputMode) -> Result<CliOutcome, CliError> {
    Err(CliError::failed("product_backend_not_enabled"))
}

#[cfg(not(feature = "product-backend"))]
fn resume_smoke(_run_id: &str, _output: OutputMode) -> Result<CliOutcome, CliError> {
    Err(CliError::failed("product_backend_not_enabled"))
}

#[cfg(not(feature = "product-backend"))]
fn run_gaia(_output: OutputMode) -> Result<CliOutcome, CliError> {
    Err(CliError::failed("product_backend_not_enabled"))
}

#[cfg(feature = "product-backend")]
mod product {
    use std::path::Path;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use adapter_gaia::GaiaPrivateInputs;
    use adapter_smoke::{SmokeAdapter, SmokePrivateInputs};
    use agent_backend_api::{
        AgentBackendError, AgentRunObserver, AgentSessionHandle, AgentTaskInput, AgentTaskOutcome,
        HeadlessAgentBackend, PrepareRequest, PrivateInputResolver, PrivateOutputHandle,
        SecretOutput, SuiteModelIdentity,
    };
    use async_trait::async_trait;
    use benchmark_core::{
        BenchmarkAdapter, BenchmarkService, ModelIdentity, NativeAgentRunner, RunManifest,
        RunSummary, Split, TaskSelection, ToolPolicyId,
    };

    use super::*;

    struct DynamicBackend(Arc<dyn HeadlessAgentBackend>);

    #[async_trait]
    impl HeadlessAgentBackend for DynamicBackend {
        fn suite_model_identity(&self) -> Option<SuiteModelIdentity> {
            self.0.suite_model_identity()
        }

        async fn prepare(
            &self,
            request: PrepareRequest,
        ) -> Result<AgentSessionHandle, AgentBackendError> {
            self.0.prepare(request).await
        }

        async fn run(
            &self,
            session: &AgentSessionHandle,
            task: AgentTaskInput,
            private_inputs: Arc<dyn PrivateInputResolver>,
            observer: Arc<dyn AgentRunObserver>,
        ) -> Result<AgentTaskOutcome, AgentBackendError> {
            self.0.run(session, task, private_inputs, observer).await
        }

        async fn cancel(&self, session: &AgentSessionHandle) -> Result<(), AgentBackendError> {
            self.0.cancel(session).await
        }

        async fn resolve_output(
            &self,
            handle: &PrivateOutputHandle,
        ) -> Result<SecretOutput, AgentBackendError> {
            self.0.resolve_output(handle).await
        }

        async fn close(&self, session: AgentSessionHandle) -> Result<(), AgentBackendError> {
            self.0.close(session).await
        }
    }

    pub(super) fn run(output: OutputMode) -> Result<CliOutcome, CliError> {
        let base = benchmark_base()?;
        pinvou_product_backend::run_with_product_backend(move |backend| async move {
            let identity = backend
                .suite_model_identity()
                .ok_or_else(|| anyhow::anyhow!("suite_model_identity_unavailable"))?;
            let adapter = SmokeAdapter::new();
            let dataset = adapter.verify_dataset(Path::new("."))?;
            let plan = adapter.plan(&dataset, &TaskSelection::all())?;
            let run_id = new_run_id();
            let manifest = RunManifest::new(
                &run_id,
                adapter.descriptor(),
                Split::new("smoke"),
                ModelIdentity::new(identity.provider(), identity.model())?,
                ToolPolicyId::new("pinvou-product/v1"),
                1,
            )?;
            let service = BenchmarkService::native_with_private_inputs(
                &base,
                Arc::new(DynamicBackend(backend)),
                Arc::new(SmokePrivateInputs::new()),
            )?;
            let summary = service.run(manifest, &plan).await?;
            finalize_summary(&base, summary, output)
        })
        .map_err(|_| CliError::failed("smoke_run_failed"))
    }

    pub(super) fn run_gaia(output: OutputMode) -> Result<CliOutcome, CliError> {
        let base = benchmark_base()?;
        let snapshot = gaia_snapshot_root()?;
        pinvou_product_backend::run_with_product_backend(move |backend| async move {
            let identity = backend
                .suite_model_identity()
                .ok_or_else(|| anyhow::anyhow!("suite_model_identity_unavailable"))?;
            let dataset = Arc::new(open_official_gaia_dataset(&snapshot)?);
            let adapter = GaiaAdapter::with_dataset(Arc::clone(&dataset));
            let verified = adapter.verify_dataset(&snapshot)?;
            let run_id = new_gaia_run_id();
            let manifest = RunManifest::new(
                &run_id,
                adapter.descriptor(),
                Split::new(GAIA_SPLIT),
                ModelIdentity::new(identity.provider(), identity.model())?,
                ToolPolicyId::new("pinvou-gaia-public-web/v1"),
                1,
            )?;
            let service = BenchmarkService::native_with_private_inputs(
                &base,
                Arc::new(DynamicBackend(backend)),
                Arc::new(GaiaPrivateInputs::new(dataset)),
            )?;
            let summary = service
                .run_adapter(manifest, &adapter, &verified, &TaskSelection::all())
                .await?;
            Ok::<_, anyhow::Error>(gaia_summary(summary, output))
        })
        .map_err(|_| CliError::failed("gaia_run_failed"))
    }

    pub(super) fn resume_gaia(run_id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
        let base = benchmark_base()?;
        let snapshot = gaia_snapshot_root()?;
        let run_id = run_id.to_owned();
        pinvou_product_backend::run_with_product_backend(move |backend| async move {
            let identity = backend
                .suite_model_identity()
                .ok_or_else(|| anyhow::anyhow!("suite_model_identity_unavailable"))?;
            let dataset = Arc::new(open_official_gaia_dataset(&snapshot)?);
            let adapter = GaiaAdapter::with_dataset(Arc::clone(&dataset));
            let verified = adapter.verify_dataset(&snapshot)?;
            let store = RunStore::open(&base, &run_id)?;
            let manifest = store.read_manifest()?;
            let model = ModelIdentity::new(identity.provider(), identity.model())?;
            if !manifest.matches_resume(
                adapter.descriptor(),
                GAIA_SPLIT,
                &model,
                "pinvou-gaia-public-web/v1",
            ) {
                return Err(anyhow::anyhow!("resume_manifest_mismatch"));
            }
            let service = BenchmarkService::native_with_private_inputs(
                &base,
                Arc::new(DynamicBackend(backend)),
                Arc::new(GaiaPrivateInputs::new(dataset)),
            )?;
            let summary = service
                .resume_adapter(
                    &run_id,
                    &manifest,
                    &adapter,
                    &verified,
                    &TaskSelection::all(),
                )
                .await?;
            Ok::<_, anyhow::Error>(gaia_summary(summary, output))
        })
        .map_err(|error| {
            if error.to_string().contains("resume_manifest_mismatch") {
                CliError::failed("resume_manifest_mismatch")
            } else {
                CliError::failed("gaia_resume_failed")
            }
        })
    }

    pub(super) fn resume(run_id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
        let base = benchmark_base()?;
        let run_id = run_id.to_owned();
        pinvou_product_backend::run_with_product_backend(move |backend| async move {
            let identity = backend
                .suite_model_identity()
                .ok_or_else(|| anyhow::anyhow!("suite_model_identity_unavailable"))?;
            let adapter = SmokeAdapter::new();
            let dataset = adapter.verify_dataset(Path::new("."))?;
            let plan = adapter.plan(&dataset, &TaskSelection::all())?;
            let store = RunStore::open(&base, &run_id)?;
            let stored = store.read_manifest()?;
            let model = ModelIdentity::new(identity.provider(), identity.model())?;
            if !stored.matches_resume(adapter.descriptor(), "smoke", &model, "pinvou-product/v1") {
                return Err(anyhow::anyhow!("resume_manifest_mismatch"));
            }
            let service: BenchmarkService<NativeAgentRunner<DynamicBackend>> =
                BenchmarkService::native_with_private_inputs(
                    &base,
                    Arc::new(DynamicBackend(backend)),
                    Arc::new(SmokePrivateInputs::new()),
                )?;
            let summary = service.resume(&run_id, &plan).await?;
            finalize_summary(&base, summary, output)
        })
        .map_err(|error| {
            if error.to_string().contains("resume_manifest_mismatch") {
                CliError::failed("resume_manifest_mismatch")
            } else {
                CliError::failed("smoke_resume_failed")
            }
        })
    }

    fn new_run_id() -> String {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        format!("smoke-{millis}-{}", std::process::id())
    }

    fn new_gaia_run_id() -> String {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        format!("gaia-{millis}-{}", std::process::id())
    }

    fn gaia_summary(summary: RunSummary, output: OutputMode) -> CliOutcome {
        let failed = summary.outcomes().len().saturating_sub(summary.completed());
        let stdout = match output {
            OutputMode::Human => format!(
                "Run: {}\nCompleted: {}\nFailed: {failed}\nRemaining: {}",
                summary.run_id(),
                summary.completed(),
                summary.remaining(),
            ),
            OutputMode::Json => format!(
                "{{\"run_id\":\"{}\",\"completed\":{},\"failed\":{failed},\"remaining\":{}}}",
                json_escape(summary.run_id()),
                summary.completed(),
                summary.remaining(),
            ),
        };
        finalized_outcome(stdout, failed)
    }

    fn finalize_summary(
        base: &Path,
        summary: RunSummary,
        output: OutputMode,
    ) -> anyhow::Result<CliOutcome> {
        finalize_smoke_outcomes(
            base,
            summary.run_id(),
            summary.completed(),
            summary.outcomes(),
            output,
        )
        .map_err(anyhow::Error::from)
    }
}

#[cfg(feature = "product-backend")]
fn run_smoke(output: OutputMode) -> Result<CliOutcome, CliError> {
    product::run(output)
}

#[cfg(feature = "product-backend")]
fn resume_smoke(run_id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store = RunStore::open(&benchmark_base()?, run_id).map_err(core_error)?;
    match store.read_manifest().map_err(core_error)?.benchmark() {
        "gaia" => product::resume_gaia(run_id, output),
        "smoke" => product::resume(run_id, output),
        _ => Err(CliError::failed("resume_manifest_mismatch")),
    }
}

#[cfg(feature = "product-backend")]
fn run_gaia(output: OutputMode) -> Result<CliOutcome, CliError> {
    product::run_gaia(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_base(name: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("pinvou-cli-{name}-{nonce}"));
        std::fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn raw_gaia_verify_success_reports_dataset_validation_without_ready_claims() {
        let source = Path::new("private-raw-snapshot");
        let outcome = verify_gaia_with(source, OutputMode::Human, |actual| {
            assert_eq!(actual, source);
            Ok(())
        })
        .unwrap();

        assert_eq!(outcome.exit_code, ExitCode::Success);
        assert!(outcome.stdout.contains("GAIA dataset verified"));
        assert!(!outcome.stdout.contains("ready"));
    }

    #[test]
    fn official_gaia_consumer_rejects_a_tampered_marker_at_the_real_ready_root() {
        let home = temp_base("gaia-tampered-ready");
        let previous = std::env::var_os("PINVOU3_HOME");
        unsafe { std::env::set_var("PINVOU3_HOME", &home) };
        let snapshot = gaia_snapshot_root().unwrap();
        std::fs::create_dir(&snapshot).unwrap();
        std::fs::write(
            snapshot.join(adapter_gaia::GAIA_READY_MARKER),
            b"tampered\n",
        )
        .unwrap();

        let error = open_official_gaia_dataset(&snapshot)
            .expect_err("official consumers must enforce the published-ready integrity marker");
        assert_eq!(error.to_string(), "gaia_verify_failed");
        assert!(!error.to_string().contains(snapshot.to_str().unwrap()));

        match previous {
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn finalized_run_with_failed_cases_returns_failed_exit_code() {
        assert_eq!(
            finalized_outcome("summary".to_owned(), 1).exit_code,
            ExitCode::Failed
        );
        assert_eq!(
            finalized_outcome("summary".to_owned(), 0).exit_code,
            ExitCode::Success
        );
    }

    #[test]
    fn smoke_finalize_publishes_report_matching_failed_summary_and_score() {
        use benchmark_core::{
            BenchmarkDescriptor, BenchmarkId, ExecutionKind, ModelIdentity, RunManifest, Split,
            TaskOutcome, TaskStatus, ToolPolicyId,
        };

        let base = temp_base("finalize");
        let descriptor = BenchmarkDescriptor::new(
            BenchmarkId::new("smoke"),
            "smoke-adapter/v1",
            "embedded-plep-smoke/v1",
            "pinvou-product-score/v1",
            vec![Split::new("smoke")],
            ExecutionKind::NativeTurn,
        );
        let manifest = RunManifest::new(
            "run-finalize",
            &descriptor,
            Split::new("smoke"),
            ModelIdentity::new("fixture", "model").unwrap(),
            ToolPolicyId::new("pinvou-product/v1"),
            1,
        )
        .unwrap();
        RunStore::create(&base, &manifest).unwrap();
        let outcomes = vec![
            TaskOutcome::new("plep_smoke_hi", TaskStatus::Completed, None, vec![], 10),
            TaskOutcome::new(
                "plep_smoke_weather",
                TaskStatus::Timeout,
                None,
                vec![],
                60_000,
            ),
        ];

        let result =
            finalize_smoke_outcomes(&base, "run-finalize", 1, &outcomes, OutputMode::Human)
                .unwrap();
        assert_eq!(result.exit_code, ExitCode::Failed);
        assert!(result.stdout.contains("Completed: 1\nFailed: 1"));
        let report_path = RunStore::open(&base, "run-finalize")
            .unwrap()
            .run_dir()
            .join("report.md")
            .canonicalize()
            .unwrap();
        assert!(result.stdout.contains(&report_path.display().to_string()));
        let report = std::fs::read_to_string(report_path).unwrap();
        let score = result
            .stdout
            .lines()
            .find_map(|line| line.strip_prefix("Smoke Health Score: "))
            .unwrap();
        assert!(report.contains(&format!("总分：{score} (pinvou-product-score/v1)")));
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn gaia_score_publishes_machine_and_markdown_artifacts() {
        use adapter_gaia::GaiaAdapter;
        use benchmark_core::{
            BenchmarkAdapter, ModelIdentity, OfficialScoreReport, RunManifest, Split, ToolPolicyId,
        };

        let base = temp_base("gaia-score-artifacts");
        let adapter = GaiaAdapter::new();
        let manifest = RunManifest::new(
            "gaia-score-artifacts",
            adapter.descriptor(),
            Split::new(GAIA_SPLIT),
            ModelIdentity::new("fixture", "model").unwrap(),
            ToolPolicyId::new("pinvou-gaia-public-web/v1"),
            1,
        )
        .unwrap();
        let store = RunStore::create(&base, &manifest).unwrap();
        let report = OfficialScoreReport::compatible(53, 31, GAIA_SPLIT, "1");

        publish_gaia_score_artifacts(&store, "gaia-score-artifacts", &report).unwrap();

        let score: serde_json::Value =
            serde_json::from_slice(&std::fs::read(store.run_dir().join("score.json")).unwrap())
                .unwrap();
        assert_eq!(score["run_id"], "gaia-score-artifacts");
        assert_eq!(score["evaluated"], 53);
        assert_eq!(score["correct"], 31);
        assert_eq!(score["status"], "official_compatible_local");
        assert_eq!(score["complete"], true);
        assert_eq!(score["official_dataset_compatible"], true);
        assert_eq!(score["agent_evaluation_eligible"], true);
        assert_eq!(
            score["diagnostics"]["integration_stability"]["status"],
            "pass"
        );
        assert_eq!(
            score["diagnostics"]["integration_layers"],
            serde_json::json!({})
        );
        assert_eq!(
            score["diagnostics"]["observed_issue_layers"],
            serde_json::json!({})
        );
        let markdown = std::fs::read_to_string(store.run_dir().join("report.md")).unwrap();
        assert!(markdown.contains("# GAIA Level 1 评测报告"));
        assert!(markdown.contains("Complete: true"));
        assert!(markdown.contains("Official dataset compatible: true"));
        assert!(markdown.contains("可用于 Agent 趋势比较: true"));
        assert!(markdown.contains("状态: **PASS**"));
        assert!(markdown.contains("31 / 53"));
        assert!(markdown.contains("## 接入诊断"));

        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn gaia_diagnostics_attribute_only_fixed_safe_failure_reasons() {
        let outcomes = vec![
            TaskOutcome::new("agent", TaskStatus::Failed, None, vec![], 11)
                .with_failure_category(SafeFailureCategory::InvalidOutput)
                .with_failure_reason(SafeFailureReason::MissingFinalAnswer),
            TaskOutcome::new("model", TaskStatus::Failed, None, vec![], 13)
                .with_failure_category(SafeFailureCategory::Backend)
                .with_failure_reason(SafeFailureReason::ModelProtocolFailed),
            TaskOutcome::new("gaia", TaskStatus::Failed, None, vec![], 17)
                .with_failure_category(SafeFailureCategory::Infrastructure)
                .with_failure_reason(SafeFailureReason::AttachmentStagingFailed),
            TaskOutcome::new("legacy", TaskStatus::Failed, None, vec![], 19)
                .with_failure_category(SafeFailureCategory::Backend)
                .with_tool_observations(vec![benchmark_core::ToolObservation {
                    canonical_name: "Web".to_owned(),
                    failed: true,
                    elapsed_ms: 25,
                    failure_code: Some("tool_call_budget_exhausted".to_owned()),
                }])
                .with_model_request_observations(vec![benchmark_core::ModelRequestObservation {
                    request_duration_ms: 1_000,
                    ttft_ms: Some(200),
                    input_tokens: 100,
                    output_tokens: 40,
                }]),
        ];

        let diagnostics = GaiaDiagnostics::from_outcomes(&outcomes).to_json();
        assert_eq!(diagnostics["integration_layers"]["agent"], 1);
        assert_eq!(diagnostics["integration_layers"]["model"], 1);
        assert_eq!(diagnostics["integration_layers"]["gaia_integration"], 1);
        assert_eq!(
            diagnostics["integration_layers"]["agent_or_model_bridge"],
            1
        );
        assert_eq!(diagnostics["elapsed_ms"], 60);
        assert_eq!(diagnostics["tool_proposals"], 1);
        assert_eq!(diagnostics["tool_calls"], 0);
        assert_eq!(diagnostics["tool_failures"], 0);
        assert_eq!(
            diagnostics["agent_control_events"]["tool_call_budget_exhausted"],
            1
        );
        assert_eq!(
            diagnostics["observed_issue_layers"]["agent_recovery_policy"],
            1
        );
        assert_eq!(diagnostics["performance"]["model_requests"]["count"], 1);
        assert_eq!(
            diagnostics["performance"]["model_requests"]["ttft_ms"]["distribution"]["p50"],
            200.0
        );
        assert_eq!(
            diagnostics["performance"]["model_requests"]["effective_decode_tokens_per_second"]["distribution"]
                ["p50"],
            50.0
        );
        assert_eq!(diagnostics["performance"]["tools"]["elapsed_samples"], 0);
        assert_eq!(diagnostics["integration_stability"]["status"], "fail");
        assert_eq!(
            diagnostics["integration_stability"]["agent_evaluation_eligible"],
            false
        );
    }

    #[test]
    fn gaia_tool_observations_have_explicit_issue_attribution() {
        assert_eq!(
            tool_observation_issue_layer(Some("invalid_arguments")),
            "model_tool_call"
        );
        assert_eq!(
            tool_observation_issue_layer(Some("host_read_only_blocked")),
            "model_tool_call"
        );
        assert_eq!(
            tool_observation_issue_layer(Some("policy_denied")),
            "model_tool_call"
        );
        assert_eq!(
            tool_observation_issue_layer(Some("tool_call_budget_exhausted")),
            "agent_recovery_policy"
        );
        assert_eq!(
            tool_observation_issue_layer(Some("network_failed")),
            "external_network"
        );
        assert_eq!(
            tool_observation_issue_layer(Some("tool_execution_failed")),
            "gaia_integration_or_unknown"
        );
        assert_eq!(
            tool_observation_issue_layer(Some("dynamic_page_unreadable")),
            "external_web_source"
        );
        assert_eq!(
            tool_observation_issue_layer(Some("resource_not_found")),
            "model_tool_call"
        );
        assert_eq!(
            tool_observation_issue_layer(Some("not_in_the_whitelist")),
            "unclassified_tool_observation"
        );
        assert_eq!(
            tool_failure_reason_key(Some("not_in_the_whitelist")),
            "unclassified"
        );
    }

    #[test]
    fn gaia_integration_gate_rejects_ambiguous_tool_failures() {
        let outcome = TaskOutcome::new("case", TaskStatus::Completed, None, vec![], 1)
            .with_tool_observations(vec![benchmark_core::ToolObservation {
                canonical_name: "Web".to_owned(),
                failed: true,
                elapsed_ms: 1,
                failure_code: Some("tool_execution_failed".to_owned()),
            }]);
        let diagnostics = GaiaDiagnostics::from_outcomes(&[outcome]);
        assert!(!diagnostics.agent_evaluation_eligible());
        assert_eq!(
            diagnostics.integration_blockers()["tool_execution_failed"],
            1
        );

        let unknown = TaskOutcome::new("case", TaskStatus::Completed, None, vec![], 1)
            .with_tool_observations(vec![benchmark_core::ToolObservation {
                canonical_name: "Web".to_owned(),
                failed: true,
                elapsed_ms: 1,
                failure_code: Some("weird_new_code".to_owned()),
            }]);
        let diagnostics = GaiaDiagnostics::from_outcomes(&[unknown]);
        assert!(
            !diagnostics.agent_evaluation_eligible(),
            "unknown tool codes must block eligibility like the unclassified sentinel"
        );
        assert_eq!(diagnostics.integration_blockers()["unclassified"], 1);
    }

    #[test]
    fn gaia_partial_score_never_publishes_fixed_artifacts() {
        use adapter_gaia::GaiaAdapter;
        use benchmark_core::{
            BenchmarkAdapter, ModelIdentity, OfficialScoreReport, RunManifest, Split, ToolPolicyId,
        };

        let base = temp_base("gaia-partial-score-artifacts");
        let adapter = GaiaAdapter::new();
        let manifest = RunManifest::new(
            "gaia-partial-score-artifacts",
            adapter.descriptor(),
            Split::new(GAIA_SPLIT),
            ModelIdentity::new("fixture", "model").unwrap(),
            ToolPolicyId::new("pinvou-gaia-public-web/v1"),
            1,
        )
        .unwrap();
        let store = RunStore::create(&base, &manifest).unwrap();
        let report = OfficialScoreReport::partial(12, 4, GAIA_SPLIT, "1");

        assert_eq!(
            publish_gaia_score_artifacts(&store, "gaia-partial-score-artifacts", &report,)
                .unwrap_err()
                .to_string(),
            "gaia_score_incomplete"
        );
        assert!(!store.run_dir().join("score.json").exists());
        assert!(!store.run_dir().join("report.md").exists());

        std::fs::remove_dir_all(base).unwrap();
    }
}

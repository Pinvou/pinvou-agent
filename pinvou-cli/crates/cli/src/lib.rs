use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use adapter_gaia::{
    GAIA_DATASET_REVISION, GAIA_LEVEL, GAIA_SPLIT, GaiaAdapter, GaiaDataset, GaiaSnapshotManager,
    GaiaSource, HfSnapshotDownloader,
};
#[cfg(any(test, feature = "product-backend"))]
use benchmark_core::TaskOutcome;
use benchmark_core::{
    BenchmarkAdapter, OfficialScoreReport, RunStore, publish_markdown_report, publish_score_json,
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
    let text = match output {
        OutputMode::Human => format!(
            "Run: {run_id}\nCompleted: {}\nFailed: {failed}\nRemaining: {}",
            recovered.completed_task_ids().len(),
            recovered.runnable_task_ids().len()
        ),
        OutputMode::Json => format!(
            "{{\"run_id\":\"{}\",\"completed\":{},\"failed\":{failed},\"remaining\":{}}}",
            json_escape(run_id),
            recovered.completed_task_ids().len(),
            recovered.runnable_task_ids().len()
        ),
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
    publish_gaia_score_artifacts(&store, run_id, &report)?;
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
    let (status, comparable_accuracy) = match report.comparable_accuracy() {
        Some(accuracy) => ("official_compatible_local", Some(accuracy)),
        None => ("unofficial_partial", None),
    };
    let mut score = serde_json::json!({
        "run_id": run_id,
        "status": status,
        "split": report.split(),
        "level": report.level(),
        "evaluated": report.evaluated(),
        "correct": report.correct(),
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
        "# GAIA Level 1 评测报告\n\n- Run ID: `{run_id}`\n- 状态: `{status}`\n- Split: `{}`\n- Level: `{}`\n- 完成评分: {}\n- 正确: {} / {}\n- Comparable accuracy: {accuracy}\n",
        report.split(),
        report.level(),
        report.evaluated(),
        report.correct(),
        report.evaluated(),
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
        let markdown = std::fs::read_to_string(store.run_dir().join("report.md")).unwrap();
        assert!(markdown.contains("# GAIA Level 1 评测报告"));
        assert!(markdown.contains("31 / 53"));

        std::fs::remove_dir_all(base).unwrap();
    }
}

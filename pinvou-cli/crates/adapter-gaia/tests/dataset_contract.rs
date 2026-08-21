use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use adapter_gaia::{
    GAIA_ADAPTER_VERSION, GAIA_DATASET_REVISION, GAIA_LEVEL, GAIA_PARQUET_SHA256,
    GAIA_PARQUET_SIZE, GAIA_REVISION_MARKER, GAIA_SCORER_REVISION, GAIA_SPLIT, GaiaAdapter,
    GaiaDataset, GaiaPrivateInputs,
};
use agent_backend_api::{
    AgentBackendError, AttachmentHandle, PrivateInputHandle, SecretOutput, SecretText,
};
use arrow_array::builder::{MapBuilder, StringBuilder};
use arrow_array::{
    Array, ArrayRef, BinaryArray, Int64Array, ListArray, RecordBatch, StringArray, StructArray,
};
use arrow_schema::{DataType, Field, Fields, Schema};
use async_trait::async_trait;
use benchmark_core::{
    BenchmarkAdapter, BenchmarkService, CompletedRun, ExecutionKind, ExecutionRequest,
    ModelIdentity, PredictionRetention, PrivatePredictionContentType, RunContext, RunManifest,
    RunStore, Split, TaskOutcome, TaskRunner, TaskSelection, TaskStatus, ToolPolicyId,
    VerifiedDataset,
};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use sha2::{Digest, Sha256};

const PARQUET_PATH: &str = "2023/validation/metadata.level1.parquet";
const EXPECTED_COLUMNS: [&str; 7] = [
    "task_id",
    "Question",
    "Level",
    "Final answer",
    "file_name",
    "file_path",
    "Annotator Metadata",
];

#[derive(Clone)]
struct FixtureRow<'a> {
    task_id: &'a str,
    question: &'a str,
    level: i64,
    reference: &'a str,
    file_name: Option<&'a str>,
    file_path: Option<&'a str>,
}

#[derive(Clone, Copy, Default)]
enum MetadataShape {
    #[default]
    Struct,
    Primitive,
    List,
    Map,
    WrongChild,
    ManyChildren,
    Deep,
    MissingField,
    RenamedField,
    ExtraField,
}

#[derive(Clone, Copy, Default)]
enum OptionalColumnShape {
    #[default]
    Utf8,
    Binary,
    Int64,
}

#[derive(Clone, Copy, Default)]
struct FixtureSchema<'a> {
    omit: Option<&'a str>,
    wrong_column: Option<&'a str>,
    metadata: MetadataShape,
    max_row_group_size: Option<usize>,
    required_column: Option<&'a str>,
}

struct TempSnapshot {
    path: PathBuf,
}

impl TempSnapshot {
    fn new() -> Self {
        // 同进程并行测试线程在粗粒度时钟(如 macOS)下纳秒时间戳可能同 tick
        // 撞名,互相 remove_dir_all;用进程内原子计数保证唯一。
        static SNAPSHOT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let unique = SNAPSHOT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pinvou-gaia-contract-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempSnapshot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn valid_rows() -> Vec<FixtureRow<'static>> {
    vec![
        FixtureRow {
            task_id: "safe-task-1",
            question: "SYNTHETIC_PRIVATE_QUESTION_ALPHA",
            level: 1,
            reference: "SYNTHETIC_PRIVATE_REFERENCE_ALPHA",
            file_name: Some("attachment.txt"),
            file_path: Some("2023/validation/attachment.txt"),
        },
        FixtureRow {
            task_id: "safe-task-2",
            question: "SYNTHETIC_PRIVATE_QUESTION_BETA",
            level: 1,
            reference: "SYNTHETIC_PRIVATE_REFERENCE_BETA",
            file_name: None,
            file_path: None,
        },
    ]
}

fn write_fixture(root: &Path, revision: &str, rows: &[FixtureRow<'_>], omit: Option<&str>) {
    write_fixture_schema(
        root,
        revision,
        rows,
        FixtureSchema {
            omit,
            ..FixtureSchema::default()
        },
    );
}

fn write_fixture_schema(
    root: &Path,
    revision: &str,
    rows: &[FixtureRow<'_>],
    fixture_schema: FixtureSchema<'_>,
) {
    fs::create_dir_all(root.join("2023/validation")).unwrap();
    fs::write(root.join(GAIA_REVISION_MARKER), revision).unwrap();
    if rows.iter().any(|row| {
        row.file_path == Some("2023/validation/attachment.txt")
            && row.file_name == Some("attachment.txt")
    }) {
        fs::write(
            root.join("2023/validation/attachment.txt"),
            b"synthetic attachment",
        )
        .unwrap();
    }

    let wrong_optional = |name: &str| match fixture_schema.wrong_column {
        Some(column) if column == name && name == "file_name" => OptionalColumnShape::Binary,
        Some(column) if column == name && name == "file_path" => OptionalColumnShape::Int64,
        _ => OptionalColumnShape::Utf8,
    };
    let optional_array = |shape: OptionalColumnShape, values: Vec<Option<&str>>| -> ArrayRef {
        match shape {
            OptionalColumnShape::Utf8 => Arc::new(StringArray::from(values)),
            OptionalColumnShape::Binary => {
                Arc::new(BinaryArray::from(vec![None::<&[u8]>; values.len()]))
            }
            OptionalColumnShape::Int64 => {
                Arc::new(Int64Array::from(vec![None::<i64>; values.len()]))
            }
        }
    };
    let metadata: ArrayRef = match fixture_schema.metadata {
        MetadataShape::Struct => {
            let names = [
                "Steps",
                "Number of steps",
                "How long did this take?",
                "Tools",
                "Number of tools",
            ];
            let metadata_fields = Fields::from(
                names
                    .iter()
                    .map(|name| Field::new(*name, DataType::Utf8, true))
                    .collect::<Vec<_>>(),
            );
            Arc::new(StructArray::new(
                metadata_fields,
                names
                    .iter()
                    .map(|_| {
                        Arc::new(StringArray::from(vec![Some("synthetic"); rows.len()])) as ArrayRef
                    })
                    .collect(),
                None,
            ))
        }
        MetadataShape::Primitive => {
            Arc::new(StringArray::from(vec![Some("synthetic"); rows.len()]))
        }
        MetadataShape::List => Arc::new(ListArray::new_null(
            Arc::new(Field::new_list_field(DataType::Utf8, true)),
            rows.len(),
        )),
        MetadataShape::Map => {
            let mut builder = MapBuilder::new(None, StringBuilder::new(), StringBuilder::new());
            for _ in rows {
                builder.append(false).unwrap();
            }
            Arc::new(builder.finish())
        }
        MetadataShape::WrongChild => Arc::new(StructArray::new(
            Fields::from(vec![Field::new("bad", DataType::Int64, true)]),
            vec![Arc::new(Int64Array::from(vec![Some(1); rows.len()]))],
            None,
        )),
        MetadataShape::ManyChildren => {
            let fields = Fields::from(
                (0..17)
                    .map(|index| Field::new(format!("field-{index}"), DataType::Utf8, true))
                    .collect::<Vec<_>>(),
            );
            Arc::new(StructArray::new(
                fields,
                (0..17)
                    .map(|_| Arc::new(StringArray::from(vec![Some("x"); rows.len()])) as ArrayRef)
                    .collect(),
                None,
            ))
        }
        MetadataShape::Deep => {
            let leaf_fields = Fields::from(vec![Field::new("leaf", DataType::Utf8, true)]);
            let leaf = StructArray::new(
                leaf_fields.clone(),
                vec![Arc::new(StringArray::from(vec![Some("x"); rows.len()]))],
                None,
            );
            Arc::new(StructArray::new(
                Fields::from(vec![Field::new(
                    "nested",
                    DataType::Struct(leaf_fields),
                    true,
                )]),
                vec![Arc::new(leaf)],
                None,
            ))
        }
        MetadataShape::MissingField | MetadataShape::RenamedField | MetadataShape::ExtraField => {
            let mut names = vec![
                "Steps",
                "Number of steps",
                "How long did this take?",
                "Tools",
                "Number of tools",
            ];
            match fixture_schema.metadata {
                MetadataShape::MissingField => {
                    names.pop();
                }
                MetadataShape::RenamedField => names[0] = "Renamed Steps",
                MetadataShape::ExtraField => names.push("Unexpected"),
                _ => unreachable!(),
            }
            Arc::new(StructArray::new(
                Fields::from(
                    names
                        .iter()
                        .map(|name| Field::new(*name, DataType::Utf8, true))
                        .collect::<Vec<_>>(),
                ),
                names
                    .iter()
                    .map(|_| {
                        Arc::new(StringArray::from(vec![Some("synthetic"); rows.len()])) as ArrayRef
                    })
                    .collect(),
                None,
            ))
        }
    };
    let mut named_arrays: Vec<(&str, ArrayRef, bool)> = vec![
        (
            "task_id",
            if fixture_schema.wrong_column == Some("task_id") {
                Arc::new(Int64Array::from(vec![None::<i64>; rows.len()]))
            } else {
                Arc::new(StringArray::from(
                    rows.iter().map(|row| row.task_id).collect::<Vec<_>>(),
                ))
            },
            true,
        ),
        (
            "Question",
            if fixture_schema.wrong_column == Some("Question") {
                Arc::new(Int64Array::from(vec![None::<i64>; rows.len()]))
            } else {
                Arc::new(StringArray::from(
                    rows.iter().map(|row| row.question).collect::<Vec<_>>(),
                ))
            },
            true,
        ),
        (
            "Level",
            if fixture_schema.wrong_column == Some("Level") {
                Arc::new(StringArray::from(vec![None::<&str>; rows.len()]))
            } else {
                Arc::new(Int64Array::from(
                    rows.iter().map(|row| row.level).collect::<Vec<_>>(),
                ))
            },
            true,
        ),
        (
            "Final answer",
            if fixture_schema.wrong_column == Some("Final answer") {
                Arc::new(Int64Array::from(vec![None::<i64>; rows.len()]))
            } else {
                Arc::new(StringArray::from(
                    rows.iter().map(|row| row.reference).collect::<Vec<_>>(),
                ))
            },
            true,
        ),
        (
            "file_name",
            optional_array(
                wrong_optional("file_name"),
                rows.iter().map(|row| row.file_name).collect::<Vec<_>>(),
            ),
            true,
        ),
        (
            "file_path",
            optional_array(
                wrong_optional("file_path"),
                rows.iter().map(|row| row.file_path).collect::<Vec<_>>(),
            ),
            true,
        ),
        ("Annotator Metadata", metadata, true),
    ];
    named_arrays.retain(|(name, _, _)| Some(*name) != fixture_schema.omit);
    let schema = Arc::new(Schema::new(
        named_arrays
            .iter()
            .map(|(name, array, nullable)| {
                Field::new(
                    *name,
                    array.data_type().clone(),
                    *nullable && fixture_schema.required_column != Some(*name),
                )
            })
            .collect::<Vec<_>>(),
    ));
    let batch = RecordBatch::try_new(
        schema,
        named_arrays
            .into_iter()
            .map(|(_, array, _)| array)
            .collect(),
    )
    .unwrap();
    let file = fs::File::create(root.join(PARQUET_PATH)).unwrap();
    let properties = fixture_schema.max_row_group_size.map(|size| {
        WriterProperties::builder()
            .set_max_row_group_row_count(Some(size))
            .build()
    });
    let mut writer = ArrowWriter::try_new(file, batch.schema(), properties).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
}

fn error_code(root: &Path) -> String {
    verify_dataset(root).unwrap_err().to_string()
}

fn fixture_expectation(root: &Path) -> (u64, [u8; 32]) {
    let bytes = fs::read(root.join(PARQUET_PATH)).unwrap();
    (bytes.len() as u64, Sha256::digest(&bytes).into())
}

fn verify_dataset(root: &Path) -> Result<GaiaDataset, adapter_gaia::GaiaDatasetError> {
    let (size, digest) = fixture_expectation(root);
    GaiaDataset::verify_with_expected_parquet(root, size, digest)
}

struct AnswerRunner;

#[async_trait]
impl TaskRunner for AnswerRunner {
    async fn run_task(
        &self,
        task: &benchmark_core::BenchmarkTask,
        _context: &RunContext,
    ) -> benchmark_core::Result<TaskOutcome> {
        let answer = match task.task_id() {
            "safe-task-1" => "SYNTHETIC_PRIVATE_REFERENCE_ALPHA",
            "safe-task-2" => "PRIVATE_WRONG_CANDIDATE_SENTINEL",
            _ => "UNKNOWN_PRIVATE_SENTINEL",
        };
        Ok(
            TaskOutcome::new(task.task_id(), TaskStatus::Completed, None, vec![], 1)
                .with_private_output(SecretOutput::new(SecretText::new(answer))),
        )
    }
}

struct PartialAnswerRunner;

#[async_trait]
impl TaskRunner for PartialAnswerRunner {
    async fn run_task(
        &self,
        task: &benchmark_core::BenchmarkTask,
        _context: &RunContext,
    ) -> benchmark_core::Result<TaskOutcome> {
        if task.task_id() == "safe-task-1" {
            return Ok(
                TaskOutcome::new(task.task_id(), TaskStatus::Completed, None, vec![], 1)
                    .with_private_output(SecretOutput::new(SecretText::new(
                        "SYNTHETIC_PRIVATE_REFERENCE_ALPHA",
                    ))),
            );
        }
        Ok(TaskOutcome::new(
            task.task_id(),
            TaskStatus::Failed,
            None,
            vec![],
            1,
        ))
    }
}

fn completed_scoring_fixture() -> (TempSnapshot, GaiaAdapter, PathBuf, CompletedRun) {
    let snapshot = TempSnapshot::new();
    write_fixture(snapshot.path(), GAIA_DATASET_REVISION, &valid_rows(), None);
    let dataset = Arc::new(verify_dataset(snapshot.path()).unwrap());
    let adapter = GaiaAdapter::with_dataset(dataset);
    let verified = VerifiedDataset::new(GAIA_DATASET_REVISION, snapshot.path());
    let manifest = RunManifest::new(
        "gaia-score-run",
        adapter.descriptor(),
        Split::new(GAIA_SPLIT),
        ModelIdentity::new("synthetic", "model").unwrap(),
        ToolPolicyId::new("pinvou-gaia-public-web/v1"),
        1,
    )
    .unwrap();
    let base = snapshot.path().join("runtime");
    fs::create_dir(&base).unwrap();
    let service = BenchmarkService::with_runner(&base, Arc::new(AnswerRunner)).unwrap();
    futures::executor::block_on(service.run_adapter(
        manifest,
        &adapter,
        &verified,
        &TaskSelection::all(),
    ))
    .unwrap();
    let run = RunStore::open(&base, "gaia-score-run")
        .unwrap()
        .completed_run()
        .unwrap();
    (snapshot, adapter, base, run)
}

#[test]
fn scorer_returns_official_compatible_aggregate_only_for_complete_level1_coverage() {
    let (snapshot, adapter, base, run) = completed_scoring_fixture();

    let report = adapter.score(&run).unwrap();
    assert_eq!(report.evaluated(), 2);
    assert_eq!(report.correct(), 1);
    assert_eq!(report.split(), "validation");
    assert_eq!(report.level(), "1");
    assert_eq!(report.comparable_accuracy(), Some(0.5));
    let public = ["manifest.json", "events.jsonl", "predictions.jsonl"]
        .into_iter()
        .map(|name| fs::read_to_string(base.join("eval/runs/gaia-score-run").join(name)).unwrap())
        .collect::<String>();
    assert!(!public.contains("SYNTHETIC_PRIVATE_REFERENCE"));
    assert!(!public.contains("PRIVATE_WRONG_CANDIDATE_SENTINEL"));
    assert!(!format!("{adapter:?} {run:?} {report:?}").contains("SYNTHETIC_PRIVATE_REFERENCE"));
    assert!(
        !format!("{adapter:?} {run:?} {report:?}").contains("PRIVATE_WRONG_CANDIDATE_SENTINEL")
    );
    drop(snapshot);
}

#[test]
fn scorer_preserves_evaluated_and_correct_counts_for_partial_runs() {
    let snapshot = TempSnapshot::new();
    write_fixture(snapshot.path(), GAIA_DATASET_REVISION, &valid_rows(), None);
    let dataset = Arc::new(verify_dataset(snapshot.path()).unwrap());
    let adapter = GaiaAdapter::with_dataset(dataset);
    let verified = VerifiedDataset::new(GAIA_DATASET_REVISION, snapshot.path());
    let manifest = RunManifest::new(
        "gaia-partial-score-run",
        adapter.descriptor(),
        Split::new(GAIA_SPLIT),
        ModelIdentity::new("synthetic", "model").unwrap(),
        ToolPolicyId::new("pinvou-gaia-public-web/v1"),
        1,
    )
    .unwrap();
    let base = snapshot.path().join("partial-runtime");
    fs::create_dir(&base).unwrap();
    let service = BenchmarkService::with_runner(&base, Arc::new(PartialAnswerRunner)).unwrap();
    futures::executor::block_on(service.run_adapter(
        manifest,
        &adapter,
        &verified,
        &TaskSelection::all(),
    ))
    .unwrap();
    let run = RunStore::open(&base, "gaia-partial-score-run")
        .unwrap()
        .completed_run()
        .unwrap();

    let report = adapter.score(&run).unwrap();
    assert_eq!(report.evaluated(), 1);
    assert_eq!(report.correct(), 1);
    assert!(!report.is_complete());
    assert!(!report.is_official_dataset_compatible());
    assert_eq!(report.comparable_accuracy(), None);
}

#[test]
fn scorer_fails_closed_for_incomplete_unknown_duplicate_or_unavailable_predictions() {
    let (_snapshot, adapter, base, run) = completed_scoring_fixture();
    let partial = |candidate: &CompletedRun| {
        let report = adapter.score(candidate).unwrap();
        assert!(!report.is_complete());
        assert!(!report.is_official_dataset_compatible());
        assert_eq!(report.comparable_accuracy(), None);
    };

    let unbound_report = GaiaAdapter::new().score(&run).unwrap();
    assert_eq!(unbound_report.comparable_accuracy(), None);
    assert!(!unbound_report.is_official_dataset_compatible());
    partial(&CompletedRun::new("missing-coverage", vec![]));
    partial(&CompletedRun::new(
        "missing-capability",
        run.outcomes().to_vec(),
    ));
    partial(&CompletedRun::new(
        "duplicate",
        vec![run.outcomes()[0].clone(), run.outcomes()[0].clone()],
    ));
    partial(&CompletedRun::new(
        "unknown",
        vec![
            TaskOutcome::new("unknown-task", TaskStatus::Completed, None, vec![], 1),
            run.outcomes()[1].clone(),
        ],
    ));
    partial(&CompletedRun::new(
        "not-completed",
        vec![
            TaskOutcome::new("safe-task-1", TaskStatus::Failed, None, vec![], 1),
            run.outcomes()[1].clone(),
        ],
    ));

    let prediction_dir = base.join("eval/runs/gaia-score-run/private/predictions");
    let blob = fs::read_dir(&prediction_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let original_blob = fs::read(&blob).unwrap();
    fs::remove_file(&blob).unwrap();
    partial(&run);
    fs::write(&blob, original_blob).unwrap();
    fs::write(blob, b"CORRUPT_PRIVATE_SENTINEL").unwrap();
    let report = adapter.score(&run).unwrap();
    assert_eq!(report.comparable_accuracy(), None);
    assert!(!format!("{report:?}").contains("CORRUPT_PRIVATE_SENTINEL"));
}

#[test]
fn adapter_descriptor_and_native_turn_contract_are_exact() {
    let snapshot = TempSnapshot::new();
    write_fixture(snapshot.path(), GAIA_DATASET_REVISION, &valid_rows(), None);
    let verified = verify_dataset(snapshot.path()).unwrap();
    let adapter = GaiaAdapter::new();

    let descriptor = adapter.descriptor();
    assert_eq!(descriptor.id().as_str(), "gaia");
    assert_eq!(descriptor.adapter_version(), GAIA_ADAPTER_VERSION);
    assert_eq!(descriptor.dataset_revision(), GAIA_DATASET_REVISION);
    assert_eq!(descriptor.scorer_revision(), GAIA_SCORER_REVISION);
    assert_eq!(descriptor.supported_splits()[0].as_str(), GAIA_SPLIT);
    assert_eq!(descriptor.execution_kind(), ExecutionKind::NativeTurn);
    assert_eq!(
        adapter.private_output_retention(),
        PredictionRetention::DurableUntilPurge
    );
    assert_eq!(
        adapter.private_prediction_content_type(),
        PrivatePredictionContentType::Utf8TextV1
    );

    let plan = adapter.plan(&verified, &TaskSelection::all()).unwrap();
    assert_eq!(plan.tasks().len(), 2);
    let task = &plan.tasks()[0];
    assert_eq!(task.task_id(), "safe-task-1");
    assert_eq!(task.level(), Some("1"));
    let ExecutionRequest::NativeTurn {
        prompt_handle,
        attachments,
        timeout,
        tool_policy,
        output_contract,
    } = task.execution()
    else {
        panic!("GAIA must use NativeTurn");
    };
    assert_eq!(*timeout, Duration::from_secs(600));
    assert_eq!(tool_policy.as_str(), "pinvou-gaia-public-web/v1");
    assert_eq!(output_contract.as_str(), "gaia-final/v1");
    assert_eq!(attachments.len(), 1);
    assert_eq!(prompt_handle.expose_to_backend(), "gaia:safe-task-1:prompt");
    assert_eq!(
        attachments[0].expose_to_backend(),
        "gaia:safe-task-1:attachment"
    );
    assert_eq!(format!("{prompt_handle:?}"), "PrivateInputHandle([opaque])");
    assert_eq!(
        format!("{:?}", attachments[0]),
        "AttachmentHandle([opaque])"
    );
    assert!(!format!("{task:?}").contains("SYNTHETIC_PRIVATE_QUESTION"));
    assert!(!format!("{task:?}").contains("SYNTHETIC_PRIVATE_REFERENCE"));

    let prepared = adapter
        .prepare_task(task, &RunContext::new("run-1", snapshot.path().join("run")))
        .unwrap();
    assert_eq!(prepared.task().task_id(), task.task_id());
}

#[test]
fn adapter_selection_requires_exact_known_nonempty_task_ids() {
    let snapshot = TempSnapshot::new();
    write_fixture(snapshot.path(), GAIA_DATASET_REVISION, &valid_rows(), None);
    let verified = verify_dataset(snapshot.path()).unwrap();
    let adapter = GaiaAdapter::new();

    let selected = adapter
        .plan(
            &verified,
            &TaskSelection::from_task_ids(vec!["safe-task-2".into()]),
        )
        .unwrap();
    assert_eq!(selected.tasks().len(), 1);
    assert_eq!(selected.tasks()[0].task_id(), "safe-task-2");

    for requested in ["", "unknown-task", "safe-task-1 "] {
        let error = adapter
            .plan(
                &verified,
                &TaskSelection::from_task_ids(vec![requested.into()]),
            )
            .unwrap_err();
        assert_eq!(error.code(), "gaia_task_selection_invalid");
    }
}

#[test]
fn adapter_private_inputs_resolve_prompt_but_reject_raw_dataset_attachment_without_leaks() {
    let snapshot = TempSnapshot::new();
    write_fixture(snapshot.path(), GAIA_DATASET_REVISION, &valid_rows(), None);
    let verified = Arc::new(verify_dataset(snapshot.path()).unwrap());
    let inputs = GaiaPrivateInputs::new(verified);

    let resolved = inputs
        .resolve_handle(&PrivateInputHandle::new("gaia:safe-task-1:prompt"))
        .unwrap();
    let prompt = resolved.prompt().expose_to_backend();
    assert!(prompt.starts_with("SYNTHETIC_PRIVATE_QUESTION_ALPHA"));
    assert!(prompt.contains("FINAL ANSWER: <answer>"));
    assert_eq!(resolved.attachments().len(), 1);
    assert_eq!(
        resolved.attachments()[0].expose_to_backend(),
        "gaia:safe-task-1:attachment"
    );
    assert!(!format!("{inputs:?}").contains("SYNTHETIC_PRIVATE_QUESTION"));
    assert!(!format!("{resolved:?}").contains("SYNTHETIC_PRIVATE_QUESTION"));

    let error = inputs
        .resolve_attachment_handle(&AttachmentHandle::new("gaia:safe-task-1:attachment"))
        .unwrap_err();
    assert!(matches!(
        error,
        AgentBackendError::Operation(ref code) if code == "gaia_attachment_unsafe"
    ));
    assert!(!error.to_string().contains("safe-task-1"));
    assert!(!format!("{error:?}").contains("attachment.txt"));
}

#[test]
fn adapter_private_inputs_reject_unknown_handles_with_fixed_safe_codes() {
    let snapshot = TempSnapshot::new();
    write_fixture(snapshot.path(), GAIA_DATASET_REVISION, &valid_rows(), None);
    let inputs = GaiaPrivateInputs::new(Arc::new(verify_dataset(snapshot.path()).unwrap()));

    for handle in [
        "unknown",
        "gaia:unknown-task:prompt",
        "gaia:safe-task-1:reference",
    ] {
        let error = inputs
            .resolve_handle(&PrivateInputHandle::new(handle))
            .unwrap_err();
        assert!(matches!(
            error,
            AgentBackendError::Operation(ref code) if code == "gaia_private_input_unknown"
        ));
    }
    for handle in [
        "unknown",
        "gaia:unknown-task:attachment",
        "gaia:safe-task-2:attachment",
    ] {
        let error = inputs
            .resolve_attachment_handle(&AttachmentHandle::new(handle))
            .unwrap_err();
        assert!(matches!(
            error,
            AgentBackendError::Operation(ref code) if code == "gaia_attachment_handle_unknown"
        ));
    }
}

#[test]
fn dataset_constants_are_exact_and_parquet_59_supports_the_workspace_msrv() {
    assert_eq!(
        GAIA_DATASET_REVISION,
        "682dd723ee1e1697e00360edccf2366dc8418dd9"
    );
    assert_eq!(
        GAIA_SCORER_REVISION,
        "1349a17979f0aca0ee9c46cd7ec26eb2fb41102e"
    );
    assert_eq!(GAIA_ADAPTER_VERSION, "pinvou-gaia-adapter/v1");
    assert_eq!(GAIA_SPLIT, "validation");
    assert_eq!(GAIA_LEVEL, 1);
    assert_eq!(GAIA_PARQUET_SIZE, 39_524);
    assert_eq!(
        GAIA_PARQUET_SHA256,
        "5e574b0faeb4603b816e426cf7c7aefb1fe398d32f9c4861e1a4e3304f2b1281"
    );
}

#[test]
fn dataset_validates_two_synthetic_level1_rows_without_debug_leaks() {
    let snapshot = TempSnapshot::new();
    write_fixture(snapshot.path(), GAIA_DATASET_REVISION, &valid_rows(), None);

    let verified = verify_dataset(snapshot.path()).unwrap();
    assert_eq!(verified.rows().len(), 2);
    assert_eq!(verified.rows()[0].task_id(), "safe-task-1");
    assert_eq!(verified.rows()[0].level(), 1);
    assert!(
        verified.rows()[0]
            .attachment()
            .unwrap()
            .path()
            .starts_with(snapshot.path().canonicalize().unwrap())
    );
    let debug = format!("{verified:?}");
    assert!(!debug.contains("SYNTHETIC_PRIVATE_QUESTION"));
    assert!(!debug.contains("SYNTHETIC_PRIVATE_REFERENCE"));
}

#[test]
fn dataset_rejects_every_missing_exact_column() {
    for missing in EXPECTED_COLUMNS {
        let snapshot = TempSnapshot::new();
        write_fixture(
            snapshot.path(),
            GAIA_DATASET_REVISION,
            &valid_rows(),
            Some(missing),
        );
        assert_eq!(error_code(snapshot.path()), "gaia_schema_mismatch");
    }
}

#[test]
fn dataset_rejects_wrong_physical_or_logical_type_for_every_column() {
    for column in EXPECTED_COLUMNS
        .into_iter()
        .filter(|name| *name != "Annotator Metadata")
    {
        let snapshot = TempSnapshot::new();
        write_fixture_schema(
            snapshot.path(),
            GAIA_DATASET_REVISION,
            &valid_rows(),
            FixtureSchema {
                wrong_column: Some(column),
                ..FixtureSchema::default()
            },
        );
        assert_eq!(
            error_code(snapshot.path()),
            "gaia_schema_mismatch",
            "{column}"
        );
    }
}

#[test]
fn dataset_rejects_primitive_list_and_map_annotator_metadata_even_when_rows_are_null() {
    for metadata in [
        MetadataShape::Primitive,
        MetadataShape::List,
        MetadataShape::Map,
    ] {
        let snapshot = TempSnapshot::new();
        write_fixture_schema(
            snapshot.path(),
            GAIA_DATASET_REVISION,
            &valid_rows(),
            FixtureSchema {
                metadata,
                ..FixtureSchema::default()
            },
        );
        assert_eq!(error_code(snapshot.path()), "gaia_schema_mismatch");
    }
}

#[test]
fn dataset_rejects_wrong_repetition_and_unbounded_metadata_structs() {
    let mut required_rows = valid_rows();
    required_rows[1].file_name = Some("attachment.txt");
    required_rows[1].file_path = Some("2023/validation/attachment.txt");
    for required_column in EXPECTED_COLUMNS {
        let snapshot = TempSnapshot::new();
        write_fixture_schema(
            snapshot.path(),
            GAIA_DATASET_REVISION,
            &required_rows,
            FixtureSchema {
                required_column: Some(required_column),
                ..FixtureSchema::default()
            },
        );
        assert_eq!(error_code(snapshot.path()), "gaia_schema_mismatch");
    }
    for metadata in [
        MetadataShape::WrongChild,
        MetadataShape::ManyChildren,
        MetadataShape::Deep,
    ] {
        let snapshot = TempSnapshot::new();
        write_fixture_schema(
            snapshot.path(),
            GAIA_DATASET_REVISION,
            &valid_rows(),
            FixtureSchema {
                metadata,
                ..FixtureSchema::default()
            },
        );
        assert_eq!(error_code(snapshot.path()), "gaia_schema_mismatch");
    }
}

#[test]
fn dataset_requires_exact_annotator_metadata_field_names() {
    for metadata in [
        MetadataShape::MissingField,
        MetadataShape::RenamedField,
        MetadataShape::ExtraField,
    ] {
        let snapshot = TempSnapshot::new();
        write_fixture_schema(
            snapshot.path(),
            GAIA_DATASET_REVISION,
            &valid_rows(),
            FixtureSchema {
                metadata,
                ..FixtureSchema::default()
            },
        );
        assert_eq!(error_code(snapshot.path()), "gaia_schema_mismatch");
    }
}

#[test]
fn dataset_rejects_revision_mismatch_duplicate_ids_and_non_level1_rows() {
    let bad_revision = TempSnapshot::new();
    write_fixture(bad_revision.path(), "mutable-or-wrong", &valid_rows(), None);
    assert_eq!(error_code(bad_revision.path()), "gaia_revision_mismatch");

    let duplicate = TempSnapshot::new();
    let mut rows = valid_rows();
    rows[1].task_id = rows[0].task_id;
    write_fixture(duplicate.path(), GAIA_DATASET_REVISION, &rows, None);
    assert_eq!(error_code(duplicate.path()), "gaia_duplicate_task_id");

    let wrong_level = TempSnapshot::new();
    let mut rows = valid_rows();
    rows[1].level = 2;
    write_fixture(wrong_level.path(), GAIA_DATASET_REVISION, &rows, None);
    assert_eq!(error_code(wrong_level.path()), "gaia_level_mismatch");
}

#[test]
fn dataset_rejects_unsafe_ids_and_empty_private_fields() {
    for task_id in ["", ".", "..", "unsafe/id", "unsafe id"] {
        let snapshot = TempSnapshot::new();
        let mut rows = valid_rows();
        rows[0].task_id = task_id;
        write_fixture(snapshot.path(), GAIA_DATASET_REVISION, &rows, None);
        assert_eq!(error_code(snapshot.path()), "gaia_invalid_task_id");
    }

    for (question, reference) in [("", "reference"), ("question", "")] {
        let snapshot = TempSnapshot::new();
        let rows = vec![FixtureRow {
            task_id: "safe-task",
            question,
            level: 1,
            reference,
            file_name: None,
            file_path: None,
        }];
        write_fixture(snapshot.path(), GAIA_DATASET_REVISION, &rows, None);
        assert_eq!(error_code(snapshot.path()), "gaia_schema_mismatch");
    }
}

#[test]
fn dataset_rejects_missing_absolute_parent_directory_and_oversized_attachments() {
    let absolute = std::env::current_dir()
        .unwrap()
        .join("absolute.txt")
        .to_string_lossy()
        .into_owned();
    let cases = [
        (
            "missing.txt",
            "2023/validation/missing.txt",
            "gaia_attachment_missing",
        ),
        ("absolute.txt", absolute.as_str(), "gaia_attachment_unsafe"),
        ("escape.txt", "../escape.txt", "gaia_attachment_unsafe"),
    ];
    for (file_name, path, expected) in cases {
        let snapshot = TempSnapshot::new();
        let rows = vec![FixtureRow {
            task_id: "safe-task",
            question: "synthetic question",
            level: 1,
            reference: "synthetic reference",
            file_name: Some(file_name),
            file_path: Some(path),
        }];
        write_fixture(snapshot.path(), GAIA_DATASET_REVISION, &rows, None);
        assert_eq!(error_code(snapshot.path()), expected);
    }

    let directory = TempSnapshot::new();
    fs::create_dir_all(directory.path().join("2023/validation/a-directory")).unwrap();
    let rows = vec![FixtureRow {
        task_id: "safe-task",
        question: "synthetic question",
        level: 1,
        reference: "synthetic reference",
        file_name: Some("a-directory"),
        file_path: Some("2023/validation/a-directory"),
    }];
    write_fixture(directory.path(), GAIA_DATASET_REVISION, &rows, None);
    assert_eq!(error_code(directory.path()), "gaia_attachment_unsafe");

    let oversized = TempSnapshot::new();
    let oversized_path = oversized.path().join("2023/validation/oversized.bin");
    fs::create_dir_all(oversized_path.parent().unwrap()).unwrap();
    let file = fs::File::create(&oversized_path).unwrap();
    file.set_len(20 * 1024 * 1024 + 1).unwrap();
    let rows = vec![FixtureRow {
        task_id: "safe-task",
        question: "synthetic question",
        level: 1,
        reference: "synthetic reference",
        file_name: Some("oversized.bin"),
        file_path: Some("2023/validation/oversized.bin"),
    }];
    write_fixture(oversized.path(), GAIA_DATASET_REVISION, &rows, None);
    assert_eq!(error_code(oversized.path()), "gaia_attachment_too_large");
}

#[test]
fn dataset_allows_exactly_20_mib_and_rejects_one_byte_more() {
    for (size, expected) in [
        (20 * 1024 * 1024, None),
        (20 * 1024 * 1024 + 1, Some("gaia_attachment_too_large")),
    ] {
        let snapshot = TempSnapshot::new();
        let attachment = snapshot.path().join("2023/validation/boundary.bin");
        fs::create_dir_all(attachment.parent().unwrap()).unwrap();
        fs::File::create(&attachment)
            .unwrap()
            .set_len(size)
            .unwrap();
        let rows = vec![FixtureRow {
            task_id: "safe-task",
            question: "synthetic question",
            level: 1,
            reference: "synthetic reference",
            file_name: Some("boundary.bin"),
            file_path: Some("2023/validation/boundary.bin"),
        }];
        write_fixture(snapshot.path(), GAIA_DATASET_REVISION, &rows, None);
        match expected {
            Some(code) => assert_eq!(error_code(snapshot.path()), code),
            None => assert!(verify_dataset(snapshot.path()).is_ok()),
        }
    }
}

#[test]
fn dataset_attachment_reopen_rejects_same_size_path_replacement() {
    let snapshot = TempSnapshot::new();
    write_fixture(snapshot.path(), GAIA_DATASET_REVISION, &valid_rows(), None);
    let dataset = verify_dataset(snapshot.path()).unwrap();
    let attachment = dataset.rows()[0].attachment().unwrap().clone();
    let original = snapshot.path().join("2023/validation/attachment.txt");
    fs::rename(&original, snapshot.path().join("kept-original.txt")).unwrap();
    fs::write(&original, b"synthetic attachment").unwrap();
    assert_eq!(
        attachment.reopen_verified().unwrap_err().to_string(),
        "gaia_attachment_unsafe"
    );
}

#[test]
fn dataset_rejects_same_size_parquet_tampering_before_decode() {
    let snapshot = TempSnapshot::new();
    write_fixture(snapshot.path(), GAIA_DATASET_REVISION, &valid_rows(), None);
    let (expected_size, expected_digest) = fixture_expectation(snapshot.path());
    let path = snapshot.path().join(PARQUET_PATH);
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    file.seek(SeekFrom::Start(8)).unwrap();
    let mut byte = [0u8; 1];
    file.read_exact(&mut byte).unwrap();
    file.seek(SeekFrom::Start(8)).unwrap();
    byte[0] ^= 1;
    file.write_all(&byte).unwrap();
    file.sync_all().unwrap();
    assert_eq!(
        GaiaDataset::verify_with_expected_parquet(snapshot.path(), expected_size, expected_digest,)
            .unwrap_err()
            .to_string(),
        "gaia_schema_mismatch"
    );
}

#[test]
fn dataset_rejects_parquet_file_row_and_row_group_resource_exhaustion() {
    let oversized_file = TempSnapshot::new();
    write_fixture(
        oversized_file.path(),
        GAIA_DATASET_REVISION,
        &valid_rows(),
        None,
    );
    fs::OpenOptions::new()
        .write(true)
        .open(oversized_file.path().join(PARQUET_PATH))
        .unwrap()
        .set_len(16 * 1024 * 1024 + 1)
        .unwrap();
    assert_eq!(error_code(oversized_file.path()), "gaia_dataset_too_large");

    let too_many_rows = TempSnapshot::new();
    let rows = vec![valid_rows()[0].clone(); 129];
    write_fixture(too_many_rows.path(), GAIA_DATASET_REVISION, &rows, None);
    assert_eq!(error_code(too_many_rows.path()), "gaia_dataset_too_large");

    let too_many_groups = TempSnapshot::new();
    let rows = vec![valid_rows()[0].clone(); 17];
    write_fixture_schema(
        too_many_groups.path(),
        GAIA_DATASET_REVISION,
        &rows,
        FixtureSchema {
            max_row_group_size: Some(1),
            ..FixtureSchema::default()
        },
    );
    assert_eq!(error_code(too_many_groups.path()), "gaia_dataset_too_large");
}

#[test]
fn dataset_rejects_oversized_private_and_path_fields() {
    let cases = [
        ("task", "x".repeat(129), "q".into(), "r".into(), None, None),
        (
            "question",
            "id".into(),
            "q".repeat(64 * 1024 + 1),
            "r".into(),
            None,
            None,
        ),
        (
            "reference",
            "id".into(),
            "q".into(),
            "r".repeat(8 * 1024 + 1),
            None,
            None,
        ),
        (
            "file_name",
            "id".into(),
            "q".into(),
            "r".into(),
            Some("x".repeat(256)),
            Some("x".repeat(256)),
        ),
        (
            "file_path",
            "id".into(),
            "q".into(),
            "r".into(),
            Some("x".into()),
            Some(format!("{}/x", "a/".repeat(512))),
        ),
    ];
    for (label, task_id, question, reference, file_name, file_path) in cases {
        let snapshot = TempSnapshot::new();
        let row = FixtureRow {
            task_id: Box::leak(task_id.into_boxed_str()),
            question: Box::leak(question.into_boxed_str()),
            level: 1,
            reference: Box::leak(reference.into_boxed_str()),
            file_name: file_name.map(|value| &*Box::leak(value.into_boxed_str())),
            file_path: file_path.map(|value| &*Box::leak(value.into_boxed_str())),
        };
        write_fixture_schema(
            snapshot.path(),
            GAIA_DATASET_REVISION,
            &[row],
            FixtureSchema::default(),
        );
        assert_eq!(
            error_code(snapshot.path()),
            "gaia_schema_mismatch",
            "{label}"
        );
    }
}

#[cfg(any(unix, windows))]
#[test]
fn dataset_rejects_symlink_attachments() {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    #[cfg(windows)]
    use std::os::windows::fs::symlink_file as symlink;

    let snapshot = TempSnapshot::new();
    fs::create_dir_all(snapshot.path().join("2023/validation")).unwrap();
    fs::write(snapshot.path().join("target.txt"), b"synthetic").unwrap();
    if let Err(error) = symlink(
        snapshot.path().join("target.txt"),
        snapshot.path().join("2023/validation/link.txt"),
    ) {
        #[cfg(windows)]
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            panic!("Windows symlink privilege required for security contract: {error}");
        }
        panic!("create test symlink: {error}");
    }
    let rows = vec![FixtureRow {
        task_id: "safe-task",
        question: "synthetic question",
        level: 1,
        reference: "synthetic reference",
        file_name: Some("link.txt"),
        file_path: Some("2023/validation/link.txt"),
    }];
    write_fixture(snapshot.path(), GAIA_DATASET_REVISION, &rows, None);
    assert_eq!(error_code(snapshot.path()), "gaia_attachment_unsafe");
}

#[cfg(any(unix, windows))]
#[test]
fn dataset_rejects_intermediate_directory_symlink_or_reparse_point() {
    #[cfg(unix)]
    use std::os::unix::fs::symlink as symlink_dir;
    #[cfg(windows)]
    use std::os::windows::fs::symlink_dir;

    let snapshot = TempSnapshot::new();
    let target = snapshot.path().join("real-directory");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("attachment.txt"), b"synthetic").unwrap();
    fs::create_dir_all(snapshot.path().join("2023/validation")).unwrap();
    if let Err(error) = symlink_dir(
        &target,
        snapshot.path().join("2023/validation/linked-directory"),
    ) {
        #[cfg(windows)]
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            panic!("Windows directory symlink privilege required for security contract: {error}");
        }
        panic!("create test directory link: {error}");
    }
    let rows = vec![FixtureRow {
        task_id: "safe-task",
        question: "synthetic question",
        level: 1,
        reference: "synthetic reference",
        file_name: Some("attachment.txt"),
        file_path: Some("2023/validation/linked-directory/attachment.txt"),
    }];
    write_fixture(snapshot.path(), GAIA_DATASET_REVISION, &rows, None);
    assert_eq!(error_code(snapshot.path()), "gaia_attachment_unsafe");
}

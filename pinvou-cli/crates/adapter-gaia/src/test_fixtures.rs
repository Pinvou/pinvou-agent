//! Synthetic GAIA parquet snapshot fixtures shared by the in-crate unit tests
//! and the `test-support`-gated integration tests. Compiled only under
//! `cfg(test)` or the `test-support` feature; never part of a normal build.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use arrow_array::builder::{MapBuilder, StringBuilder};
use arrow_array::{
    Array, ArrayRef, BinaryArray, Int64Array, ListArray, RecordBatch, StringArray, StructArray,
};
use arrow_schema::{DataType, Field, Fields, Schema};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

use crate::{GAIA_PARQUET_PATH, GAIA_REVISION_MARKER};

pub const ATTACHMENT_PATH: &str = "2023/validation/attachment.txt";
pub const ATTACHMENT_BYTES: &[u8] = b"synthetic attachment";

/// The pinned parquet path, re-exported for the `test-support`-gated
/// integration tests (the crate const itself is `pub(crate)`; a const copy
/// needs no visibility widening).
pub const GAIA_PARQUET_RELATIVE_PATH: &str = GAIA_PARQUET_PATH;

#[derive(Clone)]
pub struct FixtureRow<'a> {
    pub task_id: &'a str,
    pub question: &'a str,
    pub level: i64,
    pub reference: &'a str,
    pub file_name: Option<&'a str>,
    pub file_path: Option<&'a str>,
}

#[derive(Clone, Copy, Default)]
pub enum MetadataShape {
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
pub enum OptionalColumnShape {
    #[default]
    Utf8,
    Binary,
    Int64,
}

#[derive(Clone)]
pub struct FixtureSchema<'a> {
    pub omit: Option<&'a str>,
    pub wrong_column: Option<&'a str>,
    pub metadata: MetadataShape,
    pub max_row_group_size: Option<usize>,
    pub required_column: Option<&'a str>,
    /// Replaces the `Level` column with an arbitrary array so schema tests can
    /// feed non-Int64 physical types.
    pub level_override: Option<ArrayRef>,
    /// Bytes written to [`ATTACHMENT_PATH`] when a row references it; `None`
    /// skips the attachment file entirely.
    pub attachment_bytes: Option<&'a [u8]>,
}

impl Default for FixtureSchema<'_> {
    fn default() -> Self {
        Self {
            omit: None,
            wrong_column: None,
            metadata: MetadataShape::Struct,
            max_row_group_size: None,
            required_column: None,
            level_override: None,
            attachment_bytes: Some(ATTACHMENT_BYTES),
        }
    }
}

pub fn valid_rows() -> Vec<FixtureRow<'static>> {
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

/// Writes the complete snapshot layout: validation directory, revision marker,
/// referenced attachment, and the pinned parquet at [`GAIA_PARQUET_PATH`].
pub fn write_fixture(root: &Path, revision: &str, rows: &[FixtureRow<'_>], omit: Option<&str>) {
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

pub fn write_fixture_schema(
    root: &Path,
    revision: &str,
    rows: &[FixtureRow<'_>],
    fixture_schema: FixtureSchema<'_>,
) {
    fs::create_dir_all(root.join("2023/validation")).unwrap();
    fs::write(root.join(GAIA_REVISION_MARKER), revision).unwrap();
    if let Some(attachment_bytes) = fixture_schema.attachment_bytes
        && rows.iter().any(|row| {
            row.file_path == Some(ATTACHMENT_PATH) && row.file_name == Some("attachment.txt")
        })
    {
        fs::write(root.join(ATTACHMENT_PATH), attachment_bytes).unwrap();
    }
    let batch = snapshot_batch(rows, &fixture_schema);
    write_parquet(
        &root.join(GAIA_PARQUET_PATH),
        &batch,
        fixture_schema.max_row_group_size,
    );
}

/// Builds the exact seven-column GAIA batch (metadata shape, wrong physical
/// types, omitted and required columns included).
pub fn snapshot_batch(rows: &[FixtureRow<'_>], fixture_schema: &FixtureSchema<'_>) -> RecordBatch {
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
        MetadataShape::Struct => annotator_metadata(rows.len(), "synthetic"),
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
            annotator_metadata_from_names(&names, rows.len(), "synthetic")
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
            match &fixture_schema.level_override {
                Some(level) => level.clone(),
                None if fixture_schema.wrong_column == Some("Level") => {
                    Arc::new(StringArray::from(vec![None::<&str>; rows.len()]))
                }
                None => Arc::new(Int64Array::from(
                    rows.iter().map(|row| row.level).collect::<Vec<_>>(),
                )),
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
    RecordBatch::try_new(
        schema,
        named_arrays
            .into_iter()
            .map(|(_, array, _)| array)
            .collect(),
    )
    .unwrap()
}

/// The five-field "Annotator Metadata" struct the pinned GAIA parquet carries.
pub fn annotator_metadata(rows: usize, value: &str) -> ArrayRef {
    annotator_metadata_from_names(
        &[
            "Steps",
            "Number of steps",
            "How long did this take?",
            "Tools",
            "Number of tools",
        ],
        rows,
        value,
    )
}

fn annotator_metadata_from_names(names: &[&str], rows: usize, value: &str) -> ArrayRef {
    Arc::new(StructArray::new(
        Fields::from(
            names
                .iter()
                .map(|name| Field::new(*name, DataType::Utf8, true))
                .collect::<Vec<_>>(),
        ),
        names
            .iter()
            .map(|_| Arc::new(StringArray::from(vec![Some(value); rows])) as ArrayRef)
            .collect(),
        None,
    ))
}

/// ArrowWriter loop shared by every fixture writer.
pub fn write_parquet(path: &Path, batch: &RecordBatch, max_row_group_size: Option<usize>) {
    let file = fs::File::create(path).unwrap();
    let properties = max_row_group_size.map(|size| {
        WriterProperties::builder()
            .set_max_row_group_row_count(Some(size))
            .build()
    });
    let mut writer = ArrowWriter::try_new(file, batch.schema(), properties).unwrap();
    writer.write(batch).unwrap();
    writer.close().unwrap();
}

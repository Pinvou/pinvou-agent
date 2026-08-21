use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs::{self, File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

use agent_backend_api::{ResolvedAttachmentSource, SecretText};
use parquet::basic::{ConvertedType, LogicalType, Repetition, Type as PhysicalType};
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::record::Field;
use parquet::schema::types::Type;
use sha2::{Digest, Sha256};

use crate::{GAIA_DATASET_REVISION, GAIA_LEVEL, GAIA_PARQUET_SIZE};

pub const GAIA_REVISION_MARKER: &str = ".pinvou-gaia-dataset-revision-v1";
const GAIA_PARQUET_PATH: &str = "2023/validation/metadata.level1.parquet";
const MAX_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;
const MAX_PARQUET_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PARQUET_ROWS: usize = 128;
const MAX_ROW_GROUPS: usize = 16;
const MAX_UNCOMPRESSED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SCHEMA_DEPTH: usize = 3;
const MAX_SCHEMA_NODES: usize = 32;
const MAX_METADATA_LEAVES: usize = 16;
const MAX_QUESTION_BYTES: usize = 64 * 1024;
const MAX_REFERENCE_BYTES: usize = 8 * 1024;
const MAX_FILE_NAME_BYTES: usize = 255;
const MAX_FILE_PATH_BYTES: usize = 1024;
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
pub struct GaiaRow {
    task_id: String,
    question: SecretText,
    reference: SecretText,
    attachment: Option<GaiaAttachment>,
    level: u8,
}

impl GaiaRow {
    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    pub fn level(&self) -> u8 {
        self.level
    }

    pub fn attachment(&self) -> Option<&GaiaAttachment> {
        self.attachment.as_ref()
    }

    #[allow(dead_code)]
    pub(crate) fn question(&self) -> &SecretText {
        &self.question
    }

    #[allow(dead_code)]
    pub(crate) fn reference(&self) -> &SecretText {
        &self.reference
    }
}

#[derive(Clone)]
pub struct GaiaAttachment {
    snapshot_root: PathBuf,
    path: PathBuf,
    identity: FileIdentity,
    size: u64,
    expected_sha256: Option<[u8; 32]>,
}

impl GaiaAttachment {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    /// Reopens the verified snapshot file and repeats containment, reparse-point,
    /// size, and platform file-identity checks before handing a live handle to
    /// the runtime. Windows attachments remain gated by these runtime checks.
    pub fn reopen_verified(&self) -> Result<File, GaiaDatasetError> {
        let relative = self
            .path
            .strip_prefix(&self.snapshot_root)
            .map_err(|_| GaiaDatasetError::AttachmentUnsafe)?;
        ensure_no_link_components(&self.snapshot_root, relative)?;
        let canonical = self
            .path
            .canonicalize()
            .map_err(|_| GaiaDatasetError::AttachmentUnsafe)?;
        if canonical != self.path || !canonical.starts_with(&self.snapshot_root) {
            return Err(GaiaDatasetError::AttachmentUnsafe);
        }
        let mut file = File::open(&self.path).map_err(|_| GaiaDatasetError::AttachmentUnsafe)?;
        let metadata = file
            .metadata()
            .map_err(|_| GaiaDatasetError::AttachmentUnsafe)?;
        if !metadata.is_file()
            || is_link_or_reparse(&metadata)
            || !same_file_snapshot(
                self.identity,
                self.size,
                file_identity(&file, &metadata)?,
                metadata.len(),
            )
        {
            return Err(GaiaDatasetError::AttachmentUnsafe);
        }
        if let Some(expected_sha256) = self.expected_sha256 {
            let mut hasher = Sha256::new();
            let copied = std::io::copy(
                &mut std::io::Read::by_ref(&mut file).take(MAX_ATTACHMENT_BYTES + 1),
                &mut hasher,
            )
            .map_err(|_| GaiaDatasetError::AttachmentUnsafe)?;
            if copied != self.size || <[u8; 32]>::from(hasher.finalize()) != expected_sha256 {
                return Err(GaiaDatasetError::AttachmentUnsafe);
            }
            file.seek(SeekFrom::Start(0))
                .map_err(|_| GaiaDatasetError::AttachmentUnsafe)?;
        }
        Ok(file)
    }

    pub(crate) fn verify_immutable_source(
        &self,
        source: &ResolvedAttachmentSource,
    ) -> Result<(), GaiaDatasetError> {
        let expected_sha256 = self
            .expected_sha256
            .ok_or(GaiaDatasetError::AttachmentUnsafe)?;
        if source.local_path() != self.path
            || source
                .verified_file_size()
                .map_err(|_| GaiaDatasetError::AttachmentUnsafe)?
                != Some(self.size)
        {
            return Err(GaiaDatasetError::AttachmentUnsafe);
        }
        let verified = source
            .try_read_verified_file(|reader| {
                let mut hasher = Sha256::new();
                let copied =
                    std::io::copy(&mut reader.take(MAX_ATTACHMENT_BYTES + 1), &mut hasher)?;
                Ok((copied, <[u8; 32]>::from(hasher.finalize())))
            })
            .map_err(|_| GaiaDatasetError::AttachmentUnsafe)?
            .ok_or(GaiaDatasetError::AttachmentUnsafe)?;
        if verified.0 != self.size || verified.1 != expected_sha256 {
            return Err(GaiaDatasetError::AttachmentUnsafe);
        }
        Ok(())
    }
}

impl fmt::Debug for GaiaAttachment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GaiaAttachment([redacted])")
    }
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileIdentity {
    volume: u32,
    index: u64,
}

#[cfg(not(any(unix, windows)))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileIdentity;

impl FileIdentity {
    #[cfg(unix)]
    fn is_valid(self) -> bool {
        self.inode != 0
    }

    #[cfg(windows)]
    fn is_valid(self) -> bool {
        self.index != 0
    }

    #[cfg(not(any(unix, windows)))]
    fn is_valid(self) -> bool {
        true
    }
}

fn same_file_snapshot(
    expected_identity: FileIdentity,
    expected_size: u64,
    actual_identity: FileIdentity,
    actual_size: u64,
) -> bool {
    expected_identity.is_valid()
        && actual_identity.is_valid()
        && expected_identity == actual_identity
        && expected_size == actual_size
}

#[cfg(unix)]
fn file_identity(_file: &File, metadata: &Metadata) -> Result<FileIdentity, GaiaDatasetError> {
    use std::os::unix::fs::MetadataExt;

    Ok(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(windows)]
fn file_identity(file: &File, _metadata: &Metadata) -> Result<FileIdentity, GaiaDatasetError> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    let succeeded =
        unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, information.as_mut_ptr()) };
    if succeeded == 0 {
        return Err(GaiaDatasetError::AttachmentUnsafe);
    }
    let information = unsafe { information.assume_init() };
    Ok(FileIdentity {
        volume: information.dwVolumeSerialNumber,
        index: (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    })
}

#[cfg(not(any(unix, windows)))]
fn file_identity(_file: &File, _metadata: &Metadata) -> Result<FileIdentity, GaiaDatasetError> {
    Ok(FileIdentity)
}

impl fmt::Debug for GaiaRow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GaiaRow([redacted])")
    }
}

#[derive(Clone)]
pub struct GaiaDataset {
    snapshot_root: PathBuf,
    rows: Vec<GaiaRow>,
}

impl GaiaDataset {
    pub fn verify(snapshot_root: &Path) -> Result<Self, GaiaDatasetError> {
        Self::verify_expected(
            snapshot_root,
            GAIA_PARQUET_SIZE,
            [
                0x5e, 0x57, 0x4b, 0x0f, 0xae, 0xb4, 0x60, 0x3b, 0x81, 0x6e, 0x42, 0x6c, 0xf7, 0xc7,
                0xae, 0xfb, 0x1f, 0xe3, 0x98, 0xd3, 0x2f, 0x9c, 0x48, 0x61, 0xe1, 0xa4, 0xe3, 0x30,
                0x4f, 0x2b, 0x12, 0x81,
            ],
        )
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn verify_with_expected_parquet(
        snapshot_root: &Path,
        expected_size: u64,
        expected_sha256: [u8; 32],
    ) -> Result<Self, GaiaDatasetError> {
        Self::verify_expected(snapshot_root, expected_size, expected_sha256)
    }

    #[cfg(test)]
    pub(crate) fn verify_with_expected_parquet_for_tests(
        snapshot_root: &Path,
        expected_size: u64,
        expected_sha256: [u8; 32],
    ) -> Result<Self, GaiaDatasetError> {
        Self::verify_expected(snapshot_root, expected_size, expected_sha256)
    }

    fn verify_expected(
        snapshot_root: &Path,
        expected_size: u64,
        expected_sha256: [u8; 32],
    ) -> Result<Self, GaiaDatasetError> {
        let root_metadata =
            fs::symlink_metadata(snapshot_root).map_err(|_| GaiaDatasetError::RevisionMismatch)?;
        if !root_metadata.is_dir() || is_link_or_reparse(&root_metadata) {
            return Err(GaiaDatasetError::RevisionMismatch);
        }
        let canonical_root = snapshot_root
            .canonicalize()
            .map_err(|_| GaiaDatasetError::RevisionMismatch)?;
        verify_revision(&canonical_root)?;

        let parquet_relative = Path::new(GAIA_PARQUET_PATH);
        ensure_no_link_components(&canonical_root, parquet_relative)
            .map_err(|_| GaiaDatasetError::SchemaMismatch)?;
        let (mut parquet_file, parquet_metadata) =
            open_verified_snapshot_file(&canonical_root, parquet_relative)?;
        if parquet_metadata.len() > MAX_PARQUET_BYTES {
            return Err(GaiaDatasetError::DatasetTooLarge);
        }
        verify_parquet_digest(
            &mut parquet_file,
            &parquet_metadata,
            expected_size,
            expected_sha256,
        )?;
        let reader = SerializedFileReader::new(parquet_file)
            .map_err(|_| GaiaDatasetError::SchemaMismatch)?;
        verify_resource_budget(&reader)?;
        verify_schema(&reader)?;

        let mut rows = Vec::new();
        let mut task_ids = HashSet::new();
        let row_iter = reader
            .get_row_iter(None)
            .map_err(|_| GaiaDatasetError::SchemaMismatch)?;
        for row in row_iter {
            if rows.len() >= MAX_PARQUET_ROWS {
                return Err(GaiaDatasetError::DatasetTooLarge);
            }
            let row = row.map_err(|_| GaiaDatasetError::SchemaMismatch)?;
            let mut columns = row.into_columns();
            let task_id = take_string(&mut columns, "task_id")?;
            if task_id.len() > 128 {
                return Err(GaiaDatasetError::SchemaMismatch);
            }
            if !safe_task_id(&task_id) {
                return Err(GaiaDatasetError::InvalidTaskId);
            }
            if !task_ids.insert(task_id.clone()) {
                return Err(GaiaDatasetError::DuplicateTaskId);
            }

            let question = take_string(&mut columns, "Question")?;
            let reference = take_string(&mut columns, "Final answer")?;
            if question.trim().is_empty()
                || reference.trim().is_empty()
                || question.len() > MAX_QUESTION_BYTES
                || reference.len() > MAX_REFERENCE_BYTES
            {
                return Err(GaiaDatasetError::SchemaMismatch);
            }
            let level = take_level(&mut columns)?;
            if level != GAIA_LEVEL {
                return Err(GaiaDatasetError::LevelMismatch);
            }
            let file_name = take_optional_string(&mut columns, "file_name")?;
            let file_path = take_optional_string(&mut columns, "file_path")?;
            if file_name
                .as_ref()
                .is_some_and(|value| value.len() > MAX_FILE_NAME_BYTES)
                || file_path
                    .as_ref()
                    .is_some_and(|value| value.len() > MAX_FILE_PATH_BYTES)
            {
                return Err(GaiaDatasetError::SchemaMismatch);
            }
            take_metadata(&mut columns)?;
            if !columns.is_empty() {
                return Err(GaiaDatasetError::SchemaMismatch);
            }
            let attachment =
                verify_attachment(&canonical_root, file_name.as_deref(), file_path.as_deref())?;
            rows.push(GaiaRow {
                task_id,
                question: SecretText::new(question),
                reference: SecretText::new(reference),
                attachment,
                level,
            });
        }
        if rows.is_empty() {
            return Err(GaiaDatasetError::SchemaMismatch);
        }
        Ok(Self {
            snapshot_root: canonical_root,
            rows,
        })
    }

    pub fn rows(&self) -> &[GaiaRow] {
        &self.rows
    }

    pub fn snapshot_root(&self) -> &Path {
        &self.snapshot_root
    }

    pub(crate) fn bind_attachment_sha256(
        &mut self,
        digests: &BTreeMap<PathBuf, [u8; 32]>,
    ) -> Result<(), GaiaDatasetError> {
        let mut bound = HashSet::new();
        for row in &mut self.rows {
            if let Some(attachment) = &mut row.attachment {
                let relative = attachment
                    .path
                    .strip_prefix(&self.snapshot_root)
                    .map_err(|_| GaiaDatasetError::AttachmentUnsafe)?
                    .to_path_buf();
                let digest = digests
                    .get(&relative)
                    .ok_or(GaiaDatasetError::AttachmentUnsafe)?;
                attachment.expected_sha256 = Some(*digest);
                bound.insert(relative);
            }
        }
        if bound.len() != digests.len() {
            return Err(GaiaDatasetError::AttachmentUnsafe);
        }
        Ok(())
    }
}

impl fmt::Debug for GaiaDataset {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GaiaDataset")
            .field("snapshot_root", &"[redacted]")
            .field("rows", &self.rows)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GaiaDatasetError {
    RevisionMismatch,
    SchemaMismatch,
    InvalidTaskId,
    DuplicateTaskId,
    LevelMismatch,
    AttachmentMissing,
    AttachmentUnsafe,
    AttachmentTooLarge,
    DatasetTooLarge,
}

impl GaiaDatasetError {
    pub fn code(self) -> &'static str {
        match self {
            Self::RevisionMismatch => "gaia_revision_mismatch",
            Self::SchemaMismatch => "gaia_schema_mismatch",
            Self::InvalidTaskId => "gaia_invalid_task_id",
            Self::DuplicateTaskId => "gaia_duplicate_task_id",
            Self::LevelMismatch => "gaia_level_mismatch",
            Self::AttachmentMissing => "gaia_attachment_missing",
            Self::AttachmentUnsafe => "gaia_attachment_unsafe",
            Self::AttachmentTooLarge => "gaia_attachment_too_large",
            Self::DatasetTooLarge => "gaia_dataset_too_large",
        }
    }
}

impl fmt::Display for GaiaDatasetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for GaiaDatasetError {}

fn verify_revision(root: &Path) -> Result<(), GaiaDatasetError> {
    let marker = root.join(GAIA_REVISION_MARKER);
    let metadata = fs::symlink_metadata(&marker).map_err(|_| GaiaDatasetError::RevisionMismatch)?;
    if !metadata.is_file() || is_link_or_reparse(&metadata) || metadata.len() > 128 {
        return Err(GaiaDatasetError::RevisionMismatch);
    }
    let revision = fs::read_to_string(marker).map_err(|_| GaiaDatasetError::RevisionMismatch)?;
    if revision.trim() != GAIA_DATASET_REVISION {
        return Err(GaiaDatasetError::RevisionMismatch);
    }
    Ok(())
}

pub(crate) fn open_verified_snapshot_file(
    root: &Path,
    relative: &Path,
) -> Result<(File, Metadata), GaiaDatasetError> {
    ensure_no_link_components(root, relative).map_err(|_| GaiaDatasetError::SchemaMismatch)?;
    let path = root.join(relative);
    let path_metadata =
        fs::symlink_metadata(&path).map_err(|_| GaiaDatasetError::SchemaMismatch)?;
    if !path_metadata.is_file() || is_link_or_reparse(&path_metadata) {
        return Err(GaiaDatasetError::SchemaMismatch);
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| GaiaDatasetError::SchemaMismatch)?;
    if !canonical.starts_with(root) {
        return Err(GaiaDatasetError::SchemaMismatch);
    }
    let file = File::open(&path).map_err(|_| GaiaDatasetError::SchemaMismatch)?;
    let metadata = file
        .metadata()
        .map_err(|_| GaiaDatasetError::SchemaMismatch)?;
    let path_handle = File::open(&canonical).map_err(|_| GaiaDatasetError::SchemaMismatch)?;
    let path_handle_metadata = path_handle
        .metadata()
        .map_err(|_| GaiaDatasetError::SchemaMismatch)?;
    let opened_identity =
        file_identity(&file, &metadata).map_err(|_| GaiaDatasetError::SchemaMismatch)?;
    let path_identity = file_identity(&path_handle, &path_handle_metadata)
        .map_err(|_| GaiaDatasetError::SchemaMismatch)?;
    if !metadata.is_file()
        || !same_file_snapshot(
            opened_identity,
            metadata.len(),
            path_identity,
            path_handle_metadata.len(),
        )
    {
        return Err(GaiaDatasetError::SchemaMismatch);
    }
    Ok((file, metadata))
}

fn verify_parquet_digest(
    file: &mut File,
    metadata: &Metadata,
    expected_size: u64,
    expected_sha256: [u8; 32],
) -> Result<(), GaiaDatasetError> {
    if metadata.len() != expected_size {
        return Err(GaiaDatasetError::SchemaMismatch);
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| GaiaDatasetError::SchemaMismatch)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| GaiaDatasetError::SchemaMismatch)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or(GaiaDatasetError::DatasetTooLarge)?;
        if total > expected_size || total > MAX_PARQUET_BYTES {
            return Err(GaiaDatasetError::DatasetTooLarge);
        }
        hasher.update(&buffer[..read]);
    }
    let digest: [u8; 32] = hasher.finalize().into();
    if total != expected_size || digest != expected_sha256 {
        return Err(GaiaDatasetError::SchemaMismatch);
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| GaiaDatasetError::SchemaMismatch)?;
    Ok(())
}

fn verify_schema(reader: &SerializedFileReader<File>) -> Result<(), GaiaDatasetError> {
    let fields = reader
        .metadata()
        .file_metadata()
        .schema_descr()
        .root_schema()
        .get_fields();
    if fields.len() != EXPECTED_COLUMNS.len() {
        return Err(GaiaDatasetError::SchemaMismatch);
    }
    for name in [
        "task_id",
        "Question",
        "Final answer",
        "file_name",
        "file_path",
    ] {
        let field = schema_field(fields, name)?;
        if !is_optional(field) || !is_utf8_primitive(field) {
            return Err(GaiaDatasetError::SchemaMismatch);
        }
    }
    let level = schema_field(fields, "Level")?;
    if !level.is_primitive() || !is_optional(level) {
        return Err(GaiaDatasetError::SchemaMismatch);
    }
    let level_info = level.get_basic_info();
    let valid_int64 = level.get_physical_type() == PhysicalType::INT64
        && level_info.logical_type_ref().is_none()
        && level_info.converted_type() == ConvertedType::NONE;
    let valid_utf8 = is_utf8_primitive(level);
    if !(valid_int64 || valid_utf8) {
        return Err(GaiaDatasetError::SchemaMismatch);
    }
    let metadata = schema_field(fields, "Annotator Metadata")?;
    if !metadata.is_group()
        || !is_optional(metadata)
        || metadata.get_basic_info().logical_type_ref().is_some()
        || metadata.get_basic_info().converted_type() != ConvertedType::NONE
    {
        return Err(GaiaDatasetError::SchemaMismatch);
    }
    validate_metadata_children(metadata)?;
    Ok(())
}

fn verify_resource_budget(reader: &SerializedFileReader<File>) -> Result<(), GaiaDatasetError> {
    let metadata = reader.metadata();
    let num_rows = usize::try_from(metadata.file_metadata().num_rows())
        .map_err(|_| GaiaDatasetError::DatasetTooLarge)?;
    if num_rows == 0
        || num_rows > MAX_PARQUET_ROWS
        || metadata.num_row_groups() > MAX_ROW_GROUPS
        || metadata.file_metadata().schema_descr().num_columns() > MAX_SCHEMA_NODES
    {
        return Err(GaiaDatasetError::DatasetTooLarge);
    }
    let total_uncompressed = metadata
        .row_groups()
        .iter()
        .try_fold(0u64, |total, group| {
            let bytes = u64::try_from(group.total_byte_size())
                .map_err(|_| GaiaDatasetError::DatasetTooLarge)?;
            total
                .checked_add(bytes)
                .ok_or(GaiaDatasetError::DatasetTooLarge)
        })?;
    if total_uncompressed > MAX_UNCOMPRESSED_BYTES {
        return Err(GaiaDatasetError::DatasetTooLarge);
    }
    let root = metadata.file_metadata().schema_descr().root_schema();
    let mut nodes = 0usize;
    count_schema_nodes(root, 1, &mut nodes)?;
    if nodes > MAX_SCHEMA_NODES {
        return Err(GaiaDatasetError::DatasetTooLarge);
    }
    Ok(())
}

fn count_schema_nodes(
    field: &Type,
    depth: usize,
    nodes: &mut usize,
) -> Result<(), GaiaDatasetError> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(GaiaDatasetError::SchemaMismatch);
    }
    *nodes = nodes
        .checked_add(1)
        .ok_or(GaiaDatasetError::DatasetTooLarge)?;
    if field.is_group() {
        for child in field.get_fields() {
            count_schema_nodes(child, depth + 1, nodes)?;
        }
    }
    Ok(())
}

fn validate_metadata_children(field: &Type) -> Result<(), GaiaDatasetError> {
    const EXPECTED: [&str; 5] = [
        "Steps",
        "Number of steps",
        "How long did this take?",
        "Tools",
        "Number of tools",
    ];
    if !field.is_group()
        || field.get_fields().len() != EXPECTED.len()
        || field.get_fields().len() > MAX_METADATA_LEAVES
    {
        return Err(GaiaDatasetError::SchemaMismatch);
    }
    for name in EXPECTED {
        let child = schema_field(field.get_fields(), name)?;
        if !is_optional(child) || !is_utf8_primitive(child) {
            return Err(GaiaDatasetError::SchemaMismatch);
        }
    }
    Ok(())
}

fn is_optional(field: &Type) -> bool {
    field.get_basic_info().has_repetition()
        && field.get_basic_info().repetition() == Repetition::OPTIONAL
}

fn schema_field<'a>(
    fields: &'a [std::sync::Arc<Type>],
    name: &str,
) -> Result<&'a Type, GaiaDatasetError> {
    fields
        .iter()
        .find(|field| field.name() == name)
        .map(|field| field.as_ref())
        .ok_or(GaiaDatasetError::SchemaMismatch)
}

fn is_utf8_primitive(field: &Type) -> bool {
    field.is_primitive()
        && field.get_physical_type() == PhysicalType::BYTE_ARRAY
        && field.get_basic_info().logical_type_ref() == Some(&LogicalType::String)
        && field.get_basic_info().converted_type() == ConvertedType::UTF8
}

fn take_field(columns: &mut Vec<(String, Field)>, name: &str) -> Result<Field, GaiaDatasetError> {
    let index = columns
        .iter()
        .position(|(column, _)| column == name)
        .ok_or(GaiaDatasetError::SchemaMismatch)?;
    Ok(columns.swap_remove(index).1)
}

fn take_string(columns: &mut Vec<(String, Field)>, name: &str) -> Result<String, GaiaDatasetError> {
    match take_field(columns, name)? {
        Field::Str(value) => Ok(value),
        _ => Err(GaiaDatasetError::SchemaMismatch),
    }
}

fn take_optional_string(
    columns: &mut Vec<(String, Field)>,
    name: &str,
) -> Result<Option<String>, GaiaDatasetError> {
    match take_field(columns, name)? {
        Field::Null => Ok(None),
        Field::Str(value) if value.trim().is_empty() => Ok(None),
        Field::Str(value) => Ok(Some(value)),
        _ => Err(GaiaDatasetError::SchemaMismatch),
    }
}

fn take_level(columns: &mut Vec<(String, Field)>) -> Result<u8, GaiaDatasetError> {
    let value: i64 = match take_field(columns, "Level")? {
        Field::Long(value) => value,
        Field::Str(text) => text
            .parse::<i64>()
            .map_err(|_| GaiaDatasetError::LevelMismatch)?,
        _ => return Err(GaiaDatasetError::SchemaMismatch),
    };
    u8::try_from(value).map_err(|_| GaiaDatasetError::LevelMismatch)
}

fn take_metadata(columns: &mut Vec<(String, Field)>) -> Result<(), GaiaDatasetError> {
    match take_field(columns, "Annotator Metadata")? {
        Field::Group(_) | Field::Null => Ok(()),
        _ => Err(GaiaDatasetError::SchemaMismatch),
    }
}

fn safe_task_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn verify_attachment(
    root: &Path,
    file_name: Option<&str>,
    file_path: Option<&str>,
) -> Result<Option<GaiaAttachment>, GaiaDatasetError> {
    let (file_name, file_path) = match (file_name, file_path) {
        (None, None) => return Ok(None),
        (Some(file_name), Some(file_path)) => (file_name, file_path),
        _ => return Err(GaiaDatasetError::SchemaMismatch),
    };
    let relative = Path::new(file_path);
    if looks_absolute(file_path)
        || file_path.contains('\\')
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || Path::new(file_name)
            .file_name()
            .and_then(|name| name.to_str())
            != Some(file_name)
        || relative.file_name().and_then(|name| name.to_str()) != Some(file_name)
    {
        return Err(GaiaDatasetError::AttachmentUnsafe);
    }
    ensure_no_link_components(root, relative)?;
    let candidate = root.join(relative);
    let canonical = candidate.canonicalize().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            GaiaDatasetError::AttachmentMissing
        } else {
            GaiaDatasetError::AttachmentUnsafe
        }
    })?;
    if !canonical.starts_with(root) {
        return Err(GaiaDatasetError::AttachmentUnsafe);
    }
    let attachment_file = File::open(&candidate).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            GaiaDatasetError::AttachmentMissing
        } else {
            GaiaDatasetError::AttachmentUnsafe
        }
    })?;
    let metadata = attachment_file
        .metadata()
        .map_err(|_| GaiaDatasetError::AttachmentUnsafe)?;
    if !metadata.is_file() || is_link_or_reparse(&metadata) {
        return Err(GaiaDatasetError::AttachmentUnsafe);
    }
    if metadata.len() > MAX_ATTACHMENT_BYTES {
        return Err(GaiaDatasetError::AttachmentTooLarge);
    }
    Ok(Some(GaiaAttachment {
        snapshot_root: root.to_path_buf(),
        path: canonical,
        identity: {
            let identity = file_identity(&attachment_file, &metadata)?;
            if !identity.is_valid() {
                return Err(GaiaDatasetError::AttachmentUnsafe);
            }
            identity
        },
        size: metadata.len(),
        expected_sha256: None,
    }))
}

#[cfg(test)]
mod level_schema_contract_tests {
    use super::*;
    use std::sync::Arc;

    use arrow_array::{ArrayRef, BinaryArray, Int64Array, RecordBatch, StringArray, StructArray};
    use arrow_schema::{DataType, Field as ArrowField, Fields, Schema};
    use parquet::arrow::ArrowWriter;
    use rand::random;

    fn write_schema_fixture(level: ArrayRef) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "pinvou-gaia-level-schema-{}-{:016x}",
            std::process::id(),
            random::<u64>()
        ));
        fs::create_dir_all(&root).unwrap();
        let metadata_names = [
            "Steps",
            "Number of steps",
            "How long did this take?",
            "Tools",
            "Number of tools",
        ];
        let metadata: ArrayRef = Arc::new(StructArray::new(
            Fields::from(
                metadata_names
                    .iter()
                    .map(|name| ArrowField::new(*name, DataType::Utf8, true))
                    .collect::<Vec<_>>(),
            ),
            metadata_names
                .iter()
                .map(|_| Arc::new(StringArray::from(vec![Some("x")])) as ArrayRef)
                .collect(),
            None,
        ));
        let columns: Vec<(&str, ArrayRef)> = vec![
            ("task_id", Arc::new(StringArray::from(vec!["task-1"]))),
            ("Question", Arc::new(StringArray::from(vec!["question"]))),
            ("Level", level),
            ("Final answer", Arc::new(StringArray::from(vec!["answer"]))),
            ("file_name", Arc::new(StringArray::from(vec![None::<&str>]))),
            ("file_path", Arc::new(StringArray::from(vec![None::<&str>]))),
            ("Annotator Metadata", metadata),
        ];
        let schema = Arc::new(Schema::new(
            columns
                .iter()
                .map(|(name, array)| ArrowField::new(*name, array.data_type().clone(), true))
                .collect::<Vec<_>>(),
        ));
        let batch = RecordBatch::try_new(
            schema,
            columns.into_iter().map(|(_, array)| array).collect(),
        )
        .unwrap();
        let path = root.join("level.parquet");
        let mut writer =
            ArrowWriter::try_new(File::create(&path).unwrap(), batch.schema(), None).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
        path
    }

    fn level_schema_result(level: ArrayRef) -> Result<(), GaiaDatasetError> {
        let path = write_schema_fixture(level);
        let root = path.parent().unwrap().to_path_buf();
        let reader = SerializedFileReader::new(File::open(&path).unwrap()).unwrap();
        let result = verify_schema(&reader);
        let _ = fs::remove_dir_all(root);
        result
    }

    #[test]
    fn level_schema_rejects_raw_binary_but_accepts_utf8_and_int64() {
        assert!(level_schema_result(Arc::new(BinaryArray::from(vec![b"1".as_slice()]))).is_err());
        assert!(level_schema_result(Arc::new(StringArray::from(vec!["1"]))).is_ok());
        assert!(level_schema_result(Arc::new(Int64Array::from(vec![1]))).is_ok());
    }

    #[test]
    fn level_schema_rejects_group_without_panicking() {
        let fields = Fields::from(vec![ArrowField::new("nested", DataType::Int64, true)]);
        let group: ArrayRef = Arc::new(StructArray::new(
            fields,
            vec![Arc::new(Int64Array::from(vec![1]))],
            None,
        ));
        assert!(level_schema_result(group).is_err());
    }
}

fn looks_absolute(value: &str) -> bool {
    let bytes = value.as_bytes();
    Path::new(value).is_absolute()
        || value.starts_with('/')
        || value.starts_with('\\')
        || (bytes.len() >= 2 && bytes[1] == b':')
}

fn ensure_no_link_components(root: &Path, relative: &Path) -> Result<(), GaiaDatasetError> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(GaiaDatasetError::AttachmentUnsafe);
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if is_link_or_reparse(&metadata) => {
                return Err(GaiaDatasetError::AttachmentUnsafe);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(GaiaDatasetError::AttachmentMissing);
            }
            Err(_) => return Err(GaiaDatasetError::AttachmentUnsafe),
        }
    }
    Ok(())
}

#[cfg(windows)]
fn is_link_or_reparse(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    metadata.file_type().is_symlink()
        || windows_attributes_indicate_reparse(metadata.file_attributes())
}

#[cfg(windows)]
fn windows_attributes_indicate_reparse(attributes: u32) -> bool {
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_link_or_reparse(metadata: &Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(all(test, windows))]
mod tests {
    use super::{FileIdentity, same_file_snapshot, windows_attributes_indicate_reparse};

    #[test]
    fn dataset_windows_reparse_attribute_detection_includes_junctions() {
        assert!(windows_attributes_indicate_reparse(0x0400));
        assert!(!windows_attributes_indicate_reparse(0x0020));
    }

    #[test]
    fn dataset_windows_identity_seam_rejects_file_index_change() {
        let expected = FileIdentity {
            volume: 7,
            index: 11,
        };
        assert!(same_file_snapshot(expected, 20, expected, 20));
        assert!(!same_file_snapshot(
            expected,
            20,
            FileIdentity {
                volume: 7,
                index: 12,
            },
            20,
        ));
    }
}

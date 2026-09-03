use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Component, Path, PathBuf};

use agent_backend_api::SecretText;
use fs2::FileExt;
use hf_hub::api::sync::ApiBuilder;
use hf_hub::{Cache, Repo, RepoType};
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::record::Field;
use rand::random;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};

use crate::{
    GAIA_DATASET_REVISION, GAIA_PARQUET_SIZE, GAIA_SCORER_REVISION, GaiaDataset, GaiaDatasetError,
};

const GAIA_REPO_ID: &str = "gaia-benchmark/GAIA";
const GAIA_PARQUET_PATH: &str = "2023/validation/metadata.level1.parquet";
const READY_SCHEMA: &str = "pinvou-gaia-ready/v1";
const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_ATTACHMENTS: usize = 128;
const MAX_TRANSFER_FILES: usize = MAX_ATTACHMENTS + 1;
const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENV_NAME_BYTES: usize = 128;
const MAX_METADATA_BODY_BYTES: u64 = 2 * 1024 * 1024;
const MAX_METADATA_PATH_BYTES: usize = 512;
const MAX_METADATA_PATH_TOTAL_BYTES: usize = 1024 * 1024;
const MAX_INTEGRITY_MANIFEST_BYTES: u64 = 64 * 1024;
const INTEGRITY_ALGORITHM: &str = "sha256";
const GAIA_INTEGRITY_MANIFEST: &str = ".pinvou-gaia-integrity-v1";

struct TrustedAttachmentSpec {
    path: &'static str,
    size: u64,
    algorithm: &'static str,
    digest: &'static str,
}

// Fixed trust anchor from the Hugging Face `?blobs=true` metadata response for
// GAIA_DATASET_REVISION. These are the exact attachments referenced by the
// pinned validation Level 1 parquet; they are never derived from import input.
const OFFICIAL_LEVEL1_ATTACHMENTS: [TrustedAttachmentSpec; 11] = [
    TrustedAttachmentSpec {
        path: "2023/validation/1f975693-876d-457b-a649-393859e79bf3.mp3",
        size: 280868,
        algorithm: "sha256",
        digest: "200f767e732b49efef5c05d128903ee4d2c34e66fdce7f5593ac123b2e637673",
    },
    TrustedAttachmentSpec {
        path: "2023/validation/389793a7-ca17-4e82-81cb-2b3a2391b4b9.txt",
        size: 99,
        algorithm: "git-sha1",
        digest: "3d633996285d4df9289548811be1f108435527a1",
    },
    TrustedAttachmentSpec {
        path: "2023/validation/5cfb274c-0207-4aa7-9575-6ac0bd95d9b2.xlsx",
        size: 5115,
        algorithm: "git-sha1",
        digest: "7d106d137e62d638c9feca07b9c4aa91e5ae559b",
    },
    TrustedAttachmentSpec {
        path: "2023/validation/65afbc8a-89ca-4ad5-8d62-355bb401f61d.xlsx",
        size: 12370,
        algorithm: "git-sha1",
        digest: "8bf133a28e8138803749b2b9395c223407a5f0ef",
    },
    TrustedAttachmentSpec {
        path: "2023/validation/7bd855d8-463d-4ed5-93ca-5fe35145f733.xlsx",
        size: 5285,
        algorithm: "git-sha1",
        digest: "96946fc8eb7ff5f1052e11028dee0636f7aaba40",
    },
    TrustedAttachmentSpec {
        path: "2023/validation/9318445f-fe6a-4e1b-acbf-c68228c9906a.png",
        size: 133568,
        algorithm: "sha256",
        digest: "66556e6fcc8f881d57f8a97564932eccae691076e82fa07aaa38c9f94f4c2cf0",
    },
    TrustedAttachmentSpec {
        path: "2023/validation/99c9cc74-fdc8-46c6-8f8d-3ce2d3bfeea3.mp3",
        size: 179304,
        algorithm: "sha256",
        digest: "b218c951c1f888f0bbe6f46c080f57afc7c9348fffc7ba4da35749ff1e2ac40f",
    },
    TrustedAttachmentSpec {
        path: "2023/validation/a3fbeb63-0e8c-4a11-bff6-0e3b484c3e9c.pptx",
        size: 388996,
        algorithm: "git-sha1",
        digest: "0ed6755e6dec6e7b44fd250214d9899616e7a82b",
    },
    TrustedAttachmentSpec {
        path: "2023/validation/cca530fc-4052-43b2-b130-b30968d8aa44.png",
        size: 63080,
        algorithm: "sha256",
        digest: "daaa417b9746471ec313c3233bb63175908d49de0859b5bce99431392e45efd8",
    },
    TrustedAttachmentSpec {
        path: "2023/validation/cffe0e32-c9a6-4c52-9877-78ceb4aaa9fb.docx",
        size: 17525,
        algorithm: "git-sha1",
        digest: "03da5fc57fb0e77e5e0c7c723c1f41368fa39acf",
    },
    TrustedAttachmentSpec {
        path: "2023/validation/f918266a-b3e0-4914-865d-4faa564f1aef.py",
        size: 698,
        algorithm: "git-sha1",
        digest: "9fb358549644b19f4668cf2097b3a6340a65cbb4",
    },
];

pub const GAIA_READY_MARKER: &str = ".pinvou-gaia-ready-v1";

pub enum GaiaSource {
    TokenEnvironment(String),
    ExistingSnapshot(PathBuf),
}

impl fmt::Debug for GaiaSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TokenEnvironment(_) => formatter.write_str("TokenEnvironment([redacted])"),
            Self::ExistingSnapshot(_) => formatter.write_str("ExistingSnapshot([redacted])"),
        }
    }
}

#[derive(Clone)]
pub struct GaiaAcquisition {
    snapshot_root: PathBuf,
    revision: String,
    dataset: GaiaDataset,
}

impl GaiaAcquisition {
    pub fn snapshot_root(&self) -> &Path {
        &self.snapshot_root
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }

    pub fn dataset(&self) -> &GaiaDataset {
        &self.dataset
    }

    pub fn into_dataset(self) -> GaiaDataset {
        self.dataset
    }
}

impl PartialEq for GaiaAcquisition {
    fn eq(&self, other: &Self) -> bool {
        self.snapshot_root == other.snapshot_root && self.revision == other.revision
    }
}

impl Eq for GaiaAcquisition {}

impl fmt::Debug for GaiaAcquisition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GaiaAcquisition")
            .field("snapshot_root", &"[redacted]")
            .field("revision", &self.revision)
            .finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GaiaFetchError {
    AccessDenied,
    Busy,
    DownloadFailed,
    ImportFailed,
    VerifyFailed,
}

impl GaiaFetchError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::AccessDenied => "gaia_access_denied",
            Self::Busy => "gaia_fetch_in_progress",
            Self::DownloadFailed => "gaia_download_failed",
            Self::ImportFailed => "gaia_import_failed",
            Self::VerifyFailed => "gaia_verify_failed",
        }
    }
}

impl fmt::Debug for GaiaFetchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl fmt::Display for GaiaFetchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for GaiaFetchError {}

pub struct SnapshotDownloadRequest<'a> {
    repo_id: &'a str,
    revision: &'a str,
    remote_path: &'a str,
    token: &'a SecretText,
    scratch_root: &'a Path,
    expected: &'a SnapshotFileMetadata,
    remaining_budget: u64,
}

impl SnapshotDownloadRequest<'_> {
    pub fn repo_id(&self) -> &str {
        self.repo_id
    }

    pub fn revision(&self) -> &str {
        self.revision
    }

    pub fn remote_path(&self) -> &str {
        self.remote_path
    }

    pub fn token(&self) -> &SecretText {
        self.token
    }

    pub fn expected(&self) -> &SnapshotFileMetadata {
        self.expected
    }

    pub fn remaining_budget(&self) -> u64 {
        self.remaining_budget
    }
}

impl fmt::Debug for SnapshotDownloadRequest<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SnapshotDownloadRequest")
            .field("repo_id", &self.repo_id)
            .field("revision", &self.revision)
            .field("remote_path", &"[redacted]")
            .field("expected_size", &self.expected.size)
            .field("remaining_budget", &self.remaining_budget)
            .field("token", &"[redacted]")
            .field("scratch_root", &"[redacted]")
            .finish()
    }
}

pub struct SnapshotPreflightRequest<'a> {
    repo_id: &'a str,
    revision: &'a str,
    remote_paths: &'a [PathBuf],
    token: &'a SecretText,
    scratch_root: &'a Path,
}

impl SnapshotPreflightRequest<'_> {
    pub fn repo_id(&self) -> &str {
        self.repo_id
    }

    pub fn revision(&self) -> &str {
        self.revision
    }

    pub fn remote_paths(&self) -> &[PathBuf] {
        self.remote_paths
    }

    pub fn token(&self) -> &SecretText {
        self.token
    }
}

impl fmt::Debug for SnapshotPreflightRequest<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SnapshotPreflightRequest")
            .field("repo_id", &self.repo_id)
            .field("revision", &self.revision)
            .field("remote_path_count", &self.remote_paths.len())
            .field("token", &"[redacted]")
            .field("scratch_root", &"[redacted]")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SnapshotFileMetadata {
    remote_path: PathBuf,
    size: u64,
    expected_digest: ExpectedDigest,
}

#[derive(Clone, PartialEq, Eq)]
enum ExpectedDigest {
    Sha256([u8; 32]),
    GitSha1([u8; 20]),
    None,
}

impl SnapshotFileMetadata {
    pub fn new(remote_path: impl Into<PathBuf>, size: u64, expected_sha256: [u8; 32]) -> Self {
        Self {
            remote_path: remote_path.into(),
            size,
            expected_digest: ExpectedDigest::Sha256(expected_sha256),
        }
    }

    fn new_git_blob(remote_path: impl Into<PathBuf>, size: u64, expected_sha1: [u8; 20]) -> Self {
        Self {
            remote_path: remote_path.into(),
            size,
            expected_digest: ExpectedDigest::GitSha1(expected_sha1),
        }
    }

    #[deprecated(note = "size-only metadata is rejected by GAIA acquisition")]
    pub fn new_without_digest(remote_path: impl Into<PathBuf>, size: u64) -> Self {
        Self {
            remote_path: remote_path.into(),
            size,
            expected_digest: ExpectedDigest::None,
        }
    }

    pub fn remote_path(&self) -> &Path {
        &self.remote_path
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn expected_sha256(&self) -> Option<&[u8; 32]> {
        match &self.expected_digest {
            ExpectedDigest::Sha256(digest) => Some(digest),
            ExpectedDigest::GitSha1(_) | ExpectedDigest::None => None,
        }
    }
}

impl fmt::Debug for SnapshotFileMetadata {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SnapshotFileMetadata")
            .field("remote_path", &"[redacted]")
            .field("size", &self.size)
            .field("expected_digest", &"[redacted]")
            .finish()
    }
}

/// Downloader failures deliberately carry no payload: the error channel is
/// redacted so repository paths or digests never leak through it (same
/// discipline as [`SnapshotFileMetadata`]'s Debug impl).
///
/// Internal helpers keep plumbing `Result<_, ()>`; `From<()>` adapts them at
/// the public trait boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotFetchFailure;

impl From<()> for SnapshotFetchFailure {
    fn from((): ()) -> Self {
        SnapshotFetchFailure
    }
}

pub trait SnapshotDownloader: Send + Sync {
    fn preflight(
        &self,
        request: &SnapshotPreflightRequest<'_>,
    ) -> Result<Vec<SnapshotFileMetadata>, SnapshotFetchFailure>;

    fn download(
        &self,
        request: &SnapshotDownloadRequest<'_>,
        destination: &Path,
    ) -> Result<(), SnapshotFetchFailure>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HfSnapshotDownloader;

impl SnapshotDownloader for HfSnapshotDownloader {
    fn preflight(
        &self,
        request: &SnapshotPreflightRequest<'_>,
    ) -> Result<Vec<SnapshotFileMetadata>, SnapshotFetchFailure> {
        if request.repo_id != GAIA_REPO_ID || request.revision != GAIA_DATASET_REVISION {
            return Err(SnapshotFetchFailure);
        }
        let repo = hf_repo(
            request.token,
            request.scratch_root,
            request.repo_id,
            request.revision,
        )?;
        let requested = request
            .remote_paths
            .iter()
            .map(|path| path.to_str().ok_or(SnapshotFetchFailure).map(str::to_owned))
            .collect::<Result<HashSet<_>, _>>()?;
        if requested.len() != request.remote_paths.len() {
            return Err(SnapshotFetchFailure);
        }
        let response = repo
            .info_request()
            .query("blobs", "true")
            .call()
            .map_err(|_| SnapshotFetchFailure)?;
        Ok(parse_hf_metadata(response.into_reader(), &requested)?.siblings)
    }

    fn download(
        &self,
        request: &SnapshotDownloadRequest<'_>,
        destination: &Path,
    ) -> Result<(), SnapshotFetchFailure> {
        if request.repo_id != GAIA_REPO_ID || request.revision != GAIA_DATASET_REVISION {
            return Err(SnapshotFetchFailure);
        }
        let repo = hf_repo(
            request.token,
            request.scratch_root,
            request.repo_id,
            request.revision,
        )?;
        if request.expected.remote_path() != Path::new(request.remote_path)
            || request.expected.size() > request.remaining_budget
        {
            return Err(SnapshotFetchFailure);
        }
        let url = repo.url(request.remote_path);
        let response = ureq::AgentBuilder::new()
            .redirect_auth_headers(ureq::RedirectAuthHeaders::SameHost)
            .build()
            .get(&url)
            .set(
                "Authorization",
                &format!("Bearer {}", request.token.expose_to_backend()),
            )
            .call()
            .map_err(|_| SnapshotFetchFailure)?;
        let content_length = response
            .header("Content-Length")
            .map(|value| value.parse::<u64>().map_err(|_| SnapshotFetchFailure))
            .transpose()?;
        let parent = destination.parent().ok_or(SnapshotFetchFailure)?;
        create_private_parent_directories(parent)?;
        stream_verified_file(
            response.into_reader(),
            content_length,
            destination,
            request.expected,
            request.remaining_budget,
        )?;
        Ok(())
    }
}

struct HfRepoMetadata {
    siblings: Vec<SnapshotFileMetadata>,
}

#[derive(Deserialize)]
struct RawHfRepoMetadata {
    sha: String,
    siblings: Vec<RawHfSiblingMetadata>,
}

#[derive(Deserialize)]
struct RawHfSiblingMetadata {
    rfilename: String,
    size: Option<u64>,
    lfs: Option<RawHfLfsMetadata>,
    #[serde(rename = "blobId", alias = "blob_id")]
    blob_id: Option<String>,
}

#[derive(Deserialize)]
struct RawHfLfsMetadata {
    oid: Option<String>,
    sha256: Option<String>,
    size: Option<u64>,
}

fn hf_repo(
    token: &SecretText,
    scratch_root: &Path,
    repo_id: &str,
    revision: &str,
) -> Result<hf_hub::api::sync::ApiRepo, ()> {
    if repo_id != GAIA_REPO_ID || revision != GAIA_DATASET_REVISION {
        return Err(());
    }
    let cache_dir = scratch_root.join(".hf-cache").join("hub");
    fs::create_dir_all(&cache_dir).map_err(|_| ())?;
    let api = ApiBuilder::from_cache(Cache::new(cache_dir))
        .with_progress(false)
        .with_token(Some(token.expose_to_backend().to_owned()))
        .build()
        .map_err(|_| ())?;
    Ok(api.repo(Repo::with_revision(
        GAIA_REPO_ID.to_owned(),
        RepoType::Dataset,
        GAIA_DATASET_REVISION.to_owned(),
    )))
}

fn parse_hf_metadata(
    mut reader: impl Read,
    requested: &HashSet<String>,
) -> Result<HfRepoMetadata, ()> {
    let mut body = Vec::new();
    reader
        .by_ref()
        .take(MAX_METADATA_BODY_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|_| ())?;
    if body.len() as u64 > MAX_METADATA_BODY_BYTES {
        return Err(());
    }
    let raw: RawHfRepoMetadata = serde_json::from_slice(&body).map_err(|_| ())?;
    if raw.sha != GAIA_DATASET_REVISION
        || raw.sha.len() != 40
        || !raw.sha.bytes().all(|byte| byte.is_ascii_hexdigit())
        || raw.siblings.len() > 4096
    {
        return Err(());
    }
    let mut total_path_bytes = 0_usize;
    let mut siblings = Vec::with_capacity(raw.siblings.len());
    let mut matched = HashSet::new();
    for sibling in raw.siblings {
        if sibling.rfilename.is_empty()
            || sibling.rfilename.len() > MAX_METADATA_PATH_BYTES
            || !sibling.rfilename.is_ascii()
        {
            return Err(());
        }
        let relative = PathBuf::from(&sibling.rfilename);
        validate_relative(&relative)?;
        total_path_bytes = total_path_bytes
            .checked_add(sibling.rfilename.len())
            .ok_or(())?;
        if total_path_bytes > MAX_METADATA_PATH_TOTAL_BYTES {
            return Err(());
        }
        if !requested.contains(&sibling.rfilename) {
            continue;
        }
        if !matched.insert(sibling.rfilename.clone()) {
            return Err(());
        }
        let size = sibling.size.ok_or(())?;
        let metadata = if let Some(lfs) = sibling.lfs {
            if lfs.size != Some(size) {
                return Err(());
            }
            let oid = lfs
                .oid
                .as_deref()
                .map(|value| value.strip_prefix("sha256:").unwrap_or(value).to_owned());
            let digest_text = match (oid, lfs.sha256) {
                (Some(oid), Some(sha256)) if oid == sha256 => oid,
                (Some(oid), None) => oid,
                (None, Some(sha256)) => sha256,
                _ => return Err(()),
            };
            let digest = parse_sha256(&digest_text)?;
            SnapshotFileMetadata::new(relative, size, digest)
        } else {
            let digest = parse_git_sha1(sibling.blob_id.as_deref().ok_or(())?)?;
            SnapshotFileMetadata::new_git_blob(relative, size, digest)
        };
        siblings.push(metadata);
    }
    if &matched != requested {
        return Err(());
    }
    Ok(HfRepoMetadata { siblings })
}

fn parse_sha256(value: &str) -> Result<[u8; 32], ()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(());
    }
    let mut digest = [0_u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).map_err(|_| ())?;
    }
    Ok(digest)
}

fn parse_git_sha1(value: &str) -> Result<[u8; 20], ()> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(());
    }
    let mut digest = [0_u8; 20];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).map_err(|_| ())?;
    }
    Ok(digest)
}

pub struct GaiaSnapshotManager<D> {
    acquisition_root: PathBuf,
    worktree_root: Option<PathBuf>,
    downloader: D,
}

impl<D: SnapshotDownloader> GaiaSnapshotManager<D> {
    pub fn new(
        acquisition_root: impl AsRef<Path>,
        worktree_root: impl AsRef<Path>,
        downloader: D,
    ) -> Result<Self, GaiaFetchError> {
        Self::new_with_optional_worktree(acquisition_root, Some(worktree_root.as_ref()), downloader)
    }

    pub fn new_with_optional_worktree(
        acquisition_root: impl AsRef<Path>,
        worktree_root: Option<&Path>,
        downloader: D,
    ) -> Result<Self, GaiaFetchError> {
        let acquisition_root = secure_private_acquisition_directory(acquisition_root.as_ref())
            .map_err(|_| GaiaFetchError::ImportFailed)?;
        let worktree_root = worktree_root
            .map(secure_directory)
            .transpose()
            .map_err(|_| GaiaFetchError::ImportFailed)?;
        if worktree_root
            .as_ref()
            .is_some_and(|worktree| overlaps(&acquisition_root, worktree))
        {
            return Err(GaiaFetchError::ImportFailed);
        }
        Ok(Self {
            acquisition_root,
            worktree_root,
            downloader,
        })
    }

    pub fn downloader(&self) -> &D {
        &self.downloader
    }

    pub fn acquire(&self, source: GaiaSource) -> Result<GaiaAcquisition, GaiaFetchError> {
        self.acquire_expected(source, GAIA_PARQUET_SIZE, production_digest())
    }

    pub fn verify_offline(
        &self,
        snapshot_root: impl AsRef<Path>,
    ) -> Result<GaiaAcquisition, GaiaFetchError> {
        self.verify_ready(
            snapshot_root.as_ref(),
            GAIA_PARQUET_SIZE,
            production_digest(),
        )
    }

    pub fn verify_source(
        &self,
        snapshot_root: impl AsRef<Path>,
    ) -> Result<GaiaAcquisition, GaiaFetchError> {
        self.verify_source_expected(
            snapshot_root.as_ref(),
            GAIA_PARQUET_SIZE,
            production_digest(),
            &OFFICIAL_LEVEL1_ATTACHMENTS,
        )
    }

    fn verify_source_expected(
        &self,
        snapshot_root: &Path,
        expected_size: u64,
        expected_digest: [u8; 32],
        trusted: &[TrustedAttachmentSpec],
    ) -> Result<GaiaAcquisition, GaiaFetchError> {
        let root = secure_directory(snapshot_root).map_err(|_| GaiaFetchError::VerifyFailed)?;
        if root == self.ready_root() {
            return self.verify_ready(&root, expected_size, expected_digest);
        }
        let mut dataset = verify_dataset(&root, expected_size, expected_digest)
            .map_err(|_| GaiaFetchError::VerifyFailed)?;
        let (parquet, _) =
            crate::dataset::open_verified_snapshot_file(&root, Path::new(GAIA_PARQUET_PATH))
                .map_err(|_| GaiaFetchError::VerifyFailed)?;
        CapturedFile::capture_expected(
            parquet,
            PathBuf::from(GAIA_PARQUET_PATH),
            expected_size,
            expected_digest,
        )
        .map_err(|_| GaiaFetchError::VerifyFailed)?;
        let captured = capture_trusted_import_attachments(&dataset, expected_size, trusted)
            .map_err(|_| GaiaFetchError::VerifyFailed)?;
        verify_trusted_source_tree(&root, trusted).map_err(|_| GaiaFetchError::VerifyFailed)?;
        let attachment_digests = captured
            .iter()
            .map(|file| (file.relative.clone(), file.sha256))
            .collect::<BTreeMap<_, _>>();
        dataset
            .bind_attachment_sha256(&attachment_digests)
            .map_err(|_| GaiaFetchError::VerifyFailed)?;
        Ok(GaiaAcquisition {
            snapshot_root: root,
            revision: GAIA_DATASET_REVISION.to_owned(),
            dataset,
        })
    }

    fn acquire_expected(
        &self,
        source: GaiaSource,
        expected_size: u64,
        expected_digest: [u8; 32],
    ) -> Result<GaiaAcquisition, GaiaFetchError> {
        let ready_root = self.ready_root();
        if ready_root.exists()
            && let Ok(ready) = self.verify_ready(&ready_root, expected_size, expected_digest)
        {
            return Ok(ready);
        }

        let _lock = AcquisitionLock::claim(&self.acquisition_root)?;
        if ready_root.exists() {
            if let Ok(ready) = self.verify_ready(&ready_root, expected_size, expected_digest) {
                return Ok(ready);
            }
            remove_identity_owned_directory(&self.acquisition_root, &ready_root)?;
        }
        let staging = StagingDirectory::create(&self.acquisition_root)?;
        match source {
            GaiaSource::ExistingSnapshot(source) => {
                self.import_existing(&source, staging.path(), expected_size, expected_digest)?;
            }
            GaiaSource::TokenEnvironment(name) => {
                let token = read_named_token(&name)?;
                self.download_snapshot(&token, staging.path(), expected_size, expected_digest)?;
            }
        }

        let verified = verify_dataset(staging.path(), expected_size, expected_digest)
            .map_err(|_| GaiaFetchError::VerifyFailed)?;
        let captured = capture_dataset_files(&verified, expected_size, expected_digest)
            .map_err(|_| GaiaFetchError::VerifyFailed)?;
        publish_reserved(&ready_root, captured, |root| {
            self.verify_ready(root, expected_size, expected_digest)
        })
    }

    fn import_existing(
        &self,
        source: &Path,
        staging: &Path,
        expected_size: u64,
        expected_digest: [u8; 32],
    ) -> Result<(), GaiaFetchError> {
        let canonical_source =
            secure_directory(source).map_err(|_| GaiaFetchError::ImportFailed)?;
        if self
            .worktree_root
            .as_ref()
            .is_some_and(|worktree| overlaps(&canonical_source, worktree))
            || overlaps(&canonical_source, &self.acquisition_root)
        {
            return Err(GaiaFetchError::ImportFailed);
        }
        let dataset = verify_dataset(&canonical_source, expected_size, expected_digest)
            .map_err(classify_import_verification)?;
        let trusted: &[TrustedAttachmentSpec] =
            if expected_size == GAIA_PARQUET_SIZE && expected_digest == production_digest() {
                &OFFICIAL_LEVEL1_ATTACHMENTS
            } else {
                #[cfg(any(test, feature = "test-support"))]
                {
                    &[]
                }
                #[cfg(not(any(test, feature = "test-support")))]
                {
                    return Err(GaiaFetchError::ImportFailed);
                }
            };
        let trusted_attachments =
            capture_trusted_import_attachments(&dataset, expected_size, trusted)
                .map_err(|_| GaiaFetchError::ImportFailed)?;
        write_private_file(
            staging.join(crate::GAIA_REVISION_MARKER),
            GAIA_DATASET_REVISION.as_bytes(),
        )
        .map_err(|_| GaiaFetchError::ImportFailed)?;
        let parquet = File::open(dataset.snapshot_root().join(GAIA_PARQUET_PATH))
            .map_err(|_| GaiaFetchError::ImportFailed)?;
        let mut captured = vec![
            CapturedFile::capture_expected(
                parquet,
                PathBuf::from(GAIA_PARQUET_PATH),
                expected_size,
                expected_digest,
            )
            .map_err(|_| GaiaFetchError::ImportFailed)?,
        ];
        captured.extend(trusted_attachments);
        validate_remote_budget(&captured.iter().map(|file| file.size).collect::<Vec<_>>())
            .map_err(|_| GaiaFetchError::ImportFailed)?;
        for file in captured {
            let destination = staging.join(&file.relative);
            file.copy_to(&destination)
                .map_err(|_| GaiaFetchError::ImportFailed)?;
        }
        Ok(())
    }

    fn download_snapshot(
        &self,
        token: &SecretText,
        staging: &Path,
        expected_size: u64,
        expected_digest: [u8; 32],
    ) -> Result<(), GaiaFetchError> {
        write_private_file(
            staging.join(crate::GAIA_REVISION_MARKER),
            GAIA_DATASET_REVISION.as_bytes(),
        )
        .map_err(|_| GaiaFetchError::DownloadFailed)?;
        let parquet_paths = vec![PathBuf::from(GAIA_PARQUET_PATH)];
        let parquet_metadata = self.preflight(token, staging, &parquet_paths)?;
        if parquet_metadata.len() != 1
            || parquet_metadata[0].size() != expected_size
            || parquet_metadata[0].expected_sha256() != Some(&expected_digest)
        {
            return Err(GaiaFetchError::DownloadFailed);
        }
        validate_remote_budget(
            &parquet_metadata
                .iter()
                .map(SnapshotFileMetadata::size)
                .collect::<Vec<_>>(),
        )
        .map_err(|_| GaiaFetchError::DownloadFailed)?;
        let mut actual_bytes = 0_u64;
        self.download_one(
            token,
            staging,
            Path::new(GAIA_PARQUET_PATH),
            &parquet_metadata[0],
            &mut actual_bytes,
        )?;
        verify_parquet_digest(staging, expected_size, expected_digest)
            .map_err(|_| GaiaFetchError::VerifyFailed)?;
        let attachments =
            trusted_attachment_paths(staging).map_err(|_| GaiaFetchError::VerifyFailed)?;
        if attachments.len() > MAX_ATTACHMENTS {
            return Err(GaiaFetchError::VerifyFailed);
        }
        let attachment_metadata = self.preflight(token, staging, &attachments)?;
        let mut sizes = vec![parquet_metadata[0].size()];
        sizes.extend(attachment_metadata.iter().map(SnapshotFileMetadata::size));
        validate_remote_budget(&sizes).map_err(|_| GaiaFetchError::DownloadFailed)?;
        for relative in attachments {
            let metadata = attachment_metadata
                .iter()
                .find(|entry| entry.remote_path() == relative)
                .ok_or(GaiaFetchError::DownloadFailed)?;
            self.download_one(token, staging, &relative, metadata, &mut actual_bytes)?;
        }
        let cache = staging.join(".hf-cache");
        if cache.exists() {
            fs::remove_dir_all(cache).map_err(|_| GaiaFetchError::DownloadFailed)?;
        }
        validate_snapshot_budget(staging).map_err(|_| GaiaFetchError::DownloadFailed)?;
        Ok(())
    }

    fn preflight(
        &self,
        token: &SecretText,
        staging: &Path,
        remote_paths: &[PathBuf],
    ) -> Result<Vec<SnapshotFileMetadata>, GaiaFetchError> {
        if remote_paths.is_empty() {
            return Ok(Vec::new());
        }
        let request = SnapshotPreflightRequest {
            repo_id: GAIA_REPO_ID,
            revision: GAIA_DATASET_REVISION,
            remote_paths,
            token,
            scratch_root: staging,
        };
        let metadata = self
            .downloader
            .preflight(&request)
            .map_err(|_| GaiaFetchError::DownloadFailed)?;
        validate_preflight_identity(remote_paths, &metadata)
            .map_err(|_| GaiaFetchError::DownloadFailed)?;
        if metadata
            .iter()
            .any(|entry| matches!(entry.expected_digest, ExpectedDigest::None))
        {
            return Err(GaiaFetchError::DownloadFailed);
        }
        Ok(metadata)
    }

    fn download_one(
        &self,
        token: &SecretText,
        staging: &Path,
        relative: &Path,
        expected: &SnapshotFileMetadata,
        actual_total: &mut u64,
    ) -> Result<(), GaiaFetchError> {
        validate_relative(relative).map_err(|_| GaiaFetchError::DownloadFailed)?;
        if matches!(expected.expected_digest, ExpectedDigest::None) {
            return Err(GaiaFetchError::DownloadFailed);
        }
        let remote = relative.to_str().ok_or(GaiaFetchError::DownloadFailed)?;
        let destination = staging.join(relative);
        let request = SnapshotDownloadRequest {
            repo_id: GAIA_REPO_ID,
            revision: GAIA_DATASET_REVISION,
            remote_path: remote,
            token,
            scratch_root: staging,
            expected,
            remaining_budget: MAX_TOTAL_BYTES
                .checked_sub(*actual_total)
                .ok_or(GaiaFetchError::DownloadFailed)?,
        };
        self.downloader
            .download(&request, &destination)
            .map_err(|_| GaiaFetchError::DownloadFailed)?;
        verify_downloaded_file(&destination, expected, actual_total)
            .map_err(|_| GaiaFetchError::DownloadFailed)
    }

    fn verify_ready(
        &self,
        snapshot_root: &Path,
        expected_size: u64,
        expected_digest: [u8; 32],
    ) -> Result<GaiaAcquisition, GaiaFetchError> {
        let root = secure_directory(snapshot_root).map_err(|_| GaiaFetchError::VerifyFailed)?;
        if !root.starts_with(&self.acquisition_root) || root != self.ready_root() {
            return Err(GaiaFetchError::VerifyFailed);
        }
        let marker = root.join(GAIA_READY_MARKER);
        let metadata = fs::symlink_metadata(&marker).map_err(|_| GaiaFetchError::VerifyFailed)?;
        if !metadata.is_file() || is_link_or_reparse(&metadata) || metadata.len() > 512 {
            return Err(GaiaFetchError::VerifyFailed);
        }
        let contents = read_bounded_file(&root, Path::new(GAIA_READY_MARKER), 512)
            .map_err(|_| GaiaFetchError::VerifyFailed)?;
        let manifest_digest =
            parse_ready_marker(&contents).map_err(|_| GaiaFetchError::VerifyFailed)?;
        let manifest = read_bounded_file(
            &root,
            Path::new(GAIA_INTEGRITY_MANIFEST),
            MAX_INTEGRITY_MANIFEST_BYTES,
        )
        .map_err(|_| GaiaFetchError::VerifyFailed)?;
        if <[u8; 32]>::from(Sha256::digest(&manifest)) != manifest_digest {
            return Err(GaiaFetchError::VerifyFailed);
        }
        let entries =
            parse_integrity_manifest(&manifest).map_err(|_| GaiaFetchError::VerifyFailed)?;
        let mut dataset = verify_dataset(&root, expected_size, expected_digest)
            .map_err(|_| GaiaFetchError::VerifyFailed)?;
        verify_attachment_integrity(&dataset, &entries, expected_size)
            .map_err(|_| GaiaFetchError::VerifyFailed)?;
        verify_ready_tree(&root, &entries).map_err(|_| GaiaFetchError::VerifyFailed)?;
        let attachment_digests = entries
            .iter()
            .map(|entry| {
                Ok((
                    PathBuf::from(&entry.path),
                    parse_sha256(&entry.digest).map_err(|_| GaiaFetchError::VerifyFailed)?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>, GaiaFetchError>>()?;
        dataset
            .bind_attachment_sha256(&attachment_digests)
            .map_err(|_| GaiaFetchError::VerifyFailed)?;
        Ok(GaiaAcquisition {
            snapshot_root: root,
            revision: GAIA_DATASET_REVISION.to_owned(),
            dataset,
        })
    }

    fn ready_root(&self) -> PathBuf {
        self.acquisition_root.join(format!(
            "gaia-2023-validation-level1-{}",
            &GAIA_DATASET_REVISION[..12]
        ))
    }
}

fn production_digest() -> [u8; 32] {
    [
        0x5e, 0x57, 0x4b, 0x0f, 0xae, 0xb4, 0x60, 0x3b, 0x81, 0x6e, 0x42, 0x6c, 0xf7, 0xc7, 0xae,
        0xfb, 0x1f, 0xe3, 0x98, 0xd3, 0x2f, 0x9c, 0x48, 0x61, 0xe1, 0xa4, 0xe3, 0x30, 0x4f, 0x2b,
        0x12, 0x81,
    ]
}

fn verify_dataset(
    root: &Path,
    expected_size: u64,
    expected_digest: [u8; 32],
) -> Result<GaiaDataset, GaiaDatasetError> {
    #[cfg(feature = "test-support")]
    {
        GaiaDataset::verify_with_expected_parquet(root, expected_size, expected_digest)
    }
    #[cfg(all(test, not(feature = "test-support")))]
    {
        GaiaDataset::verify_with_expected_parquet_for_tests(root, expected_size, expected_digest)
    }
    #[cfg(not(any(test, feature = "test-support")))]
    {
        let _ = (expected_size, expected_digest);
        GaiaDataset::verify(root)
    }
}

fn classify_import_verification(error: GaiaDatasetError) -> GaiaFetchError {
    match error {
        GaiaDatasetError::AttachmentMissing
        | GaiaDatasetError::AttachmentUnsafe
        | GaiaDatasetError::AttachmentTooLarge => GaiaFetchError::ImportFailed,
        _ => GaiaFetchError::VerifyFailed,
    }
}

fn read_named_token(name: &str) -> Result<SecretText, GaiaFetchError> {
    if name.is_empty()
        || name.len() > MAX_ENV_NAME_BYTES
        || !name.is_ascii()
        || !name.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_alphanumeric() && (index > 0 || !byte.is_ascii_digit())
        })
    {
        return Err(GaiaFetchError::AccessDenied);
    }
    let token = std::env::var(name).map_err(|_| GaiaFetchError::AccessDenied)?;
    if token.trim().is_empty() {
        return Err(GaiaFetchError::AccessDenied);
    }
    Ok(SecretText::new(token))
}

fn write_ready_marker(root: &Path, manifest_digest: [u8; 32]) -> Result<(), GaiaFetchError> {
    let marker = root.join(GAIA_READY_MARKER);
    let mut file = create_private_file(&marker).map_err(|_| GaiaFetchError::VerifyFailed)?;
    file.write_all(ready_marker_contents(manifest_digest).as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|_| GaiaFetchError::VerifyFailed)
}

fn write_private_file(path: impl AsRef<Path>, contents: &[u8]) -> Result<(), ()> {
    let path = path.as_ref();
    create_private_parent_directories(path.parent().ok_or(())?)?;
    let mut file = create_private_file(path)?;
    file.write_all(contents).map_err(|_| ())?;
    file.sync_all().map_err(|_| ())
}

pub(crate) fn create_private_file(path: &Path) -> Result<File, ()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(|_| ())?;
    set_private_file_permissions(path)?;
    Ok(file)
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> Result<(), ()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|_| ())
}

#[cfg(not(unix))]
fn set_private_file_permissions(path: &Path) -> Result<(), ()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ())?;
    if !metadata.is_file() || is_link_or_reparse(&metadata) {
        return Err(());
    }
    #[cfg(windows)]
    benchmark_core::apply_windows_private_acl(path, false)?;
    Ok(())
}

fn create_private_parent_directories(path: &Path) -> Result<(), ()> {
    fs::create_dir_all(path).map_err(|_| ())?;
    let metadata = fs::symlink_metadata(path).map_err(|_| ())?;
    if !metadata.is_dir() || is_link_or_reparse(&metadata) {
        return Err(());
    }
    set_private_directory_permissions(path)
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<(), ()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|_| ())
}

#[cfg(not(unix))]
fn set_private_directory_permissions(path: &Path) -> Result<(), ()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ())?;
    if !metadata.is_dir() || is_link_or_reparse(&metadata) {
        return Err(());
    }
    #[cfg(windows)]
    benchmark_core::apply_windows_private_acl(path, true)?;
    Ok(())
}

fn ready_marker_contents(manifest_digest: [u8; 32]) -> String {
    format!(
        "schema={READY_SCHEMA}\ndataset_revision={GAIA_DATASET_REVISION}\nscorer_revision={GAIA_SCORER_REVISION}\nintegrity_sha256={}\n",
        hex_digest(&manifest_digest)
    )
}

fn parse_ready_marker(contents: &[u8]) -> Result<[u8; 32], ()> {
    let text = std::str::from_utf8(contents).map_err(|_| ())?;
    let prefix = format!(
        "schema={READY_SCHEMA}\ndataset_revision={GAIA_DATASET_REVISION}\nscorer_revision={GAIA_SCORER_REVISION}\nintegrity_sha256="
    );
    let digest = text
        .strip_prefix(&prefix)
        .ok_or(())?
        .strip_suffix('\n')
        .ok_or(())?;
    if digest.contains('\n') {
        return Err(());
    }
    parse_sha256(digest)
}

fn read_bounded_file(root: &Path, relative: &Path, limit: u64) -> Result<Vec<u8>, ()> {
    let (file, metadata) =
        crate::dataset::open_verified_snapshot_file(root, relative).map_err(|_| ())?;
    if metadata.len() > limit {
        return Err(());
    }
    let mut contents = Vec::with_capacity(metadata.len() as usize);
    file.take(limit + 1)
        .read_to_end(&mut contents)
        .map_err(|_| ())?;
    if contents.len() as u64 != metadata.len() || contents.len() as u64 > limit {
        return Err(());
    }
    Ok(contents)
}

fn verify_parquet_digest(
    root: &Path,
    expected_size: u64,
    expected_digest: [u8; 32],
) -> Result<(), ()> {
    let path = root.join(GAIA_PARQUET_PATH);
    let metadata = fs::symlink_metadata(&path).map_err(|_| ())?;
    if !metadata.is_file()
        || is_link_or_reparse(&metadata)
        || metadata.len() != expected_size
        || metadata.len() > MAX_FILE_BYTES
    {
        return Err(());
    }
    let mut file = File::open(path).map_err(|_| ())?;
    let mut hasher = Sha256::new();
    let copied = std::io::copy(&mut file, &mut hasher).map_err(|_| ())?;
    if copied != expected_size || <[u8; 32]>::from(hasher.finalize()) != expected_digest {
        return Err(());
    }
    Ok(())
}

fn verify_downloaded_file(
    path: &Path,
    expected: &SnapshotFileMetadata,
    actual_total: &mut u64,
) -> Result<(), ()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ())?;
    if !metadata.is_file()
        || is_link_or_reparse(&metadata)
        || metadata.len() != expected.size
        || metadata.len() > MAX_FILE_BYTES
    {
        return Err(());
    }
    let mut file = File::open(path).map_err(|_| ())?;
    let mut sha256 = Sha256::new();
    let mut git_sha1 = Sha1::new();
    git_sha1.update(format!("blob {}\0", expected.size).as_bytes());
    let copied = std::io::copy(
        &mut std::io::Read::by_ref(&mut file).take(MAX_FILE_BYTES + 1),
        &mut DigestingWriter::new(&mut sha256, &mut git_sha1),
    )
    .map_err(|_| ())?;
    if copied != expected.size {
        return Err(());
    }
    if !digest_matches(expected, sha256, git_sha1) {
        return Err(());
    }
    let updated = actual_total.checked_add(copied).ok_or(())?;
    if updated > MAX_TOTAL_BYTES {
        return Err(());
    }
    *actual_total = updated;
    set_private_file_permissions(path)?;
    Ok(())
}

fn stream_verified_file(
    reader: impl Read,
    content_length: Option<u64>,
    destination: &Path,
    expected: &SnapshotFileMetadata,
    remaining_budget: u64,
) -> Result<u64, ()> {
    if destination.exists()
        || expected.size > MAX_FILE_BYTES
        || expected.size > remaining_budget
        || content_length.is_some_and(|length| length != expected.size)
    {
        return Err(());
    }
    let parent = destination.parent().ok_or(())?;
    create_private_parent_directories(parent)?;
    let temporary = parent.join(format!(".pinvou-download-{:016x}.tmp", random::<u64>()));
    let result = (|| {
        let mut output = create_private_file(&temporary)?;
        let mut limited = reader.take(expected.size.saturating_add(1));
        let mut sha256 = Sha256::new();
        let mut git_sha1 = Sha1::new();
        git_sha1.update(format!("blob {}\0", expected.size).as_bytes());
        let mut copied = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = limited.read(&mut buffer).map_err(|_| ())?;
            if read == 0 {
                break;
            }
            copied = copied.checked_add(read as u64).ok_or(())?;
            if copied > expected.size || copied > remaining_budget {
                return Err(());
            }
            sha256.update(&buffer[..read]);
            git_sha1.update(&buffer[..read]);
            output.write_all(&buffer[..read]).map_err(|_| ())?;
        }
        if copied != expected.size {
            return Err(());
        }
        if !digest_matches(expected, sha256, git_sha1) {
            return Err(());
        }
        output.sync_all().map_err(|_| ())?;
        drop(output);
        fs::hard_link(&temporary, destination).map_err(|_| ())?;
        fs::remove_file(&temporary).map_err(|_| ())?;
        Ok(copied)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

struct DigestingWriter<'a> {
    sha256: &'a mut Sha256,
    git_sha1: &'a mut Sha1,
}

impl<'a> DigestingWriter<'a> {
    fn new(sha256: &'a mut Sha256, git_sha1: &'a mut Sha1) -> Self {
        Self { sha256, git_sha1 }
    }
}

impl Write for DigestingWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.sha256.update(buffer);
        self.git_sha1.update(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn digest_matches(expected: &SnapshotFileMetadata, sha256: Sha256, git_sha1: Sha1) -> bool {
    match expected.expected_digest {
        ExpectedDigest::Sha256(digest) => <[u8; 32]>::from(sha256.finalize()) == digest,
        ExpectedDigest::GitSha1(digest) => <[u8; 20]>::from(git_sha1.finalize()) == digest,
        ExpectedDigest::None => false,
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct IntegrityEntry {
    path: String,
    algorithm: String,
    digest: String,
}

fn manifest_path(relative: &Path) -> Result<String, ()> {
    validate_relative(relative)?;
    let mut rendered = String::new();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err(());
        };
        let part = part.to_str().ok_or(())?;
        if part.is_empty() || part.contains(['/', '\\']) {
            return Err(());
        }
        if !rendered.is_empty() {
            rendered.push('/');
        }
        rendered.push_str(part);
    }
    if rendered.is_empty() || rendered.len() > 1024 {
        return Err(());
    }
    Ok(rendered)
}

fn build_integrity_manifest(captured: &[CapturedFile]) -> Result<Vec<u8>, ()> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for file in captured {
        if file.relative == Path::new(GAIA_PARQUET_PATH) {
            continue;
        }
        let path = manifest_path(&file.relative)?;
        if !seen.insert(path.clone()) {
            return Err(());
        }
        entries.push(IntegrityEntry {
            path,
            algorithm: INTEGRITY_ALGORITHM.to_owned(),
            digest: hex_digest(&file.sha256),
        });
    }
    if entries.len() > MAX_ATTACHMENTS {
        return Err(());
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    let mut manifest = Vec::new();
    for entry in entries {
        serde_json::to_writer(&mut manifest, &entry).map_err(|_| ())?;
        manifest.push(b'\n');
        if manifest.len() as u64 > MAX_INTEGRITY_MANIFEST_BYTES {
            return Err(());
        }
    }
    Ok(manifest)
}

fn parse_integrity_manifest(contents: &[u8]) -> Result<Vec<IntegrityEntry>, ()> {
    if contents.len() as u64 > MAX_INTEGRITY_MANIFEST_BYTES {
        return Err(());
    }
    if contents.is_empty() {
        return Ok(Vec::new());
    }
    let records = contents.strip_suffix(b"\n").ok_or(())?;
    if records.is_empty() {
        return Err(());
    }
    let mut entries = Vec::new();
    let mut previous: Option<String> = None;
    for line in records.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            return Err(());
        }
        let entry: IntegrityEntry = serde_json::from_slice(line).map_err(|_| ())?;
        if entry.algorithm != INTEGRITY_ALGORITHM
            || parse_sha256(&entry.digest).is_err()
            || manifest_path(Path::new(&entry.path))? != entry.path
            || previous.as_ref().is_some_and(|path| path >= &entry.path)
        {
            return Err(());
        }
        previous = Some(entry.path.clone());
        entries.push(entry);
        if entries.len() > MAX_ATTACHMENTS {
            return Err(());
        }
    }
    Ok(entries)
}

fn verify_attachment_integrity(
    dataset: &GaiaDataset,
    entries: &[IntegrityEntry],
    parquet_size: u64,
) -> Result<(), ()> {
    let mut expected = BTreeMap::new();
    for row in dataset.rows() {
        if let Some(attachment) = row.attachment() {
            let relative = attachment
                .path()
                .strip_prefix(dataset.snapshot_root())
                .map_err(|_| ())?;
            let path = manifest_path(relative)?;
            expected.entry(path).or_insert(attachment);
        }
    }
    if expected.len() != entries.len() {
        return Err(());
    }
    let mut sizes = vec![parquet_size];
    for entry in entries {
        let attachment = expected.get(&entry.path).ok_or(())?;
        let expected_digest = parse_sha256(&entry.digest)?;
        let mut file = attachment.reopen_verified().map_err(|_| ())?;
        let mut hasher = Sha256::new();
        let copied = std::io::copy(
            &mut std::io::Read::by_ref(&mut file).take(MAX_FILE_BYTES + 1),
            &mut hasher,
        )
        .map_err(|_| ())?;
        if copied != attachment.size()
            || copied > MAX_FILE_BYTES
            || <[u8; 32]>::from(hasher.finalize()) != expected_digest
        {
            return Err(());
        }
        sizes.push(copied);
    }
    validate_remote_budget(&sizes)
}

fn capture_trusted_import_attachments(
    dataset: &GaiaDataset,
    parquet_size: u64,
    trusted: &[TrustedAttachmentSpec],
) -> Result<Vec<CapturedFile>, ()> {
    let mut attachments = BTreeMap::new();
    for row in dataset.rows() {
        if let Some(attachment) = row.attachment() {
            let relative = attachment
                .path()
                .strip_prefix(dataset.snapshot_root())
                .map_err(|_| ())?;
            attachments
                .entry(manifest_path(relative)?)
                .or_insert(attachment);
        }
    }
    if attachments.len() != trusted.len() {
        return Err(());
    }

    let mut seen = BTreeSet::new();
    let mut sizes = vec![parquet_size];
    let mut captured_files = Vec::with_capacity(trusted.len());
    for spec in trusted {
        let path = manifest_path(Path::new(spec.path))?;
        if path != spec.path || !seen.insert(path.clone()) {
            return Err(());
        }
        let attachment = attachments.get(&path).ok_or(())?;
        if attachment.size() != spec.size || spec.size > MAX_FILE_BYTES {
            return Err(());
        }
        let relative = PathBuf::from(&path);
        let captured =
            CapturedFile::capture(attachment.reopen_verified().map_err(|_| ())?, relative)?;
        if captured.size != spec.size {
            return Err(());
        }
        let matches = match spec.algorithm {
            "sha256" => captured.sha256 == parse_sha256(spec.digest)?,
            "git-sha1" => captured.git_sha1 == parse_git_sha1(spec.digest)?,
            _ => false,
        };
        if !matches {
            return Err(());
        }
        sizes.push(captured.size);
        captured_files.push(captured);
    }
    validate_remote_budget(&sizes)?;
    Ok(captured_files)
}

fn verify_ready_tree(root: &Path, entries: &[IntegrityEntry]) -> Result<(), ()> {
    let mut remaining_files = BTreeSet::from([
        crate::GAIA_REVISION_MARKER.to_owned(),
        GAIA_READY_MARKER.to_owned(),
        GAIA_INTEGRITY_MANIFEST.to_owned(),
        GAIA_PARQUET_PATH.to_owned(),
    ]);
    remaining_files.extend(entries.iter().map(|entry| entry.path.clone()));
    if remaining_files.len() != entries.len() + 4 {
        return Err(());
    }

    let mut allowed_directories = BTreeSet::new();
    for file in &remaining_files {
        let mut parent = Path::new(file).parent();
        while let Some(path) = parent {
            if path.as_os_str().is_empty() {
                break;
            }
            allowed_directories.insert(manifest_path(path)?);
            parent = path.parent();
        }
    }

    let mut pending = vec![root.to_path_buf()];
    let mut file_count = 0_usize;
    let mut total_bytes = 0_u64;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).map_err(|_| ())? {
            let entry = entry.map_err(|_| ())?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|_| ())?;
            if is_link_or_reparse(&metadata) {
                return Err(());
            }
            let relative = path.strip_prefix(root).map_err(|_| ())?;
            let rendered = manifest_path(relative)?;
            if metadata.is_dir() {
                if !allowed_directories.contains(&rendered) {
                    return Err(());
                }
                pending.push(path);
            } else if metadata.is_file() {
                if !remaining_files.remove(&rendered) {
                    return Err(());
                }
                file_count = file_count.checked_add(1).ok_or(())?;
                total_bytes = total_bytes.checked_add(metadata.len()).ok_or(())?;
            } else {
                return Err(());
            }
        }
    }
    if !remaining_files.is_empty()
        || file_count > MAX_TRANSFER_FILES + 3
        || total_bytes > MAX_TOTAL_BYTES + MAX_INTEGRITY_MANIFEST_BYTES + 1024
    {
        return Err(());
    }
    Ok(())
}

fn verify_trusted_source_tree(root: &Path, trusted: &[TrustedAttachmentSpec]) -> Result<(), ()> {
    let mut remaining_files = BTreeSet::from([
        crate::GAIA_REVISION_MARKER.to_owned(),
        GAIA_PARQUET_PATH.to_owned(),
    ]);
    for spec in trusted {
        let path = manifest_path(Path::new(spec.path))?;
        if path != spec.path || !remaining_files.insert(path) {
            return Err(());
        }
    }
    if remaining_files.len() != trusted.len() + 2 {
        return Err(());
    }

    let mut allowed_directories = BTreeSet::new();
    for file in &remaining_files {
        let mut parent = Path::new(file).parent();
        while let Some(path) = parent {
            if path.as_os_str().is_empty() {
                break;
            }
            allowed_directories.insert(manifest_path(path)?);
            parent = path.parent();
        }
    }

    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).map_err(|_| ())? {
            let entry = entry.map_err(|_| ())?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|_| ())?;
            if is_link_or_reparse(&metadata) {
                return Err(());
            }
            let rendered = manifest_path(path.strip_prefix(root).map_err(|_| ())?)?;
            if metadata.is_dir() {
                if !allowed_directories.contains(&rendered) {
                    return Err(());
                }
                pending.push(path);
            } else if metadata.is_file() {
                if !remaining_files.remove(&rendered) {
                    return Err(());
                }
            } else {
                return Err(());
            }
        }
    }
    if !remaining_files.is_empty() {
        return Err(());
    }
    Ok(())
}

fn hex_digest<const N: usize>(digest: &[u8; N]) -> String {
    let mut encoded = String::with_capacity(N * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

struct CapturedFile {
    file: File,
    relative: PathBuf,
    size: u64,
    sha256: [u8; 32],
    git_sha1: [u8; 20],
}

impl CapturedFile {
    fn capture(mut file: File, relative: PathBuf) -> Result<Self, ()> {
        validate_relative(&relative)?;
        let metadata = file.metadata().map_err(|_| ())?;
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
            return Err(());
        }
        file.seek(std::io::SeekFrom::Start(0)).map_err(|_| ())?;
        let (copied, sha256, git_sha1) = capture_stream_digests(&mut file, metadata.len())?;
        if copied != metadata.len() {
            return Err(());
        }
        file.seek(std::io::SeekFrom::Start(0)).map_err(|_| ())?;
        Ok(Self {
            file,
            relative,
            size: copied,
            sha256,
            git_sha1,
        })
    }

    fn capture_expected(
        file: File,
        relative: PathBuf,
        size: u64,
        sha256: [u8; 32],
    ) -> Result<Self, ()> {
        let captured = Self::capture(file, relative)?;
        if captured.size != size || captured.sha256 != sha256 {
            return Err(());
        }
        Ok(captured)
    }

    fn copy_to(mut self, destination: &Path) -> Result<(), ()> {
        create_private_parent_directories(destination.parent().ok_or(())?)?;
        let mut output = create_private_file(destination)?;
        self.file
            .seek(std::io::SeekFrom::Start(0))
            .map_err(|_| ())?;
        let mut hasher = Sha256::new();
        let mut copied = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = self.file.read(&mut buffer).map_err(|_| ())?;
            if read == 0 {
                break;
            }
            copied = copied.checked_add(read as u64).ok_or(())?;
            if copied > MAX_FILE_BYTES {
                let _ = fs::remove_file(destination);
                return Err(());
            }
            hasher.update(&buffer[..read]);
            output.write_all(&buffer[..read]).map_err(|_| ())?;
        }
        if copied != self.size || <[u8; 32]>::from(hasher.finalize()) != self.sha256 {
            drop(output);
            let _ = fs::remove_file(destination);
            return Err(());
        }
        output.sync_all().map_err(|_| ())
    }
}

fn capture_stream_digests(
    reader: &mut impl Read,
    expected_size: u64,
) -> Result<(u64, [u8; 32], [u8; 20]), ()> {
    if expected_size > MAX_FILE_BYTES {
        return Err(());
    }
    let mut sha256 = Sha256::new();
    let mut git_sha1 = Sha1::new();
    git_sha1.update(format!("blob {expected_size}\0").as_bytes());
    let copied = std::io::copy(
        &mut reader.take(MAX_FILE_BYTES + 1),
        &mut DigestingWriter::new(&mut sha256, &mut git_sha1),
    )
    .map_err(|_| ())?;
    if copied != expected_size {
        return Err(());
    }
    Ok((copied, sha256.finalize().into(), git_sha1.finalize().into()))
}

fn capture_dataset_files(
    dataset: &GaiaDataset,
    parquet_size: u64,
    parquet_digest: [u8; 32],
) -> Result<Vec<CapturedFile>, ()> {
    let parquet = File::open(dataset.snapshot_root().join(GAIA_PARQUET_PATH)).map_err(|_| ())?;
    let mut captured = vec![CapturedFile::capture_expected(
        parquet,
        PathBuf::from(GAIA_PARQUET_PATH),
        parquet_size,
        parquet_digest,
    )?];
    for row in dataset.rows() {
        if let Some(attachment) = row.attachment() {
            let relative = attachment
                .path()
                .strip_prefix(dataset.snapshot_root())
                .map_err(|_| ())?
                .to_path_buf();
            captured.push(CapturedFile::capture(
                attachment.reopen_verified().map_err(|_| ())?,
                relative,
            )?);
        }
    }
    validate_remote_budget(&captured.iter().map(|file| file.size).collect::<Vec<_>>())?;
    Ok(captured)
}

fn trusted_attachment_paths(root: &Path) -> Result<Vec<PathBuf>, ()> {
    let reader =
        SerializedFileReader::new(File::open(root.join(GAIA_PARQUET_PATH)).map_err(|_| ())?)
            .map_err(|_| ())?;
    let mut paths = HashSet::new();
    for row in reader.get_row_iter(None).map_err(|_| ())? {
        let row = row.map_err(|_| ())?;
        let field = row
            .into_columns()
            .into_iter()
            .find(|(name, _)| name == "file_path")
            .ok_or(())?
            .1;
        match field {
            Field::Str(value) if !value.is_empty() => {
                let path = PathBuf::from(value);
                validate_relative(&path)?;
                paths.insert(path);
            }
            Field::Str(_) | Field::Null => {}
            _ => return Err(()),
        }
    }
    Ok(paths.into_iter().collect())
}

fn validate_preflight_identity(
    requested: &[PathBuf],
    metadata: &[SnapshotFileMetadata],
) -> Result<(), ()> {
    let requested = requested
        .iter()
        .map(PathBuf::as_path)
        .collect::<HashSet<_>>();
    let returned = metadata
        .iter()
        .map(|entry| entry.remote_path())
        .collect::<HashSet<_>>();
    if requested.len() != metadata.len() || requested != returned {
        return Err(());
    }
    Ok(())
}

fn validate_remote_budget(sizes: &[u64]) -> Result<(), ()> {
    if sizes.len() > MAX_TRANSFER_FILES || sizes.iter().any(|size| *size > MAX_FILE_BYTES) {
        return Err(());
    }
    let total = sizes
        .iter()
        .try_fold(0_u64, |total, size| total.checked_add(*size))
        .ok_or(())?;
    if total > MAX_TOTAL_BYTES {
        return Err(());
    }
    Ok(())
}

fn validate_snapshot_budget(root: &Path) -> Result<(), ()> {
    let mut pending = vec![root.to_path_buf()];
    let mut sizes = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).map_err(|_| ())? {
            let entry = entry.map_err(|_| ())?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|_| ())?;
            if is_link_or_reparse(&metadata) {
                return Err(());
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                if path
                    .file_name()
                    .is_some_and(|name| name == crate::GAIA_REVISION_MARKER)
                {
                    continue;
                }
                sizes.push(metadata.len());
            } else {
                return Err(());
            }
        }
    }
    validate_remote_budget(&sizes)
}

fn publish_reserved<F>(
    ready: &Path,
    captured: Vec<CapturedFile>,
    final_verify: F,
) -> Result<GaiaAcquisition, GaiaFetchError>
where
    F: FnOnce(&Path) -> Result<GaiaAcquisition, GaiaFetchError>,
{
    let manifest = build_integrity_manifest(&captured).map_err(|_| GaiaFetchError::VerifyFailed)?;
    let manifest_digest: [u8; 32] = Sha256::digest(&manifest).into();
    let reservation = ReadyDirectory::reserve(ready)?;
    write_private_file(
        ready.join(crate::GAIA_REVISION_MARKER),
        GAIA_DATASET_REVISION.as_bytes(),
    )
    .map_err(|_| GaiaFetchError::VerifyFailed)?;
    for file in captured {
        let destination = ready.join(&file.relative);
        file.copy_to(&destination)
            .map_err(|_| GaiaFetchError::VerifyFailed)?;
    }
    write_private_file(ready.join(GAIA_INTEGRITY_MANIFEST), &manifest)
        .map_err(|_| GaiaFetchError::VerifyFailed)?;
    write_ready_marker(ready, manifest_digest)?;
    let acquisition = final_verify(ready)?;
    reservation.disarm();
    Ok(acquisition)
}

fn validate_relative(path: &Path) -> Result<(), ()> {
    if path.as_os_str().is_empty() || path.as_os_str().len() > 1024 {
        return Err(());
    }
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(());
    }
    Ok(())
}

fn secure_directory(path: &Path) -> Result<PathBuf, ()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ())?;
    if !metadata.is_dir() || is_link_or_reparse(&metadata) {
        return Err(());
    }
    path.canonicalize().map_err(|_| ())
}

fn secure_private_acquisition_directory(path: &Path) -> Result<PathBuf, ()> {
    let canonical = secure_directory(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&canonical)
            .map_err(|_| ())?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err(());
        }
    }
    #[cfg(windows)]
    {
        let profile = std::env::var_os("USERPROFILE").ok_or(())?;
        let profile = PathBuf::from(profile).canonicalize().map_err(|_| ())?;
        if !canonical.starts_with(profile) {
            return Err(());
        }
        benchmark_core::apply_windows_private_acl(&canonical, true)?;
    }
    Ok(canonical)
}

fn overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

#[cfg(unix)]
pub(crate) fn is_link_or_reparse(metadata: &Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(windows)]
pub(crate) fn is_link_or_reparse(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn is_link_or_reparse(metadata: &Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(unix)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct ObjectIdentity {
    device: u64,
    inode: u64,
}

#[cfg(windows)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct ObjectIdentity {
    volume: u32,
    index: u64,
}

#[cfg(not(any(unix, windows)))]
#[derive(Clone, Copy, PartialEq, Eq)]
struct ObjectIdentity;

#[cfg(unix)]
fn path_identity(path: &Path) -> Result<ObjectIdentity, ()> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path).map_err(|_| ())?;
    Ok(ObjectIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(windows)]
fn path_identity(path: &Path) -> Result<ObjectIdentity, ()> {
    use std::mem::MaybeUninit;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, GetFileInformationByHandle, OPEN_EXISTING,
    };

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(());
    }
    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    let succeeded = unsafe { GetFileInformationByHandle(handle, information.as_mut_ptr()) };
    unsafe { CloseHandle(handle) };
    if succeeded == 0 {
        return Err(());
    }
    let information = unsafe { information.assume_init() };
    let identity = ObjectIdentity {
        volume: information.dwVolumeSerialNumber,
        index: (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    };
    if identity.index == 0 {
        return Err(());
    }
    Ok(identity)
}

#[cfg(not(any(unix, windows)))]
fn path_identity(_path: &Path) -> Result<ObjectIdentity, ()> {
    Ok(ObjectIdentity)
}

struct AcquisitionLock {
    file: File,
}

impl AcquisitionLock {
    fn claim(root: &Path) -> Result<Self, GaiaFetchError> {
        let path = root.join(".pinvou-gaia-acquire.lock");
        let file = open_private_lock_file(&path).map_err(|_| GaiaFetchError::VerifyFailed)?;
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => {}
            Err(error) if error.kind() == fs2::lock_contended_error().kind() => {
                return Err(GaiaFetchError::Busy);
            }
            Err(_) => return Err(GaiaFetchError::VerifyFailed),
        }
        Ok(Self { file })
    }
}

impl Drop for AcquisitionLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn open_private_lock_file(path: &Path) -> Result<File, ()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() || is_link_or_reparse(&metadata) => {
            return Err(());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(()),
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(|_| ())?;
    set_private_file_permissions(path)?;
    Ok(file)
}

struct OwnedDirectory {
    parent: PathBuf,
    parent_identity: ObjectIdentity,
    path: PathBuf,
    identity: ObjectIdentity,
    armed: std::cell::Cell<bool>,
}

impl OwnedDirectory {
    fn create(parent: &Path, path: PathBuf) -> Result<Self, ()> {
        if path.parent() != Some(parent) {
            return Err(());
        }
        let parent_identity = path_identity(parent)?;
        fs::create_dir(&path).map_err(|_| ())?;
        set_private_directory_permissions(&path)?;
        let metadata = fs::symlink_metadata(&path).map_err(|_| ())?;
        if !metadata.is_dir() || is_link_or_reparse(&metadata) {
            return Err(());
        }
        let identity = path_identity(&path)?;
        Ok(Self {
            parent: parent.to_path_buf(),
            parent_identity,
            path,
            identity,
            armed: std::cell::Cell::new(true),
        })
    }

    fn adopt_existing(parent: &Path, path: PathBuf) -> Result<Self, ()> {
        if path.parent() != Some(parent) {
            return Err(());
        }
        let parent_identity = path_identity(parent)?;
        let metadata = fs::symlink_metadata(&path).map_err(|_| ())?;
        if !metadata.is_dir() || is_link_or_reparse(&metadata) {
            return Err(());
        }
        let identity = path_identity(&path)?;
        Ok(Self {
            parent: parent.to_path_buf(),
            parent_identity,
            path,
            identity,
            armed: std::cell::Cell::new(true),
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn disarm(&self) {
        self.armed.set(false);
    }
}

fn remove_identity_owned_directory(parent: &Path, path: &Path) -> Result<(), GaiaFetchError> {
    let owned = OwnedDirectory::adopt_existing(parent, path.to_path_buf())
        .map_err(|_| GaiaFetchError::VerifyFailed)?;
    drop(owned);
    if path.exists() {
        return Err(GaiaFetchError::VerifyFailed);
    }
    Ok(())
}

impl Drop for OwnedDirectory {
    fn drop(&mut self) {
        if !self.armed.get()
            || self.path.parent() != Some(self.parent.as_path())
            || path_identity(&self.parent).ok() != Some(self.parent_identity)
        {
            return;
        }
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) if metadata.is_dir() && !is_link_or_reparse(&metadata) => metadata,
            _ => return,
        };
        let _ = metadata;
        if path_identity(&self.path).ok() == Some(self.identity) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

struct StagingDirectory {
    owned: OwnedDirectory,
}

impl StagingDirectory {
    fn create(root: &Path) -> Result<Self, GaiaFetchError> {
        for _ in 0..16 {
            let path = root.join(format!(".pinvou-gaia-tmp-{:016x}", random::<u64>()));
            if path.exists() {
                continue;
            }
            match OwnedDirectory::create(root, path) {
                Ok(owned) => return Ok(Self { owned }),
                Err(_) => return Err(GaiaFetchError::VerifyFailed),
            }
        }
        Err(GaiaFetchError::VerifyFailed)
    }

    fn path(&self) -> &Path {
        self.owned.path()
    }
}

struct ReadyDirectory {
    owned: OwnedDirectory,
}

impl ReadyDirectory {
    fn reserve(path: &Path) -> Result<Self, GaiaFetchError> {
        let parent = path.parent().ok_or(GaiaFetchError::VerifyFailed)?;
        let owned = OwnedDirectory::create(parent, path.to_path_buf())
            .map_err(|_| GaiaFetchError::VerifyFailed)?;
        Ok(Self { owned })
    }

    fn disarm(&self) {
        self.owned.disarm();
    }
}

#[cfg(test)]
mod review_contract_tests {
    use super::*;
    use std::sync::Arc;
    use std::time::SystemTime;

    use arrow_array::{ArrayRef, Int64Array, RecordBatch, StringArray, StructArray};
    use arrow_schema::{DataType, Field as ArrowField, Fields, Schema};
    use parquet::arrow::ArrowWriter;

    struct DenyDownloader;

    impl SnapshotDownloader for DenyDownloader {
        fn preflight(
            &self,
            _request: &SnapshotPreflightRequest<'_>,
        ) -> Result<Vec<SnapshotFileMetadata>, SnapshotFetchFailure> {
            Err(SnapshotFetchFailure)
        }

        fn download(
            &self,
            _request: &SnapshotDownloadRequest<'_>,
            _destination: &Path,
        ) -> Result<(), SnapshotFetchFailure> {
            Err(SnapshotFetchFailure)
        }
    }

    fn test_directory(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "pinvou-gaia-fetch-unit-{label}-{}-{:016x}",
            std::process::id(),
            random::<u64>()
        ));
        fs::create_dir_all(&path).unwrap();
        set_private_directory_permissions(&path).unwrap();
        path
    }

    fn write_synthetic_snapshot(root: &Path) -> (u64, [u8; 32]) {
        write_synthetic_snapshot_with_attachment(root, true)
    }

    fn write_synthetic_snapshot_with_attachment(
        root: &Path,
        with_attachment: bool,
    ) -> (u64, [u8; 32]) {
        fs::create_dir_all(root.join("2023/validation")).unwrap();
        if with_attachment {
            fs::write(root.join("2023/validation/input.bin"), [0_u8; 8]).unwrap();
        }
        fs::write(
            root.join(crate::GAIA_REVISION_MARKER),
            GAIA_DATASET_REVISION,
        )
        .unwrap();
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
                .map(|_| Arc::new(StringArray::from(vec![Some("synthetic")])) as ArrayRef)
                .collect(),
            None,
        ));
        let columns: Vec<(&str, ArrayRef)> = vec![
            ("task_id", Arc::new(StringArray::from(vec!["safe-task-1"]))),
            (
                "Question",
                Arc::new(StringArray::from(vec!["PRIVATE_QUESTION_SENTINEL"])),
            ),
            ("Level", Arc::new(Int64Array::from(vec![1]))),
            (
                "Final answer",
                Arc::new(StringArray::from(vec!["PRIVATE_REFERENCE_SENTINEL"])),
            ),
            (
                "file_name",
                Arc::new(StringArray::from(vec![
                    with_attachment.then_some("input.bin"),
                ])),
            ),
            (
                "file_path",
                Arc::new(StringArray::from(vec![
                    with_attachment.then_some("2023/validation/input.bin"),
                ])),
            ),
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
        let path = root.join(GAIA_PARQUET_PATH);
        let file = File::create(&path).unwrap();
        let mut writer = ArrowWriter::try_new(file, batch.schema(), None).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
        let bytes = fs::read(path).unwrap();
        (bytes.len() as u64, Sha256::digest(bytes).into())
    }

    #[test]
    fn fetch_public_surface_has_no_arbitrary_digest_ready_helpers() {
        let public_surface = include_str!("lib.rs");
        assert!(!public_surface.contains("acquire_with_expected_parquet_for_tests"));
        assert!(!public_surface.contains("verify_offline_with_expected_parquet_for_tests"));
    }

    #[test]
    fn fetch_remote_budget_rejects_oversized_file_total_and_count_before_download() {
        assert!(validate_remote_budget(&[MAX_FILE_BYTES + 1]).is_err());
        assert!(validate_remote_budget(&[MAX_FILE_BYTES; 13]).is_err());
        assert!(validate_remote_budget(&vec![1; MAX_TRANSFER_FILES + 1]).is_err());
        assert!(validate_remote_budget(&[GAIA_PARQUET_SIZE, 20]).is_ok());
    }

    #[test]
    fn fetch_integrity_manifest_rejects_any_blank_jsonl_record() {
        assert!(parse_integrity_manifest(b"\n").is_err());
        assert!(parse_integrity_manifest(b"\n\n").is_err());
    }

    #[test]
    fn fetch_downloaded_file_rejects_growth_digest_mismatch_and_cumulative_overflow() {
        let root = test_directory("download-integrity");
        let path = root.join("payload");
        fs::write(&path, b"trusted").unwrap();
        let digest: [u8; 32] = Sha256::digest(b"trusted").into();
        let metadata = SnapshotFileMetadata::new("payload", 7, digest);
        let mut total = 0;
        verify_downloaded_file(&path, &metadata, &mut total).unwrap();
        assert_eq!(total, 7);

        fs::write(&path, b"trusted-grow").unwrap();
        assert!(verify_downloaded_file(&path, &metadata, &mut 0).is_err());
        fs::write(&path, b"TRUSTED").unwrap();
        assert!(verify_downloaded_file(&path, &metadata, &mut 0).is_err());
        fs::write(&path, b"trusted").unwrap();
        let mut almost_full = MAX_TOTAL_BYTES - 6;
        assert!(verify_downloaded_file(&path, &metadata, &mut almost_full).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn fetch_stream_stops_at_expected_limit_and_never_publishes_oversize() {
        struct CountingReader {
            remaining: usize,
            read: Arc<std::sync::atomic::AtomicUsize>,
        }
        impl Read for CountingReader {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                let count = buffer.len().min(self.remaining);
                buffer[..count].fill(b'x');
                self.remaining -= count;
                self.read
                    .fetch_add(count, std::sync::atomic::Ordering::Relaxed);
                Ok(count)
            }
        }
        let root = test_directory("bounded-stream");
        let destination = root.join("destination");
        let read = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let expected = SnapshotFileMetadata::new("payload", 7, Sha256::digest(b"trusted").into());
        let reader = CountingReader {
            remaining: 1024 * 1024,
            read: Arc::clone(&read),
        };

        assert!(stream_verified_file(reader, None, &destination, &expected, 100).is_err());
        assert!(read.load(std::sync::atomic::Ordering::Relaxed) <= 8);
        assert!(!destination.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn fetch_metadata_body_is_bounded_and_requires_content_digest() {
        let valid_digest = "11".repeat(32);
        let body = format!(
            r#"{{"sha":"{GAIA_DATASET_REVISION}","siblings":[{{"rfilename":"README.md","size":3}},{{"rfilename":"safe/file","size":7,"lfs":{{"oid":"sha256:{valid_digest}","size":7}}}}]}}"#
        );
        let requested = HashSet::from(["safe/file".to_owned()]);
        let parsed = parse_hf_metadata(body.as_bytes(), &requested).unwrap();
        assert_eq!(parsed.siblings.len(), 1);
        assert_eq!(parsed.siblings[0].expected_sha256(), Some(&[0x11; 32]));

        let no_lfs_or_blob_id = format!(
            r#"{{"sha":"{GAIA_DATASET_REVISION}","siblings":[{{"rfilename":"safe/file","size":7}}]}}"#
        );
        assert!(parse_hf_metadata(no_lfs_or_blob_id.as_bytes(), &requested).is_err());
        let size_mismatch = format!(
            r#"{{"sha":"{GAIA_DATASET_REVISION}","siblings":[{{"rfilename":"safe/file","size":7,"lfs":{{"oid":"sha256:{valid_digest}","size":8}}}}]}}"#
        );
        assert!(parse_hf_metadata(size_mismatch.as_bytes(), &requested).is_err());
        assert!(
            parse_hf_metadata(
                std::io::repeat(b'x').take(MAX_METADATA_BODY_BYTES + 1),
                &requested,
            )
            .is_err()
        );
    }

    #[test]
    fn fetch_non_lfs_blob_id_accepts_exact_content_and_rejects_same_size_tampering() {
        let requested = HashSet::from(["safe/file".to_owned()]);
        let body = format!(
            r#"{{"sha":"{GAIA_DATASET_REVISION}","siblings":[{{"rfilename":"safe/file","size":7,"blobId":"2a190ee76159c1b3cc6a437daa73d594b339cbdf"}}]}}"#
        );
        let parsed = parse_hf_metadata(body.as_bytes(), &requested).unwrap();
        let expected = &parsed.siblings[0];
        let root = test_directory("git-blob-integrity");
        let trusted = root.join("trusted");
        stream_verified_file(b"trusted".as_slice(), Some(7), &trusted, expected, 7).unwrap();

        let tampered = root.join("tampered");
        assert!(
            stream_verified_file(b"TRUSTED".as_slice(), Some(7), &tampered, expected, 7).is_err()
        );
        assert!(!tampered.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[allow(deprecated)]
    fn fetch_size_only_metadata_remains_source_compatible_but_cannot_publish_content() {
        let metadata = SnapshotFileMetadata::new_without_digest("payload", 7);
        assert_eq!(metadata.expected_sha256(), None);
        let root = test_directory("size-only-rejected");
        let destination = root.join("payload");
        assert!(
            stream_verified_file(b"trusted".as_slice(), Some(7), &destination, &metadata, 7,)
                .is_err()
        );
        assert!(!destination.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn fetch_windows_acl_policy_and_roundtrip_are_private() {
        let directory = test_directory("acl-roundtrip");
        let file = directory.join("private-file");
        fs::write(&file, b"private").unwrap();

        benchmark_core::apply_windows_private_acl(&directory, true).unwrap();
        benchmark_core::apply_windows_private_acl(&file, false).unwrap();
        assert_eq!(
            benchmark_core::windows_private_acl_ace_flags(true),
            windows_sys::Win32::Security::CONTAINER_INHERIT_ACE
                | windows_sys::Win32::Security::OBJECT_INHERIT_ACE
        );
        assert_eq!(benchmark_core::windows_private_acl_ace_flags(false), 0);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn fetch_captured_file_uses_original_handle_or_rejects_in_place_change() {
        let root = test_directory("captured-file");
        let source = root.join("source");
        fs::write(&source, b"original").unwrap();
        let captured =
            CapturedFile::capture(File::open(&source).unwrap(), "payload".into()).unwrap();
        let renamed = root.join("renamed");
        fs::rename(&source, &renamed).unwrap();
        fs::write(&source, b"replaced").unwrap();
        let destination = root.join("destination");
        captured.copy_to(&destination).unwrap();
        assert_eq!(fs::read(destination).unwrap(), b"original");

        let mutable = root.join("mutable");
        fs::write(&mutable, b"before!!").unwrap();
        let captured =
            CapturedFile::capture(File::open(&mutable).unwrap(), "mutable".into()).unwrap();
        fs::write(&mutable, b"after!!!").unwrap();
        assert!(captured.copy_to(&root.join("rejected")).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn fetch_capture_hashes_sha256_and_git_blob_in_one_stream_pass() {
        struct SwitchingReader {
            first: std::io::Cursor<&'static [u8]>,
            switched: bool,
            reads_after_switch: usize,
        }
        impl Read for SwitchingReader {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if self.switched {
                    self.reads_after_switch += 1;
                    let replacement = b"TRUSTED";
                    let count = replacement.len().min(buffer.len());
                    buffer[..count].copy_from_slice(&replacement[..count]);
                    return Ok(count);
                }
                let read = self.first.read(buffer)?;
                if read == 0 {
                    self.switched = true;
                }
                Ok(read)
            }
        }

        let mut reader = SwitchingReader {
            first: std::io::Cursor::new(b"trusted"),
            switched: false,
            reads_after_switch: 0,
        };
        let (size, sha256, git_sha1) = capture_stream_digests(&mut reader, 7).unwrap();
        assert_eq!(size, 7);
        assert_eq!(sha256, <[u8; 32]>::from(Sha256::digest(b"trusted")));
        assert_eq!(
            git_sha1,
            parse_git_sha1("2a190ee76159c1b3cc6a437daa73d594b339cbdf").unwrap()
        );
        assert_eq!(reader.reads_after_switch, 0);
    }

    #[test]
    fn fetch_cleanup_guard_does_not_delete_replacement_directory() {
        let root = test_directory("guard-identity");
        let guard = StagingDirectory::create(&root).unwrap();
        let original = root.join("original-owned-directory");
        fs::rename(guard.path(), &original).unwrap();
        fs::create_dir(guard.path()).unwrap();
        fs::write(guard.path().join("replacement-sentinel"), b"keep").unwrap();
        let replacement = guard.path().to_path_buf();
        drop(guard);
        assert_eq!(
            fs::read(replacement.join("replacement-sentinel")).unwrap(),
            b"keep"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn fetch_failed_final_verification_removes_reserved_ready_directory() {
        let root = std::env::temp_dir().join(format!(
            "pinvou-gaia-publish-contract-{}-{:016x}",
            std::process::id(),
            random::<u64>()
        ));
        let staging = root.join("staging");
        let ready = root.join("ready");
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("payload"), b"verified staging").unwrap();
        let captured = CapturedFile::capture(
            File::open(staging.join("payload")).unwrap(),
            PathBuf::from("payload"),
        )
        .unwrap();

        let result = publish_reserved(&ready, vec![captured], |_| {
            Err(GaiaFetchError::VerifyFailed)
        });

        assert_eq!(result.unwrap_err(), GaiaFetchError::VerifyFailed);
        assert!(!ready.exists());
        assert!(!ready.join(GAIA_READY_MARKER).exists());
        assert!(staging.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn fetch_synthetic_acquisition_publishes_ready_then_verifies_offline() {
        let source = test_directory("source");
        let acquisition = test_directory("acquisition");
        let worktree = test_directory("worktree");
        let (size, digest) = write_synthetic_snapshot(&source);
        let manager = GaiaSnapshotManager::new(&acquisition, &worktree, DenyDownloader).unwrap();
        let dataset = verify_dataset(&source, size, digest).unwrap();
        let captured = capture_dataset_files(&dataset, size, digest).unwrap();
        let ready = publish_reserved(&manager.ready_root(), captured, |root| {
            manager.verify_ready(root, size, digest)
        })
        .unwrap();
        let marker = fs::read_to_string(ready.snapshot_root().join(GAIA_READY_MARKER)).unwrap();
        let manifest = fs::read(ready.snapshot_root().join(GAIA_INTEGRITY_MANIFEST)).unwrap();
        assert_eq!(
            marker,
            ready_marker_contents(Sha256::digest(manifest).into())
        );
        let offline = manager
            .verify_ready(ready.snapshot_root(), size, digest)
            .unwrap();
        assert_eq!(offline.snapshot_root(), ready.snapshot_root());
        assert_eq!(offline.revision(), GAIA_DATASET_REVISION);
        let attachment = offline.dataset().rows()[0].attachment().unwrap();
        let mut opened = attachment.reopen_verified().unwrap();
        assert_eq!(opened.stream_position().unwrap(), 0);
        drop(opened);

        fs::write(ready.snapshot_root().join("unexpected.bin"), [9_u8; 1]).unwrap();
        assert_eq!(
            manager
                .verify_ready(ready.snapshot_root(), size, digest)
                .unwrap_err(),
            GaiaFetchError::VerifyFailed
        );
        fs::remove_file(ready.snapshot_root().join("unexpected.bin")).unwrap();

        fs::write(
            ready.snapshot_root().join("2023/validation/input.bin"),
            [1_u8; 8],
        )
        .unwrap();
        assert!(attachment.reopen_verified().is_err());
        assert_eq!(
            manager
                .verify_ready(ready.snapshot_root(), size, digest)
                .unwrap_err(),
            GaiaFetchError::VerifyFailed
        );

        let _ = fs::remove_dir_all(source);
        let _ = fs::remove_dir_all(acquisition);
        let _ = fs::remove_dir_all(worktree);
    }

    #[test]
    fn fetch_import_rejects_untrusted_attachment_snapshot_before_self_signing() {
        let source = test_directory("untrusted-import-source");
        let acquisition = test_directory("untrusted-import-acquisition");
        let worktree = test_directory("untrusted-import-worktree");
        let (size, digest) = write_synthetic_snapshot(&source);
        fs::write(source.join("2023/validation/input.bin"), [7_u8; 8]).unwrap();
        let manager = GaiaSnapshotManager::new(&acquisition, &worktree, DenyDownloader).unwrap();

        assert_eq!(
            manager
                .acquire_expected(GaiaSource::ExistingSnapshot(source.clone()), size, digest)
                .unwrap_err(),
            GaiaFetchError::ImportFailed
        );
        assert!(!manager.ready_root().exists());

        let _ = fs::remove_dir_all(source);
        let _ = fs::remove_dir_all(acquisition);
        let _ = fs::remove_dir_all(worktree);
    }

    #[test]
    fn fetch_import_anchor_accepts_exact_attachment_shape_and_rejects_same_size_tamper() {
        let source = test_directory("anchored-import-source");
        let (size, digest) = write_synthetic_snapshot(&source);
        let dataset = verify_dataset(&source, size, digest).unwrap();
        let trusted = [TrustedAttachmentSpec {
            path: "2023/validation/input.bin",
            size: 8,
            algorithm: "sha256",
            digest: "af5570f5a1810b7af78caf4bc70a660f0df51e42baf91d4de5b2328de0e83dfc",
        }];
        capture_trusted_import_attachments(&dataset, size, &trusted).unwrap();

        fs::write(source.join("2023/validation/input.bin"), [4_u8; 8]).unwrap();
        assert!(capture_trusted_import_attachments(&dataset, size, &trusted).is_err());
        let _ = fs::remove_dir_all(source);
    }

    #[test]
    fn fetch_trusted_source_verification_is_read_only_and_binds_attachments() {
        let source = test_directory("trusted-read-only-source");
        let acquisition = test_directory("trusted-read-only-acquisition");
        let worktree = test_directory("trusted-read-only-worktree");
        let (size, digest) = write_synthetic_snapshot(&source);
        let trusted = [TrustedAttachmentSpec {
            path: "2023/validation/input.bin",
            size: 8,
            algorithm: "sha256",
            digest: "af5570f5a1810b7af78caf4bc70a660f0df51e42baf91d4de5b2328de0e83dfc",
        }];
        let manager = GaiaSnapshotManager::new(&acquisition, &worktree, DenyDownloader).unwrap();
        let before = source_tree_metadata(&source);

        let verified = manager
            .verify_source_expected(&source, size, digest, &trusted)
            .unwrap();

        assert_eq!(verified.snapshot_root(), source.canonicalize().unwrap());
        assert!(
            verified.dataset().rows()[0]
                .attachment()
                .unwrap()
                .reopen_verified()
                .is_ok()
        );
        assert_eq!(source_tree_metadata(&source), before);
        assert!(!source.join(GAIA_READY_MARKER).exists());
        assert!(!source.join(GAIA_INTEGRITY_MANIFEST).exists());

        let _ = fs::remove_dir_all(source);
        let _ = fs::remove_dir_all(acquisition);
        let _ = fs::remove_dir_all(worktree);
    }

    #[test]
    fn fetch_trusted_source_rejects_same_size_attachment_tampering_and_extra_files() {
        let source = test_directory("trusted-tampered-source");
        let acquisition = test_directory("trusted-tampered-acquisition");
        let worktree = test_directory("trusted-tampered-worktree");
        let (size, digest) = write_synthetic_snapshot(&source);
        let trusted = [TrustedAttachmentSpec {
            path: "2023/validation/input.bin",
            size: 8,
            algorithm: "sha256",
            digest: "af5570f5a1810b7af78caf4bc70a660f0df51e42baf91d4de5b2328de0e83dfc",
        }];
        let manager = GaiaSnapshotManager::new(&acquisition, &worktree, DenyDownloader).unwrap();

        fs::write(source.join("2023/validation/input.bin"), [9_u8; 8]).unwrap();
        assert_eq!(
            manager
                .verify_source_expected(&source, size, digest, &trusted)
                .unwrap_err(),
            GaiaFetchError::VerifyFailed
        );

        fs::write(source.join("2023/validation/input.bin"), [0_u8; 8]).unwrap();
        fs::write(source.join("unexpected.bin"), [0_u8; 1]).unwrap();
        assert_eq!(
            manager
                .verify_source_expected(&source, size, digest, &trusted)
                .unwrap_err(),
            GaiaFetchError::VerifyFailed
        );

        let _ = fs::remove_dir_all(source);
        let _ = fs::remove_dir_all(acquisition);
        let _ = fs::remove_dir_all(worktree);
    }

    fn source_tree_metadata(root: &Path) -> Vec<(PathBuf, bool, u64, SystemTime)> {
        let mut pending = vec![root.to_path_buf()];
        let mut entries = Vec::new();
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(&directory).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                let metadata = fs::symlink_metadata(&path).unwrap();
                entries.push((
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    metadata.is_dir(),
                    metadata.len(),
                    metadata.modified().unwrap(),
                ));
                if metadata.is_dir() {
                    pending.push(path);
                }
            }
        }
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        entries
    }

    #[test]
    fn fetch_acquire_recovers_identity_owned_partial_ready_after_lock() {
        let source = test_directory("partial-ready-source");
        let acquisition = test_directory("partial-ready-acquisition");
        let worktree = test_directory("partial-ready-worktree");
        let (size, digest) = write_synthetic_snapshot_with_attachment(&source, false);
        let manager = GaiaSnapshotManager::new(&acquisition, &worktree, DenyDownloader).unwrap();
        fs::create_dir(manager.ready_root()).unwrap();
        set_private_directory_permissions(&manager.ready_root()).unwrap();
        fs::write(manager.ready_root().join("partial"), [0_u8; 1]).unwrap();

        let ready = manager
            .acquire_expected(GaiaSource::ExistingSnapshot(source.clone()), size, digest)
            .unwrap();
        manager
            .verify_ready(ready.snapshot_root(), size, digest)
            .unwrap();

        let _ = fs::remove_dir_all(source);
        let _ = fs::remove_dir_all(acquisition);
        let _ = fs::remove_dir_all(worktree);
    }

    #[test]
    fn acquisition_lock_reuses_a_stale_file_and_rejects_a_live_owner() {
        let root = test_directory("acquisition-lock");
        let stale_path = root.join(".pinvou-gaia-acquire.lock");
        drop(create_private_file(&stale_path).unwrap());

        let first = AcquisitionLock::claim(&root).expect("reuse stale lock file");
        let second_error = match AcquisitionLock::claim(&root) {
            Ok(_) => panic!("live acquisition lock must be exclusive"),
            Err(error) => error,
        };
        assert_eq!(second_error, GaiaFetchError::Busy);

        drop(first);
        AcquisitionLock::claim(&root).expect("OS lock releases after owner exits");
        let _ = fs::remove_dir_all(root);
    }
}

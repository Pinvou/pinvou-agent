use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use adapter_gaia::{
    GaiaFetchError, GaiaSnapshotManager, GaiaSource, SnapshotDownloadRequest, SnapshotDownloader,
    SnapshotFetchFailure, SnapshotFileMetadata, SnapshotPreflightRequest,
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "pinvou-gaia-fetch-public-{label}-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        // fetch 管理器对 acquisition/worktree 有私有权限契约(0700);
        // 默认 umask(如 0755 的 TMPDIR)会让夹具目录触发 ImportFailed。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

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

fn manager(acquisition: &TempDir, worktree: &TempDir) -> GaiaSnapshotManager<DenyDownloader> {
    GaiaSnapshotManager::new_with_optional_worktree(
        acquisition.path(),
        Some(worktree.path()),
        DenyDownloader,
    )
    .unwrap()
}

#[test]
fn fetch_missing_or_invalid_named_token_is_access_denied_without_fallback() {
    let acquisition = TempDir::new("acquisition");
    let worktree = TempDir::new("worktree");
    let manager = manager(&acquisition, &worktree);
    unsafe {
        std::env::remove_var("PINVOU_GAIA_MISSING_PUBLIC_TOKEN");
        std::env::set_var("HF_TOKEN", "FORBIDDEN_FALLBACK_SENTINEL");
    }
    let missing = manager
        .acquire(GaiaSource::TokenEnvironment(
            "PINVOU_GAIA_MISSING_PUBLIC_TOKEN".into(),
        ))
        .unwrap_err();
    unsafe { std::env::remove_var("HF_TOKEN") };
    assert_eq!(missing, GaiaFetchError::AccessDenied);
    for invalid in ["", "1STARTS_WITH_DIGIT", "HAS-DASH", "非ASCII"] {
        assert_eq!(
            manager
                .acquire(GaiaSource::TokenEnvironment(invalid.into()))
                .unwrap_err(),
            GaiaFetchError::AccessDenied
        );
    }
}

#[test]
fn fetch_rejects_snapshot_inside_worktree_or_ancestor_of_worktree() {
    let source_parent = TempDir::new("source-parent");
    let worktree_path = source_parent.path().join("repo");
    let inside = worktree_path.join("private-gaia");
    fs::create_dir_all(&inside).unwrap();
    let acquisition = TempDir::new("acquisition");
    let manager = GaiaSnapshotManager::new_with_optional_worktree(
        &acquisition.0,
        Some(worktree_path.as_path()),
        DenyDownloader,
    )
    .unwrap();

    assert_eq!(
        manager
            .acquire(GaiaSource::ExistingSnapshot(inside))
            .unwrap_err(),
        GaiaFetchError::ImportFailed
    );
    assert_eq!(
        manager
            .acquire(GaiaSource::ExistingSnapshot(
                source_parent.path().to_path_buf(),
            ))
            .unwrap_err(),
        GaiaFetchError::ImportFailed
    );
}

#[test]
fn fetch_preexisting_partial_ready_directory_is_safely_removed_before_retry() {
    let acquisition = TempDir::new("acquisition");
    let worktree = TempDir::new("worktree");
    let manager = manager(&acquisition, &worktree);
    let ready = acquisition
        .path()
        .join("gaia-2023-validation-level1-682dd723ee1e");
    fs::create_dir(&ready).unwrap();
    fs::write(ready.join("owner-sentinel"), b"do not overwrite").unwrap();

    assert_eq!(
        manager
            .acquire(GaiaSource::TokenEnvironment("IGNORED_TOKEN_NAME".into()))
            .unwrap_err(),
        GaiaFetchError::AccessDenied
    );
    assert!(!ready.exists());
}

#[test]
fn fetch_source_and_errors_redact_paths_environment_names_and_tokens() {
    let source = GaiaSource::TokenEnvironment("PRIVATE_ENV_NAME_SENTINEL".into());
    assert!(!format!("{source:?}").contains("PRIVATE_ENV_NAME_SENTINEL"));
    let error = GaiaFetchError::DownloadFailed;
    assert_eq!(
        format!("{error:?} {error}"),
        "gaia_download_failed gaia_download_failed"
    );
    assert_eq!(GaiaFetchError::Busy.code(), "gaia_fetch_in_progress");
    assert_eq!(
        format!("{:?} {}", GaiaFetchError::Busy, GaiaFetchError::Busy),
        "gaia_fetch_in_progress gaia_fetch_in_progress"
    );
}

// A downstream SnapshotDownloader implementation can only see the public
// `adapter_gaia` API. The request structs keep their fields private, so the
// public getters are the only way for such an implementation to learn the
// repository, revision, file paths, token, expected metadata, and budget of
// the preflight/download contract it is asked to fulfill. This fixture
// deliberately reads every one of those inputs, mirroring what a real
// downloader (or the shipped HfSnapshotDownloader) must do.
const PINNED_GAIA_REPO_ID: &str = "gaia-benchmark/GAIA";
const PINNED_GAIA_PARQUET_PATH: &str = "2023/validation/metadata.level1.parquet";
const READING_DOWNLOADER_TOKEN_ENV: &str = "PINVOU_GAIA_READING_DOWNLOADER_TOKEN";
const READING_DOWNLOADER_TOKEN_SENTINEL: &str = "gaia-reading-downloader-token";

// The crate pins the parquet digest as a hex string; `GAIA_PARQUET_SHA256_BYTES`
// is crate-private, so a downstream downloader parses the public hex itself.
fn pinned_parquet_sha256() -> [u8; 32] {
    let hex = adapter_gaia::GAIA_PARQUET_SHA256;
    let mut digest = [0_u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * index..2 * index + 2], 16)
            .expect("pinned digest hex must parse");
    }
    digest
}

#[derive(Default)]
struct ObservedPreflight {
    repo_id: String,
    revision: String,
    remote_paths: Vec<PathBuf>,
    token: String,
}

#[derive(Default)]
struct ObservedDownload {
    repo_id: String,
    revision: String,
    remote_path: String,
    token: String,
    expected_remote_path: PathBuf,
    expected_size: u64,
    expected_sha256: Option<[u8; 32]>,
    remaining_budget: u64,
}

#[derive(Default)]
struct ReadingDownloader {
    preflight_observation: Mutex<Option<ObservedPreflight>>,
    download_observation: Mutex<Option<ObservedDownload>>,
}

impl SnapshotDownloader for ReadingDownloader {
    fn preflight(
        &self,
        request: &SnapshotPreflightRequest<'_>,
    ) -> Result<Vec<SnapshotFileMetadata>, SnapshotFetchFailure> {
        *self.preflight_observation.lock().unwrap() = Some(ObservedPreflight {
            repo_id: request.repo_id().to_owned(),
            revision: request.revision().to_owned(),
            remote_paths: request.remote_paths().to_vec(),
            token: request.token().expose_to_backend().to_owned(),
        });
        // Echo each requested path back with the pinned official parquet
        // size/digest so the manager proceeds to the download phase.
        Ok(request
            .remote_paths()
            .iter()
            .map(|path| {
                SnapshotFileMetadata::new(
                    path.clone(),
                    adapter_gaia::GAIA_PARQUET_SIZE,
                    pinned_parquet_sha256(),
                )
            })
            .collect())
    }

    fn download(
        &self,
        request: &SnapshotDownloadRequest<'_>,
        _destination: &Path,
    ) -> Result<(), SnapshotFetchFailure> {
        *self.download_observation.lock().unwrap() = Some(ObservedDownload {
            repo_id: request.repo_id().to_owned(),
            revision: request.revision().to_owned(),
            remote_path: request.remote_path().to_owned(),
            token: request.token().expose_to_backend().to_owned(),
            expected_remote_path: request.expected().remote_path().to_path_buf(),
            expected_size: request.expected().size(),
            expected_sha256: request.expected().expected_sha256().copied(),
            remaining_budget: request.remaining_budget(),
        });
        // The request contract, not the payload, is under test: record and
        // fail so `acquire` surfaces DownloadFailed deterministically.
        Err(SnapshotFetchFailure)
    }
}

#[test]
fn fetch_external_downloader_reads_every_request_field_through_public_api() {
    let acquisition = TempDir::new("acquisition");
    let worktree = TempDir::new("worktree");
    let manager = GaiaSnapshotManager::new_with_optional_worktree(
        acquisition.path(),
        Some(worktree.path()),
        ReadingDownloader::default(),
    )
    .unwrap();

    unsafe {
        std::env::set_var(
            READING_DOWNLOADER_TOKEN_ENV,
            READING_DOWNLOADER_TOKEN_SENTINEL,
        );
    }
    let error = manager
        .acquire(GaiaSource::TokenEnvironment(
            READING_DOWNLOADER_TOKEN_ENV.into(),
        ))
        .unwrap_err();
    unsafe { std::env::remove_var(READING_DOWNLOADER_TOKEN_ENV) };
    assert_eq!(error, GaiaFetchError::DownloadFailed);

    // Read the observations back through the public `downloader()` accessor.
    let downloader = manager.downloader();
    let preflight = downloader
        .preflight_observation
        .lock()
        .unwrap()
        .take()
        .expect("preflight must observe the request");
    assert_eq!(preflight.repo_id, PINNED_GAIA_REPO_ID);
    assert_eq!(preflight.revision, adapter_gaia::GAIA_DATASET_REVISION);
    assert_eq!(
        preflight.remote_paths,
        vec![PathBuf::from(PINNED_GAIA_PARQUET_PATH)]
    );
    assert_eq!(preflight.token, READING_DOWNLOADER_TOKEN_SENTINEL);

    let download = downloader
        .download_observation
        .lock()
        .unwrap()
        .take()
        .expect("download must observe the request");
    assert_eq!(download.repo_id, PINNED_GAIA_REPO_ID);
    assert_eq!(download.revision, adapter_gaia::GAIA_DATASET_REVISION);
    assert_eq!(download.remote_path, PINNED_GAIA_PARQUET_PATH);
    assert_eq!(download.token, READING_DOWNLOADER_TOKEN_SENTINEL);
    assert_eq!(
        download.expected_remote_path,
        PathBuf::from(PINNED_GAIA_PARQUET_PATH)
    );
    assert_eq!(download.expected_size, adapter_gaia::GAIA_PARQUET_SIZE);
    assert_eq!(download.expected_sha256, Some(pinned_parquet_sha256()));
    // First download of the acquisition: the advertised budget is the full
    // transfer cap (256 MiB) with nothing consumed yet.
    assert_eq!(download.remaining_budget, 256 * 1024 * 1024);
}

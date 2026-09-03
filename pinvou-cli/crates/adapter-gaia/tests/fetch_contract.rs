use std::fs;
use std::path::{Path, PathBuf};
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
    GaiaSnapshotManager::new(acquisition.path(), worktree.path(), DenyDownloader).unwrap()
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
    let manager = GaiaSnapshotManager::new(&acquisition.0, &worktree_path, DenyDownloader).unwrap();

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

//! Short-lived capabilities that let an authenticated Web endpoint bind a
//! desktop-approved host directory to one code Session without sending the
//! native path back in a later create request.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MAX_WEB_WORKSPACE_GRANTS: usize = 64;
const WEB_WORKSPACE_GRANT_TTL: Duration = Duration::from_secs(30 * 60);
pub(super) const HOST_WORKSPACE_NOT_AUTHORIZED: &str = "host_workspace_not_authorized";

#[derive(Debug)]
struct WebWorkspaceGrant {
    endpoint_id: String,
    path: PathBuf,
    identity: WebWorkspaceIdentity,
    expires_at: Instant,
}

/// A grant removed from the live store while Session creation is in flight.
/// Dropping it consumes the one-shot capability; a failed creation may put it
/// back only for the same endpoint and without extending its original TTL.
#[derive(Debug)]
pub(crate) struct WebWorkspaceGrantReservation {
    handle: String,
    endpoint_id: String,
    path: PathBuf,
    identity: WebWorkspaceIdentity,
    expires_at: Instant,
}

impl WebWorkspaceGrantReservation {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Verify that the path about to be persisted for a code Session still
    /// names the exact directory selected by the desktop. Session creation
    /// calls this again around each durable binding step, closing the gap
    /// between grant redemption and workspace persistence without retaining
    /// the grant-store lock across asynchronous work.
    pub(crate) fn verify_bound_path(&self, bound_path: &Path) -> Result<(), String> {
        if bound_path != self.path {
            return Err("Web workspace authorization target changed".to_string());
        }
        let raw_bound_path = bound_path
            .to_str()
            .ok_or_else(|| "Web workspace authorization path is not valid Unicode".to_string())?;
        let current = crate::features::files::file_ingest::validate_browsable_path(raw_bound_path)?;
        if current != self.path || !current.is_dir() {
            return Err("Web workspace authorization target changed".to_string());
        }
        let current_identity = WebWorkspaceIdentity::capture(&current)?;
        if current_identity != self.identity {
            return Err("Web workspace authorization target changed".to_string());
        }
        Ok(())
    }

    /// Re-apply the Web browsing policy at redemption time and require the
    /// directory to retain the same filesystem identity it had when the
    /// one-shot capability was issued. This fails closed if metadata is not
    /// available on a supported platform.
    pub(crate) fn revalidate(self) -> Result<Self, String> {
        self.verify_bound_path(&self.path)?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WebWorkspaceIdentity(super::super::platform::WorkspaceIdentity);

impl WebWorkspaceIdentity {
    pub(crate) fn capture(path: &Path) -> Result<Self, String> {
        let metadata = std::fs::metadata(path)
            .map_err(|error| format!("inspect Web workspace {}: {error}", path.display()))?;
        if !metadata.is_dir() {
            return Err(format!(
                "Web code workspace must be a directory: {}",
                path.display()
            ));
        }
        super::super::platform::workspace_identity(path)
            .map(Self)
            .map_err(|error| {
                format!(
                    "Web workspace identity is unavailable for {}: {error}",
                    path.display()
                )
            })
    }

    #[cfg(test)]
    fn for_test(seed: u64) -> Self {
        Self(super::super::platform::test_workspace_identity(seed))
    }
}

#[derive(Debug, Default)]
pub(super) struct WebWorkspaceGrantStore {
    entries: HashMap<String, WebWorkspaceGrant>,
    order: VecDeque<String>,
}

impl WebWorkspaceGrantStore {
    pub(super) fn contains(&self, handle: &str) -> bool {
        self.entries.contains_key(handle)
    }

    pub(super) fn issue(
        &mut self,
        handle: String,
        endpoint_id: String,
        path: PathBuf,
        identity: WebWorkspaceIdentity,
        now: Instant,
    ) {
        self.remove_expired(now);
        while self.entries.len() >= MAX_WEB_WORKSPACE_GRANTS {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }
        self.order.retain(|candidate| candidate != &handle);
        self.entries.insert(
            handle.clone(),
            WebWorkspaceGrant {
                endpoint_id,
                path,
                identity,
                expires_at: now + WEB_WORKSPACE_GRANT_TTL,
            },
        );
        self.order.push_back(handle);
    }

    pub(super) fn reserve(
        &mut self,
        handle: &str,
        endpoint_id: &str,
        now: Instant,
    ) -> Result<WebWorkspaceGrantReservation, String> {
        validate_handle(handle)?;
        let Some(grant) = self.entries.remove(handle) else {
            return Err("Web workspace authorization is invalid or already used".to_string());
        };
        self.order.retain(|candidate| candidate != handle);
        if grant.expires_at <= now {
            return Err("Web workspace authorization has expired".to_string());
        }
        if grant.endpoint_id != endpoint_id {
            return Err("Web workspace authorization belongs to another endpoint".to_string());
        }
        Ok(WebWorkspaceGrantReservation {
            handle: handle.to_string(),
            endpoint_id: grant.endpoint_id,
            path: grant.path,
            identity: grant.identity,
            expires_at: grant.expires_at,
        })
    }

    pub(super) fn restore(
        &mut self,
        reservation: WebWorkspaceGrantReservation,
        endpoint_id: &str,
        now: Instant,
    ) -> bool {
        self.remove_expired(now);
        if reservation.endpoint_id != endpoint_id
            || reservation.expires_at <= now
            || self.entries.contains_key(&reservation.handle)
        {
            return false;
        }
        while self.entries.len() >= MAX_WEB_WORKSPACE_GRANTS {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }
        let handle = reservation.handle;
        self.entries.insert(
            handle.clone(),
            WebWorkspaceGrant {
                endpoint_id: reservation.endpoint_id,
                path: reservation.path,
                identity: reservation.identity,
                expires_at: reservation.expires_at,
            },
        );
        self.order.push_back(handle);
        true
    }

    fn remove_expired(&mut self, now: Instant) {
        self.entries.retain(|_, grant| grant.expires_at > now);
        self.order
            .retain(|handle| self.entries.contains_key(handle));
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }
}

pub(super) fn validate_handle(handle: &str) -> Result<(), String> {
    if handle.len() < 24
        || handle.len() > 128
        || !handle.starts_with("workspace_")
        || !handle
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("Web workspace authorization handle is invalid".to_string());
    }
    Ok(())
}

pub(super) fn require_host_workspace_authorization(authorized: bool) -> Result<(), String> {
    if !authorized {
        return Err(HOST_WORKSPACE_NOT_AUTHORIZED.to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_handle(seed: usize) -> String {
        format!("workspace_{seed:032x}")
    }

    fn test_identity(seed: u64) -> WebWorkspaceIdentity {
        WebWorkspaceIdentity::for_test(seed)
    }

    #[test]
    fn grant_is_endpoint_bound_and_one_shot() {
        let now = Instant::now();
        let mut store = WebWorkspaceGrantStore::default();
        let first = workspace_handle(1);
        store.issue(
            first.clone(),
            "endpoint-one".to_string(),
            PathBuf::from("workspace-one"),
            test_identity(1),
            now,
        );

        assert!(
            store
                .reserve(&first, "endpoint-two", now + Duration::from_secs(1))
                .is_err()
        );
        assert!(
            store
                .reserve(&first, "endpoint-one", now + Duration::from_secs(1))
                .is_err()
        );

        let second = workspace_handle(2);
        let path = PathBuf::from("workspace-two");
        store.issue(
            second.clone(),
            "endpoint-one".to_string(),
            path.clone(),
            test_identity(2),
            now,
        );
        let reservation = store
            .reserve(&second, "endpoint-one", now + Duration::from_secs(1))
            .unwrap();
        assert_eq!(reservation.path(), path.as_path());
        assert!(
            store
                .reserve(&second, "endpoint-one", now + Duration::from_secs(2))
                .is_err()
        );
    }

    #[test]
    fn failed_creation_can_restore_grant_without_renewing_or_crossing_endpoints() {
        let now = Instant::now();
        let mut store = WebWorkspaceGrantStore::default();
        let handle = workspace_handle(3);
        let path = PathBuf::from("workspace-three");
        store.issue(
            handle.clone(),
            "endpoint-one".to_string(),
            path.clone(),
            test_identity(3),
            now,
        );

        let reservation = store
            .reserve(&handle, "endpoint-one", now + Duration::from_secs(1))
            .unwrap();
        assert!(!store.contains(&handle));
        assert!(!store.restore(reservation, "endpoint-two", now + Duration::from_secs(2)));
        assert!(!store.contains(&handle));

        store.issue(
            handle.clone(),
            "endpoint-one".to_string(),
            path.clone(),
            test_identity(3),
            now,
        );
        let reservation = store
            .reserve(&handle, "endpoint-one", now + Duration::from_secs(3))
            .unwrap();
        assert!(store.restore(reservation, "endpoint-one", now + Duration::from_secs(4)));
        assert_eq!(
            store
                .reserve(&handle, "endpoint-one", now + Duration::from_secs(5))
                .unwrap()
                .path(),
            path.as_path()
        );

        store.issue(
            handle.clone(),
            "endpoint-one".to_string(),
            path,
            test_identity(3),
            now,
        );
        let reservation = store
            .reserve(&handle, "endpoint-one", now + Duration::from_secs(6))
            .unwrap();
        assert!(!store.restore(reservation, "endpoint-one", now + WEB_WORKSPACE_GRANT_TTL));
        assert!(!store.contains(&handle));
    }

    #[test]
    fn authorization_is_explicit() {
        assert_eq!(
            require_host_workspace_authorization(false).unwrap_err(),
            HOST_WORKSPACE_NOT_AUTHORIZED
        );
        assert!(require_host_workspace_authorization(true).is_ok());
    }

    #[test]
    fn grants_expire_and_store_is_bounded() {
        let now = Instant::now();
        let mut store = WebWorkspaceGrantStore::default();
        let expired = workspace_handle(0);
        store.issue(
            expired.clone(),
            "endpoint".to_string(),
            PathBuf::from("expired"),
            test_identity(0),
            now,
        );
        assert!(
            store
                .reserve(&expired, "endpoint", now + WEB_WORKSPACE_GRANT_TTL)
                .is_err()
        );

        for seed in 1..=MAX_WEB_WORKSPACE_GRANTS + 1 {
            store.issue(
                workspace_handle(seed),
                "endpoint".to_string(),
                PathBuf::from(format!("workspace-{seed}")),
                test_identity(seed as u64),
                now,
            );
        }
        assert_eq!(store.entries.len(), MAX_WEB_WORKSPACE_GRANTS);
        assert!(!store.contains(&workspace_handle(1)));
        assert!(store.contains(&workspace_handle(MAX_WEB_WORKSPACE_GRANTS + 1)));
    }

    #[test]
    fn redemption_rejects_a_replaced_directory_identity() {
        let root = crate::platform::paths::user_home_dir().join(format!(
            ".pinvou3-workspace-grant-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        let identity = WebWorkspaceIdentity::capture(&workspace).unwrap();
        let mut store = WebWorkspaceGrantStore::default();
        let handle = workspace_handle(99);
        store.issue(
            handle.clone(),
            "endpoint".to_string(),
            workspace.clone(),
            identity,
            Instant::now(),
        );

        std::fs::rename(&workspace, root.join("original-workspace")).unwrap();
        std::fs::create_dir(&workspace).unwrap();
        let reservation = store.reserve(&handle, "endpoint", Instant::now()).unwrap();
        let error = reservation.revalidate().unwrap_err();
        assert!(error.contains("target changed"));

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn final_binding_recheck_rejects_replacement_after_initial_validation() {
        let root = crate::platform::paths::user_home_dir().join(format!(
            ".pinvou3-workspace-binding-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        let reservation = WebWorkspaceGrantReservation {
            handle: workspace_handle(100),
            endpoint_id: "endpoint".to_string(),
            path: workspace.clone(),
            identity: WebWorkspaceIdentity::capture(&workspace).unwrap(),
            expires_at: Instant::now() + WEB_WORKSPACE_GRANT_TTL,
        };
        assert!(reservation.verify_bound_path(&workspace).is_ok());
        std::fs::rename(&workspace, root.join("original-workspace")).unwrap();
        std::fs::create_dir(&workspace).unwrap();
        let error = reservation.verify_bound_path(&workspace).unwrap_err();
        assert!(error.contains("target changed"));

        std::fs::remove_dir_all(&root).unwrap();
    }
}

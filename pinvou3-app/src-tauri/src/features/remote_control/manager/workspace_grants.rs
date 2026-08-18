//! Short-lived capabilities that let an authenticated Web endpoint bind a
//! desktop-approved host directory to one code Session without sending the
//! native path back in a later create request.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const MAX_WEB_WORKSPACE_GRANTS: usize = 64;
const WEB_WORKSPACE_GRANT_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Debug)]
struct WebWorkspaceGrant {
    endpoint_id: String,
    path: PathBuf,
    expires_at: Instant,
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
                expires_at: now + WEB_WORKSPACE_GRANT_TTL,
            },
        );
        self.order.push_back(handle);
    }

    pub(super) fn consume(
        &mut self,
        handle: &str,
        endpoint_id: &str,
        now: Instant,
    ) -> Result<PathBuf, String> {
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
        Ok(grant.path)
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
        return Err("Host workspace access was not authorized on the desktop".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_handle(seed: usize) -> String {
        format!("workspace_{seed:032x}")
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
            now,
        );

        assert!(store
            .consume(&first, "endpoint-two", now + Duration::from_secs(1))
            .is_err());
        assert!(store
            .consume(&first, "endpoint-one", now + Duration::from_secs(1))
            .is_err());

        let second = workspace_handle(2);
        let path = PathBuf::from("workspace-two");
        store.issue(
            second.clone(),
            "endpoint-one".to_string(),
            path.clone(),
            now,
        );
        assert_eq!(
            store
                .consume(&second, "endpoint-one", now + Duration::from_secs(1))
                .unwrap(),
            path
        );
        assert!(store
            .consume(&second, "endpoint-one", now + Duration::from_secs(2))
            .is_err());
    }

    #[test]
    fn authorization_is_explicit() {
        assert!(require_host_workspace_authorization(false).is_err());
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
            now,
        );
        assert!(store
            .consume(&expired, "endpoint", now + WEB_WORKSPACE_GRANT_TTL)
            .is_err());

        for seed in 1..=MAX_WEB_WORKSPACE_GRANTS + 1 {
            store.issue(
                workspace_handle(seed),
                "endpoint".to_string(),
                PathBuf::from(format!("workspace-{seed}")),
                now,
            );
        }
        assert_eq!(store.entries.len(), MAX_WEB_WORKSPACE_GRANTS);
        assert!(!store.contains(&workspace_handle(1)));
        assert!(store.contains(&workspace_handle(MAX_WEB_WORKSPACE_GRANTS + 1)));
    }
}

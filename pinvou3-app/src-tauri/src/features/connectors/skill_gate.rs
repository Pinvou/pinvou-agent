//! Skill-gate trait shared by the four CLI connectors (tmeet/dingtalk/feishu/
//! wecom): a per-connector disabled-flag file plus skill apply/discard.
//!
//! Each connector only supplies `id`, `disabled_filename`, and `apply_skills`
//! (pointing at its `Pinvou3Bundle::apply_*_skills`); the flag-file path
//! resolution and the read-side existence check are default implementations.
//!
//! Disable semantics: the `~/.pinvou3/<id>_disabled` file existing means the
//! user manually disabled that connector's skills, orthogonal to connection
//! state (auth). The write side (`set_disabled_flag`) was removed together
//! with the retired `set_*_enabled` commands; connector switches now persist
//! through the unified scope state (`set_disabled_connectors` → marketplace
//! scope), so this trait only reads the flag.

use std::path::PathBuf;

/// Skill-gate abstraction for a single CLI connector.
///
/// Implementors supply three connector-specific items and reuse the default
/// flag-file path resolution and read-side check.
pub(crate) trait ConnectorSkillGate {
    /// 连接器 id(事件前缀 / 日志标签,如 `"tmeet"`)。
    fn id(&self) -> &'static str;

    /// 停用标志文件名(如 `"tmeet_disabled"`)。
    fn disabled_filename(&self) -> &'static str;

    /// 按 `visible` 增 / 删本连接器的技能文件 —— 调各自的
    /// `Pinvou3Bundle::apply_*_skills`。返回 `Result` 以传播写盘失败。
    fn apply_skills(&self, visible: bool) -> Result<(), String>;

    /// 停用标志文件完整路径:`~/.pinvou3/<disabled_filename>`。
    fn disabled_path(&self) -> PathBuf {
        crate::platform::paths::pinvou3_home().join(self.disabled_filename())
    }

    /// 是否被手动停用(停用标志文件存在即停用)。
    fn is_disabled(&self) -> bool {
        self.disabled_path().exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::paths::tests::ENV_LOCK;

    /// Minimal fake impl driving the default implementations: exercises the
    /// disabled-path derivation under a temporary `PINVOU3_HOME`.
    struct FakeGate;
    impl ConnectorSkillGate for FakeGate {
        fn id(&self) -> &'static str {
            "fake"
        }
        fn disabled_filename(&self) -> &'static str {
            "fake_disabled"
        }
        fn apply_skills(&self, _visible: bool) -> Result<(), String> {
            Ok(())
        }
    }

    /// `disabled_path` 跟随 `PINVOU3_HOME`,且文件名由 `disabled_filename` 决定。
    #[test]
    fn disabled_path_is_derived_from_pinvou3_home() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = format!(
            "{}/pinvou3-skillgate-path-{}-{}",
            std::env::temp_dir().display(),
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let previous = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
        // serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let gate = FakeGate;
        assert_eq!(
            gate.disabled_path(),
            crate::platform::paths::pinvou3_home().join("fake_disabled")
        );

        match previous {
            // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes
            // are serialized in-process.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes
            // are serialized in-process.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

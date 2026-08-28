//! 技能门控 trait —— 5 连接器(tmeet/dingtalk/feishu/wecom/weibo)共享的
//! 「停用标志文件 + 应用技能」机制。
//!
//! 收敛前,四个连接器各自复制了一份几乎相同的四元组:
//! `*_disabled_path` / `is_*_disabled` / `set_*_disabled_flag` / `apply_*_skills`。
//! 本 trait 把「停用标志文件」的路径解析 / 读存在性 / 写删往返抽成默认实现,
//! 各连接器只需提供 `id` / `disabled_filename` / `apply_skills`(指向各自的
//! `Pinvou3Bundle::apply_*_skills`)。
//!
//! 停用语义:`~/.pinvou3/<id>_disabled` 文件**存在** = 用户手动停用该连接器技能,
//! 与连接状态(auth)正交。`set_disabled_flag` 统一返回 `Result<(), String>`:
//! 停用标志写盘失败必须传播给调用方(此前 feishu/wecom 用 `let _ =` 静默忽略,
//! Wave 1 显式批准的契约面变更)。

use std::path::PathBuf;

/// 单个 CLI 连接器的技能门控抽象。
///
/// 实现者提供三项连接器特有信息,即可复用默认的标志文件读写实现。
pub(crate) trait ConnectorSkillGate {
    /// 连接器 id(事件前缀 / 日志标签,如 `"tmeet"`)。
    fn id(&self) -> &'static str;

    /// 停用标志文件名(如 `"tmeet_disabled"`)。
    fn disabled_filename(&self) -> &'static str;

    /// 按 `visible` 增 / 删本连接器的技能文件 —— 调各自的
    /// `Pinvou3Bundle::apply_*_skills`。返回 `Result` 以传播写盘失败。
    fn apply_skills(&self, visible: bool) -> Result<(), String>;

    /// 用户可见的中文产品名(如「腾讯会议」「钉钉」),用于错误文案。
    /// 默认回退到 `id()`(ASCII),各连接器按需覆盖以保留原中文文案。
    fn display_name(&self) -> &'static str {
        self.id()
    }

    /// 停用标志文件完整路径:`~/.pinvou3/<disabled_filename>`。
    fn disabled_path(&self) -> PathBuf {
        crate::platform::paths::pinvou3_home().join(self.disabled_filename())
    }

    /// 是否被手动停用(停用标志文件存在即停用)。
    fn is_disabled(&self) -> bool {
        self.disabled_path().exists()
    }

    /// 写(`true`)/ 删(`false`)停用标志文件。统一返回 `Result<(), String>`:
    /// 写盘失败传播给调用方,不再静默 `let _ =` 忽略。
    fn set_disabled_flag(&self, disabled: bool) -> Result<(), String> {
        let p = self.disabled_path();
        let name = self.display_name();
        if disabled {
            std::fs::write(&p, b"1").map_err(|e| format!("保存{name}技能停用状态失败: {e}"))?;
        } else if p.exists() {
            std::fs::remove_file(&p).map_err(|e| format!("清除{name}技能停用状态失败: {e}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::paths::tests::ENV_LOCK;

    /// 用一个最小 fake 实现驱动默认实现:验证 `is_disabled` / `set_disabled_flag`
    /// 在临时 `PINVOU3_HOME` 下的读写往返(置停用→存在;复位→消失;幂等)。
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

    #[test]
    fn disabled_flag_roundtrip_through_default_impl() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = format!(
            "{}/pinvou3-skillgate-test-{}-{}",
            std::env::temp_dir().display(),
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let previous = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &tmp);
        let _ = std::fs::create_dir_all(crate::platform::paths::pinvou3_home());

        let gate = FakeGate;

        // 默认(无文件)= 未停用;设 false 在无文件时也应幂等成功。
        assert!(!gate.is_disabled());
        gate.set_disabled_flag(false).unwrap();
        assert!(!gate.is_disabled());

        // 置停用 → 文件落盘 → is_disabled 命中。
        gate.set_disabled_flag(true).unwrap();
        assert!(gate.is_disabled());

        // 重复置停用幂等(覆盖写不报错)。
        gate.set_disabled_flag(true).unwrap();
        assert!(gate.is_disabled());

        // 复位 → 文件删 → 未停用。
        gate.set_disabled_flag(false).unwrap();
        assert!(!gate.is_disabled());

        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
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
        std::env::set_var("PINVOU3_HOME", &tmp);

        let gate = FakeGate;
        assert_eq!(
            gate.disabled_path(),
            crate::platform::paths::pinvou3_home().join("fake_disabled")
        );

        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

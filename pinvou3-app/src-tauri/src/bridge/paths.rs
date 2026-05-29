//! `~/.pinvou3/` 目录布局解析。
//!
//! pinvou3-app 不读 `~/.deepseek/`（隔离），所有 deepseek-tui 默认会写到
//! 全局/cwd 的字段都映射到这个独立目录树。布局参见 plan「目录布局」一节。
//!
//! `PINVOU3_HOME` 环境变量可整体重定位（主要用于测试）。

use std::path::PathBuf;

/// 用户家目录 `$HOME`，是 pinvou3-app 的 engine workspace 根。
/// AI 通过相对路径访问 → 落在家目录下；通过绝对路径访问 → trust_mode 放行
/// 但敏感子目录由 path filter / instructions 引导拦截。
pub fn user_home_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home)
}

/// `~/.pinvou3/` 根目录。
pub fn pinvou3_home() -> PathBuf {
    if let Ok(custom) = std::env::var("PINVOU3_HOME") {
        return PathBuf::from(custom);
    }
    user_home_dir().join(".pinvou3")
}

pub fn settings_path() -> PathBuf {
    pinvou3_home().join("settings.json")
}

pub fn bundle_root() -> PathBuf {
    pinvou3_home().join("bundle")
}
pub fn bundle_instructions() -> PathBuf {
    bundle_root().join("instructions.md")
}
pub fn bundle_skills_dir() -> PathBuf {
    bundle_root().join("skills")
}
pub fn bundle_mcp_json() -> PathBuf {
    bundle_root().join("mcp.json")
}
pub fn bundle_version_file() -> PathBuf {
    bundle_root().join("VERSION")
}

pub fn user_root() -> PathBuf {
    pinvou3_home().join("user")
}
pub fn user_instructions() -> PathBuf {
    user_root().join("instructions.md")
}
pub fn user_skills_dir() -> PathBuf {
    user_root().join("skills")
}

/// `~/.deepseek/skills/` — DeepSeek-TUI 标准用户 skills 目录,h3c-ppt /
/// skill-creator 这种由 `/skill install` 装的 skill 都在这里。跟
/// [`user_skills_dir`](pinvou3 私有 `~/.pinvou3/user/skills/`) 平行,工作流
/// 视图 list_skills_v2 把两个目录合并去重展示 (user 覆盖 deepseek 覆盖 bundle)。
pub fn deepseek_skills_dir() -> PathBuf {
    user_home_dir().join(".deepseek").join("skills")
}

/// 兼容字段：阶段 B 旧 sandbox workspace（已不作为 engine workspace 使用，
/// 但保留作为 "AI 私人沙盒" 兜底——某些场景如 monitor 测试还在用）。
pub fn workspace_dir() -> PathBuf {
    pinvou3_home().join("workspace")
}
pub fn notes_path() -> PathBuf {
    pinvou3_home().join("notes.md")
}
pub fn memory_path() -> PathBuf {
    pinvou3_home().join("memory.md")
}
pub fn mcp_config_path() -> PathBuf {
    bundle_mcp_json()
}

/// `~/.pinvou3/sessions/` —— 所有对话历史落盘的根目录。
pub fn sessions_root() -> PathBuf {
    pinvou3_home().join("sessions")
}

/// `~/.pinvou3/sessions/<session_id>/artifacts/` —— AI 默认产物落地目录。
/// `$PINVOU3_SESSION_ARTIFACTS` 环境变量注入这个值给 engine + LLM。
pub fn session_artifacts_dir(session_id: &str) -> PathBuf {
    sessions_root().join(session_id).join("artifacts")
}

/// `~/.pinvou3/sessions/<session_id>/workspace/` —— 每个 session 独立的工作目录。
/// engine workspace 跟随当前 active session 切换，避免多 session 共享文件冲突。
/// 切换 session 时 bridge 调 `Op::SyncSession { workspace }` 重置。
pub fn session_workspace_dir(session_id: &str) -> PathBuf {
    sessions_root().join(session_id).join("workspace")
}

/// `~/.pinvou3/sessions/<session_id>/instructions.md` —— 每个 session 独立的
/// Legacy `~/.pinvou3/sessions/<sid>/instructions.md` 路径。
///
/// C 方案(P-no-disk)前用作 per-session prompt 文件,EngineConfig.instructions
/// 指向它。改成 `InstructionSource::Inline` 后这个 disk 文件**不再被生产代码读**,
/// 仅用于 boot 时 legacy 清理(早期 pinvou3 版本写下的残留)。新版 pinvou3 不再写。
pub fn session_instructions_path(session_id: &str) -> PathBuf {
    sessions_root().join(session_id).join("instructions.md")
}

/// 阶段 C 没多 session 时的 fallback artifacts dir（session_id="default"）。
/// Step 4 完成后这个会被切换 session 时动态计算的值替换。
pub fn default_session_artifacts_dir() -> PathBuf {
    session_artifacts_dir("default")
}

/// 首次启动确保所有目录存在。bundle/skills 等子目录在解包时还会再 ensure 一次。
pub fn ensure_dirs() -> std::io::Result<()> {
    std::fs::create_dir_all(bundle_skills_dir())?;
    std::fs::create_dir_all(user_skills_dir())?;
    std::fs::create_dir_all(workspace_dir())?;
    std::fs::create_dir_all(default_session_artifacts_dir())?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 进程级 env var 是测试的硬隔离障碍：cargo test 默认并行跑，多个测试
    /// 同时改 PINVOU3_HOME 会互相覆盖断言。这把全局锁让所有 mutate
    /// `PINVOU3_HOME` 的测试串行执行。bridge::sessions 模块测试也借用这把锁。
    pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn pinvou3_home_respects_env_override() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", "/tmp/pinvou3-test-override");
        assert_eq!(pinvou3_home(), PathBuf::from("/tmp/pinvou3-test-override"));
        assert_eq!(
            settings_path(),
            PathBuf::from("/tmp/pinvou3-test-override/settings.json")
        );
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
    }

    /// `user_home_dir` 应该读 $HOME（pinvou3 engine workspace 之根）。
    #[test]
    fn user_home_dir_reads_home_env() {
        if let Ok(h) = std::env::var("HOME") {
            assert_eq!(user_home_dir(), PathBuf::from(h));
        }
        // 没设 HOME 时 fallback /tmp（不强测，避免 race）
    }

    /// session artifacts 路径必须落在 ~/.pinvou3/sessions/<id>/artifacts/ 下。
    #[test]
    fn session_artifacts_layout() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", "/tmp/pinvou3-artifacts-layout-test");
        assert_eq!(
            session_artifacts_dir("abc123"),
            PathBuf::from("/tmp/pinvou3-artifacts-layout-test/sessions/abc123/artifacts")
        );
        assert_eq!(
            default_session_artifacts_dir(),
            PathBuf::from("/tmp/pinvou3-artifacts-layout-test/sessions/default/artifacts")
        );
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
    }
}

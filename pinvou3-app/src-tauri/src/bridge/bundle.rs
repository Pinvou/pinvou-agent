//! pinvou3 内置 bundle：随 app 编译进去的 instructions.md / mcp.json / skills 模板，
//! 首次启动时解包到 `~/.pinvou3/bundle/`。
//!
//! 与 user/ 严格分离：bundle/ 每次升级被覆写，user/ 永远不动。
//! 解包用 `bundle/VERSION` 比对 [`BUNDLE_VERSION`]，相同则跳过。

use std::path::PathBuf;

use super::paths;

/// Bundle 版本号：手动 base + 自动 instructions.md 内容 hash（build.rs 注入）。
/// 改 INSTRUCTIONS_MD 时不需要 bump base —— hash 自动变，ensure_extracted 自动覆写。
/// 改其他 bundle 资源（mcp.json 默认 / skills 模板等）才需要手动 bump base。
///
/// 0.4: 加 Pinvou Review 内置 skills(pinvou-review-plan / pinvou-review-final)
pub const BUNDLE_VERSION: &str = concat!("0.4-", env!("BUNDLE_INSTRUCTIONS_HASH"));

/// pinvou3 内置的 instructions.md（Qwen3.6 适配 prompt），编译时内嵌。
pub const INSTRUCTIONS_MD: &str = include_str!("../../resources/bundle/instructions.md");

/// 内置 MCP 默认配置——暂时空。`McpPool::from_config_path` 接受 `{"servers":{}}`。
pub const DEFAULT_MCP_JSON: &str = "{\n  \"servers\": {}\n}\n";

/// 内嵌的敏感目录拦截 shell 脚本——配合 bridge 注入的 hook 在 ToolCallBefore
/// 时阻止 LLM 触碰 ~/.ssh/ ~/.gnupg/ 等。
pub const DENY_SENSITIVE_PATHS_SH: &str =
    include_str!("../../resources/bundle/deny_sensitive_paths.sh");

/// Pinvou Review v2 内置 skill:plan 审查(GATE 配套)。
/// 编译时内嵌,启动时 ensure_extracted 写到 ~/.pinvou3/bundle/skills/。
/// DeepSeek-TUI SkillRegistry 期待 <skills_dir>/<skill-name>/SKILL.md 格式。
pub const PINVOU_REVIEW_PLAN_SKILL_MD: &str =
    include_str!("../../resources/bundle/skills/pinvou-review-plan/SKILL.md");

/// Pinvou Review v2 内置 skill:任务收口物理校验(advisory)。
pub const PINVOU_REVIEW_FINAL_SKILL_MD: &str =
    include_str!("../../resources/bundle/skills/pinvou-review-final/SKILL.md");

#[derive(Debug, Clone)]
pub struct Pinvou3Bundle {
    pub root: PathBuf,
    pub instructions_md: PathBuf,
    pub skills_dir: PathBuf,
    pub user_skills_dir: PathBuf,
    pub mcp_json: PathBuf,
    pub deny_sensitive_sh: PathBuf,
}

impl Pinvou3Bundle {
    pub fn paths() -> Self {
        Self {
            root: paths::bundle_root(),
            instructions_md: paths::bundle_instructions(),
            skills_dir: paths::bundle_skills_dir(),
            user_skills_dir: paths::user_skills_dir(),
            mcp_json: paths::bundle_mcp_json(),
            deny_sensitive_sh: paths::bundle_root().join("deny_sensitive_paths.sh"),
        }
    }

    /// 比对 `bundle/VERSION` 与 [`BUNDLE_VERSION`]：相同跳过；
    /// 不同则覆写 bundle 内文件并更新 VERSION。**不动 user/ 和 settings.json**。
    ///
    /// 解包时对 `INSTRUCTIONS_MD` 做模板替换，把 `{{PINVOU3_WORKSPACE}}` 占位符
    /// 替换成 `~/.pinvou3/workspace/` 的实际绝对路径——让 AI 直接拿到完整路径
    /// 给 write_file 用，避免先 exec_shell 探一遍 env var。
    pub fn ensure_extracted(&self) -> std::io::Result<()> {
        paths::ensure_dirs()?;
        let version_file = paths::bundle_version_file();
        let current = std::fs::read_to_string(&version_file).unwrap_or_default();
        if current.trim() == BUNDLE_VERSION {
            return Ok(());
        }
        std::fs::create_dir_all(&self.root)?;
        std::fs::create_dir_all(&self.skills_dir)?;
        let workspace_abs = paths::workspace_dir();
        std::fs::create_dir_all(&workspace_abs)?;
        let rendered =
            INSTRUCTIONS_MD.replace("{{PINVOU3_WORKSPACE}}", &workspace_abs.to_string_lossy());
        std::fs::write(&self.instructions_md, rendered)?;
        if !self.mcp_json.exists() {
            std::fs::write(&self.mcp_json, DEFAULT_MCP_JSON)?;
        }
        // 敏感目录拦截脚本：写入 + 加可执行位
        std::fs::write(&self.deny_sensitive_sh, DENY_SENSITIVE_PATHS_SH)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = std::fs::metadata(&self.deny_sensitive_sh)?.permissions();
            perm.set_mode(0o755);
            std::fs::set_permissions(&self.deny_sensitive_sh, perm)?;
        }
        // Pinvou Review v2 内置 skills:写到 <skills_dir>/<name>/SKILL.md
        // DeepSeek-TUI SkillRegistry::discover 期待这个 subdir + SKILL.md 格式。
        let plan_dir = self.skills_dir.join("pinvou-review-plan");
        let final_dir = self.skills_dir.join("pinvou-review-final");
        std::fs::create_dir_all(&plan_dir)?;
        std::fs::create_dir_all(&final_dir)?;
        std::fs::write(plan_dir.join("SKILL.md"), PINVOU_REVIEW_PLAN_SKILL_MD)?;
        std::fs::write(final_dir.join("SKILL.md"), PINVOU_REVIEW_FINAL_SKILL_MD)?;
        std::fs::write(&version_file, BUNDLE_VERSION)?;
        eprintln!(
            "[pinvou3-app] bundle extracted to {} (version {})",
            self.root.display(),
            BUNDLE_VERSION
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::paths::tests::ENV_LOCK;

    /// 测试 bundle 解包的两个场景：首次解包成功 + VERSION 匹配时不覆写。
    /// 借 paths::tests::ENV_LOCK 跟其他 mutate PINVOU3_HOME 的测试串行化，
    /// 不靠唯一 nanos 路径躲 race（仍会读 env var）。
    #[test]
    fn ensure_extracted_behavior() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempdir();
        std::env::set_var("PINVOU3_HOME", &tmp);

        // 1) 首次解包：文件被写入 + VERSION 记录
        let bundle = Pinvou3Bundle::paths();
        bundle.ensure_extracted().unwrap();
        assert!(bundle.instructions_md.is_file());
        assert!(bundle.mcp_json.is_file());
        assert!(paths::bundle_version_file().is_file());
        let v = std::fs::read_to_string(paths::bundle_version_file()).unwrap();
        assert_eq!(v.trim(), BUNDLE_VERSION);

        // 2) VERSION 匹配则跳过：故意改 instructions.md，再 ensure，不应覆写
        std::fs::write(&bundle.instructions_md, "USER TOUCHED").unwrap();
        bundle.ensure_extracted().unwrap();
        let content = std::fs::read_to_string(&bundle.instructions_md).unwrap();
        assert_eq!(
            content, "USER TOUCHED",
            "VERSION 匹配时不应覆写已存在的 bundle 文件"
        );

        cleanup(&tmp);
    }

    fn tempdir() -> String {
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("/tmp/pinvou3-bundle-test-{}", id)
    }

    fn cleanup(dir: &str) {
        std::env::remove_var("PINVOU3_HOME");
        let _ = std::fs::remove_dir_all(dir);
    }
}

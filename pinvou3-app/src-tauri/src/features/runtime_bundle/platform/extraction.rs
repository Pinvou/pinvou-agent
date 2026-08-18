//! Bundle 提取逻辑:版本比对、skills/workflow 解包、连接器技能门控、MCP server 写入。
//!
//! 从 mod.rs 抽离——mod.rs 保留 fork-guard 指纹(install_prompt_overrides 的
//! set_static_prompt_composer_override + tests 的 forkguard_builtin_visual_skill)
//! 与提示词静态层常量;本模块只含运行时提取逻辑,通过 `use super::*` 复用
//! mod.rs 的资产常量(SKILL_DIRS/Dir/MANIFEST/SERVER_PY 等)。

use super::*;

#[derive(Debug, Clone)]
pub struct Pinvou3Bundle {
    pub root: PathBuf,
    pub instructions_md: PathBuf,
    pub skills_dir: PathBuf,
    pub user_skills_dir: PathBuf,
    pub mcp_json: PathBuf,
    pub deny_sensitive_sh: PathBuf,
    pub deny_sensitive_ps1: PathBuf,
    pub multiagent_depth_guard_sh: PathBuf,
    pub multiagent_depth_guard_ps1: PathBuf,
    pub shell_env_sh: PathBuf,
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
            deny_sensitive_ps1: paths::bundle_root().join("deny_sensitive_paths.ps1"),
            multiagent_depth_guard_sh: paths::bundle_root().join("multiagent_depth_guard.sh"),
            multiagent_depth_guard_ps1: paths::bundle_root().join("multiagent_depth_guard.ps1"),
            shell_env_sh: paths::bundle_root().join("shell_env.sh"),
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
        let bundle_changed = current.trim() != BUNDLE_VERSION;

        // 已下线 skills 每次启动都清理(防御性):既有装机的残留目录若不清,
        // SkillRegistry 仍会从 disk 发现它们、重新触发对应协议 prompt。
        crate::platform::startup::mark("bundle_extract:cleanup_retired:start");
        self.cleanup_retired_skills()?;
        // 已从技能市场下架的预置技能(pua/女娲/头脑风暴):它们曾走 marketplace 装、带
        // `pinvou3-marketplace:` 标记,故按标记内容精确删,只跳过用户上传的同名目录。
        self.cleanup_removed_marketplace_skills()?;
        // 已从工具市场下架的预置 MCP 工具也要清理运行态残留;否则旧 manifest 仍会被
        // MarketplaceManager 扫到,在 composer「已接入工具」里继续出现。
        self.cleanup_removed_marketplace_tools()?;
        crate::platform::startup::mark("bundle_extract:cleanup_retired:done");
        // PR #132 早期构建曾把 CLI 解包进 immutable bundle；统一在线安装后清掉该
        // app 自有旧目录，避免旧二进制掩盖按需安装与 hash 校验。
        let _ = std::fs::remove_dir_all(paths::bundle_root().join("connectors"));
        // Migrate plaintext MCP secrets before bundled manifests are rewritten. If migration
        // fails, keep the old files as a recoverable source instead of overwriting the only
        // remaining plaintext copy.
        crate::platform::startup::mark("bundle_extract:migrate_mcp_secrets:start");
        let mcp_secret_migration_ok = match crate::features::marketplace::MarketplaceManager::new()
            .migrate_mcp_plaintext_secrets()
        {
            Ok(_) => true,
            Err(err) => {
                eprintln!("[pinvou3-app] MCP secret migration skipped: {err}");
                false
            }
        };
        crate::platform::startup::mark("bundle_extract:migrate_mcp_secrets:done");
        // Built-in skills and workflow resources are immutable bundle assets.
        crate::platform::startup::mark("bundle_extract:write_builtin_skills:start");
        self.write_builtin_skills()?;
        crate::platform::startup::mark("bundle_extract:write_builtin_skills:done");
        // 飞书 / 企微 / 钉钉鉴权 CLI 不得阻塞 Tauri setup。启动阶段只沿用上次落盘的完整
        // 技能目录作为缓存；React 首屏提交后调用 refresh_connector_auth_gates 并行
        // 实时探测，再按真实状态修正目录。bundle 升级时仅刷新当前可见的缓存目录。
        crate::platform::startup::mark("bundle_extract:apply_skill_gates:start");
        let feishu_show = self.cached_feishu_skills_visible();
        crate::platform::startup::mark_with_detail(
            "rust",
            "bundle_extract:feishu_cached_gate",
            &format!("show={feishu_show}"),
        );
        if bundle_changed || !feishu_show {
            self.apply_feishu_skills(feishu_show)?;
        }
        let wecom_show = self.cached_wecom_skills_visible();
        crate::platform::startup::mark_with_detail(
            "rust",
            "bundle_extract:wecom_cached_gate",
            &format!("show={wecom_show}"),
        );
        if bundle_changed || !wecom_show {
            self.apply_wecom_skills(wecom_show)?;
        }
        let dingtalk_show = self.cached_dingtalk_skills_visible();
        crate::platform::startup::mark_with_detail(
            "rust",
            "bundle_extract:dingtalk_cached_gate",
            &format!("show={dingtalk_show}"),
        );
        if bundle_changed || !dingtalk_show {
            self.apply_dingtalk_skills(dingtalk_show)?;
        }
        let tmeet_show = self.cached_tmeet_skills_visible();
        crate::platform::startup::mark_with_detail(
            "rust",
            "bundle_extract:tmeet_cached_gate",
            &format!("show={tmeet_show}"),
        );
        if bundle_changed || !tmeet_show {
            self.apply_tmeet_skills(tmeet_show)?;
        }
        crate::platform::startup::mark("bundle_extract:apply_skill_gates:done");
        // MCP server scripts are immutable as well, but wait for secret migration to avoid
        // deleting legacy plaintext before it has been copied into the credential store.
        crate::platform::startup::mark("bundle_extract:write_mcp_servers:start");
        if mcp_secret_migration_ok {
            self.write_mcp_servers()?;
        }
        // mcp.json merge:每次启动 upsert 内置 pinvou server,保留 marketplace 条目。
        // 不受 VERSION gate 限制——marketplace 安装可能在任何时候发生。
        self.ensure_builtin_mcp_servers()?;
        // 启动自愈:刷新 mcp.json 里陈旧的本地 python server command(安装时写死的裸
        // "python" → 重解析成可用路径)。必须在引擎 spawn 前跑(引擎从 mcp.json 拉起 server)。
        self.refresh_mcp_python_commands()?;
        crate::platform::startup::mark("bundle_extract:write_mcp_servers:done");

        if !bundle_changed {
            return Ok(());
        }
        std::fs::create_dir_all(&self.root)?;
        std::fs::create_dir_all(&self.skills_dir)?;
        let workspace_abs = paths::workspace_dir();
        std::fs::create_dir_all(&workspace_abs)?;
        // 首次解包按当前 sudoers 状态填 PINVOU3_SUDO_INSTRUCTION,避免占位符原文
        // 漏到 LLM 看到的 system prompt(engine boot 时是从 disk 读的)。
        // 用户切换开关时 set_super_permission 会 sync_session 重写。
        let rendered = instructions_md()
            .replace("{{PINVOU3_WORKSPACE}}", &workspace_abs.to_string_lossy())
            .replace(
                "{{PINVOU3_SUDO_INSTRUCTION}}",
                crate::platform::super_permission::instruction_block(),
            )
            // 落盘副本无 per-session locale,默认填中文兜底(LLM 实际走 mod.rs 的 inline 渲染,
            // 那里按 locale 填);此处仅防 {{PINVOU3_TITLE_LANG}} 占位符原文残留在 disk 文件。
            .replace("{{PINVOU3_TITLE_LANG}}", "简体中文");
        std::fs::write(&self.instructions_md, rendered)?;
        // PINVOU 自有 hooks：写入 + 加可执行位
        std::fs::write(&self.deny_sensitive_sh, DENY_SENSITIVE_PATHS_SH)?;
        std::fs::write(&self.deny_sensitive_ps1, DENY_SENSITIVE_PATHS_PS1)?;
        std::fs::write(&self.multiagent_depth_guard_sh, MULTIAGENT_DEPTH_GUARD_SH)?;
        std::fs::write(&self.multiagent_depth_guard_ps1, MULTIAGENT_DEPTH_GUARD_PS1)?;
        std::fs::write(&self.shell_env_sh, SHELL_ENV_SH)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for script in [
                &self.deny_sensitive_sh,
                &self.multiagent_depth_guard_sh,
                &self.shell_env_sh,
            ] {
                let mut perm = std::fs::metadata(script)?.permissions();
                perm.set_mode(0o755);
                std::fs::set_permissions(script, perm)?;
            }
        }
        std::fs::write(&version_file, BUNDLE_VERSION)?;
        eprintln!(
            "[pinvou3-app] bundle extracted to {} (version {})",
            self.root.display(),
            BUNDLE_VERSION
        );
        Ok(())
    }

    /// 清理已下线内置 skills 的残留目录(被 ensure_extracted 在 VERSION check 前
    /// 调用,每次启动都跑):
    /// - legacy-ppt-workflow:0.5 下线(workflow 功能转"开发中")
    /// - pinvou-review-plan / pinvou-review-final:0.7 下线(EXIT GATE 评审被推翻)
    ///
    /// 技能市场([`super::skill_marketplace`])装的技能带 `.installed-from` 标记、
    /// 落在同一 `bundle/skills/` 目录。清理时显式跳过带标记的目录——这是保护契约,
    /// 任何未来对 `skills_dir` 的全量重写也必须遵守,否则会误删用户装的技能。
    fn cleanup_retired_skills(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.skills_dir)?;
        for retired in [
            "legacy-ppt-workflow",
            "pinvou-review-plan",
            "pinvou-review-final",
        ] {
            let dir = self.skills_dir.join(retired);
            if dir.join(".installed-from").is_file() {
                continue; // marketplace 技能占用了该名,保护不删
            }
            let _ = std::fs::remove_dir_all(dir);
        }
        Ok(())
    }

    /// 清理已从技能市场移除的预置技能残留(dir 名 → 原市场 id)。
    ///
    /// 与 [`Self::cleanup_retired_skills`] 相反:这些**曾是** marketplace 技能、装时写了
    /// `pinvou3-marketplace:<id>` 标记,所以不能沿用"带标记即跳过"的保护——否则永远删不掉。
    /// 改为**按标记内容精确匹配**:只删标记恰为本技能市场安装(或无标记的裸残留),
    /// 显式跳过 `upload:` 开头的用户上传目录,避免误删用户自己传的同名技能。
    pub(super) fn cleanup_removed_marketplace_skills(&self) -> std::io::Result<()> {
        for (dir_name, market_id) in [
            ("pua", "pua"),
            ("huashu-nuwa", "nuwa"),
            ("brainstorming", "brainstorming"),
        ] {
            let dir = self.skills_dir.join(dir_name);
            if !dir.exists() {
                continue;
            }
            let marker = std::fs::read_to_string(dir.join(".installed-from")).unwrap_or_default();
            let marker = marker.trim();
            if marker.starts_with("upload:") {
                continue; // 用户上传的同名技能,保护不删
            }
            if marker.is_empty() || marker == format!("pinvou3-marketplace:{market_id}") {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
        Ok(())
    }

    /// 清理已从工具市场移除的预置 MCP 工具残留。
    ///
    /// 不能删除所有未知目录:未来/本地可能有自定义 MCP 工具。这里只精确处理曾经内置、
    /// 现在源码资源已经移除的 marketplace 工具。
    pub(super) fn cleanup_removed_marketplace_tools(&self) -> std::io::Result<()> {
        {
            let tool_id = "data_analysis";
            let _ = crate::features::marketplace::MarketplaceManager::new().uninstall(tool_id);

            let mut disabled = crate::features::marketplace::load_disabled_connectors();
            let before = disabled.len();
            disabled.retain(|id| id != tool_id);
            if disabled.len() != before {
                crate::features::marketplace::save_disabled_connectors(&disabled);
            }
            // 代码会话的 code scope 同样清理残留。
            crate::features::marketplace::remove_connector_from_disabled_scopes(tool_id);

            let _ = std::fs::remove_dir_all(paths::bundle_mcp_servers_dir().join(tool_id));
        }
        Ok(())
    }

    /// 解包内嵌的内置 skills 到 pinvou3 单一来源 `bundle/skills`。v0.9 clean re-fork
    /// 后 catalogue 与 `load_skill` 都只扫描此目录，不再写 `~/.agents/skills`。
    /// 每次启动防御性重写(immutable 内置资源)。当前:视觉设计。
    pub(super) fn write_builtin_skills(&self) -> std::io::Result<()> {
        let dir = self.skills_dir.join("visual-design");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("SKILL.md"), VISUAL_DESIGN_SKILL_MD)?;
        Ok(())
    }

    /// 解包内嵌的飞书官方域技能(lark-*)到 `~/.pinvou3/bundle/skills/`。
    /// 每次启动防御性重写（immutable bundle 资源）。`LARK_SKILLS_DIR` 的根对应
    /// `bundle/skills/`,内含 `lark-<域>/SKILL.md` + `references/`,直接铺到
    /// `skills_dir`——引擎 `SkillRegistry` 扫该目录的每个含 `SKILL.md` 的子目录。
    /// (顶层散落的 NOTICE.md 不含 SKILL.md,会被注册表忽略。)
    /// 飞书技能门控:`show` → 解包 9 个 lark 技能到 `skills_dir`;否则**删掉**它们(+ NOTICE.md)。
    /// 幂等(删不存在的目录不报错)。可见性 = 目录在不在,引擎重刷系统提示时重扫即生效。
    pub fn apply_feishu_skills(&self, show: bool) -> std::io::Result<()> {
        if show {
            Self::extract_dir(&LARK_SKILLS_DIR, &self.skills_dir)?;
        } else {
            for d in LARK_SKILL_DIRS {
                let _ = std::fs::remove_dir_all(self.skills_dir.join(d));
            }
            let _ = std::fs::remove_file(self.skills_dir.join("NOTICE.md"));
        }
        Ok(())
    }

    /// 启动缓存只在 9 个飞书域技能全部完整落盘时判 visible，避免上次异常中断留下
    /// 半套目录却被 SkillRegistry 当成已连接。实时真相在首屏后的 CLI 探测中刷新。
    pub(super) fn cached_feishu_skills_visible(&self) -> bool {
        crate::platform::connector_state::feishu_skills_visible()
            && LARK_SKILL_DIRS
                .iter()
                .all(|dir| self.skills_dir.join(dir).join("SKILL.md").is_file())
    }

    /// 企微域技能门控:`show` → 解包 7 个 wecomcli 技能到 `skills_dir`;否则**删掉**它们。
    /// 幂等。与飞书门控正交(各自的连接 / 停用状态独立)。
    /// 注:`WECOM_SKILLS_DIR` 根 = `wecom-skills/`,内含 `wecomcli-<域>/SKILL.md`(+ NOTICE.md);
    /// 直接铺到 `skills_dir`,引擎 `SkillRegistry` 扫每个含 `SKILL.md` 的子目录。
    /// 出处声明用 `NOTICE-wecom.md`(避开飞书的 `NOTICE.md`,两者解包到同一 skills_dir
    /// 不会互相覆盖)。隐藏时一并删掉。
    pub fn apply_wecom_skills(&self, show: bool) -> std::io::Result<()> {
        if show {
            Self::extract_dir(&WECOM_SKILLS_DIR, &self.skills_dir)?;
        } else {
            for d in WECOM_SKILL_DIRS {
                let _ = std::fs::remove_dir_all(self.skills_dir.join(d));
            }
            let _ = std::fs::remove_file(self.skills_dir.join("NOTICE-wecom.md"));
        }
        Ok(())
    }

    /// 同 [`cached_feishu_skills_visible`]，以完整的企微技能目录作为启动缓存。
    pub(super) fn cached_wecom_skills_visible(&self) -> bool {
        crate::platform::connector_state::wecom_skills_visible()
            && WECOM_SKILL_DIRS
                .iter()
                .all(|dir| self.skills_dir.join(dir).join("SKILL.md").is_file())
    }

    /// 钉钉 mono skill 门控:`show` → 解包 `dws` 到 `skills_dir`;否则删除。
    /// 出处声明用 `NOTICE-dingtalk.md`,避免覆盖飞书 / 企微的 NOTICE。
    pub fn apply_dingtalk_skills(&self, show: bool) -> std::io::Result<()> {
        if show {
            Self::extract_dir(&DINGTALK_SKILLS_DIR, &self.skills_dir)?;
        } else {
            for d in DINGTALK_SKILL_DIRS {
                let _ = std::fs::remove_dir_all(self.skills_dir.join(d));
            }
            let _ = std::fs::remove_file(self.skills_dir.join("NOTICE-dingtalk.md"));
        }
        Ok(())
    }

    /// 同 [`cached_feishu_skills_visible`]，以完整的钉钉技能目录作为启动缓存。
    pub(super) fn cached_dingtalk_skills_visible(&self) -> bool {
        crate::platform::connector_state::dingtalk_skills_visible()
            && DINGTALK_SKILL_DIRS
                .iter()
                .all(|dir| self.skills_dir.join(dir).join("SKILL.md").is_file())
    }

    /// 腾讯会议 mono skill 门控:`show` → 解包 `tmeet-skill` 到 `skills_dir`;否则删除。
    /// 出处声明用 `NOTICE-tmeet.md`,避免覆盖其他 CLI 连接器 NOTICE。
    pub fn apply_tmeet_skills(&self, show: bool) -> std::io::Result<()> {
        if show {
            Self::extract_dir(&TMEET_SKILLS_DIR, &self.skills_dir)?;
        } else {
            for d in TMEET_SKILL_DIRS {
                let _ = std::fs::remove_dir_all(self.skills_dir.join(d));
            }
            let _ = std::fs::remove_file(self.skills_dir.join("NOTICE-tmeet.md"));
        }
        Ok(())
    }

    /// 同 [`cached_feishu_skills_visible`]，以完整的腾讯会议技能目录作为启动缓存。
    pub(super) fn cached_tmeet_skills_visible(&self) -> bool {
        crate::platform::connector_state::tmeet_skills_visible()
            && TMEET_SKILL_DIRS
                .iter()
                .all(|dir| self.skills_dir.join(dir).join("SKILL.md").is_file())
    }
    /// 递归解包 `include_dir::Dir` 到磁盘目标路径。
    /// `root` 是磁盘目标根(对应 include_dir 的顶层),`dir` 可以是任意层级子目录。
    /// `Dir::files()` 返回的 `path()` 是相对于 **include_dir 根** 的完整路径
    /// (如 "roles/taizi.md"),所以一律用 `root.join(file.path())` 定位。
    /// 排除 `__pycache__/` 与 `*.pyc`:include_dir! 按文件系统内嵌(不受 .gitignore
    /// 约束),在仓库里直接运行技能脚本产生的 Python 编译缓存若不排除,会被编进
    /// 应用二进制并物化到用户 `~/.pinvou3/bundle/`(跨平台 cpython 版本耦合)。
    fn extract_dir(dir: &Dir<'_>, root: &std::path::Path) -> std::io::Result<()> {
        for file in dir.files() {
            let rel = file.path();
            if rel.components().any(|c| c.as_os_str() == "__pycache__")
                || rel
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("pyc"))
            {
                continue;
            }
            let path = root.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, file.contents())?;
        }
        for sub in dir.dirs() {
            if sub
                .path()
                .components()
                .any(|c| c.as_os_str() == "__pycache__")
            {
                continue;
            }
            Self::extract_dir(sub, root)?;
        }
        Ok(())
    }

    /// mcp.json merge：upsert 内置 pinvou server，保留 marketplace 已安装的条目。
    /// 每次启动都调用（不受 VERSION gate 限制）。
    pub(super) fn ensure_builtin_mcp_servers(&self) -> std::io::Result<()> {
        let present_server = paths::bundle_present_artifact_server();
        let mut mcp: serde_json::Value = if self.mcp_json.is_file() {
            let existing =
                std::fs::read_to_string(&self.mcp_json).unwrap_or_else(|_| "{}".to_string());
            serde_json::from_str(&existing).unwrap_or_else(|_| serde_json::json!({"servers": {}}))
        } else {
            serde_json::json!({"servers": {}})
        };
        if mcp.get("servers").and_then(|s| s.as_object()).is_none() {
            mcp.as_object_mut()
                .unwrap()
                .insert("servers".into(), serde_json::json!({}));
        }
        let servers = mcp["servers"].as_object_mut().unwrap();
        // 迁移:旧版 server key 是 `pinvou`(与产品名 `pinvou3` 差一个 3,模型采样必漂成
        // pinvou3 → `Failed to find MCP server: pinvou3`)。改用 `pinvou3` 对齐产品名,并删掉
        // 旧 `pinvou` 条目——upsert 不会自动删旧名,不删会留两个指向同一脚本的 server。
        servers.remove("pinvou");
        // Windows 用内置 pythonw(无窗口 + 自带依赖);其他平台系统 python3。见 paths::python_command。
        let python_cmd = paths::python_command();
        servers.insert(
            "pinvou3".to_string(),
            serde_json::json!({
                "command": python_cmd,
                "args": [present_server.to_string_lossy()]
            }),
        );
        let json = serde_json::to_string_pretty(&mcp).map_err(std::io::Error::other)?;
        std::fs::write(&self.mcp_json, json)
    }

    /// 启动自愈:`mcp.json` 里本地 python server 的 `command` 是**安装时写死**的,老条目
    /// 常是裸 `"python"`/`"python3"` —— 在没把 python 加进 PATH 的机器(或只有 python3 的
    /// Linux)上永远拉不起来(高德天气等 marketplace 工具静默失效)。每次启动重解析:凡
    /// command 是裸 python 家族名、或指向不存在的 python 路径,统一替换成当前
    /// `paths::python_command()`。`url` 型远程 server / 非 python command 一律不动。
    fn refresh_mcp_python_commands(&self) -> std::io::Result<()> {
        if !self.mcp_json.is_file() {
            return Ok(());
        }
        let existing = std::fs::read_to_string(&self.mcp_json).unwrap_or_default();
        let mut mcp: serde_json::Value = match serde_json::from_str(&existing) {
            Ok(v) => v,
            Err(_) => return Ok(()), // 坏 json 不碰
        };
        let resolved = paths::python_command();
        let mut changed = false;
        if let Some(servers) = mcp.get_mut("servers").and_then(|s| s.as_object_mut()) {
            for (_name, entry) in servers.iter_mut() {
                let Some(obj) = entry.as_object_mut() else {
                    continue;
                };
                let Some(cmd) = obj.get("command").and_then(|c| c.as_str()) else {
                    continue; // url 型远程 server 无 command 字段
                };
                if cmd != resolved && Self::is_stale_python_command(cmd) {
                    obj.insert(
                        "command".to_string(),
                        serde_json::Value::String(resolved.clone()),
                    );
                    changed = true;
                }
            }
        }
        if changed {
            let json = serde_json::to_string_pretty(&mcp).map_err(std::io::Error::other)?;
            std::fs::write(&self.mcp_json, json)?;
        }
        Ok(())
    }

    /// command 是否是"需要重解析"的 python:裸解释器名(python/python3/pythonw[.exe]),
    /// 或指向一个已不存在的 python 路径。非 python command 一律 false,绝不误伤别的工具。
    fn is_stale_python_command(cmd: &str) -> bool {
        let lower = cmd.to_ascii_lowercase();
        let bare = !cmd.contains('/') && !cmd.contains('\\');
        if bare {
            return matches!(
                lower.as_str(),
                "python" | "python3" | "pythonw" | "python.exe" | "pythonw.exe" | "python3.exe"
            );
        }
        // 带路径但文件不存在、且看起来是 python → 重解析(指向已删/搬走的解释器)
        lower.contains("python") && !std::path::Path::new(cmd).exists()
    }

    /// 写出内置 MCP server 脚本到 `~/.pinvou3/bundle/mcp-servers/` + 加可执行位。
    /// 每次启动防御性重写(immutable bundle 资源,无副作用)。底座按 mcp.json
    /// 用 `python <path>` 拉起,不依赖可执行位,但 chmod +x 无害。
    fn write_mcp_servers(&self) -> std::io::Result<()> {
        let dir = paths::bundle_mcp_servers_dir();
        std::fs::create_dir_all(&dir)?;
        // pinvou 内置 present_artifact server
        let server = paths::bundle_present_artifact_server();
        std::fs::write(&server, PRESENT_ARTIFACT_SERVER_PY)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = std::fs::metadata(&server)?.permissions();
            perm.set_mode(0o755);
            std::fs::set_permissions(&server, perm)?;
        }
        // 工具市场：天气 MCP server
        let weather_dir = dir.join("weather");
        std::fs::create_dir_all(&weather_dir)?;
        std::fs::write(weather_dir.join("server.py"), WEATHER_SERVER_PY)?;
        std::fs::write(weather_dir.join("manifest.json"), WEATHER_MANIFEST_JSON)?;
        // 工具市场：同花顺问财 MCP server
        let iwencai_dir = dir.join("iwencai");
        std::fs::create_dir_all(&iwencai_dir)?;
        std::fs::write(iwencai_dir.join("server.py"), IWENCAI_SERVER_PY)?;
        std::fs::write(iwencai_dir.join("manifest.json"), IWENCAI_MANIFEST_JSON)?;
        // 工具市场：企查查（远程 MCP，只有 manifest.json，无 server.py）
        let qcc_dir = dir.join("qcc");
        std::fs::create_dir_all(&qcc_dir)?;
        std::fs::write(qcc_dir.join("manifest.json"), QCC_MANIFEST_JSON)?;
        // 工具市场：华宇元典（远程 MCP + OAuth，只有 manifest.json，无 server.py）
        let yuandian_dir = dir.join("yuandian-mcp");
        std::fs::create_dir_all(&yuandian_dir)?;
        std::fs::write(yuandian_dir.join("manifest.json"), YUANDIAN_MANIFEST_JSON)?;
        // 工具市场：Canva 可画（远程 MCP + OAuth，只有 manifest.json，无 server.py）
        let canva_dir = dir.join("canva-mcp");
        std::fs::create_dir_all(&canva_dir)?;
        std::fs::write(canva_dir.join("manifest.json"), CANVA_MCP_MANIFEST_JSON)?;
        // 工具市场：智慧芽专利&文献融合检索（远程 MCP，API Key 通过请求头占位符注入）
        let patsnap_dir = dir.join("patsnap-search");
        std::fs::create_dir_all(&patsnap_dir)?;
        std::fs::write(
            patsnap_dir.join("manifest.json"),
            PATSNAP_SEARCH_MANIFEST_JSON,
        )?;
        // 工具市场：Obsidian 知识库 MCP server（本地 stdio，检索本机 vault）
        let obsidian_dir = dir.join("obsidian");
        std::fs::create_dir_all(&obsidian_dir)?;
        std::fs::write(obsidian_dir.join("server.py"), OBSIDIAN_SERVER_PY)?;
        std::fs::write(obsidian_dir.join("manifest.json"), OBSIDIAN_MANIFEST_JSON)?;
        // 工具市场：PPT 生成 MCP server（本地 stdio，python-pptx 直出 .pptx；非零依赖，装时自动 pip install）
        let pptx_dir = dir.join("pptx");
        std::fs::create_dir_all(&pptx_dir)?;
        std::fs::write(pptx_dir.join("server.py"), PPTX_SERVER_PY)?;
        std::fs::write(pptx_dir.join("manifest.json"), PPTX_MANIFEST_JSON)?;
        // 工具市场：公文写作 MCP server（本地 stdio，python-docx 直出 GB/T 9704 .docx；
        // 比别的多一个 gbt9704_styles.py 渲染模块，server.py 同目录 import 它）
        let gongwen_dir = dir.join("gongwen");
        std::fs::create_dir_all(&gongwen_dir)?;
        std::fs::write(gongwen_dir.join("server.py"), GONGWEN_SERVER_PY)?;
        std::fs::write(gongwen_dir.join("manifest.json"), GONGWEN_MANIFEST_JSON)?;
        std::fs::write(gongwen_dir.join("gbt9704_styles.py"), GONGWEN_STYLES_PY)?;
        Ok(())
    }
}

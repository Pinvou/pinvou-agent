//! Bundle 提取逻辑:版本比对、skills/workflow 解包、连接器技能门控、MCP server 写入。
//!
//! 从 mod.rs 抽离——mod.rs 保留 fork-guard 指纹(install_prompt_overrides 的
//! set_static_prompt_composer_override + tests 的 forkguard_builtin_visual_skill)
//! 与提示词静态层常量;本模块只含运行时提取逻辑,通过 `use super::*` 复用
//! mod.rs 的资产常量(SKILL_DIRS/Dir/MANIFEST/SERVER_PY 等)。

use super::*;

#[cfg(target_os = "linux")]
fn find_webkit_webdriver() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PINVOU3_WEBKIT_WEBDRIVER_BIN")
        .map(PathBuf::from)
        .filter(|path| crate::platform::filesystem::is_executable_file(path))
    {
        return Some(path);
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(|directory| directory.join("WebKitWebDriver"))
        .find(|candidate| crate::platform::filesystem::is_executable_file(candidate))
}

#[derive(Debug, Clone)]
pub struct Pinvou3Bundle {
    pub root: PathBuf,
    pub skills_dir: PathBuf,
    pub mcp_json: PathBuf,
    pub deny_sensitive_sh: PathBuf,
    pub deny_sensitive_ps1: PathBuf,
    pub multiagent_depth_guard_sh: PathBuf,
    pub multiagent_depth_guard_ps1: PathBuf,
    pub shell_env_sh: PathBuf,
}

const BROWSER_LAST_ERROR_TTL_SECS: u64 = 24 * 60 * 60;
const BROWSER_LAST_ERROR_MAX_FUTURE_SKEW_SECS: u64 = 5 * 60;
// Keep this table enumerable so tests can verify that the JavaScript wrapper
// persists exactly the codes for which Rust provides model-visible hints.
pub(super) const BROWSER_LAST_ERROR_HINTS: &[(&str, &str)] = &[
    (
        "browser/host-backend-unavailable",
        "The in-app browser host is not ready.",
    ),
    (
        "unsupported/host-backend-unavailable",
        "No in-app native browser automation backend is available on this platform.",
    ),
    (
        "browser/node-runtime-too-old",
        "The Node.js runtime required by Browser MCP is incompatible.",
    ),
    (
        "browser/mcp-runtime-start-failed",
        "The Browser MCP runtime failed to start.",
    ),
    (
        "browser/core-backend-unavailable",
        "The in-app BrowserCore automation backend is not ready.",
    ),
    (
        "browser/webkit-webdriver-not-found",
        "WebKitWebDriver was not found on this system.",
    ),
    (
        "browser/webkit-webdriver-unavailable",
        "WebKitWebDriver is currently unavailable.",
    ),
    (
        "browser/webkit-webdriver-session-timeout",
        "The WebKitWebDriver session timed out during startup.",
    ),
];

/// Parse the wrapper's deliberately small persistence contract. The returned
/// text is compiled into the app rather than copied from disk, so legacy
/// `reason` values and crafted paths can never cross into model instructions.
pub(super) fn browser_last_error_hint(raw: &str, now: u64) -> Option<(&'static str, &'static str)> {
    let state = serde_json::from_str::<serde_json::Value>(raw).ok()?;
    let code = state.get("code")?.as_str()?;
    let at = state.get("at")?.as_u64()?;
    if at == 0
        || at > now.saturating_add(BROWSER_LAST_ERROR_MAX_FUTURE_SKEW_SECS)
        || now.saturating_sub(at) > BROWSER_LAST_ERROR_TTL_SECS
    {
        return None;
    }
    BROWSER_LAST_ERROR_HINTS
        .iter()
        .copied()
        .find(|(candidate, _)| *candidate == code)
}

impl Pinvou3Bundle {
    pub fn paths() -> Self {
        Self {
            root: paths::bundle_root(),
            skills_dir: paths::bundle_skills_dir(),
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
    /// 引擎侧 system prompt 走 `mod.rs` 的 `instructions_md()` 内嵌渲染
    /// （per-session locale / workspace / sudo 占位符在会话渲染层就地替换），
    /// 不再落盘 `bundle/instructions.md` 副本。
    pub fn ensure_extracted(&self) -> std::io::Result<()> {
        let marketplace = crate::features::marketplace::MarketplaceManager::new();
        self.ensure_extracted_with_marketplace(&marketplace, |manager| {
            manager.repair_installed_python_tools()
        })
    }

    pub(super) fn ensure_extracted_with_marketplace<S, F>(
        &self,
        marketplace: &crate::features::marketplace::MarketplaceManager<S>,
        repair_python_tools: F,
    ) -> std::io::Result<()>
    where
        S: crate::platform::credential_store::CredentialStore,
        F: FnOnce(
            &crate::features::marketplace::MarketplaceManager<S>,
        ) -> Result<Vec<String>, String>,
    {
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
        let mcp_secret_migration_ok = match marketplace.migrate_mcp_plaintext_secrets() {
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
        // 首启一次性导入旧布局安装态到 BundleStore（marketplace-unification §9）。
        // 位置：cleanups 之后（退役目录不导入）、技能迁移与 gates 之前（迁移按 import
        // 的登记反推归属；gates 会把 CLI companion 技能解包到新布局，见下）；manifest
        // 清单取自内嵌目录，不再依赖 write_mcp_servers 的落盘。必须在
        // `if !bundle_changed` 提前返回之前——bundle 版本不变的老用户首次跑到新版本时
        // 也要完成导入；`legacy_imported` 闸使后续启动成为读一次的廉价 no-op。
        Self::import_legacy_bundle_store();
        // 强制迁移自定义 MCP（不在内嵌目录）到新布局：bundle/mcp-servers/<id>/ →
        // bundles/<id>/mcp/。排在技能迁移之前（四轮评审 M-7）：迁完后 available_tools
        // 才能从新布局读到自定义 MCP manifest 的 companion_skills 声明，技能迁移的
        // companion 归属（条件认领）才有直接依据，不必只靠旧布局现算兜底。沿用原
        // write_mcp_servers 的门控：明文密钥迁移失败时不搬动旧目录（保留可救援副本），
        // 此时技能迁移回退旧布局现算映射（legacy_companion_owners），口径一致。幂等。
        if mcp_secret_migration_ok {
            match crate::features::marketplace::migrate_custom_mcp_layout() {
                Ok((moved, kept)) => {
                    if moved > 0 || kept > 0 {
                        log::info!("[runtime-bundle] 自定义 MCP 迁移: moved={moved} kept={kept}");
                    }
                }
                Err(e) => log::warn!("[runtime-bundle] 自定义 MCP 迁移失败: {e}"),
            }
        }
        // 扁平技能布局 → 按包聚合的一次性物理迁移（§9.1，刀十）。排在 import 之后
        // （import 按旧布局反推登记，迁移随后把目录搬进 bundles/<pkg>/skills/ 并补写
        // 预置技能指纹）、gates 之前：已连接 CLI 的存量用户首启时，若 gates 先把技能
        // 解包到新布局，迁移会撞 move_skill_dir 的 target.exists()，每次启动都留下
        // warn 与双份物理拷贝；先迁移则 gates 的防御性重写天然幂等。单个目录失败
        // 保留旧位置（读路径 find_skill_dir 回退）。
        let migration =
            crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new()
                .migrate_flat_skills_layout();
        crate::platform::startup::mark_with_detail(
            "rust",
            "bundle_extract:skills_migration:done",
            &format!(
                "moved={} stale={}",
                migration.moved.len(),
                migration.removed_stale.len()
            ),
        );
        // 自愈对账：认领错位归位/去重、孤儿副本、瘫记录、内置释放目录残旧收敛。
        // 名称无关（按归属证明判定，各发行版构建自动适配），排在布局迁移之后
        // （旧扁平目录已搬完）、gates 之前（CLI 静态所有目录不受影响）。
        let heal = crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new()
            .self_heal_skills(Self::BUILTIN_RELEASED_SKILL_DIRS);
        crate::platform::startup::mark_with_detail(
            "rust",
            "bundle_extract:skills_self_heal:done",
            &format!(
                "rehomed={} deduped={} orphan={} stale_records={} converged={}",
                heal.rehomed.len(),
                heal.deduped.len(),
                heal.removed_orphan_dirs.len(),
                heal.removed_stale_records.len(),
                heal.converged_builtin_dirs.len()
            ),
        );
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
        self.write_mcp_servers(mcp_secret_migration_ok)?;
        // Reconcile → Python repair → builtin upsert, in that order; see
        // `run_mcp_startup_maintenance` for why the order is load-bearing.
        // Coverage boundary: tests pin the order and the corrupt-file gate
        // *inside* `run_mcp_startup_maintenance`
        // (`startup_maintenance_restores_missing_entry_before_python_repair`,
        // `startup_maintenance_preserves_corrupt_mcp_json_bytes`); this call
        // site itself is driven end to end by
        // `ensure_extracted_preserves_corrupt_mcp_json_bytes`, which runs the
        // real boot chain including the retired-tool cleanup that precedes it.
        self.run_mcp_startup_maintenance(marketplace, repair_python_tools)?;
        crate::platform::startup::mark("bundle_extract:write_mcp_servers:done");

        if !bundle_changed {
            return Ok(());
        }
        std::fs::create_dir_all(&self.root)?;
        std::fs::create_dir_all(&self.skills_dir)?;
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

    /// MCP startup maintenance, in a fixed order: reconcile the marketplace
    /// registry into mcp.json first (the managed-runtime patch fails while an
    /// entry is missing entirely, so restoring missing/dead entries first lets
    /// one startup converge to the managed form), then the Python repair
    /// (upgrades a restored entry to the managed-runtime form before any engine
    /// reads mcp.json), then the builtin-server upsert. Engine-owned keys
    /// (pinvou3/pinvou/browser — see `ENGINE_OWNED_MCP_SERVER_KEYS`) are skipped
    /// by the reconcile, and `ensure_builtin_mcp_servers` re-asserts them
    /// afterwards; note it also runs `refresh_mcp_python_commands`, which may
    /// rewrite the python command of any stale entry — a complementary self-heal
    /// outside the key-set footprint.
    ///
    /// A corrupt mcp.json is preserved for the whole boot: the reconcile backs
    /// it up and returns early, and the builtin upsert below is skipped (its
    /// repair loader would reset the file to a builtin-only skeleton). Sessions
    /// degrade to an empty MCP pool rather than fail, and the recovery path is
    /// the backup plus the timeline note. Pinned end to end by
    /// `startup_maintenance_preserves_corrupt_mcp_json_bytes`.
    ///
    /// The ordering and the reconcile call itself are load-bearing and pinned by
    /// `startup_maintenance_restores_missing_entry_before_python_repair`:
    /// deleting the reconcile call, or moving it after the repair, fails that
    /// test. Reconcile actions go through the startup timeline: release builds
    /// register no log sink, so `log::info!` alone would leave the outcomes
    /// invisible. Reconcile failures never block startup; the repair closure's
    /// integrity result and the builtin upsert's structural errors propagate to
    /// the caller, which treats them like any other extraction failure.
    pub(super) fn run_mcp_startup_maintenance<S, F>(
        &self,
        marketplace: &crate::features::marketplace::MarketplaceManager<S>,
        repair_python_tools: F,
    ) -> std::io::Result<Vec<String>>
    where
        S: crate::platform::credential_store::CredentialStore,
        F: FnOnce(
            &crate::features::marketplace::MarketplaceManager<S>,
        ) -> Result<Vec<String>, String>,
    {
        let reconcile_actions = match marketplace.reconcile_installed_mcp_entries() {
            Ok(actions) => actions,
            Err(error) => {
                log::warn!("[pinvou3-app] mcp.json reconcile failed (non-blocking): {error}");
                crate::platform::startup::mark_with_detail("rust", "mcp_reconcile:failed", &error);
                Vec::new()
            }
        };
        for action in &reconcile_actions {
            log::info!("[pinvou3-app] mcp.json reconcile: {action}");
            crate::platform::startup::mark_with_detail("rust", "mcp_reconcile", action);
        }
        // Existing Python MCP entries may still launch server.py directly without the managed
        // dependency environment. Repair or atomically downgrade them before any engine reads
        // mcp.json, leaving a stable install target for the UI to retry.
        let repair_errors = repair_python_tools(marketplace).map_err(std::io::Error::other)?;
        for error in repair_errors {
            // The packaged Windows GUI has no stderr; the log is the only way users see downgrade/retry outcomes.
            log::warn!("[pinvou3-app] {error}");
        }
        // mcp.json merge:每次启动 upsert 内置 pinvou server,保留 marketplace 条目。
        // 不受 VERSION gate 限制——marketplace 安装可能在任何时候发生。启动自愈(刷新
        // 陈旧的本地 python server command)也在同一次调用里完成,两者共享一次读盘
        // +parse;必须在引擎 spawn 前跑(引擎从 mcp.json 拉起 server)。
        // 损坏文件例外:reconcile 刚刚备份过的坏文件绝不能在这里被 repair loader
        // 重置成 builtin-only 骨架——那会毁掉备份刚保护下来的自定义条目。跳过本次
        // upsert(引擎对解析失败降级为空 MCP 池,会话不受阻),恢复路径 = 备份 +
        // timeline 提示。检测独立于 reconcile 的返回值:即使 reconcile 因其它原因
        // 提前失败,这条防线依然挡住对坏文件的重写。
        if crate::features::marketplace::mcp_json_unparseable() {
            let note = "mcp.json is unparseable; builtin MCP upsert skipped this boot to \
                        preserve the backed-up original — fix or delete the file to restore \
                        MCP servers";
            log::warn!("[pinvou3-app] {note}");
            crate::platform::startup::mark_with_detail("rust", "mcp_builtin_skip", note);
        } else {
            // 本函数对 mcp.json 的读-改-写绕过 connectors 写入器(不带内建锁),
            // 与并发安装/卸载的写入器互斥由这把锁补齐;boot 链此处无外层持锁,
            // 函数体内也无嵌套取锁,直接取即可。
            let _guard = crate::features::marketplace::mcp_json_lock();
            self.ensure_builtin_mcp_servers()?;
        }
        Ok(reconcile_actions)
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
            // 退役 id 保护（二轮评审）：`bundles/` 已是用户上传落盘区，用户可能上传过
            // 同名包——其 Upload 记录存在时跳过整段清理（不删登记、不删 mcp.json），
            // 只清理确定无主的内嵌退役残留。
            // Fail-closed like the uninstall path's `source_may_be_upload`: an
            // unreadable store means "may be an upload" → skip the cleanup
            // entirely (its last step deletes `bundles/<id>` wholesale).
            let user_uploaded = crate::features::marketplace::store::BundleStore::new()
                .records()
                .map(|records| {
                    records.iter().any(|r| {
                        r.id == tool_id
                            && matches!(
                                r.source,
                                crate::features::marketplace::store::BundleSource::Upload(_)
                            )
                    })
                })
                .unwrap_or(true);
            if !user_uploaded {
                // 廉价残留探测:所有清理面都干净时直接返回——uninstall 会无条件重写
                // installed.json / mcp.json(manifest 声明 secret_targets 时还会清
                // 系统 keyring),不值得每次启动都实例化 MarketplaceManager 跑一遍。
                // 探测只读私有布局文件,不实例化管理器;BundleStore 记录的读取开销
                // 与探测同量级,保护判定前置不亏。
                if !Self::marketplace_tool_residue_present(tool_id) {
                    return Ok(());
                }
                // 卸载失败(如今最常见的是损坏 mcp.json 拒绝写入器)必须留日志:
                // 此前 `let _ =` 无声吞掉,损坏窗口内每个启动都静默复发。
                let uninstall_error = crate::features::marketplace::MarketplaceManager::new()
                    .uninstall(tool_id)
                    .err();
                // 禁用/隐藏残留统一走 scope 模块的单临界区 RMW 助手:一次
                // DISABLED_BUNDLES_FILE_LOCK 内 load→retain→条件 save(#455 收敛范式),
                // plain + code 所有 scope 的 disabled/hidden 两套集合一并清理。此前
                // plain 走「load → 内存 retain → 条件 save」两段独立取锁的临界区,
                // 两段之间并发写方的更新会被旧快照整表覆盖(#522,与 #455 修复的 M-6b
                // 两段式同型)。uninstall 成功时其内部清理已覆盖本步,这次幂等复扫是
                // 纵深防御;回滚时本步是唯一清理面,不可省——该残留不在事务快照内,
                // 留着会让未来同名重装被误隐藏(#522)。
                // A refused cleanup (cross-process lock unavailable, #515) only
                // logs: the leftover entry fails closed and the install itself
                // already succeeded.
                if let Err(error) =
                    crate::features::marketplace::scope::remove_bundle_from_disabled_scopes(tool_id)
                {
                    eprintln!("[runtime-bundle] scope cleanup for {tool_id} skipped: {error}");
                }
                if let Some(error) = uninstall_error {
                    // 回滚说明工具仍登记在册:目录删除随之跳过,不销毁在册工具的
                    // 包目录;登记与目录都是残留探测面,下次启动会重试整套清理。
                    // 打包版 Windows GUI 无 stderr:timeline 是用户能看到这条
                    // 推迟的唯一渠道(与 reconcile 失败的上报同一范式)。
                    log::warn!(
                        "[cleanup] retired tool '{tool_id}' uninstall deferred: {error}; \
                         retrying on the next startup"
                    );
                    crate::platform::startup::mark_with_detail(
                        "rust",
                        "retired_tool_cleanup:deferred",
                        &error,
                    );
                    return Ok(());
                }

                let _ = std::fs::remove_dir_all(paths::bundle_mcp_servers_dir().join(tool_id));
                // 按包聚合新布局的退役残留：`migrate_custom_mcp_layout` 会先把旧目录
                // 搬进 bundles/<id>/mcp/，而 uninstall 的 can_redeliver=false 规则
                // （非内嵌 id 不可重释放）保留包目录——不删则 manifest 存活、退役
                // 工具以「自定义 MCP 卡」复活（G8a）。Upload 保护已在上方判定。
                let _ = std::fs::remove_dir_all(paths::bundles_root().join(tool_id));
            }
        }
        Ok(())
    }

    /// 探测已下架 marketplace 工具是否还有任何残留清理面:安装目录、installed.json、
    /// 禁用/隐藏列表落盘、mcp.json server 条目。全干净 → false(调用方据此跳过
    /// MarketplaceManager 实例化 + uninstall)。installed.json 与禁用集落盘是
    /// marketplace 模块的私有布局,这里按其落盘路径直读做 contains 级探测——宁可
    /// 误报(多跑一次幂等清理)也不漏报(残留永驻)。
    fn marketplace_tool_residue_present(tool_id: &str) -> bool {
        if paths::bundle_mcp_servers_dir().join(tool_id).exists() {
            return true;
        }
        // 新布局包目录（含迁移搬入的 bundles/<id>/mcp/）也是残留面——否则只剩
        // 它时探测漏报，退役工具的 manifest 会以自定义 MCP 卡复活。
        if paths::bundles_root().join(tool_id).exists() {
            return true;
        }
        let home = paths::pinvou3_home();
        // installed.json = ~/.pinvou3/marketplace/installed.json(镜像 MarketplaceManager
        // 私有 installed_file 布局)。禁用/隐藏残留的落盘真相源是 scope 收敛后的单一
        // disabled_bundles.json(plain + 所有 scope 的 disabled/hidden 集,镜像
        // marketplace::scope 私有布局);漏掉它会把「仅禁用/隐藏集有残留」的退役工具
        // 误判干净、跳过清理,陈旧条目继续误隐藏未来同名重装(#522)。旧布局
        // disabled_connectors.json 不探测:首个 scope 读路径「读到即迁移」把其内容
        // 并进统一文件,此后它只剩死数据(uninstall 与 scope 写方都不再碰它),探测
        // 死数据只会对已迁移用户产生永不收敛的误报;同样的残留最晚由统一文件面在
        // 下一次启动收敛。
        for probe in ["marketplace/installed.json", "disabled_bundles.json"] {
            let path = home.join(probe);
            if !path.exists() {
                continue; // 文件不存在 = 该清理面本就干净,不算误报
            }
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    if content.contains(tool_id) {
                        return true;
                    }
                }
                // 文件存在但读失败(权限/损坏):保守视为有残留——宁可误报(多跑
                // 一次幂等清理)也不漏报(残留永驻)。
                Err(_) => return true,
            }
        }
        // mcp.json 按结构探测:server key 存在即残留;坏 json 保守视为有残留——
        // uninstall 会在写入器处拒绝并整体回滚,清理推迟到文件修复后的下次启动。
        if !paths::mcp_config_path().is_file() {
            return false;
        }
        match std::fs::read_to_string(paths::mcp_config_path())
            .ok()
            .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        {
            Some(mcp) => mcp
                .get("servers")
                .and_then(|servers| servers.get(tool_id))
                .is_some(),
            None => true,
        }
    }

    /// 当前构建内嵌释放到 `bundle/skills/` 的内置技能目录集合——与
    /// [`Self::write_builtin_skills`] 的释放面保持一致。自愈对账
    /// （`self_heal_skills` 第 4 步）以此为「本构建内嵌什么」的判定基准：
    /// 不在集内且无归属证明的目录判残旧收敛；其它发行版构建内嵌更多技能
    /// 时扩充本常量即可，天然保留。
    /// Binding tests (same pattern as `wecom_skill_dirs_match_embedded_resources`):
    /// `builtin_released_skill_dirs_match_embedded_resources` and
    /// `write_builtin_skills_releases_exactly_the_listed_dirs` — a builtin
    /// skill added to `write_builtin_skills` without extending this constant
    /// would be written and self-heal-deleted in the same run.
    pub(crate) const BUILTIN_RELEASED_SKILL_DIRS: &[&str] = &["visual-design"];

    /// 解包内嵌的内置 skills 到 pinvou3 单一来源 `bundle/skills`。v0.9 clean re-fork
    /// 后 catalogue 与 `load_skill` 都只扫描此目录，不再写 `~/.agents/skills`。
    /// 每次启动防御性写出(immutable 内置资源);内容一致则跳过写——这些资源不进
    /// BUNDLE_VERSION 的 hash,升级改内容但 VERSION 不变时靠逐文件比对兜住。
    /// 当前:视觉设计。
    pub(super) fn write_builtin_skills(&self) -> std::io::Result<()> {
        let dir = self.skills_dir.join("visual-design");
        self.write_if_changed(&dir.join("SKILL.md"), VISUAL_DESIGN_SKILL_MD)?;
        Ok(())
    }

    /// 解包内嵌的飞书官方域技能(lark-*)到 `bundles/feishu/skills/`。
    /// 每次启动防御性重写（immutable bundle 资源）。`LARK_SKILLS_DIR` 的根对应
    /// 包内 `skills/`,内含 `lark-<域>/SKILL.md` + `references/`,直接铺到目标——
    /// 引擎 `SkillRegistry` 扫该目录的每个含 `SKILL.md` 的子目录。
    /// (顶层散落的 NOTICE.md 不含 SKILL.md,会被注册表忽略。)
    /// 飞书技能门控:`show` → 解包 9 个 lark 技能到包目录;否则**删掉**它们(+ NOTICE.md)。
    /// 幂等(删不存在的目录不报错)。可见性 = 目录在不在,引擎重刷系统提示时重扫即生效。
    fn connector_package_skills_dir(id: &str) -> std::path::PathBuf {
        paths::bundles_root().join(id).join("skills")
    }

    /// CLI 连接器技能门的共享实现：`show` 为真时把内嵌技能目录解包到
    /// `bundles/<id>/skills/`；否则移除技能目录 + NOTICE。幂等（目录本就不存在
    /// 时移除不算错误）。可见性 = 目录是否存在；引擎在下次 system-prompt
    /// 刷新时重扫。
    ///
    /// 四个 `apply_*_skills` 包装器原是近似重复（差异仅在内嵌目录、目录表和
    /// NOTICE 文件名），已折叠为这个表驱动助手。wecom 的包装器额外把
    /// legacy 0.1.9 目录清理内联保留在本助手之外。
    fn apply_connector_skills(
        connector_id: &str,
        embedded_dir: &Dir<'_>,
        skill_dirs: &[&str],
        notice_file: &str,
        show: bool,
    ) -> std::io::Result<()> {
        let target = Self::connector_package_skills_dir(connector_id);
        if show {
            Self::extract_dir(embedded_dir, &target)?;
        } else {
            for d in skill_dirs {
                let _ = std::fs::remove_dir_all(target.join(d));
            }
            let _ = std::fs::remove_file(target.join(notice_file));
        }
        Ok(())
    }

    /// 启动缓存的公共判定:启动缓存状态 + 连接器技能目录**全部完整落盘**才判
    /// visible,避免上次异常中断留下半套目录却被 SkillRegistry 当成已连接。
    /// 实时真相在首屏后的 CLI 探测中刷新。
    fn cached_connector_skills_visible(
        &self,
        connector_id: &str,
        skill_dirs: &[&str],
        state_fn: impl Fn() -> bool,
    ) -> bool {
        let target = Self::connector_package_skills_dir(connector_id);
        state_fn()
            && skill_dirs
                .iter()
                .all(|dir| target.join(dir).join("SKILL.md").is_file())
    }

    /// 飞书域技能门控:`show` → 解包 9 个 lark 技能到包目录;否则**删掉**它们(+ NOTICE.md)。
    /// `LARK_SKILLS_DIR` 的根对应包内 `skills/`,内含 `lark-<域>/SKILL.md` + `references/`,
    /// 直接铺到目标——引擎 `SkillRegistry` 扫该目录的每个含 `SKILL.md` 的子目录。
    /// (顶层散落的 NOTICE.md 不含 SKILL.md,会被注册表忽略。)
    pub fn apply_feishu_skills(&self, show: bool) -> std::io::Result<()> {
        Self::apply_connector_skills(
            "feishu",
            &LARK_SKILLS_DIR,
            &LARK_SKILL_DIRS,
            "NOTICE.md",
            show,
        )
    }
    /// 启动缓存只在 9 个飞书域技能全部完整落盘时判 visible，避免上次异常中断留下
    /// 半套目录却被 SkillRegistry 当成已连接。实时真相在首屏后的 CLI 探测中刷新。
    pub(super) fn cached_feishu_skills_visible(&self) -> bool {
        self.cached_connector_skills_visible(
            "feishu",
            &LARK_SKILL_DIRS,
            crate::platform::connector_state::feishu_skills_visible,
        )
    }

    /// 企微域技能门控:`show` → 解包 14 个 wecomcli 技能到包目录;否则**删掉**它们。
    /// 幂等。与飞书门控正交(各自的连接 / 停用状态独立)。
    /// 注:`WECOM_SKILLS_DIR` 根 = `wecom-skills/`,内含 `wecomcli-<域>/SKILL.md`;
    /// 直接铺到 `bundles/wecom/skills/`,引擎 `SkillRegistry` 扫每个含 `SKILL.md` 的子目录。
    /// 出处声明用 `NOTICE-wecom.md`(避开飞书的 `NOTICE.md`,两者解包到同一 skills_dir
    /// 不会互相覆盖)。
    pub fn apply_wecom_skills(&self, show: bool) -> std::io::Result<()> {
        // 0.1.9 时代的旧目录（服务改名前）在旧扁平布局下清理，无论显示与否，
        // 防残留技能教已死的命令(`msg`/`schedule`)。
        for d in WECOM_LEGACY_SKILL_DIRS {
            let _ = std::fs::remove_dir_all(self.skills_dir.join(d));
        }
        Self::apply_connector_skills(
            "wecom",
            &WECOM_SKILLS_DIR,
            &WECOM_SKILL_DIRS,
            "NOTICE-wecom.md",
            show,
        )
    }
    /// 同 [`cached_feishu_skills_visible`]，以完整的企微技能目录作为启动缓存。
    pub(super) fn cached_wecom_skills_visible(&self) -> bool {
        self.cached_connector_skills_visible(
            "wecom",
            &WECOM_SKILL_DIRS,
            crate::platform::connector_state::wecom_skills_visible,
        )
    }

    /// 钉钉 mono skill 门控:`show` → 解包 `dws` 到包目录;否则删除。
    /// 出处声明用 `NOTICE-dingtalk.md`,避免覆盖飞书 / 企微的 NOTICE。
    pub fn apply_dingtalk_skills(&self, show: bool) -> std::io::Result<()> {
        Self::apply_connector_skills(
            "dingtalk",
            &DINGTALK_SKILLS_DIR,
            &DINGTALK_SKILL_DIRS,
            "NOTICE-dingtalk.md",
            show,
        )
    }
    /// 同 [`cached_feishu_skills_visible`]，以完整的钉钉技能目录作为启动缓存。
    pub(super) fn cached_dingtalk_skills_visible(&self) -> bool {
        self.cached_connector_skills_visible(
            "dingtalk",
            &DINGTALK_SKILL_DIRS,
            crate::platform::connector_state::dingtalk_skills_visible,
        )
    }

    /// 腾讯会议 mono skill 门控:`show` → 解包 `tmeet-skill` 到包目录;否则删除。
    /// 出处声明用 `NOTICE-tmeet.md`,避免覆盖其他 CLI 连接器 NOTICE。
    pub fn apply_tmeet_skills(&self, show: bool) -> std::io::Result<()> {
        Self::apply_connector_skills(
            "tmeet",
            &TMEET_SKILLS_DIR,
            &TMEET_SKILL_DIRS,
            "NOTICE-tmeet.md",
            show,
        )
    }
    /// 同 [`cached_feishu_skills_visible`]，以完整的腾讯会议技能目录作为启动缓存。
    pub(super) fn cached_tmeet_skills_visible(&self) -> bool {
        self.cached_connector_skills_visible(
            "tmeet",
            &TMEET_SKILL_DIRS,
            crate::platform::connector_state::tmeet_skills_visible,
        )
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
    /// 每次启动都调用（不受 VERSION gate 限制）。启动维护链只会在 mcp.json 可解析
    /// （或缺失）时调用本函数——损坏文件由 reconcile 备份并整轮保持原样，repair
    /// loader 的空骨架重建因此只服务于直接调用方（测试、防御兜底），不再承担
    /// 生产链路上的坏文件自愈；那个职责已让位给数据保全（备份 + 跳过重置）。
    pub(super) fn ensure_builtin_mcp_servers(&self) -> std::io::Result<()> {
        // mcp.json 只读 + parse 一次,upsert 与 python command 自愈共享(两段语义
        // 不同:前者修内置 server 条目,后者修 marketplace 条目的陈旧 python 路径;
        // 合并的只是 IO,不是逻辑)。本函数的 repair loader 在坏 json 上仍会重建
        // 空骨架,但唯一的生产调用方已由 `mcp_json_unparseable()` 先行门控(见
        // run_mcp_startup_maintenance),坏文件根本走不到这里——骨架重建只剩
        // 测试调用方,门控注释见本函数上方的调用点。
        let mut mcp = self.load_mcp_json_for_repair();
        let present_server = paths::bundle_present_artifact_server();
        // Both paths of load_mcp_json_for_repair return an object skeleton;
        // reaching either arm below means the user hand-edited mcp.json into
        // a valid-but-unexpected shape, so include the file path in the error.
        let Some(mcp_object) = mcp.as_object_mut() else {
            return Err(std::io::Error::other(format!(
                "mcp.json top level is not an object: {}",
                self.mcp_json.display()
            )));
        };
        if mcp_object
            .get("servers")
            .and_then(|s| s.as_object())
            .is_none()
        {
            mcp_object.insert("servers".into(), serde_json::json!({}));
        }
        let Some(servers) = mcp["servers"].as_object_mut() else {
            return Err(std::io::Error::other(format!(
                "mcp.json servers value is not an object: {}",
                self.mcp_json.display()
            )));
        };
        // 迁移:旧版 server key 是 `pinvou`(与产品名 `pinvou3` 差一个 3,模型采样必漂成
        // pinvou3 → `Failed to find MCP server: pinvou3`)。改用 `pinvou3` 对齐产品名,并删掉
        // 旧 `pinvou` 条目——upsert 不会自动删旧名,不删会留两个指向同一脚本的 server。
        servers.remove("pinvou");
        // Windows 用内置 pythonw(无窗口 + 自带依赖);其他平台系统 python3。见 paths::python_command。
        let python_cmd = paths::python_command();
        servers.insert(
            "pinvou3".to_string(),
            serde_json::json!({
                "command": python_cmd.clone(),
                "args": [present_server.to_string_lossy()]
            }),
        );
        // Browser MCP lets Work-mode Agents operate the in-app native WebView and is
        // deliberately absent from global mcp.json. Only Work-mode sessions expose it.
        // Remove only historical entries owned by this app (commands targeting
        // browser-wrapper.mjs). Preserve user-defined servers with the same name, such
        // as playwright-mcp, because unconditional removal would silently destroy user
        // configuration on every startup. `work_mode_mcp_config_path` injects Browser
        // MCP through the session-specific `~/.pinvou3/browser/mcp.work.json` file.
        let remove_browser_residue = servers
            .get("browser")
            .map(is_browser_wrapper_residue)
            .unwrap_or(false);
        if remove_browser_residue {
            servers.remove("browser");
        }
        self.refresh_mcp_python_commands(&mut mcp, &python_cmd)?;
        // 写回前与现有文件比对:内容一致则跳过写盘(避免每次启动重写 mcp.json)。
        // 原子落盘复用 marketplace 的共享写方(write_json_pretty → write_atomic,
        // tmp+rename):裸写被崩溃打断会制造出启动维护防御的损坏文件本身。
        let json = serde_json::to_string_pretty(&mcp).map_err(std::io::Error::other)?;
        if std::fs::read_to_string(&self.mcp_json).is_ok_and(|existing| existing == json) {
            return Ok(());
        }
        crate::features::marketplace::write_json_pretty(&self.mcp_json, &mcp)
            .map_err(std::io::Error::other)
    }

    /// 读 + parse mcp.json 供启动自愈路径复用:文件缺失给空骨架;坏 json 同样重建
    /// 空骨架(`{"servers":{}}`)。生产启动维护链不会在文件损坏时调用本函数
    /// (`run_mcp_startup_maintenance` 以 `mcp_json_unparseable` 把整个 upsert 挡在
    /// 备份过的坏文件之外),这条重建路径只覆盖直接调用方与防御兜底;数据保全
    /// (备份 + 整轮保持原样)优先于坏文件自动重置。
    fn load_mcp_json_for_repair(&self) -> serde_json::Value {
        if !self.mcp_json.is_file() {
            return serde_json::json!({"servers": {}});
        }
        let existing = std::fs::read_to_string(&self.mcp_json).unwrap_or_default();
        serde_json::from_str(&existing).unwrap_or_else(|_| serde_json::json!({"servers": {}}))
    }

    /// Builds a unified `browser` MCP server entry on Windows, Linux, and macOS. The entry
    /// is not persisted here; [`Self::work_mode_mcp_config_path`] injects it into the
    /// Work-mode session-specific mcp.json. macOS and Linux use the app-owned BrowserCore,
    /// ignore `PINVOU3_CDMCP_BIN`, and never let an external Chrome instance impersonate an
    /// embedded page. Windows requires:
    /// 1. a vendored chrome-devtools-mcp entry point (packaged or overridden by
    ///    `PINVOU3_CDMCP_BIN`);
    /// 2. a Node.js runtime (prefer bundled Node.js, then fall back to PATH);
    /// 3. an extracted wrapper under `~/.pinvou3/bundle/mcp-servers/`.
    /// Returns `None` when any prerequisite is missing, so the Work-mode session falls back
    /// to the global configuration and the model does not receive browser tools.
    pub fn browser_mcp_entry(&self) -> Option<serde_json::Value> {
        self.browser_mcp_entry_for_session(None)
    }

    /// Unreleased platforms have no Agent automation backend. Environment variables and
    /// stale local files must not bypass the capability gate and register Browser MCP.
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    fn browser_mcp_entry_for_session(
        &self,
        _session_id: Option<&str>,
    ) -> Option<serde_json::Value> {
        None
    }

    /// Linux and macOS use the same agent-facing wrapper and host-request
    /// protocol as Windows, but BrowserCore executes directly against the
    /// task-owned system WebView. Neither platform packages nor starts Chrome
    /// MCP; only Linux additionally needs WebKitWebDriver for trusted input.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn browser_mcp_entry_for_session(&self, session_id: Option<&str>) -> Option<serde_json::Value> {
        if !crate::platform::capabilities::browser_product_enabled() {
            return None;
        }
        #[cfg(target_os = "linux")]
        find_webkit_webdriver()?;
        let node = crate::platform::os::bundled_node().or_else(find_system_node)?;
        let wrapper = paths::bundle_browser_wrapper();
        if !wrapper.is_file() {
            return None;
        }
        let cdp_port_json = paths::browser_cdp_port_json();
        let mut env = serde_json::json!({ "CI": "1" });
        if let Some(session_id) = session_id {
            env["PINVOU3_BROWSER_SESSION_ID"] = serde_json::json!(session_id);
            env["PINVOU3_BROWSER_SESSION_TOKEN"] =
                serde_json::json!(paths::browser_session_token(session_id));
        }
        Some(serde_json::json!({
            "command": node,
            "args": [
                wrapper.to_string_lossy(),
                "@pinvou/browser-core",
                cdp_port_json.to_string_lossy(),
            ],
            "env": env,
        }))
    }

    /// Builds a Browser MCP entry bound to one Work-mode session. The Windows wrapper uses
    /// the injected session identity to request the corresponding WebView2 page, then pins
    /// that page after connecting to chrome-devtools-mcp. Sessions still share one WebView2
    /// profile, including cookies and sign-in state.
    #[cfg(target_os = "windows")]
    fn browser_mcp_entry_for_session(&self, session_id: Option<&str>) -> Option<serde_json::Value> {
        if !crate::platform::capabilities::browser_product_enabled() {
            return None;
        }
        let mcp_bin = std::env::var_os("PINVOU3_CDMCP_BIN")
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_file())
            .or_else(paths::bundled_chrome_devtools_mcp_bin)?;
        let node = crate::platform::os::bundled_node().or_else(find_system_node)?;
        // Tauri's Windows resource_dir can return a `\\?\C:\...` verbatim path. Node may
        // parse only the drive component when this form is supplied as an entry-script
        // argument and exit with EISDIR. Normalize all executable and script paths passed
        // to external processes into the platform-compatible form.
        let mcp_bin = crate::platform::os::platform_compat_path(&mcp_bin.to_string_lossy());
        let node = crate::platform::os::platform_compat_path(&node.to_string_lossy());
        let wrapper = paths::bundle_browser_wrapper();
        if !wrapper.is_file() {
            return None;
        }
        let wrapper = crate::platform::os::platform_compat_path(&wrapper.to_string_lossy());
        let cdp_port_json = crate::platform::os::platform_compat_path(
            &paths::browser_cdp_port_json().to_string_lossy(),
        );
        let mut env = serde_json::json!({
            // Defense in depth for offline operation; the wrapper also sets these values.
            "CHROME_DEVTOOLS_MCP_NO_UPDATE_CHECKS": "1",
            "CI": "1"
        });
        if let Some(session_id) = session_id {
            env["PINVOU3_BROWSER_SESSION_ID"] = serde_json::json!(session_id);
            env["PINVOU3_BROWSER_SESSION_TOKEN"] =
                serde_json::json!(paths::browser_session_token(session_id));
        }
        Some(serde_json::json!({
            "command": node,
            "args": [
                wrapper.to_string_lossy(),
                mcp_bin.to_string_lossy(),
                cdp_port_json.to_string_lossy(),
            ],
            "env": env
        }))
    }

    /// Returns the static model-visible reason that browser capabilities are unavailable.
    /// `None` means all prerequisites are present and no explanation should be injected.
    /// This checks only the vendored chrome-devtools-mcp, Node.js, and wrapper files, plus a
    /// fresh (within 24 hours) allowlisted dynamic failure code recorded by the wrapper in
    /// `last-error.json`. Dynamic failures such as an unavailable native host or CDP occur
    /// after session creation, so the current session relies on the general recovery advice
    /// in the Browser capabilities instructions. A reopened session can read the recorded
    /// failure and guide the user precisely.
    pub fn browser_unavailability_reason(&self) -> Option<String> {
        if !crate::platform::capabilities::browser_product_enabled() {
            return Some(
                "Browser tools (`mcp_browser_*`) are not enabled in this product build; do not fall back to an external browser or screenshot stream."
                    .to_string(),
            );
        }
        let mut missing: Vec<&str> = Vec::new();
        #[cfg(target_os = "windows")]
        if std::env::var_os("PINVOU3_CDMCP_BIN")
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_file())
            .is_none()
            && paths::bundled_chrome_devtools_mcp_bin().is_none()
        {
            missing.push(
                "The bundled chrome-devtools-mcp runtime is not ready (packaged builds include it; development requires running the vendor build first)",
            );
        }
        #[cfg(target_os = "linux")]
        if find_webkit_webdriver().is_none() {
            missing.push("WebKitWebDriver is unavailable (install webkit2gtk-driver)");
        }
        if crate::platform::os::bundled_node().is_none() && find_system_node().is_none() {
            missing.push("The Node.js runtime is unavailable");
        }
        if !paths::bundle_browser_wrapper().is_file() {
            missing.push("The browser wrapper script was not extracted");
        }
        // The embedded browser uses only the system WebView owned by this application.
        // No platform may silently fall back to external Chrome; unreleased platforms are
        // explicitly unavailable.
        #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
        missing.push("The in-app native browser automation backend is not ready on this platform");
        if !missing.is_empty() {
            return Some(format!(
                "Browser tools (`mcp_browser_*`) are currently unavailable: {}. When browser access is needed, explain the reason above; do not fall back to an external browser or screenshot stream.",
                missing.join("; ")
            ));
        }
        // Report the most recent dynamic startup failure (for example, native-host or CDP
        // readiness) only while it remains fresh for 24 hours.
        let Ok(raw) = std::fs::read_to_string(paths::browser_last_error_json()) else {
            return None;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let (code, hint) = browser_last_error_hint(&raw, now)?;
        Some(format!(
            "Browser tools (`mcp_browser_*`) failed during the previous startup ({code}): {hint} If browser access is needed, ask the user to open a new conversation and retry."
        ))
    }

    /// Returns the MCP configuration path for a Work-mode assistant Engine session. The
    /// global mcp.json plus Browser MCP (when available) is written atomically to
    /// `~/.pinvou3/browser/mcp.work.json`. When Browser MCP is unavailable but a user owns
    /// the reserved `browser` name, a session copy is still generated and the user server
    /// is renamed to `browser_user[_N]` so it cannot impersonate the embedded browser. The
    /// global configuration is unchanged. Concurrent Work-mode sessions rebuild the same
    /// deterministic file using an idempotent temporary-file-and-rename sequence.
    pub fn work_mode_mcp_config_path(&self) -> PathBuf {
        self.write_work_mode_mcp_config(None)
    }

    /// Returns a conversation-scoped Work-mode MCP configuration. It matches the global
    /// Work-mode configuration, but the browser wrapper carries this conversation's
    /// identity. Each Engine therefore has an isolated tool-routing context that another
    /// conversation cannot change by switching pages.
    pub fn work_mode_mcp_config_path_for_session(&self, session_id: &str) -> PathBuf {
        self.write_work_mode_mcp_config(Some(session_id))
    }

    fn write_work_mode_mcp_config(&self, session_id: Option<&str>) -> PathBuf {
        let base = self.mcp_json.clone();
        let browser_entry = self.browser_mcp_entry_for_session(session_id);
        // Fall back to the global configuration when parsing fails (for example, a manual
        // edit or a concurrent partial read) or when valid JSON is not an object (such as
        // `[]`). Fabricating an empty object would silently remove all marketplace tools
        // from this session and leave only Browser MCP; continuing with a non-object would
        // panic at `as_object_mut().unwrap()`. The fallback loses only browser tools and
        // preserves the global behavior.
        let mcp: serde_json::Value = match std::fs::read_to_string(&base)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        {
            Some(v) if v.is_object() => v,
            _ => return base,
        };
        let has_reserved_browser = mcp
            .get("servers")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|servers| servers.contains_key("browser"));
        if browser_entry.is_none() && !has_reserved_browser {
            return base;
        }
        let work_path = session_id.map_or_else(paths::browser_work_mcp_json, |session_id| {
            paths::browser_session_mcp_json(session_id)
        });
        let mut obj = mcp;
        if obj.get("servers").and_then(|s| s.as_object()).is_none() {
            // mcp was validated as a JSON object in the match above, so
            // as_object_mut always yields Some here.
            #[allow(clippy::unwrap_used)]
            obj.as_object_mut()
                .unwrap()
                .insert("servers".into(), serde_json::json!({}));
        }
        // `browser` is reserved for Pinvou's embedded browser in Work-mode sessions. A
        // user-defined server with that name (for example, playwright-mcp) is renamed only
        // in the session copy to browser_user, browser_user_2, and so on; global mcp.json is
        // untouched. Agent calls to `mcp_browser_*` therefore always route to Pinvou
        // Browser MCP, while user tools remain available as `mcp_browser_user[_N]_*`.
        // Historical wrapper residue owned by this app is removed. Preserve this namespace
        // boundary even when the built-in backend is unavailable, so a user server cannot
        // impersonate the embedded same-page browser through `mcp_browser_*`.
        // The block above guarantees `servers` exists and is an object, so
        // both indexing and as_object_mut always succeed here.
        #[allow(clippy::unwrap_used)]
        let servers = obj["servers"].as_object_mut().unwrap();
        if let Some(browser_entry) = browser_entry {
            install_work_mode_browser_server(servers, browser_entry);
        } else {
            reserve_work_mode_browser_server_name(servers);
        }
        if let Some(parent) = work_path.parent() {
            let _ = std::fs::create_dir_all(parent);
            // Work-mode session creation precedes native browser-host startup, which also
            // tightens this directory to 0700. Apply the restriction immediately so
            // browser/ does not retain default umask permissions. Files are already 0600;
            // this protects directory listings consistently across the host lifecycle.
            crate::platform::os::make_private_dir(parent);
        }
        match serde_json::to_string_pretty(&obj) {
            Ok(json) => {
                // Skip an unchanged file. Every spawned session reaches this path, including
                // Code-mode sessions that later fall back to the global configuration, so
                // atomically replacing identical content would be needless disk I/O.
                if std::fs::read_to_string(&work_path)
                    .map(|existing| existing == json)
                    .unwrap_or(false)
                {
                    return work_path;
                }
                // The content includes a full global mcp.json copy and may contain secrets
                // in user-defined server environments. Create it as 0600 immediately and
                // permit atomic replacement of an existing configuration on Windows.
                if crate::platform::filesystem::atomic_write_private(&work_path, json.as_bytes())
                    .is_ok()
                {
                    return work_path;
                }
                // On write failure, return the global configuration. Returning the
                // session-specific path could make the Engine load stale or missing data;
                // the global fallback degrades by omitting browser tools instead of failing.
                base
            }
            // Serialization of a Value should not fail, but use the same global fallback
            // as a write failure if it does.
            Err(_) => base,
        }
    }

    /// 启动自愈:`mcp.json` 里本地 python server 的 `command` 是**安装时写死**的,老条目
    /// 常是裸 `"python"`/`"python3"` —— 在没把 python 加进 PATH 的机器(或只有 python3 的
    /// Linux)上永远拉不起来(高德天气等 marketplace 工具静默失效)。每次启动重解析:凡
    /// command 是裸 python 家族名、或指向不存在的 python 路径,统一替换成当前
    /// `paths::python_command()`。`url` 型远程 server / 非 python command 一律不动。
    /// 直接改传入的 `mcp`(调用方负责落盘),不再独立读写文件。
    fn refresh_mcp_python_commands(
        &self,
        mcp: &mut serde_json::Value,
        resolved: &str,
    ) -> std::io::Result<()> {
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
                        serde_json::Value::String(resolved.to_string()),
                    );
                }
            }
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

    /// 写出内置 MCP server 资源。present_artifact（pinvou 内置，非市场包）布局不变；
    /// 市场 MCP 包按 BundleStore 已装记录校验/补齐（§4：启动不再全量释放，
    /// 未安装包不占盘），随后做存量 mcp.json 路径迁移与旧布局工具目录清理。
    /// 每次启动跑（immutable 资源，内容一致时零写盘）。
    /// 首启导入旧布局安装态 → BundleStore（bundles.json，Phase 2 真相源）。
    /// 失败不阻塞启动（fail loud 到日志）；报告主要内容落启动标记与日志
    /// （迁移决策可观测，§10.5）。幂等由 `legacy_imported` 闸保证。
    fn import_legacy_bundle_store() {
        crate::platform::startup::mark("bundle_extract:bundle_store_import:start");
        match crate::features::marketplace::store::BundleStore::new().import_legacy() {
            Ok(report) => {
                crate::platform::startup::mark_with_detail(
                    "rust",
                    "bundle_extract:bundle_store_import:done",
                    &format!(
                        "already={} imported={} kept={} degraded={}",
                        report.already_imported,
                        report.imported.len(),
                        report.kept_existing.len(),
                        report.degraded.len()
                    ),
                );
                if !report.already_imported && !report.imported.is_empty() {
                    log::info!(
                        "[runtime-bundle] bundles.json 首启导入完成: imported={:?} degraded={:?}",
                        report.imported,
                        report.degraded
                    );
                }
            }
            Err(e) => {
                log::warn!("[runtime-bundle] bundles.json 首启导入失败（不阻塞启动）: {e}")
            }
        }
    }
    fn write_mcp_servers(&self, can_cleanup_legacy_manifests: bool) -> std::io::Result<()> {
        let dir = paths::bundle_mcp_servers_dir();
        // pinvou 内置 present_artifact server
        let server = paths::bundle_present_artifact_server();
        let server_written = self.write_if_changed(&server, PRESENT_ARTIFACT_SERVER_PY)?;
        self.write_if_changed(
            &paths::bundle_mcp_python_runner(),
            MCP_PYTHON_DEPENDENCY_RUNNER_PY,
        )?;
        if server_written {
            // 可执行位只在本次实际写出时补;内容未变时也不丢——上次写出后已设过。
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perm = std::fs::metadata(&server)?.permissions();
                perm.set_mode(0o755);
                std::fs::set_permissions(&server, perm)?;
            }
        }

        // 市场包：按已装记录校验/补齐包目录资源（内嵌目录为比对基准）。
        // store 读失败不阻塞启动（fail loud 到日志，下个启动周期自愈）。
        match crate::features::marketplace::store::BundleStore::new().records() {
            Ok(records) => {
                for record in records.iter().filter(|r| r.installed) {
                    if crate::features::marketplace::mcp_catalog::spec_for(&record.id).is_none() {
                        continue; // 非内嵌包（自定义/上传），无内嵌资源可校验
                    }
                    // 上传/未知来源的记录即使 id 撞内嵌目录也不得重释放：重释放
                    // 以嵌入资源为基准覆盖包目录（release_package 先删后建），对
                    // 用户内容即数据销毁。镜像卸载路径 source_may_be_upload 的
                    // fail-closed 口径（六轮评审 R1），只放行预置/内置来源。
                    if !should_ensure_embedded_release(record) {
                        log::warn!(
                            "[runtime-bundle] 跳过非预置来源记录的内嵌重释放（{}，source={:?}）",
                            record.id,
                            record.source
                        );
                        continue;
                    }
                    if let Err(e) =
                        crate::features::marketplace::mcp_catalog::ensure_package_released(
                            &record.id,
                        )
                    {
                        log::warn!("[runtime-bundle] MCP 包资源补齐失败（{}）: {e}", record.id);
                    }
                }
            }
            Err(e) => {
                log::warn!("[runtime-bundle] BundleStore 读取失败，跳过 MCP 包资源校验: {e}")
            }
        }
        // 自定义 MCP 布局迁移（bundle/mcp-servers/<id>/ → bundles/<id>/mcp/）已提前到
        // import_legacy 之后、技能迁移之前（ensure_extracted 内，M-7 排序），此处不再
        // 重复；旧布局随后只保留 present_artifact_server.py。
        // 存量 mcp.json 条目路径迁移：旧布局前缀 → 新包目录（幂等，只改本 app 写的文件）
        if can_cleanup_legacy_manifests {
            if let Err(e) = crate::features::marketplace::migrate_mcp_json_paths() {
                log::warn!("[runtime-bundle] mcp.json path migration failed: {e}");
            }
            // Only remove embedded legacy directories after plaintext credential migration has
            // succeeded; otherwise an old manifest may still be the sole recoverable copy.
            for spec in crate::features::marketplace::mcp_catalog::MCP_PACKAGES {
                let _ = std::fs::remove_dir_all(dir.join(spec.id));
            }
        }
        // The zero-dependency Browser MCP wrapper speaks MCP over stdin/stdout. Node runs
        // it directly, so it does not require an executable bit. Avoid rewriting unchanged
        // content, and add the Unix executable bit only after an actual write.
        let wrapper = paths::bundle_browser_wrapper();
        let wrapper_written = self.write_if_changed(&wrapper, BROWSER_WRAPPER_MJS)?;
        #[cfg(not(unix))]
        let _ = wrapper_written;
        self.write_if_changed(
            &dir.join("browser-wrapper-protocol.mjs"),
            BROWSER_WRAPPER_PROTOCOL_MJS,
        )?;
        self.write_if_changed(
            &dir.join("browser-core-protocol.mjs"),
            BROWSER_CORE_PROTOCOL_MJS,
        )?;
        #[cfg(unix)]
        if wrapper_written {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = std::fs::metadata(&wrapper)?.permissions();
            perm.set_mode(0o755);
            std::fs::set_permissions(&wrapper, perm)?;
        }
        Ok(())
    }

    /// 内容比对写:目标已存在且逐字节一致时跳过写盘,返回是否实际写入。
    /// 调用方据此决定是否还要 chmod / 后续动作——避免每次启动无条件重写
    /// 上百 KB 的 immutable bundle 资源。
    pub(super) fn write_if_changed(
        &self,
        path: &std::path::Path,
        contents: &str,
    ) -> std::io::Result<bool> {
        if std::fs::read(path).is_ok_and(|existing| existing == contents.as_bytes()) {
            return Ok(false);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, contents)?;
        Ok(true)
    }
}

/// 内嵌重释放放行判定（G1）：仅预置/内置来源。上传/未知来源即使 id 撞内嵌
/// 目录也不得重释放——重释放以嵌入资源为基准覆盖包目录（`release_package`
/// 先删后建），对用户内容即数据销毁。镜像卸载路径 `source_may_be_upload`
/// 的 fail-closed 口径（六轮评审 R1）。
fn should_ensure_embedded_release(
    record: &crate::features::marketplace::store::BundleRecord,
) -> bool {
    matches!(
        record.source,
        crate::features::marketplace::store::BundleSource::Preset
            | crate::features::marketplace::store::BundleSource::Builtin
    )
}

#[cfg(test)]
mod tests {
    /// G1 回归：上传来源记录不得进入内嵌重释放（防用户内容被嵌入资源覆盖）。
    #[test]
    fn embedded_release_skips_non_preset_sources() {
        use crate::features::marketplace::store::{BundleRecord, BundleSource};
        let preset = BundleRecord::installed_now("weather".to_string(), BundleSource::Preset);
        let builtin = BundleRecord::installed_now("weather".to_string(), BundleSource::Builtin);
        let upload = BundleRecord::installed_now(
            "weather".to_string(),
            BundleSource::Upload("x.zip".to_string()),
        );
        assert!(super::should_ensure_embedded_release(&preset));
        assert!(super::should_ensure_embedded_release(&builtin));
        assert!(!super::should_ensure_embedded_release(&upload));
    }

    /// G1 loop wiring: `write_mcp_servers` must actually consult
    /// `should_ensure_embedded_release` — an installed Upload record whose id
    /// collides with an embedded spec keeps its on-disk package untouched.
    /// (The predicate-only test above stays green even if the loop's guard
    /// `continue` is removed; this one fails.)
    #[test]
    fn write_mcp_servers_never_releases_over_upload_records() {
        use crate::features::marketplace::store::{BundleRecord, BundleSource};
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-g1-wiring-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        let bundle = super::Pinvou3Bundle::paths();

        // Installed Upload record colliding with the embedded spec id "weather".
        crate::features::marketplace::store::BundleStore::new()
            .upsert(BundleRecord::installed_now(
                "weather".to_string(),
                BundleSource::Upload("weather.zip".to_string()),
            ))
            .unwrap();
        // User content in the package dir — must survive write_mcp_servers.
        let pkg_mcp = crate::platform::paths::bundles_root()
            .join("weather")
            .join("mcp");
        std::fs::create_dir_all(&pkg_mcp).unwrap();
        std::fs::write(pkg_mcp.join("server.py"), "USER CODE").unwrap();
        std::fs::write(pkg_mcp.join("manifest.json"), "{}").unwrap();

        bundle.write_mcp_servers(false).unwrap();

        assert_eq!(
            std::fs::read_to_string(pkg_mcp.join("server.py")).unwrap(),
            "USER CODE",
            "上传包目录不得被内嵌重释放覆盖"
        );

        // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe { std::env::remove_var("PINVOU3_HOME") };
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Startup-maintenance wiring: the reconcile must actually run, and must run
    /// BEFORE the Python repair (the managed-runtime patch errors while an entry
    /// is missing entirely, so repair-before-reconcile never converges in one
    /// startup). The injected repair closure probes mcp.json at its invocation
    /// time: the entry seeded as missing must already exist by then, and the
    /// final file must contain the restored entry. Deleting the reconcile call
    /// from `run_mcp_startup_maintenance`, or moving it after the repair, turns
    /// this test red.
    #[test]
    fn startup_maintenance_restores_missing_entry_before_python_repair() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // Restores the host's PINVOU3_HOME (present or absent) on drop, panic paths included.
        let _env = crate::platform::paths::tests::EnvVarGuard::capture(&["PINVOU3_HOME"]);
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-mcp-maintenance-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        // Seed one installed local tool (disk manifest, absolute existing command)
        // whose mcp.json entry is missing entirely.
        let marketplace_dir = crate::platform::paths::pinvou3_home().join("marketplace");
        std::fs::create_dir_all(&marketplace_dir).unwrap();
        std::fs::write(marketplace_dir.join("installed.json"), r#"["maint-x"]"#).unwrap();
        let manifest = serde_json::json!({
            "id":"maint-x","name":"MaintX","description":"d","version":"1","icon":"x","category":"c",
            "mcp_tools":[],"command":std::env::current_exe().unwrap().to_string_lossy(),"args":[]
        });
        std::fs::create_dir_all(crate::features::marketplace::mcp_catalog::package_mcp_dir(
            "maint-x",
        ))
        .unwrap();
        std::fs::write(
            crate::features::marketplace::mcp_catalog::package_mcp_dir("maint-x")
                .join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let manager = crate::features::marketplace::MarketplaceManager::with_store(
            crate::platform::credential_store::MemoryCredentialStore::default(),
        );
        let bundle = super::Pinvou3Bundle::paths();
        assert!(!bundle.mcp_json.exists(), "precondition: no mcp.json yet");

        let entry_seen_at_repair = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let repair_probe = entry_seen_at_repair.clone();
        let mcp_json = bundle.mcp_json.clone();
        let actions = bundle
            .run_mcp_startup_maintenance(&manager, move |_manager| {
                repair_probe.store(
                    std::fs::read_to_string(&mcp_json)
                        .map(|content| content.contains("maint-x"))
                        .unwrap_or(false),
                    std::sync::atomic::Ordering::SeqCst,
                );
                Ok(Vec::new())
            })
            .unwrap();

        assert!(
            actions
                .iter()
                .any(|action| action.contains("restored missing mcp.json entry")),
            "the reconcile call inside startup maintenance must restore the entry: {actions:?}"
        );
        assert!(
            entry_seen_at_repair.load(std::sync::atomic::Ordering::SeqCst),
            "the reconcile must run before the python repair sees mcp.json"
        );
        let mcp: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&bundle.mcp_json).unwrap()).unwrap();
        assert_eq!(
            mcp["servers"]["maint-x"]["command"],
            serde_json::Value::String(
                std::env::current_exe()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            ),
            "the missing entry must be restored by the end of startup maintenance"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The corrupt-file contract must hold across the WHOLE startup maintenance
    /// chain, not just the reconcile step: a corrupt mcp.json is backed up, the
    /// reconcile returns early, and the builtin upsert must NOT reset the live
    /// file to a builtin-only skeleton (its repair loader would, unchecked).
    /// The original bytes survive every writer, the backup stays a single
    /// byte-identical copy across repeated boots, and a healthy file still gets
    /// the upsert.
    #[test]
    fn startup_maintenance_preserves_corrupt_mcp_json_bytes() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env = crate::platform::paths::tests::EnvVarGuard::capture(&["PINVOU3_HOME"]);
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-mcp-corrupt-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let manager = crate::features::marketplace::MarketplaceManager::with_store(
            crate::platform::credential_store::MemoryCredentialStore::default(),
        );
        let bundle = super::Pinvou3Bundle::paths();
        std::fs::create_dir_all(bundle.mcp_json.parent().unwrap()).unwrap();
        // Corrupt by truncation: two hand-added custom entries, no closing brace.
        let corrupt = r#"{
  "servers": {
    "my-custom-tool": {"command": "/usr/local/bin/my-tool", "args": ["--serve"]},
    "another-custom": {"command": "echo", "args": ["hi"]}"#;
        std::fs::write(&bundle.mcp_json, corrupt).unwrap();

        let run = || bundle.run_mcp_startup_maintenance(&manager, |_manager| Ok(Vec::new()));
        let actions = run().unwrap();

        assert!(
            actions
                .iter()
                .any(|action| action.contains("unparseable") && action.contains("backed up")),
            "the reconcile must report the skipped boot: {actions:?}"
        );
        assert_eq!(
            std::fs::read(&bundle.mcp_json).unwrap(),
            corrupt.as_bytes(),
            "the corrupt file must stay byte-identical through the whole maintenance chain"
        );
        let backups: Vec<_> = std::fs::read_dir(bundle.mcp_json.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("mcp.json.corrupt.")
            })
            .collect();
        assert_eq!(backups.len(), 1, "exactly one backup must exist");
        assert_eq!(
            std::fs::read(backups[0].path()).unwrap(),
            corrupt.as_bytes(),
            "the backup must hold the original bytes"
        );

        // A persistent parse failure repeats the boot: no second backup, no rewrite.
        let actions = run().unwrap();
        assert!(actions.iter().any(|action| action.contains("unparseable")));
        assert_eq!(std::fs::read(&bundle.mcp_json).unwrap(), corrupt.as_bytes());
        let backups: Vec<_> = std::fs::read_dir(bundle.mcp_json.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("mcp.json.corrupt.")
            })
            .collect();
        assert_eq!(
            backups.len(),
            1,
            "an identical corrupt file must not mint a second backup"
        );

        // Once the user repairs the file, the builtin upsert works again.
        std::fs::write(
            &bundle.mcp_json,
            r#"{"servers":{"my-custom-tool":{"command":"node","args":["/opt/t/run.js"]}}}"#,
        )
        .unwrap();
        run().unwrap();
        let mcp: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&bundle.mcp_json).unwrap()).unwrap();
        assert!(
            mcp["servers"].get("pinvou3").is_some(),
            "a repaired file gets the builtin upsert again: {mcp}"
        );
        assert!(
            mcp["servers"].get("my-custom-tool").is_some(),
            "the upsert must keep the user's entries"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The preservation contract must hold through the REAL boot chain, not
    /// just `run_mcp_startup_maintenance`: `ensure_extracted` first runs the
    /// retired-tool cleanup, whose residue probe treats a corrupt mcp.json as
    /// "residue present" and calls `uninstall` — and an uninstall that reset
    /// the file would destroy the original bytes *before* the reconcile ever
    /// got to back them up. With the refusal in `remove_from_mcp_json`, the
    /// uninstall rolls back, the corrupt file (and its single backup) survive
    /// the whole chain, and the engine builtin keys stay untouched.
    #[test]
    fn ensure_extracted_preserves_corrupt_mcp_json_bytes() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env = crate::platform::paths::tests::EnvVarGuard::capture(&["PINVOU3_HOME"]);
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-boot-corrupt-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        // A retired preset tool still present in the registry: the cleanup's
        // uninstall path runs against the corrupt file (probe returns true).
        let marketplace_dir = crate::platform::paths::pinvou3_home().join("marketplace");
        std::fs::create_dir_all(&marketplace_dir).unwrap();
        std::fs::write(
            marketplace_dir.join("installed.json"),
            r#"["data_analysis"]"#,
        )
        .unwrap();

        let bundle = super::Pinvou3Bundle::paths();
        std::fs::create_dir_all(bundle.mcp_json.parent().unwrap()).unwrap();
        let corrupt = r#"{
  "servers": {
    "my-custom-tool": {"command": "/usr/local/bin/my-tool", "args": ["--serve"], "enabled": false}
  }"#;
        std::fs::write(&bundle.mcp_json, corrupt).unwrap();
        // Seed the bundle VERSION so the boot skips re-extraction and returns
        // right after the maintenance block (the code path every normal boot
        // with an unchanged bundle takes).
        std::fs::write(super::paths::bundle_version_file(), super::BUNDLE_VERSION).unwrap();

        let manager = crate::features::marketplace::MarketplaceManager::with_store(
            crate::platform::credential_store::MemoryCredentialStore::default(),
        );
        bundle
            .ensure_extracted_with_marketplace(&manager, |_manager| Ok(Vec::new()))
            .unwrap();

        assert_eq!(
            std::fs::read(&bundle.mcp_json).unwrap(),
            corrupt.as_bytes(),
            "the corrupt file must stay byte-identical through the real boot chain"
        );
        let backups: Vec<_> = std::fs::read_dir(bundle.mcp_json.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("mcp.json.corrupt.")
            })
            .collect();
        assert_eq!(backups.len(), 1, "exactly one backup must exist");
        assert_eq!(
            std::fs::read(backups[0].path()).unwrap(),
            corrupt.as_bytes(),
            "the backup must hold the original bytes"
        );
        let installed: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(marketplace_dir.join("installed.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            installed,
            serde_json::json!(["data_analysis"]),
            "the rolled-back uninstall must leave the registry unchanged"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// #522 回归:禁用/隐藏残留可能只存在于 scope 收敛后的单一 disabled_bundles.json
    /// (旧 disabled_connectors.json 只读保留、新写入不再产生)。探测漏掉该文件会把
    /// 「仅禁用/隐藏集有残留」的退役工具误判干净,启动清理被整体跳过,陈旧条目继续
    /// 误隐藏未来同名重装。
    #[test]
    fn residue_probe_covers_disabled_bundles_layout() {
        crate::platform::test_support::with_temp_home("pinvou3-residue-probe", || {
            // 前置:所有清理面干净时探测不得误报。
            assert!(!super::Pinvou3Bundle::marketplace_tool_residue_present(
                "data_analysis"
            ));
            std::fs::write(
                crate::platform::paths::pinvou3_home().join("disabled_bundles.json"),
                r#"{"scopes":{"plain":["data_analysis"]},"hidden_scopes":{"code":["data_analysis"]}}"#,
            )
            .unwrap();
            assert!(
                super::Pinvou3Bundle::marketplace_tool_residue_present("data_analysis"),
                "残留仅在 disabled_bundles.json 时探测必须报有残留"
            );
        });
    }

    /// 旧布局 disabled_connectors.json 不探测:其内容在首个 scope 读路径「读到即
    /// 迁移」并进统一 disabled_bundles.json,此后只剩死数据(uninstall 与 scope
    /// 写方都不再碰它)——探测它只会对已迁移用户产生永不收敛的误报(评审轮 #580:
    /// 每次启动白跑一遍完整 uninstall)。活残留由统一文件面覆盖。
    #[test]
    fn residue_probe_ignores_dead_legacy_disabled_file() {
        crate::platform::test_support::with_temp_home("pinvou3-residue-legacy", || {
            // 最老形态:裸数组 = plain scope(旧版真实用户落盘)。
            std::fs::write(
                crate::platform::paths::pinvou3_home().join("disabled_connectors.json"),
                r#"["data_analysis"]"#,
            )
            .unwrap();
            assert!(
                !super::Pinvou3Bundle::marketplace_tool_residue_present("data_analysis"),
                "legacy 死数据不得触发探测:其内容已由迁移并进统一文件"
            );
        });
    }

    /// 禁用落盘存在但读失败(权限/损坏)必须保守视为有残留——宁可误报(多跑一次
    /// 幂等清理)也不漏报。以同名目录制造 read_to_string 的读失败路径。
    #[test]
    fn residue_probe_treats_unreadable_disabled_file_as_residue() {
        crate::platform::test_support::with_temp_home("pinvou3-residue-unreadable", || {
            std::fs::create_dir_all(
                crate::platform::paths::pinvou3_home().join("disabled_bundles.json"),
            )
            .unwrap();
            assert!(
                super::Pinvou3Bundle::marketplace_tool_residue_present("data_analysis"),
                "禁用落盘存在但读失败时探测必须保守报有残留"
            );
        });
    }

    /// #522 端到端:退役工具的残留只落在 disabled_bundles.json 时,探测必须报有残留
    /// (否则清理被整体跳过、陈旧禁用条目永驻),且清理把 plain/code 的 disabled 与
    /// hidden 陈旧条目一并清掉,其它工具的条目与 scope 初始化登记原样保留。
    #[test]
    fn cleanup_scrubs_disabled_bundles_only_residue() {
        crate::platform::test_support::with_temp_home("pinvou3-residue-cleanup", || {
            std::fs::write(
                crate::platform::paths::pinvou3_home().join("disabled_bundles.json"),
                r#"{"scopes":{"plain":["data_analysis"],"code":["weather"]},"hidden_scopes":{"code":["data_analysis"]},"initialized":["plain","code"]}"#,
            )
            .unwrap();

            super::Pinvou3Bundle::paths()
                .cleanup_removed_marketplace_tools()
                .unwrap();

            let file: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(
                    crate::platform::paths::pinvou3_home().join("disabled_bundles.json"),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(
                file["scopes"]["plain"],
                serde_json::json!([]),
                "plain 禁用集里的退役条目必须被清掉"
            );
            assert_eq!(
                file["scopes"]["code"],
                serde_json::json!(["weather"]),
                "其它工具的禁用条目必须原样保留"
            );
            assert_eq!(
                file["hidden_scopes"]["code"],
                serde_json::json!([]),
                "code 隐藏集里的退役条目必须被清掉"
            );
            let initialized = file["initialized"].as_array().unwrap();
            assert!(
                initialized.contains(&serde_json::json!("plain"))
                    && initialized.contains(&serde_json::json!("code")),
                "清理不得改动 scope 初始化登记: {initialized:?}"
            );
        });
    }

    /// #522 失败路径:uninstall 因 mcp.json 拒重置而整体回滚(字节保全契约见
    /// ensure_extracted_preserves_corrupt_mcp_json_bytes)时,其内部 scope 清理随回滚
    /// 被跳过——禁用/隐藏残留必须仍被外层的单临界区 RMW 调用清掉。若把这步收敛进
    /// uninstall 成功路径,本用例转红。
    #[test]
    fn cleanup_scrubs_disabled_residue_when_uninstall_rolls_back() {
        crate::platform::test_support::with_temp_home("pinvou3-residue-rollback", || {
            let home = crate::platform::paths::pinvou3_home();
            let marketplace_dir = home.join("marketplace");
            std::fs::create_dir_all(&marketplace_dir).unwrap();
            std::fs::write(
                marketplace_dir.join("installed.json"),
                r#"["data_analysis"]"#,
            )
            .unwrap();
            let disabled = home.join("disabled_bundles.json");
            std::fs::write(
                &disabled,
                r#"{"scopes":{"plain":["data_analysis"]},"hidden_scopes":{"plain":["data_analysis"]}}"#,
            )
            .unwrap();
            let bundle = super::Pinvou3Bundle::paths();
            std::fs::create_dir_all(bundle.mcp_json.parent().unwrap()).unwrap();
            let corrupt = r#"{"servers":{"mine":{"command":"x""#;
            std::fs::write(&bundle.mcp_json, corrupt).unwrap();

            bundle.cleanup_removed_marketplace_tools().unwrap();

            // 卸载回滚:登记与 mcp.json 原样保留(数据保全契约不因清理改变)。
            let installed: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(marketplace_dir.join("installed.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(
                installed,
                serde_json::json!(["data_analysis"]),
                "回滚后登记必须原样保留"
            );
            assert_eq!(
                std::fs::read(&bundle.mcp_json).unwrap(),
                corrupt.as_bytes(),
                "损坏 mcp.json 必须保持字节不变"
            );
            // 禁用/隐藏残留仍被外层清理步清掉。
            let file: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&disabled).unwrap()).unwrap();
            assert_eq!(
                file["scopes"]["plain"],
                serde_json::json!([]),
                "卸载回滚后 plain 禁用残留仍必须被清掉"
            );
            assert_eq!(
                file["hidden_scopes"]["plain"],
                serde_json::json!([]),
                "卸载回滚后 plain 隐藏残留仍必须被清掉"
            );
        });
    }

    /// #522 结构性回归:退役清理的禁用/隐藏残留必须走 scope 模块的单临界区 RMW
    /// 助手,extraction.rs 不得再内联「load → 内存 retain → 条件 save」两段独立取锁
    /// 的落盘写法(两段之间并发写方的更新会被旧快照整表覆盖)。源码钉扎与
    /// platform::filesystem 的同类测试同一风格;探测词拼接构造,避免测试源码自匹配。
    #[test]
    fn disabled_cleanup_has_no_inline_two_phase_scope_writes() {
        let source = include_str!("extraction.rs");
        let load = ["load_disabled", "_bundles_for"].concat();
        let save = ["save_disabled", "_bundles_for"].concat();
        let helper = ["remove_bundle_from", "_disabled_scopes"].concat();
        assert_eq!(
            source.matches(&load).count(),
            0,
            "退役清理不得内联 scope 禁用集读取:统一走 scope 模块单临界区 RMW 助手"
        );
        assert_eq!(
            source.matches(&save).count(),
            0,
            "退役清理不得内联 scope 禁用集写方:统一走 scope 模块单临界区 RMW 助手"
        );
        assert!(
            source.matches(&helper).count() >= 1,
            "退役清理必须调用 scope 模块的单临界区 RMW 助手清理所有 scope 残留"
        );
    }
}

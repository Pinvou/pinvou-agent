//! Skill-gate shared by the four CLI connectors (tmeet/dingtalk/feishu/
//! wecom): a per-connector disabled-flag file plus skill apply/discard,
//! table-driven via [`ConnectorGate`] / [`GATES`].
//!
//! 每个连接器模块在自己的文件里声明 [`ConnectorGate`] 表项(就绪探测与技能
//! 落盘指向本模块函数),四份逐字重复的 `{apply_skills, skills_state,
//! *_skills_should_show}` 命令块与 `refresh_connector_auth_gates` 的四段
//! spawn_blocking 收编为 [`ConnectorGate`] 的公共方法。
//!
//! Disable semantics: the `~/.pinvou3/<id>_disabled` file existing means the
//! user manually disabled that connector's skills, orthogonal to connection
//! state (auth). The write side (`set_disabled_flag`) was removed together
//! with the retired `set_*_enabled` commands; connector switches now persist
//! through the unified scope state (`set_disabled_connectors` → marketplace
//! scope), so the gate only reads the flag.

use std::path::PathBuf;

use serde_json::{Value, json};

/// 单个 CLI 连接器的门控配置(表项,注册表见 [`GATES`])。
pub(crate) struct ConnectorGate {
    /// 连接器 id(事件前缀 / scope 同步键,如 `"tmeet"`)。
    pub id: &'static str,
    /// 停用标志文件名(如 `"tmeet_disabled"`)。
    pub disabled_filename: &'static str,
    /// 用户可见名(错误信息前缀:「更新{名}技能失败」/「刷新{名}技能门控失败」)。
    pub display_name: &'static str,
    /// 实时就绪探测(spawn 对应 CLI 查 auth 状态;未装返回 false)。
    pub ready_probe: fn() -> bool,
    /// 按 `visible` 增 / 删本连接器的技能文件 —— 调各自的
    /// `Pinvou3Bundle::apply_*_skills`。
    pub apply_bundle_skills: fn(bool) -> std::io::Result<()>,
}

impl ConnectorGate {
    /// 停用标志文件完整路径:`~/.pinvou3/<disabled_filename>`。
    pub fn disabled_path(&self) -> PathBuf {
        crate::platform::paths::pinvou3_home().join(self.disabled_filename)
    }

    /// 是否被手动停用(停用标志文件存在即停用)。与连接状态正交。
    pub fn is_disabled(&self) -> bool {
        self.disabled_path().exists()
    }

    /// 技能此刻该不该出现在 skills_dir:**未手动停用 且 已连接**。
    /// 启动时(bundle)与命令里都用它判定。注:ready_probe 会 spawn 对应 CLI。
    pub fn skills_should_show(&self) -> bool {
        !self.is_disabled() && (self.ready_probe)()
    }

    /// 按 visible 写 / 删技能文件(带用户可见错误前缀)。
    fn apply_skills(&self, visible: bool) -> Result<(), String> {
        (self.apply_bundle_skills)(visible)
            .map_err(|e| format!("更新{}技能失败: {e}", self.display_name))
    }

    /// `*_apply_skills` 命令公共体:按当前"应否可见"状态写 / 删技能文件。
    /// scope 门禁同步：连接器转为可用等同「新装」——已初始化 code 开关时加入
    /// code 禁用集，保持「code 会话外部能力默认关」语义（与 MCP 新装连接器一致）。
    // &'static self:表项都是进程级 static,线程池闭包按值捕获该共享引用。
    pub async fn apply_skills_command(&'static self) -> Result<Value, String> {
        let show = tokio::task::spawn_blocking(|| -> Result<bool, String> {
            let show = self.skills_should_show();
            // Deny-first transaction boundary (#517 review): register the
            // connector in the initialized DenyAll scopes BEFORE materializing
            // skill files, so a refused gate sync aborts before `apply_skills`
            // exposes anything — the connector can never end up enabled
            // outside the deny list. The sync write can block on the
            // cross-process flock (#515), hence inside spawn_blocking; a
            // refused write (lock unavailable) fails the call so the safety
            // default is never silently skipped.
            crate::features::marketplace::deny_first_register_connector(self.id, show)?;
            if let Err(e) = self.apply_skills(show) {
                // The card renders a localized category message only; the raw
                // cause is logged here (stdout in dev runs, the app log in
                // packaged builds — the backend attaches in all builds now).
                log::warn!("[{}] apply skills failed: {e}", self.id);
                return Err(e);
            }
            if show {
                // Fail-visible belt-and-braces (review #455 R13-B3, preserved
                // through the round-19 merge): deny-first above already
                // registered the pair, and once the companion dirs are
                // materialized the sync's known-clause skips it — this leg
                // only acts (and its marker copy only surfaces) in the corner
                // where the known-clause cannot vouch for a just-applied
                // connector. Swallowing the error would let the connector go
                // live with zero consent in that corner. It runs inside THIS
                // spawn_blocking closure (the second hop it used to own was a
                // pure wrapper with nothing between the two hops): still off
                // the executor like the deny-first gate above, because the
                // sync write can block on the cross-process flock (#515), and
                // a frozen peer must not hang a Tokio worker. The fold merges
                // the old hop's join-error branch into the single
                // "apply skills task failed" log below (a JoinError no longer
                // identifies its phase); the user-visible
                // "spawn_blocking: {e}" copy is unchanged.
                crate::features::marketplace::sync_deny_all_scopes_after_install(self.id).map_err(
                    |e| {
                        log::warn!(
                            "[{}] persisting the default-off consent state failed: {e}",
                            self.id
                        );
                        crate::features::marketplace::scope::consent_sync_failure_message(
                            &format!("{} connected", self.id),
                            &e,
                        )
                    },
                )?;
            }
            Ok(show)
        })
        .await
        .map_err(|e| {
            // The connected-catch on the card replaces this string with the
            // skills_enable_failed code, so without this line the join
            // failure's cause would be lost entirely.
            log::warn!("[{}] apply skills task failed: {e}", self.id);
            format!("spawn_blocking: {e}")
        })??;
        Ok(json!({ "visible": show }))
    }

    /// `*_skills_state` 命令公共体:给前端渲染开关态
    /// `{connected, enabled(=未停用), visible(=connected&&enabled)}`。
    pub async fn skills_state_command(&'static self) -> Result<Value, String> {
        tokio::task::spawn_blocking(|| {
            let disabled = self.is_disabled();
            let connected = (self.ready_probe)();
            Ok::<Value, String>(json!({
                "connected": connected,
                "enabled": !disabled,
                "visible": connected && !disabled,
            }))
        })
        .await
        .map_err(|e| format!("spawn_blocking: {e}"))?
    }

    /// `refresh_connector_auth_gates` 的单连接器步骤(在线程池里跑):
    /// 实时探测应否可见,按结果写 / 删技能目录。
    /// Same deny-first boundary as `apply_skills_command` (#517 review
    /// round 6): the auth-gate refresh and the startup backfill also
    /// materialize skill files, so a connector flipping visible here must
    /// register in the initialized DenyAll scopes first; a refused
    /// registration fails the refresh before anything is exposed.
    pub fn refresh_step(&self) -> Result<bool, String> {
        let show = self.skills_should_show();
        crate::features::marketplace::deny_first_register_connector(self.id, show)?;
        (self.apply_bundle_skills)(show)
            .map_err(|e| format!("刷新{}技能门控失败: {e}", self.display_name))?;
        if show {
            // Round-30 m1 (review #455): materialization must not outrun the
            // consent rows — a connector connected pre-PR whose fire-and-forget
            // consent sync silently failed stays live-by-absence in its
            // initialized scope forever (the stored list is the sole truth
            // there). Round-31 BLOCKER (review #455): a plain membership push
            // here re-added the row a user enable had removed at EVERY boot
            // (`show` is always true for a connected connector — the legacy
            // disable flags are read-only), silently reverting explicit
            // enables. The startup refresh therefore uses the LEDGER-GATED
            // variant (`sync_deny_all_scopes_refresh`): it only pushes rows
            // for packs never synced before; a user enable removes the row
            // while the ledger entry survives, and teardown clears the ledger
            // so a fresh install / reconnect re-syncs. The connect command's
            // own sync stays un-gated (fresh-install semantics: connecting is
            // a user action and legitimately re-arms default-off). Failures
            // propagate like the connect path, but this function's startup
            // caller surfaces them only via a frontend console.warn — the
            // failure is additionally marked on the startup timeline where it
            // is observable.
            crate::features::marketplace::sync_deny_all_scopes_refresh(self.id).map_err(|e| {
                crate::platform::startup::mark_with_detail(
                    "rust",
                    "connector_consent_sync:failed",
                    &format!("{}: {e}", self.id),
                );
                format!(
                    "{}技能门控刷新后的默认关同意同步失败: {e}",
                    self.display_name
                )
            })?;
        }
        Ok(show)
    }
}

/// 四个 CLI 连接器的门控注册表;`refresh_connector_auth_gates` 按此并行刷新。
/// 各表项与所属连接器模块同居一处,新增 CLI 连接器 = 新模块 + 在此登记一行。
pub(crate) static GATES: [&'static ConnectorGate; 4] = [
    &super::feishu::FEISHU_GATE,
    &super::wecom::WECOM_GATE,
    &super::dingtalk::DINGTALK_GATE,
    &super::tmeet::TMEET_GATE,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal hand-built gate driving the default implementations: exercises
    /// the disabled-path derivation under a temporary `PINVOU3_HOME`.
    fn fake_gate() -> ConnectorGate {
        ConnectorGate {
            id: "fake",
            disabled_filename: "fake_disabled",
            display_name: "测试",
            ready_probe: || false,
            apply_bundle_skills: |_| Ok(()),
        }
    }

    /// `disabled_path` 跟随 `PINVOU3_HOME`,且文件名由 `disabled_filename` 决定。
    #[test]
    fn disabled_path_is_derived_from_pinvou3_home() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
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

        let gate = fake_gate();
        assert_eq!(
            gate.disabled_path(),
            crate::platform::paths::pinvou3_home().join("fake_disabled")
        );
        // 标志文件不存在 → 未停用;ready_probe 恒 false → 不应显示。
        assert!(!gate.is_disabled());
        assert!(!gate.skills_should_show());

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

    /// Marker "materialization": the closure cannot capture, so it touches the
    /// (env-isolated) PINVOU3_HOME — exactly what the real apply fns do.
    fn gate_apply_marker(visible: bool) -> std::io::Result<()> {
        let marker = crate::platform::paths::pinvou3_home().join("gatetest-skills");
        if visible {
            std::fs::create_dir_all(marker)
        } else {
            let _ = std::fs::remove_dir_all(&marker);
            Ok(())
        }
    }

    /// Round-8 review: the deny-first wiring is pinned THROUGH the two
    /// production command bodies (`apply_skills_command`, `refresh_step`),
    /// not only through `deny_first_register_connector` driven directly — a
    /// revert of these bodies to apply-then-swallow previously kept the
    /// entire suite green. A directory at the lock path makes the gate write
    /// refuse; nothing may materialize and the refusal must surface.
    #[test]
    fn command_bodies_register_deny_first_and_refusal_lands_nothing() {
        use crate::features::marketplace::ConnectorScope;
        use crate::features::marketplace::scope::{
            load_disabled_bundles_for, save_disabled_bundles_for,
        };

        // Shared RAII temp-home helper (round 9): a failing assertion unwinds
        // past a straight-line env restore, which would leave PINVOU3_HOME
        // pointed at a deleted temp dir and cascade unrelated failures.
        crate::platform::test_support::with_temp_home("pinvou3-skillgate-body", || {
            // Initialized (empty) Code scope so a fresh registration has
            // somewhere to land instead of vanishing into the default.
            save_disabled_bundles_for(ConnectorScope::Code, &[]).unwrap();
            let home = crate::platform::paths::pinvou3_home();
            let lock_dir = home.join("disabled_bundles.lock");
            // The save above created the lock FILE; replace it with a
            // directory so every lock open fails.
            let _ = std::fs::remove_file(&lock_dir);
            std::fs::create_dir_all(&lock_dir).unwrap();
            let marker = home.join("gatetest-skills");
            // apply_skills_command takes &'static self (production gates are
            // process-static table entries); leak the harness gate to match.
            let gate: &'static ConnectorGate = Box::leak(Box::new(ConnectorGate {
                id: "gatetest",
                disabled_filename: "gatetest_disabled",
                display_name: "GateTest",
                ready_probe: || true,
                apply_bundle_skills: gate_apply_marker,
            }));
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            // Refused gate: both production bodies surface the refusal and
            // land nothing.
            let err = rt
                .block_on(gate.apply_skills_command())
                .expect_err("broken lock must refuse apply_skills_command");
            assert!(
                err.contains("disabled_bundles.lock"),
                "refusal must name the lock: {err}"
            );
            assert!(!marker.exists(), "a refused gate must materialize nothing");
            let err = gate
                .refresh_step()
                .expect_err("broken lock must refuse refresh_step");
            assert!(
                err.contains("disabled_bundles.lock"),
                "refusal must name the lock: {err}"
            );
            assert!(
                !marker.exists(),
                "a refused refresh must materialize nothing"
            );

            // Lock available: both bodies proceed, register the fresh id, land.
            std::fs::remove_dir_all(&lock_dir).unwrap();
            let value = rt.block_on(gate.apply_skills_command()).unwrap();
            assert_eq!(value["visible"], serde_json::json!(true));
            assert!(marker.exists(), "the unrefused apply must materialize");
            assert!(
                load_disabled_bundles_for(ConnectorScope::Code).contains(&"gatetest".to_string()),
                "the command body must register the fresh connector deny-first"
            );
            std::fs::remove_dir_all(&marker).unwrap();
            assert!(gate.refresh_step().unwrap());
            assert!(marker.exists(), "the unrefused refresh must materialize");
        });
    }

    /// 注册表恰好覆盖四个 CLI 连接器,且 id 与文件名前缀一一对应。
    #[test]
    fn gates_table_covers_the_four_cli_connectors() {
        let ids: Vec<_> = GATES.iter().map(|g| g.id).collect();
        assert_eq!(ids, vec!["feishu", "wecom", "dingtalk", "tmeet"]);
        for gate in GATES {
            let expected_filename = format!("{}_disabled", gate.id);
            let gate_id = gate.id;
            assert_eq!(
                gate.disabled_filename, expected_filename,
                "{gate_id} 的停用标志文件名应与其 id 对应"
            );
        }
    }

    /// Round-31 BLOCKER negative control (review #455): an initialized scope,
    /// a connected connector EXPLICITLY ENABLED by the user, then
    /// `refresh_step` — the stored list must still lack the id. This is the
    /// test that fails on the round-30 form (the plain membership push
    /// re-added the row at every boot, silently reverting explicit enables):
    /// the ledger-gated sync is what makes the enable sticky. Also pins the
    /// reconnect direction — teardown (exact removal) clears the ledger
    /// entry, so a fresh install / reconnect re-syncs default-off.
    #[test]
    fn refresh_step_does_not_revert_explicit_enable_but_reconnect_resyncs() {
        use crate::features::marketplace::scope::remove_bundle_from_disabled_scopes_exact;
        use crate::features::marketplace::{
            ConnectorScope, load_disabled_bundles_for, save_disabled_bundles_for,
            sync_deny_all_scopes_after_install,
        };
        use crate::platform::test_support::with_temp_home;

        with_temp_home("pinvou3-skillgate-ledger", || {
            // Initialize plain via the raw store shape (the migration's own
            // verdict write; the composer's first write seeds the same).
            let path = crate::platform::paths::pinvou3_home().join("disabled_bundles.json");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                &path,
                r#"{"scopes":{},"initialized":["plain"],"plain_defaults_migrated":true}"#,
            )
            .unwrap();

            // First sync (install/connect equivalent): the row lands and the
            // pair is ledgered.
            sync_deny_all_scopes_after_install("connector-x").unwrap();
            assert!(
                load_disabled_bundles_for(ConnectorScope::Plain)
                    .contains(&"connector-x".to_string()),
                "the first sync must persist the install-default row"
            );

            // The user enables the pack: the composer whole-list write drops
            // the row + marker while the ledger entry survives.
            save_disabled_bundles_for(ConnectorScope::Plain, &[]).unwrap();
            assert!(
                !load_disabled_bundles_for(ConnectorScope::Plain)
                    .contains(&"connector-x".to_string()),
                "fixture: the enable removed the row"
            );

            // The startup refresh runs for the connected connector (probe
            // true, no disable flag): the LEDGER must keep it from re-adding.
            let gate = ConnectorGate {
                id: "connector-x",
                disabled_filename: "connector-x_disabled",
                display_name: "测试连接器",
                ready_probe: || true,
                apply_bundle_skills: |_| Ok(()),
            };
            let visible = gate.refresh_step().unwrap();
            assert!(
                visible,
                "fixture: the connector is connected and not disabled"
            );
            assert!(
                !load_disabled_bundles_for(ConnectorScope::Plain)
                    .contains(&"connector-x".to_string()),
                "the startup refresh must NOT revert an explicit user enable (the round-30 form fails here)"
            );

            // Reconnect direction: teardown (exact removal) clears the
            // ledger entry, so the next sync re-syncs default-off.
            remove_bundle_from_disabled_scopes_exact("connector-x").unwrap();
            sync_deny_all_scopes_after_install("connector-x").unwrap();
            assert!(
                load_disabled_bundles_for(ConnectorScope::Plain)
                    .contains(&"connector-x".to_string()),
                "a fresh install / reconnect must re-sync default-off after teardown"
            );
        });
    }

    /// Round-33 MAJOR 2 (review #455): the connect-path consent-persist
    /// failure copy carries the ONE shared frontend marker
    /// (`scope::CONSENT_SYNC_FAILURE_MARKER`) — the localized template on the
    /// store cards keys on exactly this string, so a rewording here must move
    /// the frontend matcher in the same commit. The literal is asserted (not
    /// reformatted and re-matched) so a marker rename fails here instead of
    /// silently degrading the frontend guidance to generic copy.
    #[test]
    fn skill_gate_consent_failure_message_keeps_the_frontend_marker() {
        assert_eq!(
            crate::features::marketplace::scope::CONSENT_SYNC_FAILURE_MARKER,
            "persisting their default-off consent state failed",
            "the shipped marker is the string the frontend consentFailure matcher keys on"
        );
        let message = format!(
            "{} connected, but {}: new sessions will enable it by default — turn it off in the tools list: store down",
            "wecom",
            crate::features::marketplace::scope::CONSENT_SYNC_FAILURE_MARKER
        );
        assert!(
            message.contains(crate::features::marketplace::scope::CONSENT_SYNC_FAILURE_MARKER),
            "the shipped message must carry the frontend-matched marker: {message}"
        );
    }

    /// Round-37 C1 (review #455): a consent persist failure between connect
    /// and enable left the pair unledgered, so the startup refresh reverted
    /// the user's enable across the restart (round-30 family residue). The
    /// explicit-enable path now RECORDS the ledger pair, closing the window:
    /// connect-sync persist fails → nothing lands → the enable succeeds →
    /// refresh must not backfill the row.
    #[test]
    fn explicit_enable_records_the_ledger_over_a_failed_connect_sync() {
        use crate::features::marketplace::scope::{
            enable_packages_in_scope, sync_deny_all_scopes_after_install,
        };
        use crate::features::marketplace::{ConnectorScope, load_disabled_bundles_for};
        use crate::platform::test_support::with_temp_home;

        with_temp_home("pinvou3-skillgate-ledger-failedconnect", || {
            // Pre-warm: the first read persists the freeze verdict, settling
            // that write so the failpoint below hits the SYNC's save.
            let _ = crate::features::marketplace::scope::load_disabled_bundles_for(
                ConnectorScope::Plain,
            );
            // The connect-path sync's persist FAILS: neither the row nor the
            // ledger entry reaches disk.
            let _failpoint =
                crate::features::marketplace::scope::fail_next_disabled_bundles_write_for_test();
            assert!(
                sync_deny_all_scopes_after_install("feishu").is_err(),
                "fixture: the connect sync must fail on the injected persist error"
            );

            // The user's first enable still succeeds (nothing to remove; the
            // materialization arm seeds the scope).
            let outcome =
                enable_packages_in_scope(ConnectorScope::Plain, &["feishu".to_string()]).unwrap();
            assert!(
                outcome.state_changed,
                "fixture: the enable must materialize"
            );
            assert!(
                !load_disabled_bundles_for(ConnectorScope::Plain).contains(&"feishu".to_string()),
                "fixture: the enable removed the id"
            );

            // The startup refresh must stay gated by the ledger the ENABLE
            // wrote — no silent default-off backfill over the enable.
            let gate = ConnectorGate {
                id: "feishu",
                disabled_filename: "feishu_disabled",
                display_name: "测试连接器",
                ready_probe: || true,
                apply_bundle_skills: |_| Ok(()),
            };
            gate.refresh_step().unwrap();
            assert!(
                !load_disabled_bundles_for(ConnectorScope::Plain).contains(&"feishu".to_string()),
                "the enable must record the ledger, or the next boot reverts it (round-30 family residue)"
            );
        });
    }

    /// Round-32 MAJOR 1 (review #455) twin negative control: the connect
    /// happens while plain is STILL UNINITIALIZED (fresh home), the user's
    /// first enable materializes the scope, and only then does the startup
    /// refresh run — the stored list must still lack the id. This fails on
    /// the round-31 form: the uninitialized connect sync recorded no ledger
    /// entry (its arm `continue`d before the write), so the refresh
    /// classified the row-absence as "never synced" and backfilled the
    /// default-off row over the enable across the restart. The fix records
    /// the pair in the uninitialized arm.
    #[test]
    fn enable_after_uninitialized_connect_survives_startup_refresh() {
        use crate::features::marketplace::scope::{
            enable_packages_in_scope, sync_deny_all_scopes_after_install,
        };
        use crate::features::marketplace::{ConnectorScope, load_disabled_bundles_for};
        use crate::platform::test_support::with_temp_home;

        with_temp_home("pinvou3-skillgate-ledger-uninit", || {
            // Fresh home: plain does not exist yet. The connector connects
            // FIRST (the connect-path sync — a user action) while plain is
            // uninitialized by design.
            sync_deny_all_scopes_after_install("feishu").unwrap();

            // The user's first composer/welcome write enables the pack: the
            // materialization arm seeds `initialized` + the expansion
            // snapshot with the id removed.
            let outcome =
                enable_packages_in_scope(ConnectorScope::Plain, &["feishu".to_string()]).unwrap();
            assert!(
                outcome.not_applied.is_empty(),
                "fixture: feishu is a builtin id and must sit in the uninitialized expansion"
            );
            assert!(
                outcome.state_changed,
                "fixture: the first enable must materialize the scope"
            );
            assert!(
                !load_disabled_bundles_for(ConnectorScope::Plain).contains(&"feishu".to_string()),
                "fixture: the enable removed the id"
            );

            // Restart equivalent: the startup refresh runs for the connected
            // gate — the ledger entry recorded by the uninitialized connect
            // sync must keep the enable in place.
            let gate = ConnectorGate {
                id: "feishu",
                disabled_filename: "feishu_disabled",
                display_name: "测试连接器",
                ready_probe: || true,
                apply_bundle_skills: |_| Ok(()),
            };
            gate.refresh_step().unwrap();
            assert!(
                !load_disabled_bundles_for(ConnectorScope::Plain).contains(&"feishu".to_string()),
                "the startup refresh must not backfill over an enable made after an uninitialized-scope connect (round-32 MAJOR 1)"
            );
        });
    }

    /// Round-32 minor 2 (review #455): the refresh's PUSH leg itself is
    /// load-bearing and had no pin — a regression turning the refresh into a
    /// no-op kept the suite green while silently disabling the round-30 m1
    /// backfill. The refresh must backfill the default-off row for a pair no
    /// sync ever recorded, AND record the ledger entry while doing it, so a
    /// user enable right after the backfill survives the next refresh (the
    /// recording is what keeps the round-31 BLOCKER re-opened shut).
    #[test]
    fn refresh_backfills_never_synced_row_and_records_the_ledger() {
        use crate::features::marketplace::{
            ConnectorScope, load_disabled_bundles_for, save_disabled_bundles_for,
        };
        use crate::platform::test_support::with_temp_home;

        with_temp_home("pinvou3-skillgate-ledger-backfill", || {
            // Initialized plain (post-migration shape), connector-x never
            // seen by any sync: no stored row, no ledger entry.
            let path = crate::platform::paths::pinvou3_home().join("disabled_bundles.json");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                &path,
                r#"{"scopes":{},"initialized":["plain"],"plain_defaults_migrated":true}"#,
            )
            .unwrap();

            let gate = ConnectorGate {
                id: "connector-x",
                disabled_filename: "connector-x_disabled",
                display_name: "测试连接器",
                ready_probe: || true,
                apply_bundle_skills: |_| Ok(()),
            };
            gate.refresh_step().unwrap();
            assert!(
                load_disabled_bundles_for(ConnectorScope::Plain)
                    .contains(&"connector-x".to_string()),
                "the startup refresh must backfill the install-default row for a never-synced pair"
            );

            // The ledger entry must be recorded by that same refresh: a user
            // enable right after the backfill must survive the next refresh.
            save_disabled_bundles_for(ConnectorScope::Plain, &[]).unwrap();
            gate.refresh_step().unwrap();
            assert!(
                !load_disabled_bundles_for(ConnectorScope::Plain)
                    .contains(&"connector-x".to_string()),
                "the backfill must record the ledger, or the next refresh reverts the enable (round-31 BLOCKER re-opened)"
            );
        });
    }

    /// Round-32 minor 2 (review #455): the TEARDOWN ledger-clear is what
    /// re-arms the LEDGER-GATED startup refresh. The negative control above
    /// drives its reconnect leg with the un-gated connect variant (which
    /// re-adds the row regardless), so deleting
    /// `remove_bundle_from_disabled_scopes_exact`'s ledger clear failed
    /// nothing. This pin drives the gated refresh after teardown: the row
    /// must come back, which only happens when the clear actually removed
    /// the entry.
    #[test]
    fn teardown_ledger_clear_re_arms_the_gated_startup_refresh() {
        use crate::features::marketplace::scope::remove_bundle_from_disabled_scopes_exact;
        use crate::features::marketplace::{
            ConnectorScope, load_disabled_bundles_for, save_disabled_bundles_for,
            sync_deny_all_scopes_after_install,
        };
        use crate::platform::test_support::with_temp_home;

        with_temp_home("pinvou3-skillgate-ledger-teardown", || {
            let path = crate::platform::paths::pinvou3_home().join("disabled_bundles.json");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                &path,
                r#"{"scopes":{},"initialized":["plain"],"plain_defaults_migrated":true}"#,
            )
            .unwrap();

            // Install/connect sync: the row and the ledger entry land.
            sync_deny_all_scopes_after_install("connector-x").unwrap();

            // User enable: the row drops, the ledger entry must survive.
            save_disabled_bundles_for(ConnectorScope::Plain, &[]).unwrap();
            let raw: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert!(
                raw["install_default_synced"]
                    .as_array()
                    .map(|entries| entries.iter().any(|entry| entry == "plain:connector-x"))
                    .unwrap_or(false),
                "fixture: the enable must keep the ledger entry"
            );

            // Teardown: the exact removal clears the pack's ledger rows…
            remove_bundle_from_disabled_scopes_exact("connector-x").unwrap();
            let raw: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert!(
                raw["install_default_synced"]
                    .as_array()
                    .map(|entries| entries.is_empty())
                    .unwrap_or(false),
                "the teardown must clear the pack's ledger entries"
            );

            // …which re-arms the GATED startup refresh for the same pack (a
            // fresh install seen at boot before the connect sync runs).
            let gate = ConnectorGate {
                id: "connector-x",
                disabled_filename: "connector-x_disabled",
                display_name: "测试连接器",
                ready_probe: || true,
                apply_bundle_skills: |_| Ok(()),
            };
            gate.refresh_step().unwrap();
            assert!(
                load_disabled_bundles_for(ConnectorScope::Plain)
                    .contains(&"connector-x".to_string()),
                "after teardown the startup refresh must re-sync default-off"
            );
        });
    }
}

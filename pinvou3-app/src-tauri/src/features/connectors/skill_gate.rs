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

    /// `*_apply_skills` 命令公共体:按当前"应否可见"状态写 / 删技能文件。
    /// scope 门禁同步（feat/connector-connect-grants-enable）:连接动作本身就是
    /// 同意——连接成功即授予启用（`grant_connect_enable`）,连接器在所有
    /// DenyAll scope 落地为「开」;用户之后在 composer 里显式关闭写入行并移除
    /// 授权,且 ledger 保证启动刷新/重登录不会翻回。打包/技能「安装」仍走
    /// 安装侧默认关注册（`sync_deny_all_scopes_after_install`）,语义不变。
    // &'static self:表项都是进程级 static,线程池闭包按值捕获该共享引用。
    pub async fn apply_skills_command(&'static self) -> Result<Value, String> {
        let show = tokio::task::spawn_blocking(|| -> Result<bool, String> {
            let show = self.skills_should_show();
            // Deny-first transaction boundary: record the
            // connector's connect consent BEFORE materializing
            // skill files, so a refused gate write aborts before `apply_skills`
            // exposes anything — the connector can never end up live
            // without its consent recorded. The consent write can block on the
            // cross-process flock (#515), hence inside spawn_blocking; a
            // refused write (lock unavailable) fails the call so the safety
            // default is never silently skipped.
            let granted_here =
                crate::features::marketplace::deny_first_register_connector(self.id, show)?;
            // The card renders a localized category message only; the raw
            // cause is logged here (stdout in dev runs, the app log in
            // packaged builds — the backend attaches in all builds now).
            (self.apply_bundle_skills)(show)
                .map_err(|e| format!("更新{}技能失败: {e}", self.display_name))
                .inspect_err(|e| {
                    log::warn!("[{}] apply skills failed: {e}", self.id);
                    // M4 compensation (review fix wave): the grant above
                    // already lifted the stale rows and flipped every DenyAll
                    // expansion to ON — left standing behind this failure the
                    // connector would be live while the card reports failure.
                    // Revoke grant + ledger so the state returns to
                    // fail-closed "never consented" and a retry reruns the
                    // full connect; best-effort, the apply error stays the
                    // surfaced one.
                    if granted_here {
                        if let Err(revoke_err) =
                            crate::features::marketplace::revoke_connect_enable(self.id)
                        {
                            log::warn!(
                                "[{}] revoking the connect grant after the apply failure failed: {revoke_err}",
                                self.id
                            );
                        }
                    }
                })?;
            if show {
                // Fail-visible belt-and-braces: deny-first above already
                // recorded the pair, and once the companion dirs are
                // materialized the grant's known-clause skips it — this leg
                // only acts (and its marker copy only surfaces) in the corner
                // where the known-clause cannot vouch for a just-applied
                // connector. Swallowing the error would let the connector go
                // live with zero recorded consent in that corner. It runs inside THIS
                // spawn_blocking closure: still off the executor like the
                // deny-first gate above, because the consent write can block on
                // the cross-process flock (#515), and a frozen peer must not
                // hang a Tokio worker.
                crate::features::marketplace::grant_connect_enable(self.id).map_err(|e| {
                    log::warn!(
                        "[{}] persisting the connect-enabled consent state failed: {e}",
                        self.id
                    );
                    crate::features::marketplace::scope::connect_consent_sync_failure_message(
                        &format!("{} connected", self.id),
                        &e,
                    )
                })?;
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
    /// Same deny-first boundary as `apply_skills_command`: the auth-gate
    /// refresh and the startup backfill also materialize skill files, so a
    /// connector flipping visible here must register in the initialized
    /// DenyAll scopes first; a refused registration fails the refresh before
    /// anything is exposed.
    pub fn refresh_step(&self) -> Result<bool, String> {
        let show = self.skills_should_show();
        // The refresh variant, not the gesture one: a boot refresh has no
        // user gesture behind it, so its consent write is ledger-gated and
        // never lifts a stored verdict row (routing this through the gesture
        // gate silently flipped the legacy-migrated and post-failed-persist
        // cohorts ON — the review BLOCKER).
        let granted_here =
            crate::features::marketplace::deny_first_register_connector_refresh(self.id, show)?;
        (self.apply_bundle_skills)(show)
            .map_err(|e| format!("刷新{}技能门控失败: {e}", self.display_name))
            .inspect_err(|_| {
                // Same M4 compensation as the connect body: a backfill grant
                // that outlives a failed materialization would leave the
                // connector ON behind a failed boot refresh; with grant and
                // ledger revoked the next boot retries from "never
                // consented".
                if granted_here {
                    if let Err(revoke_err) =
                        crate::features::marketplace::revoke_connect_enable(self.id)
                    {
                        log::warn!(
                            "[{}] revoking the connect grant after the refresh materialization failure failed: {revoke_err}",
                            self.id
                        );
                    }
                }
            })?;
        if show {
            // Round-30 m1 (review #455): materialization must not outrun the
            // recorded consent — a connector connected pre-PR whose fire-and-forget
            // consent write silently failed must not stay in an
            // unregistered state (the stored list is the sole truth in its
            // initialized scope). Round-31 BLOCKER (review #455): a plain
            // membership write here re-added the row a user enable had removed
            // at EVERY boot (`show` is always true for a connected connector —
            // the legacy disable flags are read-only), silently reverting
            // explicit user verdicts. The startup refresh therefore uses the
            // LEDGER-GATED variant (`grant_connect_enable_refresh`): it only
            // GRANTS (feat/connector-connect-grants-enable — the backfill
            // records the connector enabled, no default-off row) for packs
            // never consented before; a user toggle-off lives in a stored row
            // while the ledger entry survives, and teardown clears the ledger
            // (and the grant) so a fresh install / reconnect re-grants. The
            // connect command's own grant stays un-gated (fresh-install
            // semantics: connecting is a user action and legitimately re-grants
            // over a stale default-off row). Failures
            // propagate like the connect path, but this function's startup
            // caller surfaces them only via a frontend console.warn — the
            // failure is additionally marked on the startup timeline where it
            // is observable.
            crate::features::marketplace::grant_connect_enable_refresh(self.id).map_err(|e| {
                crate::platform::startup::mark_with_detail(
                    "rust",
                    "connector_consent_sync:failed",
                    &format!("{}: {e}", self.id),
                );
                format!(
                    "{}技能门控刷新后的连接启用授权落盘失败: {e}",
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
        crate::platform::test_support::with_temp_home("pinvou3-skillgate-path", || {
            let gate = fake_gate();
            assert_eq!(
                gate.disabled_path(),
                crate::platform::paths::pinvou3_home().join("fake_disabled")
            );
            // 标志文件不存在 → 未停用;ready_probe 恒 false → 不应显示。
            assert!(!gate.is_disabled());
            assert!(!gate.skills_should_show());
        });
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

    /// A connected, not-disabled harness gate: the only per-site variation is
    /// the id (the derived `{id}_disabled` filename is itself the pinned
    /// invariant `gates_table_covers_the_four_cli_connectors` asserts).
    fn connected_gate(id: &'static str) -> ConnectorGate {
        ConnectorGate {
            id,
            disabled_filename: Box::leak(format!("{id}_disabled").into()),
            display_name: "测试连接器",
            ready_probe: || true,
            apply_bundle_skills: |_| Ok(()),
        }
    }

    /// Seed the post-migration plain-scope shape ("plain" initialized, nothing
    /// stored) — the load-bearing precondition the ledger tests share verbatim.
    fn seed_initialized_plain_scope() {
        let path = crate::platform::paths::pinvou3_home().join("disabled_bundles.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"scopes":{},"initialized":["plain"],"plain_defaults_migrated":true}"#,
        )
        .unwrap();
    }

    /// The deny-first wiring is pinned THROUGH the two production command
    /// bodies (`apply_skills_command`, `refresh_step`), not only through
    /// `deny_first_register_connector` driven directly — a revert of these
    /// bodies to apply-then-swallow keeps the entire suite green. A
    /// directory at the lock path makes the gate write
    /// refuse; nothing may materialize and the refusal must surface.
    #[test]
    fn command_bodies_register_deny_first_and_refusal_lands_nothing() {
        use crate::features::marketplace::ConnectorScope;
        use crate::features::marketplace::scope::{
            load_disabled_bundles_for, save_disabled_bundles_for,
        };

        // Uses the shared RAII helper (see its doc for the unwind-restore
        // rationale).
        crate::platform::test_support::with_temp_home("pinvou3-skillgate-body", || {
            // Initialized (empty) Code scope so a fresh registration has
            // somewhere to land instead of vanishing into the default.
            save_disabled_bundles_for(ConnectorScope::Code, &[]).unwrap();
            let home = crate::platform::paths::pinvou3_home();
            // The save above created the lock FILE; replace it with a
            // directory so every lock open fails.
            crate::features::marketplace::scope::break_scope_lock_for_test();
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
            // land nothing. The APPLY path's persist refusal carries the
            // shared connect-consent marker (the realistic #515 flock refusal
            // happens deny-first, and the store cards key on it to show the
            // fail-closed copy instead of "try again"); the refresh path
            // propagates raw for the startup timeline's own copy.
            let err = rt
                .block_on(gate.apply_skills_command())
                .expect_err("broken lock must refuse apply_skills_command");
            assert!(
                err.contains("disabled_bundles.lock"),
                "refusal must name the lock: {err}"
            );
            assert!(
                err.contains(
                    crate::features::marketplace::scope::CONNECT_CONSENT_SYNC_FAILURE_MARKER
                ),
                "the connect path's persist refusal must carry the frontend marker: {err}"
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

            // Lock available: both bodies proceed, record the connect consent
            // (ENABLE grant, feat/connector-connect-grants-enable), land.
            std::fs::remove_dir_all(home.join("disabled_bundles.lock")).unwrap();
            let value = rt.block_on(gate.apply_skills_command()).unwrap();
            assert_eq!(value["visible"], serde_json::json!(true));
            assert!(marker.exists(), "the unrefused apply must materialize");
            {
                // The grant is recorded, not a default-off row: the stored
                // Code list stays empty (the scope was initialized empty) and
                // the raw store carries the grant + both ledger pairs. The
                // effective-set flip itself is pinned in scope.rs tests with a
                // real expansion member (a fake gate id is not in the
                // expansion).
                let raw: serde_json::Value = serde_json::from_str(
                    &std::fs::read_to_string(home.join("disabled_bundles.json")).unwrap(),
                )
                .unwrap();
                assert!(
                    raw["connect_granted"]
                        .as_array()
                        .map(|ids| ids.iter().any(|id| id == "gatetest"))
                        .unwrap_or(false),
                    "the command body must record the connect grant: {raw}"
                );
                let ledger = raw["install_default_synced"].as_array().unwrap();
                assert!(
                    ledger.contains(&serde_json::json!("code:gatetest"))
                        && ledger.contains(&serde_json::json!("plain:gatetest")),
                    "the grant must record both scope ledger pairs: {raw}"
                );
                assert!(
                    !load_disabled_bundles_for(ConnectorScope::Code)
                        .contains(&"gatetest".to_string()),
                    "the command body must NOT register a default-off row anymore"
                );
            }
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

    /// Round-31 BLOCKER negative control (review #455), flipped to the grant
    /// direction (feat/connector-connect-grants-enable): an initialized scope,
    /// a connected connector the user explicitly turned OFF after its connect
    /// grant, then `refresh_step` — the stored row must survive. This is the
    /// test that fails on the round-30 form (a plain membership push re-added
    /// the row at every boot); under the grant it is the re-GRANT that must
    /// stay ledger-gated: a refresh re-granting would REMOVE the user's row.
    /// Also pins the reconnect direction — teardown (exact removal) clears the
    /// ledger entry AND the grant, so a fresh install / reconnect re-grants.
    #[test]
    fn refresh_step_does_not_revert_user_off_but_reconnect_regrants() {
        use crate::features::marketplace::scope::remove_bundle_from_disabled_scopes_exact;
        use crate::features::marketplace::{
            ConnectorScope, load_disabled_bundles_for, save_disabled_bundles_for,
        };
        use crate::platform::test_support::with_temp_home;

        with_temp_home("pinvou3-skillgate-ledger", || {
            // Initialize plain via the raw store shape (the migration's own
            // verdict write; the composer's first write seeds the same).
            seed_initialized_plain_scope();

            // First connect: the grant lands — the connector is ENABLED (no
            // stored row) and the pair is ledgered.
            crate::features::marketplace::scope::grant_connect_enable("connector-x").unwrap();
            assert!(
                !load_disabled_bundles_for(ConnectorScope::Plain)
                    .contains(&"connector-x".to_string()),
                "the connect grant must leave the connector enabled"
            );

            // The user turns the connector off: the composer whole-list write
            // adds the row and removes the grant, while the ledger entry
            // survives.
            save_disabled_bundles_for(ConnectorScope::Plain, &["connector-x".to_string()]).unwrap();
            assert!(
                load_disabled_bundles_for(ConnectorScope::Plain)
                    .contains(&"connector-x".to_string()),
                "fixture: the user off added the row"
            );

            // The startup refresh runs for the connected connector (probe
            // true, no disable flag): the LEDGER must keep it from
            // re-granting — a re-grant would delete the user's row (the
            // round-30/31 family failure in the grant direction).
            let gate = connected_gate("connector-x");
            let visible = gate.refresh_step().unwrap();
            assert!(
                visible,
                "fixture: the connector is connected and not disabled"
            );
            assert!(
                load_disabled_bundles_for(ConnectorScope::Plain)
                    .contains(&"connector-x".to_string()),
                "the startup refresh must NOT revert an explicit user off (a ledger-gate loss re-grants here)"
            );

            // Reconnect direction: teardown (exact removal) clears the
            // ledger entry and the grant, so the next consent write
            // re-grants the connector enabled.
            remove_bundle_from_disabled_scopes_exact("connector-x").unwrap();
            assert!(
                !load_disabled_bundles_for(ConnectorScope::Plain)
                    .contains(&"connector-x".to_string()),
                "fixture: the teardown cleared the stored row"
            );
            crate::features::marketplace::scope::grant_connect_enable("connector-x").unwrap();
            assert!(
                !load_disabled_bundles_for(ConnectorScope::Plain)
                    .contains(&"connector-x".to_string()),
                "a fresh install / reconnect must re-grant the connector enabled after teardown"
            );
        });
    }

    /// Review BLOCKER regression pin, end-to-end through the production boot
    /// body: a connect whose grant persist FAILED (fail-closed, nothing
    /// ledgered), then a composer toggle-off (the composer writer never
    /// records the ledger) — the next boot's `refresh_step` probes the
    /// connector as shown with NO ledger entry, and routing that through the
    /// gesture gate used to delete the user's row and silently flip the
    /// connector ON. The refresh must drive the verdict-respecting variant:
    /// the row, the empty grant set, and the empty ledger all survive.
    #[test]
    fn boot_refresh_never_reverts_a_verdict_after_a_failed_connect() {
        use crate::features::marketplace::{ConnectorScope, load_disabled_bundles_for};

        crate::platform::test_support::with_temp_home("pinvou3-skillgate-bootverdict", || {
            seed_initialized_plain_scope();
            // Warm the freeze verdict so the failpoint below hits the GRANT's
            // save (same pre-warm as the explicit-enable pin).
            let _ = load_disabled_bundles_for(ConnectorScope::Plain);
            let _failpoint =
                crate::features::marketplace::scope::fail_next_disabled_bundles_write_for_test();
            assert!(
                crate::features::marketplace::scope::grant_connect_enable("feishu").is_err(),
                "fixture: the connect grant must fail on the injected persist error"
            );

            // The user turns the connector off: the row lands, nothing is
            // ledgered, nothing is granted.
            crate::features::marketplace::save_disabled_bundles_for(
                ConnectorScope::Plain,
                &["feishu".to_string()],
            )
            .unwrap();
            let raw: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(
                    crate::platform::paths::pinvou3_home().join("disabled_bundles.json"),
                )
                .unwrap(),
            )
            .unwrap();
            assert!(
                raw["connect_granted"]
                    .as_array()
                    .is_none_or(|ids| ids.iter().all(|id| id != "feishu")),
                "fixture: the failed connect left no grant: {raw}"
            );
            assert!(
                !raw["install_default_synced"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|entry| entry == "plain:feishu"),
                "fixture: no ledger entry may exist — this is the guard's blind-spot cohort: {raw}"
            );

            // The boot: show=true (connected, no disable flag), no ledger —
            // the refresh must not lift the verdict.
            let gate = connected_gate("feishu");
            gate.refresh_step().unwrap();
            assert!(
                load_disabled_bundles_for(ConnectorScope::Plain).contains(&"feishu".to_string()),
                "the boot refresh must not revert the user's post-failure off"
            );
            let raw: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(
                    crate::platform::paths::pinvou3_home().join("disabled_bundles.json"),
                )
                .unwrap(),
            )
            .unwrap();
            assert!(
                raw["connect_granted"]
                    .as_array()
                    .is_none_or(|ids| ids.iter().all(|id| id != "feishu")),
                "the boot refresh must not grant over the verdict: {raw}"
            );
            assert!(
                !raw["install_default_synced"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|entry| entry.as_str().is_some_and(|s| s.ends_with(":feishu"))),
                "the boot refresh must not ledger over the verdict: {raw}"
            );
        });
    }

    /// M4 compensation pin (review fix wave): a connect whose grant landed
    /// but whose skill materialization then FAILED must not leave the
    /// connector computed-ON behind a failure card — the command body must
    /// revoke the grant and the ledger so the state returns to fail-closed
    /// "never consented" and the next connect retries the full flow. Both
    /// production bodies are driven: the boot refresh (its backfill grant)
    /// and the apply command.
    #[test]
    fn failed_materialization_revokes_a_fresh_connect_grant() {
        use crate::features::marketplace::{ConnectorScope, load_disabled_bundles_for};

        crate::platform::test_support::with_temp_home("pinvou3-skillgate-revoke", || {
            // Fresh home on purpose: with plain UNINITIALIZED, the revoked
            // grant is observable as the DenyAll expansion denying the id
            // again (an initialized rowless scope is enabled-by-absence by
            // its own stored-list truth, which the revoke does not touch).
            let gate = Box::leak(Box::new(ConnectorGate {
                id: "feishu",
                disabled_filename: "feishu_disabled",
                display_name: "Feishu",
                ready_probe: || true,
                apply_bundle_skills: |_| {
                    Err(std::io::Error::other("injected materialization failure"))
                },
            }));

            // Boot refresh: the backfill grant lands (never-consented pair on
            // an uninitialized scope), then the materialization fails — the
            // grant must be revoked.
            let err = gate.refresh_step().unwrap_err();
            assert!(
                err.contains("刷新") && err.contains("injected materialization failure"),
                "the refresh must surface the materialization failure: {err}"
            );
            assert!(
                load_disabled_bundles_for(ConnectorScope::Plain).contains(&"feishu".to_string()),
                "the revoked grant must return the state to fail-closed: the expansion denies again"
            );
            let raw: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(
                    crate::platform::paths::pinvou3_home().join("disabled_bundles.json"),
                )
                .unwrap(),
            )
            .unwrap();
            assert!(
                raw["connect_granted"]
                    .as_array()
                    .is_none_or(|ids| ids.iter().all(|id| id != "feishu"))
                    && !raw["install_default_synced"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|entry| entry.as_str().is_some_and(|s| s.ends_with(":feishu"))),
                "the revoked boot backfill must leave neither grant nor ledger: {raw}"
            );

            // Connect command: same compensation on the gesture path.
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let err = rt.block_on(gate.apply_skills_command()).unwrap_err();
            assert!(
                err.contains("injected materialization failure"),
                "the connect must surface the materialization failure: {err}"
            );
            assert!(
                load_disabled_bundles_for(ConnectorScope::Plain).contains(&"feishu".to_string()),
                "the revoked connect grant must leave the connector denied"
            );
            let raw: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(
                    crate::platform::paths::pinvou3_home().join("disabled_bundles.json"),
                )
                .unwrap(),
            )
            .unwrap();
            assert!(
                raw["connect_granted"]
                    .as_array()
                    .is_none_or(|ids| ids.iter().all(|id| id != "feishu"))
                    && !raw["install_default_synced"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|entry| entry.as_str().is_some_and(|s| s.ends_with(":feishu"))),
                "the revoked connect grant must leave neither grant nor ledger: {raw}"
            );
        });
    }

    /// Round-33 MAJOR 2 (review #455), dual-marker form (feat/connector-
    /// connect-grants-enable): the connect channel's consent-persist failure
    /// copy carries the ONE shared connect marker
    /// (`scope::CONNECT_CONSENT_SYNC_FAILURE_MARKER`) — the localized template
    /// on the store cards keys on exactly this string, so a rewording here
    /// must move the frontend matcher in the same commit. The install
    /// channels keep `CONSENT_SYNC_FAILURE_MARKER` (their failure direction is
    /// unchanged). The literal is asserted (not reformatted and re-matched) so
    /// a marker rename fails here instead of silently degrading the frontend
    /// guidance to generic copy.
    #[test]
    fn skill_gate_consent_failure_message_keeps_the_frontend_marker() {
        assert_eq!(
            crate::features::marketplace::scope::CONNECT_CONSENT_SYNC_FAILURE_MARKER,
            "persisting the connector's connect-enabled consent failed",
            "the shipped connect marker is the string the frontend consentFailure matcher keys on"
        );
        assert_eq!(
            crate::features::marketplace::scope::CONSENT_SYNC_FAILURE_MARKER,
            "persisting their default-off consent state failed",
            "the install channels keep the original marker"
        );
    }

    /// Round-37 C1 (review #455), grant form (feat/connector-connect-grants-
    /// enable): a consent persist failure between connect and enable left the
    /// pair unledgered, so the startup refresh reverted the user's enable
    /// across the restart (round-30 family residue). The explicit-enable path
    /// now RECORDS the ledger pair, closing the window: connect-grant persist
    /// fails → nothing lands, the connector stays off (fail-closed) → the
    /// user's enable succeeds → refresh must not re-grant over it.
    #[test]
    fn explicit_enable_records_the_ledger_over_a_failed_connect_sync() {
        use crate::features::marketplace::scope::{enable_packages_in_scope, grant_connect_enable};
        use crate::features::marketplace::{ConnectorScope, load_disabled_bundles_for};
        use crate::platform::test_support::with_temp_home;

        with_temp_home("pinvou3-skillgate-ledger-failedconnect", || {
            // Pre-warm: the first read persists the freeze verdict, settling
            // that write so the failpoint below hits the GRANT's save.
            let _ = crate::features::marketplace::scope::load_disabled_bundles_for(
                ConnectorScope::Plain,
            );
            // The connect-path grant's persist FAILS: neither the grant nor
            // the ledger entry reaches disk (and the deny-first caller
            // materializes nothing — fail-closed).
            let _failpoint =
                crate::features::marketplace::scope::fail_next_disabled_bundles_write_for_test();
            assert!(
                grant_connect_enable("feishu").is_err(),
                "fixture: the connect grant must fail on the injected persist error"
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

            // The enable must have RECORDED THE LEDGER — pinned on the raw
            // store, because in the grant world a re-grant over an
            // already-enabled connector is stored-list-invisible and the
            // final refresh assert below cannot see the ledger write (the
            // defanged-pin finding).
            {
                let raw: serde_json::Value = serde_json::from_str(
                    &std::fs::read_to_string(
                        crate::platform::paths::pinvou3_home().join("disabled_bundles.json"),
                    )
                    .unwrap(),
                )
                .unwrap();
                let ledger = raw["install_default_synced"].as_array().unwrap();
                assert!(
                    ledger.contains(&serde_json::json!("plain:feishu")),
                    "the enable must record the ledger pair for the scope it enabled (the any-mode gate needs one pair to stay suppressed), or the next boot re-grants over it: {raw}"
                );
            }

            // The startup refresh must stay gated by the ledger the ENABLE
            // wrote — no re-grant over the user's state.
            let gate = connected_gate("feishu");
            gate.refresh_step().unwrap();
            assert!(
                !load_disabled_bundles_for(ConnectorScope::Plain).contains(&"feishu".to_string()),
                "the enable must record the ledger, or the next boot re-grants over it (round-30 family residue)"
            );
        });
    }

    /// Round-32 MAJOR 1 (review #455) twin negative control, grant form
    /// (feat/connector-connect-grants-enable): the connect happens while plain
    /// is STILL UNINITIALIZED (fresh home) — under the grant the connector is
    /// ENABLED immediately (the DenyAll expansion subtracts `connect_granted`),
    /// and it must still be enabled after the user's first composer write
    /// materializes the scope and the startup refresh runs. This fails on the
    /// round-31 form in BOTH directions: a default-off registration would deny
    /// the connector up front, and an ungated refresh would backfill over the
    /// recorded grant. The uninitialized arm's ledger record is what keeps the
    /// post-materialization refresh from re-denying.
    #[test]
    fn enable_after_uninitialized_connect_survives_startup_refresh() {
        use crate::features::marketplace::scope::{enable_packages_in_scope, grant_connect_enable};
        use crate::features::marketplace::{ConnectorScope, load_disabled_bundles_for};
        use crate::platform::test_support::with_temp_home;

        with_temp_home("pinvou3-skillgate-ledger-uninit", || {
            // Fresh home: plain does not exist yet. The connector connects
            // FIRST (the connect-path grant — a user action) while plain is
            // uninitialized by design.
            grant_connect_enable("feishu").unwrap();

            // THE fix: the connector is enabled in the computed default right
            // away — the user does not need to find any toggle before their
            // first lark call works.
            assert!(
                !load_disabled_bundles_for(ConnectorScope::Plain).contains(&"feishu".to_string()),
                "the connect grant must enable the connector in the uninitialized scope's DenyAll expansion"
            );

            // The ledger pair must be recorded by that same uninitialized
            // grant — pinned on the raw store, because a re-grant over an
            // already-enabled connector is stored-list-invisible: without
            // this leg, deleting the ledger write kept every later assertion
            // green (the defanged-pin finding).
            {
                let raw: serde_json::Value = serde_json::from_str(
                    &std::fs::read_to_string(
                        crate::platform::paths::pinvou3_home().join("disabled_bundles.json"),
                    )
                    .unwrap(),
                )
                .unwrap();
                let ledger = raw["install_default_synced"].as_array().unwrap();
                assert!(
                    ledger.contains(&serde_json::json!("plain:feishu"))
                        && ledger.contains(&serde_json::json!("code:feishu")),
                    "the uninitialized connect grant must record both scope ledger pairs: {raw}"
                );
            }

            // The user's first composer write materializes the scope (the
            // whole effective list, where feishu renders ON and wecom is the
            // pack the user actually opted into): the stored snapshot excludes
            // feishu both because the grant subtracted it from the expansion
            // and because the composer did not list it.
            let outcome =
                enable_packages_in_scope(ConnectorScope::Plain, &["wecom".to_string()]).unwrap();
            assert!(
                outcome.state_changed,
                "fixture: the first write must materialize the scope"
            );

            // Restart equivalent: the startup refresh runs for the connected
            // gate — the ledger entry recorded by the uninitialized connect
            // grant must keep the connector enabled.
            let gate = connected_gate("feishu");
            gate.refresh_step().unwrap();
            assert!(
                !load_disabled_bundles_for(ConnectorScope::Plain).contains(&"feishu".to_string()),
                "the startup refresh must not re-deny a connector granted on an uninitialized scope (round-32 MAJOR 1)"
            );
        });
    }

    /// Round-32 minor 2 (review #455), grant form (feat/connector-connect-
    /// grants-enable): the refresh's consent leg itself is
    /// load-bearing and had no pin — a regression turning the refresh into a
    /// no-op kept the suite green while silently disabling the round-30 m1
    /// backfill. The refresh must GRANT the connector (enabled, no default-off
    /// row) for a pair no consent write ever recorded, AND record the ledger
    /// entry while doing it, so a user toggle-off right after the backfill
    /// survives the next refresh (the recording is what keeps the round-31
    /// BLOCKER re-opened shut).
    #[test]
    fn refresh_backfills_never_synced_row_and_records_the_ledger() {
        use crate::features::marketplace::{
            ConnectorScope, load_disabled_bundles_for, save_disabled_bundles_for,
        };
        use crate::platform::test_support::with_temp_home;

        with_temp_home("pinvou3-skillgate-ledger-backfill", || {
            // Initialized plain (post-migration shape), connector-x never
            // seen by any consent write: no stored row, no ledger entry, no
            // grant.
            seed_initialized_plain_scope();

            let gate = connected_gate("connector-x");
            gate.refresh_step().unwrap();
            assert!(
                !load_disabled_bundles_for(ConnectorScope::Plain)
                    .contains(&"connector-x".to_string()),
                "the startup refresh must GRANT the never-synced connector (enabled, not default-off)"
            );
            {
                let raw: serde_json::Value = serde_json::from_str(
                    &std::fs::read_to_string(
                        crate::platform::paths::pinvou3_home().join("disabled_bundles.json"),
                    )
                    .unwrap(),
                )
                .unwrap();
                assert!(
                    raw["connect_granted"]
                        .as_array()
                        .map(|ids| ids.iter().any(|id| id == "connector-x"))
                        .unwrap_or(false),
                    "the backfill must record the grant: {raw}"
                );
            }

            // The ledger entry must be recorded by that same refresh: a user
            // toggle-off right after the backfill must survive the next
            // refresh.
            save_disabled_bundles_for(ConnectorScope::Plain, &["connector-x".to_string()]).unwrap();
            gate.refresh_step().unwrap();
            assert!(
                load_disabled_bundles_for(ConnectorScope::Plain)
                    .contains(&"connector-x".to_string()),
                "the backfill must record the ledger, or the next refresh re-grants over the user off (round-31 BLOCKER re-opened)"
            );
        });
    }

    /// Round-32 minor 2 (review #455): the TEARDOWN ledger-clear is what
    /// re-arms the LEDGER-GATED startup refresh. The negative control above
    /// drives its reconnect leg with the un-gated connect variant, so deleting
    /// `remove_bundle_from_disabled_scopes_exact`'s ledger clear failed
    /// nothing. This pin drives the gated refresh after teardown: the GRANT
    /// must come back (feat/connector-connect-grants-enable — enabled, no
    /// default-off row), which only happens when the clear actually removed
    /// the entry.
    #[test]
    fn teardown_ledger_clear_re_arms_the_gated_startup_refresh() {
        use crate::features::marketplace::scope::{
            grant_connect_enable, remove_bundle_from_disabled_scopes_exact,
        };
        use crate::features::marketplace::{
            ConnectorScope, load_disabled_bundles_for, save_disabled_bundles_for,
        };
        use crate::platform::test_support::with_temp_home;

        with_temp_home("pinvou3-skillgate-ledger-teardown", || {
            seed_initialized_plain_scope();
            let path = crate::platform::paths::pinvou3_home().join("disabled_bundles.json");

            // Connect grant: the grant and the ledger entry land.
            grant_connect_enable("connector-x").unwrap();

            // User toggle-off: the row lands, the grant drops, the ledger
            // entry must survive.
            save_disabled_bundles_for(ConnectorScope::Plain, &["connector-x".to_string()]).unwrap();
            let raw: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert!(
                raw["install_default_synced"]
                    .as_array()
                    .map(|entries| entries.iter().any(|entry| entry == "plain:connector-x"))
                    .unwrap_or(false),
                "fixture: the toggle-off must keep the ledger entry"
            );
            assert!(
                raw["connect_granted"]
                    .as_array()
                    .map(|ids| !ids.iter().any(|id| id == "connector-x"))
                    .unwrap_or(true),
                "fixture: the toggle-off must drop the grant"
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
            // …and the connect grant: a leftover grant would pre-enable a
            // future same-id reinstall's DenyAll expansion before its connect
            // flow re-grants (the sweep had no pin — deleting it kept every
            // test green).
            assert!(
                raw["connect_granted"]
                    .as_array()
                    .map(|ids| ids.iter().all(|id| id != "connector-x"))
                    .unwrap_or(false),
                "the teardown must sweep the connect grant: {raw}"
            );

            // …which re-arms the GATED startup refresh for the same pack (a
            // fresh install seen at boot before the connect grant runs): the
            // connector comes back ENABLED via the grant, not default-off.
            let gate = connected_gate("connector-x");
            gate.refresh_step().unwrap();
            assert!(
                !load_disabled_bundles_for(ConnectorScope::Plain)
                    .contains(&"connector-x".to_string()),
                "after teardown the startup refresh must re-grant the connector enabled"
            );
        });
    }
}

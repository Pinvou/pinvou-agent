//! 会话分叉：把源会话截断到第 `keep_turns` 个用户 turn 末尾，克隆成全新
//! session id 落盘（无损 what-if 分支）。与 [`super::rewind`] 同级：rewind
//! 原地截断并备份被截段，fork 不动源会话、另存新支。
//!
//! 切点口径与 rewind 同源：底座 [`deepseek_tui::user_turn_prompt_indices`]
//! 的 era 口径（比单消息 [`deepseek_tui::is_user_turn_prompt`] 多计入
//! post-provenance 历史里粘贴的压缩摘要头 turn——底座文档明确 host 编号应
//! 统一读这一个列表）。前端入口表（forkEntriesByTurnId）按
//! `turnHasUserText` + 引擎 `<context_update>` 运行时消息的 per-session
//! 计数（bridge 层在投递/重载两路把计数附加到下一个用户气泡）对齐同一
//! 列表。已知残留（前端比后端少计一格、序号向后错位）：post-provenance
//! 历史里用户粘贴的压缩摘要头 turn，后端 era 会计入而前端跳过。根治是
//! 把序号由后端随消息下发，前端不再本地计数——见 forkTurnEntries.js 注释。
//!
//! 刻意不随克隆走的内容：artifacts（实体存放在源会话 artifacts 目录，复制
//! 记录会形成跨会话指针）、journal/leaf 兼容投影（messages 是非 journal
//! 会话的规范载体，陈旧 journal 会在加载时复活被截掉的消息）、成本快照与
//! token/时长累计（新分支尚未产生消耗，保留会虚报）。
//! 随克隆保留但需要知晓的字段：审批凭据（crate 私有字段无法清除；审批历史
//! 属于对话内容，分叉分支继承它符合语义）、`work_state`（todos/plan 侧栏
//! 状态）与 `context_references`（后者的 message_index 可能指向被截掉的
//! 消息，消费方按索引越界如实降级）、`archived`/`spawn_depth`（UI 不可达
//! archived 会话分叉；仅 IPC 直调会得到同样 archived 的分支）、源会话标题，
//! 记录内的 `model`/`model_provider`/`mode` 快照字段（只是元数据陈述，桌面端
//! 每会话模型覆盖与 mode_state 是按 id 键存的侧车，新分支以默认值注水）、
//! 以及同样按 id 键存的 `last_auto_route`（crate 私有，与审批凭据同理由）与
//! `window_title`/`system_prompt`（后者可能携带旧版压缩摘要残留，分叉静默
//! 保留——rewind 会向前端暴露 had_compaction 警示，fork 暂不区分）。
//! 按 id 键存、因此**主动复刻**的状态：用户工作目录绑定 sidecar
//! （`workspace-binding.json`）——引擎 cwd、shell 目录、AGENTS.md/项目技能
//! 注入与 mode_state 的绑定派生默认全部按解析到的绑定走，分支不复刻就会
//! 静默降级成无绑定会话（绑定会话的默认姿态偏 Plan、无绑定偏 Yolo；用户
//! 显式设过的会话级 mode 按 id 键存、不随分支走，分支以默认值注水——与
//! 新建会话一致）。复刻含 keychain 全三元组：绑定路径、workspace roots
//! 与显式祖先 provenance（`explicit_ancestor_roots`，round-40 zhuowp P2
//! 的对齐产物——交付用它让祖先授权跨越项目变更存活，丢 provenance 的
//! 复刻会让分支在项目变更后静默失去祖先授权，r7 minor 7）。fork 命令持有
//! projects rebind fence（sibling-writer 契约，review #484 round-11 M3）：
//! 绑定源探测/复刻与 rebind 的候选快照-翻译窗口互斥，分支 sidecar 不会带着
//! 改绑前的旧路径逃出翻译。绑定源 sidecar 损坏/版本超前时三态探针如实拒绝
//! （而非当无绑定放行）。代码会话（原生品悟 Engine 车道）的绑定在代码会话
//! 侧车而非本 store，无法照搬，分叉对其如实拒绝。新会话引擎按既有 lazy
//! respawn 路径在首次发送时接管，无需在此触碰 pool。
//!
//! 注意：persist 落盘后附带保留预算清扫（`persist_then_reconcile`），若
//! store 超预算，最旧的记录——可能是刚分叉的源会话——会被清扫驱逐；与任何
//! 新建会话同语义，非 fork 特有，但「从某会话分叉可能清掉它」值得知晓。

use anyhow::Result;
use chrono::Utc;
use deepseek_tui::session_manager::SessionMetadata;
use deepseek_tui::user_turn_prompt_indices;

use super::SessionStore;

impl SessionStore {
    /// 从第 `keep_turns` 轮末尾分叉出新会话，返回新会话的 metadata。
    /// `keep_turns == 当前总轮数`（总轮数 > 0）即全量分支（切点即末尾）；
    /// `keep_turns == 0` 得到仅携带源会话前导（实践中为空）的空分支，与
    /// rewind 的 N=0 口径一致；0 轮会话无可分叉内容，如实报错（rewind 对
    /// 0 轮同样拒绝）；超过当前轮数如实报错。
    pub(crate) fn fork_session_record(
        &self,
        session_id: &str,
        keep_turns: u32,
    ) -> Result<SessionMetadata> {
        if self.is_scheduled_session(session_id)? {
            anyhow::bail!("定时任务运行会话不支持分叉");
        }
        // aux 是主会话的受限只读旁路（ADR-0024），分叉语义未定义，如实拒绝。
        if super::validators::is_aux_session_id(session_id) {
            anyhow::bail!("辅助对话不支持分叉");
        }
        // 原生代码会话的项目绑定/工作树状态按 id 键存于代码会话侧车（不在
        // 本 store），无法随分支复刻；分叉出的分支会以无绑定姿态落到空的
        // 私有执行根，且 mode_state 默认从 Plan 翻成 Yolo——如实拒绝，分叉
        // 语义只定义在 chat 会话上。
        if self.is_code_session(session_id) {
            anyhow::bail!("原生代码会话不支持分叉：项目绑定与工作树状态不随分支走");
        }
        // 绑定源三态探测（先于 load/落盘 fail fast，见 workspace_bindings 的
        // 探针文档）：Unbound 分支保持无绑定；Unknown（sidecar 损坏/不可读/
        // 新版本）必须拒绝——「读失败当无绑定放行」会让分支静默失去绑定，
        // mode_state 安全姿态随之降级，恰是绑定复刻要消灭的形态。绑定与
        // roots 在锁外一次读取：fork 命令持有 projects rebind fence（见
        // sessions.rs fork_session），rebind 无法在读取与落盘之间改写绑定。
        let (parent_workspace, parent_roots, parent_ancestor_roots) = match self
            .session_workspace_binding_probe(session_id)
        {
            // 绑定/roots/provenance 来自同一份 sidecar 快照（单次读取）：warm
            // 缓存下 sidecar 损坏/不可读走 Unknown fail-closed，不再经
            // `session_workspace_roots` 的 `unwrap_or_default` 折叠成静默空
            // roots（review r2）。
            super::workspace_bindings::WorkspaceBindingProbe::Bound(
                path,
                roots,
                ancestor_roots,
            ) => (Some(path), roots, ancestor_roots),
            super::workspace_bindings::WorkspaceBindingProbe::Unknown => anyhow::bail!(
                "无法读取源会话的工作区绑定（文件损坏或由更新版本写入）；为避免分支静默降级为无绑定会话，已拒绝分叉"
            ),
            super::workspace_bindings::WorkspaceBindingProbe::Unbound => {
                (None, Vec::new(), Vec::new())
            }
        };
        // load→persist 对与 delete/驱逐串行（与 truncate_to_user_turn 同一
        // 口径，delete 的文档写明该锁正是为此存在）：没有它，源会话在 load
        // 与落盘之间被删除/驱逐会留下指向已不存在记录的血缘。锁不可重入
        // （parking_lot），persist 直接走 `persist_then_reconcile` 而非 `save`。
        let mutation = self.scheduled_mutation.lock();
        let source = self.load(session_id)?;
        let turn_prompt_indices = user_turn_prompt_indices(&source.messages);
        let total_turns = turn_prompt_indices.len();
        let keep = keep_turns as usize;
        if total_turns == 0 {
            anyhow::bail!("会话还没有可分叉的用户轮次（共 0 轮）");
        }
        if keep > total_turns {
            anyhow::bail!("会话当前只有 {total_turns} 轮，无法分叉到第 {keep_turns} 轮末尾");
        }
        // keep == total_turns：全量分支（切点即末尾）。
        let cut = if keep == total_turns {
            source.messages.len()
        } else {
            turn_prompt_indices[keep]
        };

        // 血缘数据在 move 前捕获，整条记录按值接管：整份深拷贝只为了改几
        // 个字段再截断不值得（大会话上 clone 让峰值内存近乎翻倍），克隆
        // 语义（源不动、新支另存）由「先捕获后改写、只写新 id 文件」保持。
        let parent_metadata = source.metadata.clone();
        let captured_message_count = source.messages.len();
        let mut forked = source;
        forked.metadata.id = super::validators::generate_session_id();
        forked.metadata.mark_forked_from(&parent_metadata);
        // 血缘计数取实际捕获的消息数：metadata.message_count 由引擎路径维护，
        // 直存/外部改写的记录可能滞后，以 messages 为准。
        forked.metadata.forked_from_message_count = Some(captured_message_count);
        forked.metadata.created_at = Utc::now();
        forked.metadata.updated_at = Utc::now();
        forked.messages.truncate(cut);
        forked.metadata.message_count = forked.messages.len();
        // 新分支尚未产生消耗：成本/token/时长归零（血缘里已记录来源）。
        forked.metadata.cost = Default::default();
        forked.metadata.total_tokens = 0;
        forked.metadata.cumulative_turn_secs = 0;
        forked.artifacts = Vec::new();
        forked.journal = None;
        forked.leaf_id = None;
        self.persist_then_reconcile(&forked, "session fork")?;
        // 绑定复刻在锁释放后进行：`bind_session_workspace_with_roots` 只写
        // 本会话自己的 sidecar，不需要与 delete 互斥；回滚用的 `delete`
        // 会再取锁，持锁调用会死锁。绑定/roots 取自锁外探测（fork 命令的
        // projects rebind fence 保证读取与落盘之间无 rebind 改写）；源会话
        // 在落盘与绑定之间被删的残余窗口只影响绑定探针的读取时机，不产生
        // 孤儿数据。
        drop(mutation);
        if let Some(parent_workspace) = parent_workspace {
            let bind_result = self
                .bind_session_workspace_with_roots(
                    &forked.metadata.id,
                    parent_workspace,
                    parent_roots.clone(),
                )
                .and_then(|()| {
                    // 显式祖先 provenance 复刻（绑定写完即刻回填，`Ok(false)`
                    // 只在 sidecar 缺失时出现——上一行刚写过，不可达，按失败
                    // 处理保持全有或全无）。回填失败的分支同样按绑定失败口径
                    // 回滚：带一半 keychain（有 roots 无 provenance）的分支在
                    // 对齐项目变更后会静默失去祖先授权，恰是复刻要消灭的形态。
                    if parent_ancestor_roots.is_empty() {
                        return Ok(());
                    }
                    self.set_session_workspace_roots_with_explicit_ancestors(
                        &forked.metadata.id,
                        parent_roots.clone(),
                        parent_ancestor_roots.clone(),
                    )
                    .and_then(|written| {
                        if written {
                            Ok(())
                        } else {
                            anyhow::bail!("restored sidecar missing after bind")
                        }
                    })
                });
            if let Err(error) = bind_result {
                // 绑定失败的分支正是本复刻要消灭的「无绑定降级」形态，
                // 按新建会话的绑定失败回滚口径（create_session_durable）
                // 删除刚落盘的分支。
                let rollback = self.delete(&forked.metadata.id);
                return Err(match rollback {
                    Ok(()) => anyhow::anyhow!("bind workspace: {error:#}"),
                    Err(rollback_error) => anyhow::anyhow!(
                        "bind workspace: {error:#}; rollback Session {}: {rollback_error:#}",
                        forked.metadata.id
                    ),
                });
            }
        }
        Ok(forked.metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::SessionStore;
    use deepseek_tui::models::{ContentBlock, Message, Role};
    use deepseek_tui::session_tree::SessionJournal;

    /// 与 features::sessions::tests::isolated_store 同一隔离模式
    /// （test_support::locked_env 串行化 PINVOU3_HOME；守卫持锁到测试结束）。
    fn isolated_store() -> (
        SessionStore,
        std::sync::MutexGuard<'static, ()>,
        crate::platform::test_support::EnvRestore,
    ) {
        let (lock, env) = crate::platform::test_support::locked_env(&["PINVOU3_HOME"]);
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-fork-test-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        // SAFETY: test_support::locked_env 持锁；env 写入进程内串行。
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        (
            SessionStore::boot_with_scheduled_root(tmp.join("scheduled")).expect("boot"),
            lock,
            env,
        )
    }

    fn turn_prompt(text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    fn assistant_text(text: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    #[test]
    fn fork_truncates_at_turn_boundary_and_records_lineage() {
        let (store, _lock, _env) = isolated_store();
        let source = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let mut session = store.load(&source.metadata.id).expect("load");
        session.metadata.title = "源会话".into();
        // 3 个用户 turn（prompt/assistant 对 + 尾部 prompt）。
        session.messages = vec![
            turn_prompt("第一轮"),
            assistant_text("回复一"),
            turn_prompt("第二轮"),
            assistant_text("回复二"),
            turn_prompt("第三轮"),
        ];
        store.save(&session).expect("seed messages");

        let forked = store
            .fork_session_record(&source.metadata.id, 2)
            .expect("fork");
        assert_ne!(forked.id, source.metadata.id);
        assert_eq!(
            forked.parent_session_id.as_deref(),
            Some(source.metadata.id.as_str()),
            "lineage must point at the source session"
        );
        assert_eq!(forked.forked_from_message_count, Some(5));
        assert_eq!(forked.message_count, 4, "kept turns 1-2 inclusive");
        assert_eq!(forked.title, "源会话", "fork keeps the sibling title");

        let reloaded = store.load(&forked.id).expect("load forked");
        assert_eq!(reloaded.messages.len(), 4);
        assert_eq!(
            reloaded.messages[3].role,
            Role::Assistant,
            "cut lands after the kept turn's reply"
        );

        let original = store.load(&source.metadata.id).expect("reload source");
        assert_eq!(
            original.messages.len(),
            5,
            "fork must not mutate the source session"
        );
    }

    #[test]
    fn fork_rejects_out_of_range_turn() {
        let (store, _lock, _env) = isolated_store();
        let source = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let mut session = store.load(&source.metadata.id).expect("load");
        session.messages = vec![
            turn_prompt("第一轮"),
            assistant_text("回复一"),
            turn_prompt("第二轮"),
        ];
        store.save(&session).expect("seed");
        // 有轮会话上 keep > total 走专属拒绝腿（与 0 轮拒绝分支不同：这里
        // 的 total_turns > 0，keep == total 是合法全量分支，仅超出才拒绝）。
        let error = store
            .fork_session_record(&source.metadata.id, 3)
            .expect_err("2-turn session cannot fork past its end");
        assert!(
            error.to_string().contains("只有 2 轮"),
            "must name the actual turn count: {error:#}"
        );
    }

    /// 0 轮会话（无 era 可计用户 turn）无可分叉内容：keep=0 与 keep>0 一律
    /// 如实拒绝，不得触发 keep==total 的全量分支腿把整条记录当「空分支」。
    #[test]
    fn fork_rejects_zero_turn_session() {
        let (store, _lock, _env) = isolated_store();
        let source = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let mut session = store.load(&source.metadata.id).expect("load");
        session.messages = vec![assistant_text("只有助手消息，没有用户 turn")];
        store.save(&session).expect("seed");
        for keep in [0u32, 1] {
            let error = store
                .fork_session_record(&source.metadata.id, keep)
                .expect_err("0-turn session must refuse fork");
            assert!(error.to_string().contains("0 轮"), "keep={keep}: {error:#}");
        }
        // 源会话不被半成品污染。
        let original = store.load(&source.metadata.id).expect("reload");
        assert_eq!(original.messages.len(), 1);
    }

    #[test]
    fn fork_rejects_scheduled_and_aux_sessions() {
        let (store, _lock, _env) = isolated_store();
        let source = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");

        // scheduled：落一条 sched- 记录（is_scheduled_session 的前缀 + 文件
        // 存在两条腿都命中）后 fork 必须拒绝。
        let mut scheduled = store.load(&source.metadata.id).expect("load");
        scheduled.metadata.id = "sched-forktest1".into();
        store.save(&scheduled).expect("seed scheduled record");
        let error = store
            .fork_session_record("sched-forktest1", 1)
            .expect_err("scheduled-run sessions must refuse fork");
        assert!(error.to_string().contains("定时任务"), "{error:#}");

        // aux：只看 id 前缀，无需落盘记录。
        let error = store
            .fork_session_record("aux-forktest1", 1)
            .expect_err("aux sessions must refuse fork");
        assert!(error.to_string().contains("辅助对话"), "{error:#}");
    }

    #[test]
    fn fork_drops_journal_and_resets_spend_from_nonzero_source() {
        let (store, _lock, _env) = isolated_store();
        let source = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let mut session = store.load(&source.metadata.id).expect("load");
        session.messages = vec![turn_prompt("带消耗的一轮"), assistant_text("回复")];
        // 非零消耗源 + journal/leaf 投影：fork 必须归零/丢弃（陈旧 journal
        // 会在加载时复活被截掉的消息，见模块文档）。
        session.metadata.total_tokens = 1234;
        session.metadata.cumulative_turn_secs = 96;
        session.metadata.cost.session_cost_usd = 1.25;
        session.metadata.cost.session_cost_cny = 9.0;
        session.metadata.cost.subagent_cost_usd = 0.5;
        session.metadata.cost.subagent_cost_cny = 3.5;
        // 非钱累积字段也须归零：手工逐字段重置的回归（只重置四个钱项）必须
        // 被下面的整快照 JSON 相等断言抓到。
        session.metadata.cost.displayed_cost_high_water_usd = 8.8;
        let mut journal = SessionJournal::new();
        journal.append_message(turn_prompt("journal 载体"));
        session.journal = Some(journal);
        session.leaf_id = Some("leaf-probe".into());
        store.save(&session).expect("seed");

        let forked = store
            .fork_session_record(&source.metadata.id, 1)
            .expect("fork");
        assert_eq!(forked.total_tokens, 0, "fresh branch has not spent tokens");
        assert_eq!(
            forked.cumulative_turn_secs, 0,
            "fresh branch has no turn time"
        );
        // SessionCostSnapshot 无 PartialEq：逐字段断言四个成本项全部归零。
        assert_eq!(forked.cost.session_cost_usd, 0.0, "usd must reset");
        assert_eq!(forked.cost.session_cost_cny, 0.0, "cny must reset");
        assert_eq!(
            forked.cost.subagent_cost_usd, 0.0,
            "subagent usd must reset"
        );
        assert_eq!(
            forked.cost.subagent_cost_cny, 0.0,
            "subagent cny must reset"
        );
        // 整快照断言：cost 的全部字段（高水位、计价覆盖、路线凭据等）都取
        // Default 值——逐字段补断言追不上快照的字段增长，序列化相等才是
        // 完整口径。
        let default_cost = deepseek_tui::session_manager::SessionCostSnapshot::default();
        assert_eq!(
            serde_json::to_value(&forked.cost).expect("serialize forked cost"),
            serde_json::to_value(&default_cost).expect("serialize default cost"),
            "every cost snapshot field must reset to the default"
        );
        // 落盘记录不得携带源会话的 journal/leaf（load 时的 ensure_journal 会
        // 从 messages 重建投影，内存断言探不到磁盘上的陈旧投影——陈旧投影
        // 会在加载时用 journal 覆盖 messages，复活被截掉的轮次）。
        let record_bytes =
            std::fs::read_to_string(store.sessions_dir().join(format!("{}.json", forked.id)))
                .expect("read forked record");
        let record: serde_json::Value = serde_json::from_str(&record_bytes).expect("parse record");
        assert!(
            record["journal"].is_null(),
            "stale journal must not ride the clone: {}",
            record["journal"]
        );
        assert!(
            record["leaf_id"].is_null(),
            "stale leaf must not ride the clone"
        );

        let original = store.load(&source.metadata.id).expect("reload source");
        assert!(
            original.journal.is_some(),
            "fork must not mutate the source"
        );
        assert_eq!(original.metadata.total_tokens, 1234);
    }

    /// 工作目录绑定按 id 键存 sidecar，不随记录克隆：分叉必须复刻源绑定，
    /// 否则分支的引擎/shell 落回空的私有执行根，且 mode_state 对无绑定会话
    /// 默认 Yolo（安全姿态静默降级）。无绑定源的分支保持无绑定。
    #[test]
    fn fork_rebinds_workspace_binding_for_branch() {
        let (store, _lock, _env) = isolated_store();
        let source = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let mut session = store.load(&source.metadata.id).expect("load");
        session.messages = vec![turn_prompt("绑定会话的一轮"), assistant_text("回复")];
        store.save(&session).expect("seed");
        let workspace = std::env::temp_dir().join(format!(
            "pinvou3-fork-bind-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        std::fs::create_dir_all(&workspace).expect("mkdir workspace");
        store
            .bind_session_workspace_with_roots(&source.metadata.id, workspace.clone(), Vec::new())
            .expect("bind source");

        let forked = store
            .fork_session_record(&source.metadata.id, 1)
            .expect("fork");
        assert_eq!(
            store.session_workspace_binding(&forked.id).as_deref(),
            Some(workspace.as_path()),
            "branch must carry the source's workspace binding"
        );
        assert_eq!(
            store.session_workspace_roots(&forked.id),
            store.session_workspace_roots(&source.metadata.id),
            "branch must carry the source's workspace roots"
        );
        // 源会话绑定不受影响。
        assert_eq!(
            store
                .session_workspace_binding(&source.metadata.id)
                .as_deref(),
            Some(workspace.as_path())
        );

        // 无绑定源的分支保持无绑定（不误造绑定）。
        let plain = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create plain");
        let mut plain_session = store.load(&plain.metadata.id).expect("load");
        plain_session.messages = vec![turn_prompt("无绑定会话的一轮")];
        store.save(&plain_session).expect("seed");
        let forked_plain = store
            .fork_session_record(&plain.metadata.id, 1)
            .expect("fork");
        assert!(
            store.session_workspace_binding(&forked_plain.id).is_none(),
            "unbound source must not produce a bound branch"
        );
    }

    /// 原生代码会话的绑定/工作树按 id 键存于代码会话侧车（codex_acp store，
    /// 生产判定经 lib.rs 注入的 predicate 读它），无法随分支复刻：如实拒绝，
    /// 而不是产出静默降级的无绑定分支。测试走同一注入面
    /// （`set_code_session_predicate`）置源会话为代码会话。
    #[test]
    fn fork_rejects_code_sessions() {
        let (store, _lock, _env) = isolated_store();
        let source = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let mut session = store.load(&source.metadata.id).expect("load");
        session.messages = vec![turn_prompt("代码会话的一轮"), assistant_text("回复")];
        store.save(&session).expect("seed");
        let code_id = source.metadata.id.clone();
        let predicate: crate::features::sessions::CodeSessionPredicate =
            std::sync::Arc::new(move |id: &str| id == code_id);
        store.set_code_session_predicate(predicate);
        let error = store
            .fork_session_record(&source.metadata.id, 1)
            .expect_err("native code sessions must refuse fork");
        assert!(error.to_string().contains("原生代码会话"), "{error:#}");
        // 源会话不被半成品污染。
        let original = store.load(&source.metadata.id).expect("reload");
        assert_eq!(original.messages.len(), 2);
    }

    /// 绑定源三态探测（review 修复）：sidecar 损坏或版本超前时 fork 必须
    /// fail-closed 拒绝——「读失败当无绑定放行」会让分支静默失去绑定，
    /// mode_state 安全姿态随之降级。两条 Unknown 腿（解析失败/版本超前）
    /// 与恢复后的正常分叉都钉住。
    /// warm 缓存 + sidecar 损坏也必须拒绝（review r2）：探针的缓存命中
    /// 只给绑定路径，roots 与绑定必须来自**同一份** sidecar 快照——缓存
    /// 命中后仍回读 sidecar，损坏/不可读在 warm 缓存下同样走 Unknown
    /// fail-closed。否则分支带着正确路径 + 静默空 roots 绑定（正是三态
    /// 探针要堵的 fail-open 在 roots 粒度上的残留）。与上一测试相反，
    /// 本测试**不清缓存**——清了就退化为 cold 腿，钉不住 warm 路径。
    #[test]
    fn fork_probe_refuses_corrupt_sidecar_even_with_warm_cache() {
        let (store, _lock, _env) = isolated_store();
        let source = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let mut session = store.load(&source.metadata.id).expect("load");
        session.messages = vec![turn_prompt("warm 缓存的一轮"), assistant_text("回复")];
        store.save(&session).expect("seed");
        let workspace = std::env::temp_dir().join(format!(
            "pinvou3-fork-warm-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        std::fs::create_dir_all(&workspace).expect("mkdir workspace");
        store
            .bind_session_workspace_with_roots(&source.metadata.id, workspace, Vec::new())
            .expect("bind source warms the cache");
        // 不清缓存：绑定路径已在内存缓存里。
        assert!(
            store
                .session_workspaces
                .read()
                .contains_key(&source.metadata.id),
            "precondition: cache must be warm"
        );
        std::fs::write(
            store.session_workspace_sidecar_path(&source.metadata.id),
            b"{not json",
        )
        .expect("corrupt sidecar");
        // warm 缓存下损坏的 sidecar 仍走 Unknown → fork 拒绝（旧行为是
        // 缓存 Bound + `session_workspace_roots` 的 unwrap_or_default 静默
        // 空 roots 放行）。
        match store.session_workspace_binding_probe(&source.metadata.id) {
            crate::features::sessions::workspace_bindings::WorkspaceBindingProbe::Unknown => {}
            other => panic!("warm cache + corrupt sidecar must be Unknown, got {other:?}"),
        }
        let error = store
            .fork_session_record(&source.metadata.id, 1)
            .expect_err("corrupt sidecar must refuse fork even with warm cache");
        assert!(error.to_string().contains("工作区绑定"), "{error:#}");
        let entries: Vec<_> = std::fs::read_dir(store.sessions_dir())
            .expect("list sessions")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".json"))
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "refused fork must not persist a branch: {entries:?}"
        );
    }

    #[test]
    fn fork_refuses_undecodable_workspace_binding_sidecar() {
        let (store, _lock, _env) = isolated_store();
        let source = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let mut session = store.load(&source.metadata.id).expect("load");
        session.messages = vec![turn_prompt("绑定源的一轮"), assistant_text("回复")];
        store.save(&session).expect("seed");
        let workspace = std::env::temp_dir().join(format!(
            "pinvou3-fork-sidecar-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        std::fs::create_dir_all(&workspace).expect("mkdir workspace");
        store
            .bind_session_workspace_with_roots(&source.metadata.id, workspace, Vec::new())
            .expect("bind source");
        let sidecar = store.session_workspace_sidecar_path(&source.metadata.id);
        // bind 命中内存缓存（缓存只存已绑定路径、探针直接返回 Bound）：
        // 逐腿清缓存让探针真正走到 sidecar 读侧——生产里 Unknown 只在
        // 「无缓存条目且 sidecar 不可判定」（新进程/缓存未回填）时出现。
        store.session_workspaces.write().remove(&source.metadata.id);

        // 腿 1：sidecar 解析失败。
        std::fs::write(&sidecar, b"{not json").expect("corrupt sidecar");
        let error = store
            .fork_session_record(&source.metadata.id, 1)
            .expect_err("corrupt sidecar must refuse fork");
        assert!(
            error.to_string().contains("工作区绑定"),
            "must name the binding probe: {error:#}"
        );

        // 腿 2：版本超前（当前支持 v2）——同样先清缓存走 sidecar 读侧。
        store.session_workspaces.write().remove(&source.metadata.id);
        std::fs::write(
            &sidecar,
            br#"{"version": 999, "path": "/tmp/elsewhere", "workspace_roots": [], "explicit_ancestor_roots": [], "bound_at": null}"#,
        )
        .expect("write future sidecar");
        let error = store
            .fork_session_record(&source.metadata.id, 1)
            .expect_err("future-version sidecar must refuse fork");
        assert!(
            error.to_string().contains("工作区绑定"),
            "must name the binding probe: {error:#}"
        );
        // 分支不得已落盘（拒绝发生在 load/落盘之前）。
        let entries: Vec<_> = std::fs::read_dir(store.sessions_dir())
            .expect("list sessions")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".json"))
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "refused fork must not persist a branch: {entries:?}"
        );

        // 恢复正常 sidecar 后分叉照常（探针不误伤）。
        store
            .bind_session_workspace_with_roots(
                &source.metadata.id,
                std::env::temp_dir().join("pinvou3-fork-rebind-ok"),
                Vec::new(),
            )
            .expect("rebind source");
        let forked = store
            .fork_session_record(&source.metadata.id, 1)
            .expect("fork after repair");
        assert!(
            store.session_workspace_binding(&forked.id).is_some(),
            "branch carries the repaired binding"
        );
    }

    /// keychain provenance 复刻（r7 minor 7）：对齐车道写入的
    /// `explicit_ancestor_roots` 必须随分支复刻——交付用它让祖先授权跨越
    /// 项目变更存活，丢 provenance 的分支会在项目变更后静默失去祖先授权
    /// （round-40 zhuowp P2 要防的同一形态，复刻层不得重新引入）。
    #[test]
    fn fork_replicates_explicit_ancestor_provenance() {
        let (store, _lock, _env) = isolated_store();
        let source = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let mut session = store.load(&source.metadata.id).expect("load");
        session.messages = vec![turn_prompt("对齐源的一轮"), assistant_text("回复")];
        store.save(&session).expect("seed");
        let workspace = std::env::temp_dir().join(format!(
            "pinvou3-fork-ancestors-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        std::fs::create_dir_all(workspace.join("granted")).expect("mkdir workspace");
        std::fs::create_dir_all(workspace.join("nested")).expect("mkdir ancestors");
        store
            .bind_session_workspace_with_roots(
                &source.metadata.id,
                workspace.clone(),
                vec![workspace.join("nested")],
            )
            .expect("bind source");
        store
            .set_session_workspace_roots_with_explicit_ancestors(
                &source.metadata.id,
                vec![workspace.join("nested")],
                vec![workspace.join("nested")],
            )
            .expect("record ancestor provenance");
        let forked = store
            .fork_session_record(&source.metadata.id, 1)
            .expect("fork");
        assert_eq!(
            store.session_explicit_ancestor_roots(&forked.id),
            vec![workspace.join("nested")],
            "branch must carry the source's explicit-ancestor provenance"
        );
        assert_eq!(
            store.session_workspace_roots(&forked.id),
            vec![workspace.clone(), workspace.join("nested")],
            "branch must carry the source's workspace roots (cwd-first normalization) alongside the provenance"
        );
        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// 全量分支腿（`keep_turns == total_turns`，r7 minor 4）：切点即末尾，
    /// 旧实现误索引 `turn_prompt_indices[keep]` 会在这一腿越界 panic——
    /// 用户在最新一轮点分叉是合法动作，必须有行为钉。
    #[test]
    fn fork_full_branch_at_latest_turn_clones_everything() {
        let (store, _lock, _env) = isolated_store();
        let source = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let mut session = store.load(&source.metadata.id).expect("load");
        session.messages = vec![
            turn_prompt("第一轮"),
            assistant_text("答一"),
            turn_prompt("第二轮"),
            assistant_text("答二"),
            turn_prompt("第三轮"),
            assistant_text("答三"),
        ];
        store.save(&session).expect("seed");
        let forked = store
            .fork_session_record(&source.metadata.id, 3)
            .expect("fork at the latest turn");
        assert_eq!(
            forked.message_count, 6,
            "full branch keeps the whole transcript"
        );
        assert_eq!(
            forked.forked_from_message_count,
            Some(6),
            "lineage captures the full source length"
        );
    }
}

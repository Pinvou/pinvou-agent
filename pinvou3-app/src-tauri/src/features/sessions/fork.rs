//! 会话分叉：把源会话截断到第 `keep_turns` 个用户 turn 末尾，克隆成全新
//! session id 落盘（无损 what-if 分支）。与 [`super::rewind`] 同级：rewind
//! 原地截断并备份被截段，fork 不动源会话、另存新支。
//!
//! 切点口径与 rewind 同源：底座 [`deepseek_tui::user_turn_prompt_indices`]
//! 的 era 口径（比单消息 [`deepseek_tui::is_user_turn_prompt`] 多计入历史
//! 里粘贴的压缩摘要头 turn——底座文档明确 host 编号应统一读这一个列表，
//! 前端投影对这类 turn 也计 userItem，两侧因此一致）。
//!
//! 刻意不随克隆走的内容：artifacts（实体存放在源会话 artifacts 目录，复制
//! 记录会形成跨会话指针）、journal/leaf 兼容投影（messages 是非 journal
//! 会话的规范载体，陈旧 journal 会在加载时复活被截掉的消息）、成本快照与
//! token/时长累计（新分支尚未产生消耗，保留会虚报）。
//! 随克隆保留但需要知晓的字段：审批凭据（crate 私有字段无法清除；审批历史
//! 属于对话内容，分叉分支继承它符合语义）、`work_state`（todos/plan 侧栏
//! 状态）与 `context_references`（后者的 message_index 可能指向被截掉的
//! 消息，消费方按索引越界如实降级）、`archived`/`spawn_depth`（UI 不可达
//! archived 会话分叉；仅 IPC 直调会得到同样 archived 的分支）、源会话标题。
//! 每会话侧车（模型覆盖 set_session_model_id、mode_state）按 id 键存，不会
//! 跟随分叉——新分支以默认模型/模式注水。新会话引擎按既有 lazy respawn
//! 路径在首次发送时接管，无需在此触碰 pool。

use anyhow::Result;
use chrono::Utc;
use deepseek_tui::session_manager::SessionMetadata;
use deepseek_tui::user_turn_prompt_indices;

use super::SessionStore;

impl SessionStore {
    /// 从第 `keep_turns` 轮末尾分叉出新会话，返回新会话的 metadata。
    /// `keep_turns == 当前总轮数` 即全量分支（切点即末尾）；`keep_turns == 0`
    /// 得到仅携带源会话前导（实践中为空）的空分支，与 rewind 的 N=0 口径
    /// 一致；超过当前轮数如实报错。
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
        let source = self.load(session_id)?;
        let turn_prompt_indices = user_turn_prompt_indices(&source.messages);
        let total_turns = turn_prompt_indices.len();
        let keep = keep_turns as usize;
        if keep > total_turns {
            anyhow::bail!("会话当前只有 {total_turns} 轮，无法分叉到第 {keep_turns} 轮末尾");
        }
        // keep == total_turns：全量分支（切点即末尾）。
        let cut = if keep == total_turns {
            source.messages.len()
        } else {
            turn_prompt_indices[keep]
        };

        let mut forked = source.clone();
        forked.metadata.id = super::validators::generate_session_id();
        forked.metadata.mark_forked_from(&source.metadata);
        // 血缘计数取实际捕获的消息数：metadata.message_count 由引擎路径维护，
        // 直存/外部改写的记录可能滞后，以 messages 为准。
        forked.metadata.forked_from_message_count = Some(source.messages.len());
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
        self.save(&forked)?;
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
        let error = store
            .fork_session_record(&source.metadata.id, 1)
            .expect_err("0-turn session cannot fork past its end");
        assert!(error.to_string().contains("轮"), "{error:#}");
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
}

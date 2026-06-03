//! 多 session 并发的 engine 池。
//!
//! 旧模型:整个进程一个 Engine,切 session 靠 `Op::SyncSession` 整体替换内部状态
//! → 同一时刻只能服务一个 session,且切走正在跑的 session 会串台。
//!
//! 新模型:**每个 session 一个独立 Engine**(底座 `spawn_engine` 是独立工厂,见
//! [`AppEngine::spawn_for_session`])。本池按 `session_id` 管理这些 engine 的生命周期:
//!  - **lazy spawn**:首次给某 session 发消息时才 spawn(带该 session 专属 workspace +
//!    instructions);已有磁盘历史的 session 在 spawn 后用一次性 `SyncSession` 注水。
//!  - **keep-alive**:spawn 后常驻,切 session 不销毁(后台 session 继续跑各自的 turn)。
//!  - **evict**:删 session 时回收(cancel 在跑的 turn + Shutdown engine + abort forwarder)。
//!
//! 池本身是 Tauri State;`commands.rs` 里的 chat / cancel / submit_user_input 等都带
//! `session_id` 路由到对应 engine。
//!
//! 并发说明:`entries` 用 `tokio::Mutex`,`get_or_spawn` 全程持锁(spawn 很快,只建
//! channel + spawn task,无网络),从根上避免「同 session 并发 spawn 两个 engine」的
//! TOCTOU。不同 session 的发送只在各自首次 spawn 的瞬间串行,spawn 完即各自并发跑。

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use deepseek_tui::core::ops::Op;
use deepseek_tui::tools::user_input::UserInputResponse;
use deepseek_tui::tui::app::AppMode;
use tauri::async_runtime::JoinHandle;
use tauri::AppHandle;
use tokio::sync::Mutex;

use crate::bridge::mode_state::PlanPhase;
use crate::bridge::sessions::SessionStore;
use crate::bridge::Pinvou3Bridge;
use crate::engine::AppEngine;

/// 池里一个 session 的常驻条目:engine + 它专属的 event forwarder task。
struct EngineEntry {
    engine: AppEngine,
    /// 该 engine 的 event forwarder,evict 时 abort,避免僵尸 task 继续 emit。
    forwarder: JoinHandle<()>,
}

/// 多 session engine 池。Tauri State 持有,`Clone` 廉价(内部全是 Arc)。
#[derive(Clone)]
pub struct EnginePool {
    entries: Arc<Mutex<HashMap<String, EngineEntry>>>,
    app: AppHandle,
    store: SessionStore,
    /// 所有 session 共享一份已 boot 的 bridge(boot 会写盘 / 设 env,只能一次)。
    /// commands 读 model / workspace 也走这里。
    pub bridge: Pinvou3Bridge,
}

impl EnginePool {
    /// boot bridge(一次)并建空池。不预热任何 engine(lazy)。
    pub fn new(app: AppHandle, store: SessionStore) -> Result<Self> {
        let bridge = Pinvou3Bridge::boot()?;
        Ok(Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            app,
            store,
            bridge,
        })
    }

    /// 取该 session 的 engine,没有就 spawn 一个。spawn 后若该 session 有磁盘历史
    /// 则一次性 `SyncSession` 把历史 messages 注水进新 engine(冷启动 / app 重启后
    /// 打开旧会话再发消息的场景)。
    pub async fn get_or_spawn(&self, session_id: &str) -> Result<AppEngine> {
        let mut entries = self.entries.lock().await;
        if let Some(entry) = entries.get(session_id) {
            return Ok(entry.engine.clone());
        }

        let (engine, forwarder) = AppEngine::spawn_for_session(
            self.app.clone(),
            self.store.clone(),
            self.bridge.clone(),
            session_id,
        )
        .await?;

        // 注水历史:仅当磁盘上该 session 已有 messages(新建空 session 跳过)。
        if let Ok(saved) = self.store.load(session_id) {
            if !saved.messages.is_empty() {
                if let Err(e) = engine
                    .sync_session(session_id.to_string(), saved.messages)
                    .await
                {
                    eprintln!("[engine_pool] sync history for {session_id} failed: {e:?}");
                }
            }
        }

        entries.insert(
            session_id.to_string(),
            EngineEntry {
                engine: engine.clone(),
                forwarder,
            },
        );
        Ok(engine)
    }

    /// 取已存在的 engine(不 spawn)。cancel / submit_user_input 等用:engine 没起
    /// 说明该 session 没在跑,这些操作天然是 no-op。
    pub async fn handle_for(&self, session_id: &str) -> Option<AppEngine> {
        self.entries
            .lock()
            .await
            .get(session_id)
            .map(|e| e.engine.clone())
    }

    /// 回收某 session 的 engine:cancel 在跑的 turn → Shutdown engine → abort forwarder。
    /// 删除 session 时调。
    pub async fn evict(&self, session_id: &str) {
        if let Some(entry) = self.entries.lock().await.remove(session_id) {
            entry.engine.cancel_current();
            if let Err(e) = entry.engine.handle.send(Op::Shutdown).await {
                eprintln!("[engine_pool] shutdown {session_id} failed: {e:?}");
            }
            entry.forwarder.abort();
        }
    }

    // ── 高层路由(commands.rs 调用)─────────────────────────────────

    /// 发用户消息给指定 session 的 engine(没起则 lazy spawn)。
    pub async fn send_user_message(
        &self,
        session_id: &str,
        content: String,
        mode: AppMode,
        phase: PlanPhase,
    ) -> Result<()> {
        // 卡片池: 该 session 加持了专家面具时,解析成 per-turn reminder 一起注入。
        // 在 pool 层解析,所有上层调用(chat / accept_plan / pinvou_review_chat)自动带上 persona。
        let persona_reminder = self
            .store
            .active_persona_id(session_id)
            .and_then(|pid| crate::personas::get(&pid))
            .map(crate::personas::equip_reminder);
        self.get_or_spawn(session_id)
            .await?
            .send_user_message(content, mode, phase, persona_reminder)
            .await
    }

    /// 取消指定 session 正在生成的回复。engine 没起则 no-op。
    pub async fn cancel(&self, session_id: &str) {
        if let Some(engine) = self.handle_for(session_id).await {
            engine.cancel_current();
        }
    }

    /// 编辑/重发指定 session 最后一轮 user 消息。
    pub async fn edit_last_turn(&self, session_id: &str, new_message: String) -> Result<()> {
        self.get_or_spawn(session_id)
            .await?
            .edit_last_turn(new_message)
            .await
    }

    /// 手动压缩指定 session 上下文。engine 没起则 no-op(无上下文可压)。
    pub async fn compact_now(&self, session_id: &str) -> Result<()> {
        if let Some(engine) = self.handle_for(session_id).await {
            engine.compact_now().await?;
        }
        Ok(())
    }

    /// 提交指定 session 的 request_user_input 选择。
    pub async fn submit_user_input(
        &self,
        session_id: &str,
        tool_call_id: String,
        response: UserInputResponse,
    ) -> Result<()> {
        if let Some(engine) = self.handle_for(session_id).await {
            engine.submit_user_input(tool_call_id, response).await?;
        }
        Ok(())
    }

    /// 取消指定 session 的 request_user_input。
    pub async fn cancel_user_input(&self, session_id: &str, tool_call_id: String) -> Result<()> {
        if let Some(engine) = self.handle_for(session_id).await {
            engine.cancel_user_input(tool_call_id).await?;
        }
        Ok(())
    }

    /// super permission 改动后调用。**无需热刷静态 prompt**——sudo 的开/关状态
    /// 已改由 `build_send_message_op` 每 turn 注入 `<system-reminder>`
    /// (见 `super_permission::turn_reminder`),`is_enabled()` 每次实时读 disk,
    /// 所以切开关下一 turn 自动生效。静态 prompt 里只剩一句中性指引(指向
    /// per-turn reminder),过不过时都不影响行为。
    ///
    /// 本函数保留为 no-op:调用点(set_super_permission)语义上"通知一下",
    /// 但实际生效靠 per-turn 注入,不依赖这里。
    pub async fn refresh_all_instructions(&self) {
        let live_count = self.entries.lock().await.len();
        eprintln!(
            "[engine_pool] sudo permission changed; {live_count} live session(s) — \
             new state takes effect next turn via per-turn system-reminder"
        );
    }
}

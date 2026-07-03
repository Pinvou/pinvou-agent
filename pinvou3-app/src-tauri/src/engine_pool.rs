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

use crate::bridge::prefs::UserPrefs;
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

    /// 为 spawn 构造该 session 的 bridge:从 disk 读最新 prefs(模型列表/默认可能刚被
    /// GUI 改过),再按该 session 的显式 model_id 注入 session_model(没绑定则回退全局
    /// active)。绑定指向已删模型时 `model_by_id` 返回 None,自然回退 active。
    /// 这是「热切换不重启」的落点:改模型只写 disk + evict,下次 spawn 经此读到新配置。
    async fn fresh_bridge_for(&self, session_id: &str) -> Pinvou3Bridge {
        let mut b = self.bridge.clone();
        b.prefs = UserPrefs::load();
        b.session_model = self
            .store
            .session_model_id(session_id)
            .and_then(|mid| b.prefs.model_by_id(&mid).cloned());
        // 本地 vLLM:发请求的 model 名以 vLLM 实际 served name 为准(探测 /v1/models),
        // 免去写死 qwen36_35b_256k 与 --served-model-name 不一致的 model_not_found。
        // 探测失败(vLLM 没起)保持配置值;云端 provider 不探测。
        if b.provider() == "vllm" {
            let (served, max_len) = crate::monitor::probe_vllm_model_info(&b.base_url()).await;
            if let Some(served) = served {
                if let Some(mut m) = b.effective_model_owned() {
                    if m.model != served {
                        m.model = served;
                        b.session_model = Some(m);
                    }
                }
            }
            // 窗口探测:填给 bridge,build_engine_config 据此填 active_route_limits.context_tokens
            // + 按真实窗口推导压缩阈值。探测失败保持 None → 名字 hint 老路。
            b.probed_context_tokens = max_len;
        }
        b
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
            self.fresh_bridge_for(session_id).await,
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

    /// 回收当前 active session 的 engine。用于全局能力开关/连接器状态变化后,
    /// 让下一轮按最新 Skill catalogue 重建 system prompt。
    pub async fn evict_active(&self) {
        if let Some(session_id) = self.store.active_id() {
            self.evict(&session_id).await;
        }
    }

    // ── 模型热切换(commands.rs 调用)──────────────────────────────

    /// 新建会话用的默认模型:取全局 active model 的(model 名, id)。从 disk 读最新
    /// (GUI 可能刚改过默认),失败回退 boot 快照。
    pub fn default_model_for_new_session(&self) -> (String, Option<String>) {
        let prefs = UserPrefs::load();
        match prefs.active_model() {
            Some(m) => (m.model.clone(), Some(m.id.clone())),
            None => (self.bridge.model(), None),
        }
    }

    /// 切某 session 的模型(聊天 chip 热切):写 per-session 绑定 + evict 该 session
    /// engine。下次发消息 get_or_spawn 用新模型重建(跨 provider 重建 client;历史靠
    /// SyncSession 注水)。`model_id = None` = 清除绑定回退全局默认。
    pub async fn switch_session_model(&self, session_id: &str, model_id: Option<String>) {
        self.store.set_session_model_id(session_id, model_id);
        self.evict(session_id).await;
    }

    // ── 高层路由(commands.rs 调用)─────────────────────────────────

    /// 发用户消息给指定 session 的 engine(没起则 lazy spawn)。
    pub async fn send_user_message(
        &self,
        session_id: &str,
        content: String,
        mode: AppMode,
    ) -> Result<()> {
        // Side B 卡片池: 该 session 加持了专家面具时,每 turn 注入轻锚点(短)维持身份。
        // 完整 body 已在加持首条消息一次性注入(commands::chat take_pending_persona_body)。
        // 在 pool 层解析,所有上层调用(chat / accept_plan)自动带上锚点。
        let persona_reminder = self
            .store
            .active_persona_id(session_id)
            .and_then(|pid| crate::personas::get(&pid))
            .map(|c| crate::personas::equip_anchor(&c));
        self.get_or_spawn(session_id)
            .await?
            .send_user_message(content, mode, persona_reminder)
            .await
    }

    /// 取消指定 session 正在生成的回复。engine 没起则 no-op。
    pub async fn cancel(&self, session_id: &str) {
        if let Some(engine) = self.handle_for(session_id).await {
            engine.cancel_current();
        }
    }

    /// pinvou3 工具开关(全局持久):把"被禁用的工具全名"(模型可见全名,小写)广播给
    /// **所有在跑的 session engine** → 写入各自 config.disallowed_tools,下一轮即隐藏。
    /// 没起的会话下次 spawn 时从持久列表读初值(build_engine_config),所以新窗口/新对话
    /// 都继承同一份禁用状态。
    pub async fn set_disallowed_all(&self, tools: Vec<String>) {
        let entries = self.entries.lock().await;
        for (sid, entry) in entries.iter() {
            if let Err(e) = entry
                .engine
                .handle
                .send(Op::SetDisallowedTools { tools: tools.clone() })
                .await
            {
                eprintln!("[engine_pool] set_disallowed_all {sid} failed: {e:?}");
            }
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

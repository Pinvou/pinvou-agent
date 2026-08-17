//! MockRuntime: ProductChatRuntime 的 mock 实现，用于 CI 确定性测试。
//!
//! 不调真实 provider——模拟延迟后写入 timing_events，让 wait_for_completion
//! 能读到结果。PinvouChatRunner 用 MockRuntime 时行为与真实 runtime 一致，
//! 只是跳过了模型调用和工具执行。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;

use crate::features::assistant::product_runtime::{
    ProductChatRuntime, RuntimeToolEvent, SessionSpec, TurnHandle, TurnInput, TurnResult,
};
use crate::features::assistant::timing;

/// Mock 行为配置
#[derive(Clone)]
pub struct MockConfig {
    /// 模拟延迟（ms），0 = 立即完成
    pub delay_ms: u64,
    /// 模拟完成状态
    pub status: String,
    /// 模拟错误
    pub error: Option<String>,
    /// 模拟 token 用量
    pub usage: Option<timing::TurnUsage>,
    pub assistant_text: String,
    pub tool_events: Vec<RuntimeToolEvent>,
}

impl Default for MockConfig {
    fn default() -> Self {
        Self {
            delay_ms: 0,
            status: "Completed".to_string(),
            error: None,
            usage: Some(timing::TurnUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_hit_tokens: 80,
                cache_miss_tokens: 20,
                cache_write_tokens: 0,
                reasoning_tokens: 10,
            }),
            assistant_text: "mock answer".to_string(),
            tool_events: Vec::new(),
        }
    }
}

struct MockSession {
    turn_active: bool,
}

/// Mock 实现：不调 provider，模拟延迟后写 timing_events。
///
/// 用法：
/// ```
/// let mock = MockRuntime::new(MockConfig { delay_ms: 10, ..Default::default() });
/// let runner = PinvouChatRunner::new(mock);
/// let record = runner.run_case(&case).await?;
/// ```
#[derive(Clone)]
pub struct MockRuntime {
    sessions: Arc<Mutex<HashMap<String, MockSession>>>,
    config: MockConfig,
    prepared_model_ids: Arc<Mutex<Vec<Option<String>>>>,
}

impl MockRuntime {
    pub fn new(config: MockConfig) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            config,
            prepared_model_ids: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 快速创建一个立即完成的 mock（零延迟）
    pub fn immediate() -> Self {
        Self::new(MockConfig::default())
    }

    pub fn prepared_model_ids(&self) -> Vec<Option<String>> {
        self.prepared_model_ids.lock().unwrap().clone()
    }
}

impl ProductChatRuntime for MockRuntime {
    async fn prepare(&self, spec: &SessionSpec) -> Result<()> {
        self.prepared_model_ids.lock().unwrap().push(
            spec.model_selection
                .as_ref()
                .and_then(|selection| selection.model_id().map(str::to_string)),
        );
        self.sessions
            .lock()
            .unwrap()
            .insert(spec.session_id.clone(), MockSession { turn_active: false });
        Ok(())
    }

    async fn submit(&self, input: &TurnInput) -> Result<TurnHandle> {
        let turn_id = timing::start_turn(&input.session_id);

        {
            let mut sessions = self.sessions.lock().unwrap();
            if let Some(session) = sessions.get_mut(&input.session_id) {
                session.turn_active = true;
            }
        }

        // 模拟延迟后完成：spawn 一个 task 写 timing_events 并清除 active 标记
        let sessions = self.sessions.clone();
        let session_id = input.session_id.clone();
        let status = self.config.status.clone();
        let error = self.config.error.clone();
        let usage = self.config.usage;
        let delay_ms = self.config.delay_ms;

        tokio::spawn(async move {
            if delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            timing::finish_turn_with_usage(&session_id, &status, error.as_deref(), usage);
            if let Some(session) = sessions.lock().unwrap().get_mut(&session_id) {
                session.turn_active = false;
            }
        });

        Ok(TurnHandle {
            session_id: input.session_id.clone(),
            turn_id,
        })
    }

    async fn wait_for_completion(&self, handle: &TurnHandle) -> Result<TurnResult> {
        let session_id = &handle.session_id;
        while self.is_turn_active(session_id) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let timeline =
            timing::read_timeline(session_id).map_err(|e| anyhow::anyhow!("read timeline: {e}"))?;
        let entry = timeline
            .iter()
            .rev()
            .find(|e| e.turn_id == handle.turn_id && e.event == "assistant_done");
        let milestones = timeline
            .iter()
            .filter(|event| {
                event.turn_id == handle.turn_id
                    && !matches!(event.event.as_str(), "user_start" | "assistant_done")
            })
            .cloned()
            .collect();

        Ok(TurnResult {
            turn_id: handle.turn_id.clone(),
            status: entry
                .and_then(|e| e.status.clone())
                .unwrap_or_else(|| "unknown".to_string()),
            error: entry.and_then(|e| e.error.clone()),
            usage: entry.and_then(|e| e.usage),
            milestones,
            assistant_text: self.config.assistant_text.clone(),
            tool_events: self.config.tool_events.clone(),
        })
    }

    fn is_turn_active(&self, session_id: &str) -> bool {
        self.sessions
            .lock()
            .unwrap()
            .get(session_id)
            .map(|s| s.turn_active)
            .unwrap_or(false)
    }

    async fn cancel(&self, session_id: &str) {
        if let Some(session) = self.sessions.lock().unwrap().get_mut(session_id) {
            session.turn_active = false;
        }
    }

    async fn close(&self, session_id: &str) {
        self.sessions.lock().unwrap().remove(session_id);
        let _ = std::fs::remove_file(crate::platform::paths::session_timing_events(session_id));
    }
}

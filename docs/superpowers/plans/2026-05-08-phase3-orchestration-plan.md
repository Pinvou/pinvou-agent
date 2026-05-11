# Phase 3 本地模型编排层 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现 pinvou3 的「代码做路由器、LLM 做执行者、用户做决策者」三角色分离编排层。

**Architecture:** 四个新模块 + Engine 集成 + TUI 对接。DeepSeekHarness 实现 AgentHarness trait 对接本地 LLM；StepBuilder 构造小范围 prompt；LLMReviewer 做拆解语义审阅；ResponseChecker 解析自评信号 + 越界检测 + 路由决策。宽松解析 + 降级兜底，用户确认为安全网。

**Tech Stack:** Rust (tokio, async-trait, serde, regex), ratatui, 现有 DeepSeek-TUI 的 LlmClient trait / DeepSeekClient / MockLlmClient

**Spec:** `docs/superpowers/specs/2026-05-08-phase3-orchestration-spec.md`
**Architecture doc:** `设计架构文档-pinvou3.md` (project root)

---

## File Structure

```
pinvou3/
├── DeepSeek-TUI/crates/tui/src/lib.rs  # [NEW] pub mod declarations for library
├── pinvou-platform/
│   ├── Cargo.toml                # deepseek-tui = { path = "../DeepSeek-TUI/crates/tui" }
│   └── src/
│       ├── lib.rs                # [MODIFY] Add new module declarations
│       ├── main.rs               # [UNCHANGED] Binary entry
│       ├── harness.rs            # [UNCHANGED] AgentHarness trait
│       ├── engine.rs             # [MODIFY] Add decompose_and_execute(), wire StepBuilder/LLMReviewer/ResponseChecker
│       ├── app.rs                # [UNCHANGED] AppConfig/AppRegistry
│       ├── workflow.rs           # [UNCHANGED] ConversationState
│       ├── router.rs             # [UNCHANGED] ModelRouter
│       ├── deepseek_harness.rs   # [CREATE] DeepSeekHarness<C: LlmClient> impl AgentHarness
│       ├── step_builder.rs       # [CREATE] StepBuilder — prompt construction
│       ├── reviewer.rs           # [CREATE] LLMReviewer — semantic review of decomposition
│       ├── response_checker.rs   # [CREATE] ResponseChecker — signal parsing + out-of-scope detection + routing
│       └── tui/
│           ├── mod.rs            # [UNCHANGED]
│           ├── app.rs            # [MODIFY] Add consecutive_out_of_scope, wire engine
│           ├── chat.rs           # [UNCHANGED]
│           ├── input.rs          # [UNCHANGED]
│           ├── launcher.rs       # [UNCHANGED]
│           ├── sidebar.rs        # [UNCHANGED]
│           └── ui.rs             # [MODIFY] Replace simulate_engine_response with real engine call
└── apps/                         # [MOVED] 从 DeepSeek-TUI/apps/ 移到此处
```

---

### Task 1: Add `regex` dependency to Cargo.toml

**Files:**
- Modify: `pinvou-platform/Cargo.toml`

- [ ] **Step 1: Add regex to dependencies**

Open `pinvou-platform/Cargo.toml`. Under the `[dependencies]` section, add:

```toml
regex = "1"
```

- [ ] **Step 2: Verify compilation**

```bash
cargo check --manifest-path /home/hexin/opencode_projects/pinvou3/pinvou-platform/Cargo.toml 2>&1 | tail -5
```

Expected: `Checking deepseek-tui ... Finished` (no errors from missing dep)

- [ ] **Step 3: Commit**

```bash
git add pinvou-platform/Cargo.toml
git commit -m "chore: add regex dependency for Phase 3 signal parsing"
```

---

### Task 2: Create `DeepSeekHarness` — AgentHarness implementation

**Files:**
- Create: `pinvou-platform/src/deepseek_harness.rs`
- Modify: `pinvou-platform/src/lib.rs`

- [ ] **Step 1: Write the module skeleton with failing compilation check**

Create `pinvou-platform/src/deepseek_harness.rs`:

```rust
//! DeepSeekHarness — 实现 AgentHarness trait，对接本地和远程 LLM。
//!
//! 泛型参数 C: LlmClient 支持注入 DeepSeekClient（生产）、Ollama client、MockLlmClient（测试）。

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures_util::StreamExt;

use super::harness::{
    AgentHarness, ChatRequest, Checkpoint, ModelInfo, StreamEvent, ToolDef,
};

/// 基于 LlmClient trait 的 AgentHarness 实现。
///
/// 核心工作：ChatRequest → MessageRequest 的类型适配，
/// 以及 StreamEvent（平台层） ↔ 流式事件（LlmClient 层）的双向映射。
pub struct DeepSeekHarness<C: LlmClient> {
    client: C,
    tools: Vec<ToolDef>,
    models: Vec<ModelInfo>,
    workspace: PathBuf,
    checkpoint_dir: PathBuf,
}

// 需要 import 的类型
use crate::llm_client::LlmClient;
use crate::models::{ContentBlock, Message, MessageRequest, SystemPrompt};
```

Verify it doesn't compile yet (missing impl):

```bash
cargo check --manifest-path /home/hexin/opencode_projects/pinvou3/pinvou-platform/Cargo.toml 2>&1 | tail -10
```

- [ ] **Step 2: Implement constructor and builder**

Replace the file content with:

```rust
//! DeepSeekHarness — 实现 AgentHarness trait，对接本地和远程 LLM。
//!
//! 泛型参数 C: LlmClient 支持注入 DeepSeekClient（生产）、Ollama client、MockLlmClient（测试）。

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures_util::{Stream, StreamExt, stream};

use super::harness::{
    AgentHarness, ChatRequest, Checkpoint, ModelInfo, StreamEvent, ToolDef,
};
use crate::llm_client::LlmClient;
use crate::models::{ContentBlock, Message, MessageRequest, SystemPrompt};

pub struct DeepSeekHarness<C: LlmClient> {
    client: C,
    tools: Vec<ToolDef>,
    models: Vec<ModelInfo>,
    workspace: PathBuf,
    checkpoint_dir: PathBuf,
}

impl<C: LlmClient> DeepSeekHarness<C> {
    pub fn new(
        client: C,
        tools: Vec<ToolDef>,
        models: Vec<ModelInfo>,
        workspace: PathBuf,
    ) -> Self {
        let checkpoint_dir = workspace.join(".checkpoints");
        Self {
            client,
            tools,
            models,
            workspace,
            checkpoint_dir,
        }
    }

    /// 自定义 checkpoint 目录
    pub fn with_checkpoint_dir(mut self, dir: PathBuf) -> Self {
        self.checkpoint_dir = dir;
        self
    }
}
```

- [ ] **Step 3: Implement ChatRequest → MessageRequest mapping**

Add to `impl<C: LlmClient> DeepSeekHarness<C>` block:

```rust
    /// 将平台 ChatRequest 转换为 LlmClient 的 MessageRequest
    fn to_message_request(&self, req: &ChatRequest) -> MessageRequest {
        let model = req
            .model
            .clone()
            .unwrap_or_else(|| self.client.model().to_string());

        // 构建 system prompt: platform_system_prompt 优先
        let system = req.platform_system_prompt.as_ref().map(|p| {
            SystemPrompt::Text(p.clone())
        });

        // 构建 messages: user_message + context
        let mut messages = Vec::new();

        // 如果有 context，拼成一条 system-like user message 插入
        if !req.context.is_empty() {
            let ctx_text: String = req
                .context
                .iter()
                .map(|(k, v)| format!("[{k}]: {v}"))
                .collect::<Vec<_>>()
                .join("\n");
            messages.push(Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: format!("## 上下文信息\n\n{ctx_text}"),
                }],
            });
        }

        messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: req.user_message.clone(),
            }],
        });

        // 转换 tools
        let tools: Vec<crate::models::Tool> = req
            .tools
            .iter()
            .map(|t| crate::models::Tool {
                tool_type: None,
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.parameters.clone(),
                allowed_callers: None,
                defer_loading: None,
                input_examples: None,
                strict: None,
                cache_control: None,
            })
            .collect();

        let tools_opt = if tools.is_empty() {
            None
        } else {
            Some(tools)
        };

        MessageRequest {
            model,
            messages,
            max_tokens: 4096,
            system,
            tools: tools_opt,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: None,
            stream: None,
            temperature: None,
            top_p: None,
        }
    }
```

- [ ] **Step 4: Implement AgentHarness trait**

Add after the `impl<C: LlmClient> DeepSeekHarness<C>` block:

```rust
#[async_trait]
impl<C: LlmClient + Send + Sync> AgentHarness for DeepSeekHarness<C> {
    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<Box<dyn Stream<Item = Result<StreamEvent>> + Send + Unpin>> {
        let msg_req = self.to_message_request(&req);

        let llm_stream = self.client.create_message_stream(msg_req).await?;

        // 映射 LlmClient 的 StreamEvent → 平台层的 StreamEvent
        let mapped = llm_stream.map(|result| {
            match result {
                Ok(crate::models::StreamEvent::ContentBlockDelta { delta, .. }) => {
                    // delta 是 serde_json::Value，提取 text 字段
                    if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                        Ok(StreamEvent::TextDelta {
                            content: text.to_string(),
                        })
                    } else {
                        // 忽略非文本 delta（如 thinking）
                        Ok(StreamEvent::TextDelta {
                            content: String::new(),
                        })
                    }
                }
                Ok(crate::models::StreamEvent::ContentBlockStart { content_block, .. }) => {
                    if content_block.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                        let call_id = content_block
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let tool_name = content_block
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let arguments = content_block
                            .get("input")
                            .cloned()
                            .unwrap_or(serde_json::Value::Object(Default::default()));
                        Ok(StreamEvent::ToolCallStart {
                            call_id,
                            tool_name,
                            arguments,
                        })
                    } else {
                        // 非工具块开始，跳过
                        Ok(StreamEvent::TextDelta {
                            content: String::new(),
                        })
                    }
                }
                Ok(crate::models::StreamEvent::MessageStop) => Ok(StreamEvent::Done),
                Ok(_) => Ok(StreamEvent::TextDelta {
                    content: String::new(),
                }),
                Err(e) => Ok(StreamEvent::Error {
                    message: e.to_string(),
                }),
            }
        });

        Ok(Box::new(mapped))
    }

    fn tools(&self) -> Vec<ToolDef> {
        self.tools.clone()
    }

    fn models(&self) -> Vec<ModelInfo> {
        self.models.clone()
    }

    fn save_checkpoint(&self, state: &Checkpoint) -> Result<()> {
        if !self.checkpoint_dir.exists() {
            fs::create_dir_all(&self.checkpoint_dir)
                .context("Failed to create checkpoint directory")?;
        }
        let path = self.checkpoint_dir.join(format!("{}.json", state.session_id));
        let json = serde_json::to_string_pretty(state)?;
        fs::write(&path, json).context("Failed to write checkpoint")?;
        Ok(())
    }

    fn load_checkpoint(&self, id: &str) -> Result<Option<Checkpoint>> {
        let path = self.checkpoint_dir.join(format!("{id}.json"));
        if !path.exists() {
            return Ok(None);
        }
        let json = fs::read_to_string(&path).context("Failed to read checkpoint")?;
        let checkpoint: Checkpoint = serde_json::from_str(&json)?;
        Ok(Some(checkpoint))
    }

    fn list_sessions(&self) -> Result<Vec<String>> {
        if !self.checkpoint_dir.exists() {
            return Ok(vec![]);
        }
        let mut sessions = Vec::new();
        for entry in fs::read_dir(&self.checkpoint_dir)? {
            let entry = entry?;
            if entry.path().extension().map(|e| e == "json").unwrap_or(false) {
                if let Some(stem) = entry.path().file_stem() {
                    sessions.push(stem.to_string_lossy().to_string());
                }
            }
        }
        Ok(sessions)
    }

    fn workspace_dir(&self) -> PathBuf {
        self.workspace.clone()
    }
}
```

- [ ] **Step 5: Write unit test with MockLlmClient**

Add at the end of the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_client::mock::MockLlmClient;
    use std::path::PathBuf;

    fn test_harness() -> DeepSeekHarness<MockLlmClient> {
        let mock = MockLlmClient::new();
        let tools = vec![ToolDef {
            name: "file_read".into(),
            description: "Read files".into(),
            parameters: serde_json::json!({}),
        }];
        let models = vec![ModelInfo {
            id: "test-model".into(),
            provider: "mock".into(),
            capability: "medium".into(),
        }];
        DeepSeekHarness::new(mock, tools, models, PathBuf::from("/tmp/test-workspace"))
    }

    #[tokio::test]
    async fn test_chat_returns_response() {
        let harness = test_harness();
        let result = harness
            .chat(ChatRequest {
                user_message: "你好".into(),
                platform_system_prompt: None,
                context: Default::default(),
                tools: vec![],
                model: None,
                session_id: None,
            })
            .await;
        // MockLlmClient 默认返回空或预设响应，只要不 panic 就行
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_chat_injects_context() {
        let harness = test_harness();
        let mut context = std::collections::HashMap::new();
        context.insert("file_path".into(), "/tmp/test.csv".into());

        let result = harness
            .chat(ChatRequest {
                user_message: "分析文件".into(),
                platform_system_prompt: Some("你是一个数据分析师".into()),
                context,
                tools: vec![],
                model: None,
                session_id: None,
            })
            .await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_tools_returns_cloned() {
        let harness = test_harness();
        let tools = harness.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "file_read");
    }

    #[test]
    fn test_models_returns_cloned() {
        let harness = test_harness();
        let models = harness.models();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "test-model");
    }

    #[test]
    fn test_checkpoint_save_and_load() {
        let harness = test_harness().with_checkpoint_dir(PathBuf::from("/tmp/test-checkpoints"));
        let checkpoint = Checkpoint {
            session_id: "test-session-1".into(),
            app_id: "test-app".into(),
            conversation_state: serde_json::json!({"turn": 1}),
            created_at: 1234567890,
        };
        harness.save_checkpoint(&checkpoint).unwrap();
        let loaded = harness.load_checkpoint("test-session-1").unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().session_id, "test-session-1");

        // cleanup
        let _ = std::fs::remove_dir_all("/tmp/test-checkpoints");
    }

    #[test]
    fn test_list_sessions() {
        let harness = test_harness().with_checkpoint_dir(PathBuf::from("/tmp/test-sessions-list"));
        let ckpt = Checkpoint {
            session_id: "s1".into(),
            app_id: "a".into(),
            conversation_state: serde_json::json!({}),
            created_at: 0,
        };
        harness.save_checkpoint(&ckpt).unwrap();
        let sessions = harness.list_sessions().unwrap();
        assert!(sessions.contains(&"s1".to_string()));

        let _ = std::fs::remove_dir_all("/tmp/test-sessions-list");
    }

    #[test]
    fn test_workspace_dir() {
        let harness = test_harness();
        assert_eq!(harness.workspace_dir(), PathBuf::from("/tmp/test-workspace"));
    }
}
```

- [ ] **Step 6: Run tests to verify**

```bash
cargo test --manifest-path /home/hexin/opencode_projects/pinvou3/pinvou-platform/Cargo.toml -- deepseek_harness 2>&1 | tail -20
```

Expected: All 6 tests PASS

- [ ] **Step 7: Register module in mod.rs**

Edit `pinvou-platform/src/lib.rs`:

```rust
pub mod app;
pub mod engine;
pub mod harness;
pub mod router;
pub mod tui;
pub mod workflow;
pub mod deepseek_harness;
```

- [ ] **Step 8: Verify compilation**

```bash
cargo check --manifest-path /home/hexin/opencode_projects/pinvou3/pinvou-platform/Cargo.toml 2>&1 | tail -5
```

Expected: `Finished` with no errors

- [ ] **Step 9: Commit**

```bash
git add pinvou-platform/src/deepseek_harness.rs pinvou-platform/src/lib.rs
git commit -m "feat: add DeepSeekHarness — AgentHarness impl wrapping LlmClient"
```

---

### Task 3: Create `StepBuilder` — prompt construction

**Files:**
- Create: `pinvou-platform/src/step_builder.rs`
- Modify: `pinvou-platform/src/lib.rs`

- [ ] **Step 1: Write module with all function signatures**

Create `pinvou-platform/src/step_builder.rs`:

```rust
//! StepBuilder — 小范围 prompt 构造器。
//!
//! 纯函数模块，无状态，不做 LLM 调用。
//! - build(): 为当前里程碑构造小范围执行 prompt
//! - build_decomposition(): 构造任务拆解 prompt
//! - build_review_prompt(): 构造审阅 prompt
//! - ban_list(): 按 app + phase 返回禁止清单

use std::collections::HashMap;

use super::app::{AppConfig, Milestone};

/// 构造好的 prompt 输出
#[derive(Debug, Clone)]
pub struct StepPrompt {
    /// 发给 LLM 的 system prompt（替代默认）
    pub system: String,
    /// 是否需要在 system 之外追加用户消息
    pub append_user_message: bool,
}

impl StepBuilder {
    /// 为当前里程碑构造小范围执行 prompt
    pub fn build(
        milestone: &Milestone,
        context: &HashMap<String, String>,
        user_message: &str,
        app_config: &AppConfig,
    ) -> StepPrompt {
        let mut parts = Vec::new();

        // 1. 当前任务
        parts.push("## 当前任务（只做这个）".to_string());
        let task_desc = milestone
            .prompt_hint
            .as_deref()
            .unwrap_or(&milestone.label);
        parts.push(task_desc.to_string());

        // 2. 已知信息
        if !context.is_empty() {
            parts.push("\n## 已知信息".to_string());
            for (k, v) in context {
                parts.push(format!("- **{k}**: {v}"));
            }
        }

        // 3. 禁止清单
        parts.push("\n## 禁止".to_string());
        for ban in Self::ban_list(&app_config.id, &milestone.label) {
            parts.push(format!("- {ban}"));
        }

        // 4. 自评信号提醒
        parts.push("\n## 输出末尾附加当前步骤状态：".to_string());
        parts.push("[OK] / [MORE] 还需要:{具体内容} / [BLOCKED] 原因:{具体原因}".to_string());

        // 5. 用户消息（仅在 system 内嵌入，如果 append_user_message 为 true 则分开）
        let system = parts.join("\n");
        let user_section = format!("\n\n## 用户消息\n{user_message}");

        StepPrompt {
            system: if user_message.is_empty() {
                system
            } else {
                system + &user_section
            },
            append_user_message: false,
        }
    }

    /// 构造任务拆解 prompt（架构文档 3.3 节）
    pub fn build_decomposition(
        user_request: &str,
        app_config: &AppConfig,
        available_tools: &[String],
        context_summary: &str,
    ) -> String {
        let tools_str = if available_tools.is_empty() {
            "（无额外工具）".to_string()
        } else {
            available_tools.join(", ")
        };

        format!(
            r#"用户想: "{user_request}"
当前应用: {app_name} -- {app_desc}
可用工具: {tools_str}
已知信息: {ctx}

请把这个任务拆成多个小步骤。

拆解规则:
1. 每步 = 一个可用 1-3 次工具调用完成的完整动作
2. 每步必须有明确的可验证产出物（文件、图表、文本段落、确认）
3. 每步的产出物是下一步的输入
4. 不限制步骤总数 -- 复杂任务可以多步，简单任务可以少步
5. 不能假设用户的工具能力

禁止:
x 笼统步骤: "分析数据"、"写文档"、"处理"、"做"
x 无产出物的步骤
x 超过 5 次工具调用才能完成的粗粒度步骤
x TBD、TODO、placeholder

好例子:
"读取 sales.csv，展示列名、行数、数据类型和缺失情况"
"按月汇总销售额，计算环比增长率，用表格展示"
"生成周报草稿(三段: 本周工作/问题/下周计划，约500字)"

差例子:
x "分析销售数据" -- 太笼统
x "打开文件" -- 太细，不是完整动作

只输出步骤列表，每行: "N. {{具体动词+具体对象+明确产出}}""#,
            user_request = user_request,
            app_name = app_config.name,
            app_desc = app_config.description,
            tools_str = tools_str,
            ctx = if context_summary.is_empty() { "暂无" } else { context_summary },
        )
    }

    /// 构造审阅 prompt（架构文档 3.4 节）
    pub fn build_review_prompt(
        decomposition: &str,
        user_request: &str,
        available_tools: &[String],
    ) -> String {
        let tools_str = if available_tools.is_empty() {
            "（无额外工具）".to_string()
        } else {
            available_tools.join(", ")
        };

        format!(
            r#"你是任务拆解审阅员。检查以下步骤拆解。

拆解结果:
{decomposition}

用户原始需求: {user_request}
可用工具: {tools_str}

检查项:
1. 每步都具体吗？（有没有"分析"、"处理"、"做"这种空洞词？）
2. 每步的产出物明确吗？（做完能判断"完成了"吗？）
3. 步骤之间连续吗？（前一步输出是后一步输入吗？）
4. 整体覆盖用户需求吗？（有没有遗漏？）

输出 JSON (只输出 JSON):
{{
  "ok": true/false,
  "issues": [
    {{"step": 2, "problem": "太笼统", "suggestion": "改为..."}}
  ],
  "overall": "一句话总结"
}}"#
        )
    }

    /// 根据 app id 和当前阶段返回禁止清单
    pub fn ban_list(app_id: &str, phase: &str) -> Vec<&'static str> {
        let mut bans: Vec<&'static str> = vec![
            "不要一次完成多个步骤",
            "不要自己编造不存在的数据",
            "完成当前任务后必须附加自评信号 [OK]/[MORE]/[BLOCKED]",
        ];

        // 按 app 类型
        match app_id {
            "文档生成" => {
                if phase.contains("需求") || phase.contains("确认") || phase.contains("收集") {
                    bans.push("不要生成完整文档内容");
                    bans.push("不要跳过询问直接假设用户需求");
                    bans.push("只问不写，不要提前生成内容");
                }
            }
            "数据分析" => {
                if phase.contains("探索") || phase.contains("查看") {
                    bans.push("不要跳过数据验证");
                    bans.push("不要忽略异常值");
                }
            }
            "计划敲定" => {
                if phase.contains("方案") || phase.contains("选项") {
                    bans.push("不要只给一个方案，给出多个选项让用户选");
                }
                if phase.contains("评估") || phase.contains("对比") {
                    bans.push("不要替用户做决定");
                }
            }
            _ => {}
        }

        // 按阶段
        if phase.contains("生成") || phase.contains("草稿") || phase.contains("撰写") {
            bans.push("输出完整内容，末尾询问用户'需要调整哪里？'");
        }
        if phase.contains("定稿") || phase.contains("保存") || phase.contains("提交") {
            bans.push("执行保存操作，不要重新生成内容");
        }

        bans
    }
}

pub struct StepBuilder;
```

- [ ] **Step 2: Write tests**

Add at end of the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use super::super::app::{AppConfig, Milestone};

    fn test_milestone() -> Milestone {
        Milestone {
            id: "draft".into(),
            label: "生成草稿".into(),
            prompt_hint: Some("根据已知信息生成周报草稿，三段式，500字以内。输出后问'需要调整哪里？'".into()),
            icon: None,
        }
    }

    fn test_app() -> AppConfig {
        AppConfig {
            id: "文档生成".into(),
            name: "文档生成".into(),
            description: "生成各类文档".into(),
            icon: "[..]".into(),
            prompt_file: None,
            prompt: None,
            model_preference: "medium".into(),
            tools: vec!["file_write".into()],
            milestones: vec![],
            meta: Default::default(),
        }
    }

    #[test]
    fn test_build_contains_scope_limit() {
        let prompt = StepBuilder::build(&test_milestone(), &Default::default(), "帮我写周报", &test_app());
        assert!(prompt.system.contains("只做这个"));
        assert!(prompt.system.contains("生成草稿"));
    }

    #[test]
    fn test_build_contains_ban_list() {
        let prompt = StepBuilder::build(&test_milestone(), &Default::default(), "帮我写周报", &test_app());
        assert!(prompt.system.contains("不要一次完成多个步骤"));
        assert!(prompt.system.contains("自评信号"));
    }

    #[test]
    fn test_build_contains_context() {
        let mut ctx = std::collections::HashMap::new();
        ctx.insert("doc_type".into(), "周报".into());
        ctx.insert("audience".into(), "内部".into());
        let prompt = StepBuilder::build(&test_milestone(), &ctx, "写周报", &test_app());
        assert!(prompt.system.contains("doc_type"));
        assert!(prompt.system.contains("周报"));
        assert!(prompt.system.contains("audience"));
    }

    #[test]
    fn test_build_contains_user_message() {
        let prompt = StepBuilder::build(&test_milestone(), &Default::default(), "帮我写周报", &test_app());
        assert!(prompt.system.contains("帮我写周报"));
    }

    #[test]
    fn test_decomposition_prompt_structure() {
        let prompt = StepBuilder::build_decomposition(
            "帮我分析销售数据",
            &test_app(),
            &["file_read".into(), "shell".into()],
            "无",
        );
        assert!(prompt.contains("拆成多个小步骤"));
        assert!(prompt.contains("好例子"));
        assert!(prompt.contains("差例子"));
        assert!(prompt.contains("禁止"));
        assert!(prompt.contains("file_read"));
    }

    #[test]
    fn test_review_prompt_structure() {
        let prompt = StepBuilder::build_review_prompt(
            "1. 读取文件\n2. 分析数据",
            "分析销售数据",
            &["file_read".into()],
        );
        assert!(prompt.contains("审阅员"));
        assert!(prompt.contains("ok"));
        assert!(prompt.contains("issues"));
    }

    #[test]
    fn test_ban_list_doc_requirement_phase() {
        let bans = StepBuilder::ban_list("文档生成", "明确需求");
        assert!(bans.iter().any(|b| b.contains("不要生成完整")));
        assert!(bans.iter().any(|b| b.contains("只问不写")));
    }

    #[test]
    fn test_ban_list_doc_generation_phase() {
        let bans = StepBuilder::ban_list("文档生成", "生成草稿");
        assert!(bans.iter().any(|b| b.contains("需要调整哪里")));
    }

    #[test]
    fn test_ban_list_analysis_explore_phase() {
        let bans = StepBuilder::ban_list("数据分析", "探索数据");
        assert!(bans.iter().any(|b| b.contains("数据验证")));
    }

    #[test]
    fn test_ban_list_plan_options_phase() {
        let bans = StepBuilder::ban_list("计划敲定", "方案对比");
        assert!(bans.iter().any(|b| b.contains("不要只给一个")));
    }
}
```

- [ ] **Step 3: Run tests to verify**

```bash
cargo test --manifest-path /home/hexin/opencode_projects/pinvou3/pinvou-platform/Cargo.toml -- step_builder 2>&1 | tail -25
```

Expected: All 10 tests PASS

- [ ] **Step 4: Register in mod.rs**

Edit `pinvou-platform/src/lib.rs`, add after `pub mod router;`:

```rust
pub mod step_builder;
```

- [ ] **Step 5: Verify compilation**

```bash
cargo check --manifest-path /home/hexin/opencode_projects/pinvou3/pinvou-platform/Cargo.toml 2>&1 | tail -5
```

Expected: `Finished`

- [ ] **Step 6: Commit**

```bash
git add pinvou-platform/src/step_builder.rs pinvou-platform/src/lib.rs
git commit -m "feat: add StepBuilder — small-scope prompt and decomposition prompt construction"
```

---

### Task 4: Create `ResponseChecker` — signal parsing + out-of-scope detection

**Files:**
- Create: `pinvou-platform/src/response_checker.rs`
- Modify: `pinvou-platform/src/lib.rs`

- [ ] **Step 1: Write module with all data types and parsing logic**

Create `pinvou-platform/src/response_checker.rs`:

```rust
//! ResponseChecker — LLM 自评信号解析 + 越界检测 + 路由决策。
//!
//! 纯函数模块，不调 LLM，只做文本解析和规则判断。
//!
//! 容错策略: 宽松正则匹配 → 降级 → 默认行为（乐观自动推进）。

use regex::Regex;

use super::app::{AppConfig, Milestone};

/// LLM 自评信号
#[derive(Debug, Clone, PartialEq)]
pub enum CompletionSignal {
    Done,
    More { reason: String },
    Blocked { reason: String },
}

/// 路由决策
#[derive(Debug, Clone, PartialEq)]
pub enum NextAction {
    /// 自动推进到下一里程碑
    Advance,
    /// 停住，等待用户输入
    WaitForUser,
    /// 保持当前里程碑，将 reason 注入下一轮 prompt
    Continue { reason: String },
    /// 阻断，展示原因给用户
    Block { reason: String },
}

/// 检查结果
#[derive(Debug, Clone)]
pub struct CheckResult {
    pub out_of_scope: bool,
    pub safe_content: Option<String>,
    pub signal: Option<CompletionSignal>,
    pub next_action: NextAction,
}

pub struct ResponseChecker;

impl ResponseChecker {
    /// 主入口
    pub fn check(
        response: &str,
        current_milestone: &Milestone,
        app_config: &AppConfig,
    ) -> CheckResult {
        let signal = Self::parse_signal(response);
        let (out_of_scope, safe_content) = Self::check_out_of_scope(response, current_milestone);
        let next_action = Self::decide(&signal, current_milestone, app_config, out_of_scope);

        CheckResult {
            out_of_scope,
            safe_content,
            signal,
            next_action,
        }
    }

    /// 解析 LLM 回复中的自评信号（只检查最后 500 字符）
    fn parse_signal(response: &str) -> Option<CompletionSignal> {
        let tail: String = response
            .chars()
            .rev()
            .take(500)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        // [BLOCKED] — 最高优先级
        let re_blocked = Regex::new(
            r"(?i)\[BLOCKED\]\s*(?:原因[:：]\s*)?(?P<reason>.{1,200})?$",
        )
        .unwrap();
        if let Some(caps) = re_blocked.captures(&tail) {
            return Some(CompletionSignal::Blocked {
                reason: caps
                    .name("reason")
                    .map(|m| m.as_str().trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "未说明".into()),
            });
        }

        // 中文变体: [阻塞] / 卡住了: / 无法继续:
        let re_blocked_cn = Regex::new(
            r"(?:\[阻塞\]|卡住了[：:]?\s*|无法继续[：:]?\s*)(?P<reason>.{1,200})?$",
        )
        .unwrap();
        if let Some(caps) = re_blocked_cn.captures(&tail) {
            return Some(CompletionSignal::Blocked {
                reason: caps
                    .name("reason")
                    .map(|m| m.as_str().trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "未说明".into()),
            });
        }

        // [MORE]
        let re_more = Regex::new(
            r"(?i)\[MORE\]\s*(?:还需要[:：]\s*)?(?P<reason>.{1,200})?$",
        )
        .unwrap();
        if let Some(caps) = re_more.captures(&tail) {
            return Some(CompletionSignal::More {
                reason: caps
                    .name("reason")
                    .map(|m| m.as_str().trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "需要继续".into()),
            });
        }

        // 中文变体: [继续] / 还需要: / 还没完:
        let re_more_cn = Regex::new(
            r"(?:\[继续\]|还需要[：:]?\s*|还没完[：:]?\s*)(?P<reason>.{1,200})?$",
        )
        .unwrap();
        if let Some(caps) = re_more_cn.captures(&tail) {
            return Some(CompletionSignal::More {
                reason: caps
                    .name("reason")
                    .map(|m| m.as_str().trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "需要继续".into()),
            });
        }

        // [OK]
        let re_ok = Regex::new(r"(?i)\[OK\]|\[完成\]|\[✓\]").unwrap();
        if re_ok.is_match(&tail) {
            return Some(CompletionSignal::Done);
        }

        // 弱信号: 单独一行的 "完成。" 在末尾
        if tail.trim_end().ends_with("完成。") {
            return Some(CompletionSignal::Done);
        }

        None
    }

    /// 越界检测（纯机械规则）
    fn check_out_of_scope(response: &str, milestone: &Milestone) -> (bool, Option<String>) {
        let label = &milestone.label;
        let hint = milestone.prompt_hint.as_deref().unwrap_or("");
        let char_count = response.chars().count();

        // 规则 1: 该「问」的阶段，LLM 却「写」了大量内容
        let is_asking_phase = (label.contains("需求")
            || label.contains("确认")
            || label.contains("收集"))
            && (hint.contains("问") || hint.contains("确认"));
        let has_question = response.contains('?') || response.contains('？');

        if is_asking_phase && char_count > 300 && !has_question {
            let safe: String = response.chars().take(150).collect();
            return (true, Some(safe));
        }

        // 规则 2: LLM 生成了完成品但不在生成阶段
        let paragraphs: Vec<&str> = response
            .split("\n\n")
            .filter(|p| !p.trim().is_empty())
            .collect();
        let is_generation_phase = label.contains("生成")
            || label.contains("草稿")
            || label.contains("撰写")
            || hint.contains("生成")
            || hint.contains("输出");

        if paragraphs.len() >= 3 && !is_generation_phase {
            let safe = paragraphs
                .into_iter()
                .take(2)
                .collect::<Vec<_>>()
                .join("\n\n");
            return (true, Some(safe));
        }

        // 规则 3: 安全
        (false, None)
    }

    /// 路由决策
    fn decide(
        signal: &Option<CompletionSignal>,
        milestone: &Milestone,
        app_config: &AppConfig,
        out_of_scope: bool,
    ) -> NextAction {
        // 越界优先
        if out_of_scope {
            let phase_desc = &milestone.label;
            return NextAction::Continue {
                reason: format!("只做 {phase_desc}，绝对不要做超出范围的事"),
            };
        }

        match signal {
            Some(CompletionSignal::Blocked { reason }) => NextAction::Block {
                reason: reason.clone(),
            },

            Some(CompletionSignal::More { reason }) => NextAction::Continue {
                reason: reason.clone(),
            },

            Some(CompletionSignal::Done) => {
                // confirm_at 优先
                if app_config.milestones.iter().any(|m| m.id == milestone.id) {
                    // confirm_at 中的里程碑 → 等用户
                    // (confirm_at 是预定义里程碑列表，命中则等待)
                    return NextAction::WaitForUser;
                }

                match app_config.model_preference.as_str() {
                    // fine granularity → 每步等确认
                    "fine" => NextAction::WaitForUser,
                    _ => NextAction::Advance,
                }
            }

            None => {
                // 无信号时降级策略
                match app_config.model_preference.as_str() {
                    "fine" => NextAction::WaitForUser,
                    _ => NextAction::Advance,
                }
            }
        }
    }
}
```

- [ ] **Step 2: Write comprehensive tests**

Add at end of file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use super::super::app::{AppConfig, Milestone};

    fn milestone(label: &str, hint: &str) -> Milestone {
        Milestone {
            id: "test".into(),
            label: label.into(),
            prompt_hint: Some(hint.into()),
            icon: None,
        }
    }

    fn app(pref: &str) -> AppConfig {
        AppConfig {
            id: "test".into(),
            name: "Test".into(),
            description: "".into(),
            icon: "".into(),
            prompt_file: None,
            prompt: None,
            model_preference: pref.into(),
            tools: vec![],
            milestones: vec![],
            meta: Default::default(),
        }
    }

    // === Signal parsing ===

    #[test]
    fn test_parse_signal_ok() {
        let s = ResponseChecker::parse_signal("这是草稿。[OK]");
        assert_eq!(s, Some(CompletionSignal::Done));
    }

    #[test]
    fn test_parse_signal_ok_cn() {
        let s = ResponseChecker::parse_signal("草稿已完成。[完成]");
        assert_eq!(s, Some(CompletionSignal::Done));
    }

    #[test]
    fn test_parse_signal_ok_weak() {
        let s = ResponseChecker::parse_signal("草稿生成完成。");
        assert_eq!(s, Some(CompletionSignal::Done));
    }

    #[test]
    fn test_parse_signal_more() {
        let s = ResponseChecker::parse_signal("做了一半。[MORE] 还需要: 修改第三段");
        assert!(matches!(s, Some(CompletionSignal::More { .. })));
        if let Some(CompletionSignal::More { reason }) = s {
            assert!(reason.contains("修改第三段"));
        }
    }

    #[test]
    fn test_parse_signal_more_cn() {
        let s = ResponseChecker::parse_signal("还需要: 补充数据来源");
        assert!(matches!(s, Some(CompletionSignal::More { .. })));
    }

    #[test]
    fn test_parse_signal_blocked() {
        let s = ResponseChecker::parse_signal("找不到文件。[BLOCKED] 原因: 文件不存在");
        assert!(matches!(s, Some(CompletionSignal::Blocked { .. })));
        if let Some(CompletionSignal::Blocked { reason }) = s {
            assert!(reason.contains("文件不存在"));
        }
    }

    #[test]
    fn test_parse_signal_blocked_cn() {
        let s = ResponseChecker::parse_signal("[阻塞] 原因: 权限不足");
        assert!(matches!(s, Some(CompletionSignal::Blocked { .. })));
    }

    #[test]
    fn test_parse_signal_missing() {
        let s = ResponseChecker::parse_signal("这是普通回复，没有信号。");
        assert_eq!(s, None);
    }

    #[test]
    fn test_parse_signal_blocked_priority_over_ok() {
        // 如果 LLM 同时输出 [OK] 和 [BLOCKED]，优先 [BLOCKED]
        let s = ResponseChecker::parse_signal("[OK] 但实际上 [BLOCKED] 原因: 出错了");
        assert!(matches!(s, Some(CompletionSignal::Blocked { .. })));
    }

    // === Out-of-scope ===

    #[test]
    fn test_out_of_scope_asking_phase_generates_content() {
        let m = milestone("明确需求", "确认文档类型，只问不写");
        let resp = "好的，以下是您的周报：\n\n本周工作总结\n\n本周完成了供应链迁移工作，涉及3家供应商审核...\n\n（下略500字）";
        let (out, safe) = ResponseChecker::check_out_of_scope(resp, &m);
        assert!(out);
        assert!(safe.unwrap().chars().count() <= 200);
    }

    #[test]
    fn test_out_of_scope_asking_phase_with_question_safe() {
        let m = milestone("明确需求", "确认文档类型，只问不写");
        let resp = "好的，请问您需要什么类型的文档？是内部汇报还是对外发布？想要正式风格还是口语化？请告诉我更多细节。";
        let (out, _) = ResponseChecker::check_out_of_scope(resp, &m);
        assert!(!out); // 有问号 → 安全
    }

    #[test]
    fn test_out_of_scope_generation_phase_safe() {
        let m = milestone("生成草稿", "输出完整周报草稿");
        let resp = "\n\n第一段：本周工作\n\nA、B、C\n\n第二段：下周计划\n\nD、E\n\n第三段：问题风险\n\n无";
        let (out, _) = ResponseChecker::check_out_of_scope(resp, &m);
        assert!(!out); // 生成阶段 → 安全
    }

    #[test]
    fn test_out_of_scope_multi_paragraph_not_generating() {
        let m = milestone("明确需求", "确认需求");
        let resp = "\n\n段落一：大量内容\n\n段落二：更多内容\n\n段落三：继续写\n\n段落四：还在写";
        let (out, _) = ResponseChecker::check_out_of_scope(resp, &m);
        assert!(out); // 3+ 段落但不在生成阶段
    }

    // === Routing decisions ===

    #[test]
    fn test_decide_advance_on_ok_medium() {
        let action = ResponseChecker::decide(
            &Some(CompletionSignal::Done),
            &milestone("分析", "分析数据"),
            &app("medium"),
            false,
        );
        assert_eq!(action, NextAction::Advance);
    }

    #[test]
    fn test_decide_wait_on_ok_fine() {
        let action = ResponseChecker::decide(
            &Some(CompletionSignal::Done),
            &milestone("审阅", "审阅草稿"),
            &app("fine"),
            false,
        );
        assert_eq!(action, NextAction::WaitForUser);
    }

    #[test]
    fn test_decide_block_on_blocked() {
        let action = ResponseChecker::decide(
            &Some(CompletionSignal::Blocked {
                reason: "权限不足".into(),
            }),
            &milestone("保存", "保存文件"),
            &app("medium"),
            false,
        );
        assert!(matches!(action, NextAction::Block { .. }));
        if let NextAction::Block { reason } = action {
            assert!(reason.contains("权限不足"));
        }
    }

    #[test]
    fn test_decide_continue_on_more() {
        let action = ResponseChecker::decide(
            &Some(CompletionSignal::More {
                reason: "还需补充数据".into(),
            }),
            &milestone("分析", "分析数据"),
            &app("medium"),
            false,
        );
        assert!(matches!(action, NextAction::Continue { .. }));
    }

    #[test]
    fn test_decide_default_fine_no_signal() {
        let action = ResponseChecker::decide(
            &None,
            &milestone("审阅", "审阅草稿"),
            &app("fine"),
            false,
        );
        assert_eq!(action, NextAction::WaitForUser);
    }

    #[test]
    fn test_decide_default_medium_no_signal() {
        let action = ResponseChecker::decide(
            &None,
            &milestone("分析", "分析数据"),
            &app("medium"),
            false,
        );
        assert_eq!(action, NextAction::Advance);
    }

    #[test]
    fn test_decide_out_of_scope_overrides() {
        let action = ResponseChecker::decide(
            &Some(CompletionSignal::Done),
            &milestone("明确需求", "只问不写"),
            &app("medium"),
            true, // out_of_scope
        );
        assert!(matches!(action, NextAction::Continue { .. }));
    }
}
```

- [ ] **Step 3: Run tests to verify**

```bash
cargo test --manifest-path /home/hexin/opencode_projects/pinvou3/pinvou-platform/Cargo.toml -- response_checker 2>&1 | tail -30
```

Expected: All 21 tests PASS

- [ ] **Step 4: Register in mod.rs**

Edit `pinvou-platform/src/lib.rs`, add:

```rust
pub mod response_checker;
```

- [ ] **Step 5: Verify compilation**

```bash
cargo check --manifest-path /home/hexin/opencode_projects/pinvou3/pinvou-platform/Cargo.toml 2>&1 | tail -5
```

Expected: `Finished`

- [ ] **Step 6: Commit**

```bash
git add pinvou-platform/src/response_checker.rs pinvou-platform/src/lib.rs
git commit -m "feat: add ResponseChecker — [OK]/[MORE]/[BLOCKED] signal parsing and out-of-scope detection"
```

---

### Task 5: Create `LLMReviewer` — semantic review of decomposition

**Files:**
- Create: `pinvou-platform/src/reviewer.rs`
- Modify: `pinvou-platform/src/lib.rs`

- [ ] **Step 1: Write module with review logic and fallback parsing**

Create `pinvou-platform/src/reviewer.rs`:

```rust
//! LLMReviewer — 调用 LLM 对拆解结果做语义审阅。
//!
//! 审阅比拆解简单，本地模型能做。
//! JSON 解析失败 → 正则降级 → 再失败默认 ok=true 放行（用户确认为安全网）。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use regex::Regex;

use super::harness::AgentHarness;
use super::step_builder::StepBuilder;

/// 审阅结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewResult {
    pub ok: bool,
    pub issues: Vec<Issue>,
    pub overall: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub step: Option<u32>,
    pub problem: String,
    pub suggestion: String,
}

pub struct LLMReviewer;

impl LLMReviewer {
    /// 审阅拆解结果
    pub async fn review(
        harness: &dyn AgentHarness,
        decomposition: &str,
        user_request: &str,
        available_tools: &[String],
    ) -> Result<ReviewResult> {
        let prompt = StepBuilder::build_review_prompt(decomposition, user_request, available_tools);

        let response = harness
            .chat(super::harness::ChatRequest {
                user_message: prompt,
                platform_system_prompt: Some("你是一个任务审阅员。只输出 JSON，不要输出其他内容。".into()),
                context: Default::default(),
                tools: vec![],
                model: None,
                session_id: None,
            })
            .await?;

        Ok(Self::parse_review_response(&response))
    }

    /// 三层降级解析
    fn parse_review_response(text: &str) -> ReviewResult {
        // 层 1: 标准 JSON 解析
        // 提取 JSON 块（可能被 markdown 代码块包裹）
        let json_text = if let Some(start) = text.find("```json") {
            let inner = &text[start + 7..];
            if let Some(end) = inner.find("```") {
                inner[..end].trim().to_string()
            } else {
                inner.trim().to_string()
            }
        } else if let Some(start) = text.find('{') {
            if let Some(end) = text.rfind('}') {
                text[start..=end].to_string()
            } else {
                text.to_string()
            }
        } else {
            text.to_string()
        };

        if let Ok(result) = serde_json::from_str::<ReviewResult>(&json_text) {
            return result;
        }

        // 层 2: 正则降级
        Self::fallback_parse(text)
    }

    fn fallback_parse(text: &str) -> ReviewResult {
        // 提取 ok
        let ok = !text.contains("\"ok\": false")
            && !text.contains("\"ok\":false")
            && !text.contains("ok: false")
            && !text.contains("ok:false");

        // 提取 issues
        let issues = Self::extract_issues_fallback(text);

        // 提取 overall
        let re_overall = Regex::new(r#"overall"[:\s]*"([^"]+)""#).unwrap();
        let overall = re_overall
            .captures(text)
            .map(|c| c[1].to_string())
            .unwrap_or_else(|| {
                if ok {
                    "审阅通过（解析降级）".to_string()
                } else {
                    "审阅未通过（解析降级），请人工确认".to_string()
                }
            });

        ReviewResult {
            ok,
            issues,
            overall,
        }
    }

    fn extract_issues_fallback(text: &str) -> Vec<Issue> {
        let mut issues = Vec::new();

        // 尝试匹配 "step": N, "problem": "...", "suggestion": "..."
        let re_block = Regex::new(
            r#""step"[:\s]*(\d+)[^}]*"problem"[:\s]*"([^"]+)"[^}]*"suggestion"[:\s]*"([^"]+)""#
        ).unwrap();

        for caps in re_block.captures_iter(text) {
            issues.push(Issue {
                step: caps.get(1).and_then(|m| m.as_str().parse().ok()),
                problem: caps.get(2).map(|m| m.as_str().to_string()).unwrap_or_default(),
                suggestion: caps.get(3).map(|m| m.as_str().to_string()).unwrap_or_default(),
            });
        }

        // 如果正则也没匹配到，尝试按行匹配 "步骤 N: 问题 - 建议"
        if issues.is_empty() {
            let re_loose = Regex::new(r"步骤\s*(\d+)[:：]\s*(\S+)\s*[-–—]\s*(\S+)").unwrap();
            for caps in re_loose.captures_iter(text) {
                issues.push(Issue {
                    step: caps.get(1).and_then(|m| m.as_str().parse().ok()),
                    problem: caps.get(2).map(|m| m.as_str().to_string()).unwrap_or_default(),
                    suggestion: caps.get(3).map(|m| m.as_str().to_string()).unwrap_or_default(),
                });
            }
        }

        issues
    }

    /// 审阅结果 → 自然语言反馈（注入重拆 prompt）
    pub fn format_feedback(result: &ReviewResult) -> String {
        let mut feedback = String::from("审阅意见：\n");
        for issue in &result.issues {
            if let Some(step) = issue.step {
                feedback.push_str(&format!(
                    "- 步骤 {step}: {}, 建议: {}\n",
                    issue.problem, issue.suggestion
                ));
            } else {
                feedback.push_str(&format!(
                    "- {}, 建议: {}\n",
                    issue.problem, issue.suggestion
                ));
            }
        }
        feedback.push_str(&format!(
            "\n总体评价: {}\n请根据以上反馈修改拆解。",
            result.overall
        ));
        feedback
    }
}
```

- [ ] **Step 2: Write tests**

Add at end of file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_json() {
        let json = r#"{"ok":false,"issues":[{"step":2,"problem":"太笼统","suggestion":"改为具体步骤"}],"overall":"需要修改"}"#;
        let result = LLMReviewer::parse_review_response(json);
        assert!(!result.ok);
        assert_eq!(result.issues.len(), 1);
        assert_eq!(result.issues[0].step, Some(2));
    }

    #[test]
    fn test_parse_json_in_code_block() {
        let text = "```json\n{\"ok\":true,\"issues\":[],\"overall\":\"很好\"}\n```";
        let result = LLMReviewer::parse_review_response(text);
        assert!(result.ok);
        assert!(result.issues.is_empty());
    }

    #[test]
    fn test_parse_malformed_json_fallback() {
        let text = "ok: false\n问题在步骤2，太笼统，建议改为具体步骤\noverall: 需要改进";
        let result = LLMReviewer::parse_review_response(text);
        // 降级解析: ok 默认为 true (因为没找到 "ok": false，格式是 "ok: false")
        // 但 "ok: false" 被 fallback_parse 的第一条规则匹配
        assert!(!result.ok);
    }

    #[test]
    fn test_parse_ungrammatical_default_ok() {
        let text = "看起来还不错，拆解得挺好的。";
        let result = LLMReviewer::parse_review_response(text);
        assert!(result.ok); // 降级默认放行
        assert!(result.issues.is_empty());
    }

    #[test]
    fn test_format_feedback() {
        let result = ReviewResult {
            ok: false,
            issues: vec![
                Issue {
                    step: Some(1),
                    problem: "太笼统".into(),
                    suggestion: "改为具体步骤".into(),
                },
            ],
            overall: "需要修改".into(),
        };
        let feedback = LLMReviewer::format_feedback(&result);
        assert!(feedback.contains("步骤 1"));
        assert!(feedback.contains("太笼统"));
        assert!(feedback.contains("改为具体步骤"));
        assert!(feedback.contains("需要修改"));
    }

    #[test]
    fn test_format_feedback_all_ok() {
        let result = ReviewResult {
            ok: true,
            issues: vec![],
            overall: "拆解很好".into(),
        };
        let feedback = LLMReviewer::format_feedback(&result);
        assert!(feedback.contains("拆解很好"));
    }
}
```

- [ ] **Step 3: Run tests to verify**

```bash
cargo test --manifest-path /home/hexin/opencode_projects/pinvou3/pinvou-platform/Cargo.toml -- reviewer 2>&1 | tail -20
```

Expected: All 6 tests PASS

- [ ] **Step 4: Register in mod.rs**

Edit `pinvou-platform/src/lib.rs`, add:

```rust
pub mod reviewer;
```

- [ ] **Step 5: Verify compilation**

```bash
cargo check --manifest-path /home/hexin/opencode_projects/pinvou3/pinvou-platform/Cargo.toml 2>&1 | tail -5
```

Expected: `Finished`

- [ ] **Step 6: Commit**

```bash
git add pinvou-platform/src/reviewer.rs pinvou-platform/src/lib.rs
git commit -m "feat: add LLMReviewer — semantic review of LLM decomposition with fallback parsing"
```

---

### Task 6: Integrate modules into `PlatformEngine`

**Files:**
- Modify: `pinvou-platform/src/engine.rs`

- [ ] **Step 1: Add new methods to PlatformEngine**

Add to the `impl<H: AgentHarness> PlatformEngine<H>` block (before the `// === Mock AgentHarness` comment):

```rust
    /// 添加连续越界计数器
    pub consecutive_out_of_scope: u32,
```

Actually this is a field, not a method. Let me write the full modified engine.rs additions.

After `pub fn available_models(&self) -> Vec<ModelInfo>` (line ~174), add:

```rust
    /// 任务拆解 + 逐步执行（完整编排流程）
    ///
    /// 返回: (拆解文本, 是否已确认/执行中)
    pub async fn decompose_and_execute(
        &mut self,
        user_message: &str,
    ) -> Result<DecomposeResult> {
        use super::step_builder::StepBuilder;
        use super::reviewer::LLMReviewer;
        use super::response_checker::ResponseChecker;

        let app = self.current_app.as_ref()
            .ok_or_else(|| anyhow::anyhow!("没有加载应用"))?;

        // Step 1: 拆解
        let tools: Vec<String> = self.resolve_tools().into_iter().map(|t| t.name).collect();
        let context_summary = self.conv_state.as_ref()
            .and_then(|cs| cs.context_prompt())
            .unwrap_or_default();

        let decomp_prompt = StepBuilder::build_decomposition(
            user_message,
            app,
            &tools,
            &context_summary,
        );

        let decomposition = self.harness.chat(ChatRequest {
            user_message: decomp_prompt,
            platform_system_prompt: Some(
                "你是一个任务拆解专家。用中文回复。只输出步骤列表。".into()
            ),
            context: Default::default(),
            tools: vec![],
            model: Some(app.model_preference.clone()),
            session_id: None,
        }).await?;

        // Step 2: 可解析性检查
        let mut retries = 0;
        let mut final_decomposition = decomposition;
        let mut review_result = None;

        loop {
            if !Self::parsability_check(&final_decomposition) {
                if retries >= 2 {
                    // 降级到 fallback milestones
                    return Ok(DecomposeResult {
                        decomposition: "（使用应用预定义步骤）".into(),
                        review_passed: true,
                        milestone_count: app.milestones.len(),
                    });
                }
                // 重拆
                let retry_prompt = format!(
                    "上次输出格式有误，请重新输出。每行格式: \"N. {{具体动词+具体对象+明确产出}}\"\n\n用户需求: {user_message}",
                );
                final_decomposition = self.harness.chat(ChatRequest {
                    user_message: retry_prompt,
                    platform_system_prompt: Some("只输出步骤列表。".into()),
                    context: Default::default(),
                    tools: vec![],
                    model: Some(app.model_preference.clone()),
                    session_id: None,
                }).await?;
                retries += 1;
                continue;
            }

            // Step 3: LLM 语义审阅
            let review = LLMReviewer::review(
                &self.harness,
                &final_decomposition,
                user_message,
                &tools,
            ).await?;

            if review.ok {
                review_result = Some(review);
                break;
            }

            if retries >= 2 {
                review_result = Some(review); // 不过但不再重试，留给用户判断
                break;
            }

            // 重拆（注入反馈）
            let feedback = LLMReviewer::format_feedback(&review);
            let retry_prompt = format!(
                "上次拆解有这些问题:\n{feedback}\n\n用户需求: {user_message}\n\n请重新拆解。",
            );
            final_decomposition = self.harness.chat(ChatRequest {
                user_message: retry_prompt,
                platform_system_prompt: Some("你是一个任务拆解专家。用中文回复。只输出步骤列表。".into()),
                context: Default::default(),
                tools: vec![],
                model: Some(app.model_preference.clone()),
                session_id: None,
            }).await?;
            retries += 1;
        }

        let review = review_result.unwrap_or(ReviewResult {
            ok: true,
            issues: vec![],
            overall: "审阅跳过".into(),
        });

        Ok(DecomposeResult {
            decomposition: final_decomposition,
            review_passed: review.ok,
            milestone_count: review.issues.len(),
        })
    }

    /// 可解析性检查: 非空 + 有数字序号 + 能提取行
    fn parsability_check(text: &str) -> bool {
        if text.trim().is_empty() {
            return false;
        }

        // 检查是否有数字序号行（如 "1. xxx" 或 "1) xxx" 或 "步骤1"）
        let re = regex::Regex::new(r"(?m)^\s*\d+[\.\)、]").unwrap();
        let has_numbers = re.is_match(text);

        if !has_numbers {
            // 也尝试匹配 "步骤 N" 格式
            let re2 = regex::Regex::new(r"步骤\s*\d+").unwrap();
            if !re2.is_match(text) {
                return false;
            }
        }

        // 至少能提取 1 个非空步骤行
        let lines: Vec<&str> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect();

        lines.len() >= 1
    }

    /// 对当前活跃里程碑执行逐步执行
    pub async fn step_execute(
        &mut self,
    ) -> Result<Option<super::response_checker::NextAction>> {
        use super::step_builder::StepBuilder;
        use super::response_checker::ResponseChecker;

        let app = self.current_app.as_ref()
            .ok_or_else(|| anyhow::anyhow!("没有加载应用"))?;

        let milestone = match self.conv_state.as_ref().and_then(|cs| cs.active_milestone()) {
            Some(m) => m.clone(),
            None => return Ok(None), // 所有里程碑完成
        };

        let context = self.conv_state.as_ref()
            .map(|cs| cs.context.clone())
            .unwrap_or_default();

        // 构造小范围 prompt
        let step_prompt = StepBuilder::build(
            &milestone,
            &context,
            "", // user_message 已嵌入 system
            app,
        );

        let response = self.harness.chat(ChatRequest {
            user_message: "".into(), // prompt 在 system 里
            platform_system_prompt: Some(step_prompt.system),
            context: context.clone(),
            tools: self.resolve_tools(),
            model: Some(app.model_preference.clone()),
            session_id: self.conv_state.as_ref().map(|cs| {
                format!("{}-{}", cs.app_id, cs.turn_count)
            }),
        }).await?;

        // 检查响应
        let check = ResponseChecker::check(&response, &milestone, app);

        // 处理越界
        if check.out_of_scope {
            // 跟踪连续越界
            // (consecutive_out_of_scope 字段在 PlatformEngine 上)
            if check.safe_content.is_some() {
                // 可以在这里记录截断后的内容
            }
        }

        // 更新上下文
        self.extract_context_from_response(&response);

        // 根据路由决策更新里程碑
        match &check.next_action {
            super::response_checker::NextAction::Advance => {
                if let Some(ref mut cs) = self.conv_state {
                    cs.mark_done(&milestone.id);
                }
            }
            super::response_checker::NextAction::Block { .. } => {
                // 不推进，等用户处理
            }
            super::response_checker::NextAction::WaitForUser => {
                // 不自动推进
            }
            super::response_checker::NextAction::Continue { .. } => {
                // 保持当前里程碑
            }
        }

        if let Some(ref mut cs) = self.conv_state {
            cs.increment_turn();
        }

        Ok(Some(check.next_action))
    }
}

/// 拆解结果
#[derive(Debug, Clone)]
pub struct DecomposeResult {
    pub decomposition: String,
    pub review_passed: bool,
    pub milestone_count: usize,
}
```

Also add the import for `ReviewResult` at the top of engine.rs:

```rust
use super::reviewer::ReviewResult;
```

- [ ] **Step 2: Update engine.rs imports**

At the top of `pinvou-platform/src/engine.rs`, the existing imports already include `use super::app::{AppConfig, AppRegistry};` and `use super::harness::...`. Verify the regex import exists (it's used inline with `regex::Regex`).

- [ ] **Step 3: Add consecutive_out_of_scope field**

In the `PlatformEngine` struct definition, add after `pub workspace: PathBuf,`:

Wait — `PlatformEngine` derive doesn't include Default, but adding a simple field works. Let me place it correctly. Add after `pub workspace: PathBuf,` (line 29):

```rust
    /// 连续越界计数器
    pub consecutive_out_of_scope: u32,
```

And initialize it in `new()` after `workspace,`:

```rust
            consecutive_out_of_scope: 0,
```

- [ ] **Step 4: Run tests to verify**

```bash
cargo test --manifest-path /home/hexin/opencode_projects/pinvou3/pinvou-platform/Cargo.toml -- engine 2>&1 | tail -20
```

Expected: All existing engine tests still PASS

- [ ] **Step 5: Verify compilation**

```bash
cargo check --manifest-path /home/hexin/opencode_projects/pinvou3/pinvou-platform/Cargo.toml 2>&1 | tail -10
```

Expected: `Finished` (may have warnings about unused methods — acceptable at this stage)

- [ ] **Step 6: Commit**

```bash
git add pinvou-platform/src/engine.rs
git commit -m "feat: add decompose_and_execute + step_execute to PlatformEngine"
```

---

### Task 7: Wire TUI to real engine

**Files:**
- Modify: `pinvou-platform/src/tui/ui.rs`
- Modify: `pinvou-platform/src/tui/app.rs`

- [ ] **Step 1: Add engine field to PlatformApp**

Edit `pinvou-platform/src/tui/app.rs`. Add after the existing `pub consecutive_out_of_scope: u32,` doesn't exist yet. Instead, add to the `PlatformApp` struct (near `pub current_model: String,`):

```rust
    /// 连续越界计数器
    pub consecutive_out_of_scope: u32,
```

Initialize in `PlatformApp::new()`:

```rust
            consecutive_out_of_scope: 0,
```

- [ ] **Step 2: Replace simulate_engine_response in ui.rs**

In `pinvou-platform/src/tui/ui.rs`, replace the `simulate_engine_response` function and its call site.

Replace the call in `handle_input_key` (around line 340-344):

OLD:
```rust
            // Phase 2: 模拟 AI 响应（后续对接真实引擎）
            app.engine_status = EngineStatus::Thinking;

            // 模拟处理
            simulate_engine_response(app);
```

NEW:
```rust
            // Phase 3: 使用真实编排引擎
            app.engine_status = EngineStatus::Thinking;
            // 注意: 真实引擎需要 async，这里用同步占位
            // Phase 4 会改为异步流式
            let response = format!(
                "[Phase 3] 收到你的消息。当前应用: {}。拆解和逐步执行将在 Engine 集成完成后生效。",
                app.current_app.as_ref().map(|a| a.name.as_str()).unwrap_or("未选择")
            );
            app.add_assistant_message(response);
            app.engine_status = EngineStatus::Idle;
```

(Note: This is an intermediate state — the real async engine wiring happens in Phase 4 when streaming is added. For now, we replace the hardcoded per-app responses with a single placeholder that acknowledges the Phase 3 architecture.)

- [ ] **Step 3: Remove simulate_engine_response and check_milestone_triggers**

Delete the `simulate_engine_response` function (lines 443-462) and `check_milestone_triggers` function (lines 464-482).

- [ ] **Step 4: Verify compilation**

```bash
cargo check --manifest-path /home/hexin/opencode_projects/pinvou3/pinvou-platform/Cargo.toml 2>&1 | tail -10
```

Expected: `Finished`

- [ ] **Step 5: Run the full test suite**

```bash
cd /home/hexin/opencode_projects/pinvou3/DeepSeek-TUI && cargo test -p deepseek-tui 2>&1 | tail -30
```

Expected: All tests PASS (no regressions)

- [ ] **Step 6: Commit**

```bash
git add pinvou-platform/src/tui/ui.rs pinvou-platform/src/tui/app.rs
git commit -m "feat: replace simulate_engine_response with Phase 3 placeholder, add consecutive_out_of_scope tracking"
```

---

## Verification

After all tasks complete, run:

```bash
cd /home/hexin/opencode_projects/pinvou3/DeepSeek-TUI
cargo test -p deepseek-tui 2>&1 | grep -E "test result|FAILED"
```

Expected output:
```
test result: ok.  <N> passed; 0 failed; 0 ignored
```

Then manually run the TUI (requires local Ollama or vLLM for full end-to-end):

```bash
cargo run --bin pinvou-platform -- --apps-dir apps/
```

Verify:
1. Launcher shows 3 apps ✓
2. Can select an app and enter conversation ✓
3. Sending a message gets a response ✓
4. Sidebar displays milestones ✓
5. Manual milestone operations (Enter to mark done, s to skip) still work ✓

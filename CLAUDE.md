# pinvou3 项目规则

> 架构详情见 `设计架构文档-pinvou3.md`。当前进度与 P1 待办见 `process.md`。
> 本文档只列做事规则，不重复架构说明。

---

## 边界

**pinvou-platform 是编排层，DeepSeek-TUI 是底层。** 唯一接口是 `pinvou-platform/src/harness.rs` 的 `AgentHarness` trait。

只有 `deepseek_harness.rs` 这个桥接模块可以 `use deepseek_tui::*`。其他 platform 模块走 trait + 自定义类型（`ChatRequest` / `StreamEvent` / `ToolDef`）。

---

## 复用优先

加任何能力前先看 DeepSeek-TUI 有没有：

| 想加什么 | 先看 |
|---|---|
| 工具实现（联网 / 文件 / shell 等） | `DeepSeek-TUI/crates/tui/src/tools/` |
| LLM 客户端 | `crates/tui/src/client.rs` |
| 流式事件 / 消息块 / Tool schema | `crates/tui/src/models.rs`、`tools/spec.rs` |
| TUI 模态框（选择卡等） | `crates/tui/src/tui/` |
| Sandbox / workspace 信任 | `crates/tui/src/sandbox.rs`、`workspace_trust.rs` |

只有 DeepSeek-TUI 没有的能力才在 platform 新增。

---

## 修改 DeepSeek-TUI

**非必要不动。** 真要改：
- 先尝试在 platform 包装绕过
- 改之前确认上游能接受（避免之后 rebase 痛苦）
- 在 PR / 提交说明里写清动机
- 不要把它的代码 fork 进 platform

---

## 常见错误

- ❌ 在拆解 prompt 里宣传 harness 未注册的工具 → LLM 输出 `[web_search: ...]` 这类伪文本
- ❌ 绕过 `ContractValidator` 让 LLM 自评推进 → 本地 LLM 不会自觉停
- ❌ 在 `ConversationState` 外维护对话状态 → 回退（`/back` / `/redo`）清不干净
- ❌ 自己定义工具协议 → 用 DeepSeek-TUI 的 `ToolSpec` + `ToolRegistryBuilder`
- ❌ 给 `AgentHarness` trait 加 DeepSeek-TUI 内部类型 → 破坏可替换性

---

## 常见任务

- **加 agent** = 写一个 `prompts/<id>.md`（含 frontmatter）。零代码，启动时自动扫描。
- **加工具** = `engine_factory::build_default_tool_registry` 加一行 `.with_tool(Arc::new(SomeTool))` + 把工具名加进 `auto_tool_names`（若可自动执行）
- **加 milestone mode** = 三处协同更新：`contract::contract_for_mode`（规则）+ `contract_runtime::next_directive`（路由）+ `combined_planner` 拆解 prompt（让 LLM 知道）

---

## 验证

```
cargo test --manifest-path pinvou-platform/Cargo.toml --lib
```

改架构性的东西要同步更新 `设计架构文档-pinvou3.md`；完成 P1 项要更新 `process.md`。

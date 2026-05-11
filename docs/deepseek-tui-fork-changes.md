# DeepSeek-TUI Fork 改动总结

> pinvou3 对 DeepSeek-TUI 的 fork 变更文档。
> 本文档用于：拉取上游最新代码后重新应用改动，或切换其他底层 Agent 时的移植指南。

---

## 一、架构背景

pinvou3 采用两层架构：

```
+------------------------------------------+
|         pinvou-platform (独立 crate)       |
|                                           |
|  PlatformEngine — 任务编排 + 对话状态机     |
|  AppConfig/AppRegistry — 应用即配置系统     |
|  TUI — 启动器 + 对话 + 侧边栏              |
|                                           |
|  ┌─────────────────────────────┐          |
|  │  AgentHarness trait (边界)   │          |
|  └──────────────┬──────────────┘          |
+-----------------|-------------------------+
                  | path dependency
                  v
+-----------------|-------------------------+
| DeepSeek-TUI     | (fork，只读)            |
|                  |                        |
|  lib.rs — 模块声明 + CliAutoRoute 镜像      |
|  llm_client/ — LlmClient trait            |
|  client.rs — DeepSeekClient               |
|  models.rs — MessageRequest 等            |
|  tools/ — ToolRegistry                    |
|  config.rs — Config                       |
|  ...                                      |
+-------------------------------------------+
```

**核心原则**：DeepSeek-TUI 是 pinvou-platform 的 library dependency，通过 `pinvou-platform/Cargo.toml` 中的 path 依赖引入：

```toml
deepseek-tui = { path = "../DeepSeek-TUI/crates/tui" }
```

**为什么需要改动 DeepSeek-TUI？**
- DeepSeek-TUI 原本只有 `main.rs`（二进制入口），没有 `lib.rs`。一个没有 `lib.rs` 的 Rust crate 无法被其他 crate 作为 library dependency 引用。
- 需要添加 `lib.rs` 来声明 crate 的公开模块树，使 `pinvou-platform` 能够 `use deepseek_tui::llm_client::LlmClient` 等。

---

## 二、改动清单

### 2.1 总体概览

| 文件 | 操作 | 行数 | 说明 |
|------|------|------|------|
| `crates/tui/src/lib.rs` | **新增** | 89 行 | Library root，声明所有模块 + CliAutoRoute 镜像 |

**仅此 1 个文件。** 不修改任何现有代码，不删除任何文件。

### 2.2 `crates/tui/src/lib.rs` 详解

**文件作用**：让 `deepseek-tui` crate 成为 `[lib]` + `[[bin]]` 双目标 crate。

**内容分两部分：**

#### 第一部分：模块声明（第 1-56 行）

```rust
//! DeepSeek-TUI library — pinvou-platform 复用的底层能力。

pub mod audit;
pub mod auto_reasoning;
// ... (全部 56 个模块)
pub mod workspace_trust;
```

每个 `pub mod` 声明一个模块，与 `main.rs` 中的 `mod` 声明一一对应。Rust 允许 `lib.rs` 和 `main.rs` 声明相同的模块——它们共享同一个源文件。

**为什么需要全部声明？**
- 最初尝试只声明 `pinvou-platform` 实际 import 的模块（`llm_client`、`models`、`tools` 等），但遇到大量「cannot find X in crate」编译错误
- 原因：模块之间存在密集的跨模块引用（例如 `tools/subagent/mod.rs` 引用 `crate::tui::app::ReasoningEffort`）
- 声明全部模块是最简单、最可靠的方案——只需复制 `main.rs` 的 `mod` 声明列表

#### 第二部分：CliAutoRoute 镜像（第 58-89 行）

```rust
pub struct CliAutoRoute {
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub auto_model: bool,
}

pub async fn resolve_cli_auto_route(
    config: &config::Config,
    model: &str,
    prompt: &str,
) -> CliAutoRoute { /* ... */ }
```

**为什么需要？**
- `main.rs` 中定义了 `CliAutoRoute` 和 `resolve_cli_auto_route`，但它们是 `main.rs` 的私有项目，仅对 binary 上下文可见
- `core/engine/turn_loop.rs` 等模块在 library 上下文中编译时，通过 `crate::resolve_cli_auto_route` 引用此函数
- 因此需要在 `lib.rs` 中提供一个镜像定义

**类型差异处理：**
- `main.rs` 版 `CliAutoRoute.reasoning_effort` 是 `Option<ReasoningEffort>`
- `lib.rs` 版 `CliAutoRoute.reasoning_effort` 是 `Option<String>`
- 原因是 `ReasoningEffort` 在 `tui::app` 模块中，而 `lib.rs` 和 `main.rs` 是不同的 crate root，类型路径解析可能不同
- 实际不影响功能——`resolve_cli_auto_route` 只是路由辅助函数

---

## 三、拉取上游最新代码后的操作步骤

```bash
# 1. 拉取上游
cd DeepSeek-TUI
git fetch origin
git merge origin/main  # 或 rebase

# 2. 检查 main.rs 的 mod 声明是否新增了模块
diff <(git show HEAD~1:crates/tui/src/main.rs | grep "^mod " | sort) \
     <(grep "^mod " crates/tui/src/main.rs | sort)

# 3. 如果有新增模块 → 在 lib.rs 中同步添加对应的 `pub mod xxx;`
#    格式: pub mod xxx;

# 4. 验证编译
cargo check -p deepseek-tui --lib
cargo check --manifest-path ../pinvou-platform/Cargo.toml
```

**常见风险**：
- 上游新增模块有 inline `mod xxx { ... }` → lib.rs 不需要处理（这些只存在于 main.rs 作用域）
- 上游新增了在 `main.rs` 中定义的私有函数（类似 `resolve_cli_auto_route`）→ 需要在 lib.rs 中添加镜像
- 上游修改了 `CliAutoRoute` 的字段 → 需要同步更新 lib.rs 中的镜像定义

---

## 四、切换其他底层 Agent 的移植指南

如果要换掉 DeepSeek-TUI，改用 OpenCode 或其他 agent 作为底层：

### 4.1 需要新 Agent 提供的能力

新 agent 需要满足以下条件之一：

**方案 A：新 Agent 有 lib.rs（推荐）**
直接在 `pinvou-platform/Cargo.toml` 中改为 path 依赖新 agent：

```toml
# 之前
deepseek-tui = { path = "../DeepSeek-TUI/crates/tui" }
# 之后
opencode = { path = "../OpenCode/crates/core" }
```

然后在 `DeepSeekHarness` 中，把 `use deepseek_tui::*` 改为 `use opencode::*`，适配 `LlmClient` trait 的等价接口。

**方案 B：新 Agent 没有 lib.rs**
按本文档的方式给新 agent 添加 `lib.rs`（声明模块 + 镜像必要类型/函数），其余步骤同方案 A。

### 4.2 需要实现的接口

pinvou-platform 通过 `AgentHarness` trait 使用底层 agent。切换时只需：

1. 新实现 `AgentHarness`（参考 `deepseek_harness.rs`，约 150 行）
2. 适配 `ChatRequest → Agent 的请求格式` 的类型映射
3. 适配 `Agent 的流式事件 → StreamEvent` 的事件映射
4. 注册工具列表（`tools()` 方法）

### 4.3 不需要改动的部分

- `AgentHarness` trait 定义 — 不改
- `PlatformEngine` — 不改
- `StepBuilder` / `LLMReviewer` / `ResponseChecker` — 不改
- TUI 层 — 不改
- AppConfig / AppRegistry — 不改

---

## 五、验证清单

拉取上游或切换平台后，逐项验证：

- [ ] `cargo check -p deepseek-tui --lib` 通过
- [ ] `cargo check -p deepseek-tui --bin deepseek-tui` 通过（原有二进制不受影响）
- [ ] `cargo check --manifest-path pinvou-platform/Cargo.toml` 通过
- [ ] `cargo test --manifest-path pinvou-platform/Cargo.toml` 全部通过
- [ ] `cargo run --manifest-path pinvou-platform/Cargo.toml -- --apps-dir apps/` 可以启动 TUI

---

> 最后更新：2026-05-09
> 对应 DeepSeek-TUI commit: `f283e56` (v0.8.19+)
> Fork 原始仓库: https://github.com/Hmbown/DeepSeek-TUI

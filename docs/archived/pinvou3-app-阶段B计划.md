# Plan: pinvou3-app — 阶段 B 实施计划

> 创建：2026-05-12
> 状态：开干中
> 关联：
> - 阶段 A 验证报告：`docs/验证报告-qwen3.6-deepseek-tui.md`（Qwen3.6 完成率 5/5，pinvou3 编排层证伪）
> - DeepSeek-TUI 架构详解：`docs/archived/DeepSeek-TUI-架构详解.md`

---

## Context

阶段 A 数据证明 Qwen3.6 + DeepSeek-TUI 原生足够，pinvou-platform 7000 行编排层是过度设计。阶段 B 转换方向：**做一个 Tauri 桌面应用**当智能助手 UI。

**两个严格约束**：
1. 底层完全依赖 DeepSeek-TUI（非必要不动其源码）
2. 只用本地 GB10 + Qwen3.6（设计以当前模型能力为基线）

**MVP 范围**：单聊天界面 + 流式输出 + **工具调用进度可视化**（阶段 A 最缺的）。中文默认。Tauri 单机应用，不做 Web 双轨。

**砍掉**：多 session / Skills / Workflow / 多 agent 入口 / 文件附件 / 远程访问 —— 全部 MVP 后再决定。

---

## 项目布局

```
pinvou3/
├── pinvou-platform/          ← ⚠️ 冻结，不动
├── DeepSeek-TUI/             ← 库依赖，PR #1511 等 review
├── pinvou3-app/              ← 🆕 阶段 B 主体
│   ├── Cargo.toml            ← workspace（含 src-tauri）
│   ├── package.json          ← Tauri CLI
│   ├── src-tauri/
│   │   ├── Cargo.toml        ← deepseek-tui = { path = "../../DeepSeek-TUI/crates/tui" }
│   │   ├── tauri.conf.json
│   │   └── src/
│   │       ├── main.rs       (~30 行)
│   │       ├── lib.rs        (~50 行)
│   │       ├── engine.rs     (~250 行)  ← EngineHandle wrapper
│   │       ├── commands.rs   (~120 行)  ← Tauri invoke handlers
│   │       └── events.rs     (~80 行)   ← Engine Event → app.emit
│   └── src/                  ← 前端
│       ├── index.html
│       ├── chat.js
│       ├── styles/
│       │   ├── theme-liquid.css   ← 从 pinvou2 拷贝
│       │   └── tool-progress.css  ← 新增
│       └── locales/zh.js
```

---

## 任务表

### Week 1：骨架贯通

| # | 任务 | 验收 |
|---|---|---|
| B1 | `pinvou3-app/` Tauri 2.0 项目初始化 | `cargo tauri dev` 能弹窗口 |
| B2 | `src-tauri/engine.rs` 包 EngineHandle（80% 抄 `pinvou-platform/src/engine_harness.rs`） | 单测：spawn → SendMessage → 收到 TurnComplete |
| B3 | Tauri 最小命令链：前端按钮 → `invoke('chat', ...)` → SendMessage → Event::MessageDelta → 前端 token 流 | 端到端流式可见 |
| **B4** | **Week 1 验收**：窗口里输 "翻译: hello" → 看到流式 "你好" | 截图存档 |

### Week 2：套皮 + 工具进度

| # | 任务 |
|---|---|
| B5 | 拷贝 pinvou2 `theme-liquid.css` + Tailwind 配置 + 字体加载 |
| B6 | 移植 Chatroom 组件（消息气泡 / 滚动 / markdown / 思考气泡），简化到两个气泡角色（用户右 / 助手左） |
| B7 | **工具调用进度卡片**（核心差异点）：消息流里渲染 `🔧 web_search ... → 5 results ✓` `📄 read_file ✓` `💻 code_execution ✓` |
| B8 | Tauri 打包：`cargo tauri build` 生成 AppImage / .deb |

### Week 3：可演示

| # | 任务 |
|---|---|
| B9 | 中文 i18n（`locales/zh.js`，从 pinvou2 抠 Chatroom 相关词条重填） |
| B10 | 跑阶段 A 5 个核心任务 UI 端演示 |
| B11 | 收集 bug，决定是否进 Week 4 |

---

## Week 1 Day 1 具体步骤

1. 环境检查（cargo-tauri / node）
2. `cargo tauri init` 或手工建项目
3. Cargo.toml 加 `deepseek-tui` 依赖
4. 拷贝 EngineHandle spawn 逻辑
5. 写最小 `chat` 命令
6. 写极简 HTML + JS
7. `cargo tauri dev` 验收

---

## 不做的事

1. ❌ 不动 DeepSeek-TUI 源码（PR #1511 之外）
2. ❌ 不动 pinvou-platform/（冻结，留作参考；MVP 跑通后再统一删）
3. ❌ 不上 Web 双轨（只 Tauri）
4. ❌ 不做 SKILL.md / 领域 agent / Workflow 编排（往后排）
5. ❌ 不做多 session、文件附件、认证、远程访问

---

## 验证

**Week 1 验收**：截图 + 录屏 / asciinema，"翻译: hello" → 流式 "你好"

**Week 2 验收**：跑阶段 A 任务 2（CSV 环比），UI 能看到 `📄 read_file ✓ → 💻 code_execution ✓ → 渲染 markdown 表格`

**Week 3 验收**：5 个核心任务全部 UI 端跑通，跟阶段 A 的 CLI 结果对比

---

## 遇到问题

按用户授权"遇到问题再问"。规则：
- 工程细节（怎么 invoke、怎么 emit、怎么处理 channel）—— 自行决定
- 设计抉择（要不要加新组件、要不要改 DeepSeek-TUI、要不要砍计划）—— 必问
- 阻塞（连不上 vLLM、Tauri 命令报错查不出）—— 必问

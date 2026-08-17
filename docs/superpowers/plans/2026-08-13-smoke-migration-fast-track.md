# Smoke Migration Fast Track Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 快速交付可供 GAIA、BFCL、WorkBuddy 并行开发的核心契约，并把现有 Smoke 完整迁入统一 `pinvou benchmark run smoke` 后删除旧 executable。

**Architecture:** 先串行建立 `agent-backend-api` 与 `benchmark-core` 最小稳定契约；随后按文件所有权并行实现 core 运行存储、`adapter-smoke` 和唯一可依赖 `pinvou3_lib::headless_bridge` 的产品后端；最后串行接入 CLI、做真实等价验证并删除旧入口。通用 core 与 adapter 不依赖 Tauri、CodeWhale 或其他 adapter。

**Tech Stack:** Rust 1.97.1、Cargo workspace、serde、tokio、async-trait、clap、现有 Tauri/EnginePool 运行时。

---

### Task 1: 冻结最小公共契约

**Files:**
- Create: `pinvou-cli/Cargo.toml`
- Create: `pinvou-cli/crates/agent-backend-api/**`
- Create: `pinvou-cli/crates/benchmark-core/{Cargo.toml,src/lib.rs,src/contracts.rs,src/adapter.rs}`
- Test: `pinvou-cli/crates/agent-backend-api/tests/backend_contract.rs`
- Test: `pinvou-cli/crates/benchmark-core/tests/adapter_contract.rs`

- [ ] 先写编译失败测试，固定 Adapter descriptor、Native/External execution kind、Task/Outcome、安全 failure category、HeadlessAgentBackend 和只读 SafeAgentEvent。
- [ ] 运行两个包级测试，确认因 API 缺失 RED。
- [ ] 实现不包含 Tauri/App/CodeWhale 类型的最小契约；所有私有输入和 prediction 使用 opaque handle。
- [ ] 运行包级测试到 GREEN，提交 `feat(eval): 建立评测核心与无头接口`。

### Task 2: 并行实现 Smoke 所需 core

**Files:**
- Create: `pinvou-cli/crates/benchmark-core/src/{manifest,event,store,runner,service,report,security}.rs`
- Test: `pinvou-cli/crates/benchmark-core/tests/**`

- [ ] 先写 manifest 不覆盖、outcome-before-terminal、resume 跳过 completed、单并发 Native runner、JSONL/Markdown 安全发布测试并观察 RED。
- [ ] 实现仅 Smoke 必需的路径、事件、恢复、runner、报告；ExternalHarness 只保留契约，不执行进程。
- [ ] 运行 `cargo test -p benchmark-core` 到 GREEN；不得修改 adapter/App/CLI 文件。

### Task 3: 并行迁移 adapter-smoke

**Files:**
- Create: `pinvou-cli/crates/adapter-smoke/**`
- Source reference: `pinvou3-app/src-tauri/src/features/assistant/eval/**`

- [ ] 先迁移现有测试并使其因缺实现 RED：5 个 case golden、rules、Product Score、diagnosis、Judge schema/隐私、Markdown 十节。
- [ ] 迁移 cases、ToolExpectation、纯规则、Smoke Health、诊断、runtime-agnostic Judge parser/prompt 与 Smoke renderer。
- [ ] Adapter 只能依赖 core/api，禁止 `deepseek_tui`、Tauri、EnginePool；Smoke Health 不进入 official score envelope。
- [ ] 运行 `cargo test -p adapter-smoke` 到 GREEN；旧 eval 文件暂不删除。

### Task 4: 并行实现真实产品桥接

**Files:**
- Create: `pinvou3-app/src-tauri/src/headless_bridge.rs`
- Create: `pinvou-cli/crates/pinvou-product-backend/**`
- Modify narrowly: `pinvou3-app/src-tauri/{Cargo.toml,src/lib.rs}`

- [ ] 先写 bridge contract test，固定主线程无窗口 host、整批模型 snapshot、单 case Session、prepare/run/cancel/close 与安全 DTO。
- [ ] `headless_bridge` 内复用 EnginePoolRuntime 和现有 Tauri bootstrap；不暴露 EnginePool/AppHandle/ProductChatRuntime。
- [ ] `pinvou-product-backend` 是唯一可依赖 `pinvou3-tauri` 的 CLI crate；boundary guard 对此提供唯一例外。
- [ ] 只运行一次定向 bridge `cargo check/test --no-run`；不反复全量编译 Tauri。

### Task 5: 接入统一 CLI

**Files:**
- Create: `pinvou-cli/crates/cli/**`
- Modify: `pinvou-cli/Cargo.toml` 和各 crate manifests 的最终接线

- [ ] 先写 CLI RED：`benchmark list/run smoke/status/resume/report`、旧退出码 0/1/2、human/json 输出、无 credential 参数。
- [ ] 显式注册 `adapter-smoke`，通过 product backend host 调用相同 core service。
- [ ] 其余正式 benchmark 子命令可解析并返回固定 `not_available`，不得伪造 adapter。
- [ ] 运行 CLI package test 到 GREEN，提交 `feat(eval): 迁移 Smoke 并接入完整产品后端`。

### Task 6: 真实等价验证与删除旧入口

**Files:**
- Delete after parity: `pinvou3-app/src-tauri/src/bin/eval_smoke.rs`
- Delete after parity: `pinvou3-app/src-tauri/src/eval_cli.rs`
- Delete migrated duplicate eval modules
- Modify: `pinvou3-app/src-tauri/{Cargo.toml,src/lib.rs}`
- Modify: `pinvou3-app/src-tauri/src/app/commands/eval.rs` to thin service adapter or remove if unused

- [ ] 使用同一模型分别运行旧/new Smoke；比较 5 case ID/顺序、状态、退出码、JSONL schema、Markdown、Product Score、Judge status、隐私和 Session 清理，不比较自然语言逐字内容。
- [ ] 只有新命令真实通过后才删除旧 executable、re-export 和重复编排；保留 EnginePool/Session/模型固定在 App bridge。
- [ ] 定向运行 core、adapter、CLI 和 bridge check，执行架构守卫、diff-check 和敏感数据扫描。
- [ ] 提交 `refactor(eval): 切换统一 CLI 并删除旧评测入口`，冻结 Adapter Contract v1。

## 快速验证策略

开发中仅运行：

```powershell
cargo test --manifest-path pinvou-cli/Cargo.toml -p agent-backend-api
cargo test --manifest-path pinvou-cli/Cargo.toml -p benchmark-core
cargo test --manifest-path pinvou-cli/Cargo.toml -p adapter-smoke
cargo test --manifest-path pinvou-cli/Cargo.toml -p pinvou-cli --test cli_contract
```

桥接只做一次定向编译，最终只做一次真实新命令运行。跳过 release build、全仓 clippy、重复 Tauri 全量测试以及已知会触发 Windows loader `0xc0000139` 的实际 lib test executable；但不能跳过编译、格式、架构、隐私、清理和真实 Smoke 验收。

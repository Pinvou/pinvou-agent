---
description: "修复大模型状态监控显示的任务清单"
---

# 任务：修复大模型状态监控显示

**输入**：`specs/006-fix-model-status/` 下的设计文档

**前置条件**：`plan.md`、`spec.md`、`research.md`、`data-model.md`、`contracts/model-monitor-contract.md`、`quickstart.md`

**测试策略**：本 feature 涉及状态判断、远端鉴权、本地 metrics 兼容和前端展示语义，测试任务需要先于对应实现任务完成或更新。Rust 单测覆盖目标推导和监控采样，手动 smoke 覆盖 Windows 桌面页面展示。

## Phase 1：准备

- [X] T001 阅读 `specs/006-fix-model-status/plan.md`，确认本 feature 只修改 `pinvou3-app` 的模型状态监控链路
- [X] T002 检查 `pinvou3-app/src-tauri/src/bridge/mod.rs`、`pinvou3-app/src-tauri/src/monitor.rs`、`pinvou3-app/src-tauri/src/commands.rs`、`pinvou3-app/src-tauri/src/harness.rs`、`pinvou3-app/src/tauri-bridge.js`、`pinvou3-app/src/index.html` 的当前 worktree 状态
- [X] T003 [P] 复核 `specs/006-fix-model-status/contracts/model-monitor-contract.md` 中 `vllm` 兼容字段和新增诊断字段语义
- [X] T004 [P] 复核 `specs/006-fix-model-status/data-model.md` 中模型监控目标、模型状态快照、本地运行指标和诊断信息的字段边界
- [X] T005 [P] 复核 `specs/006-fix-model-status/quickstart.md` 中自动检查和手动 smoke 场景

## Phase 2：基础任务

- [X] T006 在 `pinvou3-app/src-tauri/src/bridge/mod.rs` 中定义当前模型监控目标的数据结构或 helper，复用 `Pinvou3Bridge::base_url()`、`Pinvou3Bridge::model()`、`Pinvou3Bridge::provider()`
- [X] T007 在 `pinvou3-app/src-tauri/src/monitor.rs` 中扩展大模型快照结构，保留既有 `vllm` 字段兼容性并新增 `target_kind`、`provider`、`configured_model`、`diagnostic`、`metrics_applicable`、`metric_diagnostics`
- [X] T008 在 `pinvou3-app/src-tauri/src/monitor.rs` 中定义稳定的状态值和诊断 code，覆盖 `invalid_config`、`connection_failed`、`request_timeout`、`unauthorized`、`unexpected_response`、`model_mismatch`、`remote_metrics_not_applicable`、`metrics_unavailable`、`metric_missing`
- [X] T009 [P] 在 `pinvou3-app/src/tauri-bridge.js` 中梳理现有大模型状态字段消费点，确保后续新增字段不会破坏旧字段读取
- [X] T010 [P] 在 `pinvou3-app/src/index.html` 中标记系统监控页大模型状态卡的展示入口，确认 GPU、系统内存、版本更新区域不纳入本 feature

## Phase 3：用户故事 1 - 按当前配置显示模型状态 (Priority: P1) MVP

**目标**：系统监控页的大模型状态必须检测当前实际模型配置，远端配置不得再被本机默认 vLLM 地址覆盖。

### 测试 / 验证

- [X] T011 [P] [US1] 在 `pinvou3-app/src-tauri/src/bridge/mod.rs` 中添加或更新目标推导单测，覆盖环境变量、用户设置和默认值优先级
- [X] T012 [P] [US1] 在 `pinvou3-app/src-tauri/src/monitor.rs` 中添加目标类型判断单测，覆盖 remote、local、invalid 三类地址
- [X] T013 [US1] 在 `pinvou3-app/src-tauri/src/commands.rs` 中添加或调整命令级测试，验证 `get_monitor_snapshot` 和 `get_backend_status` 使用同一个当前模型目标

### 实现

- [X] T014 [US1] 在 `pinvou3-app/src-tauri/src/bridge/mod.rs` 中实现当前模型监控目标 helper，返回 `base_url`、`configured_model`、`provider`、`target_kind` 和配置来源摘要
- [X] T015 [US1] 在 `pinvou3-app/src-tauri/src/commands.rs` 中将大模型状态采样输入改为来自 `bridge` 的当前模型监控目标
- [X] T016 [US1] 在 `pinvou3-app/src-tauri/src/harness.rs` 中将子进程相关模型地址读取统一到当前模型目标，避免与系统监控状态目标漂移
- [X] T017 [US1] 在 `pinvou3-app/src-tauri/src/monitor.rs` 中移除或降级固定 `vllm_base_url()`、`vllm_configured_model()` 对状态采样的主导作用
- [X] T018 [US1] 在 `pinvou3-app/src-tauri/src/monitor.rs` 中确保远端配置不会触发本地默认地址检测，配置无效时返回 `invalid_config` 诊断
- [X] T019 [US1] 在 `pinvou3-app/src/tauri-bridge.js` 中把后端 `vllm.target_kind`、`vllm.upstream`、`vllm.configured_model` 规范化为前端可直接显示的字段
- [X] T020 [US1] 在 `pinvou3-app/src/index.html` 中展示当前检测目标类型、地址和配置模型名，保持现有大模型状态卡结构不大改
- [X] T021 [US1] 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml bridge --lib` 并将结果记录到 `specs/006-fix-model-status/quickstart.md`

## Phase 4：用户故事 2 - 远端模型状态可被准确表达 (Priority: P2)

**目标**：远端模型可用、鉴权失败、连接失败、响应异常和模型不匹配都能给出可理解状态，而不是笼统显示本地离线。

### 测试 / 验证

- [X] T022 [P] [US2] 在 `pinvou3-app/src-tauri/src/monitor.rs` 中添加远端模型探测单测，覆盖 `/v1/models` 成功和模型匹配
- [X] T023 [P] [US2] 在 `pinvou3-app/src-tauri/src/monitor.rs` 中添加远端异常单测，覆盖 HTTP 401、连接失败、超时和非 JSON/非模型响应
- [X] T024 [P] [US2] 在 `pinvou3-app/src-tauri/src/monitor.rs` 中添加模型不匹配单测，验证状态为 `mismatch` 且诊断 code 为 `model_mismatch`

### 实现

- [X] T025 [US2] 在 `pinvou3-app/src-tauri/src/monitor.rs` 中实现远端 OpenAI-compatible 模型状态探测，优先请求当前配置目标的模型列表或等价健康接口
- [X] T026 [US2] 在 `pinvou3-app/src-tauri/src/monitor.rs` 中实现远端诊断映射，区分鉴权失败、连接失败、请求超时、响应异常和模型不匹配
- [X] T027 [US2] 在 `pinvou3-app/src-tauri/src/commands.rs` 中确保远端探测失败不会导致 `get_monitor_snapshot` 整体返回错误
- [X] T028 [US2] 在 `pinvou3-app/src/tauri-bridge.js` 中将 `diagnostic.code` 和 `diagnostic.message` 转换为前端状态文案，保留机器可读 code
- [X] T029 [US2] 在 `pinvou3-app/src/index.html` 中展示远端目标、服务返回模型信息和诊断原因，避免只显示 `offline`
- [X] T030 [US2] 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml monitor --lib` 并将远端状态测试结果记录到 `specs/006-fix-model-status/quickstart.md`

## Phase 5：用户故事 3 - 本地指标与远端状态分开展示 (Priority: P3)

**目标**：本地 vLLM 指标仅在本地目标且 metrics 可用时展示；远端目标下本地指标缺失不应被解释为异常。

### 测试 / 验证

- [X] T031 [P] [US3] 在 `pinvou3-app/src-tauri/src/monitor.rs` 中添加本地 metrics 单测，覆盖 metrics 可用、metrics 缺失但模型列表可用、单项指标缺失三类场景
- [X] T032 [P] [US3] 在 `pinvou3-app/src-tauri/src/monitor.rs` 中添加远端 metrics 不适用单测，验证 `metrics_applicable=false` 且包含 `remote_metrics_not_applicable`
- [X] T033 [P] [US3] 在 `pinvou3-app/src/tauri-bridge.js` 中补充前端字段规整验证或手动检查记录，覆盖远端指标不适用文案

### 实现

- [X] T034 [US3] 在 `pinvou3-app/src-tauri/src/monitor.rs` 中将本地 metrics 拉取限定为 `target_kind=local`，远端目标不请求本地 metrics
- [X] T035 [US3] 在 `pinvou3-app/src-tauri/src/monitor.rs` 中保留本地 vLLM metrics 解析能力，并让 metrics 缺失通过 `metric_diagnostics` 表达而不是覆盖基础状态
- [X] T036 [US3] 在 `pinvou3-app/src/tauri-bridge.js` 中根据 `metrics_applicable` 区分本地指标值、指标缺失和远端指标不适用
- [X] T037 [US3] 在 `pinvou3-app/src/index.html` 中调整队列、上下文长度、KV 命中率、TTFT、吞吐和 token 统计展示逻辑，远端目标下显示不适用说明
- [X] T038 [US3] 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml monitor --lib` 并将本地/远端指标适用性结果记录到 `specs/006-fix-model-status/quickstart.md`

## Phase 6：收尾与横切关注点

- [X] T039 运行 `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml` 并将编译结果记录到 `specs/006-fix-model-status/quickstart.md`
- [X] T040 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml bridge --lib` 和 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml monitor --lib`，并将结果记录到 `specs/006-fix-model-status/quickstart.md`
- [X] T041 在 `specs/006-fix-model-status/quickstart.md` 中补充 Windows 手动 smoke 结果，覆盖远端官方模型、远端兼容模型和本地模型三类配置
- [X] T042 检查 `DeepSeek-TUI/` 没有被本 feature 修改，并在 `specs/006-fix-model-status/quickstart.md` 中记录边界检查结论
- [X] T043 检查 `pinvou3-app/src/index.html` 中 GPU、系统内存、版本与更新栏未被本 feature 引入无关修改
- [X] T044 将 `specs/006-fix-model-status/tasks.md` 中已完成任务勾选状态与实际实现保持一致

## 依赖与执行顺序

- Phase 1 无依赖，是所有工作的入口。
- Phase 2 阻塞所有用户故事，必须先完成目标推导、快照结构和前端消费边界。
- US1 是 MVP，必须先完成，解决“远端配置却检测本地默认地址”的核心问题。
- US2 依赖 US1 的当前目标推导，但远端诊断逻辑可以在 US1 后独立验证。
- US3 依赖 US1 的 `target_kind`，并复用 US2/US1 的快照字段，但本地 metrics 适用性可以独立测试。
- Phase 6 在所有用户故事完成后执行。

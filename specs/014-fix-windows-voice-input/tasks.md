# 任务：修复 Windows 语音输入

**输入**：`specs/014-fix-windows-voice-input/` 下的 `spec.md`、`plan.md`、`research.md`、`data-model.md`、`contracts/`、`quickstart.md`

**前置说明**：本任务清单要求先通过运行/日志查明 Windows 语音输入断点，再做最小修复。不得绕开 DeepSeek-TUI 已有语音底座重写完整语音识别系统。

## Phase 1: Setup

**目标**：确认当前工作区、语音链路入口和可执行验证命令，为后续修复建立基线。

- [X] T001 记录当前工作区状态和分支信息，确认不混入无关改动：`git status --short --branch`
- [X] T002 [P] 搜索 pinvou 前端语音入口和麦克风按钮引用，记录发现到 `specs/014-fix-windows-voice-input/research.md`
- [X] T003 [P] 搜索 DeepSeek-TUI `/voice`、`AppAction::VoiceCapture`、`capture_and_transcribe` 链路，记录发现到 `specs/014-fix-windows-voice-input/research.md`
- [X] T004 [P] 检查 Tauri 权限和 capability 配置中的麦克风/WebView 权限相关项，记录发现到 `specs/014-fix-windows-voice-input/research.md`
- [ ] T005 在 Windows 开发版或安装版中触发一次语音输入，收集实际失败表现和日志，补充到 `specs/014-fix-windows-voice-input/research.md`

---

## Phase 2: Foundational

**目标**：建立共享的状态、诊断和测试基础，阻塞所有用户故事的实现。

- [X] T006 在 `pinvou3-app/src/tauri-bridge.js` 中梳理当前输入框草稿、当前会话 ID、通知机制与可复用状态更新函数
- [X] T007 在 `pinvou3-app/src/index.html` 中定位聊天输入区麦克风按钮或需要新增的语音入口挂载点
- [X] T008 在 `pinvou3-app/src-tauri/src/lib.rs` 和 `pinvou3-app/src-tauri/capabilities/default.json` 中确认前后端命令注册与权限边界
- [X] T009 [P] 在 `pinvou3-app/src/tauri-bridge.js` 中设计语音输入会话临时状态结构，覆盖 `idle/requesting_permission/recording/transcribing/completed/cancelled/failed`
- [X] T010 [P] 在 `pinvou3-app/src/tauri-bridge.js` 中设计语音诊断事件映射，覆盖 `permission/device/recording/transcribing/writeback`
- [X] T011 根据 T002-T005 的结果决定修复位置：`pinvou3-app` 内修复，或必要时在 `DeepSeek-TUI/crates/tui/src/commands/groups/core/` 做小补丁并记录理由

**检查点**：完成后应能说明 Windows 语音输入失败发生在入口、权限、录音、识别或回填的哪一阶段。

---

## Phase 3: User Story 1 - Windows 用户可以完成语音输入 (Priority: P1)

**目标**：Windows 用户可以从聊天输入区启动语音输入，完成录音/识别，并把文本回填到当前输入框。

**独立测试**：在 Windows 中使用可用麦克风完成 3 次短句语音输入，识别文本进入当前输入框，草稿不丢失，无页面卡死或静默失败。

- [X] T012 [US1] 为语音输入启动和状态流转添加前端逻辑测试或可运行调试入口，文件：`pinvou3-app/src/tauri-bridge.js`
- [X] T013 [US1] 在 `pinvou3-app/src/index.html` 中接入语音输入入口的可见状态，显示请求权限、录音中、识别中和完成状态
- [X] T014 [US1] 在 `pinvou3-app/src/tauri-bridge.js` 中实现启动语音输入的会话上下文绑定，记录启动时 `activeSessionId` 和输入框草稿
- [X] T015 [US1] 在 `pinvou3-app/src/tauri-bridge.js` 中接入实际语音捕获/识别链路，优先复用现有 DeepSeek-TUI 或 pinvou bridge 能力
- [X] T016 [US1] 若 T015 需要后端命令，在 `pinvou3-app/src-tauri/src/commands.rs` 中新增最小 Tauri command 并在 `pinvou3-app/src-tauri/src/lib.rs` 注册
- [X] T017 [US1] 若 T015 需要 Tauri 权限调整，在 `pinvou3-app/src-tauri/capabilities/default.json` 中补齐最小权限
- [X] T018 [US1] 在 `pinvou3-app/src/tauri-bridge.js` 中实现识别文本追加到启动时输入框草稿，不覆盖已有文本
- [X] T019 [US1] 运行 `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml` 验证 Rust 侧改动
- [ ] T020 [US1] 在 Windows 中执行 US1 手动 smoke，并把实际结果记录到 `specs/014-fix-windows-voice-input/quickstart.md`

---

## Phase 4: User Story 2 - 失败原因对用户可见 (Priority: P2)

**目标**：麦克风权限、设备、录音和识别失败时，用户能看到明确、可操作的提示。

**独立测试**：分别模拟或手动触发权限拒绝、无设备/设备不可用、识别失败，确认 2 秒内出现对应提示并可重试。

- [X] T021 [US2] 在 `pinvou3-app/src/tauri-bridge.js` 中实现语音错误分类：权限拒绝、设备不可用、录音失败、识别失败、超时、回填上下文不匹配
- [X] T022 [US2] 在 `pinvou3-app/src/index.html` 中展示语音输入失败提示和重试/关闭入口
- [X] T023 [US2] 在 `pinvou3-app/src/tauri-bridge.js` 中实现语音诊断事件输出，确保不记录原始音频、密钥或完整敏感路径
- [X] T024 [US2] 如果错误来自后端命令，在 `pinvou3-app/src-tauri/src/commands.rs` 中返回结构化错误类别而不是只返回字符串
- [X] T025 [US2] 为错误分类和用户提示映射添加自动化测试或轻量断言，文件：`pinvou3-app/src/tauri-bridge.js` 或 `pinvou3-app/src-tauri/src/commands.rs`
- [ ] T026 [US2] 在 Windows 中分别验证权限拒绝、无设备/禁用设备、识别失败提示，并把结果记录到 `specs/014-fix-windows-voice-input/quickstart.md`

---

## Phase 5: User Story 3 - 现有非语音输入不受影响 (Priority: P3)

**目标**：修复语音输入后，文本发送、附件上传、会话切换和取消语音输入不回归。

**独立测试**：普通文本发送、附件上传、会话切换均可用；语音输入取消后草稿保留；切换会话后识别结果不会写入错误会话。

- [X] T027 [US3] 在 `pinvou3-app/src/tauri-bridge.js` 中实现取消语音输入时保留启动前草稿
- [X] T028 [US3] 在 `pinvou3-app/src/tauri-bridge.js` 中实现识别结果返回时的会话匹配校验，防止跨会话写入
- [X] T029 [US3] 在 `pinvou3-app/src/index.html` 中确认语音状态不会遮挡或禁用普通文本发送与附件上传入口
- [X] T030 [US3] 为会话切换和取消行为添加自动化测试或调试断言，文件：`pinvou3-app/src/tauri-bridge.js`
- [ ] T031 [US3] 在 Windows 中执行普通文本发送、附件上传、会话切换、语音取消和跨会话保护 smoke，记录到 `specs/014-fix-windows-voice-input/quickstart.md`

---

## Phase 6: Polish & Cross-Cutting

**目标**：完成验证、文档和交付准备。

- [X] T032 [P] 检查 `specs/014-fix-windows-voice-input/contracts/voice-input-ui.md` 是否与最终 UI 行为一致，必要时更新
- [X] T033 [P] 检查 `specs/014-fix-windows-voice-input/contracts/voice-diagnostics.md` 是否与最终诊断事件一致，必要时更新
- [X] T034 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml voice --lib` 或说明无匹配测试时的替代命令
- [X] T035 如修改 DeepSeek-TUI，运行 `cargo test --manifest-path DeepSeek-TUI/Cargo.toml -p codewhale-tui voice`
- [X] T036 运行 `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`
- [X] T037 如修改前端或 Tauri capability，运行 `npm run build -- --bundles msi` 验证打包
- [X] T038 汇总原因、修复点、测试结果和未覆盖风险到最终回复，不自动提交代码

---

## Dependencies & Order

### Phase Dependencies

- Phase 1 → Phase 2 → US1 → US2 → US3 → Polish
- T011 是实现分流门，完成前不得开始 US1 代码修改
- US1 是 MVP，完成后语音输入应已能在正常设备和权限下闭环
- US2 可在 US1 的主链路完成后补强错误处理
- US3 依赖 US1 的状态结构和回填逻辑

### User Story Dependencies

- **US1**：依赖 Phase 1 和 Phase 2，可独立交付 MVP
- **US2**：依赖基础语音输入状态和错误入口，建议在 US1 后实施
- **US3**：依赖语音状态结构和回填逻辑，建议在 US1 后实施

---

## Parallel Execution Examples

### Phase 1

```text
T002 搜前端入口
T003 搜 DeepSeek-TUI voice 链路
T004 查 Tauri 权限配置
```

### Phase 2

```text
T009 设计语音输入状态
T010 设计诊断事件映射
```

### US2

```text
T022 做 UI 提示
T023 做诊断事件输出
```

### Polish

```text
T032 更新 UI 契约
T033 更新诊断契约
```

---

## Implementation Strategy

### MVP First

先完成 Phase 1、Phase 2 和 US1。目标是在 Windows 有麦克风和权限允许的环境下，语音输入可以完成一次从启动到文本回填的闭环。

### Incremental Delivery

1. **MVP**：US1 正常路径可用。
2. **可诊断增强**：US2 覆盖权限、设备、录音和识别失败提示。
3. **回归保护**：US3 确保文本输入、附件上传、会话切换和取消行为不受影响。

### Validation Gate

最终交付前必须至少完成：

- Windows 手动语音输入 smoke。
- `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`。
- 若涉及 DeepSeek-TUI，运行对应 voice 测试。
- 若涉及前端或 Tauri 权限，完成构建或打包验证。

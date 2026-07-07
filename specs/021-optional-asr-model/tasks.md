# 任务：ASR 模型可选下载

**输入**：`specs/021-optional-asr-model/` 下的设计文档

**前置条件**：`plan.md`、`spec.md`、`research.md`、`data-model.md`、`contracts/asr-runtime-contract.md`、`quickstart.md`

**测试**：本 feature 涉及 Windows 打包、跨平台 ASR 状态、模型下载与失败恢复，必须包含测试和手动 smoke；测试任务先于对应实现任务。

**组织方式**：任务按用户故事分组。MVP 为 US1。

## Phase 1: 准备（共享基础）

**目的**：确认上下文、现有实现、资源边界和验证方式。

- [x] T001 阅读 `specs/021-optional-asr-model/plan.md`，确认范围和不触碰 `DeepSeek-TUI/` 的约束
- [x] T002 检查 ASR 相关 worktree 状态：`pinvou3-app/src-tauri/src/voice_asr.rs`、`pinvou3-app/src-tauri/src/os/windows/windows_system.rs`、`pinvou3-app/src-tauri/src/os/windows/windows_path.rs`、`pinvou3-app/src/tauri-bridge.js`
- [x] T003 [P] 记录当前 Windows ASR 资源体积和安装包基线到 `specs/021-optional-asr-model/quickstart.md`
- [x] T004 [P] 将 FunAudioLLM 官方 `sensevoice-small-q8.gguf` ModelScope/Hugging Face URL、大小 `254208320` 和 sha256 写入 Windows `os` 层 ASR 模型规格

---

## Phase 2: 基础任务（阻塞后续故事）

**目的**：建立跨平台复用的 ASR 模型状态、路径和校验基础。

**Critical**：本阶段完成前，不应开始任一用户故事实现。

- [x] T005 在 `pinvou3-app/src-tauri/src/voice_asr.rs` 中补充 ASR 模型元数据和状态 helper 设计
- [x] T006 在 `pinvou3-app/src-tauri/src/voice_asr.rs` 中添加 sha256 文件校验 helper 和单元测试
- [x] T007 在 `pinvou3-app/src-tauri/src/os/windows/windows_path.rs` 中增加用户目录模型查找 helper
- [x] T008 在 `pinvou3-app/src-tauri/src/voice_asr.rs` 中调整 `VoiceAsrStatus`，支持 runtime 可用但 model 缺失
- [x] T009 在 `pinvou3-app/src-tauri/src/os/windows/windows_system.rs` 中定义 Windows runtime/model 分离后的平台语义

**检查点**：共享状态、路径和校验能力明确，用户故事可以独立推进。

---

## Phase 3: 用户故事 1 - 精简安装并保留语音入口 (Priority: P1) MVP

**目标**：Windows 主安装包不再包含大体积 ASR 模型；无模型时应用正常启动，语音入口展示可下载状态而非重装提示。

**独立测试**：bundle 中没有 `sensevoice-small-q8.gguf`；无模型环境下 `voice_asr_status` 返回 runtime 可用、model 缺失、installable 可用。

### 测试 / 验证

- [x] T010 [P] [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_path.rs` 中添加 wrapper/backend 存在但 bundled model 缺失的路径测试
- [x] T011 [P] [US1] 在 `pinvou3-app/src-tauri/src/voice_asr.rs` 中添加 runtime 可用但用户模型缺失的状态测试
- [x] T012 [US1] 按 `specs/021-optional-asr-model/quickstart.md` 增加 Windows 主包资源检查步骤

### 实现

- [x] T013 [US1] 更新 `pinvou3-app/src-tauri/tauri.conf.json`，停止把 `resources/windows/asr/models/sensevoice-small-q8.gguf` 打进主包
- [x] T014 [US1] 更新 `pinvou3-app/src-tauri/src/os/windows/windows_path.rs`，模型查找优先使用 `~/.pinvou3/asr/sensevoice-small-q8.gguf`
- [x] T015 [US1] 更新 `pinvou3-app/src-tauri/src/os/windows/windows_system.rs`，让缺模型时可通过应用内下载补全
- [x] T016 [US1] 更新 `pinvou3-app/src/tauri-bridge.js`，区分“缺模型可下载”和“runtime 缺失需修复安装”

**检查点**：US1 可独立演示，Windows 主包瘦身且无模型环境下语音入口仍能引导用户。

---

## Phase 4: 用户故事 2 - 按需获取模型后使用语音能力 (Priority: P2)

**目标**：用户确认后下载 ASR 模型到用户目录，校验通过后无需重启即可转写，并在重启后保持可用状态。

**独立测试**：无模型环境触发 `install_voice_asr`，观察 `voice_asr:progress`，完成后 `voice_asr_status.ready=true`。

### 测试 / 验证

- [x] T017 [P] [US2] 添加模型下载/落盘测试，覆盖 `.part` 写入、校验和 rename/promote
- [x] T018 [P] [US2] 在 `pinvou3-app/src-tauri/src/commands.rs` 中添加 `run_local_asr_cli` 环境变量测试
- [x] T019 [US2] 在 `pinvou3-app/src-tauri/src/voice_asr.rs` 中添加模型大小与 sha256 校验测试

### 实现

- [x] T020 [US2] 在 `pinvou3-app/src-tauri/src/voice_asr.rs` 中实现可复用 ASR 模型下载/校验 helper
- [x] T021 [US2] 更新 `pinvou3-app/src-tauri/src/os/windows/windows_system.rs` 的 `install_asr_runtime()`，下载 q8 模型到 `~/.pinvou3/asr/`
- [x] T022 [US2] 更新 `pinvou3-app/src-tauri/src/os/linux/linux_system.rs`，在不改变 Linux UX 的前提下对齐模型校验 helper
- [x] T023 [US2] 更新 `pinvou3-app/src-tauri/src/commands.rs`，启动 `pinvou-asr` 时设置 `PINVOU3_SENSEVOICE_MODEL`
- [x] T024 [US2] 更新 `pinvou3-app/src/tauri-bridge.js`，展示 `start`、`model`、`verify`、`done` 进度阶段

**检查点**：US2 可独立验证，下载完成后无需重启即可使用语音识别。

---

## Phase 5: 用户故事 3 - 处理下载失败和离线环境 (Priority: P3)

**目标**：网络失败、取消、磁盘不足、模型损坏或 `.part` 残留时，系统保持可恢复状态并提供重试路径。

**独立测试**：不可达 URL、损坏模型和中断残留均能给出明确提示，不启用损坏模型。

### 测试 / 验证

- [x] T025 [P] [US3] 在 `pinvou3-app/src-tauri/src/voice_asr.rs` 中添加 sha256 不匹配时清理 `.part` 的测试
- [ ] T026 [P] [US3] 在 `pinvou3-app/src-tauri/src/os/windows/windows_system.rs` 中添加 URL 不可达和写入失败测试
- [x] T027 [US3] 在 `pinvou3-app/src/tauri-bridge.js` 中补充 `failed`、`cancelled`、重试入口的手动检查说明

### 实现

- [x] T028 [US3] 在 `pinvou3-app/src-tauri/src/voice_asr.rs` 中参考 `pinvou3-app/src-tauri/src/knowledge/model_download.rs` 增加 ASR 下载取消标记
- [x] T029 [US3] 在 `pinvou3-app/src-tauri/src/voice_asr.rs` 中新增 `cancel_voice_asr` Tauri command
- [x] T030 [US3] 在 `pinvou3-app/src-tauri/src/lib.rs` 中注册 `cancel_voice_asr` command
- [x] T031 [US3] 更新 `pinvou3-app/src/tauri-bridge.js`，关闭 ASR 安装框或点击取消时调用 `cancel_voice_asr`
- [x] T032 [US3] 更新 `pinvou3-app/src-tauri/src/voice_asr.rs` 和 `pinvou3-app/src-tauri/src/os/windows/windows_system.rs`，确保失败信息不包含用户音频内容

**检查点**：所有计划内用户故事均可独立验证，失败恢复不会污染模型状态。

---

## Phase 6: 收尾与横切关注点

- [x] T033 [P] 更新 `pinvou3-app/src-tauri/resources/windows/asr/README.md`，说明主包只带 runtime、模型下载到 `~/.pinvou3/asr/`
- [x] T034 [P] 更新 `specs/021-optional-asr-model/quickstart.md`，加入最终验收记录模板
- [x] T035 运行 `cargo fmt --manifest-path pinvou3-app/src-tauri/Cargo.toml`，检查格式化结果
- [x] T036 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml voice_asr --lib`，记录结果到 `specs/021-optional-asr-model/quickstart.md`
- [x] T037 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_path --lib`，记录结果到 `specs/021-optional-asr-model/quickstart.md`
- [x] T038 运行 `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`，记录结果到 `specs/021-optional-asr-model/quickstart.md`
- [x] T039 按 `specs/021-optional-asr-model/quickstart.md` 执行 Windows 主包资源检查
- [ ] T040 按 `specs/021-optional-asr-model/quickstart.md` 执行 Windows 无模型、下载、重启、失败恢复 smoke
- [x] T041 检查 `DeepSeek-TUI/` 无本 feature 相关改动，并记录到 `specs/021-optional-asr-model/quickstart.md`

---

## 依赖与执行顺序

- Phase 1 无依赖。
- Phase 2 阻塞所有用户故事。
- US1 是 MVP，必须先完成。
- US2 依赖 US1 的状态语义和资源拆分。
- US3 依赖 US2 的下载 helper，但失败测试和前端检查可提前准备。
- Phase 6 在所有用户故事完成后执行。

## 并行机会

- T003 与 T004 可并行。
- T010 与 T011 可并行。
- T017 与 T018 可并行。
- T025 与 T026 可并行。
- T033 与 T034 可并行。

## 并行执行示例

```text
US1: T010 + T011
US2: T017 + T018
US3: T025 + T026
Polish: T033 + T034
```

## 实施策略

1. 完成 Phase 1-2，保证状态、路径、校验基础清楚。
2. 完成 US1 作为 MVP：主包移除模型，缺模型入口正确。
3. 完成 US2：接通下载、校验和转写。
4. 完成 US3：补齐失败恢复和取消。
5. 最后执行 quickstart 全链路验证和包体积验收。

## 格式校验

- 所有任务均使用 `- [ ] Txxx` checklist 格式。
- 用户故事任务均带 `[US1]`、`[US2]` 或 `[US3]` 标签。
- 可并行任务仅在不同文件或无直接依赖时标记 `[P]`。
- 每个任务描述均包含具体文件路径或明确的验证文档路径。

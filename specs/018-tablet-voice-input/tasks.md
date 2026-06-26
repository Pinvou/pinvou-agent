# 任务：平板语音输入强化

**输入**：`specs/018-tablet-voice-input/` 下的设计文档

**前置条件**：`plan.md`、`spec.md`、`research.md`、`data-model.md`、`contracts/tablet-voice-input-ui.md`、`quickstart.md`

**测试**：规格未要求 TDD 或新增自动化测试；本任务清单以实现任务配套静态检查、触屏 smoke、语音流程 smoke、桌面回归和可访问性检查为验证方式。

**组织方式**：任务按用户故事分组，保证每个故事可以独立实现和验证。主要实现文件为 `pinvou3-app/src/index.html`，`pinvou3-app/src/tauri-bridge.js` 仅作为现有语音桥接契约参考，默认不修改。

## 格式：`[ID] [P?] [Story] 描述`

- **[P]**：可并行执行，前提是修改不同文件或只做独立验证。
- **[Story]**：任务对应的用户故事，例如 US1、US2、US3、US4。
- 描述中必须包含精确文件路径。

## Phase 1: 准备（共享基础）

**目的**：确认上下文、现有 UI 结构、桥接契约和验证路径。

- [X] T001 阅读 `specs/018-tablet-voice-input/plan.md`、`specs/018-tablet-voice-input/spec.md` 和 `specs/018-tablet-voice-input/contracts/tablet-voice-input-ui.md`，确认实现边界只覆盖 Tauri 前端输入区。
- [X] T002 检查 `pinvou3-app/src/index.html` 中 `ChatView`、`inputText`、`handleSend`、`handleVoiceClick`、语音状态提示、小麦克风按钮和发送按钮的位置，记录修改锚点。
- [X] T003 [P] 检查 `pinvou3-app/src/tauri-bridge.js` 中 `voiceInput`、`startVoiceInput`、`cancelVoiceInput`、`clearVoiceInput`、`appendVoiceText` 的现有契约，确认本 feature 无需新增桥接 API。
- [X] T004 [P] 对照 `specs/018-tablet-voice-input/quickstart.md` 准备实现后的验证命令和手动 smoke 清单。

---

## Phase 2: 基础任务（阻塞后续故事）

**目的**：抽取所有故事共享的输入区派生状态和布局基础。

**关键要求**：本阶段完成前，不应开始具体用户故事实现。

- [X] T005 在 `pinvou3-app/src/index.html` 的 `ChatView` 中派生 `hasDraftText`、`hasReadyAttachment`、`canSend`、`isVoiceBusy`、`isVoiceRecording` 等共享状态，复用现有 `inputText`、`attachments` 和 `voiceInput`。
- [X] T006 在 `pinvou3-app/src/index.html` 中为底部输入区补充稳定布局容器和响应式类名，确保新增触控操作不会改变桌面默认输入流。
- [X] T007 在 `pinvou3-app/src/index.html` 中补充按钮可访问性基础属性，覆盖主语音入口、发送、清除和现有小麦克风按钮的 `aria-label`、`title` 或状态表达。

**检查点**：共享状态与布局基础完成后，可以按用户故事独立推进。

---

## Phase 3: 用户故事 1 - 平板用户快速发现语音输入 (Priority: P1) MVP

**目标**：在触屏/平板体验中新增更醒目的语音输入入口，同时保留现有输入框和原有小麦克风入口。

**独立测试**：在触屏或平板尺寸窗口中打开主聊天界面，确认现有输入区仍可用，并出现一个更醒目、更易触控的语音入口，用户能在 3 秒内识别其用途。

### 实现

- [X] T008 [US1] 在 `pinvou3-app/src/index.html` 中实现 `DeviceExperienceMode` 的前端判断，结合触控能力和视口尺寸派生平板触控体验模式。
- [X] T009 [US1] 在 `pinvou3-app/src/index.html` 的输入区附近新增主语音按钮渲染结构，保持现有小麦克风按钮继续存在。
- [X] T010 [US1] 在 `pinvou3-app/src/index.html` 中将主语音按钮绑定到现有 `handleVoiceClick`，确保点击行为复用 `bridge.startVoiceInput` 和现有写回流程。
- [X] T011 [US1] 在 `pinvou3-app/src/index.html` 中为主语音按钮添加平板触控尺寸、间距和视觉强调样式，避免遮挡输入框、附件入口、模型选择和工具菜单。
- [ ] T012 [P] [US1] 按 `specs/018-tablet-voice-input/quickstart.md` 的“平板触屏 smoke”验证主语音入口可发现性和现有输入区保留情况。

**检查点**：US1 可独立演示为 MVP；新增主语音入口出现且不替代现有 UI。

---

## Phase 4: 用户故事 2 - 触屏用户可靠完成语音输入 (Priority: P2)

**目标**：让用户通过醒目的语音入口完成录音、结束、取消、失败重试和识别文本写回，并清楚理解当前状态。

**独立测试**：点击主语音按钮后，界面能清晰表达请求权限、录音中、识别中、完成、失败和取消状态，且不会启动多个并发录音流程。

### 实现

- [X] T013 [US2] 在 `pinvou3-app/src/index.html` 中为主语音按钮映射 `voiceInput.status` 到可见状态，包括 `idle`、`requesting_permission`、`recording`、`transcribing`、`completed`、`failed`、`cancelled`。
- [X] T014 [US2] 在 `pinvou3-app/src/index.html` 中处理主语音按钮的录音中点击语义，使 `recording` 状态下点击表示结束录音并沿用现有 `handleVoiceClick`。
- [X] T015 [US2] 在 `pinvou3-app/src/index.html` 中为 `requesting_permission` 和 `transcribing` 状态添加禁用或忙碌反馈，防止重复点击启动多个录音流程。
- [X] T016 [US2] 在 `pinvou3-app/src/index.html` 中保持失败重试和取消路径可触控，复用现有 `handleVoiceCancel` 与失败提示区域。
- [ ] T017 [P] [US2] 按 `specs/018-tablet-voice-input/quickstart.md` 的“语音输入 smoke”和“失败路径 smoke”验证录音、识别、失败、取消和写回输入框流程。

**检查点**：US2 可独立验证语音状态闭环；失败后仍能继续文本输入。

---

## Phase 5: 用户故事 3 - 输入框有内容时快速处理文本 (Priority: P2)

**目标**：当输入框存在非空文本时，显示触控友好的发送和清除按钮；清除只清空文本，发送复用现有流程。

**独立测试**：在触屏环境中输入文本或写回语音识别文本，确认发送和清除按钮出现、可触控、不遮挡语音入口；清空后按钮状态恢复。

### 实现

- [X] T018 [US3] 在 `pinvou3-app/src/index.html` 中新增 `handleClearInput`，只清空 `inputText`，不移除附件、不取消会话、不改变语音桥接状态。
- [X] T019 [US3] 在 `pinvou3-app/src/index.html` 中按 `hasDraftText` 渲染清除按钮，输入框为空或仅空白字符时不占用主要操作区域。
- [X] T020 [US3] 在 `pinvou3-app/src/index.html` 中调整发送按钮显示和可用状态，使非空文本或已解析附件沿用现有 `handleSend`，空白文本不能发送。
- [X] T021 [US3] 在 `pinvou3-app/src/index.html` 中处理录音中和识别中状态下发送、清除、语音入口的可用性，避免半成品语音结果被误发或误清。
- [ ] T022 [P] [US3] 按 `specs/018-tablet-voice-input/quickstart.md` 的“平板触屏 smoke”验证输入文本、语音写回文本、发送、清除和按钮恢复状态。

**检查点**：US3 可独立验证文本处理闭环；清除行为不会影响附件和语音入口。

---

## Phase 6: 用户故事 4 - 非平板用户不被打扰 (Priority: P3)

**目标**：常规桌面键鼠用户的输入区布局、键盘输入、原有小麦克风和发送流程保持稳定。

**独立测试**：在常规桌面窗口中打开应用，确认键盘输入、Enter 发送、Shift+Enter 换行、附件、模型选择、工具菜单、原有语音入口和发送按钮仍可用。

### 实现

- [X] T023 [US4] 在 `pinvou3-app/src/index.html` 中为桌面体验设置降级布局，确保主语音入口不显示或不挤占主要输入区域。
- [X] T024 [US4] 在 `pinvou3-app/src/index.html` 中检查 Enter 发送、Shift+Enter 换行、附件添加、模型选择、工具菜单和原有小麦克风按钮的事件行为不被新增按钮影响。
- [X] T025 [US4] 在 `pinvou3-app/src/index.html` 中检查横屏、竖屏、窄窗口和输入框自动增高时的布局不重叠、不遮挡消息列表底部内容。
- [ ] T026 [P] [US4] 按 `specs/018-tablet-voice-input/quickstart.md` 的“桌面回归”和“横竖屏和尺寸验证”完成手动回归。

**检查点**：US4 可独立验证桌面用户不增加必需步骤，核心输入流程不退化。

---

## Phase 7: 收尾与横切关注点

- [X] T027 [P] 在 `specs/018-tablet-voice-input/quickstart.md` 中补充实现后的实际验证结果或未执行原因，保持中文记录。
- [X] T028 运行 `cargo check --manifest-path pinvou3-app\src-tauri\Cargo.toml`，并将结果对照 `specs/018-tablet-voice-input/quickstart.md` 记录。
- [X] T029 运行 `rg -n "voiceInput|startVoiceInput|cancelVoiceInput|clearVoiceInput|handleSend|handleVoiceClick|inputText" pinvou3-app/src/index.html pinvou3-app/src/tauri-bridge.js`，确认实现仍复用现有桥接和发送入口。
- [X] T030 检查 `pinvou3-app/src/index.html` 中新增 UI 未引入无关格式化、未修改 DeepSeek-TUI 底座、未新增 npm/Cargo/系统依赖。
- [X] T031 按 `specs/018-tablet-voice-input/contracts/tablet-voice-input-ui.md` 逐项核对桌面/平板入口可见性、语音状态、输入状态、清除行为、发送行为、可访问性和布局验收。

---

## 依赖与执行顺序

- Phase 1 无依赖。
- Phase 2 阻塞所有用户故事。
- US1 是 MVP，应最先实现和验收。
- US2 依赖 Phase 2 和 US1 的主语音入口结构，但可在 US1 完成后独立验证语音状态闭环。
- US3 依赖 Phase 2，可与 US2 在协调 `pinvou3-app/src/index.html` 修改范围后推进。
- US4 依赖 US1、US2、US3 的最终布局结果，作为桌面回归和响应式收口。
- Phase 7 在所有用户故事完成后执行。

## 并行机会

- T003、T004 可与 T001、T002 并行，因为只读取不同文档或桥接契约。
- T012、T017、T022、T026 是手动验证任务，可由不同人员在对应故事实现完成后独立执行。
- US2 与 US3 都主要修改 `pinvou3-app/src/index.html`，逻辑上可并行设计，但实际编辑同一文件时需要串行合并，避免冲突。
- T027 可在实现完成后与 T028、T029 的命令验证并行准备。

## 并行执行示例

```text
US1 完成实现后：
- T012 验证平板触屏 smoke
- T017 预备语音输入 smoke 的失败路径

US2 与 US3 分工时：
- 一人处理 T013-T016 的语音状态映射
- 一人处理 T018-T021 的发送/清除动作
- 合并前共同检查 pinvou3-app/src/index.html 的输入区布局冲突
```

## 实施策略

1. 先完成 Phase 1 和 Phase 2，避免在不了解现有 `ChatView` 状态的情况下直接改 UI。
2. 以 US1 作为 MVP：新增醒目的主语音入口，但不替换现有小麦克风和发送流程。
3. 再完成 US2 的状态闭环，确保录音、处理中、失败和取消都有清晰反馈。
4. 接着完成 US3 的发送/清除按钮，让语音识别文本写回后能快速处理。
5. 最后执行 US4 和 Phase 7，确认桌面回归、横竖屏布局、可访问性和构建检查。

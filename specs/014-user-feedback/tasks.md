# 任务：我要反馈

**输入**：`specs/014-user-feedback/` 下的设计文档

**前置条件**：plan.md、spec.md、research.md、data-model.md、contracts/

**测试**：本 feature 涉及用户输入、附件、本地文件打包和外部上传通道，必须包含 Rust 单测、`cargo check` 和前端手动 smoke。测试任务按用户故事放在实现前或同阶段前置位置。

**组织方式**：任务按用户故事分组，保证每个故事可以独立实现和验证。

## 格式：`[ID] [P?] [Story] 描述`

- **[P]**：可并行执行，前提是修改不同文件且没有依赖关系。
- **[Story]**：任务对应的用户故事，例如 US1、US2、US3。
- 描述中必须包含精确文件路径。
- 文档、任务描述和验收说明默认使用中文；英文仅保留必要命令、路径、API 字段或原文。

## Phase 1: 准备（共享基础）

**目的**：确认上下文、目录、依赖和验证方式。

- [X] T001 阅读 `specs/014-user-feedback/plan.md`、`specs/014-user-feedback/spec.md`、`specs/014-user-feedback/contracts/feedback-tauri-command.md` 和 `specs/014-user-feedback/contracts/h3c-upload-package.md`，确认本 feature 不修改 `DeepSeek-TUI/`
- [X] T002 检查 `git status --short` 并记录当前 worktree 中与 `pinvou3-app/src/index.html`、`pinvou3-app/src/tauri-bridge.js`、`pinvou3-app/src-tauri/src/commands.rs`、`pinvou3-app/src-tauri/src/lib.rs`、`pinvou3-app/src-tauri/Cargo.toml` 相关的既有改动
- [X] T003 [P] 确认 `pinvou3-app/src/index.html` 中 `SettingsView`、i18n 字典和设置页卡片结构的插入位置
- [X] T004 [P] 确认 `pinvou3-app/src/tauri-bridge.js` 中现有 Tauri `invoke` 包装和状态通知模式
- [X] T005 [P] 确认 `pinvou3-app/src-tauri/src/commands.rs` 与 `pinvou3-app/src-tauri/src/lib.rs` 中命令声明和 `generate_handler!` 注册模式

---

## Phase 2: 基础任务（阻塞后续故事）

**目的**：完成所有用户故事共同依赖的后端骨架、路径、依赖和类型边界。

**关键要求**：本阶段完成前，不应开始任何用户故事实现。

- [X] T006 在 `pinvou3-app/src-tauri/Cargo.toml` 增加生成 H3CLogCollector 兼容 `tar.gz` 所需的最小依赖，并确认不引入 .NET 或 H3CLogCollector 外部进程依赖
- [X] T007 在 `pinvou3-app/src-tauri/src/lib.rs` 声明新的 `feedback` 模块，保持 `DeepSeek-TUI/` 不变
- [X] T008 在 `pinvou3-app/src-tauri/src/bridge/paths.rs` 新增 `feedback_root()`、`feedback_pending_dir()`、`feedback_receipts_dir()` 路径 helper，全部位于 `~/.pinvou3/feedback/`
- [X] T009 在 `pinvou3-app/src-tauri/src/feedback.rs` 创建反馈模块骨架，定义 `FeedbackType`、`FeedbackSubmitRequest`、`FeedbackAttachmentRequest`、`FeedbackReceipt`、`FeedbackStatus` 和错误类型
- [X] T010 [P] 在 `pinvou3-app/src-tauri/src/feedback.rs` 添加反馈类型、标题长度、说明长度、入口来源的纯函数校验单测
- [X] T011 在 `pinvou3-app/src-tauri/src/commands.rs` 新增 `submit_feedback` Tauri 命令签名，返回 `FeedbackReceipt`，暂时调用 `feedback::submit_feedback`
- [X] T012 在 `pinvou3-app/src-tauri/src/lib.rs` 的 `tauri::generate_handler!` 注册 `commands::submit_feedback`
- [X] T013 在 `pinvou3-app/src/tauri-bridge.js` 新增 `submitFeedback(request)` bridge 方法并导出到 `window.pinvouBridge`

**检查点**：共享骨架完成，可以开始按用户故事实施。

---

## Phase 3: 用户故事 1 - 提交文字反馈 (Priority: P1) MVP

**目标**：用户能从设置页打开“我要反馈”，选择类型、填写文字并提交，看到成功或可重试失败回执。

**独立测试**：在不添加附件的情况下，从设置页提交一条文字反馈；空说明会被阻止；mock 上传成功时展示成功回执；mock 上传失败时保留草稿并展示重试状态。

### 测试 / 验证

- [X] T014 [P] [US1] 在 `pinvou3-app/src-tauri/src/feedback.rs` 添加文字反馈请求校验单测，覆盖空说明、非法类型、标题超长、合法纯文字请求
- [X] T015 [P] [US1] 在 `pinvou3-app/src-tauri/src/feedback.rs` 添加 mock uploader 单测，验证纯文字反馈可生成 `FeedbackReceipt` 且失败时返回 `failed_retryable`
- [X] T016 [US1] 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml feedback --lib`，确认 US1 新增后端测试先失败后通过

### 实现

- [X] T017 [US1] 在 `pinvou3-app/src-tauri/src/feedback.rs` 实现 `validate_feedback_request`，覆盖 `type`、`title`、`description`、`entry_point` 和 `privacy_notice_version`
- [X] T018 [US1] 在 `pinvou3-app/src-tauri/src/feedback.rs` 实现 `build_app_context` 白名单摘要，只包含 app version、OS、arch、language、entry_point、error_summary、timestamp
- [X] T019 [US1] 在 `pinvou3-app/src-tauri/src/feedback.rs` 实现纯文字 `manifest.json` 与 `description.txt` 写入到 `~/.pinvou3/feedback/pending/<feedback_id>/`
- [X] T020 [US1] 在 `pinvou3-app/src-tauri/src/feedback.rs` 实现 mockable uploader trait 或等价注入点，使 US1 可不依赖真实 H3C 网络独立验证
- [X] T021 [US1] 在 `pinvou3-app/src/index.html` 的 i18n 字典中新增反馈入口、反馈类型、表单字段、隐私提示、提交状态和错误提示文案
- [X] T022 [US1] 在 `pinvou3-app/src/index.html` 的 `SettingsView` 中新增“帮助与反馈”区域和“我要反馈”按钮
- [X] T023 [US1] 在 `pinvou3-app/src/index.html` 中新增反馈表单弹窗或面板，包含反馈类型分段控件、标题输入、说明文本框、隐私提示、提交和取消按钮
- [X] T024 [US1] 在 `pinvou3-app/src/index.html` 中接入 `bridge.submitFeedback`，实现 `idle`、`submitting`、`submitted`、`failed_retryable`、`failed_validation` 状态
- [X] T025 [US1] 在 `pinvou3-app/src/index.html` 中实现关闭未提交草稿时的确认提示，避免静默丢失已填写内容

**检查点**：US1 可独立演示和验证。

---

## Phase 4: 用户故事 2 - 上传截图和小视频 (Priority: P2)

**目标**：用户能为同一条反馈添加图片和短视频，提交前看到附件列表、大小/格式校验结果，并可删除附件。

**独立测试**：在已填写文字的反馈中添加支持格式图片或短视频可提交；添加不支持格式、超限大小或超过数量限制的附件会被阻止，并保留已填写文字。

### 测试 / 验证

- [X] T026 [P] [US2] 在 `pinvou3-app/src-tauri/src/feedback.rs` 添加附件后端校验单测，覆盖扩展名、单图片 10 MB、单视频 50 MB、总量 80 MB、数量 5 个限制
- [X] T027 [P] [US2] 在 `pinvou3-app/src-tauri/src/feedback.rs` 添加反馈包隐私单测，验证 `manifest.json` 不包含附件原始绝对路径
- [X] T028 [US2] 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml feedback --lib`，确认 US2 新增后端测试通过

### 实现

- [X] T029 [US2] 在 `pinvou3-app/src-tauri/src/feedback.rs` 实现附件真实文件校验，拒绝目录、缺失文件、不支持格式、超限大小和超限数量
- [X] T030 [US2] 在 `pinvou3-app/src-tauri/src/feedback.rs` 实现附件复制到反馈包 `attachments/`，使用包内安全文件名如 `001-image.png`
- [X] T031 [US2] 在 `pinvou3-app/src-tauri/src/feedback.rs` 为每个附件计算 `sha256` 并写入 `manifest.json`
- [X] T032 [US2] 在 `pinvou3-app/src/index.html` 的反馈表单中新增系统文件选择、附件列表、大小展示、删除按钮和前端即时校验
- [X] T033 [US2] 在 `pinvou3-app/src/index.html` 中实现附件错误提示，确保不支持格式、数量超限和大小超限时保留用户已填写文字
- [X] T034 [US2] 在 `pinvou3-app/src/tauri-bridge.js` 中确保 `submitFeedback` 请求包含附件 `path`、`name`、`media_type`、`mime`、`size_bytes` 字段，并与 `contracts/feedback-tauri-command.md` 对齐

**检查点**：US1 和 US2 均可独立验证。

---

## Phase 5: 用户故事 3 - 通过既有通道送达反馈 (Priority: P3)

**目标**：反馈包通过 H3CLogCollector 兼容上传方式送达既有接收通道，失败时显示可重试并保留待重试目录。

**独立测试**：提交含文字和附件的反馈，后端生成 `tar.gz`、XOR 为 `.dbg`、计算 `checkCode`，成功响应时前端显示回执；token 或上传失败时保留草稿并展示可重试状态。

### 测试 / 验证

- [X] T035 [P] [US3] 在 `pinvou3-app/src-tauri/src/feedback.rs` 添加 `swap_sn` 与 `checkCode` 单测，使用固定 token 和设备序列号验证 MD5 结果
- [X] T036 [P] [US3] 在 `pinvou3-app/src-tauri/src/feedback.rs` 添加 XOR 单测，验证 `.dbg` 字节等于源字节逐个 XOR `0x55`
- [X] T037 [P] [US3] 在 `pinvou3-app/src-tauri/src/feedback.rs` 添加打包单测，验证 `manifest.json`、`description.txt` 和 `attachments/` 被写入 `tar.gz`
- [X] T038 [P] [US3] 在 `pinvou3-app/src-tauri/src/feedback.rs` 添加 mock HTTP 单测或上传客户端 mock，覆盖 token 失败、上传 `retCode != 0` 和成功三类结果
- [X] T039 [US3] 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml feedback --lib`，确认 H3C 兼容上传相关测试通过

### 实现

- [X] T040 [US3] 在 `pinvou3-app/src-tauri/src/feedback.rs` 实现 `create_tar_gz_archive`，与 `contracts/h3c-upload-package.md` 的目录打包规则一致
- [X] T041 [US3] 在 `pinvou3-app/src-tauri/src/feedback.rs` 实现 `xor_to_dbg`，将 `tar.gz` 转为 `<feedback_id>.dbg`
- [X] T042 [US3] 在 `pinvou3-app/src-tauri/src/feedback.rs` 实现设备序列号解析，Windows 优先系统序列号，同时支持环境变量或配置覆盖
- [X] T043 [US3] 在 `pinvou3-app/src-tauri/src/feedback.rs` 实现 `request_upload_token`，请求 `http://sohord10.h3c.com/rest/ihomers/uploadRequest`
- [X] T044 [US3] 在 `pinvou3-app/src-tauri/src/feedback.rs` 实现 `compute_check_code`，按 H3CLogCollector 的 token + 相邻字符交换序列号计算小写 MD5
- [X] T045 [US3] 在 `pinvou3-app/src-tauri/src/feedback.rs` 实现 `upload_dbg_file`，向 `http://sohord10.h3c.com/rest/ihomers/uploadSysinfoFile` 发送 `PUT` 二进制流和 `GwSn`、`FileName`、`checkCode` 请求头
- [X] T046 [US3] 在 `pinvou3-app/src-tauri/src/feedback.rs` 实现成功后清理 `.tar.gz`、`.dbg` 和原始附件副本，并在 `~/.pinvou3/feedback/receipts/` 写入 `receipt.json`
- [X] T047 [US3] 在 `pinvou3-app/src-tauri/src/feedback.rs` 实现失败映射：token 失败和上传失败返回 `failed_retryable`，设备序列号缺失返回用户可理解的校验错误
- [X] T048 [US3] 在 `pinvou3-app/src/index.html` 中实现可重试失败状态的“重试提交”按钮，复用当前表单内容再次调用 `bridge.submitFeedback`

**检查点**：所有计划内用户故事均可独立验证。

---

## Phase 6: 收尾与横切关注点

- [X] T049 [P] 更新 `specs/014-user-feedback/quickstart.md` 的手动验收结果记录，补充实际执行环境和未覆盖项
- [X] T050 [P] 检查 `specs/014-user-feedback/contracts/feedback-tauri-command.md` 与 `pinvou3-app/src/tauri-bridge.js`、`pinvou3-app/src-tauri/src/feedback.rs` 的字段命名一致性
- [X] T051 [P] 检查 `specs/014-user-feedback/contracts/h3c-upload-package.md` 与 `pinvou3-app/src-tauri/src/feedback.rs` 的上传 URL、请求头、XOR 和 `checkCode` 实现一致性
- [X] T052 运行 `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml` 并记录结果
- [X] T053 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml feedback --lib` 并记录结果
- [ ] T054 按 `specs/014-user-feedback/quickstart.md` 执行前端 smoke：设置页入口、空说明拦截、纯文字提交、附件展示、超限附件、断网失败、成功后清理
- [X] T055 检查 `pinvou3-app/src-tauri/src/feedback.rs` 生成的 `manifest.json` 不包含聊天正文、用户文件正文、完整原始附件绝对路径、模型 API key 或搜索 API key
- [X] T056 检查 `DeepSeek-TUI/` 未发生改动，并在最终实现说明中记录底座边界未触碰

## 依赖与执行顺序

- Phase 1 无依赖。
- Phase 2 阻塞所有用户故事。
- US1 是 MVP，应最先完成；US2 依赖 US1 的反馈表单与基础命令；US3 依赖 US1/US2 的反馈包与附件模型。
- Phase 6 依赖所有用户故事完成。

## 用户故事依赖图

```text
Setup -> Foundation -> US1 -> US2 -> US3 -> Polish
```

## 并行机会

- T003、T004、T005 可并行读取不同文件。
- T010 可与 T011/T012 之后的前端准备并行，但必须在后端实现完成前用于保护校验规则。
- US1 中 T014、T015 可并行；T021 可与 T017-T020 并行，因为修改前端文案和后端逻辑不同文件。
- US2 中 T026、T027 可并行；T032、T033 可与 T029-T031 并行，但 T034 应在字段最终确定后执行。
- US3 中 T035、T036、T037、T038 可并行；T043、T044、T045 可在 T042 设备序列号策略明确后串联实现。
- Phase 6 中 T049、T050、T051 可并行。

## 并行执行示例

### US1

```text
并行：T014、T015、T021
串行：T017 -> T018 -> T019 -> T020 -> T024 -> T025
```

### US2

```text
并行：T026、T027、T032
串行：T029 -> T030 -> T031 -> T034
```

### US3

```text
并行：T035、T036、T037、T038
串行：T040 -> T041 -> T042 -> T043 -> T044 -> T045 -> T046 -> T047 -> T048
```

## 实施策略

1. 先完成 Phase 1 和 Phase 2，得到稳定的命令、路径和模块骨架。
2. 交付 US1 作为 MVP：设置页入口、纯文字反馈、基础回执和失败保留草稿。
3. 在 US1 可演示后实施 US2：附件选择、前后端双重校验和反馈包附件结构。
4. 最后实施 US3：H3CLogCollector 兼容打包、XOR、token、`checkCode` 和真实上传。
5. 每完成一个用户故事立即运行对应测试，不把验证堆到最后。

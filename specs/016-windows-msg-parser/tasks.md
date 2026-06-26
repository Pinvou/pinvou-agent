# 任务：Windows MSG 邮件解析

**输入**：`specs/016-windows-msg-parser/` 下的设计文档

**前置条件**：`plan.md`、`spec.md`、`research.md`、`data-model.md`、`contracts/`、`quickstart.md`

**测试**：本 feature 涉及附件解析和跨平台依赖体检，必须包含单元测试、回归测试和手动验收记录。测试任务应先于对应实现任务定义，实施时允许先补最小失败用例再修复。

**组织方式**：任务按用户故事分组，保证每个故事可独立实现和验证。

## Phase 1: 准备（共享基础）

**目的**：确认上下文、当前代码入口、依赖策略和验证样本。

- [X] T001 阅读 `specs/016-windows-msg-parser/plan.md`、`specs/016-windows-msg-parser/research.md`、`specs/016-windows-msg-parser/contracts/email-ingest-contract.md` 并确认 Windows `.msg`、`.eml`、Linux `.msg` 的范围边界
- [X] T002 检查当前 worktree 与 `pinvou3-app/src-tauri/src/file_ingest.rs`、`pinvou3-app/src-tauri/Cargo.toml`、`pinvou3-app/src-tauri/src/os/` 的已有改动，避免覆盖用户修改
- [X] T003 [P] 准备或记录 Windows 手动验收样本路径，在 `specs/016-windows-msg-parser/quickstart.md` 中补充 `.msg/.eml/broken.msg` 实际样本位置
- [X] T004 [P] 运行基线测试 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml file_ingest --lib` 并在 `specs/016-windows-msg-parser/quickstart.md` 记录当前失败/通过状态

---

## Phase 2: 基础任务（阻塞后续故事）

**目的**：引入共享依赖和平台策略入口，供所有用户故事复用。

**⚠️ CRITICAL**：本阶段完成前，不应开始任何用户故事实现。

- [X] T005 在 `pinvou3-app/src-tauri/Cargo.toml` 中添加 `msg_parser` 0.3.x 依赖并运行 `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml` 更新 `pinvou3-app/src-tauri/Cargo.lock`
- [X] T006 在 `pinvou3-app/src-tauri/src/os/interface/system.rs` 中定义邮件解析/体检平台策略导出函数签名，例如 Windows 是否内置 `.msg`、邮件依赖提示包名
- [X] T007 [P] 在 `pinvou3-app/src-tauri/src/os/windows/windows_system.rs` 中实现 Windows 邮件策略：`.msg` 原生可用、外部 `msgconvert` 非必需、邮件依赖提示不含 Linux 包名
- [X] T008 [P] 在 `pinvou3-app/src-tauri/src/os/linux/linux_system.rs` 中实现 Linux 邮件策略：保留 `msgconvert` 必需和 `python3 libemail-outlook-message-perl` 提示
- [X] T009 [P] 在 `pinvou3-app/src-tauri/src/os/unsupported.rs` 中实现 unsupported 平台邮件策略兜底，避免展示 Linux 专用命令
- [X] T010 在 `pinvou3-app/src-tauri/src/os/mod.rs`、`pinvou3-app/src-tauri/src/os/interface/mod.rs`、`pinvou3-app/src-tauri/src/os/windows/mod.rs`、`pinvou3-app/src-tauri/src/os/linux/mod.rs` 中补齐邮件策略函数 re-export

**检查点**：共享平台策略可编译，用户故事可开始实现。

---

## Phase 3: 用户故事 1 - Windows 直接解析 Outlook MSG 文件 (Priority: P1) 🎯 MVP

**目标**：Windows 用户无需 Perl/msgconvert 即可导入 `.msg`，获得可读邮件头、正文和附件名。

**独立测试**：在无 Perl/msgconvert 的 Windows 环境导入有效 `.msg`，返回 `markdown` 且不提示 `libemail-outlook-message-perl`；导入损坏 `.msg` 返回 warning 不崩溃。

### 测试 / 验证

- [X] T011 [P] [US1] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 中新增 `.msg` markdown 格式化单测，覆盖发件人、收件人、抄送、主题、日期、正文和附件名输出
- [X] T012 [P] [US1] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 中新增损坏 `.msg` 解析单测或错误路径单测，验证返回 warning 且保留 `basename/path/byte_size`
- [X] T013 [P] [US1] 在 `specs/016-windows-msg-parser/quickstart.md` 中补充 Windows 手动验收步骤：确认 `Get-Command msgconvert` 为空后导入真实 `.msg`

### 实现

- [X] T014 [US1] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 中新增 `parse_msg_via_msg_parser(path: &Path) -> Result<String, String>`，使用 `msg_parser::Outlook::from_path` 读取 `.msg`
- [X] T015 [US1] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 中新增或拆分 `format_msg_as_markdown(...)`，按契约输出发件人、收件人、抄送、密送、主题、日期、正文和附件名
- [X] T016 [US1] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 中调整 `ingest_email`：Windows `.msg` 走 `parse_msg_via_msg_parser`，不检查 `tools.msgconvert`，不调用 `Command::new("msgconvert")`
- [X] T017 [US1] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 中为 `.msg` 解析失败补充用户可理解错误信息，确保 `IngestResult.warning` 包含失败原因且应用不崩溃
- [X] T018 [US1] 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml file_ingest --lib` 并在 `specs/016-windows-msg-parser/quickstart.md` 记录 US1 验证结果

**检查点**：US1 可独立演示和验收；Windows `.msg` 不再依赖 `msgconvert`。

---

## Phase 4: 用户故事 2 - 依赖体检不再误报 Windows 邮件依赖 (Priority: P2)

**目标**：Windows 依赖体检不再展示 `libemail-outlook-message-perl`、Perl、`msgconvert` 或 Linux 安装命令。

**独立测试**：Windows 上调用依赖体检，邮件项不包含 Linux 包名，并能反映内置 `.msg` 能力。

### 测试 / 验证

- [X] T019 [P] [US2] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 中新增依赖体检单测，验证 Windows 邮件项 `apt` 为空或不包含 `libemail-outlook-message-perl/msgconvert`
- [X] T020 [P] [US2] 在 `specs/016-windows-msg-parser/quickstart.md` 中补充依赖体检验收记录项：搜索不到 `libemail-outlook-message-perl` 和 `msgconvert`

### 实现

- [X] T021 [US2] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 中调整 `SystemTools` 或邮件依赖判断，避免 Windows 邮件能力被 `msgconvert` 阻塞
- [X] T022 [US2] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 中调整 `check_dependencies()` 的 `email` 项，改为使用 OS 层邮件策略生成 `installed` 和 `apt`
- [X] T023 [US2] 检查 `pinvou3-app/src/index.html` 中邮件依赖展示文案，确认无需新增前端 key；如需要则更新 `dep_email` 周边提示避免误导 Windows 用户
- [X] T024 [US2] 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml file_ingest --lib` 并在 `specs/016-windows-msg-parser/quickstart.md` 记录 US2 验证结果

**检查点**：US2 可独立验证；依赖体检不再误报 Windows 邮件依赖。

---

## Phase 5: 用户故事 3 - 保持 EML 和 Linux 行为稳定 (Priority: P3)

**目标**：`.eml` 解析输出保持不变；Linux `.msg` 仍保留 `msgconvert/libemail-outlook-message-perl` 行为。

**独立测试**：导入 `.eml` 输出字段与旧版本一致；Linux 缺少 `msgconvert` 时仍提示 `sudo apt install libemail-outlook-message-perl`。

### 测试 / 验证

- [X] T025 [P] [US3] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 中补充 `.eml` 输出回归单测，覆盖发件人、收件人、主题、日期、正文和附件名字段
- [X] T026 [P] [US3] 在 `pinvou3-app/src-tauri/src/os/linux/linux_system.rs` 或相关 OS 层测试中补充 Linux 邮件依赖提示回归，确认包含 `python3 libemail-outlook-message-perl`

### 实现

- [X] T027 [US3] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 中保留非 Windows `.msg` 的现有 `msgconvert` 转 `.eml` 路径，确保 Linux warning 文案仍可用
- [X] T028 [US3] 在 `pinvou3-app/src-tauri/src/os/linux/linux_dependency.rs` 中确认 `libemail-outlook-message-perl` 仍位于一键安装白名单，必要时补充测试或注释
- [X] T029 [US3] 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml file_ingest --lib` 并在 `specs/016-windows-msg-parser/quickstart.md` 记录 US3 回归结果

**检查点**：所有计划内用户故事均可独立验证。

---

## Phase 6: 收尾与横切关注点

- [X] T030 [P] 更新 `specs/016-windows-msg-parser/research.md` 或 `quickstart.md`，记录最终采用的 `msg_parser` 版本、验证命令和真实样本结论
- [X] T031 运行 `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml` 并在 `specs/016-windows-msg-parser/quickstart.md` 记录结果
- [X] T032 运行完整 quickstart 手动验收，包含 Windows `.msg`、损坏 `.msg`、`.eml`、依赖体检四项，并记录结果到 `specs/016-windows-msg-parser/quickstart.md`
- [X] T033 检查 `pinvou3-app/src-tauri/tauri.conf.json` 和打包资源，确认本 feature 未新增需要 MSI 打包的外部运行时
- [X] T034 检查 `AGENTS.md` 的 Spec Kit 指针仍指向 `specs/016-windows-msg-parser/plan.md`
- [X] T035 使用 `git status --short` 审查改动范围，确保没有无关文件或临时样本被提交

---

## 依赖与执行顺序

- Phase 1 无依赖。
- Phase 2 阻塞所有用户故事。
- US1 是 MVP，优先完成；US2 依赖 Phase 2，可在 US1 的核心解析函数稳定后实施。
- US3 可在 US1/US2 后执行，也可由另一人并行准备回归测试，但不能破坏 US1 的 Windows 路径。
- Phase 6 在 US1-US3 验证完成后执行。

## 并行机会

- T003、T004 可并行准备样本和基线测试。
- T007、T008、T009 可并行实现不同平台 OS 策略。
- T011、T012、T013 可并行补测试和 quickstart 验收说明。
- T019、T020 可并行处理体检测试和文档验收项。
- T025、T026 可并行补 `.eml` 与 Linux 回归测试。
- T030 可与 T031 前的实现收尾并行，但 T032 必须等待功能完成。

## 实施策略

1. 先完成 Phase 1-2，保证依赖和 OS 策略可编译。
2. 以 US1 作为 MVP：先让 Windows `.msg` 在无 `msgconvert` 时可解析，并通过单测。
3. 再完成 US2：修正依赖体检，避免 Windows 用户看到 Linux 包名。
4. 最后完成 US3：验证 `.eml` 和 Linux 行为没有退化。
5. 每个用户故事完成后立即运行对应验证，不把所有风险堆到最后。

## 任务统计

- 总任务数：35
- US1：8 个任务
- US2：6 个任务
- US3：5 个任务
- Setup/Foundation/Polish：16 个任务
- MVP 范围：Phase 1 + Phase 2 + US1


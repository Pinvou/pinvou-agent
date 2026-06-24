# 任务：Windows 内置 Pandoc 安装

**输入**：`specs/013-bundle-windows-pandoc/` 下的设计文档

**前置条件**：plan.md、spec.md、research.md、data-model.md、contracts/、quickstart.md

**测试**：本 feature 涉及 Windows 安装包资源、OS 层命令解析和附件上传链路，必须包含 Rust 单测、cargo check、MSI 构建/解包验证和手动 smoke 验收。

**组织方式**：任务按用户故事分组，保证每个故事可以独立实现和验证。

## 格式：`[ID] [P?] [Story] 描述`

- **[P]**：可并行执行，前提是修改不同文件且没有依赖关系。
- **[Story]**：任务对应的用户故事，例如 US1、US2、US3。
- 描述中必须包含精确文件路径。
- 文档、任务描述和验收说明默认使用中文；英文仅保留必要命令、路径、API 字段或原文。

## 路径约定

- **Tauri 桌面应用**：`pinvou3-app/src/`、`pinvou3-app/src-tauri/`
- **Windows 资源**：`pinvou3-app/src-tauri/resources/windows/`
- **OS 分层**：`pinvou3-app/src-tauri/src/os/`
- **附件解析**：`pinvou3-app/src-tauri/src/file_ingest.rs`
- **文档**：`specs/013-bundle-windows-pandoc/`

## Phase 1: 准备（共享基础）

**目的**：确认上下文、源目录、现有 Poppler 模式和验证方式。

- [X] T001 阅读 `specs/013-bundle-windows-pandoc/plan.md`、`specs/013-bundle-windows-pandoc/research.md` 和 `specs/013-bundle-windows-pandoc/contracts/windows-pandoc-runtime.md`，确认宪章检查与运行时契约
- [X] T002 检查 `git status --short --branch`，确认当前分支为 `013-bundle-windows-pandoc` 且仅处理本 feature 相关改动
- [X] T003 [P] 确认源目录 `C:\Users\z27014\Downloads\pandoc-3.10` 存在并包含 `pandoc.exe`、`COPYING.rtf`、`COPYRIGHT.txt`、`MANUAL.html`
- [X] T004 [P] 对照 `pinvou3-app/src-tauri/resources/windows/poppler-path.wxs`、`pinvou3-app/src-tauri/src/os/windows/windows_path.rs` 和 `pinvou3-app/src-tauri/src/os/windows/windows_system.rs` 梳理可复用的 Windows 内置工具模式

---

## Phase 2: 基础任务（阻塞后续故事）

**目的**：建立 Pandoc 运行时资源、OS 层抽象和共享配置，供所有故事复用。

**⚠️ CRITICAL**：本阶段完成前，不应开始任何用户故事实现。

- [X] T005 创建 `pinvou3-app/src-tauri/resources/windows/pandoc/`，从 `C:\Users\z27014\Downloads\pandoc-3.10` 复制全部 Pandoc 源文件
- [X] T006 在 `pinvou3-app/src-tauri/resources/windows/pandoc/README.md` 记录 Pandoc 版本、文件快照和维护说明
- [X] T007 在 `pinvou3-app/src-tauri/src/os/interface/system.rs` 增加 Pandoc 工具路径、可用性、依赖体检可见性和缺失提示的接口函数
- [X] T008 在 `pinvou3-app/src-tauri/src/os/mod.rs`、`pinvou3-app/src-tauri/src/os/interface/mod.rs`、`pinvou3-app/src-tauri/src/os/windows/mod.rs`、`pinvou3-app/src-tauri/src/os/linux/mod.rs` 中导出 Pandoc OS 层接口
- [X] T009 在 `pinvou3-app/src-tauri/src/os/linux/linux_path.rs`、`pinvou3-app/src-tauri/src/os/linux/linux_system.rs` 和 `pinvou3-app/src-tauri/src/os/unsupported.rs` 中实现 Linux/unsupported 的 Pandoc 默认行为，保持 Linux 继续使用系统 `pandoc`
- [X] T010 在 `pinvou3-app/src-tauri/src/os/windows/windows_path.rs` 中实现 `{当前可执行文件目录}/pandoc/pandoc.exe` 的路径解析、命令白名单和包含空格/中文路径的单元测试
- [X] T011 在 `pinvou3-app/src-tauri/src/os/windows/windows_system.rs` 中实现 Windows 内置 Pandoc 优先、进程 PATH 注入、系统 PATH 降级和 Windows 缺失提示

**检查点**：Pandoc 资源和 OS 层接口具备，后续故事可以消费统一能力。

---

## Phase 3: 用户故事 1 - Windows 安装后现代文档解析开箱可用 (Priority: P1) 🎯 MVP

**目标**：Windows 用户安装 pinvou 后，无需手动安装 Pandoc 即可上传 `docx`/`odt` 并让文档正文进入附件上下文。

**独立测试**：在 Pandoc 不在系统 PATH 的 Windows 环境中，应用优先使用安装目录内置 `pandoc.exe` 解析 `docx`/`odt`；内置 Pandoc 缺失时返回修复安装提示。

### 测试 / 验证

- [X] T012 [P] [US1] 在 `pinvou3-app/src-tauri/src/os/interface/system.rs` 或相关 OS 层测试模块中添加 Pandoc 工具路径非空和 Windows bundled path 优先的单元测试
- [X] T013 [P] [US1] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 中添加 `pandoc_tool_command_uses_os_layer_program` 单元测试，验证文档解析命令来自 OS 层
- [X] T014 [P] [US1] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 中添加 Windows 下 Pandoc 缺失提示不包含 `sudo apt install pandoc` 的回归测试

### 实现

- [X] T015 [US1] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 中将 `system_tools().pandoc` 从 `crate::os::command_exists("pandoc")` 改为 Pandoc OS 层可用性检查
- [X] T016 [US1] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 中新增 `pandoc_tool_command()` 并让 `ingest_pandoc()` 使用 OS 层 Pandoc 路径执行命令
- [X] T017 [US1] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 中将 Pandoc 缺失 warning 改为平台化缺失提示，Windows 指向安装内容异常/修复安装，Linux 保持系统包提示
- [X] T018 [US1] 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml file_ingest --lib` 验证附件解析回归
- [X] T019 [US1] 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_path --lib` 验证 Windows 路径解析回归

**检查点**：US1 可独立演示：不依赖系统 Pandoc 时，应用能使用内置 Pandoc；缺失时错误信息符合 Windows 安装版语义。

---

## Phase 4: 用户故事 2 - 安装包携带受控 Pandoc 运行时 (Priority: P2)

**目标**：Windows MSI 携带 `pandoc-3.10` 源内容，安装后释放到 `{安装目录}/pandoc` 并加入环境变量。

**独立测试**：构建 MSI 后使用管理解包或干净环境安装，确认 `pandoc/pandoc.exe` 和许可文件存在，MSI `Environment` 表包含 Pandoc PATH 项。

### 测试 / 验证

- [X] T020 [P] [US2] 在 `specs/013-bundle-windows-pandoc/quickstart.md` 中补充实际 MSI 表查询和管理解包命令记录占位
- [X] T021 [P] [US2] 使用 `Get-ChildItem pinvou3-app/src-tauri/resources/windows/pandoc/pandoc.exe` 验证仓库资源目录包含 Pandoc 可执行文件

### 实现

- [X] T022 [US2] 在 `pinvou3-app/src-tauri/resources/windows/pandoc-path.wxs` 新增 WiX environment component，将 `[INSTALLDIR]pandoc` 写入 PATH
- [X] T023 [US2] 在 `pinvou3-app/src-tauri/tauri.conf.json` 中将 `resources/windows/pandoc/` 映射到安装目标 `pandoc`
- [X] T024 [US2] 在 `pinvou3-app/src-tauri/tauri.conf.json` 中追加 Pandoc WiX fragment 和 componentRef，保留现有 Poppler 配置
- [X] T025 [US2] 运行 `npm run build -- --bundles msi` 于 `pinvou3-app/`，确认 MSI 构建成功
- [X] T026 [US2] 使用 `msiexec /a` 管理解包 `pinvou3-app/src-tauri/target/release/bundle/msi/pinvou3_0.4.7_x64_en-US.msi`，确认解包目录包含 `pandoc/pandoc.exe`、`pandoc/COPYING.rtf`、`pandoc/COPYRIGHT.txt`
- [X] T027 [US2] 查询 MSI `Environment` 表，确认包含 `[INSTALLDIR]pandoc` 的 PATH 项且不覆盖现有 Poppler PATH 项

**检查点**：US2 可独立验证：MSI 产物携带 Pandoc，安装/解包落点和 PATH 配置符合契约。

---

## Phase 5: 用户故事 3 - 依赖体检不再要求用户补 Pandoc (Priority: P3)

**目标**：Windows 依赖体检不再显示 Pandoc/现代文档解析缺失项，其他依赖项保持可见。

**独立测试**：Windows 下 `check_dependencies()` 不包含 `office_modern`/Pandoc 手动安装项；Linux 下仍保留 Pandoc 依赖检查。

### 测试 / 验证

- [X] T028 [P] [US3] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 中添加依赖体检单元测试，验证 Windows 隐藏 Pandoc 项且其他依赖项仍存在
- [X] T029 [P] [US3] 在 `pinvou3-app/src-tauri/src/os/linux/linux_system.rs` 或相关测试中添加 Linux 保留 Pandoc 依赖检查策略的回归保护

### 实现

- [X] T030 [US3] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 中将 `office_modern` 依赖体检项改为受 `crate::os::show_pandoc_dependency_check()` 控制
- [X] T031 [US3] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 中将 `office_modern` 的安装提示包名改为 `crate::os::pandoc_dependency_packages()`
- [X] T032 [US3] 检查 `pinvou3-app/src/index.html` 的 `dep_office_modern` 文案是否无需调整；若 Windows 隐藏项后仍有误导文案，仅做最小必要更新
- [X] T033 [US3] 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml dependency_check_respects_pdf_visibility_policy --lib` 或更新后的依赖体检测试名称，确认 Poppler 与 Pandoc 隐藏策略均通过

**检查点**：US3 可独立验证：Windows 体检页不要求用户补 Pandoc，Linux 行为不回退。

---

## Phase 6: 收尾与横切关注点

- [X] T034 [P] 更新 `specs/013-bundle-windows-pandoc/quickstart.md` 的实际执行记录，包括源目录文件快照、测试命令、MSI 产物路径和未执行项原因
- [X] T035 [P] 检查 `pinvou3-app/src-tauri/resources/windows/pandoc/README.md` 是否包含第三方运行时来源、版本、许可文件和维护说明
- [X] T036 运行 `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`，确认 Rust 编译通过
- [X] T037 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml file_ingest --lib`，确认附件解析相关测试通过
- [X] T038 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_path --lib`，确认 Windows 路径与 bundled tool 测试通过
- [X] T039 运行 `npm run build -- --bundles msi` 于 `pinvou3-app/`，确认最终 MSI 可构建
- [X] T040 手动或通过管理解包验证 `pinvou3-app/src-tauri/target/release/bundle/msi/pinvou3_0.4.7_x64_en-US.msi` 包含 `pandoc/pandoc.exe` 且 PATH environment 表包含 Pandoc 项
- [X] T041 在 Windows 安装版手动上传 `docx` 或 `odt`，确认附件解析结果包含文档正文而不是 Pandoc 缺失 warning
- [X] T042 检查 `git diff --stat` 和 `git status --short`，确认没有提交构建产物、临时解包目录或无关格式化

## 依赖与执行顺序

- Phase 1 无依赖。
- Phase 2 阻塞所有用户故事，尤其是 OS 层接口与资源目录。
- US1 依赖 Phase 2，可先作为 MVP 完成并验证文档上传解析。
- US2 依赖 Phase 2，可在 US1 后或与 US3 部分并行，但最终 MSI 验收需要资源和配置完整。
- US3 依赖 Phase 2，可在 US1 的 OS 层接口稳定后实施。
- Phase 6 依赖 US1、US2、US3 的实现结果。

## 并行机会

- T003 与 T004 可并行，一个检查源目录，一个阅读现有 Poppler 模式。
- T012、T013、T014 可并行编写测试，但最终实现需统一到 `file_ingest.rs` 与 OS 层。
- T020 与 T021 可并行，一个补 quickstart 验证记录，一个检查资源目录。
- T028 与 T029 可并行补充不同平台依赖体检回归保护。
- T034 与 T035 可并行更新文档与资源 README。

## 实施策略

1. 先完成 Phase 1 和 Phase 2，建立 Pandoc 资源目录和 OS 层接口。
2. 优先完成 US1 作为 MVP，让 Windows 文档上传可使用内置 Pandoc。
3. 再完成 US2，将资源纳入 MSI 并验证安装落点和 PATH。
4. 最后完成 US3，隐藏 Windows 依赖体检中的 Pandoc 项，同时保护 Linux 行为。
5. 每个用户故事完成后立即执行对应检查点，不把 MSI 和上传 smoke 全部堆到最后。

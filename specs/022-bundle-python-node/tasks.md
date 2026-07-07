# 任务：Windows 安装包内置 Python 与 Node 运行时

**输入**：`specs/022-bundle-python-node/` 下的设计文档

**前置条件**：`plan.md`、`spec.md`、`research.md`、`data-model.md`、`contracts/windows-runtime-installation.md`、`quickstart.md`

**测试**：本 feature 涉及 Windows 安装包和系统环境变量，任务包含构建检查、生成脚本检查、安装/卸载手动验收和 Rust 单元测试；不采用完整 TDD，但每个用户故事都有独立验证任务。

**组织方式**：任务按用户故事分组，保证 US1 可作为 MVP 独立交付，US2/US3 可在基础任务完成后增量验证。

## Phase 1: 准备（共享基础）

**目的**：确认上下文、输入源包和现有 Windows 打包模式，避免覆盖已有用户改动。

- [X] T001 阅读 `specs/022-bundle-python-node/plan.md`、`specs/022-bundle-python-node/contracts/windows-runtime-installation.md` 和 `specs/022-bundle-python-node/quickstart.md`，确认验收边界
- [X] T002 检查 `git status --short --branch` 并记录当前未提交文件，确保后续只修改本 feature 相关文件
- [X] T003 [P] 校验源包存在并记录大小：`C:\Users\z27014\Downloads\node-v24.18.0-win-x64.zip` 与 `C:\Users\z27014\Downloads\python-3.13.14-embed-amd64.zip`
- [X] T004 [P] 阅读现有 Windows resource 映射和 NSIS/WiX 模式：`pinvou3-app/src-tauri/tauri.conf.json`、`pinvou3-app/src-tauri/resources/windows/nsis/installer-hooks.nsh`、`pinvou3-app/src-tauri/resources/windows/*-path.wxs`
- [X] T005 [P] 阅读 Python 运行时解析逻辑：`pinvou3-app/src-tauri/src/bridge/paths.rs`

---

## Phase 2: 基础任务（阻塞后续故事）

**目的**：建立运行时准备脚本和安装包资源入口，所有用户故事依赖此阶段。

**关键**：本阶段完成前，不应开始修改安装器环境变量或 Python 解析行为。

- [X] T006 新增 Windows 运行时准备脚本 `pinvou3-app/scripts/prepare-windows-runtimes.ps1`，校验两个 zip 存在、可展开且分别包含 `pythonw.exe` 与 `node.exe`
- [X] T007 在 `pinvou3-app/scripts/prepare-windows-runtimes.ps1` 中实现目录规范化，把 Python 展开到 `pinvou3-app/src-tauri/resources/windows/python/`，把 Node 展开到 `pinvou3-app/src-tauri/resources/windows/node/`
- [X] T008 在 `pinvou3-app/scripts/prepare-windows-runtimes.ps1` 中添加失败时的清晰错误输出和非零退出，覆盖源包缺失、zip 损坏、关键文件缺失三类情况
- [X] T009 更新 `pinvou3-app/package.json` 的 Windows NSIS 构建前置流程，确保 `npm run build:nsis` 会先运行 `scripts/prepare-windows-runtimes.ps1`
- [X] T010 更新 `pinvou3-app/src-tauri/tauri.conf.json` 的 `bundle.resources`，把 `resources/windows/python/` 映射到安装目录 `python`，把 `resources/windows/node/` 映射到安装目录 `node`
- [X] T011 [P] 更新 `pinvou3-app/scripts/clean-nsis-staging.ps1`，避免 Tauri NSIS staging 中出现错误层级或根目录运行时残留
- [X] T012 运行 `powershell -NoProfile -ExecutionPolicy Bypass -File pinvou3-app/scripts/prepare-windows-runtimes.ps1`，确认 `pinvou3-app/src-tauri/resources/windows/python/pythonw.exe` 与 `pinvou3-app/src-tauri/resources/windows/node/node.exe` 存在

**检查点**：构建前可稳定生成 Windows runtime resource 目录，后续故事可以依赖 `python` 与 `node` 资源存在。

---

## Phase 3: 用户故事 1 - 无需本机 Python 即可使用 Python 型 MCP (Priority: P1) MVP

**目标**：应用在没有真实系统 Python、只有 WindowsApps 占位符时，仍优先使用内置 Python 启动 Python 型 MCP。

**独立测试**：在构建产物或模拟安装目录中存在 `python\pythonw.exe` 时，`paths::python_command()` 不返回 WindowsApps 占位符；安装后触发 `mcp_pinvou3_present_artifact` 不出现 `Python was not found`。

### 验证

- [X] T013 [P] [US1] 在 `pinvou3-app/src-tauri/src/bridge/paths.rs` 为 Windows Python 解析添加或更新单元测试，覆盖 `PINVOU3_PYTHON`、安装目录 `python\pythonw.exe` 和 WindowsApps alias 场景
- [X] T014 [P] [US1] 在 `specs/022-bundle-python-node/quickstart.md` 补充 US1 手动验收记录位置或命令，明确如何验证 `mcp_pinvou3_present_artifact` 不再触发 `Python was not found`

### 实现

- [X] T015 [US1] 更新 `pinvou3-app/src-tauri/src/bridge/paths.rs`，把 Windows 内置 Python 探测路径从 `python-win\pythonw.exe` 调整为 `python\pythonw.exe`
- [X] T016 [US1] 更新 `pinvou3-app/src-tauri/src/bridge/paths.rs`，让系统 Python 兜底跳过或验证 Microsoft Store `WindowsApps\python.exe` / `pythonw.exe` 占位符
- [X] T017 [US1] 更新 `pinvou3-app/src-tauri/src/bridge/paths.rs` 中 `python_command()` 的注释，说明 Windows 解析顺序为 `PINVOU3_PYTHON`、安装目录 `python\pythonw.exe`、真实系统 Python、最后兜底
- [X] T018 [US1] 运行 `cargo test python_command --manifest-path pinvou3-app/src-tauri/Cargo.toml` 或等价聚焦测试，验证 Python 解析逻辑

**检查点**：US1 可独立演示：内置 Python 可被解析，WindowsApps alias 不再被误认为真实 Python。

---

## Phase 4: 用户故事 2 - 安装后运行时目录布局稳定 (Priority: P2)

**目标**：Windows 安装后固定存在 `python\pythonw.exe` 与 `node\node.exe`，无源包版本目录多层嵌套。

**独立测试**：构建 NSIS 后检查 staging/installer 脚本和安装目录，确认资源最终位于 `$INSTDIR\python` 与 `$INSTDIR\node`。

### 验证

- [X] T019 [P] [US2] 运行 `npm run build:nsis`，确认构建过程会自动准备运行时资源并生成安装包
- [X] T020 [P] [US2] 检查 `pinvou3-app/src-tauri/target/release/nsis/x64/installer.nsi`，确认包含 `python\pythonw.exe` 与 `node\node.exe` 的安装路径且没有多余版本顶层目录

### 实现

- [X] T021 [US2] 调整 `pinvou3-app/scripts/prepare-windows-runtimes.ps1`，确保 Node 源包自带的 `node-v24.18.0-win-x64` 顶层目录不会出现在最终 `resources/windows/node/` 下
- [X] T022 [US2] 调整 `pinvou3-app/scripts/prepare-windows-runtimes.ps1`，确保 Python embeddable zip 文件直接落在最终 `resources/windows/python/` 下
- [X] T023 [US2] 如资源目录应由脚本生成而非人工维护，更新 `.gitignore` 或相关说明文件，明确 `pinvou3-app/src-tauri/resources/windows/python/` 与 `pinvou3-app/src-tauri/resources/windows/node/` 的维护策略
- [X] T024 [US2] 在 `pinvou3-app/src-tauri/resources/windows/` 新增或更新运行时 README，记录源 zip 路径、期望关键文件和目录布局

**检查点**：US2 可独立验证：构建产物中的运行时目录布局稳定且可排障。

---

## Phase 5: 用户故事 3 - 系统环境变量暴露内置运行时 (Priority: P3)

**目标**：安装后系统 `PINVOU3_PYTHON` 指向内置 `pythonw.exe`，系统 `PATH` 包含内置 `python` 与 `node`，卸载时只清理本应用管理的项。

**独立测试**：安装后新进程能读取正确环境变量；卸载后仅移除本应用安装目录相关项，不移除用户其他 Python/Node 路径。

### 验证

- [X] T025 [P] [US3] 在 `specs/022-bundle-python-node/quickstart.md` 确认安装后和卸载后的 PowerShell 验证命令覆盖 `PINVOU3_PYTHON` 与系统 `PATH`
- [X] T026 [P] [US3] 静态检查生成的 `pinvou3-app/src-tauri/target/release/nsis/x64/installer.nsi`，确认包含 `PINVOU3_PYTHON`、`$INSTDIR\python` 和 `$INSTDIR\node` 的设置与清理逻辑
- [X] T027 [P] [US3] 静态检查 WiX 配置 `pinvou3-app/src-tauri/resources/windows/python-node-path.wxs` 与 `pinvou3-app/src-tauri/tauri.conf.json`，确认 MSI 也声明 Python/Node 环境变量组件

### 实现

- [X] T028 [US3] 更新 `pinvou3-app/src-tauri/resources/windows/nsis/installer-hooks.nsh`，在 `NSIS_HOOK_POSTINSTALL` 设置 HKLM 系统环境变量 `PINVOU3_PYTHON=$INSTDIR\python\pythonw.exe`
- [X] T029 [US3] 更新 `pinvou3-app/src-tauri/resources/windows/nsis/installer-hooks.nsh`，在安装时把 `$INSTDIR\python` 与 `$INSTDIR\node` 追加到 HKLM 系统 `Path`，并避免重复追加
- [X] T030 [US3] 更新 `pinvou3-app/src-tauri/resources/windows/nsis/installer-hooks.nsh`，在卸载时仅当 `PINVOU3_PYTHON` 指向 `$INSTDIR\python\pythonw.exe` 时删除该变量
- [X] T031 [US3] 更新 `pinvou3-app/src-tauri/resources/windows/nsis/installer-hooks.nsh`，在卸载时仅从 HKLM 系统 `Path` 删除 `$INSTDIR\python` 与 `$INSTDIR\node`，保留其他 Python/Node 路径
- [X] T032 [US3] 更新 `pinvou3-app/src-tauri/resources/windows/nsis/installer-hooks.nsh`，安装和卸载环境变量变更后广播 `WM_SETTINGCHANGE`，让新进程尽快看到系统环境变量变化
- [X] T033 [US3] 新增 `pinvou3-app/src-tauri/resources/windows/python-node-path.wxs`，为 MSI 声明 `PINVOU3_PYTHON`、Python PATH 和 Node PATH 环境变量组件
- [X] T034 [US3] 更新 `pinvou3-app/src-tauri/tauri.conf.json` 的 WiX `fragmentPaths` 与 `componentRefs`，引用 `python-node-path.wxs` 中的环境变量组件

**检查点**：US3 可独立验证：安装/升级/卸载时系统环境变量行为符合契约。

---

## Phase 6: 收尾与横切关注点

- [X] T035 [P] 更新 `specs/022-bundle-python-node/quickstart.md`，记录实际执行过的构建命令、安装验证命令和未执行项原因
- [X] T036 [P] 更新 `specs/022-bundle-python-node/contracts/windows-runtime-installation.md`，同步最终采用的 NSIS/WiX 环境变量行为
- [X] T037 运行 `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`，确认 Rust 侧路径解析改动可编译
- [X] T038 运行 `npm run build:nsis`，确认 Windows NSIS 安装包可生成
- [X] T039 检查生成安装包大小并记录 Python/Node 运行时带来的体积变化，命令写入 `specs/022-bundle-python-node/quickstart.md`
- [ ] T040 手动安装生成的 NSIS 包并按 `specs/022-bundle-python-node/quickstart.md` 验证安装目录、`PINVOU3_PYTHON` 和系统 `PATH`
- [ ] T041 手动卸载应用并按 `specs/022-bundle-python-node/quickstart.md` 验证 `PINVOU3_PYTHON` 和系统 `PATH` 清理行为
- [X] T042 运行 `git diff --check` 并检查 `git status --short`，确认没有无关文件或敏感本地路径产物被意外纳入提交

---

## 依赖与执行顺序

- Phase 1 无依赖。
- Phase 2 阻塞所有用户故事，必须先完成运行时准备脚本和 Tauri resource 映射。
- US1 依赖 Phase 2，可作为 MVP 最先交付，解决 Python 型 MCP 启动失败。
- US2 依赖 Phase 2，可与 US1 的部分验证任务并行，但最终布局验证需要 runtime resource 已生成。
- US3 依赖 Phase 2，且建议在 US2 的最终目录布局稳定后实施，避免环境变量指向变化。
- Phase 6 在 US1-US3 完成后执行。

## 并行机会

- T003、T004、T005 可并行读取和校验上下文。
- T011 可与 T006-T010 部分并行，但合并前需确认脚本输出目录。
- T013、T014 可并行。
- T019、T020 可在构建完成后由不同人员并行检查不同产物。
- T025、T026、T027 可并行检查文档、NSIS 和 WiX。
- T035、T036 可并行更新文档。

## 并行执行示例

### US1

```text
并行：T013 更新路径解析测试；T014 补充 quickstart 验证说明
串行：T015 -> T016 -> T017 -> T018
```

### US2

```text
串行：T021 -> T022 -> T019 -> T020
并行：T023、T024 可在目录策略确定后并行处理
```

### US3

```text
并行：T025、T026、T027
串行：T028 -> T029 -> T030 -> T031 -> T032
并行：T033 与 T034 可在 NSIS 行为确定后一起完成
```

## 实施策略

1. 先完成 Phase 2 和 US1，形成 MVP：安装包能带内置 Python，Python 型 MCP 不再误用 WindowsApps alias。
2. 再完成 US2，锁定安装目录布局和构建产物可检查性。
3. 最后完成 US3，处理系统环境变量、PATH、升级和卸载清理。
4. 每个用户故事完成后立即按 quickstart 做独立验证，不把安装器问题留到最后集中排查。

## 格式校验

- 所有任务均使用 `- [ ] T###` markdown checkbox 格式。
- 所有用户故事阶段任务均包含 `[US1]`、`[US2]` 或 `[US3]` 标签。
- 标记 `[P]` 的任务均位于不同文件或不依赖同一未完成改动。
- 每个任务描述均包含明确文件路径或可执行命令。

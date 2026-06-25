# 任务：Windows 内置 7z

**输入**：`specs/017-bundle-windows-7z/` 下的设计文档

**前置条件**：plan.md（必需）、spec.md（用户故事必需）、research.md、data-model.md、contracts/

**测试**：本 feature 涉及 Windows 文件导入、外部工具路径、MSI 资源和跨平台依赖体检，必须包含与风险匹配的自动化测试和手动验收。

**组织方式**：任务按用户故事分组，保证每个故事可以独立实现和验证。

## 格式：`[ID] [P?] [Story] 描述`

- **[P]**：可并行执行，前提是修改不同文件且没有依赖关系。
- **[Story]**：任务对应的用户故事，例如 US1、US2、US3。
- 描述中必须包含精确文件路径。
- 文档、任务描述和验收说明默认使用中文；英文仅保留必要命令、路径、API 字段或原文。

## 路径约定

- **Tauri 桌面应用**：`pinvou3-app/src/`、`pinvou3-app/src-tauri/`
- **Windows 资源**：`pinvou3-app/src-tauri/resources/windows/`
- **OS 抽象层**：`pinvou3-app/src-tauri/src/os/`
- **文件导入**：`pinvou3-app/src-tauri/src/file_ingest.rs`
- **文档**：`specs/017-bundle-windows-7z/`

## Phase 1: 准备（共享基础）

**目的**：确认上下文、源资源、现有代码入口和验证方式。

- [X] T001 阅读 `specs/017-bundle-windows-7z/plan.md` 并确认宪章检查、项目结构和 RAR 支持结论
- [X] T002 阅读 `specs/017-bundle-windows-7z/contracts/windows-7z-runtime.md` 并确认 Windows 资源布局契约
- [X] T003 [P] 执行 `& "C:\Program Files\7-Zip\7z.exe" i` 并在 `specs/017-bundle-windows-7z/quickstart.md` 对照确认输出包含 `zip`、`7z`、`Rar`、`Rar5`
- [X] T004 [P] 使用 `git status --short --branch` 检查当前 worktree，确认仅处理 `specs/017-bundle-windows-7z/` 和计划涉及文件，避免覆盖用户修改

---

## Phase 2: 基础任务（阻塞后续故事）

**目的**：建立 Windows 内置 7z 资源和跨平台 OS 抽象骨架。

**⚠️ CRITICAL**：本阶段完成前，不应开始任何用户故事实现。

- [X] T005 创建 `pinvou3-app/src-tauri/resources/windows/7zip/` 并从 `C:\Program Files\7-Zip` 仅复制 `7z.exe`、`7z.dll`、`License.txt`、`readme.txt`
- [X] T006 [P] 在 `pinvou3-app/src-tauri/resources/windows/7zip/README.md` 记录 7-Zip 版本、来源目录、许可证、随附文件清单和已验证支持 zip/rar/7z
- [X] T007 [P] 检查 `pinvou3-app/src-tauri/resources/windows/7zip/` 不包含 `7-zip.dll`、`7-zip32.dll`、`7zFM.exe`、`7zG.exe`、`7z.sfx`、`7zCon.sfx`、`7-zip.chm`、`Uninstall.exe`、`Lang/` 等非 CLI 解析必需内容
- [X] T008 在 `pinvou3-app/src-tauri/tauri.conf.json` 的 `bundle.resources` 中增加 `resources/windows/7zip/` 到 `7zip` 的资源映射
- [X] T009 [P] 新建 `pinvou3-app/src-tauri/resources/windows/7zip-path.wxs`，定义安装目录 `7zip` 的 PATH 环境变量片段和唯一 WiX component id
- [X] T010 在 `pinvou3-app/src-tauri/tauri.conf.json` 的 Windows WiX 配置中加入 `resources/windows/7zip-path.wxs` 和对应 component ref
- [X] T011 在 `pinvou3-app/src-tauri/src/os/interface/system.rs` 增加 `archive_tool_path()`、`archive_tool_exists()`、`show_archive_dependency_check()`、`archive_dependency_packages()` 的接口函数
- [X] T012 在 `pinvou3-app/src-tauri/src/os/interface/mod.rs`、`pinvou3-app/src-tauri/src/os/mod.rs`、`pinvou3-app/src-tauri/src/os/windows/mod.rs`、`pinvou3-app/src-tauri/src/os/linux/mod.rs` 中导出 archive 相关 OS 接口
- [X] T013 在 `pinvou3-app/src-tauri/src/os/unsupported.rs` 中实现 archive 相关 OS 接口的清晰降级行为

**检查点**：Windows 资源骨架、bundle 配置和 OS 接口均已存在，可以开始按用户故事实施。

---

## Phase 3: 用户故事 1 - Windows 用户直接导入压缩包 (Priority: P1) 🎯 MVP

**目标**：Windows 用户没有系统级 7z 时，也能上传 zip、rar、7z 并完成现有压缩包解析流程。

**独立测试**：在 Windows 环境中让 `file_ingest.rs` 通过内置 `7zip/7z.exe` 执行 `l -slt` 和 `x`，上传 zip/rar/7z 样本均返回 `kind=archive` 且包含内部文件汇总。

### 测试 / 验证

- [X] T014 [P] [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_path.rs` 添加单元测试，验证安装目录含空格或中文时 `bundled_archive_tool_path_for_exe()` 返回 `{exe父目录}\7zip\7z.exe`
- [X] T015 [P] [US1] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 添加压缩包工具不可用时的 warning 回归测试，验证 Windows 提示不包含 `sudo apt install p7zip-full`
- [X] T016 [P] [US1] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 添加 archive 命令路径注入或辅助函数测试，验证列表预检和解压均使用 `crate::os::archive_tool_path()`

### 实现

- [X] T017 [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_path.rs` 增加 `bundled_archive_dir()`、`bundled_archive_tool_path()`、`bundled_archive_dir_for_exe()`、`bundled_archive_tool_path_for_exe()`，优先定位 `7zip\7z.exe`
- [X] T018 [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_system.rs` 实现 `archive_tool_path()` 和 `archive_tool_exists()`，优先使用内置 7z，必要时 fallback 到系统 `7z`
- [X] T019 [US1] 在 `pinvou3-app/src-tauri/src/os/linux/linux_system.rs` 实现 `archive_tool_path()` 和 `archive_tool_exists()`，保持返回和检测系统级 `7z`
- [X] T020 [US1] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 中将 `system_tools().sevenzip` 和 `command_exists("7z")` 相关判断改为 `crate::os::archive_tool_exists()`
- [X] T021 [US1] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 中将 `Command::new("7z")` 的列表预检和解压调用改为使用 `crate::os::archive_tool_path()`
- [X] T022 [US1] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 中将 Windows 压缩包工具缺失提示改为“内置压缩包解析组件缺失或不可用，请修复或重新安装 pinvou”，非 Windows 继续保留现有安装提示
- [X] T023 [US1] 运行聚焦 archive 回归测试验证 `pinvou3-app/src-tauri/src/file_ingest.rs` 的压缩包 OS 路径和缺失提示行为

**检查点**：US1 可独立演示和验证；Windows 上传 zip/rar/7z 不依赖系统级 7z。

---

## Phase 4: 用户故事 2 - 依赖体检不再误导 Windows 用户 (Priority: P2)

**目标**：Windows 依赖体检不再提示用户安装 p7zip/系统级 7z；Linux 仍保留该提示。

**独立测试**：在没有系统级 7z 的 Windows 环境运行依赖体检，压缩包项不显示为缺失外部依赖；Linux 缺失 7z 时仍提示 `p7zip-full`。

### 测试 / 验证

- [X] T024 [P] [US2] 在 `pinvou3-app/src-tauri/src/os/windows/windows_system.rs` 添加单元测试，验证 `show_archive_dependency_check()` 为 false 且 `archive_dependency_packages()` 为空
- [X] T025 [P] [US2] 在 `pinvou3-app/src-tauri/src/os/linux/linux_system.rs` 添加单元测试，验证 `show_archive_dependency_check()` 为 true 且 `archive_dependency_packages()` 为 `p7zip-full`
- [X] T026 [P] [US2] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 添加依赖体检回归测试，验证 Windows `check_dependencies()` 不返回需要安装 `p7zip-full` 的 archive 缺失项

### 实现

- [X] T027 [US2] 在 `pinvou3-app/src-tauri/src/os/windows/windows_system.rs` 实现 `show_archive_dependency_check()` 为 false 且 `archive_dependency_packages()` 为空字符串
- [X] T028 [US2] 在 `pinvou3-app/src-tauri/src/os/linux/linux_system.rs` 实现 `show_archive_dependency_check()` 为 true 且 `archive_dependency_packages()` 返回 `p7zip-full`
- [X] T029 [US2] 在 `pinvou3-app/src-tauri/src/os/unsupported.rs` 实现 archive 依赖体检的清晰降级返回值
- [X] T030 [US2] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 的 `check_dependencies()` 中按 `crate::os::show_archive_dependency_check()` 决定是否展示 archive 项，并使用 `archive_tool_exists()` 与 `archive_dependency_packages()`
- [X] T031 [US2] 检查 `pinvou3-app/src/index.html` 中 `dep_archive` 文案是否仍适用于 Windows 内置能力，如需调整仅更新依赖体检展示相关文案

**检查点**：US1 和 US2 均可独立验证；Windows 依赖体检不再误导安装 p7zip。

---

## Phase 5: 用户故事 3 - 非 Windows 平台行为保持稳定 (Priority: P3)

**目标**：Linux 等非 Windows 平台继续使用系统级 7z/p7zip，现有依赖体检和安装提示不发生非预期变化。

**独立测试**：在 Linux 相关 OS 层单测中确认 `archive_tool_path()` 返回 `7z`，缺失提示仍为 `p7zip-full`，一键安装白名单仍包含 `p7zip-full`。

### 测试 / 验证

- [X] T032 [P] [US3] 在 `pinvou3-app/src-tauri/src/os/linux/linux_system.rs` 添加单元测试，验证 Linux archive 工具路径和依赖包名保持系统依赖策略
- [X] T033 [P] [US3] 检查 `pinvou3-app/src-tauri/src/os/linux/linux_dependency.rs` 中 `p7zip-full` 仍在一键安装白名单
- [X] T034 [P] [US3] 在 `pinvou3-app/src-tauri/src/os/unsupported.rs` 添加或更新单元测试，验证 unsupported 平台不宣称 Windows 内置 7z 可用

### 实现

- [X] T035 [US3] 在 `pinvou3-app/src-tauri/src/os/linux/linux_system.rs` 保持 `archive_tool_exists()` 使用 `command_exists("7z")`，不得引用 Windows 内置资源路径
- [X] T036 [US3] 在 `pinvou3-app/src-tauri/src/os/linux/linux_dependency.rs` 确认 `p7zip-full` 白名单未被移除，如有缺失则恢复
- [X] T037 [US3] 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml os::linux --lib` 或记录当前 Windows 环境无法执行 Linux cfg 单测的原因和替代检查结果

**检查点**：所有计划内用户故事均可独立验证。

---

## Phase 6: 收尾与横切关注点

- [X] T038 [P] 更新 `specs/017-bundle-windows-7z/quickstart.md`，记录最终随附文件清单、实际 MSI 验证路径和 zip/rar/7z 样本结果
- [X] T039 [P] 检查 `specs/017-bundle-windows-7z/contracts/windows-7z-runtime.md` 与最终实现文件名、安装目录、裁剪清单和 PATH 策略一致
- [X] T040 运行 `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml` 验证 Rust 编译
- [X] T041 运行聚焦 archive/依赖体检测试验证压缩包导入相关路径
- [X] T042 在 Windows 构建 `pinvou3-app/src-tauri/target/release/bundle/msi/` 下的 MSI，并验证安装目录包含 `7zip\7z.exe`、`7zip\7z.dll`、许可证文件且不包含 Shell 插件、GUI、SFX、CHM、History、卸载器、语言包
- [ ] T043 在无系统级 7z 的 Windows 环境中按 `specs/017-bundle-windows-7z/quickstart.md` 上传 zip、rar、7z 样本并记录结果
- [X] T044 使用 `rg -n "Command::new\\(\"7z\"\\)|command_exists\\(\"7z\"\\)|p7zip-full" pinvou3-app/src-tauri/src` 检查硬编码残留，确认业务层不再直接调用 `7z` 且 Linux 依赖提示仍保留
- [X] T045 检查 DeepSeek-TUI 底座边界，确认本 feature 未修改 `DeepSeek-TUI/` 或重新实现底座能力

## 依赖与执行顺序

- Phase 1 无依赖。
- Phase 2 阻塞所有用户故事，必须先完成资源目录、bundle 配置和 OS 接口骨架。
- US1 是 MVP，依赖 Phase 2，完成后即可独立验证 Windows 压缩包上传。
- US2 依赖 Phase 2，可在 US1 的 OS 接口骨架完成后并行推进，但最终依赖 US1 的 `archive_tool_exists()` 语义。
- US3 依赖 Phase 2，可与 US1/US2 并行推进，重点保护 Linux 行为。
- Phase 6 依赖所有用户故事完成。

## 并行机会

- T003、T004 可并行做准备检查。
- T006、T008 可与 T005/T007 并行，因为分别修改 README 和 WiX 文件。
- T013、T014、T015 可并行编写测试，但实现前应统一预期。
- T023、T024、T025 可并行编写依赖体检测试。
- T031、T032、T033 可并行做 Linux/unsupported 回归检查。
- T037、T038 可与 T039/T040 的本地验证并行。

## 实施策略

1. 先完成 Phase 1 和 Phase 2，确保 Windows 资源和 OS 层抽象边界稳定。
2. 优先完成 US1 作为 MVP，让 Windows 上传 zip/rar/7z 可以脱离系统级 7z。
3. 接着完成 US2，避免依赖体检继续提示 Windows 用户安装 p7zip。
4. 最后完成 US3，确认 Linux 和 unsupported 平台行为未被 Windows 内置资源污染。
5. 每个故事完成后立即执行对应测试；MSI、安装目录和无系统级 7z 的验证放在收尾阶段完成。

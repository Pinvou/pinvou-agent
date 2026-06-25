# 任务：Windows 内置 Tesseract OCR

**输入**：`specs/015-bundle-windows-tesseract/` 下的设计文档

**前置条件**：`plan.md`、`spec.md`、`research.md`、`data-model.md`、`contracts/`、`quickstart.md`

**测试**：本 feature 涉及 Windows 安装包、外部命令调用、OCR 语言数据和依赖体检，任务包含 Rust 单测、`cargo check`、MSI 构建/安装检查和手动扫描件 PDF 验收。

**组织方式**：任务按用户故事分组，确保每个故事可以独立实现和验收。

## 格式：`[ID] [P?] [Story] 描述`

- **[P]**：可并行执行，前提是修改不同文件且没有依赖关系。
- **[Story]**：用户故事阶段任务必须标注 `US1`、`US2`、`US3`。
- 每个任务描述都包含明确文件路径或验证记录路径。

## Phase 1: 准备（共享基础）

**目的**：确认上下文、源数据和工作区状态，避免实现阶段猜测 Tesseract 来源。

- [X] T001 阅读并对齐 `specs/015-bundle-windows-tesseract/plan.md`、`specs/015-bundle-windows-tesseract/research.md`、`specs/015-bundle-windows-tesseract/contracts/windows-tesseract-runtime.md`
- [X] T002 检查当前 worktree 中 `pinvou3-app/src-tauri/src/file_ingest.rs`、`pinvou3-app/src-tauri/src/os/windows/windows_system.rs`、`pinvou3-app/src-tauri/src/os/windows/windows_path.rs` 是否存在未提交冲突或用户改动
- [X] T003 [P] 在 `pinvou3-app/src-tauri/resources/windows/tesseract/README.md` 记录 Tesseract 源为 `C:\Program Files\Tesseract-OCR`、版本 `v5.5.0.20241111`、必需语言包和裁剪原则
- [X] T004 [P] 在 `specs/015-bundle-windows-tesseract/quickstart.md` 补充实际验收样本路径或样本准备说明

---

## Phase 2: 基础任务（阻塞后续故事）

**目的**：导入受控 runtime，并建立后续代码和打包任务共同依赖的资源落点。

**检查点**：完成后，仓库中应存在最小可运行的 Windows Tesseract 资源目录。

- [X] T005 从 `C:\Program Files\Tesseract-OCR` 导入最小运行集到 `pinvou3-app/src-tauri/resources/windows/tesseract/`
- [X] T006 复制 `C:\Program Files\Tesseract-OCR\tessdata\chi_sim.traineddata` 和 `C:\Program Files\Tesseract-OCR\tessdata\eng.traineddata` 到 `pinvou3-app/src-tauri/resources/windows/tesseract/tessdata/`
- [X] T007 复制 `C:\Program Files\Tesseract-OCR\doc\LICENSE` 和 `C:\Program Files\Tesseract-OCR\doc\README.md` 到 `pinvou3-app/src-tauri/resources/windows/tesseract/`
- [X] T008 [P] 在 `pinvou3-app/src-tauri/resources/windows/tesseract/README.md` 写明哪些训练工具、man page 或无关语言数据未被纳入 MSI，以及对应理由

---

## Phase 3: 用户故事 1 - Windows 安装后可识别扫描件 PDF (Priority: P1) MVP

**目标**：Windows 用户安装 pinvou 后，无需手动安装 Tesseract，即可上传中英文扫描件 PDF 并得到 OCR 文本。

**独立测试**：在未安装系统级 Tesseract 的 Windows 环境安装 MSI，上传 3 页以内中英文扫描件 PDF，确认 30 秒内返回可读 OCR 文本，并提示 OCR 可能存在识别误差。

### 测试 / 验证

- [X] T009 [P] [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_path.rs` 添加单元测试，覆盖 `{安装目录}/tesseract`、`tesseract.exe`、`tessdata` 在空格和中文路径下的解析
- [X] T010 [P] [US1] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 添加或更新单元测试，验证 OCR 命令通过 OS 层获取 Tesseract 路径、语言参数和 `tessdata` 参数
- [X] T011 [P] [US1] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 添加或更新回归测试，确认普通图片上传仍不走 Tesseract 主路径

### 实现

- [X] T012 [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_path.rs` 实现 `bundled_tesseract_dir`、`bundled_tesseract_tool_path`、`bundled_tessdata_dir` 及对应 `_for_exe` 测试辅助函数
- [X] T013 [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_system.rs` 实现 Windows OCR 工具解析、`tessdata` 解析、内置路径加入当前进程 `PATH` 的逻辑
- [X] T014 [US1] 在 `pinvou3-app/src-tauri/src/os/interface/system.rs`、`pinvou3-app/src-tauri/src/os/interface/mod.rs`、`pinvou3-app/src-tauri/src/os/mod.rs` 导出 OCR 路径、OCR 可用性、OCR 语言参数和 `tessdata` 目录能力
- [X] T015 [US1] 在 `pinvou3-app/src-tauri/src/os/linux/linux_system.rs` 和 `pinvou3-app/src-tauri/src/os/unsupported.rs` 补齐 OCR OS 接口的 Linux/unsupported 实现，保持 Linux 现有系统包行为
- [X] T016 [US1] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 将 `Command::new("tesseract")` 改为 `HiddenCommand` 加 OS 层路径，并在 Windows 内置场景显式传入 `--tessdata-dir`
- [X] T017 [US1] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 将 `system_tools().tesseract` 从 `command_exists("tesseract")` 改为 OS 层 OCR 可用性判断
- [X] T018 [US1] 在 `pinvou3-app/src-tauri/tauri.conf.json` 添加 `resources/windows/tesseract/` 到 `tesseract` 的资源映射，确保 MSI 安装后释放到 `{安装目录}/tesseract`
- [X] T019 [US1] 在 `pinvou3-app/src-tauri/resources/windows/tesseract-path.wxs` 添加 Tesseract PATH 环境变量片段，并在 `pinvou3-app/src-tauri/tauri.conf.json` 的 WiX `fragmentPaths` 和 `componentRefs` 中引用

**检查点**：US1 可独立演示；Windows 安装目录存在 `tesseract` 后，扫描件 PDF OCR 使用内置 Tesseract。

---

## Phase 4: 用户故事 2 - 依赖体检不再要求 Windows 用户手动补装 OCR (Priority: P2)

**目标**：Windows 依赖体检不再把 Tesseract 展示为用户必须手动安装的阻断项；损坏时提示修复安装。

**独立测试**：在干净 Windows 环境安装 pinvou 后打开依赖体检页，不出现手动安装 Tesseract 的阻断提示；删除内置 OCR 后，提示指向修复安装或重新安装 pinvou。

### 测试 / 验证

- [X] T020 [P] [US2] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 添加或更新依赖体检单元测试，确认 Windows 策略不会留下 `tesseract-ocr` 或 `tesseract-ocr-chi-sim` 手动安装提示
- [X] T021 [P] [US2] 在 `pinvou3-app/src-tauri/src/os/linux/linux_system.rs` 添加或更新单元测试，确认 Linux OCR 依赖提示仍包含 `tesseract-ocr tesseract-ocr-chi-sim poppler-utils`

### 实现

- [X] T022 [US2] 在 `pinvou3-app/src-tauri/src/os/interface/system.rs`、`pinvou3-app/src-tauri/src/os/interface/mod.rs`、`pinvou3-app/src-tauri/src/os/mod.rs` 增加并导出 `show_ocr_dependency_check`
- [X] T023 [US2] 在 `pinvou3-app/src-tauri/src/os/windows/windows_system.rs` 实现 `show_ocr_dependency_check` 为 Windows 内置策略，并将 OCR 缺失提示改为修复或重新安装 pinvou
- [X] T024 [US2] 在 `pinvou3-app/src-tauri/src/os/linux/linux_system.rs` 和 `pinvou3-app/src-tauri/src/os/unsupported.rs` 实现 `show_ocr_dependency_check`，Linux 保持展示 OCR 系统包依赖
- [X] T025 [US2] 在 `pinvou3-app/src-tauri/src/file_ingest.rs` 的 `check_dependencies()` 中按 `show_ocr_dependency_check()` 控制 `ocr` 项展示，并保持 Poppler/Pandoc/ASR 项行为不变

**检查点**：US1 和 US2 均可独立验收；Windows 不再引导用户手动安装 Tesseract。

---

## Phase 5: 用户故事 3 - 维护者可复现 Windows OCR 安装验收 (Priority: P3)

**目标**：维护者能确认安装包包含 OCR runtime、语言数据、许可证，并能按固定步骤验收扫描件 PDF。

**独立测试**：维护者按 quickstart 检查 MSI 安装目录和样本扫描件 PDF，能记录 OCR 成功、失败或质量不足的明确结果。

### 测试 / 验证

- [X] T026 [P] [US3] 在 `specs/015-bundle-windows-tesseract/quickstart.md` 补充 MSI 解包或安装目录检查命令，覆盖 `tesseract.exe`、`chi_sim.traineddata`、`eng.traineddata`、许可证文件
- [X] T027 [P] [US3] 在 `specs/015-bundle-windows-tesseract/quickstart.md` 补充破坏性验收记录格式，覆盖删除 `tesseract.exe` 和删除 `chi_sim.traineddata`

### 实现

- [X] T028 [US3] 在 `pinvou3-app/src-tauri/resources/windows/tesseract/README.md` 补充 runtime 文件清单、语言数据清单、许可证位置和来源说明
- [X] T029 [US3] 在 `pinvou3-app/src-tauri/resources/windows/tesseract/README.md` 记录最终 MSI 体积影响和后续裁剪注意事项
- [X] T030 [US3] 执行 `npm run tauri build` 并将 MSI 中 `tesseract` 目录检查结果记录到 `specs/015-bundle-windows-tesseract/quickstart.md`

**检查点**：所有计划内用户故事均可独立验收。

---

## Phase 6: 收尾与横切关注点

- [X] T031 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_path --lib` 并将结果记录到 `specs/015-bundle-windows-tesseract/quickstart.md`
- [X] T032 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml file_ingest --lib` 并将结果记录到 `specs/015-bundle-windows-tesseract/quickstart.md`
- [X] T033 运行 `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml` 并将结果记录到 `specs/015-bundle-windows-tesseract/quickstart.md`
- [X] T034 在干净 Windows 环境执行中英文扫描件 PDF 上传验收，并将结果记录到 `specs/015-bundle-windows-tesseract/quickstart.md`
- [X] T035 检查 `pinvou3-app/src-tauri/src/file_ingest.rs`、`pinvou3-app/src-tauri/src/os/windows/windows_system.rs`、`pinvou3-app/src-tauri/src/os/linux/linux_system.rs` 未引入 DeepSeek-TUI 底座改动或跨平台回归
- [X] T036 检查 `specs/015-bundle-windows-tesseract/spec.md`、`specs/015-bundle-windows-tesseract/plan.md`、`specs/015-bundle-windows-tesseract/tasks.md` 与最终实现保持一致

---

## 依赖与执行顺序

- Phase 1 无依赖。
- Phase 2 阻塞所有用户故事，因为 US1/US2/US3 都依赖内置资源落点。
- US1 是 MVP，必须先完成，交付扫描件 PDF OCR 基础能力。
- US2 依赖 US1 的 OS 层 OCR 可用性能力，但可独立验收依赖体检行为。
- US3 依赖 Phase 2 的资源目录和 US1 的 MSI 资源映射，但文档补充任务可与 US2 并行。
- Phase 6 在所有用户故事完成后执行。

## 并行机会

- T003 和 T004 可并行，因为分别更新资源 README 与 quickstart。
- T009、T010、T011 可并行，因为测试覆盖不同关注点。
- T020 和 T021 可并行，因为分别覆盖 Windows 依赖体检和 Linux 依赖提示。
- T026、T027 可并行补充 quickstart 中不同验收段落，但合并时需避免同文件冲突。
- US2 的 Linux 保持项与 US3 的 README/quickstart 文档任务可并行推进。

## 并行执行示例

```text
# US1 测试先行
T009: 更新 windows_path 单元测试
T010: 更新 file_ingest OCR 命令测试
T011: 更新普通图片不走 OCR 回归测试
```

```text
# US2 平台策略验证
T020: 更新 Windows 依赖体检测试
T021: 更新 Linux OCR 依赖提示测试
```

```text
# US3 验收文档
T026: 补充安装目录检查命令
T027: 补充破坏性验收记录格式
```

## 实施策略

1. 先完成 Phase 1 和 Phase 2，锁定 Tesseract 源数据和资源目录。
2. 完成 US1 作为 MVP，确保扫描件 PDF 在 Windows 安装后可 OCR。
3. 完成 US2，避免依赖体检继续误导用户手动安装 Tesseract。
4. 完成 US3 和 Phase 6，补齐 MSI 验收、许可证来源和回归记录。

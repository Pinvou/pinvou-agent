# 研究：Windows 内置 7z

## 决策 1：沿用 Windows 内置资源模式

**Decision**: 将 Windows 压缩包解析运行时作为 Tauri bundle resource 随 MSI 安装，运行时优先使用安装目录下的 `7zip/`。

**Rationale**: 项目已经对 Poppler、Pandoc、Tesseract、ASR 采用相同模式。复用该模式能减少 Windows 用户手动安装依赖，同时保持 Linux 仍走系统包管理和一键安装白名单。

**Alternatives considered**:

- 使用 Windows `Expand-Archive`：仅覆盖 zip，不满足当前 zip/rar/7z 范围。
- 改用 Rust 原生库：zip 可行，但 7z/rar 覆盖和安全预检需要较大改动，不适合本 feature 的小步目标。
- 要求用户手动安装 7-Zip：不能解决 Windows 开箱即用问题。

## 决策 2：业务层调用 OS 层压缩包工具路径

**Decision**: 新增 OS 层压缩包工具能力，例如 `archive_tool_path()`、`archive_tool_exists()`、`show_archive_dependency_check()`、`archive_dependency_packages()`，让 `file_ingest.rs` 不再直接 `Command::new("7z")`。

**Rationale**: 当前业务层直接硬编码 `7z`，无法区分 Windows 内置资源和 Linux 系统命令。OS 层抽象已经承载 Poppler/Pandoc/Tesseract/ASR 的平台差异，新增 archive 能力符合现有边界。

**Alternatives considered**:

- 在 `file_ingest.rs` 中使用 `cfg!(windows)`：会把平台判断重新带回业务层，不符合近期 OS 层收敛方向。
- 修改 PATH 后继续 `Command::new("7z")`：对当前进程和 MSI 环境变量顺序依赖更强，测试粒度较差。

## 决策 3：使用完整 7-Zip 安装目录作为源包

**Decision**: 使用 `C:\Program Files\7-Zip` 作为 Windows 内置 7z 源目录，并裁剪为 `7z.exe`、`7z.dll`、`License.txt`、`readme.txt`、`README.md` 作为随附资源。

**Rationale**: 已执行 `C:\Program Files\7-Zip\7z.exe i`，输出包含 `7z`、`zip`、`Rar`、`Rar5`，并列出 `Rar1/2/3/5` 解码器。另将 `7z.exe` 和 `7z.dll` 单独复制到临时目录后执行 `7z.exe i`，确认 CLI 可加载本地 `7z.dll` 并保留 RAR/RAR5 支持。GUI 程序、帮助文档、SFX、卸载器和语言包不参与 `file_ingest.rs` 的 `l -slt`/`x` 调用，应裁剪以控制 MSI 体积。

**Alternatives considered**:

- 使用精简版 `7za.exe` 源包：格式覆盖不如完整 7-Zip 安装目录，不能稳定满足当前规格。
- 复制完整 `C:\Program Files\7-Zip` 目录：实现最简单，但会打包 GUI、帮助文档、SFX、卸载器和语言包等不必要内容，增加 MSI 体积。
- 从代码层对 rar 特判为不支持：与当前规格和用户可见能力不一致。
- 保留系统 `7z` fallback：可作为兜底，但不应作为 Windows 开箱即用的主要路径。

## 决策 4：依赖体检按平台拆分

**Decision**: Windows 依赖体检不再提示 `p7zip-full` 或系统级 `7z`；Linux 保持现有 `archive` 检查和 `p7zip-full` 安装提示。

**Rationale**: Windows 安装包提供内置资源后，提示用户安装 Linux 包名没有意义；Linux 的一键安装链路仍依赖系统包，不能被 Windows 内置资源影响。

**Alternatives considered**:

- 所有平台都隐藏压缩包依赖：会破坏 Linux 用户的依赖诊断。
- Windows 继续显示 archive 项但安装包名为空：容易让用户困惑，除非 UI 明确表达“已内置”。本 feature 优先沿用 Poppler/Pandoc/Tesseract 的 Windows 隐藏策略。

## 决策 5：安全预检逻辑保持不变

**Decision**: 继续使用列表预检和解压后递归 ingest 的现有流程，只替换工具路径来源。

**Rationale**: 现有 `7z l -slt` 预检已用于条目数和解压后总大小限制；`7z x` 解压后仍复用当前嵌套压缩包不展开规则。替换工具来源不应扩大攻击面。

**Alternatives considered**:

- 直接解压后再统计：会削弱压缩炸弹拦截。
- 为不同格式重写解析器：范围过大，且会重复已有稳定流程。

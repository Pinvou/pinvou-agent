# 研究：Windows 内置 Pandoc 安装

## 决策 1：Pandoc 资源进入 Tauri app 的 Windows 资源目录

**Decision**：将 `C:\Users\z27014\Downloads\pandoc-3.10` 复制为仓库内受控资源，目标目录为 `pinvou3-app/src-tauri/resources/windows/pandoc/`。

**Rationale**：用户要求将该目录作为源文件数据放入 pinvou Windows 项目资源文件夹。当前源目录根部包含 `pandoc.exe`、`COPYING.rtf`、`COPYRIGHT.txt` 和 `MANUAL.html`，复制为同级目录后可以直接安装到 `{安装目录}/pandoc`，并与用户要求的 PATH 目录一致。

**Alternatives considered**：
- 继续要求用户手动安装 Pandoc：被拒绝，无法满足开箱可用。
- 只在构建时下载 Pandoc：被拒绝，会引入构建环境不稳定和外部网络依赖。
- 放入通用 `resources/bundle`：被拒绝，该目录用于应用内 bundle 解包到用户数据目录，不等同于 Windows 安装目录运行时。

## 决策 2：Windows 运行时优先使用安装目录内置 Pandoc

**Decision**：Windows 下文档解析命令优先解析 `{当前可执行文件目录}/pandoc/pandoc.exe`；找不到时再按既有系统 PATH 行为降级。

**Rationale**：安装目录内置资源是本 feature 的受控运行时，优先使用它可以避免用户机器上的其他 Pandoc 版本影响文档解析结果。降级到 PATH 有助于开发态或安装内容异常时保持可诊断性。

**Alternatives considered**：
- 只依赖全局 PATH：被拒绝，不能保证受控版本优先。
- 在 `file_ingest` 中硬编码 Windows 路径：被拒绝，平台差异应在 OS 层封装。
- 启动时永久修改进程 PATH：可作为补充，但命令调用仍应使用明确路径，减少 PATH 顺序不确定性。

## 决策 3：安装包负责释放资源，应用负责运行时可发现性

**Decision**：MSI 打包配置负责把 Pandoc 文件安装到 `{安装目录}/pandoc`；应用启动或命令调用前负责让当前进程能找到该目录，必要时也可配合安装器写入用户/系统 PATH。

**Rationale**：用户明确要求“安装的时候释放到安装目录”和“加入环境变量”。打包配置和运行时发现性各自有清晰责任：安装器提供文件落点，应用保证自身文档解析可用。

**Alternatives considered**：
- 仅安装文件、不处理 PATH/运行时发现：被拒绝，`Command::new("pandoc")` 仍可能找不到。
- 仅修改 PATH、不使用安装目录路径：被拒绝，无法保证 pinvou 使用受控资源。

## 决策 4：Windows 依赖体检隐藏 Pandoc/现代文档解析项

**Decision**：Windows 下依赖体检不再展示 Pandoc/现代文档解析作为用户需手动补全的依赖；Linux 仍展示 `pandoc` 相关依赖。

**Rationale**：Windows 安装版已经内置 Pandoc，继续提示用户手动补全会制造误导。Linux 仍依赖系统包，原行为应保留。

**Alternatives considered**：
- 展示为“已内置”：可选，但用户要求是去掉检查；如后续需要透明度，可在非错误信息区展示“现代文档能力已随安装提供”。
- 全平台去掉 Pandoc 检查：被拒绝，会破坏 Linux 现有依赖引导。

## 决策 5：安装异常错误指向“修复安装”

**Decision**：Windows 下如果内置 Pandoc 缺失或不可执行，相关文档上传提示应说明安装内容异常或需要修复安装，不再提示用户自行安装 Pandoc。

**Rationale**：内置运行时意味着 Pandoc 的可用性成为安装包完整性问题，而不是用户手动依赖问题。错误信息应帮助用户或支持人员判断 MSI/安装目录是否异常。

**Alternatives considered**：
- 继续显示 `sudo apt install pandoc` 或手动安装 Pandoc：被拒绝，Windows 场景错误且与本 feature 目标冲突。
- 静默降级为 binary 附件：被拒绝，会让用户以为文档已被正常解析。

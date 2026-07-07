# 研究：Windows 安装包内置 Python 与 Node 运行时

## 决策 1：使用指定离线 zip 作为构建输入，但安装包内包含展开后的目录

**Decision**：打包前读取 `C:\Users\z27014\Downloads\node-v24.18.0-win-x64.zip` 和 `C:\Users\z27014\Downloads\python-3.13.14-embed-amd64.zip`，校验关键文件后展开到 `pinvou3-app/src-tauri/resources/windows/node/` 与 `pinvou3-app/src-tauri/resources/windows/python/`，再由 Tauri resource 机制打进 Windows 安装包。

**Rationale**：用户要求“安装时解压到安装目录”，但 Tauri resource 更适合打包文件树。打包前展开可避免 NSIS 安装阶段再依赖解压命令、临时目录和错误处理，同时最终安装效果仍是安装目录下的 `python` 与 `node` 目录。

**Alternatives considered**：

- 直接把 zip 打进安装包，NSIS 安装时解压：会增加安装器脚本复杂度，也需要确保安装时解压工具可用。
- 运行时首次启动再解压：会让核心 MCP 首次启动依赖应用逻辑，失败更晚暴露，不适合作为安装契约。

## 决策 2：Windows 安装目录使用 `python` 与 `node` 固定目录名

**Decision**：最终安装目录固定为 `$INSTDIR\python` 和 `$INSTDIR\node`，并确保关键文件分别为 `python\pythonw.exe` 与 `node\node.exe`。

**Rationale**：规格明确要求目录名；固定目录也能让环境变量、PATH、排障文档和 `paths::python_command()` 保持简单稳定。

**Alternatives considered**：

- 保留源包自带版本目录名：会导致路径包含 `node-v24.18.0-win-x64` 等版本层级，不利于升级和环境变量稳定。
- 使用 `python-win`：现有注释曾提到该目录，但与本 feature 规格不一致，需迁移到 `python`。

## 决策 3：NSIS 通过 hook 管理系统环境变量，WiX 通过 fragment 管理

**Decision**：NSIS 安装包在 `NSIS_HOOK_POSTINSTALL` 中设置 `PINVOU3_PYTHON`，并把 `$INSTDIR\python`、`$INSTDIR\node` 加入 HKLM 系统 PATH；在卸载 hook 中只删除指向本安装目录的变量和值。MSI/WiX 侧沿用现有 `*-path.wxs` 模式新增 Python/Node PATH 和 `PINVOU3_PYTHON` 环境变量组件。

**Rationale**：项目当前 NSIS 是主要交付方式，已有 `installer-hooks.nsh`；MSI 配置也已有 poppler/pandoc/tesseract/7zip PATH fragment，复用该模式可保持一致。

**Alternatives considered**：

- 只依赖 Tauri resource，不写系统环境变量：不满足用户补充需求。
- 只改 NSIS，不改 MSI：会造成两种 Windows 包行为不一致，后续维护容易踩坑。

## 决策 4：运行时解析优先 `PINVOU3_PYTHON`，其次安装目录 `python\pythonw.exe`

**Decision**：`paths::python_command()` 的 Windows 分支继续优先读取 `PINVOU3_PYTHON`；内置目录探测从旧的 `python-win\pythonw.exe` 调整为 `python\pythonw.exe`。系统 Python 兜底需要跳过或验证 Microsoft Store WindowsApps 占位符。

**Rationale**：环境变量是安装器设置的系统契约；安装目录探测是环境变量未生效或开发场景下的兜底。跳过 WindowsApps alias 是解决 `Python was not found` 的关键。

**Alternatives considered**：

- 仅依赖 PATH 解析：PATH 中可能已有用户 Python 或 WindowsApps 占位符，不够确定。
- 仅依赖 `PINVOU3_PYTHON`：旧安装或开发运行时可能没有该变量，需要本地目录兜底。

## 决策 5：源包校验作为构建前置步骤

**Decision**：新增 Windows runtime 准备脚本，校验两个 zip 存在、可展开、关键文件存在，并在失败时中止构建。

**Rationale**：缺运行时的安装包会导致用户安装后才发现核心功能不可用。构建前失败更便于维护者修复源包路径或内容。

**Alternatives considered**：

- 仅在文档里要求人工检查：容易漏检。
- 安装阶段检查：错误暴露给最终用户，且安装器回滚和提示复杂度更高。

## 决策 6：暂不内置第三方 Python 包

**Decision**：本 feature 首先保证 Python 解释器可用，不把 `python-pptx`、`python-docx` 等第三方依赖纳入本次必须交付。

**Rationale**：规格假设里已明确第三方 Python 依赖由后续规划决定；本次优先修复 `present_artifact`、天气、问财等 stdlib 型 MCP 启动失败。

**Alternatives considered**：

- 同步 vendor 所有 Python 依赖：会扩大安装包体积与测试范围，且超出当前明确需求。

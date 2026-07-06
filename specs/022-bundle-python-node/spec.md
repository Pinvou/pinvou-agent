# 功能规格：Windows 安装包内置 Python 与 Node 运行时

**功能分支**：`022-bundle-python-node`

**创建日期**：2026-07-06

**状态**：Draft

**输入**：用户描述：“将 `C:\Users\z27014\Downloads\node-v24.18.0-win-x64.zip` 和 `C:\Users\z27014\Downloads\python-3.13.14-embed-amd64.zip` 作为数据源，打包在 Windows 的安装包中，并且在安装的时候，解压到安装目录下的 `python` 及 `node` 目录，并在系统环境变量中将 `PINVOU3_PYTHON` 指向解压后的 `pythonw.exe`；补充需求：`node` 及 `python` 目录需要添加到系统环境变量 `PATH` 中”

## 用户场景与测试 *(必填)*

### 用户故事 1 - 无需本机 Python 即可使用 Python 型 MCP (Priority: P1)

Windows 用户在未安装真实 Python、且系统 `PATH` 中存在 Microsoft Store Python 占位符的机器上安装应用后，应用内依赖 Python 的本地 MCP 能力仍可正常启动，尤其是成品卡展示能力不再因为 `Python was not found` 失败。

**为什么是此优先级**：成品卡是应用产出交付的核心体验，提示词会反复要求调用该能力；如果 Python 运行时缺失，用户每次产出成品都会遇到稳定失败。

**独立测试**：在干净 Windows 环境中卸载或禁用真实 Python，仅保留 WindowsApps Python 占位符，安装应用后触发一次成品卡展示，确认工具可用且不依赖系统 Python。

**验收场景**：
1. **Given** Windows 机器没有真实 Python，**When** 用户通过 Windows 安装包安装应用并启动，**Then** 应用可使用随安装包提供的 Python 运行时启动本地 Python 型 MCP。
2. **Given** 系统 `PATH` 中的 `python.exe` 是 Microsoft Store 占位符，**When** 应用启动 Python 型 MCP，**Then** 不应调用该占位符，也不应出现 `Python was not found`。

---

### 用户故事 2 - 安装后运行时目录布局稳定 (Priority: P2)

维护者或支持人员需要在用户机器上快速判断内置运行时是否正确安装，因此安装完成后 Python 与 Node 必须位于固定、可预测的安装目录子目录中。

**为什么是此优先级**：稳定目录布局可以降低支持成本，也避免压缩包自带顶层目录导致运行时路径多嵌套一层。

**独立测试**：安装应用后检查安装目录，确认 `python` 与 `node` 子目录存在，且关键可执行文件位于预期位置。

**验收场景**：
1. **Given** 用户完成 Windows 安装，**When** 检查应用安装目录，**Then** 应存在 `python\pythonw.exe` 与 `node\node.exe`。
2. **Given** 源压缩包自身包含版本号顶层目录，**When** 安装器展开运行时，**Then** 最终目录不应出现影响调用的多余嵌套层级。

---

### 用户故事 3 - 系统环境变量暴露内置运行时 (Priority: P3)

管理员、应用子进程或后续工具需要通过统一环境变量定位应用内置运行时，因此安装完成后系统环境变量 `PINVOU3_PYTHON` 应指向安装目录中的 `pythonw.exe`，系统 `PATH` 应包含安装目录下的 `python` 与 `node` 子目录。

**为什么是此优先级**：环境变量为应用运行、排障和子进程启动提供统一入口；同时保留对现有 `paths::python_command()` 覆盖机制的兼容，并允许需要命令解析的流程直接找到内置 Python 与 Node。

**独立测试**：安装完成后打开新的系统会话，读取系统环境变量，确认 `PINVOU3_PYTHON` 为安装目录下的 `python\pythonw.exe` 绝对路径，且系统 `PATH` 包含安装目录下的 `python` 与 `node` 子目录。

**验收场景**：
1. **Given** 用户完成 Windows 安装，**When** 在新进程中读取 `PINVOU3_PYTHON`，**Then** 其值应指向已存在的 `python\pythonw.exe`。
2. **Given** 用户完成 Windows 安装，**When** 在新进程中读取系统 `PATH`，**Then** 应包含安装目录下的 `python` 与 `node` 子目录。
3. **Given** 用户卸载应用，**When** `PINVOU3_PYTHON` 当前值或 `PATH` 中的运行时目录仍指向该应用安装目录，**Then** 卸载流程应清理这些由本应用创建的环境变量项，避免留下无效路径。

### 边界情况

- 源 zip 文件缺失、损坏或内容不符合预期时，打包流程应失败并给出清晰错误，而不是生成缺运行时的安装包。
- 安装目录包含空格或非 ASCII 字符时，`PINVOU3_PYTHON` 与 `PATH` 中的运行时目录仍应保存可用的绝对路径。
- 用户机器已有真实 Python 或 Node 时，应用仍应优先使用本次安装包提供的内置运行时，不应依赖用户环境。
- 用户已有 `PINVOU3_PYTHON` 或 `PATH` 中已有其他 Python/Node 目录时，安装后的应用应使用本次安装的内置运行时；卸载时只清理由本应用安装目录创建的值。
- 重复安装、覆盖安装或升级安装后，运行时目录和环境变量应保持一致，不应保留旧版本残留路径。

## 需求 *(必填)*

### 功能需求

- **FR-001**: Windows 安装包 MUST 包含来自 `C:\Users\z27014\Downloads\node-v24.18.0-win-x64.zip` 的 Node 运行时内容。
- **FR-002**: Windows 安装包 MUST 包含来自 `C:\Users\z27014\Downloads\python-3.13.14-embed-amd64.zip` 的 Python 运行时内容。
- **FR-003**: 安装完成后，系统 MUST 在应用安装目录下提供 `python` 子目录，并确保其中存在可执行的 `pythonw.exe`。
- **FR-004**: 安装完成后，系统 MUST 在应用安装目录下提供 `node` 子目录，并确保其中存在可执行的 `node.exe`。
- **FR-005**: 安装流程 MUST 将系统环境变量 `PINVOU3_PYTHON` 设置为安装目录下 `python\pythonw.exe` 的绝对路径。
- **FR-006**: 安装流程 MUST 将安装目录下的 `python` 与 `node` 子目录加入系统环境变量 `PATH`。
- **FR-007**: 应用启动和本地 Python 型 MCP 启动时 MUST 能通过安装后的配置定位到内置 Python，而不是误用 Microsoft Store Python 占位符。
- **FR-008**: 打包或安装流程 MUST 规范化 zip 展开后的目录结构，确保最终运行时路径稳定，不受源压缩包顶层目录名称影响。
- **FR-009**: 升级或覆盖安装 MUST 更新 `PINVOU3_PYTHON` 和系统 `PATH` 中由本应用管理的运行时目录到当前安装目录。
- **FR-010**: 卸载流程 MUST 在确认 `PINVOU3_PYTHON` 指向本应用安装目录时移除该变量；如果变量指向用户自定义位置，则不得删除用户自定义值。
- **FR-011**: 卸载流程 MUST 从系统 `PATH` 中移除指向本应用安装目录下 `python` 与 `node` 子目录的条目；不得移除用户或其他软件管理的 Python/Node 路径。
- **FR-012**: 安装包生成前 MUST 验证两个源 zip 文件可读取且包含预期关键文件，验证失败时不得继续生成可发布安装包。

### 关键实体 *(涉及数据或文档结构时填写)*

- **运行时源包**：用于构建 Windows 安装包的离线 zip 文件，包含源路径、期望关键文件、版本标识和校验状态。
- **安装后的运行时目录**：应用安装目录下的 `python` 与 `node` 子目录，代表用户机器上实际可用的内置运行时。
- **运行时环境变量**：系统级 `PINVOU3_PYTHON` 与系统 `PATH` 条目，用于让应用和子进程稳定定位内置 Python 与 Node。

## 成功标准 *(必填)*

### 可度量结果

- **SC-001**: 在没有真实系统 Python 的 Windows 环境中，安装后 100% 的核心 Python 型 MCP 启动检查通过，不再出现 `Python was not found`。
- **SC-002**: 安装完成后，`python\pythonw.exe`、`node\node.exe`、`PINVOU3_PYTHON` 和系统 `PATH` 中的两个运行时目录四类检查均在 10 秒内可验证通过。
- **SC-003**: 使用新安装包完成全新安装、覆盖安装和卸载三类流程时，运行时目录与环境变量结果均符合验收场景。
- **SC-004**: 打包流程在源包缺失或内容异常时能够稳定阻止发布，避免产生缺少 Python 或 Node 的 Windows 安装包。

## 假设

- 本 feature 仅覆盖 Windows 安装包；Linux 与 macOS 的运行时分发不在本次范围内。
- 指定的两个 zip 文件为可信、已预下载的数据源，且由维护者在打包前放置到固定路径。
- Node 运行时要求安装到应用目录下的 `node` 子目录，并通过系统 `PATH` 暴露；不要求额外设置 Node 专用环境变量。
- Python 运行时优先满足 `present_artifact`、天气、同花顺问财等 Python 型 MCP 的启动需求；第三方 Python 依赖是否随包内置由后续规划决定。
- 系统环境变量变更需要在新的进程或会话中生效；已启动进程不要求自动感知变更。

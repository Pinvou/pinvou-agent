# 功能规格：Windows 内置 Poppler 安装

**功能分支**：`010-bundle-windows-poppler`

**创建日期**：2026-06-23

**状态**：Draft

**输入**：用户描述：“关于 poppler 在 windows 下的安装，采用以下策略：1 将 C:\Users\z27014\Downloads\poppler-26.02.0 作为 poppler 的源文件数据，放到 pinvou windows 项目的资源文件夹下。2 打包的时候将此资源打包到 msi 中 3 安装的时候，将此资源释放到 {安装目录}/poppler 文件夹下 4 将 {安装目录}/poppler 加入环境变量 5 去掉依赖检查页中关于 poppler 的检查”

## 用户场景与测试 *(必填)*

### 用户故事 1 - Windows 安装后 PDF 能力开箱可用 (Priority: P1)

Windows 用户安装 pinvou 后，无需手动下载或配置 Poppler，即可上传普通 PDF 附件并获得 PDF 文本解析能力。

**为什么是此优先级**：这是当前 Windows PDF 上传体验的关键阻塞点；用户不应为了基础 PDF 能力理解或配置外部命令行工具。

**独立测试**：在一台未预装 Poppler 且未配置 Poppler PATH 的 Windows 环境中安装 pinvou，上传包含文字层的 PDF，验证附件解析流程不再因为缺少 Poppler/pdftotext 而失败或给出 Poppler 缺失提示。

**验收场景**：

1. **Given** Windows 机器未安装 Poppler，**When** 用户通过 MSI 安装 pinvou 后上传文字层 PDF，**Then** PDF 能被解析为可发送附件内容。
2. **Given** Windows 用户刚完成安装，**When** 用户首次打开应用并上传 PDF，**Then** 用户不需要执行任何手动 Poppler 安装步骤。

---

### 用户故事 2 - 安装包携带受控 Poppler 运行时 (Priority: P2)

发布人员在构建 Windows 安装包时，能够确认指定 Poppler 发行内容被纳入安装包，并随应用安装到应用安装目录下的 `poppler` 位置。

**为什么是此优先级**：受控运行时可以避免不同用户机器上的 Poppler 版本、路径、安装方式不一致，降低支持成本。

**独立测试**：构建 Windows MSI 后检查安装结果，确认 Poppler 文件随应用安装，并位于应用安装目录的 `poppler` 目录下。

**验收场景**：

1. **Given** Windows MSI 构建完成，**When** 在干净环境安装该 MSI，**Then** 安装目录下存在 `poppler` 文件夹且包含可用于 PDF 文本提取的 Poppler 可执行文件。
2. **Given** 安装目录被检查，**When** 对比内置 Poppler 来源版本，**Then** 安装结果应与批准的 Poppler 源内容一致。

---

### 用户故事 3 - 依赖体检不再要求用户补 Poppler (Priority: P3)

Windows 用户查看依赖体检时，不再看到 Poppler/PDF 文本提取缺失项，也不会被引导去手动安装 Poppler。

**为什么是此优先级**：在 Poppler 已由安装包内置后，依赖体检继续显示 Poppler 缺失会造成误导。

**独立测试**：在 Windows 安装版应用中打开依赖体检，验证 Poppler/PDF 文本提取不作为用户需手动补全的依赖项展示。

**验收场景**：

1. **Given** Windows 安装版应用已包含内置 Poppler，**When** 用户打开依赖体检，**Then** 体检页不显示 Poppler 或 PDF 文本提取为缺失依赖。
2. **Given** 其他依赖仍可能缺失，**When** 用户打开依赖体检，**Then** 体检页仍能展示与 Poppler 无关的缺失项。

### 边界情况

- 如果安装目录路径包含空格或中文字符，内置 Poppler 仍应可被应用找到并使用。
- 如果用户机器已有其他 Poppler 版本，pinvou 的 PDF 解析应优先使用随应用安装的受控 Poppler 能力，避免被外部版本差异影响。
- 如果内置 Poppler 文件缺失、被删除或不可执行，PDF 上传应给出明确、可理解的错误提示，并避免继续提示用户通过依赖体检手动安装 Poppler。
- 如果 Windows 安装包未包含 Poppler 资源，发布验收应能在安装前或安装后发现该问题。
- 非 Windows 平台不受本功能影响，既有依赖体检与安装策略保持原状。

## 需求 *(必填)*

### 功能需求

- **FR-001**: Windows 发布包 MUST 包含批准的 Poppler 运行时内容，来源为 `C:\Users\z27014\Downloads\poppler-26.02.0` 对应的 Poppler 发行目录。
- **FR-002**: Windows 安装完成后，Poppler 运行时 MUST 位于应用安装目录下的 `poppler` 文件夹。
- **FR-003**: Windows 安装完成后，应用 MUST 能在未手动安装 Poppler、未手动配置 Poppler 路径的环境中执行 PDF 文本提取。
- **FR-004**: Windows 安装完成后，系统环境或应用运行环境 MUST 能解析到安装目录下的 Poppler 可执行文件位置，使 PDF 能力对应用可用。
- **FR-005**: Windows 依赖体检 MUST 不再将 Poppler 或 PDF 文本提取作为用户需要手动补全的缺失依赖项展示。
- **FR-006**: 依赖体检 MUST 继续展示与 Poppler 无关的其他缺失依赖项，不得因隐藏 Poppler 检查而移除其他能力检查。
- **FR-007**: 如果内置 Poppler 缺失或不可用，PDF 上传流程 MUST 返回明确错误，说明安装内容异常或需要修复安装，而不是要求用户自行安装 Poppler。
- **FR-008**: Windows 安装、升级和重新安装流程 MUST 保持 Poppler 运行时可用，且不得要求用户重复手动配置。

### 关键实体

- **Poppler 运行时包**：随 Windows 安装包分发的 PDF 文本提取能力文件集合，包含执行 PDF 文本提取所需文件。
- **Windows 安装目录**：用户安装 pinvou 后应用所在目录，内置 Poppler 应位于其下的 `poppler` 子目录。
- **依赖体检项**：设置页向用户展示的可选文件解析能力状态；本功能要求 Windows 下不再把 Poppler 作为用户手动补全项。
- **PDF 附件解析结果**：用户上传 PDF 后产生的附件解析状态，应该反映 PDF 内容是否被成功读取或安装内容是否异常。

## 成功标准 *(必填)*

### 可度量结果

- **SC-001**: 在未预装 Poppler 的干净 Windows 环境中，安装 pinvou 后上传文字层 PDF 的成功率达到 100%。
- **SC-002**: Windows 安装完成后，95% 的用户无需任何额外配置即可在首次会话中上传并发送 PDF 附件。
- **SC-003**: Windows 依赖体检页中与 Poppler/PDF 文本提取相关的手动安装提示减少到 0 条。
- **SC-004**: 安装目录检查能够在 1 分钟内确认 `poppler` 文件夹存在且包含 PDF 文本提取所需文件。
- **SC-005**: 当内置 Poppler 缺失或损坏时，用户收到的错误提示能明确指向“安装内容异常/需要修复安装”，不再指向手动安装 Poppler。

## 假设

- 本功能仅针对 Windows 安装版；开发态、便携运行方式和非 Windows 平台不在本次核心范围内。
- `C:\Users\z27014\Downloads\poppler-26.02.0` 是已批准用于随 pinvou Windows 安装包分发的 Poppler 源内容。
- Windows 安装包允许携带第三方运行时文件，且 Poppler 发行许可已被项目接受。
- 应用安装目录在安装后对普通用户可读，足以供应用调用内置 Poppler 能力。
- “去掉依赖检查页中关于 Poppler 的检查”仅指 Windows 上不再把 Poppler/PDF 文本提取显示为用户需手动补全的依赖；其他依赖检查保持可见。

# 功能规格：Windows MSG 邮件解析

**功能分支**：`016-windows-msg-parser`

**创建日期**：2026-06-25

**状态**：Draft

**输入**：用户描述：“Windows下使用msg_parser解析msg文件，移除对libemail-outlook-message-perl的依赖”

## 用户场景与测试 *(必填)*

### 用户故事 1 - Windows 直接解析 Outlook MSG 文件 (Priority: P1)

Windows 用户上传或导入 Outlook `.msg` 邮件附件时，系统能够直接提取邮件可读内容，无需用户额外安装 Perl、`msgconvert` 或 Linux 包。

**为什么是此优先级**：`.msg` 是企业邮件归档和附件流转的常见格式；当前依赖 Linux 包名会让 Windows 用户误以为缺少不可安装的模块，影响附件导入体验。

**独立测试**：在一台未安装 Perl、未安装 `msgconvert` 的 Windows 环境中导入包含发件人、收件人、主题、正文和附件名的 `.msg` 文件，确认系统返回可读邮件内容且不提示缺失 `libemail-outlook-message-perl`。

**验收场景**：
1. **Given** Windows 环境没有 Perl 和 `msgconvert`，**When** 用户上传有效 Outlook `.msg` 文件，**Then** 系统展示邮件发件人、收件人、主题、日期、正文和附件名。
2. **Given** `.msg` 邮件包含中文主题和中文正文，**When** 用户导入该文件，**Then** 系统保留中文内容且不出现乱码。
3. **Given** `.msg` 邮件包含附件，**When** 用户导入该文件，**Then** 系统至少列出附件文件名，且邮件正文解析不受附件存在影响。

---

### 用户故事 2 - 依赖体检不再误报 Windows 邮件依赖 (Priority: P2)

Windows 用户查看“依赖体检”时，邮件解析能力不再显示 Linux 专用包 `libemail-outlook-message-perl`，也不再要求用户安装 `msgconvert` 才能处理 `.msg`。

**为什么是此优先级**：依赖体检是用户排查附件导入失败的入口，展示不可安装的 Linux 包会导致错误操作和支持成本。

**独立测试**：在 Windows 环境打开依赖体检页，确认邮件项不会展示 `libemail-outlook-message-perl`，并能准确反映 `.eml` 与 `.msg` 的可用状态。

**验收场景**：
1. **Given** 用户运行 Windows 版应用，**When** 打开依赖体检，**Then** 邮件项不出现 `libemail-outlook-message-perl` 或 Linux 安装命令。
2. **Given** Windows 邮件解析能力可用，**When** 用户查看依赖体检，**Then** 邮件项显示为可用或不再阻塞用户导入 `.msg`。

---

### 用户故事 3 - 保持 EML 和 Linux 行为稳定 (Priority: P3)

现有 `.eml` 邮件解析能力保持不变；Linux 上的邮件依赖安装和 `.msg` 转换行为不因 Windows 改造而退化。

**为什么是此优先级**：本次改造目标是解决 Windows 依赖问题，不能破坏已有邮件附件解析和 Linux 一键安装流程。

**独立测试**：分别在 Windows 和 Linux 环境导入 `.eml`；在 Linux 环境导入 `.msg` 并验证现有依赖提示仍然可用。

**验收场景**：
1. **Given** 用户导入 `.eml` 文件，**When** 系统解析邮件，**Then** 输出字段和当前版本保持一致。
2. **Given** Linux 环境缺少邮件转换工具，**When** 用户导入 `.msg` 或查看依赖体检，**Then** 系统仍能给出适用于 Linux 的安装提示。

### 边界情况

- `.msg` 文件损坏、不是 Outlook MSG 格式或缺少关键邮件字段时，系统应给出可理解的失败提示，不应崩溃。
- `.msg` 只有 HTML 正文或只有纯文本正文时，系统应尽可能提取可读正文。
- `.msg` 包含多个收件人、抄送、密送或长主题时，系统应保留可读信息并避免截断关键字段。
- `.msg` 附件体积较大或附件内容无法解析时，系统至少保留附件名，不应把附件解析失败视为邮件正文解析失败。
- Windows 环境变量中存在旧的 `msgconvert` 时，系统行为应以 Windows 内置/原生解析能力为准，避免依赖外部命令造成差异。

## 需求 *(必填)*

### 功能需求

- **FR-001**: 系统 MUST 在 Windows 上直接解析 Outlook `.msg` 文件并生成与 `.eml` 类似的可读邮件文本。
- **FR-002**: 系统 MUST 从 `.msg` 中提取发件人、收件人、主题、日期、正文和附件名；字段缺失时必须优雅降级。
- **FR-003**: 系统 MUST 支持中文 `.msg` 邮件内容，包括中文发件人显示名、主题、正文和附件名。
- **FR-004**: 系统 MUST 从 Windows 邮件解析路径中移除对 `libemail-outlook-message-perl`、Perl 运行时和 `msgconvert` 的必需依赖。
- **FR-005**: Windows 依赖体检 MUST 不再提示用户安装 `libemail-outlook-message-perl` 或 Linux 专用邮件依赖。
- **FR-006**: `.eml` 文件解析 MUST 保持现有输出结构和用户可见行为不变。
- **FR-007**: Linux 平台现有 `.msg` 解析依赖提示和一键安装能力 MUST 保持可用。
- **FR-008**: 解析失败时，系统 MUST 返回包含失败原因的用户可理解提示，并保留文件名、文件类型和大小等基础信息。
- **FR-009**: 系统 MUST 避免在 Windows 上弹出解析过程中的命令行窗口或外部安装提示。

### 关键实体

- **邮件附件**：用户导入的 `.eml` 或 `.msg` 文件，包含文件名、路径、大小、扩展名和解析结果。
- **邮件解析结果**：系统生成的可读文本，包含邮件头、正文、附件名、token 估算和可选警告。
- **依赖体检项**：前端展示的能力状态，描述邮件解析是否可用以及缺失时的补全方式。
- **平台能力**：Windows、Linux 等平台对邮件格式解析的不同可用能力和补全策略。

## 成功标准 *(必填)*

### 可度量结果

- **SC-001**: 在未安装 Perl 和 `msgconvert` 的 Windows 验证环境中，用户导入有效 `.msg` 文件的成功率达到 95% 以上。
- **SC-002**: Windows `.msg` 解析结果在 5 秒内返回给用户，且不会出现命令行弹窗。
- **SC-003**: Windows 依赖体检中不再出现 `libemail-outlook-message-perl`、Perl 或 Linux 安装命令相关提示。
- **SC-004**: 现有 `.eml` 解析回归测试通过率保持 100%，输出字段不发生用户可见退化。
- **SC-005**: 对损坏或不受支持的 `.msg` 文件，系统能够在 5 秒内返回明确失败提示，应用不崩溃。

## 假设

- Windows 用户主要需要从 `.msg` 中获得邮件头、正文和附件名，用于后续对话上下文，不要求本功能递归解析附件内容。
- `.eml` 解析能力沿用当前行为，本功能只改变 Windows `.msg` 的解析方式和依赖提示。
- Linux 仍可保留现有邮件转换工具和安装提示；本功能不要求替换 Linux 路径。
- 用户指定的原生 MSG 解析方案将在计划阶段评估并落实；本规格以用户可见能力和验收结果为准。

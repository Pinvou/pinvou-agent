# Feature Specification: Windows 迁移与接手维护文档

**Feature Branch**: `001-windows-onboarding-docs`

**Created**: 2026-06-15

**Status**: Draft

**Input**: User description: "作为一个Windows应用开发工程师，我刚接触到这个项目，我需要对此项目迁移到Windows系统上，并对其进行此项目的迭代和维护。现在请你帮我认真、仔细、全面地梳理此项目，理清楚此项目的调用流程、依赖项目、注意要点，以及其他你认为对我重要的事项，并输出为文档"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - 快速理解项目全貌 (Priority: P1)

Windows 应用开发工程师首次接触项目时，可以通过一份集中化文档理解项目定位、模块边界、调用流程、底座依赖、运行数据目录和主要维护红线。

**Why this priority**: 没有全貌说明，后续 Windows 迁移容易误把底座能力重新实现，或遗漏 submodule、vLLM、本地工具链等关键依赖。

**Independent Test**: 让一名未参与项目的工程师阅读文档后，能够用自己的话说明 pinvou3-app、DeepSeek-TUI、vLLM、前端 bridge、Rust command、EnginePool、session/workflow 的关系，并指出哪些能力不能在 pinvou3 重造。

**Acceptance Scenarios**:

1. **Given** 工程师只拥有当前仓库，**When** 阅读文档的架构与调用流程章节，**Then** 能说清一次聊天请求从前端到 LLM 再回到 UI 的完整链路。
2. **Given** 工程师准备修改工具或 agent 能力，**When** 阅读维护红线章节，**Then** 能判断应改 SKILL、command、MCP server、pinvou3-app 还是 DeepSeek-TUI fork。

---

### User Story 2 - 识别 Windows 迁移工作面 (Priority: P2)

Windows 应用开发工程师可以根据文档列出当前代码中偏 Linux 的依赖、命令、路径、安装/更新机制、Tauri 打包目标和系统工具探测方式，并形成迁移检查清单。

**Why this priority**: 当前项目主线以 Linux `.deb` 和本地 GB10/vLLM 为默认环境，Windows 迁移的主要风险来自系统集成差异，而不是业务代码本身。

**Independent Test**: 工程师阅读文档后，能够列出至少 10 项 Windows 迁移风险，并为每项标出涉及模块和建议处理方向。

**Acceptance Scenarios**:

1. **Given** 工程师准备在 Windows 上运行开发环境，**When** 阅读 Windows 迁移章节，**Then** 能知道需处理 submodule 初始化、Rust/Tauri 工具链、Node 依赖、本地 vLLM 地址、外部文档解析工具和环境变量。
2. **Given** 工程师准备移植安装/升级能力，**When** 阅读依赖与打包章节，**Then** 能知道 `.deb`、`apt`、`pkexec`、`which`、Linux GUI 打开命令和 WebKit/GTK 环境变量是高风险项。

---

### User Story 3 - 支撑后续迭代维护 (Priority: P3)

工程师可以用文档作为后续维护入口，定位常见功能的代码路径、测试守护、文档依据和验证命令，减少改动前的摸索成本。

**Why this priority**: 项目包含 Tauri 前端、Rust bridge、DeepSeek-TUI fork、workflow 脚本、bundle 资源、MCP server 和本地推理配置，维护必须有稳定导航。

**Independent Test**: 工程师收到一个常见迭代需求时，可以在文档中找到对应修改入口、风险提示和推荐验证步骤。

**Acceptance Scenarios**:

1. **Given** 需求是新增领域 agent 或工具组合，**When** 查阅文档，**Then** 能优先选择 SKILL/command/MCP，而不是新增 Rust tool loop。
2. **Given** 需求是同步 DeepSeek-TUI 上游，**When** 查阅文档，**Then** 能找到 fork guard、system prompt diff、tool catalog 检查和 submodule 注意事项。

### Edge Cases

- 当前工作区的 `DeepSeek-TUI/` 子模块目录可能尚未初始化，文档必须明确提醒初始化步骤和未初始化时的症状。
- 当前文档与实际代码可能随上游同步而漂移，文档必须标注以 `docs/fork-modifications.md`、`process.md` 和 git log 作为持续更新依据。
- Windows 原生运行、WSL 运行、远程 GB10 推理三种模式边界不同，文档必须避免把一种模式的依赖误描述成全部模式都必需。
- 项目中有已推翻或已下线方案，文档必须标注它们的当前状态，避免工程师按 archived 方案继续开发。

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: 文档 MUST 说明项目定位、主目录职责、底座依赖边界，以及 pinvou3-app 与 DeepSeek-TUI 的分工。
- **FR-002**: 文档 MUST 描述聊天主链路，从前端发送消息到 Rust command、EnginePool、AppEngine、DeepSeek-TUI EngineHandle、vLLM 请求、事件回传与 session 持久化。
- **FR-003**: 文档 MUST 描述工作流链路，包括工作流定义、bundle 同步、scheduler/gate 脚本、SubAgent 派发和前端事件。
- **FR-004**: 文档 MUST 列出运行与构建依赖，包括 Rust/Tauri、Node、DeepSeek-TUI submodule、本地 vLLM、系统文档解析工具和更新/安装机制。
- **FR-005**: 文档 MUST 专门列出 Windows 迁移风险点，并对每个风险给出影响范围与建议处理方向。
- **FR-006**: 文档 MUST 标注维护红线，尤其是不得在 pinvou3 重写 DeepSeek-TUI 已有 Engine、ToolRegistry、SSE、Session、SkillRegistry、Commands、MCP、Hooks、Cycle、Compaction 能力。
- **FR-007**: 文档 MUST 指出 fork 修改和 fork guard 的重要性，并给出上游同步后必须验证的项目。
- **FR-008**: 文档 MUST 说明本地数据目录布局、session/workflow/artifacts/settings/bundle 的位置与用途。
- **FR-009**: 文档 MUST 提供新工程师常见任务的定位表，使其能快速判断应修改哪一层。
- **FR-010**: 文档 MUST 使用中文撰写，面向 Windows 应用开发工程师，避免只给项目老成员才能理解的隐含语境。

### Key Entities

- **接手工程师**: 目标读者，需要理解项目并负责 Windows 迁移与后续维护。
- **项目梳理文档**: 面向工程师的长期维护入口，包含架构、调用链、依赖、迁移风险和验证流程。
- **迁移风险项**: 与 Windows 运行环境不兼容或需适配的行为、工具、路径、打包方式或系统 API。
- **维护红线**: 项目边界约束，用于避免重复实现底座能力或破坏 fork 契约。
- **验证清单**: 工程师改动后应运行或人工检查的项目，用于降低回归风险。

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 新工程师在阅读文档 60 分钟内，能够准确描述至少 8 个核心模块及其职责。
- **SC-002**: 新工程师能够根据文档列出至少 10 个 Windows 迁移风险点，并为每项指出涉及目录或模块。
- **SC-003**: 对 5 类常见维护需求，工程师能够在 5 分钟内定位推荐修改入口和验证方式。
- **SC-004**: 文档覆盖的关键调用流程至少包括聊天、附件 ingestion、session 持久化、工作流、更新安装、fork 同步 6 条链路。
- **SC-005**: 文档中不得出现与当前项目状态冲突的已下线方案作为推荐实现路径。

## Assumptions

- 当前目标是帮助 Windows 应用开发工程师接手和迁移项目，而不是立即完成全部 Windows 代码适配。
- 文档可以基于当前仓库、已有设计文档和源码进行梳理；后续迁移实施可继续通过 `/speckit-plan` 和 `/speckit-tasks` 分解。
- Windows 首版迁移可选择原生 Windows 或 WSL 辅助开发，但文档需要明确哪些依赖当前是 Linux 预设。
- DeepSeek-TUI 仍是底座，pinvou3-app 继续作为 Tauri UI 与 Engine 配置/编排层。

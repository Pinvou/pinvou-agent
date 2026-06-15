<!--
Sync Impact Report
Version change: template -> 1.0.0
Modified principles:
- template principle 1 -> I. 中文文档优先
- template principle 2 -> II. DeepSeek-TUI 底座优先
- template principle 3 -> III. 本地算力与数据边界
- template principle 4 -> IV. 小步高质量变更
- template principle 5 -> V. 可测试性与可验证交付
- Added: VI. 可维护性与长期演进
Added sections:
- 技术边界
- 开发流程与质量门禁
Removed sections:
- Placeholder-only template sections
Templates requiring updates:
- ✅ .specify/templates/constitution-template.md
- ✅ .specify/templates/plan-template.md
- ✅ .specify/templates/spec-template.md
- ✅ .specify/templates/tasks-template.md
- ⚪ .specify/templates/commands/*.md (not present in this repository)
Runtime guidance:
- ✅ AGENTS.md reviewed; existing project rules already align
- ✅ README.md reviewed; no constitution reference update required
Follow-up TODOs:
- None
-->
# pinvou3 项目宪章

## Core Principles

### I. 中文文档优先

凡是能用中文清晰表达的项目文档、规格、计划、任务、交接说明、注释性设计记录和用户可见说明，MUST 使用中文输出。保留英文仅限于代码标识符、命令、API 字段、错误原文、外部协议名称、第三方专有名词或英文原文引用。面向非中文协作者的材料 MAY 追加英文版本，但中文版本必须是主版本或同等完整版本。

**Rationale**: pinvou3 的主要设计语境、维护记录和用户沟通以中文为主。中文优先能降低接手成本，避免重要约束只存在于口头语境或混杂说明中。

### II. DeepSeek-TUI 底座优先

DeepSeek-TUI 是 pinvou3 的 agent 底座。pinvou3 MUST NOT 重新实现 DeepSeek-TUI 已有的 Engine、ToolRegistry、流式 SSE、Session、SkillRegistry、Commands 路由、MCP client、Hooks、Cycle 或 Compaction。扩展领域能力时 MUST 优先使用既有扩展点：领域 agent 或工具组合走 `SKILL.md`， slash command 走 `~/.deepseek/commands/*.md`，外部 API 走独立 MCP server，LLM 行为引导走 `.deepseek/instructions.md`，Tauri UI、Rust wrapper 和 Engine 配置才放在 `pinvou3-app/`。

通用底座 bug 或通用优化 SHOULD 在 DeepSeek-TUI fork 中以小补丁修复，并视通用性向上游提交 PR；pinvou3 专用行为 MUST 留在 fork 或 app 层并明确标注。

**Rationale**: 项目价值来自复用成熟底座并在桌面体验、配置、工作流和本地算力场景上做薄而清晰的编排。重复造轮子会增加维护面并破坏上游同步能力。

### III. 本地算力与数据边界

设计 MUST 以本地 GB10 + Qwen3.6-35B-A3B-FP8 + vLLM 的当前能力为基线。默认路径 SHOULD 优先支持本地或用户明确配置的 OpenAI-compatible endpoint；外发数据、远端模型、外部 API 或网络搜索能力必须由配置、用户意图或明确产品场景驱动，不得作为隐式默认前提。

涉及用户文件、session、artifact、workflow run、settings、bundle、persona 等数据时，MUST 明确落盘位置和生命周期。跨平台迁移时 MUST 保护用户数据目录、敏感路径拦截和本地模型配置的可理解性。

**Rationale**: pinvou3 的核心场景是本地算力桌面助手。架构和默认配置必须尊重数据主权、可预测成本和离线/内网可用性。

### IV. 小步高质量变更

代码变更 MUST 小步、聚焦、可审查。实现应遵循当前代码结构和本地模式，避免无关重构、跨层耦合和大范围格式化。新增抽象 MUST 证明能降低真实复杂度、减少重复或匹配已有架构边界。对 DeepSeek-TUI fork 的改动 MUST 尽量小、可定位、可由 fork 文档和守护测试追踪。

每个改动 SHOULD 先读相关源码、已有文档和 git 历史，再做实现判断。遇到用户或他人已有 worktree 改动时，MUST 与其共存，不得擅自回滚无关修改。

**Rationale**: 当前项目同时包含 Tauri app、Rust bridge、底座 fork、workflow 脚本和大量设计决策。小步变更能降低同步、回归和交接风险。

### V. 可测试性与可验证交付

每个功能、修复、迁移或文档交付 MUST 定义验证方式。代码变更 SHOULD 根据风险选择对应测试：低风险局部改动可用静态检查或聚焦单测；跨模块、底座 fork、session、工具、workflow、附件、更新、打包和 Windows 迁移相关改动 MUST 有更强验证，例如 `cargo test`、fork guard、端到端 smoke、手动验收清单或明确未执行说明。

涉及规格和计划的工作 MUST 保持用户故事、成功标准、数据模型、契约和任务之间可追踪。生成任务时 SHOULD 优先按可独立验证的用户故事组织。

**Rationale**: pinvou3 依赖本地模型和底座事件链路，许多问题只能通过契约、回归测试或真实链路验证发现。没有验证方式的交付不可维护。

### VI. 可维护性与长期演进

长期维护信息 MUST 留在仓库中可检索的位置。架构约束、fork 修改、同步流程、迁移风险、已下线方案和已知问题 SHOULD 记录在 `AGENTS.md`、`process.md`、`docs/`、Spec Kit artifacts 或 commit message 中。文档与代码不一致时，维护者 MUST 更新文档或在变更说明中标出过期范围。

上游同步、Windows 迁移、打包更新、附件解析、工作流、prompt composer、256K context、tool blocklist 等高风险区域 MUST 有明确 owner 文件、验证步骤和回滚思路。已废弃设计 MUST 标注状态，避免后续工程师按 archived 方案继续实现。

**Rationale**: 项目的复杂度主要来自多层集成和长期演进。可维护性要求决策可追溯、边界可解释、风险可验证。

## 技术边界

- `pinvou3-app/` 是 Tauri 2.0 + EngineHandle wrapper 主线，负责 UI、设置、桥接、session/workflow 编排和底座配置。
- `DeepSeek-TUI/` 是 submodule/fork，负责 agent 底座能力；fork 改动必须遵循底座优先原则和 fork 文档。
- `docs/fork-modifications.md` 是 DeepSeek-TUI fork 修改的单一真相源；同步上游后必须运行 fork guard 并检查 system prompt、工具集合和动态工具激活路径。
- `process.md` 是跨阶段状态、待办和已知问题的长期记录；重大方向调整必须同步更新。
- Windows 迁移 MUST 优先处理路径、外部工具探测、安装更新、打包目标、用户数据目录和 vLLM endpoint 配置，不得把 Linux-only 机制直接平移为默认。

## 开发流程与质量门禁

1. 规格阶段 MUST 写清用户价值、可独立验证的用户故事、边界条件、成功标准和假设。
2. 计划阶段 MUST 执行宪章检查，说明是否触碰底座边界、本地算力边界、测试门禁、可维护性和中文文档要求。
3. 任务阶段 MUST 将工作拆成可验证的步骤，并标注并行安全性、依赖顺序和涉及文件。
4. 实现阶段 MUST 优先遵循现有代码模式；新增依赖、跨平台分支、fork 改动和外部工具调用必须说明必要性。
5. 验证阶段 MUST 报告实际执行的检查和未执行原因。高风险改动不得只以“未运行测试”收尾，必须给出补验路径。
6. 文档阶段 MUST 使用中文更新相关说明；英文外部术语可保留，但核心解释必须中文可读。

## Governance

本宪章优先于普通开发习惯和临时偏好。任何规格、计划、任务、实现或评审如果违反本宪章，MUST 在计划或评审中记录原因、风险和替代方案；无法合理解释的违反项不得继续推进。

宪章修订流程：

- 新增原则、实质扩展治理范围或改变项目边界时，版本号 MINOR 增加。
- 删除原则、弱化既有约束或改变底座/数据边界时，版本号 MAJOR 增加。
- 文字澄清、示例补充、格式修正且不改变治理语义时，版本号 PATCH 增加。
- 每次修订 MUST 更新 Sync Impact Report、版本号、修订日期，并同步检查 Spec Kit 模板和运行时指导文档。

合规评审期望：

- `/speckit-specify` 产物 MUST 能映射到本宪章原则。
- `/speckit-plan` MUST 在 Constitution Check 中评估中文文档、底座边界、本地算力、质量、测试和维护性。
- `/speckit-tasks` MUST 生成能验证的任务，而不是仅描述意图。
- 代码评审 MUST 优先检查边界破坏、测试缺口、fork drift、文档漂移和跨平台风险。

**Version**: 1.0.0 | **Ratified**: 2026-06-15 | **Last Amended**: 2026-06-15

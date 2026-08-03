# Pinvou Agent 项目公约

## 核心约束

### 0. 本地私人记忆

- 如果仓库根目录存在 `.codex-memory.md`，开始工作前先读取；该文件是本地私人记忆，不提交。

### 1. 开发与 PR 公约

- 新任务首次开发、创建 PR 前及合并前安全同步最新 `origin/main`，并将 submodule 对齐父仓 gitlink；同一任务的持续开发和评审期间不因主线日常更新反复同步。
- 处理冲突时保全双方可共存的功能和用户改动；涉及行为取舍或无法证明等价时不得擅自选择，必须向用户说明选项和影响。
- 提交信息采用 `<type>(<scope>): <中文描述>`（`scope` 可省略）并带有效 `Signed-off-by`；具体格式和 DCO 要求遵循 `CONTRIBUTING.md` 与提交规范。
- PR 必须以目标分支的实际差异为依据，正文简洁说明背景、变更、实际验证和已知风险；Agent 创建的 PR 默认使用中文，外部贡献者可按 `CONTRIBUTING.md` 使用中文或英文。
- 最终提交和创建 PR 前必须自检需求完整性、根因及同类问题、实现边界、既有功能影响、异常状态和验证充分性；测试通过不能代替自检。
- 自检发现范围内缺口时先修复并验证；存在超范围改动、互斥方案、未验证项或回归风险时必须如实说明并等待决策。

### 2. CodeWhale 与 fork 边界

CodeWhale 提供模型调用、流式输出、工具循环、Session、Skills、Commands、MCP、Hooks 和 Compaction 等底座能力，Pinvou Agent 不重复实现。

扩展按以下边界落位：

| 改动类型 | 位置 |
|---|---|
| 领域 Agent 或工具组合 | `SKILL.md` |
| 外部 API 或独立能力 | MCP server / connector |
| 模型行为引导 | bundle `instructions.md` |
| UI、Tauri 集成或 Engine 配置 | `pinvou3-app/` |
| 可复用的底座问题 | CodeWhale，上游优先 |

- 只有必须进入底座生命周期且无法在 app、Skill 或 MCP 完成的 Pinvou 专用语义才留在 fork；通用修复应优先回馈上游。
- 新增或修改 fork-distinct 行为时，必须在同一 PR 更新 `docs/fork-modifications.md`、相关指纹和行为测试，并运行 `./scripts/fork-guard.sh --fast`。
- 仅更新 CodeWhale gitlink 且行为不变时，仍须按 guard 结果更新登记和指纹；能够证明现有测试已覆盖时，不强制新增行为测试。
- fork 基线、规模、主题和同步流程以 `docs/fork-policy.md` 与 `docs/fork-modifications.md` 为单一真相源。

### 3. 多平台架构边界

Pinvou Agent 按“业务功能优先、平台适配次之”组织：

| 改动类型 | 应放位置 |
|---|---|
| 前端业务 | `pinvou3-app/src/features/<name>/` |
| Tauri / Web 宿主适配 | `pinvou3-app/src/platform/{tauri,web}/` |
| Rust 业务及其平台差异 | `pinvou3-app/src-tauri/src/features/<name>/`，专属适配放功能内 `platform/` |
| 跨功能 OS 原语 | `pinvou3-app/src-tauri/src/platform/`，接口与各 OS 实现放 `platform/os/` |
| 共享资源与平台配置 | `pinvou3-app/src-tauri/resources/common/`、`pinvou3-app/src-tauri/resources/platforms/`、`pinvou3-app/src-tauri/config/platforms/` |

- 业务逻辑留在 `features/`；只有跨功能复用的低层能力才能进入全局 `platform/`。依赖保持 `app → features → platform/core`，不得反向依赖。
- React 不判断 user agent 或直接访问 Tauri 全局对象；通过 `get_platform_capabilities` 和 `can(capability)` 消费语义化能力。
- OS 差异使用 `cfg(target_os)` 和明确接口；不支持的能力显式返回 unsupported，不得静默借用其他平台实现。
- 构建统一走项目 npm 命令，不直接运行 `npx tauri build/bundle`。改动后运行 `python3 scripts/architecture-guard.py` 及影响范围内的测试。

### 4. 社区版开发公约

- 开工前先查现有 Issue、PR 和底座能力，避免重复建设；大型功能或破坏性变更先确认方案和验收标准。
- 社区版功能必须完整可用，不得依赖私有服务、内部地址或企业专属数据；企业能力通过通用扩展接口接入。
- 新功能必须形成用户可使用的完整闭环，并处理关键错误、权限不足和平台不支持等状态，不得以占位代码或假数据作为完成状态。
- 保持现有配置、数据和公开接口兼容；确需破坏性变更时，必须先确认并提供迁移方案和回归测试。
- 应用自身界面文案必须复用 `pinvou3-app/src/shared/i18n.js`，分别提供简体中文、英文和日文，不得在组件中新增单语文案或依赖其他语言回退。
- 涉及联网、上传、外部命令或新增依赖时，采用安全默认值并明确告知用户；行为变更必须同步更新测试和文档。
- 不得将账号、密码、密钥、Token、Cookie、客户或私人数据、内部地址写入代码、提交、PR、示例或日志；疑似泄露按 `SECURITY.md` 私密报告。

## 项目事实

- `pinvou3-app/`：Tauri 2 + React/Vite 桌面应用与 Engine wrapper。
- `CodeWhale/`：Pinvou/CodeWhale submodule，改动遵循“CodeWhale 与 fork 边界”。
- 运行时数据在 `~/.pinvou3/`（sessions / settings.json / bundle / knowledge / connectors）。
- bundle 扩展源码在 `pinvou3-app/src-tauri/resources/common/bundle/`，编译进应用并释放到 `~/.pinvou3/bundle/`。
- 开发启动使用 `./pinvou3-app/run-dev.sh`。
- 版本号以根目录 `VERSION` 为单一来源；修改后运行 `node scripts/sync-version.mjs`，CI 使用 `--check` 校验一致性。

## 规则入口

- `CONTRIBUTING.md`：贡献、DCO、检查与 PR 流程的单一真相源；`CONTRIBUTING.zh-CN.md` 为中文参考，内容冲突时以前者为准。
- `SECURITY.md`：安全漏洞与敏感信息的私密报告流程。
- `docs/fork-policy.md` / `docs/fork-modifications.md`：CodeWhale fork 策略与现状。
- `pinvou3-app/src/ARCHITECTURE.md`、`pinvou3-app/src-tauri/src/README.md`、`pinvou3-app/src-tauri/config/README.md`：前端、Rust 与平台配置边界。
- `docs/architecture-guard.md`：架构守卫规则。
- `docs/Git Commit 信息规范文档.md`：提交信息规范。

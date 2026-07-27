# pinvou3 项目规则

## 核心约束

### 0. 本地私人记忆

- 如果仓库根目录存在 `.codex-memory.md`，开始工作前先读取；该文件是本地私人记忆，不提交。

### 1. 开发与 PR

- 开始新的开发任务前，先安全同步最新 `origin/main` 和 submodule，不得丢失已有本地改动。
- 创建 PR 前及合并前确保分支基于最新 `origin/main` 并解决全部冲突。
- 评审和修复期间不要仅因 `main` 更新而反复 rebase；准备合并时再同步主线并通过完整 CI，默认使用 Squash Merge。

### 2. CodeWhale 是底座，不重复造轮子

CodeWhale 已有：Engine / ToolRegistry / 流式 SSE / Session / SkillRegistry / Commands 路由 / MCP client / Hooks / Cycle / Compaction。

**绝不在 pinvou3 重新实现这些**。扩展时按场景选：

| 想做的事 | 用哪个 |
|---|---|
| 加领域 agent / 工具组合 | SKILL.md（复用 SkillRegistry） |
| 接外部 API | 写独立 MCP server |
| 改 LLM 行为引导 | `instructions.md`（见下「主体」bundle 布局） |
| Tauri UI / Rust wrapper / Engine 配置 | pinvou3-app 内 Rust |
| 修上游 bug | CodeWhale fork(详见 `docs/fork-policy.md`,软上限 1500 行) + 视情况 PR |

> **fork 维护**：策略/sync 流程见 [`docs/fork-policy.md`](docs/fork-policy.md)；fork 现状按 **6 个长期主题**维护，清单与 sync 后验证 checklist 见 [`docs/fork-modifications.md`](docs/fork-modifications.md)。**基线/drift 以 fork-policy §0 为单一真相源**(软上限 1500 行,超过必须记录强制评估)；守护手段 = fork-guard.sh 指纹 + forkguard_ 测试 + dump_system_prompt 前后 diff。
> **fork patch 指纹随 patch 同 PR**：新增/改 fork patch 的 PR 必须**同 PR**带上 fork-guard.sh 指纹 + 更新 fork-modifications.md——指纹随 patch 走,**不拆事后 catch-up PR**(出现 catch-up PR = 原始 PR 漏了指纹)。提 PR 前跑 `./scripts/fork-guard.sh --fast` 自查。**main CI `required-gate` 已 enforce 此约束**：缺指纹 / 改 gitlink 没登记 fork-modifications 都会挡合(`.github/workflows/pr-check.yml`)。
> **fork 改动是否要 PR**：通用优化 / bug 修复才提；pinvou3 专用留 fork。详见 `docs/fork-policy.md` §2 决策树。
> **底座上游PR规范**：https://github.com/Hmbown/CodeWhale/blob/main/CONTRIBUTING.md

### 3. 多平台架构边界

pinvou3 按“业务功能优先、平台适配次之”组织。新增代码按下表确定落点：

| 改动类型 | 应放位置 |
|---|---|
| 前端业务 | `pinvou3-app/src/features/<name>/` |
| Tauri / Web 宿主适配 | `pinvou3-app/src/platform/{tauri,web}/` |
| Rust 业务及其平台差异 | `pinvou3-app/src-tauri/src/features/<name>/`，专属适配放功能内 `platform/` |
| 跨功能 OS 原语 | `pinvou3-app/src-tauri/src/platform/`，接口与各 OS 实现放 `platform/os/` |
| 共享 / 平台资源 | `pinvou3-app/src-tauri/resources/common/` / `pinvou3-app/src-tauri/resources/platforms/<os>[/<arch>]/` |
| Tauri 平台配置 | `pinvou3-app/src-tauri/config/platforms/<os>/` |

- 业务逻辑留在 `features/`；只有跨功能复用的低层能力才能进入全局 `platform/`。依赖保持 `app → features → platform/core`，不得反向依赖。
- React 不判断 user agent 或直接访问 Tauri 全局对象；通过 `get_platform_capabilities` 和 `can(capability)` 消费语义化能力。
- OS 差异使用 `cfg(target_os)` 和明确接口；不支持的能力显式返回 unsupported，不得静默借用其他平台实现。
- 构建统一走项目 npm 命令，不直接运行 `npx tauri build/bundle`。改动后运行 `python3 scripts/architecture-guard.py` 及影响范围内的测试。

详细规则见 `pinvou3-app/src/ARCHITECTURE.md`、`pinvou3-app/src-tauri/src/README.md`、`pinvou3-app/src-tauri/config/README.md` 和 `docs/architecture-guard.md`。

### 4. 提交日志

- git log 提交日志统一使用中文，并在标题中注明类型前缀，例如 `fix:` / `feat:` / `docs:`；类型前缀后的描述使用中文。

### 5. GitHub PR 规范

- GitHub PR 的标题和正文统一使用中文（代码标识、命令、路径等保留原文）。
- PR 正文必须明确说明以下内容：
  - **改了什么**：概括本次修改的主要内容。
  - **改动原因**：说明问题背景、修改目的或采用该方案的原因。
  - **影响面**：列出受影响的功能、模块、平台、兼容性及潜在风险；没有影响也要明确说明。
- 提交 PR 前遵循 `CONTRIBUTING.md` 中的流程和 CI 门控要求。

### 6. 合并冲突保全

- merge、rebase、cherry-pick、跨仓迁移或手工移植时，必须把当前仓库已有功能和用户改动视为受保护基线。
- 能够共存的双方功能必须合并保留，不得为了消除文本冲突而整文件选择一侧或用来源分支覆盖本地行为。
- 只有不改变行为的机械性冲突、明确重复或可证明等价的实现可以独立处理。
- 遇到互斥方案、产品行为、安全/兼容性取舍或无法证明等价的实现时，必须保持未决，向用户说明选项和影响并等待明确决策，不得猜测。

### 7. 基于基线生成更新内容

- 当用户要求根据某个基线生成代码变更内容时，必须以用户指定分支或 commit 相对于该基线的实际代码差异为事实依据；可以参考相关 commit 的标题和说明辅助理解，但不得直接复制 commit 内容或仅凭提交信息生成。
- 变更内容必须按“功能新增”“体验优化”“问题修复”三类整理。同一功能及其从属变更只能归入一类；如果某项体验优化或问题修复从属于本次新增功能，则统一计入“功能新增”，不得在其他类别重复列出。
- 每项原则上使用一句话概括，重点说明“做了什么”及用户可感知的结果，不展开具体实现方式或底层技术细节。
- 必须输出两份内容：第一份为带贡献者的完整更新内容，按上述三类组织，并在每项末尾以 `@姓名` 标注该项实际贡献者；第二份为不带贡献者的精简更新内容，分类保持一致，但每项表述应更简洁。
- 两份变更内容都必须在每个分类内对条目独立编号；每个分类均从 `1` 开始连续编号，分类之间不延续编号。
- 贡献者必须根据实际代码变更和版本历史确认，并把账号映射为真实姓名：`hexin` → `贺欣`、`zhuowp` → `卓文培`、`cacdcaecawae` → `刘洋`、`xuyajing620-max` → `徐雅婧`、`Biilow-Bailang` → `白浪`、`asto18089` → `梁宇逸`。
- 两份内容都必须面向全量用户进行脱敏和发布适配，避免披露账号、密码、密钥、内部地址、私人数据、安全敏感信息及仅供研发理解的内部细节；不适宜展示给用户的内容应省略，第二份执行更严格的精简与脱敏标准。

### 8. Windows OTA 包构造

- 用户提供 Windows EXE 安装程序并要求构造 OTA 包时，直接运行 `powershell -ExecutionPolicy Bypass -File .\scripts\build-windows-ota.ps1 -SourceExe "<安装程序路径>"`，不再要求用户提供历史 OTA 模板。
- 脚本从安装程序文件名或版本信息识别版本，三段版本自动补为四段版本（例如 `0.5.12` → `0.5.12.0`），默认在源文件目录生成 `Pinvou3_<四段版本>.zip`。
- OTA 固定为双层 ZIP：外层只包含 `FullPack.zip` 和 `UpdatePackInfo.json`；内层只包含 `Files\Pinvou3\<安装程序文件名>` 和 `OtaInfo.json`。
- `OtaInfo.json` 中安装程序 `hash` 使用 EXE 的 MD5，`UpdatePackInfo.json` 中 `fullPack.hash` 使用 `FullPack.zip` 的 MD5；两份 JSON 必须使用 UTF-8 **无 BOM** 编码，禁止使用 UTF-8 BOM。
- 生成后必须验证内外层条目、四段版本、两级 MD5、JSON 可解析性和无 BOM 编码，并向用户报告输出路径、文件大小及最终 OTA 包 MD5。
- 只有在 OTA 协议发生变化或用户明确要求重新取样时才需要读取模板；日常构包以脚本中固化的协议为准。

### 9. Pull Request 创建规范

- 以后创建任何 PR，都必须先以目标分支为基线检查实际代码差异，并据此撰写标题和正文；提交记录只能用于辅助理解，不得代替代码差异分析。
- PR 标题采用 `<type>(<scope>): <中文简短描述>`；`scope` 不适用时可省略为 `<type>: <中文简短描述>`。`type` 按变更性质选用 `feat`、`fix`、`refactor`、`perf`、`docs`、`style`、`test`、`build`、`ci`、`chore` 或 `revert`，标题应准确概括主要变更且保持单一职责。
- PR 正文按以下顺序组织：
  - `## 概述`：用 1～2 句话概括此 PR 做了什么以及带来的结果。
  - `## 背景`：说明为什么需要本次变更，包括原有问题、业务需求或上下文。
  - `## 变更`：以简洁条目列出实际完成的主要变更，重点描述“做了什么”，避免展开不必要的实现细节。
  - `## 验证`：列出实际执行且有结果的测试、构建或人工验证；没有验证内容时整节省略，不输出空标题、占位文字或未执行的勾选项。
  - `## 备注`：仅在存在风险、兼容性影响、已知限制、待决事项或需要 reviewer 特别关注的内容时输出；没有内容时整节省略。
- 除“验证”“备注”外，其他补充章节也遵循“有实际内容才输出”的原则；默认必须保留“概述”“背景”“变更”三节。
- 正文应简洁、可审阅并与代码差异一致，不包含账号、密码、密钥、内部敏感地址、私人数据等不应进入 PR 的信息。
- 创建 PR 前按实际影响执行必要检查；涉及 CodeWhale fork 时必须遵守本文件第 2 节的同 PR 指纹和文档要求，并运行 `./scripts/fork-guard.sh --fast`。

### 10. 禅道问题解决规则

- 涉及禅道 BUG 的获取、复现、修复版本确认、解决备注、状态流转或结果回查时，必须先读取 `.specify/memory/constitution-zentao.md`；该文件是解决禅道问题的专用规则文档和单一真相源。
- 必须先完成代码验证、规范提交以及规则文档要求的 GitHub/GitLab 推送，再将禅道 BUG 设置为已解决；更新状态后必须按规则文档回查状态、版本、解决动作和备注。
- 当前修复版本、`buildID`、接口步骤或已验证流程发生变化时，必须同步更新 `.specify/memory/constitution-zentao.md`，不得只保留在临时对话或个人记忆中。
- 账号、密码、Token、Cookie 等认证信息不得写入仓库、提交信息、禅道备注或可见日志；专用规则与本文件、项目宪章、安全约束或仓库事实冲突时，以上位规则和事实为准，并修正规则文档。

## 主体

- `pinvou3-app/` — 🟢 Tauri 2.0 + EngineHandle wrapper（主线）
- `CodeWhale/` — submodule（Pinvou/CodeWhale fork），改动遵循约束 2
- 运行时数据在 `~/.pinvou3/`（sessions / settings.json / bundle / knowledge）
- 扩展物（instructions.md / skills / mcp-servers / personas）源码在 `pinvou3-app/src-tauri/resources/common/bundle/`，**编译进 app**，启动释放到 `~/.pinvou3/bundle/`

启动：`./pinvou3-app/run-dev.sh`

版本号单一事实来源是根目录 `VERSION` 文件；bump 版本 = 改 `VERSION` 后运行 `node scripts/sync-version.mjs`（同步 tauri.conf.json / Cargo.toml / package.json 三处，`--check` 供 CI 校验）。

## 参考文档

- `CONTRIBUTING.md` — 贡献 / PR 流程 + CI 门控(人类协作者入口)
- `docs/验证报告-qwen3.6-deepseek-tui.md` — 阶段 A 实证报告
- git log + commit message — 决策记录与已知坑修复
- `docs/Git Commit 信息规范文档.md` — **强制** Git commit 信息规范。任何方式发起的 commit 都必须符合该规范；本仓库通过 `.githooks/commit-msg` 与 CI 校验执行。
- `.specify/memory/constitution-zentao.md` — 解决禅道问题的专用规则文档；处理禅道 BUG 前必须读取并遵守。

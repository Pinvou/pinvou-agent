# 能力档案统一（Capability Profile）— 改动说明

> 关联：`.luzeyang/capability-unified/`（00-README 索引 / 01 fork ② 申请 / 02 实施交接 / 03 留档，下称「方案」）、`.luzeyang/capability-profile/`（tool/skill/fork 三线分线说明）、`docs/skill-scope-governance-改动说明.md`（skill 线，本方案的前置已落地部分）。
> 分支：`feat/skill-scope-governance`（与 skill 线同分支延续）。
> 本文件登记方案（02 交接 §3 六步）的实施结果，作为 PR 的验收依据。

## 一、为什么需要这些改动

skill 线落地后，「能力边界按会话表达」在 skill 维度已闭环（双 scope 开关 + 组合目录 + fork ①）。但 tool 维度仍是三处散落的硬编码：底座编译期常量 `PINVOU3_HIDDEN_TOOLS`（约 80 个，进程级、先于 app 配置生效、不可按会话翻案）、`SessionPolicy::extra_hidden_tools()`（if-else）、连接器 scope 禁用集。同一诉求（"哪个模式能用什么工具"）无法用数据表达——新增模式 = 改判断代码（T3/T4）。

统一解法（方案 §3）：**一份档案（per-mode JSON，v1 编译内嵌）、一个解析器（SessionPolicy::resolve）、三个生效通道**（skills_dir 组合目录 / disallowed_tools / hidden_tools），配合**唯一 fork 改动**（fork ②：隐藏集按会话注入，缺省回退常量）。

## 二、改动总览

| 项 | 内容 | 性质 |
|---|---|---|
| C-1 | `CapabilityProfile` 数据模型（`features/assistant/capability_profile.rs` + `resources/common/capability-profiles.json` 编译内嵌） | 新模块 |
| C-2 | `SessionPolicy::resolve()` 统一解析器（三通道产出；if-else 只保留在解析器内部） | 重构（行为等价） |
| C-3 | `tools.exclude` → disallowed_tools 通道（`shape_disallowed_tools` 并入，spawn 初值 + 热刷下轮生效） | 接线 |
| C-4 | fork ②：`EngineConfig.hidden_tools: Option<Vec<String>>` + 隐藏判定注入集优先、缺省回退常量（`tool_catalog.rs`） | fork 改动 |
| C-5 | `tools.include` → hidden_tools 通道（hidden = 常量 − include，engine config 按会话注入，respawn 生效） | 接线 |
| C-6 | 首个 include 放出：code 档案 `["git_status"]`（只读 git 工具，事件走前端通用 tool_call 渲染） | 功能 |
| C-7 | 文档（本文件 + `fork-modifications.md` 中英 + fork-guard T3 指纹） | 文档 |

## 三、逐项改动说明

### C-1：档案数据模型（v1 编译内嵌）

- `resources/common/capability-profiles.json`：每模式一份 `CapabilityProfile { mode, skills:{exclude, include_project}, tools:{base, exclude, include}, connectors:{scope_defaults} }`。
- **编译内嵌、不写用户数据**（方案决策：首版规避版本迁移负担；"运行期不变"是 v1 语义，未来可编辑化需重议 respawn）。
- 语义 = **基础集 + 差量**（方案 §3.5）：上游新增工具仍被基础集（底座隐藏常量）挡住，模式只表达差异。v1 档案：plain 全空差量（零影响），code `tools.include = ["git_status"]`（步骤 C-6，已评估事件渲染的只读工具）。
- 档案解析失败（打包错误）→ panic（编译内嵌受控资源，崩溃优于静默降级）。

### C-2：统一解析器

- `SessionPolicy::resolve() -> ResolvedCapabilities { connector_scope, extra_hidden_tools, tool_exclude, tool_include }`——现有 `connector_scope()` / `extra_hidden_tools()` 改为 resolve 的便捷访问（行为不变）；档案 tools 差量从 `profile()` 读。
- **if-else 只保留在解析器内部**：外部消费者（`shape_disallowed_tools`、engine config 构造）统一走 resolve / profile。新增模式 = 加档案条目 + 解析器的 mode 映射，不改消费者判断（U-2）。
- 连接器 scope 整形（方案步骤 6 的收尾收敛）：scope 选择（mode → ConnectorScope）本就是模式固有属性，已由 resolve 统一产出；档案 `connectors.scope_defaults` 作为设计期默认挂载点（v1 空，用户开关覆盖）。skill 组合目录输入（project_workspace / 用户开关）保持既有通道，档案 `skills.include_project` 字段预留（v1 默认 false，行为不变）。

### C-3：tools.exclude → disallowed_tools 通道

- `bridge::shape_disallowed_tools`：档案 `tools.exclude` 并入（两模式各自生效；v1 plain 为空 → 逐字节等价）。spawn 初值（engine_pool）与 `set_disallowed_all` 热刷都经此整形 → 档案变更下轮请求生效（U-3/T-V5）；底座 `command_denies_tool` 执行兜底拦截幻觉调用（T-V6）。

### C-4：fork ②（唯一 fork 改动，依据 01 申请文档实施）

- `EngineConfig.hidden_tools: Option<Vec<String>>`（default None）。
- `tool_catalog.rs`：`pinvou3_should_defer_native_tool` 的隐藏判定改为 **注入集优先、缺省回退常量**（`is_pinvou3_hidden_with_injection`）；`apply_native_tool_deferral` / `build_model_tool_catalog_with_surface` 透传；engine.rs catalog 构建处从 config 取。
- **不变式**（forkguard 测试守护）：
  - `tool_search` 的 gate **恒查编译期常量、不可注入**（`forkguard_tool_search_always_gated`：注入集刻意"放出"tool_search 的最坏情况下 catalog 仍不含它——模型无法用搜索复活被藏工具，exclude 语义不可击穿）；
  - `request_user_input` 硬豁免不受注入影响（`forkguard_request_user_input_exempt_from_injected_hidden`）；
  - 注入集生效由 `forkguard_hidden_tools_injectable` 守护（注入含 X → X 隐藏；常量含 Y 注入不含 → Y 放出）；
  - `PINVOU3_BLOCKLIST_OVERRIDE` env 豁免只作用于常量回退路径（测试通道语义不变）。
- 缺省 `None` 回退常量 → CLI/TUI/测试/plain 全部逐字节不变（既有 golden 守护）。
- fork-guard：T3 指纹新增 `hidden_tools 按会话注入`（`is_pinvou3_hidden_with_injection`）；`docs/fork-modifications.md`（中英）T3 登记。

### C-5：tools.include → hidden_tools 通道

- `build_engine_config_for_session_at`：`cfg.hidden_tools = 常量 − 档案 tools.include`（仅 include 非空时 Some；空 → None → 底座回退常量，与现状逐字节等价，plain 零影响）。
- **生效语义（U-7）**：hidden 集在 **spawn 时定型**、运行期不变（v1 档案是设计期产物）——档案变更仅 respawn 生效，**无热刷通道**（`set_disallowed_all` 只覆盖 disallowed，不覆盖 hidden）；两通道 spawn 叠加顺序（hidden 贴 defer 标签 → disallowed 硬删）互不重叠，注释钉清。

### C-6：首个 include 放出（git 只读工具）

- code 档案放出 **`git_status`**（04 PR-E：首个 include 建议 git_status，**每个工具单独 PR/逐个放出**——`git_diff` 后续单独评估放出，不在本批）。PINVOU3_HIDDEN_TOOLS 原含 git_status，2026-07-03 因纯办公定位全隐藏；code 模式为编码场景，按方案步骤 5 的"逐个评估、小步放出"恢复该只读查询工具。
- **事件渲染已确认（U-9）**：git_status 是标准 tool_call 事件（前端 `acp-state.js` 通用处理 tool_call/tool_call_update），无 agent_spawn 类特殊事件流，不产生裸 JSON。`apply_patch` 属"定位性隐藏"按方案默认不放。
- **前端菜单按档案渲染（04 PR-E 配套）**：新增 `get_profile_tools(scope)` 命令（返回该 scope 档案 tools.include），ComposerToolMenu 在 code scope 展示「档案放出工具」只读分组（i18n 三语）；档案是设计期产物，无运行期开关。

## 三·五、行为影响矩阵（05 路线图 §1.4 强制）

v1 档案状态：plain 零差量；code `tools.include = ["git_status"]`、`exclude` 空。

| 消费方 | 改动前 | 改动后 | 差异与恢复路径 |
|---|---|---|---|
| GUI plain 会话 | catalog = 全量 − 底座常量隐藏（无 git_status） | 同前（档案零差量 → resolve 空差量；hidden_tools 不注入 → None 回退常量） | **无**；恢复 = 无操作 |
| GUI code 会话 | 同 plain（git_status 在常量隐藏集） | git_status 放出可见可执行（hidden = 常量 − [git_status] 注入） | **刻意行为变化**（04 PR-E 首个 include）；恢复 = 档案 include 清空（app 侧，零底座回退） |
| CLI（codewhale-tui） | 常量隐藏 | 同前（`main.rs` hidden_tools 恒 None） | **无**；恢复 = 无操作 |
| TUI | 同 CLI | 同前（engine config 缺省 None） | **无** |
| 测试（fork 仓） | 常量路径 golden（forkguard_blocklist_golden 等） | 常量路径不变；新增 3 个 forkguard 注入测试（None 分支仍走常量） | **无**（纯新增）；恢复 = 删测试 |
| skill 线（P1-P4 组合目录） | — | 未触碰 skill_materialization / skill_scope | **无** |
| exclude 通道 | 无（历史上无 per-mode exclude 表达） | v1 档案 exclude 空 → 全模式无行为变化；通道就绪（spawn + 热刷） | **无**（功能新增但未激活） |
| 前端工具菜单 | 无底座工具项 | code scope 新增「档案放出工具」只读分组（get_profile_tools 命令，i18n 三语） | **UI 新增**（只读展示）；恢复 = 移除分组 |

> 回归面：fork ② 的缺省回退由 forkguard 注入测试 + 既有 golden 双守护；`tool_search` gate 恒查常量（forkguard_tool_search_always_gated）——任意注入值下 catalog 不含 tool_search，隐藏语义不可被模型复活击穿。

## 四、验收矩阵对照（方案 §4）

| # | 验收项 | 状态 |
|---|---|---|
| U-1 | plain 零影响（技能/工具/catalogue 与改动前一致） | 档案 plain 零差量 + hidden None 回退常量 + golden 既有守护；手动 dump 待做 |
| U-2 | 档案即数据（新增模式只加档案条目） | 解析器唯一 if-else 点（mode 映射）；代码走查 ✓ |
| U-3 | exclude 按模式生效（A 藏 X、B 可见 X；幻觉调用被拦截） | exclude 并入 disallowed 通道（spawn + 热刷）；单测编译通过；手动两会话待做 |
| U-4 | include 按模式生效（code 可见可执行 git_status，plain catalog 不含） | fork ② + 档案 include 注入；forkguard 单测编译通过；手动待做 |
| U-5 | tool_search 不复活 | `forkguard_tool_search_always_gated`（注入最坏情况） |
| U-6 | skill 线不回退（P1-P4 全绿） | 本方案未触 skill 线代码（skill_materialization 无改动）；既有测试在 |
| U-7 | 时效语义（disallowed 热刷下轮生效 / hidden 仅 respawn） | 注释钉清（C-5）+ 代码走查 ✓ |
| U-8 | prefix-cache 稳态（无档案变更时逐轮字节稳定） | 档案编译内嵌运行期不变；物化时机沿用 skill 线三时机（无每轮 diff） |
| U-9 | 前端渲染（include 工具事件有 UI 渲染，无裸 JSON） | git_status 走通用 tool_call 渲染（C-6 确认） |
| U-10 | 自动检查 | 见 §五 |

## 五、验证结果

- fork 侧：`cargo check -p codewhale-tui --all-targets` ✅；`forkguard_hidden_tools_injectable` / `forkguard_tool_search_always_gated` / `forkguard_request_user_input_exempt_from_injected_hidden` 编译通过（本机测试 exe `0xc0000139` 既有环境问题，实际运行以 CI 为准）。
- app 侧：`cargo check --tests` ✅；`SessionPolicy::resolve` 单测编译通过。
- 前端：`lint:ui` / `test:ui-language` / `test:composer-tools` 基线不变；代码页工具菜单按档案渲染 + code scope 开关恢复可写 + 项目级 skills 开关与警告（31345457，i18n 三语）。
- `cargo fmt --check` / `clippy --tests` 零新增 warning；`./scripts/fork-guard.sh --fast`；`architecture-guard.py`（新模块均在 features/assistant 内，无新环）。
- 待做（需启动应用，人工走查）：U-1 dump_system_prompt 前后 diff、U-3/U-4 两会话 catalog 对照、code 会话 git_status 可见可执行。

## 六、行为兼容与遗留

- **行为不变面**：plain 会话全通道零变化（档案零差量 + None 回退）；CLI/TUI 走缺省（main.rs 恒 None）；`tool_search` gate 与 `request_user_input` 豁免不变式由 forkguard 锁定。
- **行为变化面（刻意）**：code 会话放出 `git_status`（C-6）；`SessionPolicy` 内部重构（resolve 统一产出，外部行为等价）。
- **遗留**：fork ② 依据 `.luzeyang/capability-unified/01-fork改动申请-领导评审.md` 实施，与 fork ① 合并为单一提交 `52d3ce4a7`，已推送个人 fork 并建跨仓 fork PR（Pinvou/CodeWhale#7，待评审合入）；若评审未过可整体回退——app 停止注入（档案 include 清空）即回退现状，revert 为纯代码回退、无数据迁移。档案 v1 编译内嵌不可运行期编辑（未来可编辑化需重议 respawn，方案 §5.3 已记录）。`git_diff` 与后续 include 候选按 04 PR-E 逐个单独放出。

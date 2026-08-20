# 能力治理（Capability Governance）

本文档描述 pinvou3 当前的能力治理架构：哪些能力存在、谁决定它们在某个会话
中可用、运行时如何生效。取代已删除的 `tool-governance.md`（v0.9.0 blocklist
时代）与 `skill-scope-governance-改动说明.md`（PR 验收记录，内容已沉淀于此）。

> **落地状态**（2026-08-14）：§1、§2 为现状（能力档案已退役，模式能力差量
> 已收敛为静态表 `MODE_TABLE`）；§3 的存储已泛化为按模式键控的 map
> （`{scopes, initialized}`，见 §3.2），仍是 `disabled_connectors.json` +
> `disabled_skills.json` 两份（合并为 `disabled_bundles.json` 未做）；§3.1
> 的统一包模型与「一个包 = 一个开关」、§3.3 的运行时工具名发现（现为
> manifest 预测）、内置 CLI 连接器归并、统一失效入口（现为各开关命令分别
> 触发刷新）与 §6 的泛化命令面（现为 `set_disabled_connectors` /
> `set_disabled_skills` 等）为**已定方向、未实施**，实施时以本文档为准并
> 更新本注记。§7 冻结模型能力与 HostWork 控制的隔离边界；当前工作树已实现
> schema v6 HostWork、首版 Linux Supervisor、显式 profile helper、固定 E2E
> harness 与 deb 固定 payload mode/hash 门禁，但 MegaBook 真实 deb 安装、
> High、OOM 与 purge 尚未执行，唯一权威见 ADR-0009。

---

## 1. 总览：两条线

```
能力面 = 原生家族线（编译期决策）+ 能力包线（运行期用户开关）
```

| 线 | 管什么 | 决策时机 | 用户开关 |
|---|---|---|---|
| 原生家族线 | 底座 canonical 家族（`Bash`/`File`/`Git`/`Web`/`agent`/`workflow` 等） | 编译期 | **无** |
| 能力包线 | 一切外部能力：MCP 连接器、组合工具、CLI 连接器、独立技能 | 运行期 | 有（按模式 scope） |

设计纪律：**底座能力是产品承诺，不是用户偏好**——不开放用户级开关，避免
"关掉 `File` 后应用坏了"这类 footgun。运行期配置只给真正有运行期写入者
（用户开关）的能力包线；没有写入者的运行期配置只是常量的间接层。

## 2. 原生家族线（编译期）

```
某模式可见集 = PINVOU3_ALLOWED_TOOLS（白名单）− MODE_TABLE[模式].unavailable_tools
```

- **白名单**（`features/assistant/tool_policy.rs` 的 `PINVOU3_ALLOWED_TOOLS`）：
  产品级安全决策，spawn 时经底座 `allowed_tools` 同时约束首轮目录、
  `tool_search` 结果与实际执行（deny 优先于 allow）。改动需 respawn，
  属安全评审事项。
- **模式能力静态表 `MODE_TABLE`**（`features/assistant/session_policy.rs`）：
  每模式一行 `ModeCapabilities { unavailable_tools,
  skills_empty_hides_load_skill, project_skills_opt_in }`，编译期常量、
  查表取数（取代散落的 match 臂），新增模式漏填由穷尽性测试兜底
  （测试遍历的 `SessionMode::ALL` 由编译期穷尽哨兵绑定到枚举变体，
  漏挂即编译失败）。
  语义全部是"该模式架构上有/无此能力"（如 code 的
  `mcp_pinvou3_present_artifact`：产物卡在代码车道没有 UI 消费者），
  不是"默认关掉"——不出现在任何开关面。
- 能力档案（`capability-profiles.json` + `capability_profile.rs` 统一解析器）
  已退役：v0.9.5 起基础集由白名单承担，档案只剩 per-mode 差量，而差量
  没有运行期写入者，JSON + 解析器是多余的间接层。plain 曾默认禁 `Git`
  家族，经决策放开。

## 3. 能力包线（运行期）

### 3.1 数据模型：能力包（已定方向、未实施）

> 现状：连接器与技能仍是两份独立开关文件（§3.2），companion 技能按
> scope 联动排除；下述统一「包」模型（含 `bundle_kind` 推导与
> 「一个包 = 一个开关」）为目标设计，实施时以本节为准。

一切外部能力统一建模为**包**，三个部分均可空：

```
Bundle = { id, name, mcp_servers: [], skills: [], cli: [] }
```

- 纯 MCP 包（`servers` 非空）：本地 stdio 型 / 远程 OAuth 型；
- 组合包（`servers` + `skills` 均非空）：MCP 函数 + 使用引导一体；
- CLI 包（`cli` 非空）：飞书/企微/钉钉/tmeet/ima 等内置连接器；
- 纯技能包（仅 `skills`）：市场预置、用户上传、手放技能。

包的**类型不做存储标签**，由内容现算（`bundle_kind` 推导），只用于 UI
徽标与规则查表——存储标签会和事实漂移，且不可信输入（项目技能、上传包）
自报的标签是提权通道，分类事实只由安装/加载层（可信代码）推导。

**一个包 = 一个开关**：包的暴露面（MCP 工具 + 包内技能引导 + CLI 引导）
整体上下线。包内技能（原 companion skill）没有独立开关，可见性唯一跟随
所属包——不存在"引擎在、引导不在"的半截状态。

### 3.2 默认姿态与用户数据

存储：`~/.pinvou3/disabled_connectors.json` 与 `~/.pinvou3/disabled_skills.json`
两份，结构同构为按模式键控的 map：

```json
{ "scopes": { "<mode>": ["<id>"] }, "initialized": ["<mode>"] }
```

scope 键即 `SessionMode` 的 kebab-case 名（当前 `plain` / `code`）；
`initialized` 集合取代原 `code_initialized` 布尔（旧格式 `{plain, code,
code_initialized}` 与裸数组读时自动迁移；`disabled_bundles.json` 合并未做）。

每个模式的默认策略显式声明为**模式身份**（`core/session_mode.rs` 的
`SessionMode::pack_default_policy()`），不再是存储层的硬编码分支：

| 模式 | 包默认策略 | 含义 |
|---|---|---|
| plain | AllowAll | 全开 |
| code | **DenyAll** | 全禁（外部能力显式开启，封泄露面/攻击面） |

用户数据语义（三条线一致）：

- 某 scope 无记录 → 回落编译期默认（跟随产品演进）；
- 用户首次 toggle 时物化整个 scope 列表落盘 → 此后冻结，默认调整不穿透
  已做过选择的用户（`initialized` 集合标记初始化）；
- 未知条目（工具下架、上游改名残留）静默忽略，写回时清理。

项目级技能（`.agents/skills` 等）保持**独立开关**、各 scope 默认关：
项目内文本是 prompt-injection 面，信任级别与包装机技能不同，开启路径
展示注入风险警告。

### 3.3 生效通道

```
开关 → capability_changed(scope, line) → 下一轮生效（不 respawn）：
         持久化 → 重算投影 → 组合目录重写 → disallowed 热刷 → 事件广播

   包的 MCP 部分:  工具名进 disallowed_tools → catalog retain 过滤
   包的技能部分:   组合目录物化（~/.pinvou3/sessions/<sid>/skills/）
                  → 底座每轮重扫渲染 ## Skills 块
                  → 组合目录为空 → load_skill 一并隐藏（无"假开关"状态）
```

- **工具名获取以 manifest 预测为主**（现状）：`marketplace/mod.rs` 的
  `model_tool_names` 按 `mcp_{server}_*` 前缀 + manifest 声明工具名生成
  禁用名单；查引擎实际暴露的 `model_name`（运行时发现，底座权威）为
  **已定方向、未实施**——落地后清单错配（如 id 含连字符）才不会导致
  "禁不掉"；
- **唯一失效入口** `capability_changed`：任何开关不可能漏刷下游；
- spawn 初值与热刷同经 `bridge.shape_disallowed_tools` 按会话整形。

## 4. 模式扩展（新增设计/聊天等模式时）

模式是**编译期封闭集合**（模式身份影响 spawn 时的底座配置，不允许运行期
自定义）。新增一个模式要回答的问题已全部显式化，按清单逐项回答即可：

1. `core/session_mode.rs`：加 `SessionMode` 变体并挂入 `declare_all_modes!`
   列表（编译器守所有穷尽 match；漏挂 ALL 直接编译失败），补
   `as_str`/`from_scope_str`；
2. **安全姿态决策**：`pack_default_policy()` 为该模式选 AllowAll/DenyAll
   ——依据是该模式会话内容是否会出现不受控外部文本（prompt-injection 面），
   这是代码评审级别的决策；
3. `session_policy.rs`：`MODE_TABLE` 加一行，逐格回答——模式缺席工具
   （`unavailable_tools`）、空目录是否隐藏 `load_skill`、是否参与项目级
   skills。漏填由穷尽性测试兜底失败；
4. 存储层**零改动**：scope 键即模式名，用户首次 toggle 时 map 自然多键；
5. 前端：scope 字符串协议不变，新模式名即传新的 kebab-case 名。

已知演进分支（今天不写代码）：若产品要求"不发版热更新模式能力"，模式是
编译期封闭集的前提才被打破，届时模式身份层整体迁运行期配置（带播种与
合并逻辑），属独立立项。另一个待决产品问题：模式增多后 scope 应按模式
键控还是按信任类共享（避免用户为每个模式各拨一遍开关）——留给第一个
真实的新模式落地时回答。

## 5. 底座硬防线（与上层状态无关）

```
白名单 allowed_tools（编译期，spawn 给定）
→ disallowed retain（deny 优先）
→ 执行门逐次校验（catalog 缺席 ⇒ 调用拒绝）
→ 审批分类（McpRead / McpAction 风险分级）
```

UI 或状态层出 bug 也放不出白名单外能力。已知开放侧翼：CLI 包的真实执行
面是经 `Bash` 调用 CLI，开关只能隐藏引导；要封死需 Bash hook 拦截，
当前作为已接受风险记录于此。

## 6. 前端接线

- 命令面：`list_capability_items(scope)` 读全量状态（默认已合并），
  `set_capability_enabled(scope, id, enabled)` 唯一写入口；前端不在 JS 侧
  计算默认、不判断模式差异、不直接读写 JSON；
- 事件面：`capabilities_changed` 广播，各窗口/远程实例监听后重取状态；
- UI：设置页「能力管理」区，plain/code scope 切换 + 能力包分组
  （类型徽标）+ 项目级技能独立开关；界面文案三语走 `shared/i18n.js`。

## 7. 模型能力不等于 Host 控制权

能力治理回答“某个会话的模型目录里可见哪些工具”；HostWork 治理回答“宿主能否对一个
由可信代码登记的后台工作执行 pause / stop / resume”。两者必须保持不同的授权面：

- 工具出现在 catalog、技能描述提到 stop、用户打开某个能力包，都不会注册 HostWork，
  也不会授予 PID、systemd unit、cgroup 或进程控制权；
- HostWork 只能由 Rust 组合根和受信 feature 代码以 opaque `work_id + generation`
  注册；支持的动作来自编译期封闭 Adapter，不来自模型参数或 Renderer 状态；
- Resource Agent 与 `pinvou-resource` Skill 只读 Runtime Resource 投影。Governor 可以确定性
  签发 `Pending` Directive，但只有受信 Adapter / Supervisor 的 ACK 加后验观测
  才能证明动作生效；
- 受信 worker 的耐久链是 `Pending → HostWorkDirectiveDispatchRecorded → ACK →
  status reconcile`。dispatch 事件必须在 Adapter 副作用前 append + flush +
  `sync_data`；它只是 attempt fence，不是执行或成功证明。已有 marker 的
  `Pending` 和 prior-boot 遗留但没有 marker 的 `Pending` 都只能以同一
  `directive_id` 做 `OutcomeUnknown` / status-only 对账，不得重放副作用；
- 模型、Web、MCP、A2UI 与普通 Tauri 命令不得传入任意 PID、unit 名、命令行或
  `systemctl` 动作；不得复用超级权限或 `NOPASSWD:ALL` 作为控制后门；
- 当前工作树的组合根已注册 scheduled、knowledge、编译期固定连接器、受限
  detached sub-agent 与 ASR 的异步 Stop Adapter；app cgroup 是 `essential + non-governable`
  的 status-only HostWork，不是自动停止 PinvouOS / WebKit 的动作面。旧 Mission
  同步 callback 控制面已移除，Mission Adapter 数量为 0；
- 能力管理 UI 即使显示 Resource 工具可用，也只能表示“可读取资源事实”，
  不会向模型授予 HostWork 注册或停止权。Profile helper、固定 E2E harness
  和 deb 固定 payload mode/hash 门禁已在工作树实现，但 MegaBook 真实
  deb 安装、High、OOM 与 purge 尚未执行；普通 desktop 仍 direct，4 GiB /
  8 GiB / 2 GiB 只属于显式激活的 MegaBook canary profile。不得用仓库实现、
  harness 存在或旧 direct transient canary 冒充部署事实。

HostWork、Governor、Supervisor、账本回执和 cgroup 的完整不变量见
[ADR-0009](adr/0009-PinvouOS-资源治理与Host-Supervisor.md)。

## 8. 相关文件

| 项 | 位置 |
|---|---|
| 白名单 | `pinvou3-app/src-tauri/src/features/assistant/tool_policy.rs` |
| 模式身份（`SessionMode` / 包默认策略） | `pinvou3-app/src-tauri/src/core/session_mode.rs` |
| 模式能力静态表 `MODE_TABLE` | `pinvou3-app/src-tauri/src/features/assistant/session_policy.rs` |
| 整形聚合点 | `pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs`（`shape_disallowed_tools`） |
| 禁用名单生成（含通配兜底） | `pinvou3-app/src-tauri/src/features/marketplace/mod.rs`（`model_tool_names`） |
| 组合目录物化 | `pinvou3-app/src-tauri/src/features/assistant/skill_materialization.rs` |
| 开关命令 | `pinvou3-app/src-tauri/src/app/commands/connectors.rs` |

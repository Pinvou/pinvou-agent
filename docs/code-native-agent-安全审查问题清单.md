# 原生代码会话（品悟）本地安全审查问题清单

> 来源：PR #138「代码模块接入品悟原生代码会话」合入前的本地审查（三轮）。
> 审查基准：合入后主线 `7d493aeb`。行号以该版本为准。
> 状态：PR 已合入，本清单登记的问题**均已随合入进入 main**，需在本分支逐项修复。

## 背景

#138 在代码模块新增第四个 Agent「品悟」（`agent_id = "pinvou"`，进程内 CodeWhale Engine 原生代码会话），核心设计为「两个根」：执行根（engine cwd / 文件工具 / shell）可绑定用户自选项目目录，账本根（附件 / 审计 / 产物 / 远程授权）恒为 `~/.pinvou3/sessions/<sid>/`。审阅者 h3c-hexin 提出三项合入前收口建议（持久化真相源、统一 SessionRoots、安全默认边界），修复于 `84e9e5e6` 完成并合入。本清单是修复合入后复审发现的**残留与新引入问题**。

## 问题总览

| # | 严重度 | 问题 | 位置 |
|---|---|---|---|
| 1 | 🔴 Major | AGENTS.md symlink 可注入工作区外任意文件 | bridge.rs:553 |
| 2 | 🔴 Major | 家目录边界在 Windows 上是死代码 | bridge.rs:546, 558-563 |
| 3 | 🔴 Major | sidecar 持久化修复单向，无回填自愈 | store.rs:397, codex_acp/mod.rs |
| 4 | 🔴 Major | 代码页 skill 开关静默无效（假开关） | SettingsView.jsx + skill_marketplace.rs:123 |
| 5 | 🔴 Major | 被禁连接器的 companion skills 仍活跃 | skill_marketplace.rs:126-129 |
| 6 | 🔴 Major | SessionRoots 建成未启用，调用点零迁移 | bridge.rs:385-413 等 |
| 7-20 | 🟡 Minor | 见下文 | — |
| 21-28 | ⚪ Nit | 见下文 | — |
| 29 | ⚠️ 验证缺口 | 合并后 Rust 测试本机无法复现 | CI |

---

## 🔴 Major 详述

### 1. AGENTS.md symlink 注入 — 可读工作区外任意文件进入系统提示词

**位置**：`pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs:553`

**根因**：`code_session_project_rules`（bridge.rs:535）沿项目目录链收集 `AGENTS.md` 时用 `is_file()` 检查，底座 `render_instructions_block` 用 `std::fs::read_to_string` 读取——两者都跟随 symlink。

**攻击场景**：恶意 git 仓库在根目录提交 `AGENTS.md -> ~/.aws/credentials`（或 `.ssh/id_rsa`）的 symlink。用户克隆并绑为代码会话项目目录后，打开会话即把密钥内容包进 `<instructions>` 标签发给模型供应商，全程无感知。敏感目录 shell hook 只拦截 shell 命令，拦不到提示词组装期的文件读取。

**上游防御范式**：底座 `CodeWhale/crates/tui/src/project_context.rs:1246-1252` 的 `load_context_file` 用 `symlink_metadata` 拒绝 symlink，并有测试 `project_context_rejects_symlinked_agents_md`（project_context.rs:1630）。fork 砍空 `PROJECT_CONTEXT_FILES` 的登记理由之一即「symlink 攻击面整体不可达」——本次 app 侧重开读取面未带防御。

**修复方向**：`symlink_metadata` 拒读 symlink（与上游对齐），补 symlink 拒绝测试。

### 2. 家目录边界在 Windows 上是死代码 — `~/AGENTS.md` 会被注入

**位置**：bridge.rs:546, 558-563

**根因**：`user_home_dir().canonicalize()` 在 Windows 上必返回 verbatim 路径（`\\?\C:\Users\foo`）；遍历链来自绑定归一化时已去掉 `\\?\` 前缀的项目路径（store.rs:722-727）。Rust `Path` 相等按组件类型比较，`VerbatimDisk` ≠ `Disk`，故 `dir.parent() == Some(home)` **永假**。大小写差异同理失效；home canonicalize 失败时边界完全失效（fail-open）。

**后果**：遍历穿过家目录直到盘符根，`~/AGENTS.md` 被注入——注释（bridge.rs:494-496）与 `docs/fork-modifications.md` 登记承诺的「不越过用户家目录」在 Windows 主平台上不成立。家目录边界无测试（bridge.rs:1761 注释自认「无法伪造家目录」），是漏网原因。

**修复方向**：home 与项目路径走同一归一化函数（参照 store.rs:725 `platform_compat_path`）；canonicalize 失败时不注入祖先层；补 `dir == home`（项目根即家目录）停止条件；把边界判断抽为可注入 home 参数的纯函数以便单测。

### 3. sidecar 持久化修复单向 — 存量会话无保护

**位置**：`pinvou3-app/src-tauri/src/features/codex_acp/store.rs:397`（唯一写入点）、codex_acp/mod.rs 恢复逻辑

**根因**：启动恢复只做 sidecar→索引；索引记录 `code_session=true` 但 sidecar 缺失时无补写路径。两类来源：a) 修复前构建创建的存量会话（从未写过 sidecar）；b) 绑定时 sidecar 写失败被 `eprintln!` 吞掉（store.rs:174-179）。

**后果**：此类会话在 `session-agents.json` 损坏时仍走「静默降级」老路——掉回聊天列表、执行根退回私有目录、提示词退化，正是审阅项 1 要求消灭的场景。

**修复方向**：启动恢复时对索引在而 sidecar 缺失的记录补写 sidecar。

### 4. 代码页 skill 开关静默无效（假开关）

**位置**：`pinvou3-app/src/features/settings/SettingsView.jsx`（按 scope 落盘）+ `pinvou3-app/src-tauri/src/features/marketplace/skill_marketplace.rs:123-152`

**根因**：skill 开关在 code scope 下正常落盘，但 skill 过滤器是进程级全局（fork 有意设计：「一次 toggle 全 session/窗口继承」），只读 plain scope。用户关闭的 skill 在任何会话中都仍然可见可用。

**后果**：UI 显示已关闭、实际未关闭——安全开关失效，用户基于错误的安全感授权操作。

**修复方向**（推荐方案 D）：`shape_disallowed_tools` 对代码会话追加禁用 `load_skill` 工具（skill 触达模型的唯一工具通道），代码页 skill 分组只读/隐藏。真隔离（按会话禁用单个 skill）需改 fork 底座的全局 `DISABLED_SKILLS` 机制（`CodeWhale/crates/tui/src/skills/mod.rs:876`），另开 PR 跟踪。

### 5. 被禁连接器的 companion skills 仍活跃 — 绕过安全默认

**位置**：skill_marketplace.rs:126-129（companion skill 联动只认 plain scope）

**根因**：code scope 默认全禁连接器只从 `disallowed_tools` 摘除工具，companion skills 经 `## Skills` catalogue + `load_skill` 通道仍可见可用。

**后果**：「外部能力显式开启」从 skill 维度失守；恶意仓库 prompt injection 可经 companion skill 外发代码。

**修复方向**：随方案 D 一并封死（禁用 `load_skill` 后 companion skills 同样不可加载）。

### 6. SessionRoots 建成未启用 — 调用点零迁移

**位置**：入口 bridge.rs:385-413；未迁移的分散判断：`app/commands/chat.rs:135-138`、`codex_acp/mod.rs:930-1005`、`engine.rs:853`、`engine_pool.rs:483`、`codex.rs:655`、`remote_control/snapshot.rs:194`；`SessionStore` 侧 25+ 账本消费点拿不到 bridge 入口。

**根因**：`session_roots()` 仅 bridge 内部 2 个方法消费；`SessionStore::execution_workspace()` 误导性命名仅改了注释（sessions/mod.rs:544-554）。

**后果**：后续开发者按名字误用 `execution_workspace` 当 Engine cwd（无编译错误、无运行时告警），或把账本文件写进用户项目目录造成污染；snapshot.rs:194 已演示第三条绕过路径。

**修复方向**：迁移调用点并将入口下沉到 `SessionStore` 可达层（`execution_workspace` 改名/deprecate）；或在 PR 回复中与审阅者明确收敛范围。

---

## 🟡 Minor

| # | 问题 | 位置 | 说明 |
|---|---|---|---|
| 7 | 每次启动误报 "recovered" 日志 | store.rs:564-568 + mod.rs:307-310 | 索引完好时早退 Ok 被计入 restored，淹没真实恢复信号 |
| 8 | `session_roots` 双次调 resolver 竞态 | bridge.rs:393-412 | 绑定中途解除时账本根短暂错指项目目录，造成项目目录污染 |
| 9 | 工具 profile 缺口记录被删 | docs/code-native-agent.md | 「代码会话继承全集工具」待决策项在 84e9e5e6 被整条删除而非兑现/改写，需恢复并精确描述 |
| 10 | 远程事件过滤器零单测 | remote_control/manager.rs:2452-2468 | 安全敏感代码无测试（字段名变体、None predicate、误伤普通会话） |
| 11 | RPC 通道残留未入文档 | manager.rs web_access_* 命令 | 远程端凭 session id 仍可读/写代码会话消息；事件面已封、RPC 面未封，需登记并请审阅者确认 |
| 12 | 项目根==家目录时边界失效 | bridge.rs:549-565 | 停止条件是「dir 是 home 直接子级」，绑家目录本身为项目根时永不触发 |
| 13 | 注入顺序与文档/惯例相反 | bridge.rs:548-555 | 实现 cwd→root（祖先后置生效），文档与 codex/claude 惯例为 root→cwd；需改顺序或文档明示取舍 |
| 14 | 文档/注释自相矛盾 | bridge.rs:494-496, 528-529 | 「覆盖到文件系统根」与「不越过家目录」并存，家在链上时互斥 |
| 15 | fork 登记与实现不符 | docs/fork-modifications.md | 「不越过用户家目录」受 #2 影响在 Windows 不属实 |
| 16 | sidecar `version` 读时不校验 | store.rs:183-199 | 未来高版本格式会被静默按 v1 解析，应拒读更高版本 |
| 17 | ACP 占用拒绝日志误导且不自愈 | codex_acp/mod.rs:312-314 | 残留 sidecar 每启动报 "remains degraded" 且不清理、反复重试 |
| 18 | 扫描根与读取根两个来源 | mod.rs:290 vs mod.rs:304/store.rs:140 | 当前等价，潜在分歧点 |
| 19 | `disabled_connectors.json` 读-改-写非原子 | marketplace/mod.rs:473-518 | 开关与安装同步并发时可能丢更新 |
| 20 | `parse_connector_scope` 静默回退 | app/commands/connectors.rs:36-40 | 任意非 "code" 字符串静默映射为 plain，前端笔误无报错 |

## ⚪ Nit

| # | 问题 | 位置 |
|---|---|---|
| 21 | 注释误写「原生代码会话（ACP/品悟）」，实际仅品悟原生 | manager.rs:2305-2310 |
| 22 | 家目录边界无测试且自认不可测——抽纯函数即可测 | bridge.rs:1761 |
| 23 | 缺 100KB 截断端到端断言、双门控「resolver 命中但非 code_session」分支测试 | bridge.rs 测试 |
| 24 | 会话中途编辑 AGENTS.md 不生效（spawn 时定型），文档未说明 | docs/code-native-agent.md §8.5 |
| 25 | `entries.flatten()` 静默丢弃 read_dir 迭代错误 | codex_acp/mod.rs:296 |
| 26 | `set_acp_workspace` 先删 sidecar 后 persist，persist 失败时短暂不一致 | store.rs:358-360 |
| 27 | 死代码：`ConnectorScope::as_str`、`apply_disabled_connectors` 无调用方 | marketplace/mod.rs:424-430, 24-26 |
| 28 | prompts.rs `source` 属性未转义（底座既有行为，纯外观） | CodeWhale prompts.rs:281 |

## ⚠️ 验证缺口

29. 合并后 head 的 Rust 测试二进制在审查机三次尝试（含全新重建）均启动即 `0xc0000139`（STATUS_ENTRYPOINT_NOT_FOUND），判定为审查机环境问题（同机早前跑通过、未动依赖、失败在 DLL 加载阶段）。「905 过」的测试声称无法本机独立复现，**需以 CI `rust-test` 正式 check 为准**。

## 修复优先级

1. **#1**（symlink 拒读）→ **#2**（home 归一化比较）→ **#4/#5**（方案 D：代码会话禁用 `load_skill` + UI 诚实化）→ **#3**（sidecar 回填）→ **#6**（SessionRoots 收敛口径）→ #9/#10/#11 → 其余 minor → nit。
2. #1/#2/#4/#5 为安全相关必修；#3 是审阅项 1 的根因残留；#6 需与审阅者对齐口径。
3. 组合风险：#1+#2+#4/#5 可形成完整外发链路（symlink 读密钥进提示词 → 注入链穿家目录 → 未真正禁用的 skill 外发），单独看是边界情况，组合即实际攻击路径。

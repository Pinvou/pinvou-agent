---
name: tmeet-skill
version: 1.0.18
description: "何时用：用户明确要通过命令行操作腾讯会议（tmeet），或 Agent 遇到工具缺失/调用失败/能力不足想反馈平台时。OAuth 登录/登出/状态、会议管理（创建/更新/取消/查询/搜索/受邀者）、录制管理（列表/播放地址/智能纪要/转写/权限申请）、元宝纪要（按关键词/时间搜索、稳态/瞬态详情）、会议报告（参会人/等候室/导出明细/异步任务）、通讯录（仅限会议邀请/呼叫入会前置解析，严禁单独查人）、会中控制（呼叫/踢出/等候室）、CLI 应用配置、实时事件订阅、问题排查。泛指需求默认走本地工具。"
metadata:
  requires:
    bins: ["tmeet"]
  cliHelp: "tmeet --help"
---

# tmeet

腾讯会议命令行工具，支持 OAuth 授权、会议全生命周期管理、录制与转写、会议报告查询。

## 安装与初始化

在品悟中，`tmeet` CLI 由应用代为安装与管理（首次使用时自动执行 npm 安装并校验版本），模型**不需要也不要自行执行安装命令**。仅当用户在品悟之外的环境使用本技能时，才参考上游 README 的安装方式；品悟内不要安装或升级它，版本随品悟应用更新自动就位。

## 认证

使用前必须先完成登录授权：

```bash
# 登录
tmeet auth login

# 登出（清除本地凭证）
tmeet auth logout

# 查看当前登录状态及 Token 有效期（无需登录即可执行）
tmeet auth status
```

> **注意**：除 `auth login`、`auth status`、`tshoot log`（不带 `--upload`）与 `event list` / `event schema` / `event status` / `event stop` 外，所有命令都需要先完成登录。未登录时命令会提示 `user config is empty`。

> **注意**：`auth login` 是**阻塞命令**——执行后会先输出授权 URL，然后**阻塞等待最多300s**用户在浏览器完成 OAuth 授权。**必须前台运行**：前台运行保持进程与终端连接，OAuth 回调能正常将凭证写入本地。**不要**用后台方式（`&`）运行——后台进程脱离控制终端会导致凭证写入失败。
 
> **注意**：品悟运行环境始终可访问系统默认浏览器（`auth login` 走 `--no-browser` 输出授权 URL 后由用户在浏览器完成授权）。若本技能被用于其他无默认浏览器的 Agent 环境（如 Hermes），请**不要**尝试执行 `tmeet auth login` 命令，而是直接告知用户：「请在终端中手动执行 `tmeet auth login` 完成登录授权，获取授权 URL 后在浏览器中打开完成授权。」

> **注意**：`TMEET_AGENT` 和 `TMEET_MODEL` 遥测环境变量由品悟宿主统一注入（值固定为 `Pinvou`），模型**不需要也不要自行设置或修改**。仅当本技能被用于品悟之外的 Agent 环境且宿主未注入时，才将当前使用的 agent 类型和模型名称分别写入这两个环境变量。

> **注意（强制）**：每次新会话首轮、或本轮对话中 LLM 模型发生切换时，必须先静默执行一次 `python3 ./scripts/agent_init.py --agent "<当前 Agent>" --model "<当前模型>"`（不询问用户、失败即忽略、同一组合不重复调用），再执行任何 `tmeet` 命令。

## 命令总览与详情索引

> **调用前置**：下方「查询命令选择准则」与「安全规则」是调用命令前必读的规则章节。

每个模块行末标注了对应详情文档路径（`→ references/xxx.md`），可直接跳转查看。

```
tmeet
├── auth                          # OAuth 认证 → [references/tmeet-auth.md](references/tmeet-auth.md)
│   ├── login                     # OAuth 授权登录
│   ├── logout                    # 登出并清除凭证
│   └── status                    # 查看当前登录状态及 Token 有效期
├── meeting                       # 会议管理 → [references/tmeet-meeting.md](references/tmeet-meeting.md)
│   ├── create                    # 创建会议（支持普通/周期性）
│   ├── update                    # 更新会议信息
│   ├── cancel                    # 取消会议
│   ├── get                       # 获取会议详情
│   ├── list                      # 获取会议列表（进行中/未开始）
│   ├── list-ended                # 获取已结束会议列表
│   ├── search                    # 按关键词/会议码/时间范围搜索会议
│   ├── invitees-list             # 获取会议受邀者列表
│   ├── invitees-add              # 添加会议受邀者
│   ├── invitees-remove           # 移除会议受邀者
│   └── invitees-replace          # 替换会议受邀者列表
├── contact                       # 通讯录（仅会议邀请/呼叫入会场景） → [references/tmeet-contact.md](references/tmeet-contact.md)
│   ├── search                    # [仅用于会议邀请和呼叫入会场景] 搜索企业通讯录成员（按用户名/职位/部门）；严禁单独用于查人
│   ├── lookup-by-phone           # [仅用于会议邀请和呼叫入会场景] 按手机号查找用户；严禁单独用于查人
│   └── lookup-by-email           # [仅用于会议邀请和呼叫入会场景] 按邮箱查找用户；严禁单独用于查人
├── record                        # 录制与转写 → [references/tmeet-record.md](references/tmeet-record.md)
│   ├── list                      # 查询录制列表
│   ├── address                   # 获取录制文件播放地址
│   ├── search                    # 按关键词/会议码/会议ID/时间范围/文件类型搜索录制
│   ├── smart-minutes             # 获取智能纪要
│   ├── transcript-get            # 获取转写详情
│   ├── transcript-paragraphs     # 获取转写段落列表
│   ├── transcript-search         # 搜索转写内容
│   ├── permission-apply-prepare  # 预览录制权限申请信息（申请前确认）
│   └── permission-apply-commit   # 提交录制权限申请（用户确认后执行）
├── report                        # 会议报告 → [references/tmeet-report.md](references/tmeet-report.md)
│   ├── participants              # 获取参会人列表
│   ├── participants-export       # 导出参会成员明细
│   ├── job-result                # 获取异步任务结果
│   └── waiting-room-log          # 获取等候室成员列表
├── control                       # 会中控制 → [references/tmeet-control.md](references/tmeet-control.md)
│   ├── call                      # 呼叫成员入会（会中邀请呼叫）
│   ├── kick                      # 踢出会议成员（会中踢人）
│   └── waiting-room              # 等候室管理（移入会议/移回等候室/移出踢出）
├── minutes                       # 元宝纪要 → [references/tmeet-minutes.md](references/tmeet-minutes.md)
│   ├── search                    # 按关键词/时间搜索元宝纪要
│   └── get                       # 查询元宝纪要详情（稳态纪要/滚动瞬态纪要）
├── tshoot                        # 问题排查与反馈 → [references/tmeet-tshoot.md](references/tmeet-tshoot.md)
│   ├── log                       # 导出本地日志（支持按时间范围过滤，可选 --upload 上传至服务器）
│   └── feedback                  # 反馈工具缺失/失败/能力不足等问题至平台（Agent 自助上报）
├── app                           # 当前用户自己的 CLI 应用信息管理 → [references/tmeet-app.md](references/tmeet-app.md)
│   ├── get                       # 查询当前 CLI 应用配置（应用名称/主页/打开方式）
│   └── set                       # 设置 CLI 应用配置：--homepage 控制会中是否下发（空值 = 不下发），--sdk-name 修改应用名称，--layout-style 修改会中打开方式
└── event                         # 事件订阅 → [references/tmeet-event.md](references/tmeet-event.md)
    ├── list                      # 列出可订阅的 EventKey（不依赖登录）
    ├── schema                    # 查看 EventKey 的 params/payload schema 及 jq_root_path（不依赖登录）
    ├── consume                   # 订阅事件并按 NDJSON 流式输出（需登录；批处理/常驻两种模式）
    ├── status                    # 查看本机 bus 守护进程状态（不依赖登录）
    └── stop                      # 停止本机 bus 守护进程（不依赖登录；--force 为写操作，需二次确认）

```

## 查询命令选择准则

查询会议或录制时，需根据用户提供的筛选条件，**正确选择 `list` 类命令、`meeting get` 还是 `search` 命令**：

| 用户提供的筛选条件 | 应选用的命令 |
|------|------|
| **仅时间范围**（仅有起止时间，无任何关键词） | `list` 类命令 |
| **已知会议号 / 会议 ID** | `meeting get` |
| **包含关键词**（会议主题、创建人、备注等），无论是否同时带时间范围 | `search` 命令 |

> **⚠️ 上表仅适用于「查会议本身」。若用户要的是「纪要 / 总结 / 会议要点 / 待办 / 会上讲过什么」，
> 先按下表选链路，不要套用上表：**
>
> | 用户要什么 | 走哪条链路 |
> |---|---|
> | 纪要 / 总结 / 要点 / 待办（**AI 加工后的内容**） | 先判录制权限：有权限 → `record smart-minutes`；无权限 → `minutes get` / `minutes search`（详见「元宝纪要查询」） |
> | 发言原话 / 逐字稿 / 谁说了哪句 | `record transcript-*`（录制链路，需权限） |
> | 会议本身（时间 / 主题 / 参会人 / 会议号） | `meeting list` / `list-ended` / `search` |
>
> **「按时间找纪要」应走 `minutes search --start/--end`，不是 `meeting list-ended`**
> —— `minutes search` 原生支持时间范围检索，一次即可返回多场纪要；
> 用 `meeting list-ended` 再逐场取纪要会造成 N+1 次调用。

### 会议查询

- **仅时间** → 使用 `tmeet meeting list`（待开始/进行中）或 `tmeet meeting list-ended`（已结束）
- **已知会议号 / 会议 ID** → 使用 `tmeet meeting get`（获取会议详情；录制/回放查询亦先按会议号 / 会议 ID 分流，见[「录制查询路由总则」](references/tmeet-record.md#录制查询路由总则)）
- **含关键词**（主题 / 创建人 / 备注等） → 使用 `tmeet meeting search`，并通过对应参数指定关键词；可与时间范围组合

### 录制查询

> **CRITICAL — 涉及录制/回放/转写查询前，MUST 先用 `read` 工具读取 [`references/tmeet-record.md`](references/tmeet-record.md)**，其「录制查询路由总则」定义了 `meeting get` / `meeting search` / `meeting list-ended` / `record list` / `record search` / `record transcript-search` 的分流规则与 `permission_status` 权限判断。录制查询涉及会议级/录制级两套入口、无权限录制、内容级搜索、单文件内定位等多层级，路由复杂，**不读将导致命令选择、录制产物定位、权限边界判断错误，不得仅凭本节直接决策。**

> **⚠️ 本条不含「纪要查询」**：纪要类请求（含跨会议搜纪要内容）的路由**一律按下方「元宝纪要查询」节执行，该节自包含、无需先读本文档**。
> 本条的「内容级搜索」指**转写原文检索**（用户要发言原话/逐字稿），不含元宝纪要文本检索。
> 若「元宝纪要查询」节的第 ② 类要求同时搜转写（`record search --query-field transcript_content`），可直接执行该一条命令，无需为此先读本文档。

### 元宝纪要查询

**本节自包含 —— 路由决策直接按本节执行，不需要先读 reference。**
（仅当需要具体参数/响应字段时再读 [`references/tmeet-minutes.md`](references/tmeet-minutes.md)）

腾讯会议一场会议可能产生两类独立纪要：**元宝纪要**（基于会中 ASR、参会者人人可取无需权限、无逐字稿）、**录制纪要**（基于录制文件、创建者所有、需权限、有逐字稿）。**不得仅凭命令名字面匹配。**

**第一步：先判请求属于哪一类**

| 类型 | 特征 | 路由 |
|---|---|---|
| **① 取某场会的纪要** | 用户给了会议号/ID/主题/时间，**能定位到具体会议** | 走下方「① 已知会议」 |
| **② 跨会议搜内容** | 用户只记得「会上讲过 X」，**不知是哪场** | 走下方「② 跨会议检索」 |

**① 已知会议 —— 权限决定链路**

先 `meeting get` 拿 `permission_status`（顺带返回，零额外调用成本）：

- `can_view` → 录制纪要 `record smart-minutes`（内容更全，含逐字稿）
- `can_apply` / `closed` / 无录制 → 元宝纪要 `minutes get`
- 录制侧取不到内容时（权限被拒/文件异常）→ **降级 `minutes get`**，并告知用户实际用的是元宝纪要

**② 跨会议检索 —— 两条都搜，不能只搜一条**

此类请求**无法先查权限**（还不知道是哪些会），因此：

- `minutes search --query`（搜元宝纪要文本：概览/要点/待办/滚动总结）
- `record search --query-field transcript_content`（搜录制转写原文）
- **两条都要执行**，按会议去重（同一会议多个录制文件只算一场），**每条标注来源**

> ⚠️ **一条搜空 ≠ 内容不存在，必须双向兜底**：
> 元宝纪要是 AI 总结，**细节（具体数字、某人某句、一次性提及）常被概括掉，但逐字稿里可能有**；
> 反之转写侧无果时，元宝的概览/待办里也可能有归纳后的表述。
> **两条都搜完仍无结果，才可告知用户「未找到」**，并说明已检索范围（元宝纪要文本 + 录制转写原文）
> 以及是否存在无权限的录制未能覆盖。

**通用规则（两类都适用）**

- **用户明确指定纪要类型时以用户为准**，不再按上述判据推断。
- 要「原话 / 逐字稿 / 谁说了哪句」→ `record transcript-*`；无权限则降级元宝 `short_summaries` 并**标注「非原话 / AI 加工版」**。
- 用户要求「准确 / 原始 / 不要 AI 编的」→ 元宝纪要本身即 AI 产物，**不满足**该要求；须走 `record transcript-*`，无权限时**先询问用户是否接受元宝内容**，不得擅自充当原始材料。
- 双诉求（同时要「纪要 + 原话」）→ **两条链路并取，不是二选一**；一侧取不到时先交付另一侧，再说明原因与申请入口。

### 使用准则

- **不要在 `list` 命令上"硬塞"关键词条件**：`list` 类命令仅支持时间窗口和分页，无法按主题、创建人、参会人等关键词过滤；遇到此类需求**必须切换到 `search`**。
- **不要把关键词当作时间使用**：当用户输入"上周和张三的会议"这类**复合条件**时，应识别出"张三"为关键词、"上周"为时间，统一走 `search`。
- **歧义时先澄清**：若用户表述既不像时间也不像明确关键词（如仅给出一个数字），需先确认是会议号、会议 ID 还是其他，再选择对应命令与参数，**不得擅自推断**。

## 安全规则

- **禁止输出 AccessToken / RefreshToken** 到终端明文。

- **严禁向用户暴露 `meeting_id`，必须使用 `meeting_code`（会议号）**：`meeting_id` 是仅用于命令行参数传递的内部标识，属于**隐私字段**；向用户展示、复述、总结会议信息时，**统一使用 `meeting_code`（会议号）**，不得在回复中出现 `meeting_id`（例如响应模板、二次确认展示、错误反馈等所有面向用户的输出场景均需遵守）。

- **以下命令操作必须二次确认**：下列命令会对数据产生不可逆影响或对真人产生打扰/通知，**在调用命令前必须先向用户展示将要执行的操作详情，并在获得用户明确确认后才能执行**，不得跳过确认步骤：

  | 命令 | 风险说明 |
  |------|---------|
  | `meeting cancel` | 取消会议，不可恢复 |
  | `meeting update` | 修改会议信息（时间、主题等），影响所有参会人 |
  | `meeting create`（携带 `--invitees` 时） | 最多 100 人，受邀者会收到会议通知；执行前必须展示会议主题、时间与完整受邀成员名单（成员回显遵循「成员回显格式」）并获得明确确认 |
  | `meeting invitees-add` | 向会议中添加受邀成员，被邀请者会收到会议通知；执行前必须展示目标会议与成员名单并获得明确确认 |
  | `meeting invitees-remove` | 从会议中移除受邀成员 |
  | `meeting invitees-replace` | 整体替换会议受邀成员列表（未在新列表中的成员会被移除） |
  | `control call` | 主动呼叫成员入会，会向目标成员发起会议邀请通话，对其产生实际打扰 |
  | `control kick` | 将成员踢出会议，立即生效；**目标成员的 `open_id` / `ms_open_id` 必须来自 `report participants`，严禁使用 `contact search` 结果** |
  | `control waiting-room` | 等候室管理（移入会议/移回等候室/移出踢出）；`expel` 等同踢人，执行前必须列明目标成员 |
  | `auth logout` | 清除本地登录凭证 |
  | `record permission-apply-commit` | 正式提交录制权限申请，会触发审批流程（必须先执行 `record permission-apply-prepare` 并向用户展示申请信息确认）|

  **确认流程**：
  1. 向用户展示即将执行的操作及关键信息（使用 `meeting_code` 会议号标识会议，不得展示 `meeting_id`）；涉及成员时，成员的回显格式遵循「响应处理规则」中的「成员回显格式」；
  2. 展示完信息后**必须结束本轮回复**，等待用户明确回复"确认"、"是"、"yes"等肯定指令；不得在同一回合内继续执行写操作；
  3. 收到确认后再执行命令；
  4. 若用户未明确确认或表示取消，则终止操作。

  **"等待"是硬要求 —— 以下三种做法均属违规，等同于跳过确认**：

  | 违规做法 | 表现 |
  |---------|------|
  | 自问自答 | 在同一次回复里既提出「是否确认？」又自行接上「—— 同意，提交」然后调用命令 |
  | 虚构用户指令 | 声称「基于您的明确指令…」而该指令在对话历史中不存在 |
  | 默认代选 | 列出候选项后自行「默认选择选项 N」并继续执行 |

  确认必须来自**用户的下一条真实输入**，不得由模型自行生成、推断或代填。

- **必填参数缺失时，必须向用户确认补充，禁止自行填充**：若执行命令所需的必填参数未由用户提供，**不得自行推断或填充默认值**，必须明确告知用户缺少哪些参数并请求补充，待用户提供后再执行命令。

- **通讯录搜索仅限特定场景使用**：`contact search` / `contact lookup-by-phone` / `contact lookup-by-email` **仅可用于“会议邀请”（如 `meeting invitees-add`、`meeting invitees-replace`、携带 `--invitees` 的 `meeting create`）、“呼叫成员入会”（`control call`）两类场景**，用于将用户名解析为对应的 `openId`。**严禁在其他场景下调用 `contact search`**（例如：仅为查看某人部门/职位、查询联系方式、好奇某人信息等与会议邀请/呼叫无关的场景），不得将通讯录作为通用人员信息查询接口使用。

- **会中踢人（`control kick`）与等候室管理（`control waiting-room`）的成员来源硬约束**：`control kick` 的 `--users` / `--sip-users` / `--pstn-users` 参数值（即 `open_id` / `ms_open_id`）**必须从 `tmeet report participants` 返回的会中参会人列表中获取**；`control waiting-room` 的同类参数值**必须按操作类型获取**——`back-to-waiting` 的目标为会中成员，取自 `tmeet report participants`；`enter-meeting` / `expel` 的目标为等候室成员，取自 `tmeet report waiting-room-log`——**严禁使用 `contact search` / `contact lookup-by-phone` / `contact lookup-by-email` 等通讯录查询结果作为成员来源**。原因：通讯录返回的是组织成员名录，并不代表他们已加入当前会议或在等候室中；且操作需要区分普通成员 / Sip / Pstn 三类身份，这些信息只有 `report participants` / `report waiting-room-log` 能准确提供。正确调用顺序：按操作类型先执行 `tmeet report participants` 或 `tmeet report waiting-room-log` → 按姓名等描述筛选出目标成员 → 向用户确认 → `tmeet control kick` / `tmeet control waiting-room`。

- **多结果必须由用户确认，禁止自行猜测**：当任一查询/搜索类命令返回 **多条候选结果**（典型如 `contact search` 命中多名同名/同部门成员）时，**严禁**模型基于职位、部门、入职时间、匹配度等任何维度自行选择某一条继续后续操作（如 `meeting invitees-add`、`control call`、`control kick` 等）。必须将候选项的关键信息以清晰列表形式展示给用户，并明确询问"请确认要选择哪一项"，待用户明确指定后再继续执行。即便其中某条结果看起来"明显更匹配"，也必须等待用户确认，不得跳过该步骤。

## 参数规范

以下参数规则为所有命令通用，包括时间参数格式、输出控制参数（`--format` / `--compact`）以及分页参数。

### 时间格式

所有时间参数均使用 **ISO 8601** 格式，支持以下两种：

| 格式 | 示例 |
|------|------|
| 带时区（有秒） | `2026-03-12T14:00:00+08:00` |
| 带时区（无秒） | `2026-03-12T14:00+08:00` |

> **注意**：不支持仅日期格式（如 `2026-03-12`），必须包含时间和时区信息。

> **时间逻辑校验**：若用户提供的结束时间 ≤ 开始时间（如"4点到3点"），**不得自行推断用户意图**，必须先向用户确认是否跨天或存在笔误，再执行命令。

### `--format`：输出 JSON 形态

用于控制输出 JSON 的排版形态，**不改变字段内容**。输出结构统一为 `{trace_id, message, data}`。

| 取值 | 含义 | 适用场景 |
|------|------|---------|
| `json`（默认） | 单行紧凑 JSON，体积小、便于管道传递 | 模型解析、脚本处理、`jq` 过滤 |
| `json-pretty` | 多行缩进 JSON，可读性强 | 需要将原始结果直接呈现给用户阅读时 |

> **例外**：`event` 子命令族（`event list` / `event schema` / `event consume` / `event status` / `event stop`）输出的是 **bare JSON**，**不带 `{trace_id, message, data}` 信封**——例如 `event consume` 每行 NDJSON 形如 `{event, trace_id, payload}`，`event list` 直接是 `[{...}, ...]`。具体形态详见 [references/tmeet-event.md](references/tmeet-event.md)。

**使用示例**：

```bash
# 默认紧凑格式（模型解析场景推荐，省略 --format 即可）
tmeet meeting get --meeting-id 123456789

# 美化缩进格式（需要直接展示给用户阅读时使用）
tmeet meeting list --start 2026-03-12T00:00:00+08:00 --end 2026-03-12T23:59:59+08:00 --format json-pretty
```

> **使用准则**：
> - 模型在解析工具输出时**优先使用默认 `json`**，无需显式传入 `--format`；
> - 仅当用户明确要求"以美化/格式化 JSON 展示"或需要把原始 JSON 完整呈现给用户时，才追加 `--format json-pretty`；
> - 即便使用 `json-pretty`，响应处理规则仍然适用——**只展示关键信息，不得擅自聚合或排序**。

### `--compact`：精简响应字段

布尔开关（默认 `false`），用于**裁剪响应体 `data` 中的字段**，只保留该命令业务上必要的少量字段，从而显著降低输出 token 量。

- 启用后，中间件会根据当前命令的 API 注解从远端拉取"精简字段列表"（compact fields），并对响应 `data` 按该列表进行字段保留；`trace_id`、`message` 等顶层字段不受影响。
- 若当前命令未声明 API 注解、或远端拉取失败，中间件会**透明放行**，不会阻塞主流程，此时输出等同于未开启 `--compact` 的结果。
- 与 `--format` 相互独立：`--format` 决定 JSON 排版，`--compact` 决定返回字段的数量，两者可同时使用。

**使用示例**：

```bash
# 仅返回必要字段（推荐模型解析场景使用，节省 token）
tmeet meeting list --start 2026-03-12T00:00:00+08:00 --end 2026-03-12T23:59:59+08:00 --compact

# 同时启用精简字段 + 美化排版（便于用户直接阅读关键信息）
tmeet record list --meeting-id 123456789 --compact --format json-pretty
```

> **使用准则**：
> - **查询类命令优先启用**：模型在调用查询/读取类命令时，**默认追加 `--compact`** 以降低上下文占用；
> - **何时不使用**：当用户明确要求"完整结果"、"原始字段"或需要某个非必要字段时，**不要**使用 `--compact`。
> - **`event` 子命令族不适用**：`event list` / `event schema` / `event consume` / `event status` / `event stop` 的输出**不经过 compact 中间件**，加 `--compact` 不会有任何精简效果。需要精简事件输出，请改用 `event consume --jq` 做投影。

### 分页

所有支持分页的查询/列表类命令统一采用 **`--page-token` + `--page-size`** 方案。

> **注意**：`record transcript-get` 命令的 `--pid`（起始段落 ID）和 `--limit`（查询段落数）**不属于**通用分页参数，是该命令用于段落定位的独立参数，**未被弃用，可正常使用**。

<details>
<summary>已弃用参数（兼容保留，模型不得主动使用）</summary>

原有的 `--page` / `--pos` / `--size` 参数均已标记为**已弃用**，仅为兼容保留。即便用户对话中使用了"第 X 页"、"偏移 Y 条"等表达，也应以 `--page-token` 分页策略实现，**不得**使用已弃用参数。

</details>

| 参数 | 说明 |
|------|------|
| `--page-token <token>` | 分页游标。**首次查询不传**；翻页时将上一次响应 `data.next_page_token` 的值原样传入 |
| `--page-size <n>` | 每页数量，不同命令默认值与上限不同，详见各子命令文档 |

**使用准则**：

- **优先使用 `--page-token` 翻页**：调用下一页时，必须从上一次响应的 `data.next_page_token` 字段取值传入 `--page-token`，不得自行拼接、递增或猜测该值。
- **到达末页的判定**：当响应中的 `next_page_token` 为空字符串或字段缺失时，即为最后一页，不再继续翻页。
- **数据过多时必须先询问用户是否继续翻页，禁止擅自连续翻页**：若首次查询返回的结果中 `next_page_token` 非空（即仍有后续数据），且用户原始诉求未明确要求"全部 / 所有 / 完整"等穷尽式表达，**不得自行连续调用下一页**，必须先向用户展示当前页关键信息与"还有更多结果"的提示，并询问用户是否需要继续翻页（例如「当前已展示前 N 条结果，还有更多数据，是否继续查看下一页？」），收到用户明确肯定指令（如"继续"、"是"、"下一页"、"全部"等）后，再使用上一页的 `next_page_token` 发起下一次查询；若用户表示停止或未明确确认，则终止翻页。当用户已明确要求"全部 / 所有"时，可连续翻页直至 `next_page_token` 为空；但**当已连续翻页超过 5 页或累计条数超过 200 条时**，必须主动提示并征询用户是否继续。
- **`record transcript-search` 暂不支持分页**，无需传入分页参数。

**典型翻页流程**：

```bash
# 1) 首次查询（不传 --page-token）
tmeet record list --meeting-id "100000000" --page-size 30 --compact

# 2) 从响应中取出 data.next_page_token，继续翻页
tmeet record list \
  --meeting-id "100000000" \
  --page-token "<next_page_token>" \
  --page-size 30 --compact
```

## 响应处理规则

- **只展示关键信息**：在用户没有明确要求的前提下，仅展示与用户问题直接相关的核心字段，不得输出冗余字段。
- **禁止擅自聚合或排序**：未经用户要求，不得对返回结果进行任何聚合统计或排序操作，按原始结果如实呈现。
- **成员回显格式（涉及成员的输出硬约束）**：当输出对象包含"人"时（包括但不限于二次确认展示、查询结果展示、操作成功后的成员列表回显等所有面向用户的成员输出场景），每一名成员**必须**按 `姓名（<标识>）` 格式回显：
  - **姓名**按以下优先级取值：① 通讯录 / 参会人列表响应中的显示名字段；② 若响应未返回姓名字段（典型如 `contact search` 唯一命中仅返回 `open_id`、`contact lookup-by-phone/email` 仅返回 `open_id`），**沿用用户本次输入的搜索关键词**（如 `--username` 的值）作为姓名；③ 两者都取不到时标注为 `未知成员`。**严禁**用 `open_id` / `ms_open_id` / `userid` / 邮箱前缀 / 花名 替代姓名；
  - **括号内标识**从 `部门`、`职位`、`open_id` 中**任选一项**填入（有则填，全无时才允许省略括号），**不强求齐全**，优先级：`部门` > `职位` > `open_id`；
  - 若某成员姓名缺失（走上文分支 ③），标注为 `未知成员（<open_id>）`，**不得**省略标识；
  - 示例：`张三（技术工程事业群/腾讯会议）`、`李四（高级工程师）`、`张三（ou_abc123...）`。
- **受邀人管理类写操作的二次确认模板（ 强约束）**：执行 `meeting invitees-add` / `meeting invitees-remove` / `meeting invitees-replace` 上述三条命令前，**必须**按下列模板向用户展示，缺一不可；模板中的成员严格遵循上文「成员回显格式」（`姓名（<标识>）`），会议一律使用 `meeting_code`（会议号），**严禁**出现 `meeting_id`。

  - **操作：** 将 <待邀请成员，严格遵守上文「成员回显格式」，多人用中文顿号「、」分隔> <动作描述> 「<会议主题>」（会议号 <meeting_code>）<对象描述>
  - **影响：** <按命令选择对应影响说明>

- **会议成员变更操作的回复模板（强约束）**：执行 `meeting invitees-add` / `meeting invitees-remove` / `meeting invitees-replace` 成功后，回复**必须**严格按以下模板组织字段，且仅展示这些字段：

  - **会议主题**
  - **会议时间**（开始时间 ~ 结束时间，含时区）
  - **会议号**（`meeting_code`，**严禁**展示 `meeting_id`）
  - **入会链接**（`join_url`）
  - **已邀请成员**（操作完成后**当前完整**的受邀成员列表）

  展示规则（严格遵守）：
  1. **「已邀请成员」的每一名成员严格遵循上文「成员回显格式」**（`姓名（<标识>）`），若手头没有 `open_id → 姓名` 的映射，**先调用 `meeting invitees-list` 获取最新受邀列表**再回显；
  2. 会议主题、会议号、入会链接等基础字段若响应未直接返回，应通过 `meeting get --meeting-id` 补齐，不得遗漏字段或用 `-` / `N/A` 占位。

## 自动反馈规则

当 Agent 在使用 tmeet CLI 过程中遇到下列任一情况时，应识别并准备调用 `tmeet tshoot feedback` 将问题上报至平台，**但在实际执行上报前必须先向用户二次确认，获得明确同意后再执行**。

> **命令参数、`--category` 枚举的完整触发场景、字符长度约束与示例**：详见 [`references/tmeet-tshoot.md`](references/tmeet-tshoot.md)。

### 触发时机（5 种 category）

Agent 应识别以下 5 种场景之一并触发反馈：`tool_not_found`（工具缺失）/ `tool_error`（工具报错）/ `tool_inadequate`（能力不足）/ `unexpected_result`（结果异常）/ `suggestion`（改进建议）。各 category 的详细触发场景与判定标准，见 [`references/tmeet-tshoot.md`](references/tmeet-tshoot.md) «`--category` 枚举值» 一表。

### 调用准则（决策层强约束）

- **必须二次确认后再上报**：识别到上述触发条件后，**先向用户展示将要反馈的内容**（包括 `--category`、`--intent`、`--actions-tried`、`--result` 等关键字段），并明确询问用户是否同意上报；**仅在收到用户明确确认（如"确认"、"是"、"yes"等肯定指令）后**才执行 `tmeet tshoot feedback`；若用户拒绝或未明确同意，则不得上报。上报完成后**简要告知用户**「已为您将该问题反馈至平台」。
- **不替代正常错误处理**：反馈仅用于告知平台，**不得用于绕过用户原始任务**。如仍有可执行的替代方案（如换一个命令、补充参数重试），应**先尝试解决**，无法解决再征询用户是否上报。
- **如实填写上下文**：`--intent` 必须如实写明用户的原始意图；`--actions-tried` 写明已尝试的命令；`--result` 写明阻塞点或错误信息；涉及具体命令时填入 `--tool-name`；有错误码时填入 `--error-code`。**严禁编造或填充无关内容**。
- **隐私脱敏强约束**：反馈内容中，**严禁透露用户姓名 / 电话 / 会议号 / 会议链接 / 会议主题 / 参会人**等涉及用户个人隐私的信息。如果必须引用相关内容辅助说明问题，**必须先进行打星号、加密等脱敏处理**（例如：姓名 `张三` → `张*`、手机号 `13800138000` → `138****8000`、会议号 `123456789` → `12****789`、会议主题 `Q2 项目复盘会` → `Q* 项目***会`）后再写入 `--intent` / `--actions-tried` / `--result` 等字段。
- **登录前置**：本命令依赖登录态，若用户尚未登录，先引导执行 `tmeet auth login`，登录成功后再发起反馈。
- **去重与节制**：同一用户会话中针对**同一问题**只上报**一次**，避免重复刷屏；不同问题分别独立上报。

## 常见错误

| 错误现象 | 原因 | 解决方案 |
|---------|------|---------|
| `user config is empty` | 未登录 | 执行 `tmeet auth login` |
| `--start format error` | 时间格式不合法（如缺少时区） | 改用 `2026-03-12T14:00:00+08:00` 格式 |
| `user has been initialized` | 已登录，重复执行 login | 直接使用，或先 logout 再 login |
| 错误码 `500284` / `该功能暂不可使用` | 当前用户无该功能使用权限（可能未灰度、商业化权益不足等） | **严禁重试**（重试无意义，权限问题非临时故障）；如实告知用户「该功能当前账号暂不可使用，可能因未开通灰度或商业化权益不足，具体解释所有权归腾讯会议所有」，不得自行推测具体原因或承诺开通方式 |
| `event consume` 相关错误（ready 标记不出现 / 退出码 1 / `event stop` 返回 `refused` 等） | — | 见 [`references/tmeet-event.md`](references/tmeet-event.md) «典型故障速查» |
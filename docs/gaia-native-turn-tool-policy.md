# GAIA NativeTurn 工具与权限安全规范

状态：拟实施

版本：v1

日期：2026-08-13

## 目标

为 GAIA `NativeTurn` 提供评测专属、可审计、默认拒绝的工具权限，同时复用完整产品
EnginePool、模型路由、工具循环和附件解析链路。允许按显式 profile 使用隔离 session
workspace、本地图片/Office 预解析及可选公网检索；禁止 shell、代码执行、通用写入、
子智能体和动态/MCP 扩权；普通 GUI 行为不得变化。

## 当前风险

当前 headless bridge 提交 `AppMode::Yolo`、`restrict_tools=false`。Pinvou bridge 会将其构造为
`allow_shell=true`、`trust_mode=true`、自动批准和不受限工具表。GAIA 问题和附件是不可信
输入，prompt injection 因而可能诱导读取 workspace 外文件或执行命令。

`AppMode::Plan` 不是替代方案：Pinvou 当前 Plan turn 仍为 trust mode，CodeWhale Plan 工具集
也远超 GAIA 需求，且规划提示会改变答题语义。

CodeWhale 路径保护还有两个旁路：用户持久化的 `trusted_external_paths`，以及
`workspace_follow_symlinks=true` 时 workspace 内链接可指向外部。因此只关闭 `trust_mode`
不足以建立边界。

## 非目标与 fork 边界

- 不为 GAIA 开放任意 shell 或任意写文件能力。
- 不让 adapter 直接提交工具名数组、hook 或 `EngineConfig`。
- 不用提示词代替权限校验。
- 不修改普通 GUI 的工具策略、审批设置或 mode 语义。
- 优先在 app 的 hook/config/runtime seam 完成，不修改 CodeWhale fork。
- 若 app 层不能实现硬门禁，必须另立提案，遵循 `docs/fork-policy.md`、更新
  `docs/fork-modifications.md`、指纹和行为测试，并运行 fork guard。
- 不声称公网 profile 能保护输入机密性。

## 安全不变量

1. Adapter 只能选择已注册 `ToolPolicyId`，不能定义权限。
2. Policy ID 从 immutable manifest 传到 product backend；未知 ID 在模型调用前固定失败为
   `unsupported_tool_policy`。
3. Profile 仅在 app 注册表映射成结构化权限，模型文本不能修改映射。
4. `allowed_tools` 同时用于 catalog 和 dispatch，deny/hook 再做第二道门。
5. eval ToolContext 必须满足 `trust_mode=false`、`trusted_external_paths=[]`、
   `workspace_follow_symlinks=false`。
6. 每个 task 使用独立 session execution root，read 工具不得解析到该根之外。
7. 两个 v1 profile 均禁用写入、shell、代码执行、子智能体和 MCP/dynamic tools。
8. Prompt、附件原路径/内容、工具参数和最终答案不得进入持久化或日志。

## Profiles

### `pinvou-gaia-public-web/v1`

仅适用于公开 GAIA 数据。允许 session workspace 只读和公网检索。搜索 query、请求 URL、
网页内容及图片可能发送到配置的外部 provider；入口和报告必须披露。

候选 exact allowlist：

```text
read_file
list_dir
grep_files
file_search
retrieve_tool_result
web_search
fetch_url
image_analyze
```

这只是待验证候选。发布前必须对真实产品 tool catalog 验证每个精确名称、能力和 alias，
并将 snapshot 测试作为 v1 权威清单。名字不存在时删除，不得换成更宽工具补偿。

### `pinvou-gaia-offline/v1`

适用于私人、客户或未公开数据，只允许 session workspace 本地只读：

```text
read_file
list_dir
grep_files
file_search
retrieve_tool_result
```

不得暴露 `web_search`、`fetch_url` 或远程 `image_analyze`。只有经验证完全本地执行的 vision
backend 才能加入 offline profile，否则图片分析固定返回能力错误。

### 公共拒绝面

Allowlist 默认拒绝未来新增工具，hook 对不在 profile 的工具再次拒绝。至少禁止：

- `exec_shell`、终端交互、进程/开发服务器等待；
- `code_execution`、`js_execution`；
- `write_file`、`append_file`、`edit_file`、`apply_patch`；
- agent/subagent/task 派生；
- MCP、connector、dynamic tools、`tool_search`；
- git、plan/goal/todo/review、automation、`request_user_input`、notify；
- `web_run` 等复合浏览/执行工具。

## Turn 权限投影

`TurnInput` 新增 app 内部 eval profile，默认 `None`。`None` 必须继续走现有 GUI 路径。
GAIA profile 使用 Agent mode，避免 Plan 行为提示，并结构化投影为：

```text
mode = Agent
allow_shell = false
trust_mode = false
auto_approve = false
approval_mode = Never
allowed_tools = profile exact allowlist
dynamic_tools = []
provenance = ImportedTranscript
trusted_external_paths = []
workspace_follow_symlinks = false
```

必须用集成测试确认 `ApprovalMode::Never` 不阻止无需批准的 allowlisted read 工具；若不满足，
app 层应建立显式安全工具自动执行策略，不得退回 Yolo/Bypass。

`ImportedTranscript` 是现有最接近“不可信导入数据”的 provenance，可避免继承 ExternalUser 的
standing auto authority。除非 app 层无法表达所需语义，不新增 fork provenance。

## ToolPolicyId 传递链

```text
ExecutionRequest::NativeTurn.tool_policy
  -> NativeAgentRunner
  -> agent-backend-api PrepareRequest
  -> ProductHeadlessBackend session policy binding
  -> EnginePoolPort::run_content
  -> TurnInput.eval_policy
  -> app registry -> structured turn authority
  -> EnginePool catalog + dispatch + ToolCallBefore hook
```

任一段丢失都固定失败，不得默认成产品全量工具。Policy ID 是安全 manifest 元数据；实际工具
配置只存在 app 注册表。prepare 后 policy 与 session 绑定，不能跨 task 替换或复用。

## Workspace、附件与 Office

- Product backend 将已解析附件复制到 per-session 临时 workspace，不持久化原路径。
- 拒绝目录、symlink/reparse point、危险文件名和超限文件。
- Office/PDF 使用 host `file_ingest` 在 `spawn_blocking` 中预处理；这不等于开放 Office/MCP
  或 shell 工具。
- Ingest 共享 task 单一 deadline，并受输入/输出大小和解析深度限制。
- 绝对外部路径、`..`、symlink/junction 逃逸和用户 trusted roots 均拒绝。
- cancel、timeout、prepare/run/close 错误均清理 session、staging 和中间产物。

## 写入策略

v1 最终答案通过 private output handle 在内存返回，无需模型写文件，故两个 profile 全禁通用
写工具。未来若需要产物，只新增 `write_eval_artifact` host tool：固定写入
`<session-root>/outputs`，拒绝 symlink/reparse，采用 create-new 或原子发布，限制文件名、
扩展、大小和数量，只返回 opaque handle。不得开放 shell、`write_file` 或 `apply_patch`。

## Prompt injection 与联网

Schema allowlist、dispatch 和 hook 能阻止附件/网页诱导 shell、home 读取、写入、派生 agent
或隐藏工具，但不能让“任意公网访问”和“输入机密性”同时成立。恶意输入可要求把内容编码进
`web_search` query 或公网 URL。

- Public-web 只能接收确认公开的数据，选择进入 manifest/report。
- 私密数据必须使用 offline。
- 私密数据若未来必须联网，应另建受控 egress broker（域名 allowlist、重定向复检、query
  redaction、请求/响应限制和审计），不能扩宽本 profile。
- `fetch_url` 现有 http(s)、localhost、RFC1918、link-local、metadata 和 redirect SSRF 防护
  必须在 eval 集成层复测。
- 限制 tool 次数、请求/响应大小、重定向、总耗时和 turn 次数。
- Observer 只记录规范化工具名、失败和耗时，不记录 query、URL、文件名或内容。

## 验收标准

1. GUI `profile=None` 的 Op/EngineConfig snapshot 与变更前一致。
2. 两 profile 的结构字段与经验证 exact catalog snapshot 一致。
3. 未知或 manifest 不匹配 policy 在模型调用前固定失败。
4. 伪造 tool call 被 catalog、dispatch 或 hook 拒绝且无副作用。
5. workspace 外路径及 symlink/junction 逃逸全部拒绝。
6. Public-web 可访问合规公网；offline 无网络 schema 且伪造网络调用失败。
7. 恶意 prompt/附件不能获得 shell、写入、外部文件、子智能体或动态工具权限。
8. 生命周期终止有界清理，且不泄漏 secret。
9. CodeWhale fork 无改动；若必须改，进入独立 fork 审查流程。

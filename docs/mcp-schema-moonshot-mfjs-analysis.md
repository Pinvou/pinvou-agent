# MCP 工具 schema 导致 Moonshot 请求整体失败 — 深度分析

> 日期：2026-08-21。现象：安装/启用企查查远程 MCP 后，所有会话对 Kimi（kimi-for-coding）路由发送失败，报
> `Moonshot function parameters failed safe compatibility validation: Moonshot function parameters contain an unsupported MFJS keyword`。
> 结论：**上游 CodeWhale 的结构性 bug，与「改动随对话回退」功能无关**（回退后 engine 重建只是恰好触发 MCP 重注册的时机）。

## 1. 根因链条

1. **MCP schema 绕过 provider 中立清洗层**
   - `CodeWhale/crates/tui/src/mcp.rs:2066` `discover_tools` 发 `tools/list`，逐条反序列化（`mcp.rs:2100`），**原样保留 `inputSchema`**；畸形条目仅 debug 跳过（`mcp.rs:2102-2108`，#1410）。`McpTool.input_schema`（`mcp.rs:993-998`）无任何规范化。
   - `mcp.rs:3389` `McpPool::to_api_tools` 直接 `input_schema.clone()` 进 `models::Tool`（`mcp.rs:3398`）——不过任何 sanitizer。
   - 经 `engine.rs:5233-5253` → `engine.rs:3839-3841` `build_model_tool_catalog_with_surface`（`tool_catalog.rs:200-221`，只做 deferral/预算/排序，不清洗）进入请求目录。
   - 对比：内置 registry 工具在 `registry.rs:259` 会过 `schema_sanitize::sanitize` + `schema_canonicalize`——**MCP 工具是清洗体系的结构性盲区**。
2. **撞上游最严的 MFJS 白名单校验**
   - 每步请求 `turn_loop.rs:815` 组装 `MessageRequest`（`tools: active_tools.clone()`，含企查查原样 schema）→ `client.rs:1760` `prepare_outbound_request` → `client.rs:1774` `build_chat_wire_body`（`chat.rs:626`）→ `chat.rs:663-667` 转 chat 形状，仅 provider=Moonshot 时调 `sanitize_moonshot_chat_tools`（`chat.rs:564`）。
   - 白名单（`schema_sanitize.rs:1461-1481`）仅 19 个关键字：`$id, $ref, $defs, anyOf, properties, additionalProperties, items, type, enum, required, maxLength, minLength, maximum, minimum, maxItems, minItems, title, description, default`。其他任何 key 一律 `UnsupportedKeyword`（`:1482-1487`）。
   - 第三方 MCP schema 常见但会被拒的：`$schema`（draft 声明，最可疑）、`pattern`、`format`、`exclusiveMinimum/Maximum`、`multipleOf`、`nullable`、`not`、`contains`、`propertyNames`、`if/then/else`、`$comment` 等。
   - 其他约束：`$defs` 只允许在 root；`$ref` sibling 仅限 `title/description`；anyOf ≤10 分支；root 必须是纯 `type:object` 无组合/`$ref`；属性 ≤100、enum ≤500 值、schema ≤120KB、对象深度 ≤5（`:1414-1421`）。
3. **全仓库唯一的 fail-closed**
   - 其他 provider 全部 fail-open：`sanitize_for_responses`（`schema_sanitize.rs:98`）返回 `Option<String>` 无 `Result`；xAI 复用它；Responses（`responses.rs:752`）、Anthropic（`anthropic.rs:95`）调用点拿不到错误，永不失败。
   - **只有 Moonshot 路径 fail-closed**：`chat.rs:576-581` 任一工具校验失败 → 整个请求报错，会话全灭。
   - 诊断文案刻意不含关键字/属性名（`schema_sanitize.rs:1100-1102`，防泄露 MCP 私有值），所以报错无法直接定位工具。

## 2. 定性：上游 bug，fork 未碰过

- `schema_sanitize.rs` 在上游 tag `v0.9.1` 已存在，`git diff v0.9.1..HEAD` 为空，fork 从未修改。
- fail-closed 调用点由上游提交 `0c8d55aac`（"fix(tui): sanitize Moonshot tool parameters per MFJS"，2026-07-20）引入，是 `v0.9.1` 的祖先。
- 未登记在父仓 `docs/fork-modifications.md` 四主题。
- **复现条件通用**：任意工具 schema 含白名单外关键字的 MCP server + Moonshot/Kimi 路由 = 全部会话发送失败。

## 3. 企查查侧事实

- manifest：`pinvou3-app/resources/mcp-servers/qcc/manifest.json`（安装在 `~/.pinvou3/bundle/mcp-servers/qcc/`）——`mcp_tools: []` 为空，只有远程 server `https://agent.qcc.com/mcp/company/stream`（OAuth）；schema 只能运行时 tools/list 现取，**本地无缓存副本**。
- `pinvou3-app/src-tauri/src/features/marketplace/mod.rs:916` 注释亦说明"远程 server 连接器可能没有静态 mcp_tools 列表（qcc 即如此）"。
- 要拿到实际违规关键字：dev 环境在 `mcp.rs:2100` 或 `chat.rs:672` 附近临时加日志抓 tools/list 返回（用完即弃），或在诊断中补充工具名（fork 改动）。

## 4. 错误发送的完整过程（按代码顺序）

1. **组装请求（engine 层，不碰 schema）**：`core/engine/turn_loop.rs:815` 组装 `MessageRequest`；`:824` `tools: active_tools.clone()` 携带全部工具目录（内置 + MCP），企查查 schema 原封不动；`:829` `tool_choice: "auto"`。
2. **进入 client 层**：`client.rs:1760` `prepare_outbound_request` 按路由选 wire 格式，Kimi 走 Chat Completions → `client.rs:1774` `build_chat_wire_body`（`chat.rs:626`）。
3. **Moonshot 专属校验（事故现场）**：`chat.rs:663-673` 把全部工具转 chat 形状后，仅 Moonshot 调 `sanitize_moonshot_chat_tools`。该函数（`chat.rs:564`）for 循环逐工具校验 `function.parameters`；企查查 schema 含白名单外关键字 → `Err(UnsupportedKeyword)` → `chat.rs:576-581` `map_err` 包装成 "failed safe compatibility validation" 后 `?` 抛出，**循环中断**。
4. **错误传播，整轮死亡**：`build_chat_wire_body` 返回 `Err` → `client.rs:1779` `?` 继续抛 → turn loop 收到错误，整个回合失败。**请求根本没发到网络层**——wire body 都没构造完。

后果被三个设计放大：

- **粒度错**：一个工具坏，惩罚整个请求；哪怕 50 个工具里 49 个好的、本轮根本不需要企查查，也全部发不出去。
- **每轮都炸**：工具目录每轮都带，会话之后每一轮都失败，表现为"会话全灭"。
- **无法定位**：报错刻意不含工具名/关键字名（防泄露 MCP 私有值），只有一句笼统的 "unsupported MFJS keyword"。

## 5. 为什么只有 Kimi 会这样

- **客户端预处理策略不同**：Moonshot 的 `sanitize_for_kimi_parameters` 返回 `Result` 且调用点 `?` 传播（fail-closed）；OpenAI Responses / xAI / Anthropic 的 sanitizer 返回 `Option`、原地改写 schema（fail-open），同样的企查查工具走这些路由请求照常发出。
- **服务端容忍度不同**：MFJS 是 Moonshot walle 的 JSON Schema 严格子集，服务端硬拒白名单外关键字；OpenAI/Anthropic 面对标准 JSON Schema，`$schema`/`pattern`/`format` 要么支持要么忽略。
- 上游加 Moonshot 预校验的初衷是好的（fail before transport，避免被服务端 400 拒），**bug 不在加了校验，而在校验粒度**：应是"哪个工具不兼容剔哪个"，实际写成"任一不兼容整轮报废"。

## 6. 为什么不能在应用层（pinvou3-app）清洗

1. **app 没有 schema 的写入路径**：企查查 schema 由 engine 自己连 MCP、自己 `tools/list`、自己存 `McpPool`；app 只在启动时传服务器地址，之后整条"工具目录 → MessageRequest → wire body"链路都在底座内部，没有 hook 能把清洗后的 schema 塞回去。
2. **app 连哪个工具有问题都不知道**：报错刻意不含工具名；app 自行定位需重建 MCP 客户端 + OAuth + 复制 MFJS 校验逻辑——为避开底座 ~30 行改动重建几百行重复设施。
3. **app 唯一的杠杆是 `SetDisallowedTools` 整工具禁用**：会把工具对所有 provider、所有会话永久关掉；而底座改粒度是"仅 Moonshot 路由本次请求临时剔除"，切 provider 或对方修 schema 即自动恢复。
4. **时机不对**：远程 server 可随时改 schema，启动期静态清洗覆盖不了运行中变化；请求路径上的降级每轮生效。

结论：这属于 fork 策略决策表中"必须在 Engine 生命周期原子完成、且所有 embedder 都受益"的改动——正确去向是上游 PR，fork 补丁是上游响应慢时的临时手段。

## 7. 修复方案评估

消费同一份工具目录的路径：`build_chat_wire_body`（`chat.rs:626`，唯一 Chat wire 构造点，阻塞/流式/preview-request 共用）、tool search haystack（`tool_catalog.rs:549-556`）、工具执行按名解析（`mcp.rs:3331`，不依赖 wire schema）、token 估算不算 tools。

| 方案 | 内容 | 评估 |
|---|---|---|
| A. 逐工具降级（治标，~30 行） | `sanitize_moonshot_chat_tools` 改为单工具失败 → 本次请求剔除 + warn + status 事件告知用户 | **必须项**。与上游 #1410（单个畸形 tools/list 条目跳过）和 strict 模式逐工具降级（`schema_sanitize.rs:50-62`）先例同构；preview 与实际发送天然一致。残留瑕疵：被剔工具仍在目录/tool search 里，模型可见但调不到；需注意剩余工具为空时的行为 |
| B. MCP 注册层过清洗（治本不充分） | `McpPool::to_api_tools` 或 engine 合并处让 MCP schema 过 registry 同款 `sanitize()` | 可选补齐。对齐内置工具行为、全 provider 受益；但 `sanitize()` 不删白名单外关键字，**单独做解决不了本题**；注册层 provider 中立，不能按 Moonshot 白名单剔除（误伤其他 provider） |
| C. A + B 组合 | — | 终态 |

## 8. 修复计划（方案 A，已建分支）

分支：CodeWhale `fix/moonshot-mcp-tool-degrade`（off `a36e6cd53` = origin/pinvou3-clean）；父仓 `fix/moonshot-mcp-schema-fail-open`（off origin/main）。

核心改动只在 `CodeWhale/crates/tui/src/client/chat.rs`：

1. `sanitize_moonshot_chat_tools`（:564）改为 `&mut Vec<Value>` 逐工具降级：通过的工具保持现有行为（原地清洗 + description 追加 note）；失败的工具从本次请求剔除，`tracing::warn!` 记录工具名 + 错误 variant（enum Display 按设计不含 schema 私有值；工具名 UI 可见不涉密）。返回被剔工具名列表。
2. 调用点（:663-707）处理空工具集：全部被剔除时不写 `body["tools"]`（:700），`tool_choice` 块（:702-707）同样以工具集非空为前提，避免空 `tools` 数组或 `tool_choice` 引用不存在工具导致新 400。
3. 已知残留瑕疵（接受）：被剔工具仍在目录/tool search haystack，模型可按名调用且执行路径不校验 wire schema；用户可见性仅 tracing 日志，UI 状态事件留后续/上游。

测试：

4. 改造两条不再成立的上游测试（`client.rs:4732` `assert_kimi_code_invalid_root_ref_fails_before_transport`、`:4779` `assert_kimi_code_untyped_default_fails_before_transport` 及 streaming/non-streaming 包装）：断言从"传输前整体失败"改为"请求正常发出、问题工具已从 wire body 剔除"，注释标注行为变更原因（fork-policy §3.4，不静默删除）。
5. 新增 `forkguard_moonshot_drops_only_incompatible_tool`：一个合法 + 一个含 `pattern` 的工具 → 请求到达 transport 且 wire body 只含合法工具；全部不兼容时 wire body 无 `tools`/`tool_choice`，请求仍发出。
6. 验证：`cargo test -p codewhale-tui --lib --locked`、`cargo test -p codewhale-tui --tests --locked`、`cargo clippy --workspace --all-targets --locked -- -D warnings`、`cargo fmt`。

父仓登记（fork-policy §3 同 PR 配套）：

7. `docs/fork-modifications.md`（及 `.en.md`）归入 T2 工具兼容与命令执行安全主题，更新内容、守护清单与 commit。
8. `scripts/fork-guard.sh` 新增 2 条指纹（降级函数特征串 + 新 forkguard 测试名）。
9. `./scripts/fork-guard.sh --fast` 通过；CodeWhale 侧 PR 合并发布后 bump 父仓 gitlink。

## 9. 建议行动

1. **临时**：工具商店停用企查查连接器即恢复。
2. **先提上游 issue**：附通用复现（"任意 schema 含 `$schema`/`pattern` 的 MCP server + Moonshot 路由 → 全请求失败"）+ 方案 A 建议；提之前可先抓企查查实际违规关键字（§3）提高 issue 质量。
3. **fork 临时补丁（可选）**：上游响应慢且企查查重要时，按方案 A 做 fork 补丁——属新增 fork-distinct 行为，需登记 `docs/fork-modifications.md` + fork-guard 指纹 + 行为测试（`docs/fork-policy.md` 流程），上游合入后退役。

## 10. 与「改动随对话回退」的关系澄清

本 bug 在时间线上出现在回退操作之后，但二者无因果关系：回退后的 engine 回收重建与全新会话 spawn 走同一条路径（`engine_pool.rs` get_or_spawn → `Op::SyncSession`），工具目录由 EngineConfig + MCP 连接决定，与是否回退无关。回退只是恰好触发了 engine 重建、使新安装的企查查工具首次进入请求。

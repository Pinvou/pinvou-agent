# present_artifact 落地方案(pinvou3)

> 状态:**已实现,待端到端验证(需 GUI+vLLM)** · 2026-06-02
> 已过:MCP server 协议单测 / cargo check / bundle 解包测试 / JS 语法 / 工具名格式代码核对。
> 端到端(agent 真调用 → 弹卡 → 点击打开 → 切会话不丢)需在跑起来的 GUI 里走 §5。
> 路线:**本地 MCP server(零 fork drift)** · 前端渲染成品卡 + 复用现成打开能力
> 诉求:pinvou2 会把"阶段性成品"以可点击卡片推到聊天区;pinvou3 现在产物只进右侧「产物与代码」抽屉,客户要多步操作才看得到。本方案在**保留产物面板**的同时,让 agent 主动 promote 的成品**直接弹聊天卡,点一下打开**。

---

## 1. 为什么选 MCP server(而非 fork 加工具)

用户决策:零 fork。下表是支撑这条路成立的**已验证事实**(均带代码证据,非假设):

| 关键问题 | 结论 | 证据 |
|---|---|---|
| MCP 工具会不会走前端能渲染的事件通道 | ✅ MCP 工具是 registry first-class citizen(`McpToolAdapter impl ToolSpec`),调用走和普通工具**完全一样**的 `chat:tool_start/tool_end` | `registry.rs:1052-1114` |
| 前端能否按工具名特判渲染成卡片 | ✅ 前端 `tool_end` 已按 name 分支(`request_user_input`/`careful_blocked`/`write_file` 各有特判),加一支即可 | `tauri-bridge.js:816/832/836` |
| 切会话会不会丢卡(memory 老坑) | ✅ **不丢**。present_artifact 是真工具调用,tool_use 天然进 messages;`rerenderFromMessages` 已按 `b.name` 还原专属卡,加一支同构 | `tauri-bridge.js:453/469` |
| 会不会每次弹审批(破坏非阻塞) | ✅ **不会**。`approval_requirement()` 默认只看 `ExecutesCode`/`WritesFiles`,present_artifact 都没有 → `Auto` 放行;且 pinvou3 前端还有 `ApprovalRequired` 自动放行兜底 | `spec.rs:630-639` · `engine.rs:384` |
| MCP 工具默认 defer、小模型看不到 | ✅ **当前不成立**。pinvou3 GUI 跑 **Yolo** 模式,`apply_mcp_tool_deferral` 在 Yolo 下 `defer=false` → MCP 工具全部可见 | `engine.rs:476` · `tool_catalog.rs:146-149` |
| 打开文件能力 | ✅ 全现成:`open_in_system`(图片/pdf)、`open_artifact_window`(html 独立窗,绕 snap 沙箱)、前端 `openArtifactExternal` | `commands.rs:532/559` |
| 接 MCP server 的位置 | ✅ pinvou3 已有 `bundle/mcp.json`(默认空 `{"servers":{}}`),改它加 server 条目即可 | `bridge/bundle.rs:61/134` · `paths.rs:75` |

**结论**:工具本体走 MCP,fork drift = 0;后端(Rust)零改动;工作量集中在「一个 MCP server 脚本 + 前端卡片」。

---

## 2. 改动清单(三层)

### 2.1 新建:MCP server(零依赖 python stdio)

`pinvou3-app/src-tauri/resources/common/bundle/mcp-servers/present_artifact_server.py`(新建,~80 行)

- 协议:stdio JSON-RPC(MCP `initialize` / `tools/list` / `tools/call`),只用 python stdlib,不引第三方 SDK → deb 分发只需 `python3`(对齐 memory:依赖型能力走 deb Depends)
- 暴露一个工具 `present_artifact`,input schema:
  ```json
  {"path": "string(必填,绝对路径或相对 workspace)",
   "title": "string(必填,客户一眼看懂的中文标题)",
   "description": "string(可选)"}
  ```
- `tools/call` 行为:验证 `path` 存在且是文件 → 返回 `{ok, kind, abs_path, basename, bytes}`(kind 按后缀:html/markdown/image/other)。**不做快照**(见 §4)。文件不存在 → 返回 error。
- 展示信息全部来自入参 args,server 只做"验证 + 回显",**不需要 session 上下文**。相对路径以 server 进程 cwd(= session workspace)解析;引导 agent 优先传绝对路径更稳。

### 2.2 注册:bundle/mcp.json

`~/.pinvou3/bundle/mcp.json`(运行时文件;默认内容在 `pinvou3-app/src-tauri/src/bridge/bundle.rs` 的 `DEFAULT_MCP_JSON`,从 `{"servers":{}}` → 加一条)

```json
{"servers": {
  "pinvou3": {
    "command": "python3",
    "args": ["<bundle>/mcp-servers/present_artifact_server.py"]
  }
}}
```
> **已实测**(从底座 `mcp.rs:2061` `all_tools()` 确认):透传工具名 = `mcp_{server}_{tool}` = **`mcp_pinvou3_present_artifact`**(server 名初版为 `pinvou`,后改 `pinvou3` 消除模型漂名)。instructions 引导名 + 前端匹配都按此全名;前端 `isPresentArtifactTool` 用 `endsWith("present_artifact")` 命中,改 server 名也不破。`bundle.rs` 的 DEFAULT_MCP_JSON 用 `{{PINVOU3_PRESENT_SERVER}}` 占位符,`ensure_extracted` 写出时替换成绝对路径。

### 2.3 前端:卡片渲染 + 还原(工作量大头,但都同构)

`pinvou3-app/src/tauri-bridge.js`:
1. `chat:tool_end` 监听里(~805 行)加分支:`name === "<实测名>" && success` → `addChatItem({type:"artifact_card", title, path, kind, description, ...})`(**不要**落进灰色 ToolCard)
2. `rerenderFromMessages`(~448 行)加分支:tool_use `b.name === "<实测名>"` → 还原同样的 `artifact_card`(切会话不丢)

`pinvou3-app/src/index.html`:
3. `ChatBubble` dispatcher(~1108 行)加 `if (item.type === 'artifact_card') return <ArtifactCard .../>`
4. 新增 `ArtifactCard` 组件(仿 `PlanCard`):图标(按 kind)+ 标题 + 描述 + 文件名 + "点击打开 →";点击 → `bridge.openArtifactExternal(path)`(已封装 html 走独立窗、其他走系统应用)
5. (可选)thinking label:`present → "正在展示作品…"`

### 2.4 prompt 引导

`pinvou3-app/src-tauri/resources/common/bundle/instructions.md` 加一小段:产出 html/markdown/图片等**给客户看的成品**后,立刻调 `present_artifact`(传 path + 中文 title);中间文件/配置/脚本**不要**调。

---

## 3. 数据流

```
Pinvou 产出成品.html → 调 present_artifact{path,title}
  ↓ 底座 registry → McpToolAdapter.execute → python server 验证文件
  ↓ 底座 emit chat:tool_end {name:"present_artifact", args:{path,title,...}, success:true}
  ↓ engine.rs spawn_event_forwarder → app.emit
前端 tauri-bridge.js: tool_end 特判 name → addChatItem(artifact_card)
  ↓ index.html ChatBubble → ArtifactCard 渲染聊天卡
  ↓ 用户点击 → bridge.openArtifactExternal(path)
  ↓ commands.rs: open_artifact_window(html) / open_in_system(其他)
切会话 → rerenderFromMessages 按 tool_use.name 还原同卡(不丢)
```

`write_file` 产物仍照旧进右侧产物面板(`artifact:disk` + trackArtifact),两条路并存,互不影响。

---

## 3.5 自动续卡(2026-06-02 反馈修复)

**问题**:第一次产出弹卡正常,第二次"迭代/重写"同一成品后**不弹卡**。根因实测(session `tuxlngzueyid0.json`):`write_file` 调 2 次,`present_artifact` 只调 1 次 —— **Qwen3.6 把第二次当"修 bug"没重新 present**,非前端 bug。依赖小模型主动调工具的固有弱点。

**修复(确定性兜底,不赌模型遵循率)**:已 present 过的成品文件(按 basename 识别),后续 `write_file`/`append_file` 成功 → 前端**自动续弹一张新成品卡**(每次新卡,对齐 pinvou2),复用首次的 title/description/path。

- 实现:`findPresentedArtifact(path)` 扫 `state.chatItems` 找同名成品卡 —— present 信息已活在 chatItems 里,**零额外 per-session 状态**(chatItems 已按 session 隔离 + rerender 重建)。
- 实时(`tool_end` 的 write_file 分支)+ 重建(`rerenderFromMessages` 的 write_file 还原)两处对齐,切会话不丢。
- prompt 也加"迭代/修复后重写成品也再调一次"作辅助双保险。
- 边界:只对**已 present 过**的文件自动续(零误报);首次产出若 agent 就漏 present,自动续救不了(集合空)—— 靠 prompt 兜首次。

## 4. 已知约束 / v1 不做

- **依赖 Yolo 模式**:MCP 工具可见的前提是 pinvou3 GUI 跑 Yolo(当前如此)。若将来切非 Yolo(Plan/Agent),MCP 工具会被 defer,agent 看不到 → 那时需让 agent 先 tool_search,或改 `should_keep_mcp_tool_loaded`(那才动 fork)。**当前不是问题,但属于约束依赖,改运行模式时要回看这里。**
- **v1 不做快照**:pinvou2 拷贝快照防"后续改同名文件污染历史卡"。pinvou3 产物在 session workspace、file_watcher 已追踪,覆盖风险低 → 先验证主链路,快照留 v2。
- **工具名以实测为准**:底座 MCP adapter 透传的工具名(有无前缀)实施第一步实测确认,前端特判 + prompt 引导统一用实测名。
- **markdown 在独立窗显纯文本**:同 pinvou2 v1 限制,要漂亮渲染留后续。

---

## 5. 实施顺序 + 验收

1. 写 `present_artifact_server.py` + 单测(喂存在/不存在文件,验返回结构)
2. 改 mcp.json + bump base 指纹 → 起 app,**日志确认透传工具名**
3. 前端 4 处改动(tool_end 分支 / ArtifactCard / ChatBubble / rerender 还原)
4. instructions.md 加引导
5. 端到端:对 Pinvou 说"做个 SpaceX 单页总结 html" → 聊天区弹成品卡 → 点击 → 独立窗渲染 → **切会话回来卡还在**
6. 反向回归:让 agent 只跑只读命令/写中间文件 → **不弹卡**

---

## 6. 改动文件汇总

| 文件 | 类型 | fork? |
|---|---|---|
| `resources/common/bundle/mcp-servers/present_artifact_server.py` | 新建 ~80 行 | 否 |
| `~/.pinvou3/bundle/mcp.json`(默认在 `bundle.rs` DEFAULT_MCP_JSON) | 改默认 | 否 |
| `resources/common/bundle/instructions.md` | 加引导段 | 否 |
| `src/tauri-bridge.js` | tool_end 分支 + rerender 还原 | 否 |
| `src/index.html` | ArtifactCard 组件 + dispatcher | 否 |
| `src-tauri/src/bridge/bundle.rs` | mcp.json 默认写入逻辑 + base 指纹 bump | 否(app 层) |

**DeepSeek-TUI submodule:零改动。fork drift 增量 = 0。**

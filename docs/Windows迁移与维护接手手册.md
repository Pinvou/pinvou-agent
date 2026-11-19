# pinvou3 Windows 迁移与维护接手手册

> 面向：刚接触本项目、需要把项目迁移到 Windows 并长期迭代维护的 Windows 应用开发工程师。
>
> 同期姊妹篇：[`Windows端功能清单与测试用例.md`](./Windows端功能清单与测试用例.md)（2026-07-23 功能盘点与验收用例快照）。
>
> 生成日期：2026-06-15。本文基于当时仓库源码、`README.md`、`process.md`（内部协作文档，未随本仓库发布，见 §16 注）、[`docs/fork-modifications.md`](./fork-modifications.md)、[`docs/auto-compact-256K-tuning.md`](./auto-compact-256K-tuning.md)、[`docs/DeepSeek-TUI-工具系统.md`](./DeepSeek-TUI-工具系统.md) 与相关 Tauri/Rust/前端代码梳理。
>
> **注（2026-11 回填）**：本文为 2026-06-15 迁移期快照，未按当前主干逐条复核，接手时注意以下布局均已变化：底座子模块已由 `DeepSeek-TUI/` 更名 `CodeWhale/`（无 `branch` 字段，另新增 `private-runtimes/windows` 私有运行时子模块）；前端由 `tauri-bridge.js` 单文件迁移为 `src/features/*` + `src/platform/` 分层；Rust 由根级 `commands.rs`/`engine.rs`/`bridge/` 迁移为 `app/commands/` + `features/` + `platform/`；§8 的工作流子系统（`workflows/`、`harness.rs`、`scheduler.py`）已在主干整体移除（正文其余散落的 workflow 提及——§1/§2/§3/§4/§5/§6/§9/§12/§13/§14/§15/§18 等——同为快照口径，不随本注逐条列举）；§11「Windows 打包必须重做」的结论已过时——NSIS 打包已在主干落地。现行权威入口：`AGENTS.md`、`docs/fork-modifications.md`、`docs/windows-private-runtime-submodule.md`、`docs/windows-rust-ci.md`。

## 1. 一句话定位

pinvou3 是一个本地算力桌面智能助手：`pinvou3-app/` 提供 Tauri 2.0 桌面壳、前端交互、配置桥接、session/workflow 编排；`DeepSeek-TUI/` 是底座 submodule，负责 LLM 调用、工具循环、流式 SSE、session、skill、MCP、hooks、compaction 等核心 agent 能力；本地 vLLM 上的 Qwen3.6-35B-A3B-FP8 是默认推理后端。（2026-11 注：现为平台感知默认预设 —— Windows/macOS 首启默认 DeepSeek 远程 API，Linux 保持 LocalVllm，见 `platform/prefs/model.rs`。）

最重要的边界：不要在 pinvou3 里重写 DeepSeek-TUI 已有的 Engine、ToolRegistry、SSE、Session、SkillRegistry、Commands 路由、MCP client、Hooks、Cycle、Compaction。新增领域能力优先走 `SKILL.md`、`~/.deepseek/commands/*.md`（2026-11 注：底座现以 `<workspace>/.codewhale/commands/`（项目级）与 `~/.codewhale/commands/`（用户级）为主，`.deepseek/commands` 仅 legacy 回退，见底座 `commands/user_commands.rs`）、独立 MCP server 或 pinvou3-app 的 UI/Rust wrapper。

维护判断口诀：凡是“让 AI 如何思考/调用工具/管理上下文/执行工具循环”的问题，先查 DeepSeek-TUI；凡是“桌面 UI 如何呈现、配置如何落地、session/workflow 如何接到 UI”的问题，再查 `pinvou3-app/`。

## 2. 当前仓库结构

（2026-11 注：本节表格为快照期 0.6.x 布局，路径与「当前」措辞按当时口径陈述；现行布局为 `app/` + `features/` + `platform/`，Windows 打包已上线——平台配置含 `msi`+`nsis` 目标，发布 CI 实际产出 nsis 安装包。）

| 路径 | 作用 | Windows 接手要点 |
|---|---|---|
| `pinvou3-app/` | 主应用，Tauri 2.0 + Rust 后端 + 静态前端 | 迁移主战场；当前打包目标主要是 Linux `.deb`（⚠️ 快照条目：现行 Windows 打包已上线，见本节首注；配置实况见 `pinvou3-app/src-tauri/config/platforms/windows/tauri.conf.json`，§11 结论已纠正） |
| `pinvou3-app/src/` | 前端入口，`index.html` + `tauri-bridge.js` + vendor JS | `tauri-bridge.js` 是前后端通信唯一集中封装 |
| `pinvou3-app/src-tauri/` | Rust 后端、Tauri 配置、bundle 资源 | Windows 兼容主要改这里和打包配置 |
| `DeepSeek-TUI/` | 底座 submodule，Cargo path dependency | 当前工作区该目录可能为空；必须先初始化 submodule |
| `docs/` | 项目设计、阶段报告、fork 说明、测试方案 | 接手时要先读 `fork-modifications.md` 和 `process.md` |
| `workflows/` | 开发源工作流数据与 Python 调度脚本（快照条目：已在主干整体移除） | `build.rs` 同步到 Tauri bundle 快照（该同步已不存在） |
| `scripts/` | 辅助脚本，包括 fork guard、模拟脚本、release 脚本 | 多数脚本偏 Bash/Linux，需要 Windows 适配 |

接手时优先确认这 8 个核心模块：`pinvou3-app/src/tauri-bridge.js`、`commands.rs`、`engine_pool.rs`、`engine.rs`、`bridge/mod.rs`、`file_ingest.rs`、`harness.rs`、`DeepSeek-TUI/crates/tui`。前 7 个在 app 层，最后一个是底座依赖。（2026-11 注：`harness.rs` 属已移除的工作流子系统，实际待确认模块以现行 `app/commands/` + `features/` 布局为准。）

快照期 `.gitmodules` 指向（⚠️ 快照条目：现行 `.gitmodules` 见下方块注）：

```ini
[submodule "DeepSeek-TUI"]
	path = DeepSeek-TUI
	url = https://github.com/Pinvou/CodeWhale.git
	branch = pinvou3-clean
```

> 当前主干实际为：`[submodule "CodeWhale"]`（`path = CodeWhale`，无 `branch` 字段）+ `[submodule "private-runtimes/windows"]`（Windows 私有运行时，`update = none`；快照块 `branch` 字段指向内部仓分支，同样不随本仓库发布；快照块 `url` 字段亦已由内部 fork 地址改写为公开镜像，非快照原文）。

首次克隆或当前目录为空时先执行：

```bash
git submodule update --init --recursive
```

否则 `pinvou3-app/src-tauri/Cargo.toml` 中的 path dependency `../../DeepSeek-TUI/crates/tui` 会找不到。

## 3. 主调用流程：一次聊天从前端到模型

主链路如下：

```text
用户输入
  ↓
pinvou3-app/src/index.html
  ↓
pinvou3-app/src/tauri-bridge.js
  ↓ invoke("chat", { message, attachments, sessionId })
pinvou3-app/src-tauri/src/commands.rs::chat
  ↓
附件拼接 / session mode / persona / skill pending instruction
  ↓
EnginePool::send_user_message(session_id, content, mode, phase)
  ↓
EnginePool lazy spawn 或复用 AppEngine
  ↓
AppEngine::send_user_message
  ↓
Pinvou3Bridge::build_send_message_op
  ↓
DeepSeek-TUI EngineHandle.send(Op::SendMessage)
  ↓
DeepSeek-TUI turn loop / tool dispatch / OpenAI-compatible vLLM
  ↓
DeepSeek-TUI Event
  ↓
engine.rs::spawn_event_forwarder
  ↓ app.emit("chat:*")
tauri-bridge.js listen("chat:*")
  ↓
前端更新消息流、工具卡、产物卡、token 状态、session 持久化
```

关键文件：

- `pinvou3-app/src/tauri-bridge.js`：所有 `invoke` 和 `listen` 的集中桥。
- `pinvou3-app/src-tauri/src/commands.rs`：Tauri commands，含 `chat`、session、artifact、workflow、settings、update、dependency 等入口。
- `pinvou3-app/src-tauri/src/engine_pool.rs`：每个 session 一个独立 Engine，lazy spawn，后台 session 可继续跑。
- `pinvou3-app/src-tauri/src/engine.rs`：`AppEngine` 包装 DeepSeek-TUI `EngineHandle`，并把底座事件转成 Tauri 事件。
- `pinvou3-app/src-tauri/src/bridge/mod.rs`：把用户设置、bundle、模型配置、工具策略、prompt 注入翻译成 `EngineConfig` / 底座配置。

维护要点：

- `commands.rs::chat` 是前端消息进入后端的主入口，但不应在这里写新的 agent loop。
- `EnginePool` 负责 per-session engine 生命周期，避免多会话串台。
- `engine.rs::spawn_event_forwarder` 是底座事件进入 UI 的关键翻译层，新增 UI 状态时优先查这里和 `tauri-bridge.js` 的对应 `listen`。
- DeepSeek-TUI 负责工具执行和 LLM 循环；如果需求是新增领域能力，先考虑 skill、command 或 MCP。

## 4. 前端通信与事件

前端不是多页面应用，而是一个静态前端加 Tauri bridge：

- `index.html` 承载 UI、i18n 文案和组件逻辑。
- `tauri-bridge.js` 管理全局 state、session buffer、消息流、附件、工作流、更新、依赖体检、卡牌池等。
- 前端通过 `window.__TAURI__.core.invoke` 调后端命令。
- 后端通过 `app.emit` 推送事件。

主要事件：

| 事件 | 来源 | 用途 |
|---|---|---|
| `chat:delta` | DeepSeek-TUI message delta | 流式 assistant 文本 |
| `chat:tool_start` / `chat:tool_end` | 工具调用事件 | 渲染工具卡、产物卡、dirty artifact |
| `chat:done` | `TurnComplete` | 收尾、持久化、清 busy、补产物卡 |
| `chat:usage` | `TurnComplete.usage` | token 使用量 |
| `chat:compaction` | compaction 事件 | 上下文压缩提示 |
| `chat:user_input_required` | request_user_input 工具 | 渲染用户选择气泡 |
| `chat:plan_ready` | Plan 模式方案产出 | 渲染方案卡 |
| `artifact:disk` | file watcher | 监听 session workspace 新产物 |
| `workflow:*` | harness/workflow | 工作流看板、角色状态、gate、fanout（快照条目：工作流子系统已移除） |
| `update:progress` | updater | 应用内升级下载进度 |

多 session 并发是前端和后端一起保证的：后端每个 Engine 事件都带 `session_id`，前端 `onSessionEvent` 按 session buffer 分流，避免后台会话的 token 流污染当前会话。

## 5. 后端启动流程

`pinvou3-app/src-tauri/src/main.rs` 只负责调用 `pinvou3_lib::run()`。

`lib.rs::run()` 做这些事：

1. `ensure_release_env()` 注入 HTTP/SSE timeout 和平台 UI 等运行时默认值；provider 由当前模型预设决定，输出上限也不再通过全局环境变量注入。
2. 初始化 Tauri builder 和插件。
3. 执行工作流迁移 `workflow_migrate::migrate_if_needed()`。（快照注：工作流子系统已移除，该迁移步骤已不存在。）
4. 启动 `SessionStore::boot()`，加载 `~/.pinvou3/sessions/`。
5. 初始化 `EnginePool::new()`，但不立即 spawn engine。
6. 初始化 `MonitorState`。
7. 启动 file watcher，监听 session 目录下新产物。
8. 注册大量 Tauri commands。

Windows 迁移时注意：`ensure_release_env()` 里有 Linux GUI 变量，如 `GDK_BACKEND`、`WEBKIT_DISABLE_COMPOSITING_MODE`、`GTK_IM_MODULE`，Windows 下应按平台条件拆分。（📌 2026-11 注：已落地——Linux UI 变量现按平台收于 `platform/ui_cache.rs` 的 `LINUX_UI_ENV_DEFAULTS` 与 `cfg(target_os = "linux")` 门，Windows 构建不再携带。）

## 6. 配置、目录与持久化

pinvou3 不直接使用 `~/.deepseek/` 作为主数据目录，而是隔离到 `~/.pinvou3/`。路径定义在 `bridge/paths.rs`（⚠️ 快照平铺路径：现居 `platform/paths.rs`，见 §12/§17 注）。

| 目录或文件 | 用途 |
|---|---|
| `~/.pinvou3/settings.json` | 用户设置、模型 preset、本地/远端 API 配置 |
| `~/.pinvou3/bundle/` | 内嵌 bundle 解包后的 prompt、workflow、MCP server 等（快照条目：现行 bundle 为说明、内置技能、MCP 配置、personas 等，无 workflow 内容） |
| `~/.pinvou3/user/skills/` | pinvou3 私有用户 skills |
| `~/.deepseek/skills/` | DeepSeek-TUI 标准用户 skills，工作流视图会合并展示（快照条目：底座现以 `.codewhale` 为主目录、`.deepseek` 仅 legacy fallback，工作流视图已移除） |
| `~/.pinvou3/user/personas/` | 用户自创专家卡 |
| `~/.pinvou3/sessions/<id>.json` | session 元数据和 messages |
| `~/.pinvou3/sessions/<id>/workspace/` | 每个 session 独立工作目录 |
| `~/.pinvou3/sessions/<id>/artifacts/` | AI 产物默认落地目录 |
| `~/.pinvou3/workflows/<run_id>/project/` | 工作流 run 的项目目录（快照条目：工作流子系统已在主干整体移除，此目录不再产生） |
| `~/.pinvou3/updates/` | Linux `.deb` 更新包暂存目录 |

Windows 风险：代码多处默认读 `HOME`，Windows 原生通常是 `USERPROFILE`。Rust 标准库和 Tauri 有跨平台目录 API，迁移时建议集中改 `paths.rs`，不要在各模块散落平台判断。

数据目录维护要点：

- session JSON 与 artifacts 是用户价值数据，迁移/卸载/升级都应默认保留。
- bundle 是可再生成资源，版本变化时可重解包，但不能覆盖 `~/.pinvou3/user/` 下的用户自定义内容。
- workflow run 目录与 session 目录不同，不要把工作流项目误清理成普通聊天产物。（快照条目：工作流子系统已移除，现无 workflow run 目录。）

## 7. 附件 ingestion 流程

入口在前端：

```text
选择/拖拽/粘贴文件
  ↓
tauri-bridge.js addAttachmentByPath / addPasteImage
  ↓ invoke("ingest_file") 或 invoke("save_paste_image")
  ↓
file_ingest.rs::ingest
  ↓
返回 IngestResult
  ↓
commands.rs::build_message_with_attachments
  ↓
把小附件 markdown 直接嵌入 user message；大附件/图片转路径模式
```

支持类型：

- 文本：直接读。
- PDF：`pdftotext -layout`，扫描件走 OCR。
- docx/odt：`pandoc -t markdown`。
- doc/rtf/wps：LibreOffice headless 转文本。
- ppt/pptx/odp/dps：LibreOffice 转 PDF 再 `pdftotext`。
- xls/xlsx/ods/et：LibreOffice 转 CSV。
- 图片：登记元数据，真正理解交给底座 `image_analyze`。
- zip/rar/7z：`7z` 解压后递归识别。
- eml/msg：python 标准库或 `msgconvert`。

Windows 风险集中在这里：

| 当前实现 | Windows 问题 | 建议 |
|---|---|---|
| `which("pandoc")` 等 | Windows 没有 `which` | 改为跨平台探测：Windows 用 `where.exe` 或 Rust path lookup |
| `python3` | Windows 常叫 `python` 或 `py` | 同时探测 `python3`、`python`、`py` |
| `soffice` | Windows 可能是 `soffice.exe`，路径带空格 | 允许配置 LibreOffice 路径，或搜索常见安装目录 |
| `pdftotext` / `pdftoppm` | 需 Poppler for Windows | 文档和 UI 需提示 Windows 安装来源 |
| `tesseract` | 需 Tesseract Windows 包和语言数据 | 语言包路径可能要配置 |
| `7z` | Windows 常是 `7z.exe` | 支持 PATH 和常见安装目录 |
| `pkexec apt install` | Linux-only | Windows 不能一键 apt；改成依赖检查 + 安装指引，或 winget/choco 可选 |

> ⚠️ 快照口径（2026-11 注）：本表「当前实现」列为 2026-06 快照状态，表中建议主干均已落地——外部工具探测已收敛为跨平台 helper（`platform::os::{pdf_tool_path,pandoc_tool_path,ocr_tool_path,archive_tool_path,libreoffice_tool_path}`，`features/files/ingest_deps.rs`；`python_command()` 同经 `platform::os`），裸 `which(` 在 `features/files/` 零命中；依赖指引/安装已按平台拆分（`features/dependencies/platform/`，Windows 走 winget 探测 + UAC 安装文案）；Poppler/Tesseract/Pandoc/7-Zip 现随 Windows 安装包携带（见 [`windows-private-runtime-submodule.md`](./windows-private-runtime-submodule.md)），`.msg` 已有 Windows 原生 Rust 解析。本表仅作迁移决策史料。

## 8. 工作流链路

> ⚠️ 快照章节：本节描述的工作流子系统（`workflows/`、`harness.rs`、`scheduler.py`、`build.rs::sync_workflow_bundle`、`Op::SpawnSubAgent` 链路）已在当前主干整体移除（`sansheng-liubu` 已列入 `features/retirement.rs` 退役清单），仅作历史留档，请勿据此接手。

工作流当前处于“功能重新设计中，GUI 开发中占位”的大背景，但代码里已有卡片流/三省六部工作流框架。

链路：

```text
前端 startWorkflowTask
  ↓ invoke("start_workflow")
commands.rs 创建 workflow run / project
  ↓
invoke("kick_workflow")
  ↓
harness.rs::step_fresh
  ↓
scheduler.py 决策下一节点
  ↓
engine.rs::apply_harness_action
  ↓
Op::SpawnSubAgent
  ↓
DeepSeek-TUI SubAgent 执行
  ↓
Event::AgentComplete / TokenUsage / request_user_input
  ↓
harness 推进 gate / rollback / fanout / all_done
  ↓
workflow:* 事件推前端看板
```

数据源：

- 开发源：`workflows/sansheng-liubu/`
- 引擎脚本：`workflows/_engine/scripts/`
- 打包快照：`pinvou3-app/src-tauri/resources/common/bundle/workflow/sansheng-liubu/`
- 同步逻辑：`pinvou3-app/src-tauri/build.rs::sync_workflow_bundle`

Windows 风险：

- 工作流脚本是 Python，路径、换行、可执行方式要按 Windows 验证。
- `build.rs` 使用相对路径同步，Windows 下路径分隔符由 Rust `Path` 处理，原则上可行，但需要实际跑 `cargo build`。
- 工作流 prompt 里可能注入绝对路径，Windows 盘符和反斜杠要验证模型是否能稳定使用。

## 9. DeepSeek-TUI fork 依赖与维护红线

`docs/fork-modifications.md` 是 fork 单一真相源。当前关键点：

- submodule 分支应是 `pinvou3-clean`。（快照口径；当前主干 gitlink 不固定分支。）
- fork drift 有明确统计和 fork guard 守护。
- pinvou3 通过 Cargo path dependency 使用底座 crate：`deepseek-tui = { path = "../../DeepSeek-TUI/crates/tui", package = "codewhale-tui" }`（当前主干 path 为 `../../CodeWhale/crates/tui`）。
- 上游 v0.8.41 起 rebrand 为 `codewhale-tui`，本地 alias 保持 `deepseek-tui`（当前 path 为 `../../CodeWhale/crates/tui`）。

必须保护的 fork 主题：

| 主题 | 为什么重要 |
|---|---|
| library facade | pinvou3-app 需要把底座作为库调用 |
| tool blocklist | Qwen3.6 可见工具需精简，且要防 tool_search 绕过（⚠️ 快照条目：blocklist 已于 2026-08-11 退役，现行以 `PINVOU3_ALLOWED_TOOLS` 正向白名单治理，见 `docs/tool-governance.md`） |
| append_file / 大产物保护 | 本地慢 vLLM 和大文件生成依赖分块写 |
| YOLO careful hook | 危险命令在 YOLO 下也要拦 |
| project_context / skills 路径收窄 | GUI 助手不应乱读其他 AI 工具上下文 |
| static prompt composer | pinvou3 用它密封系统 prompt，防上游新增块泄漏 |
| 256K context 适配 | 本地 Qwen3.6 256K 的 compaction、output budget、模型名后缀依赖这些改动 |
| 工作流 SubAgent 扩展 | role_id、allowed_tools、max_steps、structured output 等工作流能力依赖它（⚠️ 快照条目：工作流子系统已从主干移除，见 §8/§12/§13） |

同步上游后不要只看能不能编译，至少做：

```bash
./scripts/fork-guard.sh --fast
./scripts/fork-guard.sh
cargo test -p codewhale-tui --lib
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib -- --test-threads=1
```

还要人工做：

- dump system prompt 前后 diff，非 0 diff 要查谁漏进静态 prompt。
- 扫 `turn_loop.rs` / `engine.rs` 是否有新的 per-turn transient message。
- 盘点新工具和新激活机制，尤其是能激活 deferred tools 的路径。

## 10. 本地模型与 256K 约束

默认模型名是 `qwen36_35b_256k`。`_256k` 后缀不是装饰，它让底座派生 256K context window。（2026-11 注：该名是 LocalVllm 预设；Windows/macOS 首启默认预设为 DeepSeek 远程 API，见 `platform/prefs/model.rs`。本节描述本地 vLLM 场景。）

vLLM 需要用匹配的 served model name：

```bash
vllm serve /path/to/model \
  --served-model-name qwen36_35b_256k \
  --max-model-len 262144
```

关键默认值：

| 项 | 值 |
|---|---|
| `DEEPSEEK_BASE_URL` 默认 | `http://127.0.0.1:8000/v1` |
| `DEEPSEEK_PROVIDER` | 不全局注入，由当前模型预设决定 |
| `DEEPSEEK_REASONING_EFFORT` | `off`（⚠️ 快照条目：现行主干不注入也不读取该变量，reasoning effort 已改为按模型偏好 `saved_models[].reasoning_effort`，此行仅作快照留档） |
| 输出上限 | 本地 vLLM route 显式为 `24576`（⚠️ 快照条目：现行 route 输出声明由 `route_limits_for_model` 按窗口档位给出——≥500K→131072、≥250K→65536、否则 min(窗口/4, 32768)，未探测本地 vLLM 落 128K 回退→32000；24576 仅存于 `PINVOU3_MAX_OUTPUT_TOKENS`/prefs 回退，用于 min-clamp 显式 `max_output_tokens` 与压缩阈值推导）；云端由底座请求上限、厂商及 route 能力共同决定 |
| compaction token threshold | 约 `190000`（⚠️ 快照条目：现由 `derive_compaction_threshold` 动态推导，见 [`auto-compact-256K-tuning.md`](./auto-compact-256K-tuning.md)） |
| emergency input budget | 约 `230400`（⚠️ 快照条目：同上，以现行代码为准） |

Windows 迁移建议：

- 本机 Windows 跑 vLLM 可能不现实，GB10/Linux 远程服务更可行。
- UI 设置页已经支持自定义 `custom_base_url` / `custom_model_name`。
- 确认 Windows 防火墙、代理、公司网络不会拦截本地/内网 HTTP。
- 如果模型名不含 `_256k`，要明确接受 context 退化或增加显式 context window 配置。

## 11. 安装、更新、打包现状

当前 `tauri.conf.json`：

```json
"bundle": {
  "active": true,
  "targets": ["deb"]
}
```

> 快照口径：当前主干已落地 Windows 打包——`pinvou3-app/src-tauri/config/platforms/windows/tauri.conf.json` 已含 `"targets": ["msi","nsis"]`（perMachine、LZMA、VC++ 检测与安装钩子齐备；发布 CI 实际产出 nsis 安装包，msi 目标暂仅配置与本地构建）；底座 `tauri.conf.json` 不再写 `targets`，按平台目录拆分。本节「必须重做」清单仅对 0.6.x 状态成立，现行契约见 `docs/windows-private-runtime-submodule.md`。

Linux 安装说明在 `pinvou3-app/INSTALL.md`，依赖 `.deb`、`apt`。应用内升级也围绕 `.deb` 下载、sha256 校验、`pkexec apt` 安装（2026-11-18 订正：`pkexec` 流程属应用内升级器 `features/updater`，INSTALL.md 本文不含 `pkexec`）。

Windows 迁移必须重做或拆分：

| 能力 | 当前 | Windows 方向 |
|---|---|---|
| 打包目标 | `.deb` | `msi` / `nsis` / `appx` 之一 |
| 安装依赖 | `apt` | winget/choco/manual 指引，或内置检测不自动装 |
| 提权安装 | `pkexec` | UAC / installer 权限 |
| 更新包 | `.deb` | Windows installer 包和签名 |
| 打开文件/文件夹 | Linux 命令可能存在 | 使用 Tauri shell/open 或 Windows `explorer` |
| WebView | Linux WebKitGTK | Windows WebView2 |

不要直接把 Linux 的依赖安装体验照搬到 Windows。Windows 版更适合先做“依赖检测 + 明确安装指引 + 设置页路径配置”。

## 12. Windows 迁移风险清单

| 风险 | 涉及模块 | 建议优先级 |
|---|---|---|
| `DeepSeek-TUI/` 子模块未初始化导致 Cargo path 依赖断 | Git / Cargo | P0 |
| Tauri bundle 只配了 `deb` | `tauri.conf.json` | P0（⚠️ 快照条目：现行 Windows 打包已上线——配置含 `msi`+`nsis`，发布 CI 实际产出 nsis，见 §11 注） |
| `HOME` 环境变量假设 | `bridge/paths.rs`、`file_ingest.rs` | P0（⚠️ 快照条目：已落地——现行 Windows 读 `USERPROFILE`，用户目录解析统一经 `platform::os`（`windows_path.rs`）） |
| `which` 命令探测依赖 | `file_ingest.rs` | P0（⚠️ 快照条目：已落地跨平台探测 helper，见 §7 注） |
| `pkexec` / `apt` 一键安装依赖 | `file_ingest.rs`、`updater.rs` | P0（⚠️ 快照条目：已拆平台策略 `features/dependencies/platform/`，Windows 走 winget/安装指引；updater 已拆 `features/updater/platform/`（现行仅 `unsupported` 占位，见 §17）） |
| `.deb` 应用内升级 | `updater.rs`、前端设置页（⚠️ 快照平铺路径：现居 `app/commands/updater.rs` + `features/updater/platform/`） | P0 |
| Linux GTK/WebKit env | `lib.rs::ensure_release_env` | P1（⚠️ 快照后已消解：Linux UI 变量按平台拆分至 `platform/ui_cache.rs`，见 §5 注） |
| Bash 脚本启动 | `run-dev.sh`（现居 `pinvou3-app/` 下）、`scripts/*.sh` | P1 |
| Poppler/Tesseract/LibreOffice/7z 路径 | `file_ingest.rs` | P1（⚠️ 快照后已消解：`platform::os::*_tool_path` 跨平台探测 + 安装包内置运行时，见 §7 注） |
| Python 命令名 `python3` | `file_ingest.rs`、workflow scripts | P1（⚠️ 快照后已消解：`platform::os::python_command` 跨平台命令名） |
| 工作流绝对路径注入 | `harness.rs`、workflow scripts | P1（快照条目：`harness.rs` 与工作流子系统已在主干移除，此风险随之消失） |
| 打开文件/目录命令 | `commands.rs` | P1 |
| vLLM 默认本机地址 | `bridge/mod.rs`、settings | P1（⚠️ 快照后已消解：Windows/macOS 首启默认预设已改远端 DeepSeek，`platform/prefs/model.rs`） |
| Windows 路径反斜杠与模型提示 | prompt / workflow | P2 |
| fork guard Bash 脚本 | `scripts/fork-guard.sh` | P2 |

风险表验收口径：P0 是“Windows 首次运行或打包会直接失败”的阻塞项；P1 是“功能可启动但关键能力缺失或不稳定”的迁移项；P2 是“体验、便利性或长期维护风险”。

## 13. 常见维护任务该改哪里

| 想做的事 | 首选入口 | 不建议 |
|---|---|---|
| 新增领域 agent / 工具组合 | `SKILL.md`，放用户或 bundle skills | 直接写 Rust agent loop |
| 新增 `/xxx` 命令 | `~/.deepseek/commands/xxx.md`（快照条目：现为 `~/.codewhale/commands/xxx.md`，`.deepseek` 仅 legacy 回退，见 §1 注） | 在 Tauri command 里硬编码 LLM 行为 |
| 接外部业务 API | 独立 MCP server | 直接塞进底座 ToolRegistry |
| 修改模型、base_url、API key | `bridge/prefs.rs`、`bridge/mod.rs`、设置页（⚠️ 快照平铺路径：现居 `platform/prefs/` 与 `features/assistant/platform/bridge.rs`） | 到处读 env |
| 改聊天 UI | `index.html` + `tauri-bridge.js` | 绕过 bridge 直接调用后端 |
| 改 session 存储 | `bridge/sessions.rs`（⚠️ 快照平铺路径：现居 `features/sessions/`） | 直接读写底座全局目录 |
| 改附件解析 | `file_ingest.rs` | 让 LLM 自己决定所有附件读取 |
| 改工作流 | `workflows/` + `harness.rs` + bundle 快照 | 让主聊天 assistant 手工串所有角色（快照条目：工作流子系统已在主干整体移除，此维护入口已失效） |
| 同步底座上游 | `DeepSeek-TUI` + fork guard + prompt diff | 整文件 `--theirs` 粗暴覆盖 |
| 修通用底座 bug | DeepSeek-TUI fork，考虑上游 PR | 在 pinvou3-app 绕一层补丁 |

## 14. 验证建议

Windows 迁移前的基线验证：

```bash
git submodule update --init --recursive
cd pinvou3-app
npm install
npm run dev
```

Rust 层：

```bash
cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib -- --test-threads=1
```

底座与 fork：

```bash
./scripts/fork-guard.sh --fast
./scripts/fork-guard.sh
cargo test -p codewhale-tui --lib
```

Windows 原生后建议增加这些测试：

- 路径测试：`HOME`/`USERPROFILE`、盘符、中文路径、空格路径。
- 附件测试：PDF、docx、xlsx、pptx、png、zip、eml。
- 打包测试：安装、卸载、升级、重启、数据目录保留。
- vLLM 测试：本机地址、远程地址、代理、超时、模型名 `_256k`。
- 多 session 测试：后台 session 流式输出、切换、取消、持久化。
- 工作流测试：启动、kick、SubAgent、gate、失败重试、恢复 run。（快照条目：工作流子系统已在主干整体移除，此条仅作 0.6.x 留档，现行验证跳过。）

## 15. 已下线或易误读的历史方案

这些内容可能还在 archived 文档或旧讨论里出现，接手时不要按它们继续开发：

- 品悟 v2 review 已推翻并从源码移除（2026-11 注：后续新方案已设计并落地为 v4 召唤式评审，见 [`品悟v4-召唤式评审设计.md`](./品悟v4-召唤式评审设计.md)）。
- 多 subagent 大研究 fan-out 已废弃，只保留单 + 串行。（2026-11 注；2026-11-18 r16 订正、2026-11-19 r19 溯源订正：r15 修复波注记曾引「4 并行 + 8 排队」系 ADR-0006 2026-08-04 保守档且双 scope 写反——该句已随 r15/r16 修复波推送、非未发布早稿——已被其 2026-09-14 蜂群修订取代（「历史表述以此为准」）——现行 ADR-0006 多智能体（`features/multiagent/`）支持会话内主动委派；蜂群开启时数量边界钉在底座硬顶：直属并发 128 / 全树准入 1024（`MAX_SUBAGENTS`/`MAX_SUBAGENT_ADMISSION`）+ AIMD 自适应；「4 直属并行 / 全树排队+运行合计 8」保守档仅作非蜂群会话预留，当前仅测试可达。）
- Plan/YOLO 收敛已完成：`PlanPhase` 状态机已删除，composer 提供 Plan/Yolo 手切 chip（默认按会话类型分流：code/workspace 绑定会话首启默认 Plan，仅未绑定普通会话默认 Yolo——`features/sessions/mode_state.rs` `resolved_default_mode`），`plan_stuck` 兜底卡保留；不要再按旧 PlanPhase / plan-fallback 逻辑开发。（改造方案与决策记录见 [`plan-yolo-回归底座式改造方案.md`](./plan-yolo-回归底座式改造方案.md)。）
- 大工具输出额外 large_output_router 不需要，底座已有 12K/结果上下文压缩（现为按路由的 `route_inline_char_budget`——窗口的 3%×4 字符、封顶 10 万字符；「12K」为写档时点平铺值，结论不变）。

## 16. 推荐阅读顺序

1. `AGENTS.md`：项目规则和底座边界。
2. `README.md`：产品定位与能力概览。
3. `process.md`：内部仓库的过程时间线（未随本仓库发布，跳过）。
4. `docs/fork-modifications.md`：DeepSeek-TUI fork 单一真相源。
5. `docs/auto-compact-256K-tuning.md`：本地 Qwen3.6 256K 上下文适配（史料；写死阈值已被取代）。
6. `docs/DeepSeek-TUI-工具系统.md`：底座工具注册与执行链路（v0.8.37 史料）。
7. `pinvou3-app/src-tauri/src/lib.rs`、`engine_pool.rs`、`engine.rs`、`bridge/mod.rs`、`commands.rs`：后端主链路。（⚠️ 快照平铺路径已随布局重构移除——仅 `src-tauri/src/lib.rs` 本体仍在，现承载契约测试、非主链路；现行入口见 `features/assistant/platform/bridge.rs` 与 `app/commands/`）
8. `pinvou3-app/src/tauri-bridge.js`：前端状态与事件主链路。（⚠️ 快照路径已移除，现行前端入口见 `src/features/`）
9. `docs/DeepSeek-TUI源码职责分析.md`：底座代码地图姊妹篇——关键调用链与维护风险清单与本手册 §3/§12/§14 同源，彼处含逐层代码锚点。

## 17. 给 Windows 迁移的建议路线

第一阶段先跑起来：

- 初始化 submodule。
- 安装 Rust、Node、Tauri CLI、WebView2。
- 先用远程或 WSL/GB10 的 vLLM，设置 `DEEPSEEK_BASE_URL` 和 `DEEPSEEK_MODEL=qwen36_35b_256k`。
- 暂时关闭或降级 Linux-only 的依赖安装与应用内升级。

第二阶段做跨平台抽象：

- 集中重构 `paths.rs` 的用户目录解析。（✅ 已完成：`platform/paths.rs` + `platform::os`）
- 把外部工具探测封成跨平台 helper。（✅ 已完成：`platform::os::*_tool_path` 族）
- 把依赖安装从 Linux `apt/pkexec` 拆成平台策略。（✅ 已完成：`features/dependencies/platform/`）
- 把更新器拆成 Linux `.deb` 与 Windows installer 两条路径。（部分完成：Linux 侧完整；现行 Windows 无应用内更新，安装包走 GitHub Releases，`features/updater/platform/` 仅 mod+unsupported）

第三阶段补 Windows 产品化：

- 配置 Windows bundle target。（✅ 已完成：现行配置 `msi`+`nsis`，发布 CI 实际产出 nsis，见 §11 注）
- 处理代码签名、安装目录、卸载保留用户数据。
- 验证附件解析工具的安装指引。
- 补 Windows 专用 smoke tests。

最后再考虑功能迭代。先稳住底座边界、路径、外部工具和打包安装，别一上来重写 agent 能力。

## 18. 人工验收清单

### US1：快速理解项目全貌

读者阅读本文后，应能回答：

- `pinvou3-app/` 和 `DeepSeek-TUI/` 分别负责什么？
- 一条聊天消息如何从 `tauri-bridge.js` 进入 `commands.rs::chat`，再经过 `EnginePool`、`AppEngine`、DeepSeek-TUI 和 vLLM 回到 `chat:*` 事件？
- 为什么不能在 pinvou3 里重新实现 Engine、ToolRegistry、SSE、Session、SkillRegistry、Commands、MCP、Hooks、Cycle、Compaction？
- `~/.pinvou3/settings.json`、`sessions/`、`workspace/`、`artifacts/`、`workflows/`、`bundle/` 分别保存什么？（快照口径：`workflows/` 已随工作流子系统移除）
- 新增领域 agent、slash command、外部 API、LLM 行为引导和 Tauri UI 分别应该改哪里？
- `file_ingest.rs`、`harness.rs`、`bridge/mod.rs` 在主链路中分别承担什么职责？（快照口径：`harness.rs` 已移除）
- 当前 `DeepSeek-TUI/` submodule 为空时会出现什么问题，如何初始化？
- 哪些文档是接手时必须优先阅读的单一真相源？

### US2：识别 Windows 迁移工作面

读者阅读本文后，应能列出至少 10 个迁移风险，并为每项指出影响模块和处理方向。最低应包含：

- `DeepSeek-TUI/` submodule 未初始化。
- Tauri bundle 只配置 `.deb`。（⚠️ 快照条目：现行 Windows 打包已上线——配置含 `msi`+`nsis`，发布 CI 实际产出 nsis，见 §11 ⚠️ 注与 §2 现行路径）
- `HOME` 与 Windows `USERPROFILE` 差异。（⚠️ 快照条目：已落地，见 §12 注）
- `which` 不适用于 Windows。（⚠️ 快照条目：已落地跨平台探测，见 §7 注）
- `pkexec` / `apt` 是 Linux-only。（⚠️ 快照条目：已拆平台策略，见 §7 注）
- `.deb` updater 不能直接用于 Windows。
- `run-dev.sh` 和 `scripts/*.sh` 偏 Bash/Linux。
- Poppler、Tesseract、LibreOffice、7z 的 Windows 安装和路径探测。
- `python3` 命令名在 Windows 上可能不同。
- Windows 防火墙、代理或内网策略影响 vLLM endpoint。

### US3：支撑后续迭代维护

读者面对以下需求时，应能在 5 分钟内指出入口文件和验证方式：

- 新增领域 agent 或工具组合：优先 `SKILL.md`，验证 skill 加载和对话效果。
- 接外部业务 API：优先独立 MCP server，验证 MCP 注册、工具 schema 和调用结果。
- 修改附件解析：入口 `pinvou3-app/src-tauri/src/features/files/file_ingest.rs`，验证多格式样本和缺依赖提示。
- 同步 DeepSeek-TUI 上游：入口 `DeepSeek-TUI/` 和 `docs/fork-modifications.md`，验证 fork guard、prompt diff、工具集合和动态激活路径。
- 修改 Windows 打包/更新：入口 `pinvou3-app/src-tauri/config/platforms/windows/tauri.conf.json`（平台层配置，base `tauri.conf.json` 不含打包 targets）、`pinvou3-app/src-tauri/src/app/commands/updater.rs`、设置页更新入口，验证安装、升级、卸载和用户数据保留。

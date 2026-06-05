# pinvou3

本地算力的桌面智能助手 —— Tauri 2.0 壳子 + 品悟 + [DeepSeek-TUI](https://github.com/h3c-hexin/DeepSeek-TUI) 底座 + 本地 vLLM 上的 Qwen3.6-35B-A3B-FP8（256K 窗口）。

## 架构

```
pinvou3-app (Tauri 2.0, 编排层)
   ↕ EngineHandle / AgentHarness
DeepSeek-TUI (底座 submodule, fork: pinvou3-patches 分支)
   ↕ OpenAI-compatible /v1/chat/completions
vLLM (本地 GB10) + Qwen3.6-35B-A3B-FP8 · max_model_len 256K
```

LLM 调用 / 工具循环 / 流式 SSE / Compaction 全在底座，pinvou3 编排层只做路由 / 构造 / 检查。

## 核心能力

### 方案准备好 (Plan / YOLO 双模式)

Plan 模式下 AI 不直接动手，先 `update_plan` 出方案 → 前端弹"方案准备好"卡片：

- ✅ 就这么干 — `accept_plan` 进 Executing
- ✏️ 改改 — 预填"修订方案:"，AI 重出方案
- 🚪 算了 — 丢弃，回 Planning 重谈

切换由顶部 💡 按钮，状态机贯穿前后端（`mode_state.rs`）。YOLO 模式下还有 careful hook 红卡片 BLOCK Dangerous 工具。

### 品悟（重做中）

v2 嘴替 review（EXIT GATE + 3 节点触发）设计已推翻，实现自源码移除（2026-06-05），留档 `docs/archived/Pinvou-品悟设计.md`。新方案另行设计。

### 多对话 + 产物持久化

- 左侧栏多对话历史，inline 重命名、删除、切换
- 切换时整段 messages 重渲染（含 plan_card / 工具卡片还原）
- 右栏"产物面板"自动跟踪 AI 写过的文件，列表 + 预览（文本/图片/PDF）+ 系统应用打开 + 文件夹定位
- 全部落盘 `~/.pinvou3/sessions/<id>.json`，每轮 `TurnComplete` 自动持久化

### 附件 ingestion pipeline

输入框 📎 / 拖拽 / Ctrl+V 粘贴均可：

- PDF → `pdftotext -layout`
- docx / xlsx → `pandoc`
- 图片 → 占位 + 路径
- 文本类 → 直接读

发送前转 markdown 嵌入 user message，省 LLM 工具决策成本，AI 直接拿到数据。

### 本地推理监控

5 秒刷新一屏看完：

- GPU：VRAM / 利用率 / 温度
- 系统：内存 / 磁盘
- vLLM：上游地址 / max_model_len / 运行+等待队列 / 历史累计 prefix cache 命中率（首字延迟代理指标）
- App：版本号 / 后端状态

### 256K 窗口适配

底座默认参数按 V4 1M 调，本地 256K 直接撞墙。pinvou3 在 fork 里改了 4 个子系统：`context_window_for_model` 识别 qwen 模型、`context_input_budget` 按窗口分级、`cycle saturating_sub` 关闭、`max_output_tokens` env wire。详 `docs/auto-compact-256K-tuning.md`。

### 其他

- **request_user_input 气泡** — AI 中途要决策，弹选择按钮气泡而非纯文本
- **edit_last_turn** — 最近一条 user 消息支持 inline 改文本重发
- **Skills / Commands** — 直接复用底座 SkillRegistry + `~/.deepseek/commands/*.md`，加领域 agent 不需要写 Rust
- **MCP server** — 底座自带 client，写独立 server 即可接外部 API
- **i18n** — 中文 / 英文 切换（DOM 扫描 `data-i18n`）
- **代码块** — 自动加语言标签 + COPY 按钮
- **token bar** — 上下文使用率绿/黄/红三档（256K 窗口下默认隐藏）

## 启动

```bash
git clone --recursive git@github.com:Pinvou/pinvou3.git
cd pinvou3
./pinvou3-app/run-dev.sh
```

`run-dev.sh` 集中处理 vLLM 端点 / 模型名（`qwen36_35b_256k`，后缀 `_256k` 触发底座 256K 窗口派生）/ 关闭 thinking 模式 / 允许内网 HTTP / 强制 HTTP/1.1。

前置：本地 vLLM 已用 `--served-model-name qwen36_35b_256k --max-model-len 262144` 起好。Rust 默认端点 `http://127.0.0.1:8000/v1`（release `.deb` 装到任何机器都连本机 vLLM）；本机 dev 走 `run-dev.sh` 里 `export DEEPSEEK_BASE_URL=http://10.214.74.113:8000/v1` 覆盖回开发机 GB10。

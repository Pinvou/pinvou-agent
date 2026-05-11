---
Claw Code (ultraworkers/claw-code) 全面研究报告
---
1. 语言/框架
主要语言：Rust（97.1%），辅以少量 Python（3.0%）和 Shell 脚本。
- Rust：2,942,496 字节 — 这是规范的、生产级的运行时实现
- Python：90,876 字节 — 伴随的参考/审计工作空间，不是主要运行时
- Shell：14,330 字节 — 构建脚本和 CI
- Dockerfile：271 字节
Rust 工作空间使用 edition 2021，依赖 tokio（异步运行时）、reqwest（HTTP 客户端）、serde/serde_json（序列化）、crossterm（终端渲染）、syntect（语法高亮）、pulldown-cmark（Markdown 解析）、rustyline（REPL 行编辑）。
---
2. 架构
仓库采用 双工作空间架构：
Rust 工作空间（rust/）— 规范实现
包含 9 个 crate，以 workspace 形式组织：
Crate	职责
rusty-claude-cli	CLI 入口点，二进制名为 claw；包含 REPL、渲染、输入处理
api	LLM 提供商抽象层；Anthropic / OpenAI-compat / xAI 客户端；SSE 解析；请求/响应类型；prompt cache
runtime	核心运行时：会话持久化、权限评估、prompt 组装、MCP 管道、文件操作、bash 执行、上下文压缩、恢复配方
tools	工具注册、分派、权限检查；内置工具规格定义；插件/MCP/运行时工具的统一注册表
commands	CLI 子命令（doctor、init、state、status 等）
plugins	插件生命周期管理（发现、健康检查、降级模式）
compat-harness	兼容性/奇偶校验测试工具
mock-anthropic-service	确定性 Anthropic 模拟服务（用于测试）
telemetry	遥测和分析事件
Python 工作空间（src/）— 参考/审计辅助
不是主要运行时表面。包含大量模块如 tools.py、models.py、context.py、permissions.py 等，以及 reference_data/ 目录下的快照文件（tools_snapshot.json、commands_snapshot.json），用于奇偶校验审计。
关键文档文件
- USAGE.md — 构建和用法指南
- PARITY.md — Rust 移植奇偶校验状态
- ROADMAP.md — 产品路线图
- PHILOSOPHY.md — 项目哲学
---
3. 支持的 LLM 后端
Claw Code 支持 三种提供商类型，通过 ProviderKind 枚举区分：
Anthropic（原生）
- 认证方式：ANTHROPIC_API_KEY（x-api-key 头）或 ANTHROPIC_AUTH_TOKEN（Authorization: Bearer 头）
- 自定义端点：ANTHROPIC_BASE_URL
- 模型别名：opus → claude-opus-4-6，sonnet → claude-sonnet-4-6，haiku → claude-haiku-4-5-20251213
OpenAI 兼容（通过 openai_compat.rs）
- OpenAI：OPENAI_API_KEY + OPENAI_BASE_URL，默认 https://api.openai.com/v1
  - 支持：gpt-4.1、gpt-4.1-mini、gpt-4.1-nano、gpt-5.4、gpt-5.4-mini、gpt-5.4-nano
- OpenRouter：通过 OPENAI_API_KEY + OPENAI_BASE_URL=https://openrouter.ai/api/v1
- Alibaba DashScope：DASHSCOPE_API_KEY + DASHSCOPE_BASE_URL，默认 https://dashscope.aliyuncs.com/compatible-mode/v1
  - 支持：qwen-* 系列（qwen-max、qwen-plus、qwen-turbo、qwen-qwq 等）和 kimi-* 系列（kimi-k2.5、kimi-k1.5）
- 本地模型服务器（Ollama、LM Studio、vLLM 等）：通过 OPENAI_BASE_URL 配置
xAI
- 认证：XAI_API_KEY + XAI_BASE_URL，默认 https://api.x.ai/v1
- 模型别名：grok/grok-3 → grok-3，grok-mini/grok-3-mini → grok-3-mini，grok-2
提供商自动检测逻辑
1. 模型名前缀优先（openai/、gpt-、qwen/、qwen-、kimi/、kimi-、grok）
2. OPENAI_BASE_URL 存在时优先路由到 OpenAI（支持本地模型）
3. 回退到环境变量嗅探：ANTHROPIC_API_KEY → OPENAI_API_KEY → XAI_API_KEY
---
4. 工具使用 / Function Calling
Claw Code 实现了完整的 工具调用循环：
工具分派架构
- GlobalToolRegistry：统一注册表，管理内置工具、运行时工具和插件工具
- ToolSpec：定义工具名称、描述、输入 JSON Schema、所需权限级别
- execute_tool_with_enforcer()：执行工具前先经过 PermissionEnforcer 权限检查
- 工具结果通过 ToolResultContentBlock 反馈到对话中
内置工具（MVP 工具集，由 mvp_tool_specs() 定义）
工具	权限级别
bash	DangerFullAccess
read_file	ReadOnly
write_file	WorkspaceWrite
edit_file	WorkspaceWrite
glob_search	ReadOnly
grep_search	ReadOnly
以及更多...	—
延迟工具（deferred_tool_specs()）
包含通过 MCP 动态注册的工具、代理子工具等，在搜索时可见但不一定是即时可用的。
Function Calling 协议
- 使用标准的 ToolDefinition（name + description + input_schema）发送给 LLM
- 支持 ToolChoice::Auto 和显式工具选择
- OpenAI-compat 路径会将 Anthropic 格式的工具调用翻译为 OpenAI Chat Completion 的 tool_calls 格式
---
5. TUI/UI 框架
不使用现成的 TUI 框架（如 ratatui、cursive 等），而是 自研终端渲染系统：
- 渲染层（render.rs）：使用 crossterm 直接控制终端（光标移动、颜色、清屏）
- Markdown 渲染：pulldown-cmark 解析 Markdown → syntect 语法高亮代码块 → crossterm 样式输出
- 特性：
  - 自定义颜色主题（ColorTheme）：标题（Cyan）、强调（Magenta）、粗体（Yellow）、行内代码（Green）、链接（Blue）、引用（DarkGrey）
  - Spinner 动画（braille 字符 ⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏）
  - 表格渲染状态机
  - 有序/无序列表嵌套栈
  - 语法高亮（通过 syntect 的 SyntaxSet + ThemeSet）
- 输入层（input.rs）：使用 rustyline 提供 REPL 行编辑（历史、补全等）
- 交互模式：REPL（claw 无参数）、单次提示（claw prompt "..."）、简写模式（claw "..."）
---
6. 开发活跃度
指标
Stars
Forks
Watchers
Open Issues
创建时间
最近推送
仓库大小
主要语言
贡献者
贡献者
Yeachan-Heo
code-yeongyu (YeonGyu-Kim)
andhai
sigridjineth
最近提交（2026年5月）
- 非常活跃，几乎每天都有合并
- 最近重点：技能系统路由修复、REPL 显示修复、/compact panic 修复、DeepSeek reasoning 支持、OpenAI token limit 加固、MCP 修复、插件市场别名路由
生态项目
- clawhip (https://github.com/Yeachan-Heo/clawhip) — 事件和通知路由
- oh-my-openagent (https://github.com/code-yeongyu/oh-my-openagent) — 多代理协调
- oh-my-claudecode (https://github.com/Yeachan-Heo/oh-my-claudecode) — Claude Code 增强
- oh-my-codex (https://github.com/Yeachan-Heo/oh-my-codex) — 工作流层
---
7. 代码库组织
claw-code/
├── rust/                          ← 规范 Rust 工作空间
│   ├── Cargo.toml                 ← workspace 定义
│   ├── crates/
│   │   ├── rusty-claude-cli/      ← CLI 二进制 (claw)
│   │   │   ├── src/main.rs        ← 入口点
│   │   │   ├── src/render.rs      ← 终端渲染引擎
│   │   │   ├── src/input.rs       ← REPL 输入处理
│   │   │   └── src/init.rs        ← 初始化命令
│   │   ├── api/                   ← LLM 提供商抽象
│   │   │   └── src/providers/
│   │   │       ├── anthropic.rs   ← Anthropic 原生客户端
│   │   │       └── openai_compat.rs ← OpenAI 兼容客户端
│   │   ├── runtime/               ← 核心运行时 (44个模块)
│   │   │   ├── src/bash.rs        ← Bash 执行 (283 LOC)
│   │   │   ├── src/file_ops.rs    ← 文件操作 (744 LOC)
│   │   │   ├── src/compact.rs     ← 上下文压缩
│   │   │   ├── src/session.rs     ← 会话持久化
│   │   │   ├── src/permissions.rs ← 权限模型
│   │   │   ├── src/config.rs      ← 配置加载
│   │   │   ├── src/mcp*.rs        ← MCP 系统全套
│   │   │   ├── src/sandbox.rs     ← 沙箱 (385 LOC)
│   │   │   └── ...                ← 36个其他模块
│   │   ├── tools/                 ← 工具注册和分派
│   │   │   └── src/lib.rs         ← 全局工具注册表
│   │   ├── plugins/               ← 插件生命周期
│   │   ├── commands/              ← CLI 子命令
│   │   ├── telemetry/             ← 遥测
│   │   ├── compat-harness/        ← 兼容性测试
│   │   └── mock-anthropic-service/ ← 模拟服务
│   └── tests/                     ← 集成测试
├── src/                           ← Python 参考/审计工作空间
├── tests/                         ← 测试
├── docs/                          ← 文档
├── scripts/                       ← 构建脚本
├── USAGE.md / PARITY.md / ROADMAP.md / PHILOSOPHY.md
└── Containerfile                  ← 容器构建
统计数据（来自 PARITY.md）：48,599 Rust LOC，2,568 测试 LOC，292 次 main 分支提交。
---
8. 本地模型原生支持
是的，通过 OpenAI 兼容层原生支持。 关键机制：
1. OPENAI_BASE_URL 自动路由：设置此环境变量后，即使模型名没有已知前缀（如 qwen2.5-coder:7b），也会路由到 OpenAI-compat 提供商
2. 无需 API Key 的本地服务器：如果只有 OPENAI_BASE_URL 而没有 OPENAI_API_KEY（如 Ollama），仍然路由到 OpenAI 提供商
3. ANTHROPIC_BASE_URL：也可以指向本地代理或兼容服务
代码中的检测逻辑（来自 detect_provider_kind()）：
// 当 OPENAI_BASE_URL 设置时，优先路由到 OpenAI-compat
// 这是本地提供商（Ollama、LM Studio、vLLM 等）的常见情况
if std::env::var_os("OPENAI_BASE_URL").is_some() && openai_compat::has_api_key("OPENAI_API_KEY") {
    return ProviderKind::OpenAi;
}
// ... 最后：即使没有 API key，只要有 OPENAI_BASE_URL 也路由到 OpenAI
if std::env::var_os("OPENAI_BASE_URL").is_some() {
    return ProviderKind::OpenAi;
}
---
9. 可扩展性
插件系统
- plugins crate 完整管理插件生命周期
- PluginLifecycle：状态机驱动的生命周期（发现 → 健康检查 → 运行 → 降级）
- PluginTool：插件可以注册自定义工具，与内置工具统一调度
- 插件冲突检测：禁止与内置工具同名、禁止重复插件名
- 插件配置：RuntimePluginConfig 管理 enabled_plugins、external_directories、install_root、registry_path、bundled_root
- 插件可以覆盖 maxOutputTokens
- 捆绑插件目录：rust/crates/plugins/bundled/（含 example-bundled、sample-hooks）
- 插件市场：支持 /plugins 斜杠命令
MCP (Model Context Protocol) 系统
- 完整的 MCP 实现：stdio、SSE、HTTP、WebSocket、SDK、ManagedProxy 六种传输方式
- McpToolRegistry：MCP 服务器注册表，跟踪连接状态、工具列表、资源列表
- McpServerManager：管理 MCP 服务器的启动和通信
- MCP 工具自动注册为可用工具（带 mcp__ 前缀）
- 降级模式：部分 MCP 服务器失败时，系统仍可在降级模式下运行
- 配置支持：.claw/settings.json 中的 mcpServers 字段
Hook 系统
- RuntimeHookConfig：pre_tool_use、post_tool_use、post_tool_use_failure 三个生命周期阶段
- HookRunner：执行钩子命令，支持中止信号和进度报告
配置层级
User (~/.claw/settings.json) → Project (.claw/settings.json) → Local (.claw/settings.local.json)
权限规则扩展
- RuntimePermissionRuleConfig：allow、deny、ask 三种规则列表
- PolicyEngine：可执行的策略引擎，评估合并条件、审查状态等
---
10. 许可证
MIT 许可证。在 Cargo.toml 工作空间中明确声明：
[workspace.package]
license = "MIT"
但 GitHub API 返回 license: null（可能是因为仓库没有单独的 LICENSE 文件，而是在 Cargo.toml 中声明）。
重要免责声明：仓库声明"不声称拥有原始 Claude Code 源材料的所有权"，也"不隶属于、不被认可或维护于 Anthropic"。
---
11. 上下文管理
Claw Code 有完善的上下文管理系统：
自动压缩（compact.rs）
- CompactionConfig：配置压缩阈值
  - preserve_recent_messages：保留最近 N 条消息（默认 4）
  - max_estimated_tokens：触发压缩的最大 token 估算（默认 10,000）
- should_compact()：检测会话是否超过压缩预算
- compact_session()：执行压缩 — 将旧消息摘要为 summary，保留最近消息
- estimate_session_tokens()：粗略估算当前会话的 token 占用
- 压缩续接消息：自动生成"此会话从先前对话延续"的系统消息
- 工具配对保护：压缩时不会拆散 tool-use / tool-result 配对（避免 OpenAI-compat 路径的 400 错误）
- auto_compaction_threshold_from_env()：可通过环境变量配置
Prompt Cache（prompt_cache.rs）
- PromptCache：管理 Anthropic 的 prompt caching 机制
- PromptCacheStats：跟踪缓存命中/未命中
- CacheBreakEvent：检测缓存何时被打破
Preflight 检查
- preflight_message_request()：在发送前检查请求是否超出模型上下文窗口
- 如果估算的总 token 数（输入 + 输出）超过 context_window_tokens，返回 ContextWindowExceeded 错误
- 支持所有已注册模型的上下文窗口元数据
系统提示词
- SystemPromptBuilder：动态组装系统提示
- ModelFamilyIdentity：根据模型家族（Claude vs Generic）选择不同的提示策略
- FRONTIER_MODEL_NAME：前沿模型标识
- SYSTEM_PROMPT_DYNAMIC_BOUNDARY：动态边界标记
---
12. 支持的工具类型
内置核心工具（mvp_tool_specs()）
工具	描述
bash	执行 shell 命令，支持超时、后台、沙箱、网络隔离、文件系统隔离
read_file	读取工作空间文本文件，支持 offset/limit 分页
write_file	写入工作空间文件
edit_file	文本替换编辑，支持 replaceAll 全局替换
glob_search	glob 模式文件搜索
grep_search	正则表达式内容搜索
运行时注册的扩展工具
通过 runtime_tools 注册，来自 Rust 运行时的各子系统：
工具类别	来源模块
任务工具	task_registry.rs：TaskCreate、TaskGet、TaskList、TaskStop、TaskUpdate、TaskOutput
团队/定时工具	team_cron_registry.rs：TeamCreate、TeamDelete、CronCreate、CronDelete、CronList
MCP 工具	mcp_tool_bridge.rs：动态注册的 MCP 服务器工具（带 mcp__ 前缀）
LSP 工具	lsp_client.rs：语言服务器协议客户端
代理工具	子代理派生和协调
问答工具	AskUserQuestion（运行时实现，非桩）
MCP 动态工具
- 通过 MCP 协议从外部服务器动态发现
- 支持工具发现（McpListTools）、资源读取（McpReadResource）、工具调用（McpToolCall）
- MCP 服务器可以是 stdio 进程、远程 HTTP/SSE/WS 服务、SDK 插件或托管代理
插件工具
- 通过插件系统动态注册
- 与内置工具享有相同的分派路径
- 插件可声明自己的权限级别
权限模型
三种权限级别控制工具访问：
- ReadOnly：只读操作（读文件、搜索）
- WorkspaceWrite：可写工作空间（写文件、编辑文件）
- DangerFullAccess：完全访问（bash 命令）
可通过 --permission-mode 和 --allowedTools CLI 参数，或配置文件中的 allow/deny/ask 规则进一步限制。
---
总结
Claw Code 是一个 Rust 构建的高性能 CLI 编码代理，具有以下核心特征：
1. 多提供商 LLM 后端：Anthropic 原生、OpenAI 兼容（含本地模型）、xAI，以及通过 DashScope 的 Qwen/Kimi
2. 原生本地模型支持：通过 OPENAI_BASE_URL 环境变量即可连接 Ollama、vLLM、LM Studio 等
3. 完善的工具系统：内置文件/Bash/搜索工具 + 任务注册 + MCP 动态工具 + 插件扩展
4. 自研 TUI：基于 crossterm + syntect + pulldown-cmark 的高质量终端渲染
5. 智能上下文管理：自动压缩、prompt caching、preflight 上下文窗口检查
6. 高度可扩展：插件系统、MCP 协议支持、Hook 系统、策略引擎
7. MIT 许可，不隶属于 Anthropic
8. 极其活跃：190K+ stars，每日提交，3 位核心贡献者

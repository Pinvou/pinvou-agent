OpenCode (anomalyco/opencode) 深度研究报告
1. 语言/框架
主要语言：TypeScript（占代码库 ~58%）
语言
TypeScript
MDX
CSS
Astro
JavaScript
Shell
Nix
核心框架和技术栈：
- 运行时：Bun（包管理器指定为 bun@1.3.13），也支持 Node.js
- AI SDK：Vercel AI SDK (ai@6.0.168) 作为统一的 LLM 调用层
- TUI 框架：OpenTUI（@opentui/core@0.2.4、@opentui/solid@0.2.4、@opentui/keymap@0.2.4）—— 这是他们自己开发的终端 UI 框架，基于 SolidJS 响应式系统
- Web 框架：SolidJS + Solid Start（用于 Web 控制台和桌面应用）
- 后端 HTTP：Hono（用于服务器/路由/中间件）
- 数据库：SQLite + Drizzle ORM（drizzle-orm@1.0.0-beta）
- 函数式编程：Effect 框架（effect@4.0.0-beta）
- Schema 验证：Zod v4（zod@4.1.8）
- 构建：Turborepo（monorepo 管理）
- 代码解析：Tree-sitter（bash、powershell 语法解析）
- 桌面应用：Electron
- 基础设施：SST（sst@3.18.10）
---
2. 架构
OpenCode 采用 monorepo + 客户端/服务器架构。
顶层目录结构
opencode/
├── packages/          # 核心代码（monorepo 工作区）
├── sdks/              # SDK（目前有 VSCode 扩展）
├── infra/             # 基础设施配置
├── specs/             # 规格定义
├── patches/           # 依赖补丁
├── script/            # 构建和工具脚本
├── nix/               # Nix 打包配置
├── .opencode/         # OpenCode 自身配置
├── github/            # GitHub Actions 相关
├── install            # 安装脚本
├── package.json       # 根 package.json（workspace 配置）
├── turbo.json         # Turborepo 构建配置
├── bun.lock           # Bun 锁文件
├── sst.config.ts      # SST 基础设施配置
├── flake.nix          # Nix flake 配置
├── AGENTS.md          # Agent 开发指南
├── CONTRIBUTING.md    # 贡献指南
└── README.md (+ 多语言版本)
packages/ 子包（18 个包）
包名
opencode
core
app
console
desktop
docs
enterprise
extensions
function
identity
plugin
sdk
script
slack
storybook
ui
web
containers
packages/opencode/src/ 核心模块
模块
agent/
bus/
cli/
command/
config/
control-plane/
file/
git/
ide/
lsp/
mcp/
permission/
plugin/
project/
provider/
pty/
server/
session/
shell/
skill/
snapshot/
storage/
sync/
tool/
worktree/
auth/
format/
share/
acp/
---
3. 支持的 LLM 后端
OpenCode 基于 Vercel AI SDK 构建了 Provider 抽象层，支持极其广泛的 LLM 后端。从 packages/opencode/package.json 中的依赖可以看到完整的支持列表：
云端 API 提供商
提供商
Anthropic (Claude)
OpenAI
Google (Gemini)
Google Vertex AI
Amazon Bedrock
Azure OpenAI
Groq
Mistral
Cohere
xAI (Grok)
Cerebras
Perplexity
Together AI
DeepInfra
Alibaba (通义千问)
Vercel AI Gateway
OpenRouter
Venice AI
GitLab
AI Gateway
OpenAI Compatible
特殊 Provider
- GitHub Copilot：有专门的 packages/opencode/src/provider/sdk/copilot/ 目录
- OpenCode Zen：自有托管服务（通过 @ai-sdk/vercel 和 control-plane 模块）
---
4. 工具使用 / 函数调用机制
OpenCode 实现了一套完整的工具系统，位于 packages/opencode/src/tool/：
工具注册和调度
- tool.ts —— 工具基础类型定义和接口
- registry.ts —— 工具注册中心，负责注册和管理所有可用工具
- schema.ts —— 工具参数 schema 定义
内置工具类型（12+ 种）
工具
read
write
edit
apply_patch
glob
grep
shell
lsp
webfetch
websearch
question
skill
task
todo
plan
mcp-exa
truncate
external-directory
invalid
每个工具都有对应的 .txt 文件作为 prompt 描述模板（告诉 LLM 如何使用该工具）。
工具调用流程
工具调用通过 Vercel AI SDK 的标准 function calling 机制实现。LLM 返回工具调用请求，session 层的 processor.ts 处理执行，结果返回给 LLM 继续推理。
---
5. TUI/UI 框架
OpenTUI —— OpenCode 团队自研的终端 UI 框架
关键信息：
- 包名：@opentui/core@0.2.4、@opentui/solid@0.2.4、@opentui/keymap@0.2.4
- 基于 SolidJS 响应式系统在终端中渲染
- 有专门的 keymap 引擎（最近提交 "introduce opentui keymap as sole key/cmd engine" 证实了这点）
- 由 neovim 用户和 terminal.shop 创始人构建，极度注重终端体验
- 还有 opentui-spinner@0.0.6 用于加载动画
除 TUI 外，还有：
- Web Console：基于 SolidJS + Solid Start 的 Web UI
- Desktop App：基于 Electron 的桌面应用
- 都通过 HTTP API 连接到同一个 server 后端
---
6. 开发活跃度
极其活跃！
指标
Stars
Forks
Open Issues
Watchers
创建时间
最近推送
默认分支
npm 版本
Top 贡献者
贡献者
thdxr
adamdotdevin
rekram1-node
actions-user
kitlangton
opencode-agent[bot]
iamdavidhill
jayair
fwang
Brendonovich
最近 10 个提交全在今天（2026-05-08），包含性能优化、UI 修复、桌面应用改进等，还有 opencode-agent 机器人自动提交。
---
7. 代码库组织（关键文件及职责）
顶层
- package.json —— workspace 配置、依赖 catalog
- turbo.json —— 构建流水线
- sst.config.ts —— 云基础设施
- AGENTS.md —— Agent 开发规范（编码风格指南）
- CONTRIBUTING.md —— 贡献流程
packages/opencode/（核心包）
- src/index.ts —— 主入口，启动 TUI 和服务器
- src/provider/provider.ts (65KB) —— Provider 核心实现
- src/provider/transform.ts (44KB) —— 消息格式转换
- src/provider/models.ts —— 模型定义
- src/session/session.ts (31KB) —— 会话管理
- src/session/prompt.ts (77KB) —— 最大文件，prompt 构建逻辑
- src/session/compaction.ts (22KB) —— 上下文压缩
- src/session/processor.ts (28KB) —— 消息处理循环
- src/session/message-v2.ts (41KB) —— 消息格式定义
- src/tool/registry.ts —— 工具注册中心
- src/tool/shell.ts —— Shell 工具（最复杂，20KB）
- src/tool/edit.ts —— 文件编辑工具
- src/config/config.ts (34KB) —— 配置系统
- src/mcp/index.ts (33KB) —— MCP 客户端实现
- src/server/server.ts —— HTTP 服务器
- src/storage/storage.ts —— 存储层
条件导入（平台适配）
"#db": { "bun": "./db.bun.ts", "node": "./db.node.ts" }
"#pty": { "bun": "./pty.bun.ts", "node": "./pty.node.ts" }
"#hono": { "bun": "./adapter.bun.ts", "node": "./adapter.node.ts" }
---
## 8. 本地模型支持
**是的，原生支持！** 证据如下：
1. **`@ai-sdk/openai-compatible`** —— 通用 OpenAI API 兼容提供者，可直接连接本地推理服务器（如 Ollama、vLLM、LM Studio、llama.cpp server 等）
2. **`@ai-sdk/gateway`** —— AI Gateway 支持，也可路由到本地模型
3. README 明确说明："OpenCode can be used with Claude, OpenAI, Google, **or even local models**"
4. 配置系统中的 `config/provider.ts` 和 `config/model-id.ts` 允许自定义 provider 和模型 ID，可以通过 OpenCode 的 `opencode.json` 配置文件指定任何兼容 OpenAI API 的本地端点
---
9. 可扩展性
OpenCode 具有非常好的可扩展性，体现在多个层面：
插件系统
- packages/plugin/ —— 专用插件包
- src/plugin/ —— 插件加载器
- src/config/plugin.ts —— 插件配置
MCP (Model Context Protocol)
- src/mcp/ —— 完整的 MCP 客户端实现（33KB），支持：
  - MCP Server 发现和连接
  - OAuth 认证（auth.ts、oauth-provider.ts、oauth-callback.ts）
  - 工具动态注册
- src/config/mcp.ts —— MCP 配置（在 opencode.json 中配置 MCP 服务器）
Skill 系统
- src/skill/ —— Skill 加载和执行
- src/config/skills.ts —— Skill 配置
- Skills 类似于 Claude Code 的能力注入，可以动态添加能力
Agent 可配置
- src/config/agent.ts —— 可自定义 Agent（build、plan 或创建新 Agent）
- 每个 Agent 可以有不同的工具集、权限和 prompt
自定义 Provider/Model
- src/config/provider.ts —— 可在配置文件中添加自定义 provider
- src/config/model-id.ts —— 自定义模型 ID 格式
ACP (Agent Client Protocol)
- src/acp/ —— 支持 Agent Client Protocol，用于与其他工具集成
VSCode 扩展
- sdks/vscode/ —— VSCode 扩展 SDK
键绑定自定义
- src/config/keybinds.ts —— 完全可自定义的键绑定
权限系统
- src/config/permission.ts —— 细粒度权限控制
- src/permission/ —— 权限执行层
---
10. 许可证
MIT License —— 完全开源，无商业限制
---
11. 上下文管理
OpenCode 有一套成熟的上下文管理系统：
会话层 (src/session/)
- session.ts —— 会话生命周期管理，包含对话历史
- prompt.ts (77KB，最大的源文件) —— 构建 LLM prompt，包括系统指令、工具描述、文件上下文等
- processor.ts —— 消息处理循环，管理工具调用链
上下文压缩 (src/session/compaction.ts, 22KB)
- 当上下文超出模型窗口限制时，自动压缩对话历史
- 使用 LLM 生成对话摘要来减少 token 消耗
- 这是一个智能压缩机制，不是简单截断
上下文溢出处理 (src/session/overflow.ts)
- 处理上下文超出限制的情况
Projectors (src/session/projectors.ts, projectors-next.ts)
- "投影器"——将不同类型的数据（文件、工具输出等）投影到 LLM 可理解的格式
- 管理哪些信息应该包含在上下文中
指令系统 (src/session/instruction.ts)
- 从项目的 AGENTS.md、CLAUDE.md 等文件加载自定义指令
- 类似 Claude Code 的项目级指令
系统提示 (src/session/system.ts)
- 构建系统提示，包含环境信息、项目结构等
摘要生成 (src/session/summary.ts)
- 生成会话摘要，用于跨会话持久化上下文
重试机制 (src/session/retry.ts)
- LLM 调用失败时的重试和错误恢复
---
12. 支持的工具类型
完整列表总结：
类别
文件读取
文件写入
文件编辑
文件编辑
文件搜索
内容搜索
命令执行
语言服务
网络获取
网络搜索
用户交互
能力注入
子任务
任务管理
模式切换
输出控制
目录访问
搜索集成
MCP 工具
---
总结
OpenCode 是一个极其雄心勃勃且执行出色的开源 AI 编码代理项目。关键亮点：
1. 技术先进：TypeScript + Bun + Vercel AI SDK + OpenTUI + Hono + Drizzle ORM，现代技术栈
2. Provider 无关：支持 20+ 种 LLM 提供商，包括本地模型，真正不绑定任何供应商
3. 客户端/服务器架构：TUI 只是客户端之一，还有 Web、桌面、移动端潜力
4. 社区活跃：156K+ 星标，20+ 活跃核心贡献者，每天都有多个提交
5. 高度可扩展：MCP、插件、Skill、自定义 Agent、自定义 Provider
6. MIT 许可：完全开源，商业友好
7. 上下文管理成熟：自动压缩、智能投影、指令注入，应对长对话
8. 工具丰富：18+ 内置工具 + MCP 动态工具，覆盖编码全流程

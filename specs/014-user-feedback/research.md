# 研究：我要反馈

## Decision: 主入口放在设置页的帮助/支持区域

**Rationale**：现有 app 已有 `SettingsView`，侧边栏底部也有稳定设置入口。反馈是低频但需要可靠发现的支持类能力，放在设置页内符合用户预期，也不会打断聊天主流程。关键错误提示可额外提供“我要反馈”上下文入口，把错误摘要预填到反馈说明中。

**Alternatives considered**：
- 放在聊天输入框工具栏：更靠近主要流程，但会把反馈和对话操作混在一起，增加常驻 UI 噪音。
- 新增侧边栏一级导航：发现性强，但对低频能力过重，也会挤占已有 monitor/workflow/tool store/card pool 导航。

## Decision: 不新建后台，复用 H3CLogCollector 的上传协议语义

**Rationale**：用户明确补充“后台环境不用自己实现，使用 H3CLogCollector 中的方式上传”。读取 `D:\0_Projects\01_全家桶\H3CLogCollector\LogCollector.Business\WebService\LogWebService.cs` 后确认既有方式为：目录打包成 `tar.gz`，按 `0x55` 对字节 XOR 生成 `.dbg`，通过 `uploadRequest` 获取 token，再以 `PUT uploadSysinfoFile` 上传二进制流，并附带 `GwSn`、`FileName`、`checkCode` 请求头。

**Alternatives considered**：
- 自建反馈后台：与最新需求冲突，且扩大范围。
- 直接调用外部 C# 项目或进程：耦合部署环境，不适合 Tauri 安装包内的稳定功能。
- 只上传普通 JSON：不能复用既有接收通道，团队侧无法按现有流程接收。

## Decision: 在 Rust 中实现兼容上传流程

**Rationale**：pinvou3-app 的 Tauri 后端已经使用 Rust、`reqwest` 和 `md5`，可在本应用内实现 H3CLogCollector 兼容流程。这样不需要引入 .NET runtime 或外部服务，也能把路径、隐私、错误提示和测试放在 pinvou3 的质量门禁内。

**Alternatives considered**：
- 把 H3CLogCollector 作为外部可执行程序调用：难以跨安装环境保证存在，错误处理也不透明。
- 抽成 MCP server：反馈上传是 app UI 的提交动作，不是 LLM 工具能力；MCP 会把用户支持流程错误地放入 agent 工具层。

## Decision: 反馈包使用目录结构和 `manifest.json`

**Rationale**：H3CLogCollector 原流程上传目录。反馈文字、环境摘要、附件和回执可先组织到一个临时目录，再复用相同打包流程。`manifest.json` 作为包内索引，便于团队侧打开后快速识别类型、时间、app 版本、附件清单和用户说明。

**Alternatives considered**：
- 单一 JSON 内嵌 base64 附件：文件会膨胀，视频处理不友好。
- 多次分别上传文字与附件：可能出现部分送达，团队侧难以关联。

## Decision: 附件限制前后端双重校验

**Rationale**：前端即时反馈提升体验，后端强校验保证安全与一致性。首版建议图片允许 `png`、`jpg`、`jpeg`、`gif`、`webp`，视频允许 `mp4`、`mov`、`webm`；附件总数不超过 5 个，单图片不超过 10 MB，单视频不超过 50 MB，全部附件合计不超过 80 MB。计划阶段先以这些数值生成任务，后续如接收通道有更严格限制，可集中调整。

**Alternatives considered**：
- 不限制视频大小：上传失败率和等待时间不可控。
- 只在前端限制：可被绕过，且 Rust 命令直接接收路径时仍需保护。

## Decision: 自动环境摘要采用白名单

**Rationale**：规格明确不得在未告知用户的情况下收集聊天内容、文件正文或无关敏感数据。环境摘要只包含 app 版本、OS、架构、语言、提交入口、可选错误摘要和时间；不包含当前聊天消息、附件原始路径的完整敏感目录、用户文档正文或模型输出。

**Alternatives considered**：
- 自动附带最近日志和聊天上下文：排查价值高，但隐私风险过大，且不符合本地数据边界。
- 完全不附带环境：会降低定位效率，尤其是版本和系统差异问题。

## Decision: 设备序列号采用 Windows 优先采集、配置覆盖和明确失败

**Rationale**：H3CLogCollector 默认用设备序列号生成 token 与 `checkCode`。pinvou3 不能依赖其 C# 私有库，因此计划在 Rust 上传模块中提供设备序列号解析：Windows 优先取系统序列号，允许通过配置或环境变量覆盖；无法取得时提交失败并提示用户当前设备无法使用既有上传通道。Linux 首版不承诺上传成功，除非用户配置可用序列号。

**Alternatives considered**：
- 生成随机 ID：无法通过既有 H3C token 校验。
- 静默使用空字符串：会造成难以理解的上传失败。

## Decision: 失败时保留待重试目录

**Rationale**：规格要求网络或服务不可用时保留用户已填写内容。后端在上传失败时保留 `~/.pinvou3/feedback/pending/<feedback_id>/`，前端提示可稍后重试。首版可先实现当前表单内重试；持久化重试队列可作为后续增强。

**Alternatives considered**：
- 失败即删除临时包：用户体验差，也不利于问题定位。
- 后台自动无限重试：容易造成用户不可见的外发行为，不符合数据边界。

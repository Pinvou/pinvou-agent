# 研究：修复 Windows 语音输入

## 决策 1：先诊断现有语音链路，而不是重做语音识别

**Decision**：本 feature 以查明 Windows 语音输入断点为首要工作，沿现有 pinvou UI/bridge 和 DeepSeek-TUI voice capture 能力修复。

**Rationale**：DeepSeek-TUI 已有 `/voice` 命令、`AppAction::VoiceCapture` 和 voice capture/transcribe 流程。项目宪章要求不重写底座能力；Windows 不可用更可能来自桌面集成、权限、入口或进程环境差异。

**Alternatives considered**：
- 直接新增前端 `MediaRecorder` + 浏览器语音识别：拒绝，可能绕开底座并引入外部/浏览器差异。
- 内置新的本地 ASR：拒绝，超出“修复不可用”的范围，也会扩大包体和配置复杂度。

## 决策 2：Windows 权限与设备状态必须有用户可见反馈

**Decision**：实现阶段必须区分至少三类失败：麦克风权限不足、录音设备不可用、识别/转写失败。

**Rationale**：语音输入涉及系统权限、硬件设备、Tauri WebView 或底座 capture，多数失败如果只表现为按钮无响应，会严重降低可诊断性。规格要求 2 秒内给出明确提示。

**Alternatives considered**：
- 只在日志中记录错误：拒绝，用户仍无法自助处理。
- 只给通用“语音输入失败”：可作为兜底，但不足以满足可操作提示。

## 决策 3：识别结果必须绑定启动时会话

**Decision**：语音输入启动时记录当前会话/输入上下文；结果返回时若上下文已变化，不自动写入错误会话。

**Rationale**：pinvou 支持多会话和异步操作。语音识别可能耗时，用户期间切换会话时，结果误写会破坏对话上下文。

**Alternatives considered**：
- 始终写入当前活动会话：拒绝，存在跨会话污染风险。
- 切换会话时强制取消：可作为实现策略之一，但仍要有明确状态和提示。

## 决策 4：测试采用单测 + Windows 手动 smoke

**Decision**：计划阶段定义自动化覆盖状态机、上下文绑定、错误映射；硬件麦克风和系统权限用 Windows 手动 smoke 验证。

**Rationale**：麦克风权限和 WebView 设备访问很难在普通 CI 中稳定复现。自动化测试覆盖纯逻辑，真实设备验证覆盖端到端风险。

**Alternatives considered**：
- 只做手动验证：拒绝，状态机和错误映射容易回归。
- 完全自动化硬件录音：当前仓库没有稳定设备仿真基础，不适合作为本 feature 前置。

## 实施排查记录：Windows 语音输入断点

**工作区基线**：当前分支为 `014-fix-windows-voice-input`。实施前已有 `.specify/feature.json`、`AGENTS.md` 和 `specs/014-fix-windows-voice-input/` 的 Spec Kit 产物改动。

**pinvou 前端入口**：`pinvou3-app/src/index.html` 仅定义了 `Mic` 图标，聊天输入区原本没有麦克风按钮、录音状态、权限提示或语音回填调用；`pinvou3-app/src/tauri-bridge.js` 原本也没有 `getUserMedia`、`MediaRecorder`、语音状态或语音 IPC。

**DeepSeek-TUI 底座链路**：`DeepSeek-TUI/crates/tui/src/commands/groups/core/voice.rs` 已有 `/voice`、`AppAction::VoiceCapture`、`capture_and_transcribe`。该链路的录音依赖平台命令行工具：Linux 用 `arecord/sox`，macOS 用 `sox/rec`，Windows 只检测 `sox`。当前 Windows 环境 `Get-Command sox` 未找到可执行文件，因此底座 TUI 命令在 Windows GUI 安装包中不具备可用录音器。

**Tauri 权限/能力**：`pinvou3-app/src-tauri/capabilities/default.json` 没有自定义麦克风权限项；现有自定义 Tauri commands 通过 invoke handler 暴露，不需要为本项目命令逐项列 capability。麦克风采集改为 WebView `navigator.mediaDevices.getUserMedia`，权限失败由前端分类并提示。

**修复位置决策**：修复放在 `pinvou3-app`。原因是当前故障主要是桌面 UI 未接入语音入口，以及 Windows GUI 不应依赖终端工具 `sox` 才能录音。实现复用当前 pinvou 模型/provider 配置和 MiMo-compatible ASR 请求格式，不改 DeepSeek-TUI Engine、Session 或 Commands 路由。

## 实施决策：WebView 录音 + 当前 ASR 配置

**Decision**：在 `tauri-bridge.js` 中使用 WebView 音频采集录制一次性 WAV，调用新增 Tauri command `transcribe_voice_audio`，由后端按当前会话模型/全局模型配置请求 `mimo-v2.5-asr`。

**Rationale**：这样避免 Windows 安装包额外携带 `sox`，也不使用浏览器 Web Speech/外部默认云识别；识别仍沿项目当前显式配置的 OpenAI-compatible endpoint。前端只负责麦克风权限、短时录音、状态机和回填。

**诊断边界**：诊断事件只记录阶段、类别和简短消息，不记录原始音频、完整输入文本、API key 或本地敏感路径。

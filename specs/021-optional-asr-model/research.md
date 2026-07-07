# 研究结论：ASR 模型可选下载

## 决策 1：沿用现有 Linux ASR 按需下载交互契约

**Decision**：跨平台继续使用 `voice_asr_status` 查询状态、`install_voice_asr` 触发安装/下载、`voice_asr:progress` 推送进度，前端继续在用户首次点击语音入口时检测并弹出安装框。

**Rationale**：Linux 已经具备模型按需下载雏形，前端也已经围绕这些 command/event 实现了安装弹窗和进度展示。复用该契约符合“小步改造”要求，避免新增一套 Windows 专用 UI 或并行状态机。

**Alternatives considered**：
- 新增 Windows 专用下载命令：拒绝，会造成前端和平台层双轨维护。
- 启动时自动下载模型：拒绝，违背用户确认和安装包瘦身的产品目标。

## 决策 2：Windows 主包保留小体积 runtime，移除大体积模型

**Decision**：Windows MSI/NSIS 主包保留 `pinvou-asr.exe` 和 `llama-funasr-sensevoice.exe` 等小体积 runtime；不再打包 `sensevoice-small-q8.gguf`。模型通过按需下载落到用户目录。

**Rationale**：当前安装包体积主要来自 `sensevoice-small-q8.gguf`（约 242 MB 原始体积）。保留 runtime 可减少下载完成后的配置复杂度，同时模型移出主包即可达成至少 150 MB 的体积下降目标。

**Alternatives considered**：
- 同时移除 runtime 和模型：拒绝，首次使用 ASR 需要额外处理 backend 安装，范围变大。
- 保留完整 ASR 离线包：拒绝，无法满足安装包瘦身目标。

## 决策 3：模型统一落到 `~/.pinvou3/asr/`

**Decision**：Windows 下载模型也使用 `bridge::paths::pinvou3_home().join("asr")`，与 Linux 的 `voice_asr::asr_dir()` 对齐。

**Rationale**：该目录已经是 Linux ASR 的模型、下载缓存和副产物落点。跨平台统一目录便于状态检测、清理、迁移和用户支持。

**Alternatives considered**：
- 下载到安装目录 `asr/models/`：拒绝，安装目录可能需要管理员权限，且不符合用户数据边界。
- 下载到临时目录：拒绝，重启后状态不可预测。

## 决策 4：Windows 状态从“全量 bundled runtime”调整为“runtime 与模型分离”

**Decision**：Windows 平台层应能分别判断 wrapper/backend 是否存在、用户模型或旧内置模型是否存在；缺模型时 `voice_asr_status` 应返回可安装状态，允许前端触发下载。

**Rationale**：当前 Windows `asr_bundled_runtime_status()` 将 ASR 视为全量随包内置，缺任一文件即提示 repair/reinstall。模型可选后，该布尔语义过粗，需要最小扩展为“runtime 可用但模型缺失可下载”。

**Alternatives considered**：
- 继续用 `repair/reinstall` 文案：拒绝，用户无法通过应用内下载恢复模型。
- 让前端自行判断 Windows 缺模型：拒绝，平台路径细节应留在 Rust 平台层。

## 决策 5：完整性校验先采用长度加固定摘要的最小可信声明

**Decision**：Windows 模型下载应至少校验文件大小和固定摘要；Linux 现有仅校验长度的行为可在本 feature 中顺带补齐到同一校验策略。

**Rationale**：规格要求启用前验证完整性、来源可信度和版本适配性。摘要校验是小改动且可测试的最低可信实现，避免启用损坏模型。

**Alternatives considered**：
- 只校验文件是否存在：拒绝，无法覆盖损坏或部分下载。
- 引入复杂签名/清单系统：暂不采用，超出本次“小改”范围，可作为后续发布安全增强。

## 决策 6：取消下载采用最小可恢复语义

**Decision**：保留前端关闭安装框/失败后重试的既有体验；若实现成本可控，在下载循环中增加取消标记，取消后删除 `.part` 文件并返回可重试状态。

**Rationale**：规格要求用户可取消。当前 Linux ASR 下载尚无专用 cancel command，知识库模型已有取消模式可参考。若任务拆分时发现取消会扩大范围，优先保证失败/关闭/重试不破坏状态，并在任务中标出取消语义的实现边界。

**Alternatives considered**：
- 完全不支持取消：不符合规格。
- 引入下载管理器：拒绝，范围过大。

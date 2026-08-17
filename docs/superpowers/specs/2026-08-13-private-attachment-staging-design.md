# 私有附件解析与暂存设计

## 目标与边界

为 GAIA 等后续 adapter 冻结通用附件契约，同时保持现有无附件 Smoke 行为不变。Adapter 与可持久化报告只接触 `AttachmentHandle`；本地源路径只存在于进程内私有对象，不实现 `Serialize`，`Debug` 固定脱敏。

本批不修改 benchmark-core、CLI 或 adapter。core owner 后续负责在 `prepare` 前调用 resolver，并把已解析 source 放入 `PrepareRequest`。

## API

- `PrivateInputResolver` 新增有安全默认实现的 `resolve_attachment(&AttachmentHandle)`；默认返回固定 `attachment_resolution_unsupported`，既保持 mock/source 兼容，也不伪造附件支持。
- `ResolvedAttachmentSource` 持有受控本地文件路径与建议文件名。它只向 backend 暴露字段，不实现 serde，`Debug` 只显示脱敏占位。
- `PrepareRequest` 保持现有构造器，并增加 builder/accessor 传递已解析 sources。其自定义 `Debug` 只显示附件数量，绝不显示路径或文件名。

## Bridge 暂存生命周期

`ProductHeadlessBackend::prepare` 为含附件的 session 建立 `tempfile::TempDir`。每个 source 必须存在、是普通文件且不是符号链接；建议名必须是单一安全 basename；单文件大小上限 25 MiB。验证通过后复制为 session 私有文件并登记 RAII workspace。

任一解析/校验/复制/runtime prepare 失败都会丢弃 workspace。含附件的 `run` 当前固定返回 `attachments_runtime_unsupported`，因为产品 runtime 尚无把 workspace 注入工具层的能力；返回前清理。`cancel` 与 `close` 无论 runtime 结果如何都清理。无附件路径不创建 workspace，原有行为不变。

## 测试

- agent API：resolver 默认错误固定且不含 handle；resolved source/prepare Debug 不泄露路径和文件名。
- bridge：成功暂存后当前 run 明确 unsupported；拒绝不存在、目录、符号链接、路径穿越名和超大文件；prepare 失败、run、cancel、close 均清理 workspace。
- 只运行 agent API 轻量测试；bridge 使用 feature-gated contract tests，避免反复编译完整 Tauri。

## 剩余门禁

GAIA adapter 不得启用，直到 ProductRuntimePort/产品工具层得到显式 staged-workspace 注入并有端到端测试。仅成功暂存不代表产品已经支持读取附件。

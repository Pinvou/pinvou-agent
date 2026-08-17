# GAIA 产品附件注入设计

## 结论

GAIA validation Level 1 的附件链路可以复用现有产品能力落地，但只有在真实 Engine workspace 注入、产品 ingest/prompt builder 复用、真实工具 E2E 和 Windows no-follow 门禁完成后才能宣称支持。本设计不改变普通 GUI 行为。

## 已有产品路径

GUI 的 `src/platform/tauri/bridge/artifacts.js::addAttachmentByPath` 调用 `ingest_file`，`bridge/chat.js::sendMessage` 把 `IngestResult` 交给 `src-tauri/src/app/commands/chat.rs::chat_with_reservation`。后者通过 `SessionStore::session_roots` 选择账本根和执行根，再调用 `app/commands/attachments.rs::build_message_with_attachments_in_dir`：小文本内联；大文本/表格转换产物落入 workspace 并注入 `read_file`/`exec_shell` 路径；图片落入 workspace 并注入 `image_analyze` 路径。最后经 `EnginePool::send_reserved_user_message` 进入完整 Agent 工具环境。

`CodeWhale/crates/tui/src/tools/file.rs::ReadFileTool` 使用 `ToolContext` 解析路径。Pinvou Yolo/Plan 当前启用 trust mode，但 `CodeWhale/crates/tui/src/vision/tools.rs::resolve_image_path` 仍强制图片是 Engine workspace 内相对路径。因此不能只把 headless TempDir 的绝对路径写进 prompt。

## 最窄接口

保留现有 `ProductRuntimePort::run(session_id, prompt)`，新增有固定安全默认错误的方法：

```rust
async fn run_with_staged_attachments(
    &self,
    session_id: &str,
    prompt: &str,
    staged_workspace: &Path,
) -> Result<ProductTurnOutcome> {
    anyhow::bail!("attachments_runtime_unsupported")
}
```

普通 mock 和无附件 Smoke 不需修改。`ProductHeadlessBackend::run` 从 session map 同步取出 `TempDir`；有附件时在该局部变量仍存活期间 await `run_with_staged_attachments`，无附件仍调用 `run`。future 被取消时 TempDir 析构。

`EnginePoolPort` 是唯一覆盖新方法的生产实现。为取得权威根，新增：

```rust
EnginePoolRuntime::eval_session_execution_root(session_id) -> Result<PathBuf>
EnginePool::eval_session_execution_root(session_id) -> Result<PathBuf>
```

后者只委托 `self.store.session_roots(session_id)?.execution`，不得拼接 `~/.pinvou3/sessions/...`。

## 文件导入与 prompt 构造

把 `app/commands/attachments.rs::stage_image_in_workspace` 的安全通用部分提取为 crate-private `stage_file_in_workspace(src, basename, workspace, attachment_dir)`；原图片函数继续包装它，GUI 调用与返回值不变。

`EnginePoolPort::run_with_staged_attachments` 固定执行：

1. 再次枚举 staged workspace 的直接 regular files，拒绝目录、symlink、非 UTF-8 名和嵌套项；按安全文件名排序。
2. 调 `stage_file_in_workspace(..., execution_root, "attachments")` 导入真实 eval session execution root；不把 TempDir 注册为 Engine workspace，也不增加 trusted external path。
3. 对导入后的路径调用 `features/files/file_ingest.rs::ingest`，复用 20 MiB 上限、私钥拦截和 PDF/Office/XLSX/image 分类。
4. 调现有 `crate::build_message_with_attachments(prompt.to_owned(), results, &execution_root)` 生成产品等价 content。
5. 复用当前 `EnginePoolRuntime::submit(TurnInput { content, ... })` 与完成等待逻辑。

导入后的文件和转换产物随 eval session workspace 由 `delete_eval_session` 删除；失败仍走已有 late sweep。TempDir 在该轮返回或 future drop 时删除。

## 文件所有权

- `pinvou3-app/src-tauri/src/headless_bridge.rs`：ProductRuntimePort 扩展、TempDir 传递、EnginePoolPort 产品附件运行。
- `pinvou3-app/src-tauri/src/features/assistant/product_runtime/mod.rs`：权威 execution root 的窄 facade。
- `pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs`：通过 SessionStore 返回 eval execution root。
- `pinvou3-app/src-tauri/src/app/commands/attachments.rs`：泛化现有安全文件暂存 helper，保持 GUI wrapper 行为。
- `pinvou3-app/src-tauri/tests/headless_bridge_contract.rs`：mock 生命周期与固定 unsupported 默认契约。
- `pinvou3-app/src-tauri/tests/l1_dialog_harness.rs`：真实产品 text/image 附件 E2E（ignore，需 vLLM/本机依赖）。

不修改 `pinvou-cli` core/CLI/adapter，不修改前端 GUI bridge，不修改 CodeWhale。

## 测试与解除门禁

1. helper 单测：安全 basename、重复名、existing target、symlink/junction escape、导入后位于 canonical execution root。
2. product runtime 单测：无附件仍逐字节使用原 prompt；附件固定排序后调用 ingest+builder；摄取 warning 不被吞掉。
3. feature-gated headless contract：TempDir 在 runtime await 期间存在，完成/错误/future abort 后删除；默认 mock 仍返回固定 `attachments_runtime_unsupported`。
4. 真 E2E：小文本答案命中私有随机码；图片必须出现 `image_analyze` 工具事件并命中像素随机码；大 XLSX 必须使用 `read_file`/`exec_shell` 且回答预览之外事实。
5. 回归：现有 GUI attachment 单测与无附件 Smoke 全绿。

只有以上 E2E 通过，并且 Windows 使用平台 handle 的 no-follow/open-by-handle + file identity 校验后，才删除 GAIA 的 `attachments_runtime_unsupported` 能力门禁。若首批 GAIA 明确限定 Linux runner，可在文档和 manifest 标记 Linux-only 后先启用 Level 1；跨平台不得提前宣称闭环。

## 风险

- **HIGH：根权限与生命周期。** 必须由 `SessionStore::session_roots` 提供根；任何错误/取消都不得残留 TempDir 或 eval session。
- **HIGH：Windows TOCTOU。** Rust std 的 metadata 比对不能完全阻止 reparse-point 竞态，当前实现只是缩窄窗口，不是关闭风险。
- **隐私。** builder 会把导入后的 execution path 写入临时 transcript；它不得进入 benchmark report，close 失败必须依赖可观测错误和 late sweep。
- **产品依赖。** PDF/Office 转换依赖本机工具，继续沿用 GUI warning 降级，不能把 warning 当成功读取。
- **一致性。** 必须复用 `ingest` 与 builder；自行拼附件 prompt 会与 GUI 的 token 预算、图片硬规则和安全拦截分叉。

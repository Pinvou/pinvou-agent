# Windows 独立运行时

Windows 大型运行时存放在独立的公开仓库，不直接提交到主仓库的 `resources/` 或 `packaging/`。主仓库只保留：

- `../../../config/platforms/windows/runtime/x86_64.lock.json`：锁定 runtime submodule commit 和 manifest SHA-256；
- `scripts/resolve-runtime.ps1`：校验 submodule、manifest、ZIP 内部清单，原子 staging 并生成 runtime descriptor；
- `scripts/stage-runtime.ps1`：供构建入口调用的轻量包装；
- `scripts/init-submodule.ps1`：初始化 runtime submodule checkout；
- `scripts/stage-onnx-runtime.ps1`：为开发启动展开 ONNX Runtime 组件；
- `scripts/test-onnx-runtime.ps1`：验证 ONNX Runtime staging 产物。

发布机构建前执行：

```powershell
npm run runtime:windows:init
npm run runtime:windows:validate
npm run runtime:windows:stage
npm run test:windows-runtime-manifest
```

Windows 桌面开发先执行一次 `npm run runtime:windows:init:onnx`。该命令初始化固定版本的 runtime checkout，
但只拉取 ONNX Runtime 的 Git LFS 对象。此后 `npm run dev` 只校验并展开
ONNX Runtime 组件到 `target/windows-runtime/<commit>-<manifest-sha>-onnx-dev/`，不会为开发启动
展开 Node、Python、ASR、OCR、Pandoc 等完整安装包资源；结果带指纹缓存，后续启动直接复用。

`runtime:windows:init` 只在当前 checkout 与主仓库 gitlink 不一致时更新 runtime submodule；gitlink 未变化时直接复用。
初始化与 runtime staging 共用 `target/windows-runtime/.resolver.lock`（120 秒超时）串行化，避免并发调用相互覆盖缓存。
脚本会检查受 LFS 管理的实际文件，只有仍存在 pointer 时才按路径执行 `git lfs pull`，并输出
`pinvou3-windows-runtime-v5-<commit>-<lock-sha-prefix>` 形式的 Jenkins 缓存键；lock 文件内容变化或缓存 schema
升级时不会复用旧缓存，lock 文件缺失时脚本直接失败。并发调用等待 `.resolver.lock` 最多 120 秒，超时即失败退出，
稍后重试即可。

校验内容包括 submodule commit、gitlink、origin URL、工作树状态、manifest SHA-256、文件大小与 SHA-256，以及受管理 ZIP 解压后的逐文件清单。解析器支持 schema 1 与 schema 2：schema 2 的 `stagedFiles` 记录生命周期快照，取自 payload 复制、各组件解压、按需分发的 ASR 主模型移除之后，派生文件生成与 payload 清理之前；校验会同时拒绝缺失与多余的暂存文件。

完整校验通过后，脚本会在 `target/windows-runtime/.verified-lock` 记录 runtime commit、manifest SHA-256、lock 文件
SHA-256 和目标平台，作为 submodule 源校验元数据缓存：Stage 模式命中缓存时只按 lock 复核源文件大小，跳过逐文件
SHA-256；Validate 模式仍对全部锁定源文件重新哈希。lock 文件或 submodule 身份（commit、gitlink、origin、工作树状态）
变化都会使该缓存失效；身份校验失败会直接报错终止，通过后才执行上一句所述的 Stage 大小复核或 Validate 整体重哈希。
staging 内的 `.verified-lock` 绑定 runtime commit、manifest SHA-256、lock 文件 SHA-256 和目标平台，`.verified-stage.json`
则记录全部展开文件的路径、大小和 SHA-256；任一暂存产物变化或使用 `-Force`，都会自动回退到原子 staging。当仅缓存元数据
命中而暂存产物需要重建时，会先对源文件重新执行完整 SHA-256 校验再复制。runtime descriptor、Tauri overlay 和 ONNX 开发
descriptor 以临时文件加原子替换写入，避免并发或中断留下半写文件。已验证并解包的 payload 以及 ASR 主模型不会留在
staging 中。

验证后的运行时写入：

```text
src-tauri/target/windows-runtime/<commit>-<manifest-sha>/
```

生成的 Tauri overlay 位于 `src-tauri/target/windows-runtime/tauri.generated.conf.json`，构建工具路径和安装器输入由同目录的
`runtime-descriptor.json` 描述。ASR 主模型采用首次启用时下载策略，不进入安装资源映射；VC Runtime 只作为通用组件展开，
由 NSIS installer adapter 按目标消费。公共 `tauri.conf.json` 不引用大型运行时资源，因此未初始化 runtime submodule 时仍可执行 `cargo check`。

`PINVOU3_WINDOWS_RUNTIME_ROOT` 只用于本地迁移或故障排查，且目标仓库的 commit 必须与 lock 完全一致；正式 CI 和发布始终使用 submodule 默认路径。

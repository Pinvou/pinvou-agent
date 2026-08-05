# Windows 独立运行时

Windows 大型运行时存放在独立的公开仓库，不直接提交到主仓库的 `resources/` 或 `packaging/`。主仓库只保留：

- `../../../config/platforms/windows/runtime/x86_64.lock.json`：锁定 runtime submodule commit 和 manifest SHA-256；
- `scripts/resolve-runtime.ps1`：校验 submodule、manifest、ZIP 内部清单，原子 staging 并生成 runtime descriptor；
- `scripts/stage-runtime.ps1`：供构建入口调用的轻量包装。

发布机构建前执行：

```powershell
npm run runtime:windows:init
npm run runtime:windows:validate
npm run runtime:windows:stage
```

`runtime:windows:init` 只在当前 checkout 与主仓库 gitlink 不一致时更新 runtime submodule；gitlink 未变化时直接复用。
脚本会检查受 LFS 管理的实际文件，只有仍存在 pointer 时才按路径执行 `git lfs pull`，并输出
`pinvou3-windows-runtime-<commit>` 形式的 Jenkins 缓存键。

校验内容包括 submodule commit、gitlink、origin URL、工作树状态、manifest SHA-256、文件大小与 SHA-256，以及受管理 ZIP 解压后的逐文件清单。

每次构建都会按 runtime manifest 复核源文件的大小和 SHA-256。staging 内的 `.verified-lock` 绑定 runtime commit、manifest
SHA-256、lock 文件 SHA-256 和目标平台，`.verified-stage.json` 则记录全部展开文件的路径、大小和 SHA-256；任一暂存产物变化
或使用 `-Force`，都会自动回退到原子 staging。已验证并解包的 payload 以及 ASR 主模型不会留在 staging 中。

验证后的运行时写入：

```text
src-tauri/target/windows-runtime/<commit>-<manifest-sha>/
```

生成的 Tauri overlay 位于 `src-tauri/target/windows-runtime/tauri.generated.conf.json`，构建工具路径和安装器输入由同目录的
`runtime-descriptor.json` 描述。ASR 主模型采用首次启用时下载策略，不进入安装资源映射；VC Runtime 只作为通用组件展开，
由 NSIS installer adapter 按目标消费。公共 `tauri.conf.json` 不引用大型运行时资源，因此未初始化 runtime submodule 时仍可执行 `cargo check`。

`PINVOU3_WINDOWS_RUNTIME_ROOT` 只用于本地迁移或故障排查，且目标仓库的 commit 必须与 lock 完全一致；正式 CI 和发布始终使用 submodule 默认路径。

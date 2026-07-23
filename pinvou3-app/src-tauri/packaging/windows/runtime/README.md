# Windows 私有运行时

Windows 私有运行时不存放在主仓库的 `resources/` 或 `packaging/` 中。主仓库只保留：

- `../../../config/platforms/windows/runtime/x86_64.lock.json`：锁定私有 submodule commit 和 manifest SHA-256；
- `scripts/resolve-runtime.ps1`：校验 submodule、manifest、ZIP 内部清单并原子 staging；
- `scripts/stage-runtime.ps1`：供构建入口调用的轻量包装。

发布机构建前执行：

```powershell
git submodule update --init -- private-runtimes/windows
git -C private-runtimes/windows lfs pull
npm run runtime:windows:validate
npm run runtime:windows:stage
```

校验内容包括 submodule commit、gitlink、origin URL、工作树状态、manifest SHA-256、文件大小与 SHA-256，以及受管理 ZIP 解压后的逐文件清单。

验证后的运行时写入：

```text
src-tauri/target/windows-runtime/<commit>-<manifest-sha>/
```

生成的 Tauri overlay 位于 `src-tauri/target/windows-runtime/tauri.generated.conf.json`。公共 `tauri.conf.json` 不引用私有资源，因此未初始化私有 submodule 时仍可执行 `cargo check`。

`PINVOU3_WINDOWS_RUNTIME_ROOT` 只用于本地迁移或故障排查，且目标仓库的 commit 必须与 lock 完全一致；正式 CI 和发布始终使用 submodule 默认路径。

# Windows runtime resources

Windows 私有运行时不存放在主仓库的 `resources/windows/` 中。主仓库只保留：

- `../windows-runtime.lock.json`：锁定私有 submodule commit 和 manifest SHA-256；
- `main.wxs`、`python-node-path.wxs`、`nsis/`：可审查、可合并的安装器配置；
- `scripts/windows-runtime-submodule.ps1`：逐文件校验与原子 staging 入口。

Windows 发布机构建前初始化私有 submodule 和 Git LFS：

```powershell
git submodule update --init -- private-runtimes/windows
git -C private-runtimes/windows lfs pull
npm run runtime:windows:validate
npm run runtime:windows:stage
```

解析器依次验证：

1. submodule `HEAD` 等于主仓库 lock 中的 commit；
2. submodule 工作树没有 tracked 修改；
3. 私有 manifest 的 SHA-256 等于主仓库 lock；
4. manifest 中每个文件的大小与 SHA-256 正确，且不是未展开的 LFS pointer。
5. 7-Zip、ASR、Poppler、Tesseract 解压后的文件数、大小与 SHA-256 符合 ZIP 内部清单。

验证完成后，运行时会被复制、展开到：

```text
src-tauri/target/windows-runtime/<commit>-<manifest-sha>/
```

并生成被 Git 忽略的 `src-tauri/tauri.windows-runtime.generated.conf.json`。基础
`tauri.conf.json` 不引用私有资源，因此没有初始化 submodule 时仍能执行 `cargo check`。

仅用于本地迁移或故障排查时，可通过 `PINVOU3_WINDOWS_RUNTIME_ROOT` 指向另一个、commit
与 lock 完全一致的私有仓库工作树；正式 CI 和发布应始终使用 submodule 默认路径。

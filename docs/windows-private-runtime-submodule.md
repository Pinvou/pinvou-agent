# Windows 私有运行时 submodule

## 目标

Windows 分支只维护代码、安装器模板和 submodule gitlink。Poppler、Tesseract、Python、
Node.js、Pandoc、ONNX Runtime、ASR、7-Zip 和 VC Runtime 存放在私有仓库：

```text
https://github.com/Pinvou/pinvou3-windows-runtime.git
```

主仓库不保存这些二进制，也不在基础 `tauri.conf.json` 中硬编码私有资源路径。

## 目录和版本锁定

主仓库引用：

```text
private-runtimes/windows  -> 私有仓库的确定 commit
```

私有仓库的 `payload/` 按组件保存资源。Poppler、Tesseract、ASR、7-Zip 分别使用一个
确定性 ZIP；Python、Node.js、Pandoc、ONNX Runtime 继续使用各自的上游组件包；
VC Runtime 保持独立文件。二进制由 Git LFS 管理。

`windows-runtime.manifest.json` 当前锁定 9 个仓库级资源，并额外记录四个组件 ZIP 内部
113 个 7-Zip/Poppler/Tesseract/ASR 文件的路径、大小和 SHA-256。staging 会先校验 ZIP，
解压后再逐文件复核内部清单。

主仓库 `pinvou3-app/src-tauri/config/runtime/windows-x86_64.lock.json` 再锁定：

- submodule URL、路径和 commit；
- 私有 manifest 路径及 SHA-256；
- 目标平台 `windows-x86_64`。

因此版本完整性由三层保证：主仓库 gitlink、主仓库 lock、私有文件级 manifest。

## 初始化和构建

```powershell
git submodule update --init -- private-runtimes/windows
git -C private-runtimes/windows lfs pull
cd pinvou3-app
npm run runtime:windows:validate
npm run runtime:windows:stage
npm run build:msi
```

`tauri-build-with-secrets.js` 在 Windows 的 `tauri build` / `tauri bundle` 前自动执行
staging，并把生成的 Tauri config overlay 传给 CLI。

迁移验证阶段可设置 `PINVOU3_WINDOWS_RUNTIME_ROOT` 指向相同 commit 的本地私有仓库；
正式发布不得用它绕过主仓库锁定的 submodule。

普通开发和检查不需要私有仓库：

```powershell
cd pinvou3-app/src-tauri
cargo check
```

## 增量更新资源

1. 在临时目录展开并修改对应组件；
2. 使用 `scripts/pack-component.ps1` 重新生成对应的确定性 ZIP；
3. 执行 `scripts/update-manifest.ps1`；
4. 提交并推送私有仓库，只有变更组件产生新的 LFS 对象；
5. 主仓库更新 submodule gitlink；
6. 更新主仓库 lock 中的 commit 和 manifest SHA-256；
7. 从干净 checkout 验证 staging 和安装包。

不得使用 `git submodule update --remote` 参与发布构建，也不得只更新 gitlink 而不更新
lock。若 Git LFS 没有拉取成功，resolver 会识别 pointer 文件并在打包前失败。

## 多平台扩展

macOS 私有运行时应使用独立路径和仓库，例如：

```text
private-runtimes/windows
private-runtimes/macos
```

这样 Windows 与 macOS 更新不会争用同一个 gitlink，主线仍只处理平台无关代码和少量
平台适配配置。

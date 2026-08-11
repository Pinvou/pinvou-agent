# Windows 独立运行时 submodule

## 目标

Windows 分支只维护代码、安装器模板和 submodule gitlink。Poppler、Tesseract、Python、
Node.js、Pandoc、ONNX Runtime、ASR 引擎、7-Zip 和 VC Runtime 存放在公开仓库：

```text
https://github.com/Pinvou/pinvou3-windows-runtime.git
```

主仓库不保存这些大型二进制，也不在基础 `tauri.conf.json` 中硬编码平台资源路径。
`private-runtimes/windows` 是为保持 gitlink、缓存和构建脚本兼容而保留的历史路径名，
不代表当前仓库可见性。

## 目录和版本锁定

主仓库引用：

```text
private-runtimes/windows  -> 公开运行时仓库的确定 commit
```

运行时仓库的 `payload/` 按组件保存资源。Poppler、Tesseract、ASR、7-Zip 分别使用一个
确定性 ZIP；Python、Node.js、Pandoc、ONNX Runtime 继续使用各自的上游组件包；
VC Runtime 保持独立文件。二进制由 Git LFS 管理。

`windows-runtime.manifest.json` 当前锁定 9 个仓库级资源，并额外记录四个组件 ZIP 内部
113 个 7-Zip/Poppler/Tesseract/ASR 文件的路径、大小和 SHA-256。staging 会先校验 ZIP，
解压后再逐文件复核内部清单。Windows 安装包不映射 `sensevoice-small-q8.gguf`，应用在用户首次启用本地语音识别时
下载并校验到用户目录；resolver 在复核归档后会从 staging 删除主模型和已解包的 payload，只保留 ASR 引擎与 VAD 模型。

主仓库 `pinvou3-app/src-tauri/config/platforms/windows/runtime/x86_64.lock.json` 再锁定：

- submodule URL、路径和 commit；
- runtime manifest 路径及 SHA-256；
- 目标平台 `windows-x86_64`。

因此版本完整性由三层保证：主仓库 gitlink、主仓库 lock、运行时文件级 manifest。

## 初始化和构建

```powershell
cd pinvou3-app
npm run runtime:windows:init
npm run runtime:windows:validate
npm run runtime:windows:stage
npm run build:nsis
```

运行时 submodule 配置为 `update = none`，普通的递归 submodule 初始化只处理公共底座，
Windows 构建机通过 `runtime:windows:init` 显式覆盖该策略并拉取 Git LFS 对象。

### Jenkins 子模块准备

Jenkins 不应执行无路径限制的 `git submodule sync` 或 `git submodule update --init --recursive`。构建阶段只准备实际需要的两个子模块：

```powershell
git submodule update --init --checkout -- CodeWhale
npm --prefix pinvou3-app run runtime:windows:init
```

`CodeWhale` 当前没有嵌套 submodule，因此不需要 `--recursive`。`runtime:windows:init` 会比较主仓库 gitlink 与本地 runtime
commit；一致且工作树中不存在 LFS pointer 时，跳过 submodule update 和 `git lfs pull`。脚本输出的
`PINVOU3_WINDOWS_RUNTIME_CACHE_KEY=pinvou3-windows-runtime-<commit>` 可作为 Jenkins 缓存键，缓存 runtime checkout、
Git LFS objects 和 `pinvou3-app/src-tauri/target/windows-runtime`。

Jenkins 的 `CheckoutSubmodule` 阶段只负责准备 submodule，不再单独执行 `runtime:windows:validate`。后续直接运行
`npm --prefix pinvou3-app run build:nsis`：统一的 `build.js` 入口只执行一次 runtime 校验和 staging，读取
`runtime-descriptor.json`，再由 Windows installer adapter 为 NSIS 原子准备经过校验的 VC++ 引导程序。

恢复缓存前可直接从主仓库 gitlink 取得缓存键，不会访问 runtime 远端：

```powershell
npm --prefix pinvou3-app run runtime:windows:cache-key
```

建议缓存 `.git/modules/private-runtimes/windows/lfs/objects` 与 `pinvou3-app/src-tauri/target/windows-runtime`；前者避免重复下载
LFS 对象，后者复用已验证、已展开的安装资源。每次构建都会按 runtime manifest 重新核对源文件 SHA-256；复用 staging 前还会按
`.verified-stage.json` 对全部会进入构建链路的展开文件重新核对路径、大小和 SHA-256，缓存内容发生缺失或修改时自动回退到原子 staging。

主仓库 checkout 的 refspec、tag 获取和 shallow clone 由 Jenkins SCM 插件控制，不在仓库脚本内。发布 Job 应启用
`Honor refspec on initial clone`、`No tags`，并把 refspec 限制为实际构建分支或 MR ref；不要在每次构建中抓取全部分支和 tag。

GitHub Actions 构建 Windows NSIS 时仍进入 `windows-release` Environment，作为正式发布边界；
job 自身只允许 `main`，Environment 可按发布策略进一步配置 protected branch 和 required reviewers。
运行时仓库已公开，不需要仓库级、组织级或 Environment Secret。checkout 设置
`persist-credentials: false`，初始化脚本按锁定 URL、gitlink 和 manifest 匿名拉取运行时及
Git LFS 对象。

所有 PR（包括同仓 PR）只运行不拉取大型运行时的 Windows 打包契约测试。只有 `main` 的发布链路
push 或在 `main` 上执行的 `workflow_dispatch` 才能进入 `windows-release` 构建正式 NSIS；
运行时缺失、gitlink 不一致或 LFS 对象未物化时仍会明确失败，避免产生不完整发布。

`scripts/tauri/build.js` 是项目内 `tauri build` / `tauri bundle` 的统一入口：Windows
构建前自动执行 staging，所有平台都会加载对应 config overlay，并在调用 Tauri CLI 前
生成有效合并配置和安装包资源清单。不要直接运行 `npx tauri build/bundle`。
基础 Tauri 配置的构建/打包钩子会拒绝未经过包装器的调用。

resolver 只负责验证、展开运行时并生成 `target/windows-runtime/runtime-descriptor.json`；
`scripts/tauri/windows-installer.js` 只在目标包含 NSIS 时消费 descriptor 中的 VC Runtime。
Codex Bridge 同样从 descriptor 取得已锁定 Node，不反向解析 Tauri 资源映射。

迁移验证阶段可设置 `PINVOU3_WINDOWS_RUNTIME_ROOT` 指向相同 commit 的本地运行时仓库；
正式发布不得用它绕过主仓库锁定的 submodule。

普通开发和检查不需要初始化运行时仓库：

```powershell
cd pinvou3-app/src-tauri
cargo check
```

## 增量更新资源

1. 在临时目录展开并修改对应组件；
2. 使用 `scripts/pack-component.ps1` 重新生成对应的确定性 ZIP；
3. 执行 `scripts/update-manifest.ps1`；
4. 提交并推送运行时仓库，只有变更组件产生新的 LFS 对象；
5. 主仓库更新 submodule gitlink；
6. 更新主仓库 lock 中的 commit 和 manifest SHA-256；
7. 从干净 checkout 验证 staging 和安装包。

不得使用 `git submodule update --remote` 参与发布构建，也不得只更新 gitlink 而不更新
lock。若 Git LFS 没有拉取成功，resolver 会识别 pointer 文件并在打包前失败。

## 多平台扩展

macOS 平台运行时应使用独立路径和仓库，例如：

```text
private-runtimes/windows
private-runtimes/macos
```

这样 Windows 与 macOS 更新不会争用同一个 gitlink，主线仍只处理平台无关代码和少量
平台适配配置。

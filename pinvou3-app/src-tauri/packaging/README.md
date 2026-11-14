# 平台打包目录

这里仅保存社区构建需要的可审查安装资源；大型运行时不得直接提交到本目录。

```text
packaging/
├─ linux/
│  └─ deb/                 # desktop、postinst、prerm
├─ macos/                  # 社区版打包配置
└─ windows/
   ├─ nsis/                # NSIS 模板与 installer hooks(基于 tauri-bundler 2.12.x 模板)
   │  ├─ installer-template.nsi   # 自定义模板:staged 资源宏、EstimatedSize 补偿、dev 工具排除
   │  ├─ installer-hooks.nsh      # VC++ 预检 + PINVOU3_PYTHON 机器环境安装/卸载
   │  ├─ runtime-env.ps1          # 安装期机器环境写入(由 installer hooks 打包执行)
   │  ├─ vcredist-temp-preflight.ps1
   │  └─ scripts/          # clean-staging / stage-resources(生成 staged hooks 与 staging 配置)
   ├─ wix/                 # MSI:main.wxs 模板 + python-node-path.wxs(PINVOU3_PYTHON 环境组件)
   └─ runtime/             # 独立 runtime 的锁校验、descriptor 与原子 staging 脚本
```

Windows NSIS 构建流水线(`npm run build:nsis`):

1. `prebuild:nsis` → `clean-staging.ps1` 清理 `target/release` 中会被误当作外部二进制的运行时残留;
2. `build:nsis:stage` → `build.js build --no-bundle` 编译主程序(期间生成 runtime overlay),`stage-resources.ps1`
   把预签名 runtime 资源复制到 `target/release/nsis-stage/`,生成 `installer-hooks.nsh`(安装/卸载宏)与
   `tauri.nsis-stage.conf.json`(以 null 移除原 Tauri 资源映射,绕过其资源枚举与校验);
3. `bundle:nsis` → `stage-resources.ps1 -ValidateOnly` 校验 stage 与 hooks 一致后,`build.js bundle --bundles nsis`
   以 staged 配置驱动 Tauri 使用自定义 NSIS 模板与生成的 hooks。

`runtime-env.ps1` 在 NSIS 安装/卸载时设置/清除机器级 `PINVOU3_PYTHON` 并清理 PATH 中的私有 runtime 项,
与 MSI 的 `python-node-path.wxs` 环境组件对齐。MSI 卸载默认保留用户数据(`~/.pinvou3`);显式传入
`PINVOU_REMOVE_USER_DATA=1`(如 `msiexec /x <msi> PINVOU_REMOVE_USER_DATA=1`)才会删除。

公共构建编排位于 `../../scripts/tauri/`：

- `platform-config.js`：只负责选择当前平台 overlay。
- `build.js`：组合平台配置并启动 Tauri CLI。
- `windows-runtime.js`：读取 runtime descriptor，不感知安装器细节。
- `windows-installer.js`：按 bundle 目标准备 NSIS 专属资源。
- `codex-bridge.js`：准备应用隔离的 Codex ACP Bridge 运行时。
- `knowledge-host.js`：准备 pinvou-knowledge-host-helper 运行时资源。

以上清单以 `scripts/tauri/` 目录为准。

工具市场不在构建期注入共享 API Key。社区构建从公开、锁定的独立仓库取得 Windows 运行时，
不包含私有凭据或官方签名工具。

平台脚本不得修改其他平台的资源树；所有生成物必须写入 `src-tauri/target/`。Windows
安装包通过 `private-runtimes/windows` 的锁定 gitlink 合入运行时，协议和初始化方式见
[`docs/windows-private-runtime-submodule.md`](../../../docs/windows-private-runtime-submodule.md)。

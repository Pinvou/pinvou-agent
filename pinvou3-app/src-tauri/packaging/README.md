# 平台打包目录

这里仅保存社区构建需要的可审查安装资源；大型或私有运行时不得提交到本目录。

```text
packaging/
├─ linux/
│  └─ deb/                 # desktop、postinst、prerm
├─ macos/                  # 社区版打包配置
└─ windows/
   ├─ nsis/                # 最小化 NSIS 安装 hook
   └─ runtime/             # 私有 runtime 的锁校验、descriptor 与原子 staging 脚本
```

公共构建编排位于 `../../scripts/tauri/`：

- `platform-config.js`：只负责选择当前平台 overlay。
- `build.js`：组合平台配置并启动 Tauri CLI。
- `windows-runtime.js`：读取 runtime descriptor，不感知安装器细节。
- `windows-installer.js`：按 bundle 目标准备 NSIS 专属资源。

工具市场不在构建期注入共享 API Key。社区构建不包含私有运行时或官方签名工具。

平台脚本不得修改其他平台的资源树；所有生成物必须写入 `src-tauri/target/`。Windows
安装包通过 `private-runtimes/windows` 的锁定 gitlink 合入运行时，协议和初始化方式见
[`docs/windows-private-runtime-submodule.md`](../../../docs/windows-private-runtime-submodule.md)。

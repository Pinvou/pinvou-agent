# 平台打包目录

这里仅保存可审查的安装器模板、签名工具入口和构建期 staging 脚本；大型或私有运行时不得提交到本目录。

```text
packaging/
├─ linux/
│  └─ deb/                 # desktop、postinst、prerm
├─ macos/                  # 后续放置 entitlements、公证和 DMG 定制
└─ windows/
   ├─ wix/                 # MSI/WiX 模板和 fragments
   ├─ nsis/                # NSIS 模板、hooks、环境脚本和 staging
   ├─ runtime/             # 私有 runtime 校验、展开与 Tauri overlay 生成
   └─ signing/             # 发布签名实现
```

公共构建编排位于 `../../scripts/tauri/`：

- `platform-config.js`：只负责选择当前平台 overlay。
- `builtin-secrets.js`：只负责构建密钥加载与校验。
- `windows-runtime.js`：只在 Windows 构建时触发私有 runtime staging。
- `build.js`：组合以上能力并启动 Tauri CLI。

平台脚本不得修改其他平台的资源树；所有生成物必须写入 `src-tauri/target/`。

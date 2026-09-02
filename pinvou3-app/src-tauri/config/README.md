# Tauri 平台配置

`../tauri.conf.json` 是所有平台共享的唯一基础配置。本目录只保存按目标平台叠加的 overlay 和构建锁：

```text
config/
└─ platforms/
   ├─ linux/tauri.conf.json
   ├─ macos/tauri.conf.json
   └─ windows/
      ├─ tauri.conf.json
      └─ runtime/x86_64.lock.json
```

- `tauri.conf.json`：对应平台的安装包目标、资源映射和安装器参数；macOS 额外把主窗口覆盖为原生顶栏（`decorations: true` + `titleBarStyle: "Overlay"` + `hiddenTitle`，系统红绿灯替代前端自绘三键）。
- `windows/runtime/x86_64.lock.json`：锁定 Windows 独立 runtime 的 submodule commit、manifest 与目标架构；实际资源映射由 staging 后生成的 overlay 提供。

`--config` overlay 按 JSON Merge Patch 合并：对象合并、**数组整体替换**。
macOS overlay 中的 `app.windows` 是基础配置主窗口定义的完整拷贝，改动基础配置 `app.windows` 字段时必须同步。Linux 的隐藏启动 overlay 由 `scripts/tauri/startup-window-config.js` 从基础配置动态生成，开发和 packaging 共用，不重复维护窗口宽高等字段。

`scripts/tauri/build.js` 根据当前操作系统在 **build / bundle** 时加载 `platforms/<os>/tauri.conf.json`
（Linux 的 dev 与 packaging 都会额外注入动态隐藏启动 overlay，Windows dev 不注入，见 `tests/tauri_effective_config.test.js`）；macOS
例外：原生顶栏定义在 overlay 里，因此 dev（`npm run dev` 与 `run-dev.sh` 两条入口）都会以 `--config`
带上同一份 overlay，保证 dev 与打包产物顶栏一致。公共配置不得引用私有签名工具或直接硬编码大型平台运行时。

Windows 例外由统一构建入口先验证并 staging `private-runtimes/windows`，再加载
`target/windows-runtime/tauri.generated.conf.json`；Node、VC Runtime 和 ASR 模型交付策略通过同目录的
`runtime-descriptor.json` 交给构建适配层。未初始化或锁不匹配时明确失败，不会静默生成缺少运行时的安装包。

项目内所有 `build` / `bundle` 命令必须通过该包装器执行；`npm run tauri -- build`、
`npm run build`、`npm run build:msi` 和 NSIS 脚本均遵循同一入口。包装器将自动 overlay
放在显式 overlay 之前，并在 `target/tauri-config/<platform>/` 生成：

- `effective-config.json`：按 Tauri JSON Merge Patch 顺序得到的有效配置；
- `installer-resources.manifest.json`：已验证存在、目标路径无冲突的最终资源文件清单。

基础配置的 `beforeBuildCommand` / `beforeBundleCommand` 会拒绝没有包装器标记的构建。
不要直接运行 `npx tauri build` 或 `npx tauri bundle`，否则命令会在打包前明确失败。

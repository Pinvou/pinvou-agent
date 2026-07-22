# Tauri 平台配置

`../tauri.conf.json` 是所有平台共享的唯一基础配置。本目录只保存按目标平台叠加的 overlay 和构建锁：

```text
config/
└─ platforms/
   ├─ linux/tauri.conf.json
   ├─ macos/tauri.conf.json
   └─ windows/
      ├─ tauri.conf.json
      ├─ signing.wosign.conf.json
      └─ runtime/x86_64.lock.json
```

- `tauri.conf.json`：对应平台的安装包目标、资源映射和安装器参数。
- `signing.*.conf.json`：发布签名 overlay，仅由对应发布流程显式加载。
- `runtime/*.lock.json`：锁定私有运行时来源和 manifest，不存放制品本身。

`scripts/tauri/build.js` 根据当前操作系统加载 `platforms/<os>/tauri.conf.json`。公共配置不得引用平台专属安装器模板、签名工具或私有运行时。

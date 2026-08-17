# Tauri 资源目录

资源按“是否跨平台”分层，源码位置与安装后的目标路径是两个独立概念：

```text
resources/
├─ common/
│  ├─ bundle/             # 编译进应用并释放到 ~/.pinvou3/bundle
│  └─ skill-marketplace/  # 编译期内嵌的技能市场索引
└─ platforms/
   └─ linux/asr/          # 仅由 Linux overlay 打包的 ASR 启动资源
```

- `common/` 中不得放入仅在单一操作系统可用的二进制或安装脚本。
- `platforms/<os>/` 只能由对应的 `config/platforms/<os>/tauri.conf.json` 引用。
- 社区仓库不包含任何私有运行时；平台依赖必须来自可再分发的开源组件或由用户自行安装。
- Tauri `bundle.resources` 的目标路径保持稳定，源码目录重组不应改变运行时查找协议。

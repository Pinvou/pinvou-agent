# 架构门禁

`scripts/architecture-guard.py` 检查 Rust、前端和平台适配边界。它只使用 Python
标准库，可在本地和 CI 中运行：

```bash
python scripts/architecture-guard.py
```

## 强制规则

- 前端 feature 不得 import `src/app`。
- `window.__TAURI__` / `globalThis.__TAURI__` 只能出现在 `src/platform`。
- 禁止通过 `navigator.userAgent`、`navigator.userAgentData` 或
  `navigator.platform` 判断平台。
- Rust feature 不得反向依赖 `app`。
- Rust `platform` 不得反向依赖 `app` 或 `features`。
- Rust feature 不得新增或扩大循环依赖，也不得增加现有循环内部的依赖边和引用。
- 平台 `target_os` 分支应位于 platform adapter 或组合根；PowerShell、注册表、
  `xdg-open` 等实现细节应位于 platform adapter。合理例外按下文显式登记。
- `#[tauri::command]` 和 `tauri::generate_handler!` 必须保持在 app 宿主边界内。
- `resources/common` 不得混入 PE、ELF 等平台专属二进制。

门禁只约束可以稳定、客观检测的架构边界，不以文件行数或仓库总代码量作为模块化
合规条件。模块是否需要拆分，应依据职责、依赖、状态耦合和独立测试能力，遵循根目录
`AGENTS.md` 的模块化开发公约进行评审。

## 显式例外

平台条件编译和平台实现细节规则允许少量、可评审的文件级例外。确有合理场景时，在
文件前 20 行内分别使用以下注释，并在 `--` 后写明具体理由：

```rust
// architecture-guard: allow-target-cfg -- 第三方类型仅在 Windows 目标存在
// architecture-guard: allow-platform-detail -- 此处只负责解析外部平台标识
```

例外必须保持最小范围并补充覆盖该场景的测试；缺少理由、位于文件头之外或仅为绕过
模块迁移而添加的标记不会被接受。`include!` 和 `#[path]` 本身不再作为架构违规；是否
合理由实际职责、依赖方向和生成代码边界判断。

## 历史基线

`scripts/architecture-baseline.json` 记录启用门禁时已经存在的架构债务。门禁不允许：

- 在新文件或新依赖边中出现同类违规；
- 增加现有违规数量；
- 新建或扩大 feature 依赖环。

修复历史债务后，必须在同一变更中下调或删除对应基线；否则门禁会报告基线过期并
失败。这使基线成为严格棘轮，已经删除的违规不能在后续变更中恢复。

PR CI 还会通过 `--base-ref` 对比目标分支中的基线。当前分支不能增加目标分支已有的
额度、增加新条目或扩大依赖环，因此同时修改当前基线不能绕过门禁。首次引入基线时，
目标分支尚无该文件，脚本会明确报告初始化并允许通过。

查看当前扫描结果：

```bash
python scripts/architecture-guard.py --print-current
```

不要直接用该输出覆盖基线来绕过失败。清债时只能下调相关条目。若确需改变门禁政策，
必须作为独立架构决策修改规则和文档；单纯提高基线会被 CI 拒绝。

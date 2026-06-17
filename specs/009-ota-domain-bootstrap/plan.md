# 实施计划：Windows OTA 域名引导

**分支**：`011-ota-domain-bootstrap` | **日期**：2026-06-17 | **规格**：[spec.md](./spec.md)

**输入**：来自 `specs/009-ota-domain-bootstrap/spec.md` 的功能规格

**说明**：本计划遵守 `.specify/memory/constitution.md` 中的项目宪章。

## 概要

本 feature 将 Windows OTA 后台地址从固定默认地址调整为“域名引导结果”。Windows 用户点击“检查更新”或应用执行静默更新检查时，应用先读取可编辑配置中的域名引导后台地址，默认使用 `https://bootstrap.magic.h3c.com`，再使用设备 BIOS SN 或指定固定 SN 请求 `/v2/bootstrap`，从返回的 `data.smarthubOta` 取出本次 OTA 后台地址。随后查询更新、下载完整包和反馈升级结果都使用该 OTA 来源。

实现边界保持在 `pinvou3-app` 的 Windows OS 分支内：新增 Windows 专用域名引导模块，改造 `windows_update.rs` 的 `OtaConfig` 解析路径，并在待反馈记录中保存本次解析出的 OTA host，保证升级后首次启动反馈仍指向安装前同一 OTA 来源。不修改 DeepSeek-TUI 底座，不改变 Linux 更新流程，也不新增配置 UI；配置通过 `~/.pinvou3/windows-ota-bootstrap.json` 外部文件完成。

## 技术上下文

**语言/版本**：Rust 2021（`pinvou3-app/src-tauri/Cargo.toml` 当前 `rust-version = "1.88"`）；JavaScript 前端保持既有更新面板调用方式

**主要依赖**：Tauri 2、现有 `updater.rs` 命令层、`os/windows/windows_update.rs` Windows OTA 实现、`reqwest` HTTP/JSON、`serde_json` 配置与响应解析、`md5` 签名、`chrono` 时间、Windows 本地 BIOS SN 读取能力；不新增 DeepSeek-TUI 依赖

**存储**：新增用户可编辑配置文件 `~/.pinvou3/windows-ota-bootstrap.json`；继续使用 `~/.pinvou3/updates/` 保存下载包、解压文件和 `update-feedback.json`；待反馈记录新增 OTA host 字段以跨版本保留反馈目标

**测试**：`cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_update --lib`、`cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml updater --lib`、`cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`、Windows 更新面板手动 smoke、mock 域名引导服务联调、升级后反馈手动验收

**目标平台**：Windows 桌面为本 feature 主目标；Linux 和其他非 Windows 平台 OTA 行为保持不变

**项目类型**：desktop-app

**性能目标**：域名引导失败或缺少 `smarthubOta` 时，手动检查更新 15 秒内返回友好失败提示；域名引导成功时不额外改变现有 OTA 下载/安装体验；BIOS SN 读取不得造成设置页或更新面板长时间卡顿

**约束**：中文文档优先；尽量只改 Windows 系统相关代码；不重写 DeepSeek-TUI 底座；域名引导是产品更新场景下的显式外部网络调用；SN 不在用户提示或普通日志中完整暴露；配置文件缺失、为空或格式非法时自动使用默认值但不得覆盖用户已修改配置；域名引导失败不使用旧固定 OTA host 兜底

**规模/范围**：预计涉及 `pinvou3-app/src-tauri/src/os/windows/windows_domain_bootstrap.rs`（新增）、`pinvou3-app/src-tauri/src/os/windows/windows_update.rs`、`pinvou3-app/src-tauri/src/os/windows/mod.rs`；`pinvou3-app/src-tauri/src/bridge/paths.rs` 仅作为 `pinvou3_home()` 等通用根目录来源，不承载 Windows 专属配置路径函数；前端 `index.html`/`tauri-bridge.js` 原则上不变，除非需要优化错误文案承接；本 feature 文档与测试说明同步更新

## 宪章检查

*门禁：Phase 0 研究前必须通过；Phase 1 设计后必须复查。*

- **中文文档优先**：PASS。本计划、研究、数据模型、契约和 quickstart 使用中文；仅保留 `smarthubOta`、`bootstrapHost`、API 路径、命令和字段名等必要英文标识。
- **DeepSeek-TUI 底座优先**：PASS。域名引导属于 Tauri Windows OTA wrapper，不改 Engine、ToolRegistry、SSE、Session、SkillRegistry、Commands、MCP、Hooks、Cycle 或 Compaction。
- **本地算力与数据边界**：PASS。外发调用限于产品更新检查/反馈场景；不引入远端模型或隐式搜索；SN 使用规则、配置路径和反馈状态文件均明确。
- **小步高质量变更**：PASS。新增模块只放在 Windows 系统目录，改造现有 Windows OTA 配置来源，避免跨平台重构和前端大改。
- **可测试性与可验证交付**：PASS。计划包含配置读取、SN 选择、签名、响应解析、OTA host 传递、反馈目标保留和非 Windows 回归验证。
- **可维护性与长期演进**：PASS。契约记录 C# 参考组件的请求/签名/响应细节，quickstart 记录现场可编辑配置文件和 mock 验证方式。

**门禁结果**：PASS，无需复杂度豁免。

## 项目结构

### 文档（本 feature）

```text
specs/009-ota-domain-bootstrap/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── domain-bootstrap-contract.md
│   └── windows-ota-flow-contract.md
└── checklists/
    └── requirements.md
```

### 源码（仓库根目录）

```text
pinvou3-app/
└── src-tauri/
    └── src/
        ├── bridge/
        │   └── paths.rs                  # 仅复用通用 pinvou3_home() 根目录约定
        ├── updater.rs                    # 原则上保持跨平台薄命令层不变
        └── os/
            ├── mod.rs                    # 不新增跨平台接口，继续导出既有 OTA 能力
            ├── linux/
            │   └── linux_update.rs       # 不改行为
            └── windows/
                ├── mod.rs
                ├── windows_system.rs     # 仅在复用系统工具读取 BIOS SN 时调整
                ├── windows_update.rs
                └── windows_domain_bootstrap.rs
```

**结构决策**：域名引导只服务 Windows OTA，所有请求结构、配置结构、SN 规则、签名和响应解析放入 `pinvou3-app/src-tauri/src/os/windows/windows_domain_bootstrap.rs`，符合“Windows OTA 共享数据结构放到 Windows 系统目录下”的既有要求。`windows_update.rs` 继续负责 OTA 查询、下载、安装和反馈，但不再直接使用固定 `DEFAULT_OTA_HOST` 作为生产默认；它通过域名引导模块解析得到 OTA host。跨平台 `updater.rs` 保持薄命令层，Linux 更新实现不参与该 feature。

## 复杂度追踪

> 仅当宪章检查存在需要解释的违反项时填写。

| 违反项 | 为什么必要 | 拒绝的更简单替代方案 |
|---|---|---|
| 无 | N/A | N/A |

## Phase 0：研究结论

见 [research.md](./research.md)。

## Phase 1：设计产物

- 数据模型：[data-model.md](./data-model.md)
- 域名引导服务契约：[contracts/domain-bootstrap-contract.md](./contracts/domain-bootstrap-contract.md)
- Windows OTA 流程契约：[contracts/windows-ota-flow-contract.md](./contracts/windows-ota-flow-contract.md)
- 验证指南：[quickstart.md](./quickstart.md)

## Phase 1 复查

- **中文文档优先**：PASS。新增计划、研究、模型、契约和 quickstart 均为中文。
- **DeepSeek-TUI 底座优先**：PASS。设计产物没有要求修改底座或 fork。
- **本地算力与数据边界**：PASS。外发边界限于域名引导、OTA 查询/下载/反馈；SN 隐私处理和配置文件位置已定义。
- **小步高质量变更**：PASS。改动路径集中在 Windows OTA 模块，前端与 Linux 默认不动。
- **可测试性与可验证交付**：PASS。契约列出 mock、单测、静态检查和手动验收路径。
- **可维护性与长期演进**：PASS。C# 参考组件关键业务契约已记录，后续任务可直接追踪到实现文件。

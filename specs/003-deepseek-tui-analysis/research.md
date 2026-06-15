# 研究：DeepSeek-TUI 源码职责分析

## 决策 1：按“底座能力 + pinvou3 接入点”双轴组织分析

**Decision**：最终文档采用两条主线：先说明 DeepSeek-TUI 自身工作区的底座能力，再说明 pinvou3-app 如何通过 bridge、engine、commands、session store 等接入这些能力。

**Rationale**：用户的目标是接手当前项目，而不是泛读上游源码。单纯按 DeepSeek-TUI 目录顺序介绍会缺少 pinvou3 的维护语境；单纯按 pinvou3 调用链介绍又会看不清底座职责。

**Alternatives considered**：
- 只按 DeepSeek-TUI crate 列表展开：证据清楚，但维护者难以判断 pinvou3 真实使用路径。
- 只按 pinvou3 功能流展开：便于排查，但容易忽略底座已有能力与不应重复实现的边界。

## 决策 2：源码证据粒度使用“文件/模块/关键类型/调用点”

**Decision**：每个关键结论至少绑定到文件路径、crate、类型名或调用点，不要求逐行摘录大量源码。

**Rationale**：维护文档需要可追溯，但不应变成源码全文复述。文件和类型粒度足以指导后续定位，也能避免文档过快随小改动失效。

**Alternatives considered**：
- 每个结论附行号：更精确，但行号在合并和格式调整后容易漂移。
- 只写模块名：过粗，后续维护者仍需重新搜索。

## 决策 3：将 DeepSeek-TUI 能力分为当前接入、间接依赖、当前未直接接入

**Decision**：文档需要标明能力状态：pinvou3 当前直接调用、通过底座间接使用、或仅存在于 DeepSeek-TUI 但当前主线未直接接入。

**Rationale**：DeepSeek-TUI 工作区包含 CLI、TUI、app-server、extensions、integrations、whaleflow 等大量能力。维护者需要知道哪些与当前 Windows Tauri app 相关，哪些只是背景能力。

**Alternatives considered**：
- 全部能力同等展开：会稀释重点。
- 只写当前直接调用：会漏掉底座内部间接能力，例如工具注册、Compaction、Hooks 等。

## 决策 4：维护风险必须覆盖子模块版本和 Windows 构建运行

**Decision**：风险清单必须包含子模块提交不匹配、`Cargo.lock` 变化、Rust 版本、release exe 进程占用、Windows 路径与用户目录、打包产物路径。

**Rationale**：最近的合并后测试已经暴露过 DeepSeek-TUI 子模块检出到旧提交导致 API 字段不兼容，以及 release exe 被运行中进程锁定导致构建失败。这些是 Windows 接手维护的高频真实风险。

**Alternatives considered**：
- 只写源码架构风险：不够贴近用户当前维护任务。
- 把所有 Windows 打包细节放入本文档：会越界，MSI 打包已有独立 feature 文档。

## 决策 5：最终交付放入 `docs/DeepSeek-TUI源码职责分析.md`

**Decision**：最终源码分析文档放在 `docs/` 下，文件名使用中文，便于维护者从项目文档入口发现。

**Rationale**：Spec Kit 工件用于计划和任务追踪，长期交接资料应进入 `docs/`。中文标题符合项目宪章“中文文档优先”。

**Alternatives considered**：
- 放在 feature 目录：便于追踪，但长期可见性较弱。
- 只更新 AGENTS.md：AGENTS.md 应保留规则和入口，不适合承载完整源码分析。

## 决策 6：验证方式以文档契约检查为主，不运行完整测试矩阵

**Decision**：本 feature 的验证重点是文档契约、证据覆盖、调用链覆盖和中文可读性；不要求运行完整应用测试矩阵，除非实现阶段触碰业务代码。

**Rationale**：本 feature 不修改运行时代码。运行完整构建对文档交付价值有限，但需要通过 `git status` 确认未误改业务代码。

**Alternatives considered**：
- 强制运行 release build：成本高，且与文档改动风险不匹配。
- 完全不验证：不符合宪章的可验证交付原则。

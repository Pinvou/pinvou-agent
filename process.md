# pinvou3 进度记录

跨阶段定位 + 关键决策 + 待排期事项。**细节走 git log + `docs/`,这里每条尽量一句话。**

最后更新: 2026-05-28

---

## 当前状态

- **main**: `e6f246c` — 阶段 L 附件管线已并入;推 `Pinvou/pinvou3`(owner 直推 bypass PR 保护)+ backup `h3c-hexin/pinvou3`
- **fork**: `bf048a7c` (`pinvou3-patches`, v0.8.47) — 上游 PR 活动状态见 `docs/fork-modifications.md`,本文件不维护
- **worktrees**: `issue-1-attachment-pipeline`(本会话)、`workflow-desing-01`
- **subagent**: 只用单 + 串行,并行 fan-out 已废弃(见决策);行为调优归 owner

---

## 已完成阶段(一句话)

| 阶段 | 一句话 | 文档 |
|---|---|---|
| **E** 工具表精简 | LLM 可见工具 85→16+4、schema 119KB→14KB、翻译 19s→8s;OpenAI batch tool_calls 修复进上游 #1686 | `docs/工具表精简方案.md` |
| **F** subagent + L1 测试 | 单 subagent context isolation 可用;L2 49 测试 CI gate + L1 vLLM 18 场景 harness + Judge rubric r2 | judge-report |
| **G** 品悟 v2 review | 常驻嘴替压成 3 节点触发:Plan 出炉(L2 blocking)/ 任务收口(L1 advisory)/ stuck 兜底(未做) | `docs/Pinvou-品悟设计.md` |
| **H** auto-compact 256K | 底座 V4 1M 默认参数在 256K 窗口退化,一次性收口(hint vendor + budget 分级 + 关 cycle + careful BLOCK) | `docs/auto-compact-256K-tuning.md` |
| **I** workflow phase 可视化 + 大文件 SSE 治本 | SKILL `phases:` → chip 可视化(两套并存待选);append_file + 骨架量化约束跑通 15 页 PPT | git log |
| **J** SSE Phase1 + v0.8.45 同步 | write/append 64KB 硬上限 + 截断感知错误;rebrand `codewhale-tui`;bridge 按 recoverable 分流瞬态错误 | git log |
| **K** React UI 迁移 | 同事纯 React UI 替换 vanilla(bridge 唯一状态源/React 纯渲染),37 命令 + 14 事件全移植 + workflow 视图 | git log |
| **L** 附件识别补全(#1) | file_ingest 补 OCR + 修错派 pandoc 的格式(office/archive/eml) + deb 依赖声明;8 单测 + e2e + Qwen3.6 语义验证全过 | git log |

---

## 待办

**🟡 中等**
- **WorkFlow 视图编排**(差异化最强,用户明确要做):交互模型待讨论(todo checklist / 节点流 / 专家协作),可能与 `plan_card` 融合
- **模型预设切换 GUI**:bridge 已有 `ModelPreset` 占位,缺 GUI(远程 DeepSeek API / OpenRouter)
- **GUI subagent 体验**:卡片方案 B/C(串行视图 / 内部 timeline)+ Settings toggle 启用 subagent(当前需 env override)

**🟢 低优先**
- 真 sanity 跨 session 重评(当前 sanity 是同 session retake)
- GB10 self-hosted GitHub Actions runner 跑 L1 nightly(等团队 ≥2 人或发版加快)

**⏸️ 已决策不做**
- 视觉模型补足(Qwen-VL 等)、图片 VLM caption、音视频转录 — 均等 GB10/vision 接入再作独立能力,现降级处理
- 多 vLLM 实例给 subagent — 不解决根本(慢主因是模型行为),破坏 single-vLLM 简洁
- 多 subagent 大研究 fan-out — 弱模型不可用(见决策)
- prompt 工程消解死磕 / reminder-vs-XML / L3 Playwright / scenario 提 toml / 产物内嵌预览 — ROI 低或已回退

---

## 关键决策(一句话)

- **并行 fan-out 废弃**(2026-05-27):弱模型不可用,真根因是主 agent 编排认知 + 结果提取协议(看不懂 eval 里的子 agent 结果→反复重 spawn 撞超时),非工具/后端;只留单 subagent + 串行,`max_subagents=1`,调优归 owner
- **prompt 工程看规则形态**:量化具体边界(「骨架 ≤200 行、不 inline CSS/JS」)Qwen3.6 严格遵循,抽象意图(「必须拆」)无效;改 reminder 前必多 scenario A/B,单样本不可信
- **LLM 复述本性改不动**:写「让我…」是 mode-independent,reminder 强改反而 token 暴增;±0.5 才算 signal,±0.2 是 noise
- **上游 PR 取舍**:通用 bug/优化 → 提 PR,pinvou3 专用/opinionated → 留 fork;活动状态归 `docs/fork-modifications.md`
- **soffice 并发**:多附件 ingest 各用独立 UserInstallation profile,避免同 profile lock

---

## 已知问题 / 边界(一句话)

- **subagent**:单(context isolation)+ 2-3 串行可用;后端并发非瓶颈(N=4 探针 first-token <1s);并行 fan-out 不可用(见决策)
- **LLM 行为不稳**(vLLM 抽奖):偶发 detour 写文件/调 web_search,单 sample 不下结论
- **grep_files**:fork patch 在 v0.8.45 合并时被上游 harvest 版覆盖丢失;上游 per-file cancel-check 大目录够用,spawn_blocking+硬超时走 PR #2146(详 `docs/fork-modifications.md`)
- **Judge 局限**:Claude/Qwen 同为 LLM 有共同盲区,单 session retake 非真 sanity
- **L1 离群点**:暂无待处理(历史离群点已结案或随 fan-out 废弃关闭);跟进流程见 `docs/L1-judge-rubric.md` §3

---

## 相关文档

- `docs/fork-modifications.md` — fork 修改清单 + 上游 PR 活动状态(单一真相源)
- `docs/工具表精简方案.md` — 工具精简 + 附件 pipeline §6.1 + 视觉模型 §6.2
- `docs/auto-compact-256K-tuning.md` — 256K 窗口调优
- `docs/Pinvou-品悟设计.md` — 品悟 review 系统
- `docs/L1-judge-rubric.md` / `docs/l1-baselines/` — Judge rubric + baseline
- `docs/自动化测试方案.md` — 测试系统现状
- `docs/DeepSeek-TUI-架构详解.md` — 底座解析
- `docs/验证报告-qwen3.6-deepseek-tui.md` — 阶段 A 实证报告

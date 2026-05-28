# pinvou3 进度记录

跨阶段定位 + 关键决策 + 待排期事项。**细节走 git log + `docs/`,这里每条尽量一句话。**

最后更新: 2026-05-28

---

## 当前状态

- **main**: `e6f246c` — 阶段 L 附件管线已并入;推 `Pinvou/pinvou3`(owner 直推 bypass PR 保护)+ backup `h3c-hexin/pinvou3`
- **fork**: `bf048a7c` (`pinvou3-patches`, v0.8.47) — 上游 PR 活动状态见 `docs/fork-modifications.md`,本文件不维护
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
- GB10 self-hosted GitHub Actions runner 跑 L1 nightly(等团队 ≥2 人或发版加快)

**⏸️ 已决策不做**
- 音视频转录 — 等 GB10/相应模型接入再作独立能力,现降级处理(视觉已完成,见关键决策)
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
- **视觉接入走工具式复用**(2026-05-28):Qwen3.6 实测有视觉(base64 image_url 识图通过,推翻"不是 vision 模型"旧判断);复用底座已有 `image_analyze` 工具(零底座功能改动),pinvou3 四处接线——(1)开启 `Feature::VisionModel` feature(默认 Experimental 关)+(2)`vision_config` 指向同一 vllm 端点,**两道门缺一不可**(tool_setup.rs:99 同时 require feature+config 才注册 image_analyze)+(3)blocklist 放出 image_analyze +(4)附件图拷进 session workspace `attachments/` 引导 LLM 调用;真 e2e 验证(`l1_dialog_harness::image_vision_analyze`,13s 一击命中 KX7-93)——组件测试全过但漏了 feature 门,只有 e2e 抓到。**路线 1 软肋(GUI 手测暴露)**:图不在主上下文,模型有时跳过 image_analyze**凭空幻觉**(同一张证件照,不调工具时编成"Qwen 文档页",调了才得真相);修法是引导 prompt 翻转框架——不说"你有视觉能力"(诱导直接描述),改硬约束"调用前你对图一无所知,绝不能描述/猜测",并点名"这是什么/帅吗"等问法(`commands.rs`)。治标不治本(概率性),仍偶发就该上路线 2 根治。原生多模态(ContentBlock::Image,~100 行 fork)留作后续升级

---

## 已知问题 / 边界(一句话)

- **subagent**:单(context isolation)+ 2-3 串行可用;后端并发非瓶颈(N=4 探针 first-token <1s);并行 fan-out 不可用(见决策)
- **LLM 行为不稳**(vLLM 抽奖):偶发 detour 写文件/调 web_search,单 sample 不下结论
- **grep_files**:fork patch 在 v0.8.45 合并时被上游 harvest 版覆盖丢失;上游 per-file cancel-check 大目录够用,spawn_blocking+硬超时走 PR #2146(详 `docs/fork-modifications.md`)
- **Judge 局限**:跨 session 盲评已做(2026-05-28,19/20 点 0 漂移,稳定;唯一 ±1 落在 plan_mode 简洁性 3-vs-4 模糊带,详 `docs/l1-baselines/v0.8.37-r1/judge-sanity-retake-newsession.md`);残留盲区是 Claude/Qwen 同为 LLM 的同向偏,需换模型家族评才查得出
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

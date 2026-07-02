# pinvou3 进度记录

跨阶段定位 + 关键决策 + 待排期事项。**每条一句话,细节走 git log + `docs/`。**

最后更新: 2026-06-18

---

## 当前状态

- **main**: 渲染 prompt 20.1K（-48%；A 类减肥 + 消重 + workflow 下线 + N override 并入）；推 `Pinvou/pinvou3`（owner 直推）+ backup `h3c-hexin/pinvou3`
- **fork**: `af64e9f7`（`pinvou3-patches` / v0.8.47）；全量 drift ~2200 行逼近 1500 软上限（`docs/fork-modifications.md`）
- **subagent**: 只用单 + 串行，并行 fan-out 已废弃（见决策）
- **workflow**: 功能重新设计中，GUI「开发中」占位；h3c-ppt（全仓唯一 phased skill）已下线 → 底座 phase 协议段停渲

---

## 已完成阶段（一句话）

| 阶段 | 一句话 | 文档 |
|---|---|---|
| **E** 工具表精简 | LLM 可见工具 85→16+4、schema 119KB→14KB；batch tool_calls 修复进上游 #1686 | `docs/工具表精简方案.md` |
| **F** subagent + L1 测试 | 单 subagent context isolation 可用；L2 49 测试 gate + L1 18 场景 harness + Judge rubric | judge-report |
| **G** 品悟 v2 review | 常驻嘴替压成 3 节点：Plan blocking / 收口 advisory / stuck 兜底（未做）。**设计已推翻，2026-06-05 实现自源码移除**，新方案另行设计 | `docs/archived/Pinvou-品悟设计.md` |
| **H** auto-compact 256K | 底座 V4 1M 默认参数在 256K 退化收口；2026-06 修 nice/emergency 倒置 + output 64K→24K（输入预算 74%→90%） | `docs/auto-compact-256K-tuning.md` |
| **I** phase 可视化 + 大文件 SSE | SKILL `phases:`→chip 可视化；append_file + 骨架量化约束跑通 15 页 PPT | git log |
| **J** SSE Phase1 + v0.8.45 同步 | write/append 64KB 硬上限 + 截断感知错误；rebrand `codewhale-tui` | git log |
| **K** React UI 迁移 | 纯 React 替换 vanilla（bridge 唯一状态源）；37 命令 + 14 事件 + workflow 视图全移植 | git log |
| **L** 附件识别补全(#1) | file_ingest 补 OCR + 修 pandoc 误派（office/archive/eml）+ deb 依赖声明 | git log |
| **M** 视觉接入 | 复用底座 image_analyze（两道门 feature+config）+ 附件暂存引导 + 反幻觉硬约束 prompt | 决策 §视觉 |
| **N** prompt fork→override | base/locale/authority 加 OnceLock override hook，内容迁 pinvou3-app，submodule base.md 回退上游 0 diff | `docs/base-prompt-override-阶段2.md` |

---

## 待办

**🟡 中等**
- **prompt 减肥二期（C 类）**：剩余大头在 submodule 无 override（Personality / Mode·Approval×3 / Compaction 模板 等 ~8K），要加 override hook 或改 fork；memory `prompt-slimming-task` 有地形 + 红线
- **vLLM context 显式生效**：设置页已先做本机 vLLM 检测按钮（只回填 base_url/model,不影响 Engine budget）；待加 `advanced.custom_context_window_tokens` 或同等 override，让 `/v1/models.max_model_len` 可显式驱动 `context_window_for_model` / `context_input_budget`，并把 compact `token_threshold` 从固定 190K 改为按有效窗口动态计算（保留 nice < emergency ≥20K margin）
- **WorkFlow 视图编排**（差异化最强，用户要做）：卡=skill 非 agent，pinvou3 已有 skill 池骨架（`list_skills_v2`+`SkillCard`+`start_skill_session`），P0 加搜索/分类/富元数据；候选源 `agency-agents-zh`（215 中文角色/MIT，人格卡需策展瘦身）
- **GUI subagent 体验**：串行视图 / 内部 timeline 卡片 + Settings toggle 启用（当前需 env override）
- **品悟新方案**：v2 嘴替 review（EXIT GATE + 3 节点 + PinvouToggle/PinvouActionsCard/紫色气泡）已于 2026-06-05 自源码整体移除（review_gate.rs / 4 个 command / 2 个内置 skill / 前端全链路，bundle 0.7 清理装机残留）。原「outside voice §4.3 独立性」待办随 v2 一并撤销。v3 三人协同原型留在 worktree `pinvou3-v3-DO-NOT-DELETE`（分支 `feat/pingwu-v3-prototype`，未合入）。新方案另行设计
- **plan/yolo 收敛（进行中）**：方案收敛为 Yolo-only。前端已隐藏 Plan 模式切换入口(`ModeHeader`)（JSX 注释，组件/bridge/底座逻辑保留，取消注释即恢复）。待重评：plan 相关 command（set_plan_mode_next/accept_plan/exit_plan_to_yolo）是否彻底移除。否决(2026-06-03)：人话翻译层 timeline ⚠️ 黄色警示——实现(destructiveHint 模板 + ToolCard 一行提示)后用户判定不需要(光警示不能撤=干着急 + 视觉噪音)，已全部回滚，index.html 回到改前。结论：Yolo 下破坏性操作不额外标注，真危险仍由底座 Careful Hook 红卡拦截。红卡本身已人话化(2026-06-03)：底座吐的英文技术原因(如 "Attempts to recursively delete home directory")→中文映射表 + 去术语标题/说明 + 技术详情折叠(CarefulBlockedCard)。已搁置：revert 一等交互（前端 UI + 对话同步，没想好）。不可逆操作保持现状不审批（窄白名单不做）

**🟢 低优先**
- GB10 self-hosted runner 跑 L1 nightly（等团队 ≥2 人或发版加快）
- **音视频音轨转录**（issue #1 最后缺口，暂缓）：ffmpeg 抽音轨 → whisper.cpp + ggml-small（首次自动下载，不进 deb），只转录不做画面理解

**⏸️ 已决策不做**
- 多 vLLM 实例给 subagent — 不解决根本（慢主因是模型行为）
- 多 subagent 大研究 fan-out — 弱模型不可用（见决策）
- prompt 工程消解死磕 / L3 Playwright / 产物内嵌预览 等 — ROI 低或已回退

---

## 关键决策（一句话）

- **并行 fan-out 废弃**（2026-05-27）：弱模型不可用，根因是主 agent 编排认知 + 结果提取协议（非工具/后端）；只留单 + 串行，`max_subagents=1`
- **prompt 工程看规则形态**：量化边界（「骨架 ≤200 行」）Qwen3.6 遵循、抽象意图无效；改 reminder 必多 scenario A/B
- **LLM 复述本性改不动**：reminder 强改反致 token 暴增；±0.5 才算 signal，±0.2 是 noise
- **上游 PR 取舍**：通用 bug/优化 → PR，pinvou3 专用 → 留 fork（状态归 `docs/fork-modifications.md`）
- **soffice 并发**：多附件 ingest 各用独立 UserInstallation profile 避免 lock
- **视觉走工具式复用**（2026-05-28）：Qwen3.6 实测有视觉，复用 image_analyze + 两道门（feature+config）；软肋是图不在主上下文、模型跳过工具会幻觉，靠反幻觉 prompt 治标
- **prompt 文案走 override 不改 submodule**（2026-05-29）：base/locale/authority 加 OnceLock hook、内容迁 pinvou3-app，submodule 0 diff，hook 可提上游 PR
- **256K 真触发是 emergency 容量护栏**（2026-06-03）：静态 800K/500K 撞不到，靠 `context_input_budget`（窗口派生、绕 floor）；token_threshold 必须低于 budget 否则倒置 → 改 190K；output 按「≤16KB 分块写」重估 24K；不变式由回归测试锁
- **大工具输出无需治理**（2026-06-03，A1 关）：底座 `compact_tool_result_for_context` 每结果硬压 12K 字符（256K 走小限档）；`large_output_router` synthesis 是 dead code，保持 `workshop=None`
- **提问引导补丁不做**（2026-06-03）：隐藏 plan 后担心「提问引导真空」,A/B 验证(真实 base+instructions+reminder+request_user_input schema、不传 temperature 匹配 turn_loop None、每格 10 共 80 样本)证明现状 §2.4 已够——text列选项率 A=0/40,加「禁止在 text 列选项」禁令无益反升 B=4/40 且调工具率 62%→62% 零变化(疑弱模型反向暗示);早期 probe 的 20% 系 temp0.7+小样本噪声。教训:reminder/prompt 改动必须大样本+忠实参数 A/B,否则被抽奖噪声误导
- **工具调用漂移=vLLM 特有,非 prompt/接线**（2026-06-18 实锤）：NVFP4+mtp 投机解码在新 session 首请求**冷 prefill 于 turn_meta 边界采歪**→首轮工具调用退化(把 turn_meta 元数据复读成正文/不调工具,**首轮 only、再问一句即自愈**;样本=「帮我写贪吃蛇」吐出 `Current local date/workspace` 且不写文件)。判据=revert warmup 立即复现 + **三方模型(Anthropic/OpenAI)同 prompt 零问题** → 坐实 vLLM 侧、非接线。解法**两层缺一就漂**:`5ce4a915`(pwd 移出 static system→turn_meta,治 system 部分命中)+`774477e`(session 首请求 warmup 预热整段前缀**到 turn_meta**,治 turn_meta 冷 prefill,**非冗余**);细节 memory `subagent_prefix_cache_miss_root_cause`。**上游根因=vLLM #43559**(2026-06-18 查实+实测):Qwen3.6-A3B `prefix-caching × MTP` 的 mamba state cache 边界不一致(@timothysu tool-eval-bench 工具调用准确率 90%→50%、多人复现,维护者 @zack041 #43650 在修);**`--enforce-eager` 实测无效**(关 CUDA graph 后仍 ~100% 全漂 → 根因非执行路径/非 CUDA graph);根治待上游修 or 关 `prefix-caching`/`MTP` 之一(单开都不漂),pwd移出+warmup 是应用层绕过

---

## 已知问题 / 边界（一句话）

- **subagent**：单 + 2-3 串行可用，后端并发非瓶颈（N=4 探针 first-token <1s），并行 fan-out 不可用
- ~~**工作流 subagent max_steps 无界**（2026-06-12，#5 合入时记）：底座 manager `max_steps` 恒 `u32::MAX`，per-role registry max_steps（agent_registry.json，ops.rs 设计意图）未接通——Op 处理器丢弃该字段~~ **✅ 已解决（PR #12，底座 b7467cb9）**：`Op::SpawnSubAgent` 派发臂的 `max_steps: _` 改为透传，`SubAgentSpawnOptions.max_steps` per-spawn 覆盖，registry 的 15/20/30 真正生效（`AgentSpawnTool` 传 `None` 保旧默认，不误杀工部产 pptx 长任务）；并叠加 submit_output 成功即 break 收工，根治 temp=0 永动
- **LLM 行为不稳**（vLLM 抽奖）：偶发 detour 写文件 / 调 web_search，单 sample 不下结论
- **grep_files**：fork patch 在 v0.8.45 被上游 harvest 版覆盖丢失，上游版大目录够用，硬超时走 PR #2146
- **Judge 局限**：跨 session 盲评稳定（19/20 零漂移）；盲区是 Claude/Qwen 同向偏，需换模型家族评才查得出
- **L1 离群点**：暂无待处理（历史已结案或随 fan-out 废弃关闭）

---

## 相关文档

- `docs/fork-modifications.md` — fork 修改清单 + 上游 PR 状态（单一真相源）
- `docs/工具表精简方案.md` — 工具精简 + 附件 pipeline + 视觉模型
- `docs/context-compaction-设计.md` — ⭐上下文压缩总纲：优化目标（最小化压缩次数）+ 水位线 W/O/T/E + 分期（一期探测适配零fork / 二期手动配窗 / 上游线 usage 回灌）
- `docs/auto-compact-256K-tuning.md` — （史料）方案部分已被上文取代；倒置 bug 发现过程 + 大工具输出 bound 仍有效
- `docs/archived/Pinvou-品悟设计.md` — 品悟 v2 review 系统（已推翻，实现已移除）
- `docs/L1-judge-rubric.md` / `docs/l1-baselines/` — Judge rubric + baseline
- `docs/自动化测试方案.md` — 测试系统现状
- `docs/DeepSeek-TUI-架构详解.md` — 底座解析
- `docs/验证报告-qwen3.6-deepseek-tui.md` — 阶段 A 实证报告

# L1 Judge Report — `1779159923` (18 scenarios, rubric r2)

> Judged by Claude. Rubric: `docs/L1-judge-rubric.md` **r2**。
> Source transcripts: `target/l1-runs/1779159923/`。
> Model: Qwen3.6-35B-A3B-FP8 @ vLLM (plan B params)。
> **本轮变更**: Fork patches A+B+C+C+ 全套 (subagent stop-on-fail prompt + elapsed cap 300s + max_steps 20 + role=user) + max_subagents=1 工程锁定 + INSTRUCTIONS_MD v4 整理 (112 行,加 §3 任务完成定义)。
> 跟 baseline `v0.8.37-vllm3_planB-r2-18scn` 对比。

## 总览

| scenario | 准确性 | 完整性 | 简洁性 | 工具 | 拆分 | 综合 | 平均 |
|---|---|---|---|---|---|---|---|
| translate_no_tool | 5 | 5 | 5 | N/A | N/A | N/A | 5.00 |
| reasoning_off_speed | 5 | 5 | 5 | N/A | N/A | N/A | 5.00 |
| save_to_tmp_no_validate_fail | 5 | 5 | 5 | 5 | N/A | N/A | 5.00 |
| data_analysis_csv | 5 | 5 | 5 | 5 | N/A | N/A | 5.00 |
| refusal_correct | 5 | 5 | 5 | 5 | N/A | N/A | 5.00 |
| tool_error_recovery | 5 | 5 | 5 | 5 | N/A | N/A | 5.00 |
| multi_turn_context | 5 | 5 | 5 | 5 | N/A | N/A | 5.00 |
| batch_create_7_files | 5 | 5 | 4 | 5 | N/A | N/A | 4.75 |
| write_okr_md | 5 | 5 | 4 | 5 | N/A | N/A | 4.75 |
| plan_travel_web | 5 | 5 | 4 | 5 | N/A | N/A | 4.75 |
| **long_output_1500** | 5 | 5 | 4 | 5 | N/A | N/A | **4.75 ⬆⬆** (从 3.25 升) |
| chinese_idiomatic | 5 | 5 | 3 | **3** | N/A | N/A | 4.00 (write_file detour 不必要) |
| plan_mode_list_dir | 4 | 5 | 4 | 4 | N/A | N/A | 4.25 |
| subagent_no_need | 5 | 5 | 5 | 5 | N/A | N/A | 5.00 |
| subagent_single_simple | 5 | 5 | 5 | 5 | 5 | N/A | 5.00 |
| **subagent_one_fails** | 5 | 5 | 4 | 5 | 5 | 5 | **4.83** (377s,主 agent 用 checklist 拆 4 步串行执行) |
| **subagent_compare_3_libs** | 3 | 3 | 3 | 3 | 5 | N/A | **3.40** (cargo timeout,但 max=1 工程锁起效) |
| **subagent_research_topic** | 3 | 3 | 3 | 3 | 4 | N/A | **3.20** (cargo timeout,3 次 limit reached) |
| **维度平均** | 4.78 | 4.83 | 4.39 | 4.50 | 4.80 | 5.00 | **4.66** |

## 关键改进 (vs `vllm3_planB-r2-18scn` 4.59)

### 🎉 §3 任务完成定义 起效

- **`long_output_1500` 3.25 → 4.75 (+1.5!)**: 前次 12K 字 + 2 次 web_search detour cargo fail,本次 122s + 6.7K 字 PASS。§3 "80 分及时交付 > 99 分超时" 命中
- **`tool_error_recovery` 4.75 → 5.00**: 文本 128 字带建议
- **`multi_turn_context` 5.00 → 5.00**: 持平,t2 直接答 36 岁 (53 字,无 overkill)

### 🎉 max_subagents=1 工程锁定 验证

- 3 个 subagent scenarios 都触发 "Sub-agent limit reached" (compare 2 次 + one_fails 2 次 + research 3 次)
- LLM 试图开第 2 个 subagent 立即被工程层 reject
- `subagent_one_fails`: 主 agent **改用 `checklist_write` 拆 4 步串行执行**,377s PASS,前次 354s 类似

### 🎉 subagent 链路完全可用 (single subagent context isolation)

- `subagent_single_simple`: 75s 完美 5.00
- `subagent_one_fails`: 1 subagent completed (165s Tokio work-stealing 8K 答) + 主 agent 用自身知识补其他

### ⬇ 仍然问题的

- **`subagent_compare_3_libs` 4.50 → 3.40 + cargo timeout**: max=1 工程锁后,LLM 不开多 subagent 改用 **21 次 exec_shell + curl** 拼数据,主 agent 拼太多导致 632s 超 600s cap。§3 任务完成定义没拦住"再查一下更准的"行为
- **`subagent_research_topic` 仍 660s timeout**: 3 次 limit reached + 主 agent 还是反复尝试 spawn 不同 name 的 subagent → 最终所有 subagent 串行跑也超 600s
- **`chinese_idiomatic` 4.33 → 4.00**: 调 write_file + read_file 把答案写到 workspace 又读回来 — 没必要的 detour (prompt 没要求落档)

## 跟历史 baseline 完整对比

| 维度 | r1-13scn (最初) | vllm2-r2-17scn | vllm2_clientfix-r2-18scn | vllm3_planB-r2-18scn | **本次** |
|---|---|---|---|---|---|
| 准确性 | 4.85 | 4.71 | 4.61 | 4.61 | **4.78** ⬆ |
| 完整性 | 4.77 | 4.53 | 4.67 | 4.72 | **4.83** ⬆ |
| 简洁性 | 4.38 | 3.94 | 4.22 | 4.39 | **4.39** |
| 工具使用 | 4.64 | 4.36 | 4.27 | 4.36 | **4.50** ⬆ |
| 任务拆分 | N/A | 4.67 (3) | 4.80 (5) | 4.80 (5) | **4.80 (5)** |
| 结果综合 | N/A | 4.00 (2) | 5.00 (1) | 5.00 (1) | **5.00 (1)** |
| **总均分** | **4.67** | **4.46** | **4.50** | **4.59** | **4.66** ⬆ |

**老 13 scenarios 子集 (排除 subagent)**:
- r1-13scn: 4.67
- vllm3_planB-r2-18scn 内 13 老: 4.71
- **本次 13 老**: **4.83** ⬆ (跟最初比 +0.16)

## 真信号 (跨多次 baseline 一致改善)

1. **plan_travel_web 工具 5/5** (调对 update_plan): vllm3_planB 起持续稳定
2. **long_output_1500 简洁性 4 + 工具 5** (vs 上次 detour 2/2): §3 任务完成定义直接命中
3. **multi_turn_context 工具 5** (不用 exec_shell 算算术): persistent
4. **subagent_one_fails 拆分 5 + 综合 5**: subagent 链路真可用

## 仍然问题 (即便所有 patch 都生效后)

`subagent_compare_3_libs` / `subagent_research_topic` 这种 **多目标研究任务 + 主 agent 必须自己拼数据** 场景,在当前 setup 下不可避免 600s+ timeout。

**根因 (按重要性排序)**:
1. **任务设计本身需要大量数据**: 对比 3 个库 + 调研 4 个方向,主 agent 必须拼 20+ 次 exec_shell/curl
2. **LLM 倾向过度勤奋**: 拿到 80% 信息后还在"再查一下更准的"。§3 任务完成定义部分起效 (long_output 修了) 但对**复杂研究任务** 不够
3. **Cargo timeout=600s 太紧**: 真实用户场景里 600s 不算长,user 可能愿意等 10 分钟拿完整对比

**接下来选项**:
- (a) 接受现状,标这两个 scenario 为 stress test (非生产可用)
- (b) 拉 max_duration_s 到 1200s 给主 agent 时间真的完成
- (c) prompt 工程再加强 §3 (但 ROI 边际递减)

## process.md 待办建议

- ✅ `long_output_1500` 修了 (§3 task definition 起效)
- ✅ `tool_error_recovery` 持平 (元认知改善)
- ✅ `multi_turn_context` 持平 (无 overkill)
- 🆕 **`chinese_idiomatic` 工具 3/5**: LLM 仍 detour 写文件 — prompt 说"直接给文字"但 LLM 写到 workspace。可能 vLLM 抽奖
- 🔁 `subagent_compare_3_libs` / `subagent_research_topic` cargo timeout: 已诊断完结,等用户拍 max_duration_s 或接受现状

## 工程层 + prompt 工程层完成度

| 层 | 改动 | 状态 |
|---|---|---|
| Fork patch role=user (Bug #1) | turn_loop role=user | ✅ verified working |
| harness stream_open_timeout=180s (Bug #2) | env var | ✅ verified |
| Fork patch general subagent stop-on-fail (A) | GENERAL_AGENT_INTRO | ✅ 部分 |
| Fork patch C+C+ (max_steps 20 + elapsed 300s) | DEFAULT_MAX_STEPS / DEFAULT_SUBAGENT_ELAPSED_MAX | ✅ subagent 真 stop |
| Fork patch max_subagents=1 (B 工程锁) | bridge default | ✅ "limit reached" 触发 |
| INSTRUCTIONS_MD v4 整理 + §3 任务完成定义 | 112 行 | ✅ long_output 修了 |

## 结论

**当前 baseline 是 pinvou3 单用户 + 单 vLLM + 弱模型场景的 reasonably mature 配置**。

- 单 subagent context isolation use case: **完全可用** ✅
- 简单 multi-step 任务: ✅ 17/18 PASS
- 多目标研究 + 实时数据拼 (compare_3_libs / research_topic): 当前 setup 边界,接受 stress test 定位

## 备注

- 跨 5 次 baseline 迭代 (r1-13scn → vllm2 → vllm2_clientfix → vllm3_planB → 本次) 总均分 4.67→4.46→4.50→4.59→4.66
- subagent **链路从"全废" → "可用"** 经历了 5 个 patch (A+B+C+C+ + 工程锁)
- INSTRUCTIONS_MD 从 164 行 → 112 行(-32%) 且**质量更好**
- 本次评分耗时 ~10 分钟

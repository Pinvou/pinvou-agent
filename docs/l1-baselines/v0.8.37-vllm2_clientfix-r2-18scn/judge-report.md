# L1 Judge Report — `1779095350` (18 scenarios, rubric r2)

> Judged by Claude. Rubric: `docs/L1-judge-rubric.md` **r2**.
> Source transcripts: `target/l1-runs/1779095350/`.
> Model: Qwen3.6-35B-A3B-FP8 @ vLLM (max-num-seqs=8 + chunked-prefill + 256K context)。
> **Fork patch**: subagent completion role=user + `DEEPSEEK_STREAM_OPEN_TIMEOUT_SECS=180`。
> 跟 baseline `v0.8.37-vllm2-r2-17scn` (1779089467) 对比。

## 总览

| scenario | 准确性 | 完整性 | 简洁性 | 工具 | 拆分 | 综合 | 平均 |
|---|---|---|---|---|---|---|---|
| translate_no_tool | 5 | 5 | 5 | N/A | N/A | N/A | 5.00 |
| reasoning_off_speed | 5 | 5 | 4 | N/A | N/A | N/A | 4.67 |
| save_to_tmp_no_validate_fail | 5 | 5 | 5 | 5 | N/A | N/A | 5.00 |
| data_analysis_csv | 5 | 5 | 5 | 5 | N/A | N/A | 5.00 |
| refusal_correct | 5 | 5 | 5 | 5 | N/A | N/A | 5.00 |
| batch_create_7_files | 5 | 5 | 4 | 5 | N/A | N/A | 4.75 |
| write_okr_md | 5 | 5 | 4 | 5 | N/A | N/A | 4.75 |
| long_output_1500 | 5 | 5 | 3 | 5 | N/A | N/A | 4.50 |
| **tool_error_recovery** | 5 | 4 | 5 | 5 | N/A | N/A | **4.75** ⬇ |
| **multi_turn_context** | 5 | 5 | **2** | **3** | N/A | N/A | **3.75** ⬇⬇ |
| **plan_travel_web** | 4 | 5 | 4 | **3** | N/A | N/A | **4.00** ⬇ |
| **chinese_idiomatic** | 5 | 5 | 4 | **3** | N/A | N/A | **4.25** ⬇ |
| plan_mode_list_dir | 4 | 5 | 4 | 4 | N/A | N/A | 4.25 ⬆ |
| **subagent_no_need** | 5 | 5 | 5 | 5 | N/A | N/A | 5.00 |
| **subagent_single_simple** (新) | 5 | 5 | 5 | 5 | 5 | N/A | **5.00** 🆕 |
| **subagent_one_fails** | 5 | 5 | 5 | 5 | 5 | 5 | **5.00** ⬆⬆ |
| **subagent_compare_3_libs** | 2 | 2 | 4 | 3 | 5 | N/A | **3.20** (cargo test timeout) |
| **subagent_research_topic** | 3 | 3 | 3 | 3 | 4 | N/A | **3.20** (cargo test timeout) |
| **维度平均** | 4.61 | 4.67 | 4.22 | 4.27 | 4.80 (5 个) | 5.00 (1 个) | **4.50** |

## 关键变化 (vs `v0.8.37-vllm2-r2-17scn`)

### 🎉 Fork patch 红利

- **`subagent_single_simple` 新加,直接 5.00**: 1 个 subagent 19s 完成,主 agent 正确转述。验证基本 subagent 链路通畅
- **`subagent_one_fails` 4.50 → 5.00**: 这次主 agent 真的**识别失败重派** (4 agent_open + 5 agent_eval),不再被迫降级到自身知识。综合报告含完整 work-stealing 解析。结果综合 5/5,任务拆分 5/5

### ⬇ 抽奖 regression

- **`multi_turn_context` 5.00 → 3.75**: t2 用 `exec_shell python3` 算 2026-1990 overkill (前次直接答),且 final text 复述两遍 "今天是你36岁生日。今天是你36岁生日。"。简洁性 2/5
- **`plan_travel_web` 4.75 → 4.00**: **没调 update_plan**!跟 v0.8.37-r1-13scn 同问题再现 (上次 vLLM 优化自动修了,这次又回去)。工具 3/5
- **`chinese_idiomatic` 4.67 → 4.25**: 用 `list_dir` 1 次"探工作区有没有 embedding/vector 文件" — 过激探目录 detour。工具 3/5
- **`tool_error_recovery` 5.00 → 4.75**: 不再主动猜测"可能是测试场景" (前次的元认知亮点消失)

### ⚠️ Subagent 大任务受 vLLM 配置限制

- **`subagent_compare_3_libs` 660s timeout**: 3 subagent 都 `status: running`,主 agent 4 次 agent_eval 拿到 running。**vLLM 调度限制** — chunked-prefill + max-num-seqs=8 下,3 个 long-prompt subagent 同时 prefill 60s 内完不成。任务拆分 5/5 正确,但执行环境跑不动
- **`subagent_research_topic` 603s timeout**: 类似问题 + 1 次 stream_stall warning

## 跟历史 baseline diff

| 维度 | r1-13scn (老 vLLM) | vllm2-r2-17scn (1779089467) | vllm2_clientfix-r2-18scn (本次) |
|---|---|---|---|
| 准确性 | 4.85 | 4.71 | 4.61 |
| 完整性 | 4.77 | 4.53 | 4.67 |
| 简洁性 | 4.38 | 3.94 | 4.22 |
| 工具使用 | 4.64 | 4.36 | 4.27 |
| 任务拆分 | N/A | 4.67 (3 个) | 4.80 (5 个) |
| 结果综合 | N/A | 4.00 (2 个) | 5.00 (1 个) |
| **总均分** | **4.67** | **4.46** | **4.50** |

**结论**:
1. **Fork patch 让 subagent 链路真的能用**了 (single_simple/one_fails 完美工作),拆分 +0.13,综合 +1.00
2. **Single subagent 完全可用**,但 ≥3 个长 prompt subagent 受 vLLM 并发限制
3. **老 scenarios 抽奖 regression** (multi_turn_t2 / plan_travel_web / chinese_idiomatic) — LLM 行为本身有 ±0.2 波动是正常的,fork patch 可能改变了 prefix_cache 命中模式

## process.md 待办建议 (闭环)

任一维度 ≤3 的项已 append 到 `process.md`:

- **`subagent_compare_3_libs` / `subagent_research_topic` cargo timeout** (新 🆕): **不是模型能力问题**,是 vLLM `chunked-prefill + max-num-seqs=8` 调度下多 subagent prefill 排队累积 → 多 subagent 大任务在当前配置下完成时间 >10 分钟。**可能的调参**:
  - `--disable-chunked-prefill` 让 prefill 先做完再开 SSE
  - `--max-num-seqs 16` 允许更多并发 sequence
  - `--max-num-batched-tokens 65536` 提高单批吞吐
  - 这是 vLLM 配置 trade-off (latency vs throughput),建议用户根据 subagent 使用频率拍板
- **`multi_turn_context` 简洁性 2 + 工具 3** (新 🆕): 用了 exec_shell 算小学数学 overkill。回到 1779089467 之前的状态,LLM 单次抽奖。**3+ 次出现再考虑 prompt 工程**
- **`plan_travel_web` 工具 3** (🔁 再现): 之前自动修了 update_plan,这次又退回。LLM 行为不稳。**累积证据**
- **`chinese_idiomatic` 工具 3** (新 🆕): 过激探目录 (类似 1779077762 时的 chinese_idiomatic detour 行为再现)

## 备注

- 修复**真的有效**: subagent 链路从"3/4 全失败"到"single 完美 + 2 个完成 + 2 个 vLLM 配置限制"
- **抽奖 noise 占主导**: 老 scenarios 跟前次比 ±0.2-0.5 波动,LLM 输出本质不确定,单次跑不能下结论
- **subagent 在生产可用度**:
  - ≤1 subagent: ✅ 19s 完成,context isolation 起效
  - ≤3 subagent + 短 prompt: ✅ 200-400s 完成
  - 3+ subagent + 长 prompt: ⚠️ 当前 vLLM 配置下 >10 分钟,需要调参或减并发
- 本次评分耗时 ~10 分钟

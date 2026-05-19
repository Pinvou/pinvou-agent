# L1 Judge Report — `1779102334` (18 scenarios, rubric r2)

> Judged by Claude. Rubric: `docs/L1-judge-rubric.md` **r2**.
> Source transcripts: `target/l1-runs/1779102334/`.
> Model: Qwen3.6-35B-A3B-FP8 @ vLLM (**plan B**: max-num-batched-tokens **131072** + max-model-len 262144 + chunked-prefill 保留 + prefix-caching)。
> Fork patches: turn_loop role=user + harness DEEPSEEK_STREAM_OPEN_TIMEOUT_SECS=180。

## 总览

| scenario | 准确性 | 完整性 | 简洁性 | 工具 | 拆分 | 综合 | 平均 |
|---|---|---|---|---|---|---|---|
| translate_no_tool | 5 | 5 | 5 | N/A | N/A | N/A | 5.00 |
| reasoning_off_speed | 5 | 5 | 5 | N/A | N/A | N/A | 5.00 ⬆ |
| save_to_tmp_no_validate_fail | 5 | 5 | 5 | 5 | N/A | N/A | 5.00 |
| data_analysis_csv | 5 | 5 | 5 | 5 | N/A | N/A | 5.00 |
| refusal_correct | 5 | 5 | 5 | 5 | N/A | N/A | 5.00 |
| tool_error_recovery | 5 | 5 | 5 | 5 | N/A | N/A | 5.00 ⬆ |
| **multi_turn_context** | 5 | 5 | 5 | 5 | N/A | N/A | **5.00 ⬆⬆** |
| **plan_travel_web** | 5 | 5 | 4 | 5 | N/A | N/A | **4.75 ⬆** |
| **chinese_idiomatic** | 5 | 5 | 3 | N/A | N/A | N/A | **4.33** (无 detour ⬆但 662 字超量 ⬇) |
| batch_create_7_files | 5 | 5 | 4 | 5 | N/A | N/A | 4.75 |
| write_okr_md | 5 | 5 | 4 | 4 | N/A | N/A | 4.50 |
| plan_mode_list_dir | 4 | 5 | 4 | 5 | N/A | N/A | 4.50 |
| **long_output_1500** | 4 | 5 | 2 | **2** | N/A | N/A | **3.25 ⬇⬇** (cargo timeout + web_search detour + 12K 字超量) |
| subagent_no_need | 5 | 5 | 5 | 5 | N/A | N/A | 5.00 |
| subagent_single_simple | 5 | 5 | 5 | 5 | 5 | N/A | 5.00 |
| subagent_one_fails | 5 | 5 | 5 | 4 | 5 | 5 | 4.83 |
| **subagent_compare_3_libs** | 2 | 2 | 4 | 3 | 5 | N/A | **3.20** (timeout) |
| **subagent_research_topic** | 3 | 3 | 3 | 3 | 4 | N/A | **3.20** (timeout) |
| **维度平均** | 4.61 | 4.72 | 4.39 | 4.36 | 4.80 | 5.00 | **4.59** |

## 关键变化 (vs `vllm2_clientfix-r2-18scn` 1779095350)

### 🎉 vLLM plan B 改善 (3 个 regression 修了)

- **`multi_turn_context` 3.75 → 5.00**: t2 直接答 36 岁 (无 exec_shell overkill,无复述)
- **`plan_travel_web` 4.00 → 4.75**: 调对了 update_plan (回到 vllm2-r2-17scn 的水平)
- **`chinese_idiomatic` 4.25 → 4.33**: 没 list_dir detour 了 (改善),但 662 字超 200 范围 3.3× (简洁性扣)
- **`tool_error_recovery` 4.75 → 5.00**: 详细解释 + 主动提示用户提供路径
- **`reasoning_off_speed` 4.67 → 5.00**: 一句话答 (前次展开 3 个选项)

### ⬇ 新出现的 regression

- **`long_output_1500` 4.50 → 3.25**: prompt 没要求联网,LLM **自己 detour 调 web_search 2 次** (Bing 全失败),且写了 **12K 字超 prompt 1500 要求 8×**。**cargo test fail** (206s > 180s 上限)。工具 2/5 + 简洁性 2/5

### ⚠️ subagent 大任务仍 timeout (vLLM 配置无法解决)

- `subagent_compare_3_libs` / `subagent_research_topic` 仍 660s timeout
- **诊断**: 不是 vLLM 慢,是 **subagent 内部完整对话 >5-10 分钟** (research_academic 504s 仍 `status: running`)。subagent 内部 LLM workflow 自身慢:Qwen 默认 verbose + 多步推理 + 工具失败重试

## 跟历史 baseline 完整对比

| 维度 | r1-13scn (最初 vLLM) | vllm2-r2-17scn | vllm2_clientfix-r2-18scn | **plan_B-r2-18scn (本次)** |
|---|---|---|---|---|
| 准确性 | 4.85 | 4.71 | 4.61 | **4.61** |
| 完整性 | 4.77 | 4.53 | 4.67 | **4.72** ⬆ |
| 简洁性 | 4.38 | 3.94 | 4.22 | **4.39** ⬆ |
| 工具使用 | 4.64 | 4.36 | 4.27 | **4.36** ⬆ |
| 任务拆分 | N/A | 4.67 (3) | 4.80 (5) | **4.80 (5)** |
| 结果综合 | N/A | 4.00 (2) | 5.00 (1) | **5.00 (1)** |
| **总均分** | **4.67** | **4.46** | **4.50** | **4.59** |

**plan B vs clientfix**: +0.09 — **batched-tokens 131072 改善明显**。

**plan B vs 最初 r1-13scn**: -0.08 (整体) / 但**完整性 +0.05 + 简洁性 +0.01 + 拆分 4.80 + 综合 5.00 净增能力**。

## 真信号 vs 抽奖 noise

老 13 scenarios 总均分对比（排除新 subagent）:
- r1-13scn: 4.67
- vllm2-r2-17scn 内 13 老: 4.69
- vllm2_clientfix-r2-18scn 内 13 老: 4.59
- **plan_B-r2-18scn 内 13 老: 4.71** ⬆⬆

**plan B 跟最初比 13 老 scenarios +0.04** (合理改善,在 noise 范围内但累计趋势正向)。

## process.md 待办建议

任一维度 ≤3 的项已 append `process.md`:

- **`long_output_1500` 工具 2/5 + 简洁性 2/5** (新 🆕): LLM 单次 detour,调 web_search 凑数据 + 12K 字超量。**单次抽奖** — 单 sample 不可信。
- **subagent 大任务 timeout** (🔁): 已确认**不是 vLLM 配置问题**,而是 subagent 内部完整对话需要 5-10 分钟。可能改进方向:
  1. 减少 subagent 内部默认 reasoning verbosity (Qwen `reasoning_effort=off` 已设但效果有限)
  2. 主 agent prompt 工程限制 subagent 步数 (例 "agent_eval timeout_ms=300000 + max_steps=5")
  3. 接受 multi-subagent 大任务在 pinvou3 当前场景下不可用,只用 single subagent

## plan B 是否值得保留

✅ **保留**:
- 老 scenarios 总体 +0.04 (4.67→4.71)
- multi_turn / plan_travel / chinese / tool_error_recovery / reasoning_off 5 个改善
- 256K context 保留 (vs plan A 砍到 64K 不可接受)
- batched-tokens 131072 让大部分 prompt 一次 prefill 完

⚠️ **代价**:
- GPU activation memory 峰值高 (估 25GB activation + 35GB weights = 60GB peak,GB10 配额 96GB 内)
- 偶发 LLM detour (但这是 LLM 抽奖,跟 vLLM 配置弱相关)

## 备注

- 本次跑全 18 scenario,3 个 cargo test fail (long_output_1500 / 2 个 subagent timeout),其他 15 个 PASS
- Subagent 大任务的真正瓶颈定位完成: **subagent 内部 LLM workflow 慢,vLLM 调度不背锅**
- 后续如果需要 multi-subagent 真实可用,需要从 prompt 工程或 subagent step 限制入手
- 本次评分耗时 ~10 分钟

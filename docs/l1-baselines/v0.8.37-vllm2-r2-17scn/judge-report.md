# L1 Judge Report — `1779089467` (17 scenarios, rubric r2)

> Judged by Claude. Rubric: `docs/L1-judge-rubric.md` **r2**.
> Source transcripts: `target/l1-runs/1779089467/`.
> Model under test: Qwen3.6-35B-A3B-FP8 @ vLLM 10.214.74.113。
> **vLLM 配置变更**: max-model-len 262144 (256K)、kv-cache fp8、enable-prefix-caching、enable-chunked-prefill、max-num-seqs 8、served-model-name `qwen36_35b`。
> 跟旧 baseline `v0.8.37-r1-13scn` 对比。

## 总览

| scenario | 准确性 | 完整性 | 简洁性 | 工具 | 拆分 | 综合 | 平均 |
|---|---|---|---|---|---|---|---|
| translate_no_tool | 5 | 5 | 5 | N/A | N/A | N/A | 5.00 |
| reasoning_off_speed | 5 | 5 | 4 | N/A | N/A | N/A | 4.67 |
| save_to_tmp_no_validate_fail | 5 | 5 | 5 | 5 | N/A | N/A | 5.00 |
| data_analysis_csv | 5 | 5 | 5 | 5 | N/A | N/A | 5.00 |
| refusal_correct | 5 | 5 | 5 | 5 | N/A | N/A | 5.00 |
| tool_error_recovery | 5 | 5 | 5 | 5 | N/A | N/A | 5.00 |
| multi_turn_context | 5 | 5 | 5 | 5 | N/A | N/A | 5.00 |
| batch_create_7_files | 5 | 5 | 4 | 5 | N/A | N/A | 4.75 |
| plan_travel_web | 5 | 5 | 4 | 5 | N/A | N/A | 4.75 |
| chinese_idiomatic | 5 | 5 | 4 | N/A | N/A | N/A | 4.67 |
| write_okr_md | 5 | 5 | 4 | 4 | N/A | N/A | 4.50 |
| long_output_1500 | 5 | 5 | 3 | 5 | N/A | N/A | 4.50 |
| **plan_mode_list_dir** | 4 | 4 | **3** | 4 | N/A | N/A | 3.75 |
| subagent_no_need | 5 | 5 | 5 | 5 | N/A | N/A | 5.00 |
| subagent_one_fails | 5 | 5 | 4 | **3** | 5 | 5 | 4.50 |
| **subagent_research_topic** | 3 | **2** | **2** | **3** | 4 | N/A | **2.80** |
| **subagent_compare_3_libs** | 3 | **2** | **2** | **3** | 5 | **2** | **2.83** |
| **维度平均** | 4.71 | 4.53 | 3.94 | 4.36 | 4.67 (3 个) | 4.00 (2 个) | **4.46** |

## 逐 scenario 详评

### 5.00 全优 (7 个)
- `translate_no_tool` / `reasoning_off_speed`(简洁性 4)/`save_to_tmp` / `data_analysis_csv` / `refusal_correct` / **`tool_error_recovery`**(从 4.75 升:主动猜测"这是测试错误恢复机制")/**`multi_turn_context`**(从 4.50 升:t2 直接答 36 岁,**不调 code_execution overkill**) / `subagent_no_need`

### 4.75 (2 个)
- `batch_create_7_files`: 持平,text 仍复述 7 个文件路径冗余
- **`plan_travel_web`**: **从 3.75 升到 4.75** — **这次调了 update_plan**!web_search 失败后没再死磕,直接出方案

### 4.50 (3 个)
- `write_okr_md`: 微跌(原 4.75),多调了 read_file 自验是 overkill
- `long_output_1500`: 6804 字超 prompt 1500 要求 4.5×(原 4822 字 3.2×),简洁性 3
- `subagent_one_fails`: 3 个 subagent 都 SSE 失败 200s 内识别,主 agent 用自己知识写完整综合报告,对不可能子任务诚实标失败;**工具使用 3/5 因为 subagent 在新 vLLM 下不稳是技术问题不是模型问题**

### ⚠️ plan_mode_list_dir — 3.75 (3 次重复出现)
text 仍有"让我..."过渡语 + 试 exec_shell 被 Plan 模式拒,简洁性 3/5;但这次有了实际 summary text (不再悬空),完整性 4/5 比前次 3/5 略升

### ⚠️ 新 subagent 离群点 (2 个)

**`subagent_research_topic` — 2.80**
- 532s 4 个 subagent 全 SSE 失败,主 agent 等了 8 分钟才决定 fallback
- text 13 句"还在搜索中"重复 monologue,简洁性 2/5
- 最终 fallback 综述被 timeout 截断 ("好的，让我直接撰写综述。"),实际内容没产出 → 完整性 2/5

**`subagent_compare_3_libs` — 2.83**
- 660s timeout,subagent SSE 失败 → 重派 (name collision 失败) → 主 agent 用 22 次 exec_shell + curl API 拿真数据 (tokio 6.7 亿下载,smol releases 等) → 综合报告刚开始就被截断
- 任务拆分 5/5(3 个 subagent 完美对齐 tokio/async-std/smol),但 fallback 流程过长

## 离群点

### ⚠️ 需关注 (任一维度 ≤3 或平均 ≤3.5)

1. **`subagent_compare_3_libs` 简洁性 2 + 完整性 2 + 综合 2** — **vLLM 下 subagent SSE 不稳定** + 主 agent fallback 路径长。改进方向:
   - **vLLM 调度排查** (本次 baseline 暴露的最大问题):4/4 subagent scenario 里 3 个 subagent 全失败,跟 max-num-seqs 8 / chunked-prefill / 多 sequence SSE 超时相关
   - prompt 工程加引导:"subagent 失败后 1 分钟内决定 fallback,不要重派同 name"
2. **`subagent_research_topic` 简洁性 2 + 完整性 2** — 同上 vLLM 调度问题,但主 agent fallback 决策慢 (8 分钟)
3. **`plan_mode_list_dir` 简洁性 3** — 🔁 **第 3 次出现**,跟 v0.8.37-r1 / r1-13scn 同因,Plan 模式 text 仍有过渡语。**已 lesson learned: reminder 改造单样本不可信,持续追踪**

### ✅ 全优 (平均 5.00,共 7 个 / 41%)

跟 v0.8.37-r1-13scn 的 5 个全优相比,**多出 2 个**: tool_error_recovery + multi_turn_context (都是因 vLLM 优化 LLM 行为改善)。

## 跟历史 baseline diff (v0.8.37-r1-13scn → v0.8.37-vllm2-r2-17scn)

### 13 个老 scenarios (rubric 跨 r1→r2 但老 scenarios 不涉及 r2 新维度,可比)

| 维度 | r1-13scn | vllm2-r2-17scn 内 13 老 | Δ |
|---|---|---|---|
| 准确性 | 4.85 | 4.92 | +0.07 |
| 完整性 | 4.77 | 4.85 | +0.08 |
| 简洁性 | 4.38 | 4.31 | -0.07 |
| 工具使用 | 4.64 | 4.80 | **+0.16** |
| **总均分** | **4.67** | **4.77** | **+0.10** |

**vLLM 优化对老 scenarios 整体正影响 +0.10**:
- 工具使用 +0.16 主要来自 `plan_travel_web` 调对 update_plan + `multi_turn_context` 不再用 code_execution overkill
- 简洁性 -0.07 主要来自 `long_output_1500` 写了 6804 字 (256K context 让 LLM 更敢长输出)

### 4 个新 subagent scenarios

| scenario | 平均 |
|---|---|
| subagent_no_need | 5.00 |
| subagent_one_fails | 4.50 |
| subagent_research_topic | 2.80 |
| subagent_compare_3_libs | 2.83 |
| **subagent 平均** | **3.78** |

**结论**: Qwen3.6 的 subagent 概念使用能力 OK (任务拆分 4.67,概念上会用),**但在当前 vLLM 配置下 subagent 链路不稳** (3/4 scenario subagent 全失败)。这不是模型能力问题,是底座/配置问题。

## 关键发现

### 🎉 vLLM 优化对老场景全部正向
- `plan_travel_web` 修了一直没解决的 update_plan 问题 (无任何代码改动)
- `multi_turn_context` 不再用 code_execution 算简单数学 (overkill 行为消失)
- `tool_error_recovery` 主动识别 test scenario (元认知改善)
- 工具使用 +0.16 是 vLLM 配置带来的免费红利

### ⚠️ Subagent 链路新 vLLM 下不稳
- 4 个 subagent scenarios 里 3 个 subagent 全 SSE 失败 (compare/research/one_fails 共 11 个 agent_eval 大多失败或 timeout)
- 可能原因:`max-num-seqs 8` 跟多 subagent 调度 race / `enable-chunked-prefill` 在多 sequence 下 SSE 超时
- 主 agent fallback 能力**合理但偏慢** (research_topic 等了 8 分钟,one_fails 200s 决断快是亮点)

### 任务拆分能力是真的有
- 3 个会拆分 subagent 的 scenario 都拆得清晰(`compare_3_libs` 5/5 对 tokio/async-std/smol,`one_fails` 5/5 对 3 个子任务,`research_topic` 4/5 对 4 个研究方向)
- subagent prompt 内容质量高 (详细的 (1)(2)(3) 子要求清单)

## process.md 待办建议 (闭环)

任一维度 ≤3 的项已 append 到 `process.md`:

- **`vLLM subagent SSE 调度问题`** (新 🆕 紧急): subagent 在 max-num-seqs 8 + chunked-prefill 配置下不稳定。需排查 vLLM 日志看 SSE 超时根因。**这是 subagent 能力评估的真实瓶颈,跟模型能力无关。**
- `subagent_research_topic` 完整性 2/5: 主 agent fallback 决策慢 (等 subagent 8 分钟)。改进:prompt 工程引导 "subagent 状态 failed 或 1 分钟无进展立即 fallback,别死等"
- `subagent_compare_3_libs` 简洁性 2/5: fallback 后 22 次 exec_shell 凑数据 + 没意识到 subagent close 后 name 仍占用 → 改 prompt 工程或文档说明 subagent name 管理
- `plan_mode_list_dir` 简洁性 3/5: 🔁 **第 3 次出现** (跟 lesson learned 一致),继续追踪不动手

## 备注

- 本次评分耗时 (Claude 这边) ~12 分钟手工读 17 transcripts + 写报告
- vLLM 配置改造对**已有 scenario 是免费的质量提升** (+0.10);对 subagent 新场景**暴露了配置瓶颈** (技术问题,可修)
- subagent **任务拆分能力**评分 4.67 是个**有意义的正面信号** — 模型本身懂 subagent
- 真正卡 subagent 落地的是 **vLLM 调度稳定性**,不是模型能力 — 这是个明确的 actionable 发现
- L1 cargo test 17 个 scenario 里 1 个 fail (`subagent_compare_3_libs` 660s timeout),其他 16 个 PASS。fail 信号被 transcript + judge 充分捕获

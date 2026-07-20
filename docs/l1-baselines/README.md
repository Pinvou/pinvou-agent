# L1 Judge Baselines

锚定历史 L1 跑结果作为质量参照系。改了 INSTRUCTIONS_MD / bridge / 模型 / system-reminder 后,新跑跟最近 baseline diff 看质量漂移。

## 目录结构

```
docs/l1-baselines/
├── README.md (本文件)
└── v<app_ver>-r<rubric_ver>/          ← 一次 baseline (双版本号)
    ├── <scenario>.md × N              ← harness 落的 transcript
    └── judge-report.md                ← Claude 按 rubric 评分报告
```

**命名维度** (`v<app_ver>[-<vllm_tag>]-r<rubric_ver>[-<suffix>]`) 的意义:
- `v<app_ver>` 标 pinvou3-app 代码版本 + INSTRUCTIONS_MD
- `<vllm_tag>` (可选) 标 vLLM 配置变更 (例 `vllm2` = 256K context + prefix-caching + chunked-prefill + max-num-seqs 8 优化)
- `r<rubric_ver>` 标评分尺子 (`docs/L1-judge-rubric.md` 当前版本)
- `<suffix>` 可加 scenario 集合大小 (例 `13scn` / `17scn`)
- 跨 r 版本的分数**不可直接 diff** (4 分@r1 跟 4 分@r2 不是一个尺子,见 rubric §6)
- 跨 vllm 配置的分数**可以 diff**但要意识到底座变了 (例 `r1-13scn` vs `vllm2-r2-17scn` 内 13 老 scenarios diff 有效)
- rubric bump 后,旧 baseline 文件夹不动,新跑用新 rubric 起新文件夹

## 已有 baseline

| 文件夹 | 日期 | scenario 数 | 总均分 | 说明 |
|---|---|---|---|---|
| `v0.8.37-r1` | 2026-05-18 | 5 | 4.75 | 首次 baseline (MVP scenario 集),Qwen3.6 + L1.5 工具表 + INSTRUCTIONS_MD v0.8.37,rubric r1 |
| `v0.8.37-r1-13scn` | 2026-05-18 | 13 | 4.67 | 扩 scenario 集 (multi_turn/write_okr/data_csv/plan_travel/refusal/long_output/chinese/tool_err),同 app/rubric 版本。3 个 ≤3 离群点已 append 到 process.md |
| `v0.8.37-vllm2-r2-17scn` | 2026-05-18 | 17 | 4.46 (老 13 子集 4.77 +0.10) | **vLLM 参数优化** (256K context + prefix-caching + chunked-prefill + max-num-seqs 8) + rubric r2 (加 2 维 subagent 评估) + 加 4 个 subagent scenarios。老 scenarios 整体 +0.10 (plan_travel_web 修复 + multi_turn 不再 overkill),但 subagent 链路 3/4 SSE 失败暴露 vLLM 调度问题 |
| `v0.8.37-vllm2_clientfix-r2-18scn` | 2026-05-18 | 18 | 4.50 | **Fork patch 双修**: Bug #1 turn_loop role=user (避免 Qwen chat_template raise) + Bug #2 stream_open_timeout 180s。新加 `subagent_single_simple` 验证 1 个 subagent 完美 19s。**subagent 拆分 4.80 + 综合 5.00**,链路真正可用。但 ≥3 个 long-prompt subagent 仍受 vLLM 调度限制 (Bug #3 trade-off)。老 scenarios 有 LLM 抽奖 regression (multi_turn_t2 / plan_travel_web / chinese_idiomatic) |
| `v0.8.37-vllm3_planB-r2-18scn` | 2026-05-18 | 18 | **4.59** | **vLLM plan B**: max-num-batched-tokens 32768→131072 (4×) + 保留 chunked-prefill + 保留 256K context。**老 13 scenarios 4.67→4.71** (multi_turn/plan_travel/chinese/tool_error_recovery/reasoning_off 5 个改善)。**新发现**: subagent 大任务 timeout **不是 vLLM 调度问题**,是 subagent 内部 LLM workflow 慢 (research_academic 504s 仍 running)。`long_output_1500` 新 regression: LLM detour 调 web_search + 写 12K 字 cargo timeout |
| `v0.8.37-vllm3_final-r2-18scn` | 2026-05-19 | 18 | **4.66** | **完整 patch 栈 + INSTRUCTIONS_MD v4**: Fork A (subagent stop-on-fail) + B (max_subagents=1 工程锁) + C (elapsed cap 300s) + C+ (max_steps 100→20) + INSTRUCTIONS_MD 整理 (164→112 行,新增 §3 任务完成定义)。**老 13 scenarios 4.67→4.83 (+0.16 vs 最初)**。`long_output_1500` 修了 (3.25→4.75 §3 起效),`subagent_one_fails` 主 agent 改用 checklist_write 拆分串行。`subagent_compare_3_libs/research_topic` 仍 cargo timeout 是已知边界 (复杂研究任务需 >10 分钟主 agent 拼数据) |

**suffix 约定**: 文件夹后缀 `-<N>scn` 表示 scenario 集合大小,用于区分相同 app/rubric 版本下不同 scenario 集合的 baseline。覆盖更全的集合 (≥) 可作为后续 baseline 的 ground truth。

## 专项重评估 (非常规 baseline,无总均分)

| 文件夹 | 日期 | 说明 |
|---|---|---|
| `v0.8.45-subagent-reeval-2026-05-26` | 2026-05-26 | **多 subagent 后端重评估 + 设计决策** (`max_subagents=4` 跑 C-0~C-4)。后端并发瓶颈已消除(N=4 first-token <1s)。底座修复后 C-1 3/3 完成、C-2 2/4 完成，但管理开销极大(eval×12+重试)。**关键决策**: 弱模型(Qwen3.6)下多 subagent 并行研究模式 ROI 为负，废弃。保留单 subagent，复杂研究改为主 agent 直接工具调用或串行单任务子 agent。详见 `report.md` §7 |

## 怎么用

### 1. 锚一份新 baseline

```bash
cargo test --test l1_dialog_harness -- --ignored --test-threads=1
# 拿到新 ts (target/l1-runs/<ts>/)
# 跟 Claude 说: "评一下 target/l1-runs/<ts>"
# 拿到 judge report 后(report 文件名形如 <ts>-r<N>-report.md):
ts=<ts>
ver=v<app_ver>-r<rubric_ver>     # 例: v0.8.38-r1
mkdir -p docs/l1-baselines/$ver
cp pinvou3-app/src-tauri/target/l1-runs/$ts/*.md docs/l1-baselines/$ver/
cp pinvou3-app/src-tauri/target/l1-judge/$ts-r*-report.md docs/l1-baselines/$ver/judge-report.md
# 更新本 README 表格
git add docs/l1-baselines/$ver/ docs/l1-baselines/README.md
git commit -m "锚 L1 baseline $ver"
```

### 2. 跟历史 baseline diff

```
跟 Claude 说: "对比 docs/l1-baselines/<ver>/ 跟 target/l1-runs/<新ts>/"
```

Claude 读两边 judge-report 的总览表 + 关注 ±0.5 以上的维度变化,**diff 报告告诉哪些维度漂了、可能原因、是否要 rollback**。

### 3. 锚 baseline 的时机

- 每个 release tag 前 (release-v0.8.37 / release-v0.9.0 / ...)
- 改 INSTRUCTIONS_MD 大块内容前/后
- 升级 vLLM 或 Qwen 模型版本前/后
- 改 system-reminder 文案前/后

平时改代码不需要锚——只在"可能影响 LLM 输出质量"的改动前/后锚。

## 注意

- `target/l1-runs/` 跟 `target/l1-judge/` 在 `.gitignore` 内(cargo build 产物目录),不可作为长期参照
- `docs/l1-baselines/` 进 git,跨 worktree / 团队 / 时间维度可看
- rubric 改版本时,旧 baseline 的 judge-report 用的是旧 rubric,diff 要谨慎(rubric v1 vs v2 同个 4.5 分意思可能不同)

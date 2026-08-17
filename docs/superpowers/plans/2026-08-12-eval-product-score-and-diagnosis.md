# Eval Product Score and Product Diagnosis Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 Product smoke 增加可解释的确定性 0–100 健康评分和面向产品的改进摘要，同时保持 Judge 分数独立，并明确公开榜单不可比边界。

**Architecture:** 在 `eval/analysis` 下新增独立 `product_score` 深模块，输入仅为安全的 `EvalRecord` 与 finding，输出版本化评分、扣分明细和聚合产品诊断。Markdown 只负责安全展示；CLI/GUI 在规则分析后计算评分，并将可选总分/版本写入 JSONL complete。评分不读取 `EvalRecord.analysis`，也不依赖 Judge。

**Tech Stack:** Rust 2021、Serde、现有 eval analysis/Markdown/JSONL 模块、Tokio 测试、Cargo、Python architecture guard。

---

## File responsibility map

- Create `pinvou3-app/src-tauri/src/features/assistant/eval/analysis/product_score.rs`: 评分公式、扣分去重、等级和产品问题聚合。
- Modify `pinvou3-app/src-tauri/src/features/assistant/eval/analysis/mod.rs`: 导出评分领域类型和入口。
- Modify `pinvou3-app/src-tauri/src/features/assistant/eval/markdown_report.rs`: 渲染问题摘要、健康评分、扣分解释和榜单边界。
- Modify `pinvou3-app/src-tauri/src/features/assistant/eval/report.rs`: JSONL complete 增加可选总分与公式版本。
- Modify `pinvou3-app/src-tauri/src/eval_cli.rs`, `app/commands/eval.rs`, `lib.rs`: CLI/GUI 接线与非敏感 outcome。
- Modify `pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs`: 评分、聚合、Markdown、隐私与 JSONL 回归。
- Modify `PROGRESS.md`: 使用与可比性说明。

### Task 1: Implement the deterministic Product Score domain

**Files:**
- Create: `pinvou3-app/src-tauri/src/features/assistant/eval/analysis/product_score.rs`
- Modify: `pinvou3-app/src-tauri/src/features/assistant/eval/analysis/mod.rs`
- Test: `pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs`

- [ ] **Step 1: Impact-check and write failing score tests**

Run GitNexus upstream impact for `EvalFinding`, `RuleAnalysis`, and `merge_findings`; if unavailable, use:

```powershell
rg -n "EvalFinding|RuleAnalysis|merge_findings" pinvou3-app/src-tauri/src -g "*.rs"
```

Add `product_score_` tests for: clean non-empty run = 100/Excellent; empty records = unavailable; tool failure only deducts tool reliability by 30; unexpected tool use only deducts constraints by 25; latency outlier only deducts performance by 12; duplicate key deducts once; unknown ID and Judge findings do not deduct; dimensions floor at zero; total uses fixed integer weights.

- [ ] **Step 2: Verify RED**

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo test --lib product_score_ --no-run
```

Expected: compile failure because `calculate_product_score` and score types do not exist.

- [ ] **Step 3: Implement the versioned score model**

Define `PRODUCT_SCORE_VERSION = "pinvou-product-score/v1"`, `ProductScore`, `ProductGrade`, `ProductScoreDimensions`, `ProductScoreDeduction`, and `ProductScoreConfidence`. Use an exact allowlist mapping from known rule finding/status to dimension and deduction. Deduplicate by `(id, case_id, evidence)`, ignore Judge/unknown findings, use `u16` intermediates, clamp dimensions to `0..=100`, and calculate:

```text
(task_completion*35 + tool_reliability*25 + constraint_adherence*15
 + performance_efficiency*15 + runtime_stability*10 + 50) / 100
```

Return unavailable for empty records; mark 1–9 records low-sample and 10+ standard.

- [ ] **Step 4: Verify GREEN and commit**

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo test --lib product_score_ --no-run
git add pinvou3-app/src-tauri/src/features/assistant/eval/analysis/product_score.rs pinvou3-app/src-tauri/src/features/assistant/eval/analysis/mod.rs pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs
git commit -s -m "feat(eval): 增加确定性产品健康评分"
```

Expected: compile succeeds; actual execution is attempted when the Windows loader permits it.

### Task 2: Aggregate findings into actionable product diagnoses

**Files:**
- Modify: `pinvou3-app/src-tauri/src/features/assistant/eval/analysis/product_score.rs`
- Test: `pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs`

- [ ] **Step 1: Write failing diagnosis tests**

Add `product_diagnosis_` tests asserting `summarize_product_problems` returns area, highest priority, affected case count/IDs, Chinese conclusion, action, and measurable acceptance. Cover tool failure, missing tool, forbidden tool, repeated calls, latency, high-token latency, cache ratio, case failure/timeout, same-area aggregation, stable order, five-area cap, and no-finding fallback.

- [ ] **Step 2: Verify RED**

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo test --lib product_diagnosis_ --no-run
```

Expected: compile failure because diagnosis types/functions do not exist.

- [ ] **Step 3: Implement fixed safe templates**

Define `ProductDiagnosis` and `ProductProblemArea`. Map known finding IDs to the approved Chinese templates. Aggregate by area, keep highest severity, sort/deduplicate safe case IDs, use numeric counts plus existing safe evidence, and cap at five areas. Rule facts win over Judge inference for the same area; usable Judge-only diagnosis keeps `[AI 推断]` provenance.

- [ ] **Step 4: Verify GREEN and commit**

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo test --lib product_diagnosis_ --no-run
git add pinvou3-app/src-tauri/src/features/assistant/eval/analysis/product_score.rs pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs
git commit -s -m "feat(eval): 归纳产品问题与验收方向"
```

### Task 3: Render score and diagnoses in Markdown

**Files:**
- Modify: `pinvou3-app/src-tauri/src/features/assistant/eval/markdown_report.rs`
- Modify: `pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs`

- [ ] **Step 1: Write failing report tests**

Extend the wished-for `EvalMarkdownReport` with score/diagnoses. Assert the fixed order: 运行结论 → 产品问题与改进方向 → 产品健康评分 → 关键指标 → 逐用例诊断 → 工具与性能观察 → 确定性规则发现 → 独立 Judge 质量评分 → P0/P1/P2 → 限制说明.

Assert total/grade, five subscores, formula version, deduction reasons, conclusion/action/acceptance, low-sample warning, `公开榜单分数：不可用`, and BFCL incompatibility reason. Empty input shows unavailable; no findings gives an honest “未发现规则可识别的问题” plus larger-sample advice. Include privacy sentinels and assert no prompt/answer/raw error/credential appears.

- [ ] **Step 2: Verify RED**

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo test --lib markdown_product_score_ --no-run
```

Expected: compile failure because the renderer lacks score/diagnosis inputs.

- [ ] **Step 3: Implement safe rendering**

Insert the two new sections after run conclusion, reuse `markdown_text` and the credential guard, and render only deterministic domain values. Rename Judge heading to `独立 Judge 质量评分`; on failed/not-configured explain that Product Score remains valid and separate.

- [ ] **Step 4: Verify all Markdown tests and commit**

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo test --lib markdown_product_score_ --no-run
rustup run stable-x86_64-pc-windows-msvc cargo test --lib markdown_report_ --no-run
git add pinvou3-app/src-tauri/src/features/assistant/eval/markdown_report.rs pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs
git commit -s -m "feat(eval): 突出产品评分与改进摘要"
```

### Task 4: Wire CLI, GUI, JSONL, and documentation

**Files:**
- Modify: `pinvou3-app/src-tauri/src/features/assistant/eval/report.rs`
- Modify: `pinvou3-app/src-tauri/src/eval_cli.rs`
- Modify: `pinvou3-app/src-tauri/src/app/commands/eval.rs`
- Modify: `pinvou3-app/src-tauri/src/lib.rs`
- Modify: `pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs`
- Modify: `PROGRESS.md`

- [ ] **Step 1: Impact-check and write failing wiring tests**

Impact-check `EvalReportWriter::finish`, `finalize_eval_outputs`, `run_product_eval_smoke`, and GUI `run_eval_smoke`. Add tests requiring JSONL complete optional fields `product_score` and `product_score_version`, omission for empty runs, CLI outcome total/version, Judge failure score invariance, and identical GUI/CLI score for identical records/findings.

- [ ] **Step 2: Verify RED**

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo test --lib eval_product_score_wiring_ --no-run
```

Expected: compile failure because finish/outcome/report construction lack score fields.

- [ ] **Step 3: Implement composition-root wiring**

After `analyze_rules` and before Judge, calculate score from records and rule findings. After merge, generate diagnoses for Markdown; never recalculate score from Judge findings. Extend JSONL complete with optional `u8` score and version. Preserve ordering: rules/score → Judge → merge/diagnosis → JSONL → Markdown. Public outcome exposes only total/version, not deductions.

- [ ] **Step 4: Update documentation**

Document five dimensions, formula version, same-case/model/config prerequisites, three-run median recommendation, and the requirement for future `official-compatible` BFCL comparison.

- [ ] **Step 5: Run complete verification**

```powershell
git diff --check
python scripts/architecture-guard.py
rustup run stable-x86_64-pc-windows-msvc cargo check --bin eval_smoke --features dev-tools
rustup run stable-x86_64-pc-windows-msvc cargo test --lib eval_ --no-run
rustup run stable-x86_64-pc-windows-msvc cargo run --bin eval_smoke --features dev-tools -- --mode product
```

Expected: build/test-link pass; real Markdown contains score, five subscores, actionable product diagnosis, measurable acceptance, and BFCL boundary. Privacy scan returns zero; no `.tmp` or `eval_*` session remains.

- [ ] **Step 6: Final audit and commit**

Run GitNexus `detect_changes({scope: "compare", base_ref: "main"})`; if unavailable, audit `git diff --name-only 1d7428a6..HEAD` and full diff.

```powershell
git add pinvou3-app/src-tauri/src/features/assistant/eval/report.rs pinvou3-app/src-tauri/src/eval_cli.rs pinvou3-app/src-tauri/src/app/commands/eval.rs pinvou3-app/src-tauri/src/lib.rs pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs PROGRESS.md
git commit -s -m "feat(eval): 串联产品评分与报告输出"
```

## Final acceptance checklist

- [ ] Product Score is deterministic, versioned, bounded, and transparent.
- [ ] Judge status cannot change Product Score.
- [ ] Diagnoses include conclusion, affected scope, action, and measurable acceptance.
- [ ] Empty/small samples do not imply unwarranted confidence.
- [ ] JSONL remains backward compatible.
- [ ] Markdown retains privacy and atomic no-overwrite behavior.
- [ ] Product Score is explicitly not a BFCL score.
- [ ] Real artifacts contain no raw prompt/answer/tool IO/error detail.

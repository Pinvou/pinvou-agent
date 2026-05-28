---
name: h3c-ppt
description: >-
  H3C 集团对外 PPT(HTML deck)全流程工具 — 双轨 playbook(轻量轨 6 步快做内部/小型 deck · 全量轨 16-phase 重型外部交付)+ 自动 audit 脚本 + 5 类页面母板 + 一键 build mega 单文件. 既用于新项目接活(调研/大纲/设计/实现/图像生成 QA loop/集成映射),也用于既有 deck 的审查打包. 触发短语 "新项目要做 PPT / 客户让我做 deck / playbook / 全流程 / 接活 5 问 / pipeline / 改完了 / 审一遍 / 走一遍体检 / audit / 检查 PPT / 准备打包 / 送审前 / build mega / 重 build". 在 Edit/Write 了 **/HTML_Deck/slides/*.html 之后主动触发跑 audit + 提议 rebuild. 自包含 — scripts/ 含字号字体/页码章节连贯/截图清洁度 OCR/浏览器渲染审查/build 工具;templates/ 含 H3C 蓝版 5 类页面母板 + 品牌资产(背景图 + logo) + base.css;reference/ 含设计 token / 8 条内容铁律 / 历史踩坑修法目录. 通用 H3C 蓝版,不绑定具体项目.
phases: p0:五问|p1:收料|p2:调研*|p3:决策|p4:大纲|p5:CLAUDE.md|p6:设计 spec|p7:HTML 实现|p8:图像生成|p8_5:图像 QA*|p9:图像集成*|p10:结构审计|p11:内容审计|p11_5:内部签字|p12:Build|p13:交付|p14:反馈*
demo_file: demo/sample-deck.html
demo_desc: H3C 蓝版 5 页脱敏示例 deck — 封面 / 章节扉页 / 标题正文 / 三点要点 / KPI 数字页, 真实呈现成品视觉风格
demo_duration: 轻量轨 ~20-40 分钟(≤30 页)· 全量轨 4-6 小时(80-150 页)
---

# H3C PPT Workflow · 16-Phase End-to-End Playbook

The end-to-end workflow for producing an H3C external corporate presentation (typically a customer-facing or government-facing HTML deck with 80-150 pages, blue-aurora visual system). The workflow is built from a real production run that delivered a multi-chapter deck on a tight timeline, and is intentionally generic across projects.

This skill answers two questions:

1. **For someone new to the workflow:** "What are the steps, in what order, and what does each step actually produce?"
2. **For someone in the middle of a project:** "Which phase am I in, what's the loop exit condition, and what's the next phase?"

The workflow has **16 phases** and **4 explicit loops**. Each loop has a stop condition based on what actually worked in past projects — not "until perfect," but "until [a specific signal]". Read the loop section before beginning a project so you know in advance what "done" looks like at each loop.

---

## Two lanes — choose one right after Phase 0

This playbook runs in two lanes. **Most pinvou3 requests are Light.** Decide from the five-question answers, state the chosen lane in one line, then follow only that lane.

- **Light lane (default)** — internal / quick decks. Choose Light when **all** hold:
  - Audience is internal (boss, own team, internal review) — not an external customer / government reviewer
  - Page count ≤ ~30
  - No AI image generation needed (text + diagram deck, or reuse of existing assets)
  - No formal customer-review or multi-round external delivery cycle required
- **Full lane** — external 80-150 page production. Choose Full if **any** Light condition fails (external/government audience, large page count, image-heavy, formal review gates).

When in doubt for an internal request, pick **Light**. The Light lane uses a 6-milestone subset of the same phase ids — **`p0 → p4 → p6 → p7 → p10 → p12`** — and legitimately **skips** p1/p2/p3/p5/p8/p8_5/p9/p11/p11_5/p13/p14. After `p0`, emit the `p4` marker directly; skipping here is expected, not a violation.

> **Dependency note.** Full-lane phases p8/p8_5 (AI image generation) need the `product-scene-gen` skill; if it isn't installed, build an image-free deck (text + diagrams) or reuse existing assets. Delivery (p13) is a local file handoff and feedback (p14) is in-chat — no external skill required. The Light lane needs none of these.

---

## Light lane · condensed playbook

Six milestones. Each: do this → produce this → stop. Don't pull in Full-lane research / image / review machinery.

**p0 · Five questions.** Ask the five questions (see Phase 0 below), then from the answers confirm the lane. Output a one-line intake note.

**p4 · Outline.** Draft the deck outline directly from the brief: total pages, section splits, one action-title takeaway per page. Save `大纲.md`. (Light folds the old p1-p3 research/decision into this one step — an internal deck has no external scenarios to audit.)

**p6 · Design spec (lightweight).** Copy this skill's `templates/base.css` to `HTML_Deck/assets/base.css` and reuse the 5 layouts (`L01`-`L05`) **as-is** — don't design from scratch. Only if a token genuinely must change, note it in a short `HTML_Deck/DESIGN.md`; otherwise skip the doc.

**p7 · HTML implementation — chunked, multi-file. (This is exactly where a past run stalled.)**
- Same layout as the Full lane: `HTML_Deck/index.html` (host shell + `SLIDES = [...]` order array) and `HTML_Deck/slides/NN_name.html` — **one file per slide.** (Use `HTML_Deck/` so the p10/p12 scripts and the skill's auto-trigger find your slides.)
- **Never write the whole deck — or multiple slides — in a single `write_file`.** One slide is one small file. Use `append_file` only for tail-appending content in final file order; if you create placeholders inside an existing slide file, replace them with `edit_file` / `apply_patch`.
- Why: a huge `content` arg out-runs the SSE idle timeout on local inference, the stream is cut mid-arguments, and the call comes back truncated ("missing required field 'content'"). One small file per slide avoids it entirely. A past pinvou3 run dumped the whole deck into one root `.html` and stalled twice — don't repeat it.
- Reuse the `L01`-`L05` templates per page type.

**p10 · Structural audit.** Run the bundled scripts on the deck folder:
```bash
bash <skill>/scripts/run_all.sh /abs/path/to/HTML_Deck
```
Fix until exit 0 (or consciously accept warnings).

**p12 · Build.** `bash <skill>/scripts/rebuild_mega.sh /abs/path/to/HTML_Deck` → single-file `mega.html`. Report its path to the user — done.

*Optional, only if it surfaces:* a quick content sanity pass (plain language, no overclaiming) before p12.

---

## Full lane · Quick map · all 16 phases

| # | Phase | Loop? | Output | Tools |
|---|---|---|---|---|
| 0 | Pre-engagement five questions | — | A one-page intake note | conversation |
| 1 | Receive brief · gather raw materials | — | `配套材料/产品资料/` | manual upload |
| 2 | Research audit ★ | **Loop A** | `配套材料/0X_*调研.md` ×3-4 | parallel research subagents |
| 3 | Decision: cut / change / add | — | A reasoned scenario list with citations | LLM on research reports |
| 4 | Content outline | — | `V1 PPT 大纲.md` | LLM on master + decisions |
| 5 | Project `CLAUDE.md` | — | Project root `CLAUDE.md` | LLM on research |
| 6 | Visual design spec | — | `HTML_Deck/DESIGN.md` + `assets/base.css` tokens | designer or Stitch / Figma |
| 7 | HTML deck implementation | — | `index.html` + `slides/*.html` × N | LLM coding |
| 8 | Image generation (8a select model · 8b batch generate · 8c cost track) | — | `assets/scene-library*/` | DashScope / Imagen / etc. |
| 8.5 | Image QA closed-loop ★ | **Loop B** | `_audit/13_image_qa_round_*.md` + cleaned images | VLM scorer + PIL/cv2 post-processing |
| 9 | Per-slide image integration mapping | **Loop C** | `_audit/07_image_integration_decisions.md` (terminal version) | LLM mapping + decision-maker review |
| 10 | Visual / structural audit | — | clean PASS report | **bundled audit scripts** |
| 11 | Content guardrails audit (11a 8-rule LLM walkthrough · 11b UI Designer role review) | — | `_audit/03_ui_final_review.md` | **bundled audit scripts** + role-play subagent |
| 11.5 | Pipeline close · internal sign-off | — | `_audit/09_pipeline_close.md` | decision-maker walks the deck once |
| 12 | Build & package | — | `<DECK_ROOT>_inline/mega.html` + `slides.bak.<date>/` | `scripts/rebuild_mega.sh` |
| 13 | Delivery | — | `mega.html` path handed to the user (+ optional zipped source) | local file handoff |
| 14 | Feedback loop · revision | **Loop D** | each revision = one back-and-forth iteration | conversation + re-trigger phase 7-12 |

★ = High-leverage phase (most projects fail in phases 2 / 8.5 / 9 / 14 if you skip the loop)

---

## Phase 0 · Five questions

Get five short answers before opening any tool — these also decide the lane (see **Two lanes**). Keep it to a quick exchange; don't interrogate.

1. **Audience — internal or external?** Boss / own team / internal review → Light. External customer / government bureau / board → Full. (Who exactly will flip through this?)
2. **Purpose — the one takeaway.** The single thing the audience must walk away knowing or deciding. Get this right before any visuals.
3. **Scale — how many pages?** A rough page count / size. (≤ ~30 → Light; 80-150 → Full.) Don't ask about deadlines — an autonomous run can't schedule to one, and "ship at the deadline" already lives in the Full-lane Loop D. A second version for a different audience is a later re-run (phase 14), not an up-front question.
4. **Must-haves & constraints.** Real data sources, specific products / screens, images needed? Any hard "don't say X" red lines? And is this a bespoke build, a quick template adaptation, or a full repositioning?
5. **(External / Full lane only) Review mechanism & our role.** Who on the customer side reviews / has veto power, the language register they expect (formal government / commercial / technical), and whether we're the lead vendor / strategic partner / sub-contractor / tech-slot provider. (Determines the "humble voice" register in phase 11.)

Output a one-line intake note plus the chosen lane, and pin it — phases 5 (CLAUDE.md) and 11 (content guardrails) depend on these answers. For an internal request, default to Light.

---

## Phase 1 · Receive brief · gather raw materials

Drop everything the customer / requestor has given you (typically a master PDF or PPT) plus internal product collateral (datasheets, prior decks, real product photos, logos) into `配套材料/` and `配套材料/产品资料/`. Don't curate yet — at this stage volume beats taste, since phase 2 will sift.

Also collect the relevant industry policy / regulation primary sources (PDF or web URLs). Don't rely on second-hand summaries; the audit in phase 2 needs to cite primary documents with file numbers.

---

## Phase 2 · Research audit (Loop A)

The single most decision-determining phase. Get 3-4 research reports out before writing a single slide.

**Standard research cuts** (do all of them in parallel):

- **Policy depth** — read the primary regulations / file numbers / official speeches in the relevant industry; extract the "iron facts" the deck will cite. Identify red lines (what claims would trigger a regulator / put the customer at risk).
- **Industry / market reality** — current market state, key players, recent shifts, customer baseline pain points (cite with sources)
- **Scenario feasibility audit** — for every proposed scenario in the master PDF, mark ✅ / 🟡 / ❌ on (a) physical plausibility, (b) regulatory compliance, (c) reference-case existence, (d) customer-side feasibility
- **P0 incremental finds** — anything new (recent news, recent regulation change, recent failure case) that shifts the calculus

**How to run.** Spawn parallel research subagents (general-purpose agent, one per cut) with very specific search briefs. Don't do them serially — research is independent and benefits from concurrent reading.

**Loop A — when to stop researching.**

The trap: you can always research more. Stop when:

- Every scenario the deck wants to claim has a case-driven reference, or has been explicitly demoted to "exploratory direction" (not commitment)
- Every numeric claim has a primary source with a file number
- Every red line a reviewer might flag has been audited (✅ / 🟡 / ❌) with a one-line justification

If a scenario is still 🟡 after research, decide explicitly with the decision-maker whether to keep it as exploratory or cut it. Don't carry unresolved 🟡 into phase 3.

---

## Phase 3 · Decision: cut / change / add

Walk the master PDF / outline page-by-page with the research reports open. For each scenario, decide one of:

- **Cut** — fails physical / regulatory / feasibility audit; remove from scope
- **Change** — keep the slot but reframe (e.g. "we do X" → "we support customer doing X")
- **Add** — new scenario surfaced by research that's stronger than what's in the master

Document each decision with a one-line "why" referencing the research report section. The output is a "X cut / Y changed / Z added" list with citations.

---

## Phase 4 · Content outline

With decisions in hand, draft the chapter structure: total page count, chapter splits, per-page takeaway sentence (action title), and the narrative arc. Save as `V1 PPT 大纲.md`.

This is "what the deck says" before "what the deck looks like." Two pages can have identical visuals but different takeaways — get the takeaways right first.

---

## Phase 5 · Project `CLAUDE.md`

Anchor the project's standing rules in a `CLAUDE.md` at the project root. Standard sections:

- **§0 Language rules** — three registers (government / commercial / customer-facing) with switching rules; technical-jargon blacklist
- **§0.6 Role discipline** — explicit "we do" list and "we don't" list (informed by phase 0 answer about our role)
- **§Compliance redlines** — industry-specific (informed by phase 2 policy research)
- **§Data sources** — single source of truth for every numeric claim used in the deck (so revisions don't drift)
- **§Reference materials** — pointers to the research reports and master PDF

This file is the contract between you (Claude) and the human reviewer for the rest of the project. When phase 11 audits content, it audits against this file.

---

## Phase 6 · Visual design spec

Either inherit an existing H3C corporate design system, or design the deck-specific spec. Either way the output is two files:

- `HTML_Deck/DESIGN.md` — the rules (color palette, font-size scale, font stacks, grid, anti-patterns). The full H3C blue-aurora design system is documented in this skill at `reference/design_tokens.md`.
- `HTML_Deck/assets/base.css` — the rules in code (CSS variables for colors, font-sizes, spacing). All slide HTML files reference these tokens.

If working from an external design package (Stitch / Figma): read the `feedback_design_anchors_content` and `feedback_never_alter_design` patterns — translate values exactly, do not silently modify.

---

## Phase 7 · HTML deck implementation

Build the actual slides:

- `HTML_Deck/index.html` — the host shell with the `SLIDES = [...]` array defining playback order (this array is the single source of truth for total page count and chapter sequence in phases 10/11)
- `HTML_Deck/slides/*.html` — N self-contained pages, each with inline `<style>` overriding base.css where needed

Conventions that have proven robust:

- **One page = one small file. Never write multiple slides (or the whole deck) in a single `write_file` call** — a large `content` arg out-runs the SSE idle timeout on local inference and the call gets truncated ("missing required field 'content'"). Use `append_file` only for tail-appending content in final file order; use `edit_file` / `apply_patch` for placeholders or middle-of-file edits.
- Each page is self-contained (links to `../assets/base.css` only) — easier to debug and easier to inline for build
- Use master / scene page pairs for story-driven pages (master = takeaway + data, scene = in-situ photo)
- Page chapter labels (`CH.NN`) on scene pages must match their master — phase 10 audit will catch drift

---

## Phase 8 · Image generation

Three sub-phases:

**8a · Model selection.** Different scenes need different models. Spend 30 minutes generating the same prompt across 2-3 candidate models, evaluate on (a) physical fidelity (does the product look right), (b) cultural fidelity (do the people / setting look right), (c) text rendering quality. Pick one primary model + one fallback for failures.

The non-obvious result from past runs: for Chinese-language UI screens, **no current image model renders Chinese reliably** — see phase 8.5 below + `reference/known_smells.md` S6.

**8b · Batch generate.** Run all scenes through the chosen model. Track the cost per image and the total budget. A typical 50-80 scene generation pass runs ~10-30元 at China-domestic rates.

**8c · Cost / quality report.** Before moving on, output a `_audit/02_image_gen_report.md` with:
- model used, parameters, total cost, total time
- pass / fail counts (any prompts that got auto-moderated, any failures)
- a quality summary (which scenes look great, which need rework)

---

## Phase 8.5 · Image QA closed-loop (Loop B) ★

The single most-loop-heavy phase. Almost no batch generation comes out clean on round 1. Plan for 3-4 rounds.

**Standard round structure:**

1. **VLM scoring** — pass each generated image to a vision-LLM (qwen-vl-max or comparable) with a per-scene scoring rubric. Common dimensions:
   - Product fidelity (does the depicted hardware / UI match real product)
   - Cultural fidelity (do depicted people / setting match the audience)
   - Text correctness (any English placeholders, AI Chinese garbage, watermarks)
   - Composition (anything visually broken)
2. **Identify failures** — output `_audit/13_image_qa_round_<N>.md` listing which scenes scored below threshold and why
3. **Post-processing fix attempts** (in order of preference, most reliable first):
   - **PIL overlay** — for screens with wrong text, paint correct Chinese text directly with Pillow + a real CJK font (this is the only reliable way to get Chinese text on an image)
   - **cv2 perspective warp** — for screens with perspective distortion, warp a clean PIL-rendered panel onto the four screen corners
   - **PIL whiteout for chrome** — for AI watermarks (e.g. platform UI badges), paint white rectangles over them
   - **Re-generate** — only as a last resort, since this changes the image entirely; only use for scenes with composition failures

**Loop B — when to stop iterating.**

In practice, 3 rounds usually clears the deck. Stop when:

- All scenes meet a threshold score (commonly ≥ 24/30 on three dimensions)
- The two highest-leverage flaws called out by the human reviewer at round 0 are gone (e.g. "the product doesn't look right" and "the screen text is gibberish" — these usually swamp everything else)
- Further rounds aren't lifting scores (scores plateau)

The wall-clock time for this loop on a real 50-scene deck has been ~90 minutes total across 3-4 rounds. Budget for it.

See `reference/known_smells.md` (S6, S7, S8) for the specific recipes.

---

## Phase 9 · Per-slide image integration mapping (Loop C)

Decide which generated image goes on which slide. This produces a per-slide-config decision table.

**The reason this needs a loop:** the first version is often "conservative" — every slide gets the most obvious-fit image, leaving high-impact slides under-decorated. The decision-maker often comes back with "this is too cautious, I want stronger visuals on the hero pages." Then v2 is bolder. Sometimes v3.

**Standard outputs** (each round = one document):

- `_audit/07_image_integration_decisions.md` — round 1 decision table
- `_audit/10_image_integration_v2.md` — round 2 (after reviewer pushback)
- `_audit/11_per_slide_config.md` — increment / refinement
- `_audit/12_final_image_integration.md` — terminal

**Loop C — when to stop iterating.**

- Every slide that should have an image has one (no empty slots without explicit reason)
- Every image is used (no orphan images sitting unused)
- The decision-maker has signed off on the table (often "OK" in chat is enough)

---

## Phase 10 · Visual / structural audit

Run the bundled audit scripts (this skill's `scripts/` directory). They auto-check:

- Font-size 6-tier compliance (A)
- Page-number X/Y consistency, CH chapter sequence, page-footer right margin (B)
- Demo screenshot cleanliness (C)
- One-click rebuild (E)

**Trigger the skill explicitly** by saying "audit" or letting the skill activate from its own description.

The standard call:
```bash
bash ~/.claude/skills/h3c-ppt/scripts/run_all.sh /abs/path/to/HTML_Deck
```

Exit code 0 = clean, 1 = warnings, 2 = hard failures. Don't proceed to build until exit 0 or you've explicitly accepted the warnings.

---

## Phase 11 · Content guardrails audit (two passes)

Two distinct passes — both needed.

**11a · 8-rule LLM walkthrough.** Walk the deck page-by-page against the 8 rules in `reference/content_redlines.md` (plain language, role discipline, humble voice, case-driven, physical plausibility, compliance edges, data freshness, design fidelity). Flag and fix anything that fails.

**11b · UI Designer role review.** Spawn a subagent in the role of a UI Designer who is sitting next to the customer at the moment they flip through the deck. The subagent's job is to score each page on a 4-tier scale:

- **A** — directly usable in front of customer, no shortfalls
- **B** — usable but minor polish would help
- **C** — needs work, has a noticeable issue
- **D** — must rework before showing customer

Save as `_audit/03_ui_final_review.md`. The reason this beats a self-audit: the UI Designer role is allowed to say "this slide is boring" or "this slide doesn't say anything specific" — feedback you wouldn't easily say about your own work.

**Working mode that has worked best in past runs:** "audit + immediate fix as you go," not "audit-everything-then-fix-everything." Each page's issues are usually small enough to fix in seconds. Walking the deck twice is more expensive than fixing inline.

---

## Phase 11.5 · Pipeline close · internal sign-off

The decision-maker walks the deck end-to-end one final time, in slideshow mode (full-screen, advance one page at a time, read the speaker view). This is not an audit — it's a "would I send this if I had to send it right now" judgment call.

Common findings here are different from phase 10/11 — they're "the story doesn't flow" or "page 47 should come before page 35" — narrative-level, not visual or content-level. Be ready to re-order slides, not just edit them.

Save the close as `_audit/09_pipeline_close.md` even if it's a one-line "OK to ship" — the timestamp is useful evidence later.

---

## Phase 12 · Build & package

Run `scripts/rebuild_mega.sh <DECK_ROOT>`. This:

1. Snapshots `slides/` to `slides.bak.<YYYYMMDD-HHMM>/`
2. Inlines all images as base64
3. Concatenates all slides into a single self-contained `mega.html`
4. Reports the final path and size

The single-file mega.html is what goes to delivery — recipients can double-click to view, no server, no asset folder.

---

## Phase 13 · Delivery

Hand the finished artifact back to the user directly:

- Report the absolute path of the single-file `mega.html` (double-click to view — no server, no asset folder). Offer the zipped `HTML_Deck/` too if someone wants to inspect source.
- Give a one-line summary: deck title, version label, page count, and key changes since the last version.

(No external delivery channel is wired into pinvou3 — delivery is a local file handoff. If a project later needs an external channel (drive upload, email, chat push), add it as a separate skill and invoke it here.)

---

## Phase 14 · Feedback loop · revision (Loop D)

The decision-maker (and possibly the customer in later rounds) reviews and sends feedback. Each round of feedback is a small revision pass:

1. **Read every comment** the user gives in chat (and any review notes they paste/attach)
2. **Categorize** — bug / content / visual / out-of-scope
3. **Fix in place** — most fixes are small, do them inline rather than batching
4. **Re-trigger phase 10 audit** for any structural change
5. **Re-build** (phase 12)
6. **Re-deliver** (phase 13) with a clear "since last version: X / Y / Z" note

**Loop D — when to stop iterating.**

The signal is a clear "OK to send" / "OK 可以发了" from the decision-maker, often after the customer-facing handoff has happened. Stop the loop when:

- The decision-maker explicitly approves
- The customer has been shown the deck and given direct feedback (positive or "ship it")
- Or you hit the deadline — at which point ship the latest stable version with a one-line note about known unfixed issues

Don't let this loop run forever. If you're on round 5+ and still getting feedback, the issue is usually scope drift or unstated audience changes — escalate the conversation to phase 0 questions, not more revisions.

---

## Cross-cutting practices

- **Backup before any batch change.** `cp -r slides slides.bak.<YYYYMMDD-HHMM>/` before any sed-style mass edit. The `rebuild_mega.sh` script does this automatically; for any other batch operation, do it by hand.
- **Two visual-version backup.** It's worth maintaining a `HTML_Deck_v1_<color>` snapshot at major milestones in case the visual direction needs an emergency revert (a real project has had this save it).
- **De-identify intermediate outputs before sharing.** Audit reports can leak project-specific scenarios — when sharing the workflow itself, redact.
- **Use the project's `CLAUDE.md` as the contract.** Every phase 11 finding can be traced back to a specific section in CLAUDE.md. If a finding has no anchor in CLAUDE.md, either add it there or accept the finding doesn't apply.
- **Loop budgets.** Loops A/B/C/D consume real time. A reasonable per-loop budget on a 90-page deck has been: A=2h, B=90min, C=30min × 3, D=2h × N rounds. Knowing this in advance prevents "let's just iterate more" from eating the deadline.

---

## When this skill should hand off to another skill

- Audit a deck or rebuild mega.html → use this skill's own bundled `scripts/` (no separate skill needed)
- Generate product-in-scene images → invoke `product-scene-gen` (if installed)

This skill is the playbook layer; those skills are the tool layer.

---

## A short version (for memory)

If you only remember four things:

1. **Pick the lane first.** Light (`p0→p4→p6→p7→p10→p12`) for internal/quick/≤30-page decks — the pinvou3 default; Full (all 16) only for external large productions. Don't run the heavy lane on a small internal deck.
2. **In p7, one page = one small file.** Dumping the whole deck into one `write_file` stalls the stream; `append_file` is tail-append only, so use `edit_file` / `apply_patch` for placeholders.
3. **(Full lane) Phase 2 research is non-negotiable.** A deck without case citations dies in customer review.
4. **(Full lane) Loop D / 8.5 are multi-round by design** — ship when the decision-maker says "OK," not when "perfect."

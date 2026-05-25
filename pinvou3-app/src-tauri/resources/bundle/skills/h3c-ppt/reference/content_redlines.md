# Content Guardrails · 8 rules for H3C corporate decks

These are the qualitative content rules that scripts can't reliably check. Walk through them page by page (or sample-page by sample-page on a large deck). For each page, run the checklist in your head and stop on anything that flags.

---

## 1. Plain language

**The rule.** A deck written for an external corporate audience should be readable by someone who is technically literate but not in your sub-field. Technical acronyms, framework names, and jargon should be replaced with the plain-language thing they actually mean to the reader.

**Why.** The audience often includes a customer's executive sponsor, a procurement lead, a board observer, a journalist, or a frontline salesperson who has to re-present the slide to their own boss the next day. None of these people can decode jargon, and any of them being lost makes the slide useless.

**Concrete signals to look for.**

- Methodology acronyms (sales playbook codes, framework abbreviations, internal product codenames before public launch) used without expansion
- Anglicized verbs ("opt-in / RAG / agent pool / orchestrate") in a Chinese deck where the rest of the page is Chinese
- Vague abstractions ("赋能 / 闭环 / 中台 / 双轮驱动 / 数智化") used as load-bearing nouns rather than as flavor
- Compliance / cert terminology cited without the underlying meaning (e.g. mentioning a security cert name without saying what it lets the customer do)

**The fix.** Translate each flagged term into a sentence a non-specialist could understand. If the translation is awkward, the term probably doesn't earn its place on the slide.

---

## 2. Role discipline

**The rule.** The deck should be honest about who owns what. Don't claim outcomes that depend on a partner's execution, the customer's execution, or third parties outside the speaker's control.

**Why.** Overclaiming corrodes trust in everything else on the slide. The audience often knows the supply chain better than the deck author and will spot a misattributed claim immediately. Also, in any post-mortem, "who promised X" is the first question.

**Concrete signals.**

- "We will deliver [foot-traffic / revenue uplift / market share]" when the outcome depends on the customer's operations
- "We will handle [tendering / regulatory filing / financial settlement]" when those steps belong to a different organization
- Claiming credit for a partner's product as if it were ours
- Treating an aspirational MOU activity as if it were a contracted deliverable

**The fix.** Use scoped language: "We provide X — the customer / partner Y drives the outcome." If the desired outcome genuinely depends on us, say what specifically we commit to, with a measurable boundary.

---

## 3. Humble voice (strategic partner positioning)

**The rule.** In a deck where we are positioned as a strategic partner — not the lead — language should reflect that. We support, we enable, we help; we don't dictate, we don't promise outcomes that exceed our actual commitment scope.

**Why.** A customer who sees a partner deck using "我们做 / 我们承诺 / 我们一定" tends to feel one of two things: (a) the partner is overreaching, or (b) the partner thinks they're the lead. Either reading damages the relationship.

**Concrete replacement table.**

| Avoid | Prefer |
|---|---|
| 我们做 / 我们承担 / 我们主导 | X 可承载 / 我们愿提供 / 在 Y 主导下,我们... |
| 痛点 / 命门(speaker's own judgment about customer) | 政策关切 / 客户关心(framed as the customer's view) |
| 直接缓解 / 直接解决 | 有助于呼应 / 有助于缓解 |
| 政绩 | 民生回响 / 政策呼应 / 对应方向 |
| 承诺 / 保证 / 一定 | 初心 / 愿景 / 愿尽全力 / 有信心 |
| 助推冲击(strong agency) | 服务于...战略目标 / 配合...目标 |
| 我们的方案 / 我们的项目 | 在 [客户] 主导的方案中,我们承载的部分 |
| 必须 / 应当 / 一定要 | 建议 / 可以考虑 / 我们体会 |

**The narrative-order rule.** Within a page, the speaking order should be:

1. The customer's situation / concern (not our capability)
2. The customer's stated direction / strategy (our reading of it)
3. How our technology serves that direction
4. Decision returns to the customer

Decks that open with our product and end with "and so the customer should buy this" read as sales-led; the reverse order reads as advisory.

---

## 4. Case-driven, not aspirational

**The rule.** Every page that asserts an impact ("this saves N%", "this lifts conversion", "this avoids problem Z") must cite a real reference: the project where the impact was measured, the year, and the source document. If no such reference exists, the page either becomes "exploratory direction" (marked clearly) or gets cut.

**Why.** Aspirational impact claims are the single most common reason a customer dismisses a partner pitch as "理论很好,落不落地不知道". A grounded reference — even from a different vertical — flips the same slide into "they've done it elsewhere, we can do it deeper here."

**Concrete checks per page.**

- Find every numeric claim ("提升 25% / 降本 50 万 / 服务 4500 万人")
  - Does it have a source attached? (footnote, parenthetical, page reference, file number)
  - Is the source verifiable? (a real public report, a project number, a regulation file, a press release)
- Find every qualitative outcome claim ("减少滞销 / 提高效率")
  - Is there a project name attached to the claim?
- For any claim without a source: either find a real reference, demote the page to "🟡 探索方向", or remove the claim.

**Closely related rule:** for any decision point on the slide (recommended action, risk warning), cite the regulation / file number / industry standard that the recommendation is anchored to. Decisions without anchors look like opinion.

---

## 5. Physical plausibility

**The rule.** A capability claim on the slide must stay inside the physical envelope of the product being shown. If the product is a fixed-location device, it can't sense events at a different location. If the product has no microphone, it can't accept voice input. If the product is at counter height, it can't detect a fall at ground level. Etc.

**Why.** Engineers in the audience will catch a physical impossibility instantly, and once trust breaks on one claim it breaks on the whole deck. The non-engineer audience won't catch it, which is worse — the deck makes it through review, gets quoted in the contract, and the gap surfaces only at acceptance testing.

**The diagnostic question.** For every "the device X does Y" sentence on the slide:

- Where is X physically located?
- Where does Y need to happen?
- What sensor / channel connects the two?
- Is the device's role "detect" or "receive a report and then act"?

The device is almost always in role 2 ("receive and act"), and the slide should say so explicitly. Phrasing like "device detects [event happening 30 meters away in a different building]" is the smell.

---

## 6. Compliance edges

**The rule.** Any capability that touches a regulated activity — identity verification (face / fingerprint / ID scan), money handling (payments / settlements / lending), personal data collection, regulated industry services (medical / financial / educational) — must cite the specific regulation, file number, and effective date. If the activity skirts the edge of what's legal, say so explicitly and propose a compliant alternative on the same slide.

**Why.** A reviewer's eye goes straight to the riskiest claim on the deck. If it's anchored to a file number and a clear scope, the reviewer reads it as "the author knows the rules." If it's an unsourced bold claim, the reviewer reads it as "the author doesn't know what they're proposing might be illegal" — which is a deal-breaker for serious customers.

**Per project this needs a redlines list specific to the industry / vertical.** Common slots a project's redlines list typically includes:

- Identity & biometrics regulations (especially face recognition rules)
- Data protection and personal information laws
- Sector-specific licensing (medical / financial / pharma / education / telecom)
- Critical infrastructure protection
- Public communication channels (emergency broadcast / official media)
- Anti-monopoly / unified-market regulations (especially for any "unified pricing / unified branding" claim across many storefronts)

Each project should maintain its own redlines list — this skill's job is to remind you to check, not to be the list itself.

**Phrasing fix.** Replace "we will [regulated thing]" with "we provide [supporting tech]; [customer's licensed entity] performs [regulated thing]; we cite [regulation X §Y] for compliance scope."

---

## 7. Data freshness

**The rule.** Model versions, benchmark numbers, product spec values, and any "the latest is …" claim must reflect the current state at the moment of delivery. A spec slide that still shows a deprecated model name, or a benchmark from an obsolete hardware generation, is worse than no slide — it makes the rest of the deck look stale.

**Why.** Tech audiences calibrate their trust by spot-checking one thing they know well. If that one thing is six months out of date, the rest of the deck is assumed to be the same.

**Quick check per page that has numbers or model names.**

- Is the named model the one currently being shipped / recommended?
- Is the benchmark number from the current generation of hardware?
- Are the throughput / latency / capacity numbers from a configuration that's actually being sold today?
- Are the example use cases for the product the ones the team is actually positioning today — not from an earlier positioning round?

**The "generic LLM demo" smell.** If a product page's example use cases all sound like generic chatbot demos (travel planning, stock recommendations, recipe suggestions, generic Q&A), the page hasn't been thought through. The use cases should be specific to the deployment context — drawn from the customer's actual data, the customer's actual workflow, and grounded in the customer's actual users. "I want to plan a trip to Greece" is never a load-bearing example on an enterprise deck.

---

## 8. Design fidelity

**The rule.** When the deck is based on a designer's design package (Figma / Stitch / mockups), the design is the anchor and the content has to flex to fit. Don't quietly modify the design's numeric parameters (card overlap, animation distance, rotation angle, opacity) to "make it look more normal." If you think a design parameter is wrong, raise it with the designer — don't silently override it.

**Why.** Quietly overriding the designer's choices is the single most common reason a "translate design into code" task takes 3× as long as expected. Each silent override gets caught in review, gets reverted, and the round-trip eats time. Worse, the silent overrides drift the implementation away from the design system, so subsequent pages built from the same template inherit the drift.

**The two-way version of this rule.**

- **Code → from design package:** every numeric value (px, deg, ms, alpha) in the design = same value in code. The only acceptable exceptions are when the designer has explicitly approved the change.
- **Content → from design package:** when given a new design, rewrite the deck's content to match the new design's voice and structure. Don't paste yesterday's copy under today's design — it always reads as a mismatch.

**Self-check before committing a design-based change.**

- Did I change any value the designer specified? If yes, was that explicitly approved?
- Does my content speak in the same register as the design? If the design is "tactical archive cyber-grid" and my copy is "warmly inviting," one of them is wrong.

---

## Using this checklist

The realistic workflow is:

1. After the script audits pass (Sections A / B / C / E in SKILL.md), open the deck in a viewer and walk through it page by page.
2. For each page, run rules 1 → 8 in your head. Most pages clear all eight in seconds.
3. For any page that flags, decide:
   - **Fix it now** (e.g. "this number has no source — drop it or find the source")
   - **Demote / re-scope it** (e.g. "this is aspirational, mark as 探索方向 not commitment")
   - **Delete the page** (no source, no role to play, can't be salvaged)

If the deck is large (> 60 pages), sample every page in the first 10, then sample every third page, then full-walk the last 10. Most content drift concentrates in the middle.

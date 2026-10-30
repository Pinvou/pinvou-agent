# Supplemental public-API checks discovered during V2.3 review

Author: Codex (OpenAI). Model: GPT-6 (exact variant/reasoning level unavailable).

Review found a shipping function renamed from `shipping_fee(items)` to
`shipping_fee(quantity_items)`. The frozen nested-root oracle checks positional
results, annotations and docstring, so it misses the broken `items=` keyword call.
The newer third-root oracle already includes a keyword call. This is a limitation
of the original oracle, not evidence that the renamed API is acceptable.

Keep raw and calibrated primary scores unchanged. The secondary instruction
review flags the incompatible rename. Add a separately labeled **post-freeze**
diagnostic using `scripts/prompt_eval_extra_api.py` uniformly across both models,
versions and repetitions for relevant code outputs. Do not make new model calls
or replace failed trials. Generated code still runs in the read-only, networkless
verifier; publish only boolean checks and safe trial identifiers.

Check original function parameter names, positional-or-keyword kinds and absence
of new defaults, allowing annotations/docstrings. Report signature preservation
separately from correct keyword-call behavior. An added default can preserve the
sample keyword call while changing the original signature. These finite checks
do not prove complete API equivalence, arbitrary object semantics or all caller
compatibility. They supplement, rather than replace, the existing functional and
aliasing diagnostics. Trusted positive/negative controls cover renaming,
positional-only parameters, changed defaults, incorrect formulas, early exits,
import failures and timeouts.

This diagnostic was designed after observing the failure and is exploratory.
Do not present it as preregistered holdout evidence or silently fold it into the
headline score. No production source, live runner, case fixture or frozen grader
is changed by this addition.

Signed: Codex (OpenAI) / GPT-6 (exact variant/reasoning level unavailable).

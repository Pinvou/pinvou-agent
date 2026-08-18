---
name: pinvou-asr-context
description: Inspect the bounded Qwen3-ASR vocabulary context, refresh time, term sources, and English term coverage. Use for speech recognition errors involving names, product names, acronyms, or technical vocabulary.
---

# PinvouOS ASR Context

Call `pinvou_asr_context_status`. The snapshot is compiled at startup and every 30 minutes, contains at most 100 terms, and is read synchronously by ASR without an LLM call.

It may use Runtime Agent/capability names, active Mission terms, verified world claims, recent extracted terms, and the local private lexicon. It must not store whole utterances or secrets. The legacy Memory projection is not a source. Only after the new Memory architecture exposes an approved stable read-only Context Projection may a future adapter contribute terms.

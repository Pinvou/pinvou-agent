---
name: pinvou-attention
description: Rank concurrent PinvouOS goals and allocate work and interruption budgets using priority, deadlines, user blocking, resource class, and interruptibility. Use when several Missions compete or resource pressure requires preemption.
---

# PinvouOS Attention

Call `pinvou_attention_plan`. Supply bounded goal facts; the tool takes current resource pressure and current time from Runtime rather than trusting caller values.

Use the returned rank, disposition, attention share, work budget, interrupt budget, and reason codes. `Warm`, `Hot`, and `Critical` progressively reduce concurrency and heavy work. An Atomic section may continue only to its declared safe boundary; it does not gain immunity from Critical stop. Attention proposes scheduling; the Runtime scheduler and execution adapters apply it.

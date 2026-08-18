---
name: pinvou-resource
description: Inspect CPU, memory, GPU, temperature, power, current pressure, and Governor evidence when work may need throttling, pausing, stopping, or resuming. Use for heat, slowness, overload, battery, or safe scheduling questions.
---

# PinvouOS Resource

Call `pinvou_runtime_status` and use only the returned Resource projection.

- `Normal` permits ordinary scheduling.
- `Warm` defers optional heavy work.
- `Hot` pauses or throttles interruptible heavy work.
- `Critical` requests hard stop through the Governor.

Never manufacture a missing sensor value or downgrade pressure from a stale or incomplete sample. Resource Agent submits facts; Governor owns control directives; the execution adapter must acknowledge whether a directive was actually applied. Report pressure, evidence time, missing sensors, and resulting action separately.

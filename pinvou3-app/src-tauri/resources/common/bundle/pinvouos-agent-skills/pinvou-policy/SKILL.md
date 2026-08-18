---
name: pinvou-policy
description: Check permissions, user boundaries, confirmations, safety invariants, risk, and side effects before a PinvouOS action. Use for writes, device changes, credentials, external communication, destructive operations, or any action needing consent.
---

# PinvouOS Policy

Policy is deterministic and fail-closed. A model may propose an action but may never supply its own grants, safety facts, or confirmation proof.

Call `pinvou_capability_report` for `policy.authorize`. Until the kernel Authority Store and evaluator adapter are connected, treat the capability as unavailable and return the missing requirement. Never simulate an allow decision in prose.

When connected, confirmations must bind the complete action digest, actor, target, parameters, effects, policy version, and expiry. A confirmation cannot override a hard safety invariant or missing permission. Secrets are references to Keyring entries, never prompt content or Memory.

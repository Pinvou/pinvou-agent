---
name: pinvou-capability
description: Explain what PinvouOS can do now, can do after a requirement is met, or does not support. Use before promising actions, before Orchestrator builds a work graph, and whenever Agent or device availability may have changed.
---

# PinvouOS Capability

Call `pinvou_capability_report` with the exact atomic capability IDs. Use `includeRegistered: true` only for discovery.

Preserve the three outcomes:

- `available`: at least one registered, running executor satisfies current resource constraints.
- `temporarily_unavailable`: the capability exists but a state, resource, provider, permission, or device requirement blocks it now.
- `unsupported`: no registered contract implements it.

Return candidate Agent IDs, requirements, reason codes, and evidence sequence. Never upgrade `Starting`, `Paused`, or missing-provider Agents to available from model intuition.

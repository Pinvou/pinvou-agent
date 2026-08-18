---
name: pinvou-inference
description: Inspect the active model route, provider, readiness, probe latency, last successful completion, and failure reason. Use when PinvouOS replies stop, the model appears disconnected, credentials may be wrong, or latency needs diagnosis.
---

# PinvouOS Inference

Call `pinvou_runtime_status` and inspect `inference` plus `connectivity`.

- Network online plus inference unavailable usually points to route, credential, quota, protocol, or model failure.
- Inference ready requires an authoritative health probe or successful completion, not merely a configured model name.
- Report model, provider, `checkedAtMs`, latest success time, latency, and reason code. Do not expose keys or tokens.
- A successful old completion is evidence of the past, not proof of current readiness. Prefer the freshest probe and state explicitly when it is stale.

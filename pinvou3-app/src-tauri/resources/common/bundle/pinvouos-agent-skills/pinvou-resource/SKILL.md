---
name: pinvou-resource
description: Inspect CPU, memory, GPU, temperature, power, current pressure, and Governor evidence when work may need throttling, pausing, stopping, or resuming. Use for heat, slowness, overload, battery, or safe scheduling questions.
---

# PinvouOS Resource

Call `pinvou_runtime_status` and use only the returned Resource projection.

- `Normal` permits ordinary scheduling in the Governor plan.
- `Warm` asks scheduling to defer optional heavy work.
- `Hot` proposes pausing or throttling registered interruptible work.
- `Critical` asks the Governor to evaluate a hard-stop candidate. It is not evidence that any work stopped.

Never manufacture a missing sensor value or downgrade pressure from a stale or incomplete sample. Resource Agent submits facts; Governor owns control directives; only a trusted, pre-registered Host adapter or Supervisor may execute one. The current production build has no Resource Control Adapter, so report control as unavailable unless the Runtime projection contains a matching adapter acknowledgement and later observed state.

Use only opaque work identifiers already returned by the Runtime projection. Never invent or request a PID, systemd unit, cgroup path, command line, shell command, or `systemctl` action. Do not claim that a desired state, issued directive, pressure badge, missing work, or lost response proves success. Report pressure, evidence time, missing sensors, desired action, acknowledgement, and observed result separately; use `outcome_unknown` when the execution result cannot be reconciled.

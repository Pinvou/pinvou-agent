---
name: pinvou-device
description: Check verified presence, operational state, and capabilities of keyboards, pointers, microphones, displays, cameras, storage, Bluetooth, and other devices before proposing an interaction or hardware-dependent task.
---

# PinvouOS Device

Call `pinvou_capability_report` for `device.inspect` and the exact required device capability. A `Starting` Device Agent means inventory/provider evidence is not yet connected; fail closed instead of guessing.

- Separate physical presence from operational readiness.
- A device name alone does not prove an input method is usable.
- Before proposing keyboard-controlled software, require an observed keyboard capability. If absent, choose touch, pointer, voice, or on-screen controls only when those are also verified.
- Provider errors and stale observations must remain visible in the result.
- Device Agent reports facts. Connectivity handles network diagnosis; Resource handles load and heat; Policy authorizes changes.

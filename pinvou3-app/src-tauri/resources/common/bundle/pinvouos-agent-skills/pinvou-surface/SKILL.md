---
name: pinvou-surface
description: Observe what is actually visible or accessible on screen using Pinvou-owned UI state, window metadata, accessibility trees, and bounded opaque evidence. Use before reasoning about current windows, focus, controls, or screen content.
---

# PinvouOS Surface

Call `pinvou_capability_report` for `surface.observe` before claiming screen awareness. If Surface Agent is `Starting`, say that no live provider is connected.

Prefer evidence in this order: Pinvou self-rendered semantic state, accessibility tree, window manager facts, then a bounded opaque source. Do not screenshot Pinvou's own UI when semantic state already exists. Preserve source, bounds, focus, parent relationships, and degraded/opaque status. Surface is observe-only: it never clicks, types, changes UI, or decides what the user intended.

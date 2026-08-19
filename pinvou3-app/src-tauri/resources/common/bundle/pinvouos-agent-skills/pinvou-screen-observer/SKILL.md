---
name: pinvou-screen-observer
description: Observe what is actually visible or accessible on screen using Pinvou-owned UI state, window metadata, accessibility trees, and bounded opaque evidence. Use before reasoning about current windows, focus, controls, or screen content.
---

# PinvouOS Screen Observer Agent

Call `pinvou_capability_report` for `screen.observe` before claiming screen awareness. If Screen Observer Agent is `Starting`, say that no live observation provider is connected.

Prefer evidence in this order: Pinvou self-rendered semantic state, accessibility tree, window manager facts, then a bounded opaque source. Do not screenshot Pinvou's own UI when semantic state already exists. Preserve source, bounds, focus, parent relationships, and degraded/opaque status. Screen observation is read-only: it never clicks, types, changes UI, or decides what the user intended.

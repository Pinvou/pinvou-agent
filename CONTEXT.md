# Pinvou Agent

Pinvou Agent is a desktop assistant organized around task conversations and work surfaces that open only when needed.

## Browser workspace

**Task conversation**:
An independent conversation in the task list. It is the ownership boundary for browser pages and tab state; collapsing a work surface or switching to another conversation does not destroy its state.
_Avoid_: execution step, subtask, global session

**Browser side panel**:
The browser workspace shown beside the current task conversation. It can be expanded or collapsed, but it does not replace the task conversation.
_Avoid_: browser tab, browser home page, global browser entry in the left sidebar

**Right work Dock**:
The single resizable work area beside a task conversation. It hosts the browser, artifacts, subagents, or a code workspace; switching content does not destroy the state of those surfaces.
_Avoid_: multiple right sidebars, nested splitters

**Single-pane work mode**:
The responsive state used when the window is too narrow to show both the task conversation and the right work Dock. Only one is shown at a time; returning to two panes restores the previous ratio instead of saving the temporary size as a preference.
_Avoid_: compressing the conversation into a narrow strip, overwriting the user's ratio with a temporary width

**Browser presentation intent**:
The foreground intent to show the browser in the right work Dock. The first visible operation in the current task conversation may trigger it automatically; background activity must not, and an explicit user selection has priority.
_Avoid_: opening on every tool call, letting a background conversation steal focus

**Browser session**:
The set of browser pages, tabs, and navigation state owned by one task conversation. Collapsing the side panel or switching conversations preserves the state so it can be shown again later.
_Avoid_: global browser, application-wide tabs

**Browser session restoration**:
After an application restart, restore the minimal URL list for each task conversation with new WebViews, tab identities, automation targets, and leases while preserving tab order and the active page. The persistent browser identity is retained, but no process-lifetime identity is reused. Restored ownership starts neutral and unclaimed; the next real user or Agent action claims it atomically. A restart itself is neither user takeover nor silent Agent authorization. Temporary in-page memory is outside the restoration guarantee.
_Avoid_: full process snapshot, complete in-page memory restoration

**Browser identity**:
The user browsing identity shared by the application, including website sign-in state. It does not determine which pages belong to a task.
_Avoid_: task account, session profile

**Default browser identity**:
The persistent browsing identity initially provided by the application and shared by all task conversations. It can be reset, but deleting any one task conversation does not delete it.
_Avoid_: default task, temporary session identity

**Native browser surface**:
The real browser surface that the user can view and operate directly. The Agent and user control the same page; a continuous screenshot stream is neither the page-display path nor a failure fallback.
_Avoid_: screenshot browsing mode, video-stream browser

**Visible-page operation**:
An Agent observation or interaction with a browser page. The target must first become the current tab of its browser session. A background task conversation may continue operating its own current page without taking over the conversation or page the user is viewing; the user can observe and take over immediately on return.
_Avoid_: operating a background page, interacting with a hidden tab

**Background browser activity**:
Agent activity in the browser session of a task conversation other than the one currently shown. It remains isolated to that conversation and is surfaced as status only; it does not change the user's current conversation or Dock selection.
_Avoid_: cross-conversation takeover, global browser operation

**Background tab precreation**:
Create and load a tab without making it the current page yet. Apart from preloading, the Agent must activate that tab before observing or interacting with it.
_Avoid_: background-tab operation, hidden-page interaction

**Browser control ownership**:
The exclusive state that determines whether the user or Agent may operate a browser session. Direct page interaction gives the user a short-lived lease; each real interaction renews it. Control returns to the Agent after three seconds of user inactivity, or immediately when the user explicitly hands it back.
_Avoid_: concurrent control, delayed auto-release without a revision guard

**User takeover**:
The user directly operates a page, receives short-lived control of that browser session, and pauses subsequent Agent page operations. Takeover affects only the corresponding task conversation and is released after three seconds of inactivity through revision-guarded compare-and-swap. A browser tool that has already passed its lease check and begun dispatch completes atomically; takeover does not promise to interrupt an operation already committed to the platform backend, but it blocks the next tool. Ending an operation immediately revokes its active-operation state. Only calls that actually dispatched native input retain a post-dispatch callback grace period of at most 100 ms so a slightly delayed WebView callback can be attributed correctly. Explicit UI takeover still takes effect immediately and clears that window; 750 ms is only a fail-safe upper bound that prevents an abnormal dispatch exit from suppressing takeover forever.

After dispatch begins, MCP cancellation is cooperative. The wrapper must wait for the platform backend to return, or stop its child process and prove that no further input can be dispatched, before calling `end_agent_operation`. If the terminal state cannot prove whether the page action occurred, return a non-retryable commit-unknown result. An external caller that has already cancelled may discard the result, but it must not release the host operation window early.

A popup caused by an already-started dispatch may retain Agent ownership only by carrying the complete in-memory Rust lease through hidden staging and the final compare-and-swap. A popup without valid dispatch authorization becomes user-owned. If user takeover wins before the final compare-and-swap, the late popup must be rejected safely.
_Avoid_: treating page focus as ownership, concurrent user and Agent input

**Protected browser capability request**:
A browser side-panel flow that presents a request and waits for the user before a page or Agent may use files, media devices, location, notifications, or another protected capability. This PR does not implement a general approval broker. Embedded downloads are denied by default and instruct the user to open the page in the system browser. No file-upload tool is exposed, and direct calls or workarounds are forbidden. Other capabilities retain the engine's default deny-or-prompt behavior and must never be granted silently.
_Avoid_: silent permission grant, model-approved permission

**Semantic high-risk approval**:
User confirmation triggered by the business meaning of an action, such as payment, deletion, or final submission. This belongs to the general Agent approval system, not to browser platform permissions.
_Avoid_: protected browser capability request, protocol interception

**Core browser capability**:
The browsing, observation, and interaction functionality required for ordinary web tasks and provided consistently across supported desktop platforms.
_Avoid_: platform-specific tool, complete developer-tools surface

**Native page input**:
After resolving a target from page semantics, the Agent uses the browser surface's native input channel for clicks, text, keys, or drag operations so the page receives the same semantics as direct user interaction.
_Avoid_: synthetic JavaScript click, global system control

**Compatibility interaction fallback**:
A constrained interaction path used when native page input is unavailable. Its capability boundary must be explicit, and it does not count as satisfying the core browser capability.
_Avoid_: silent downgrade, claiming core interaction is complete

**Advanced browser capability**:
Diagnostics, performance analysis, or another extension provided by a particular browser engine. Platforms may declare these capabilities explicitly, but they must not change the behavior of the core browser contract.
_Avoid_: cross-platform baseline capability, default required tool

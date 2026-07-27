# Design Mode Handoff

Last updated: 2026-07-27
Branch/worktree: `work/design-mode-ui-shell-20260724`
Status: paused for handoff
PR status: Draft only. Do not merge until the missing business integrations are complete.
Draft PR: https://github.com/Pinvou/pinvou-agent/pull/16

## Handoff Summary

This branch is an interaction/UI shell for the new composer modes. It is intended for product/design review and for the next engineer to continue wiring behavior.

Do not treat this branch as feature-complete:

- Code Agent provider buttons are UI state only.
- Work/design secondary tabs are category selectors only.
- Content generation and Code Agent dispatch are not implemented.

## Current State

- The composer has three primary modes: `工作`, `设计`, `代码`.
- Primary mode switching uses the shared compact `IosSegmentedControl` with the `prominent` size and a sliding iOS-style selection plate.
- Secondary tabs are lightweight inline tabs with small icons and a sliding underline.
- Work secondary tabs: `公文写作`, `PPT设计`, `数据可视化`.
- Design secondary tabs: `海报`, `网页`, `Banner`, `Logo`, `UI界面`.
- Code Agent secondary tabs: `Codex`, `Claude Code`, `Kimi Code`.
- Code Agent brand assets are local:
  - `src/brand-icons/claude.svg`
  - `src/brand-icons/kimi-official.svg`
  - Codex uses an inline OpenAI SVG component so it remains visible in light and dark modes.
- Composer dropdown panels for model/tools/knowledge were made opaque to avoid tab UI bleeding through in dark mode.
- The model/tools/knowledge composer pills were visually reduced to better match the iOS-style primary tabs.

## Key Files

- `src/features/chat/ChatView.jsx`: composer mode UI, secondary tabs, Code Agent provider picker, design mini panel wiring.
- `src/features/chat/pinvou-mode-state.js`: persisted mode/provider state reducer and storage helpers.
- `src/features/chat/design-changes.js`: design-change normalization, reduction, and dedupe logic.
- `src/features/artifacts/ArtifactsPanel.jsx`: artifact preview integration point for design runtime.
- `src/features/artifacts/design-runtime.js`: iframe visual editing runtime injection for selectable/editable HTML preview elements.
- `src/components/IosControls.jsx`: iOS-style segmented control with `prominent` compact mode.
- `src/features/settings/SettingsView.jsx`: composer model/tool pill visual alignment and opaque popovers.
- `src/brand-icons/claude.svg` and `src/brand-icons/kimi-official.svg`: local brand assets for code provider choices.
- `tests/design_mode_entry_smoke.js`: end-to-end UI smoke for work/design/code entry behavior.

## Removed UI

- Removed the design-mode blue status banner.
- Removed the empty preview-selection prompt panel.
- Removed the earlier heavy outer wrapper around primary and secondary tabs.
- Removed the labels `选择代码 Agent` and `占位配置` from the composer area.

## Not Implemented Yet

- Code Agent execution is not wired to `Codex`, `Claude Code`, or `Kimi Code`.
- Work/design secondary tab business behavior is not wired.
- Content generation flows are not implemented for `公文写作`, `PPT设计`, `数据可视化`, `海报`, `网页`, `Banner`, `Logo`, or `UI界面`.
- No backend dispatch contract for Code Agent execution is finalized in this branch.

## Verification

Last full verification was run from `pinvou3-app` in the correct `Pinvou/pinvou-agent` worktree:

```powershell
npm run lint:ui
npm run test:pinvou-mode-state
npm run test:design-runtime
npm run test:design-changes
npm run build:ui
npm run test:design-mode-entry-smoke
```

All commands above passed on 2026-07-24 after the branch was migrated onto `Pinvou/pinvou-agent/main`.

The Tauri dev app was running during iteration at `http://127.0.0.1:1420`.

## PR / Repository Notes

- Correct target repository: `https://github.com/Pinvou/pinvou-agent`.
- Correct Draft PR: https://github.com/Pinvou/pinvou-agent/pull/16.
- Correct PR branch: `work/design-mode-ui-shell-20260724`.
- Local commits:
  - `e95fc84c Add design mode composer UI`
  - `565eee71 Update design mode handoff notes`
  - `4149bad7 Clarify draft PR handoff status`
  - `da962082 Update handoff for agent PR branch`
- A draft PR was accidentally opened against the old repository: `https://github.com/Pinvou/pinvou3/pull/250`.
- Do not continue or merge the old `Pinvou/pinvou3` PR.
- The Draft PR description already says this branch is UI/interaction shell only and must not be merged before content/code execution integration is complete.
- Untracked local dev file intentionally left out of commit: `src-tauri/config/dev-port-1421.conf.json`.

## Resume Checklist

1. Pull the PR branch:

```powershell
git fetch agent work/design-mode-ui-shell-20260724
git switch work/design-mode-ui-shell-20260724
```

2. Re-run focused checks before changing behavior:

```powershell
cd pinvou3-app
npm run lint:ui
npm run test:pinvou-mode-state
npm run test:design-runtime
npm run test:design-changes
npm run build:ui
npm run test:design-mode-entry-smoke
```

3. Keep the PR as Draft until Code Agent execution and content-generation flows are implemented.
4. Update this file when backend contracts or ownership decisions are made.

## Next Implementation Steps

- Define the backend/frontend dispatch contract for Code Agent execution.
- Wire `Codex`, `Claude Code`, and `Kimi Code` provider selection to real execution paths.
- Decide what each work/design secondary tab should do when selected or submitted.
- Implement content-generation flows for `公文写作`, `PPT设计`, `数据可视化`, `海报`, `网页`, `Banner`, `Logo`, and `UI界面`.
- Recheck narrow-width layout if more secondary tab labels are added; the row currently uses horizontal overflow.
- Keep code provider icons as real brand assets. Do not replace them with generic or approximate icons.

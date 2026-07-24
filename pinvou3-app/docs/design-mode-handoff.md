# Design Mode Handoff

Date: 2026-07-24
Branch/worktree: `work/design-mode-20260724`
Status: paused until next week

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

Last run from `pinvou3-app`:

```powershell
npm run lint:ui
npm run test:pinvou-mode-state
npm run test:design-runtime
npm run test:design-changes
npm run build:ui
npm run test:design-mode-entry-smoke
```

All commands above passed on 2026-07-24.

The Tauri dev app was running during iteration at `http://127.0.0.1:1420`.

## PR / Repository Notes

- Correct target repository: `https://github.com/Pinvou/pinvou-agent`.
- Local commit: `8e6048f7 Add design mode composer UI`.
- A draft PR was accidentally opened against the old repository: `https://github.com/Pinvou/pinvou3/pull/250`.
- Do not continue that PR if the target repository has changed.
- When work resumes next week, push `work/design-mode-20260724` to `Pinvou/pinvou-agent` and open a new Draft PR there.
- The Draft PR description should explicitly say this branch is UI/interaction shell only and leaves content/code execution integration for follow-up.
- Untracked local dev file intentionally left out of commit: `src-tauri/config/dev-port-1421.conf.json`.

## Follow-Up Notes

- The current secondary tabs are visual-only selectors for work/design categories; wire behavior when the product flow is defined.
- If the tab labels grow, check the composer on narrow widths because the secondary row uses horizontal overflow.
- Keep code provider icons as real brand assets. Do not replace them with generic or approximate icons.

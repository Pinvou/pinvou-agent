# Design Mode Handoff

Date: 2026-07-24
Branch/worktree: `work/design-mode-20260724`

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

## Removed UI

- Removed the design-mode blue status banner.
- Removed the empty preview-selection prompt panel.
- Removed the earlier heavy outer wrapper around primary and secondary tabs.
- Removed the labels `选择代码 Agent` and `占位配置` from the composer area.

## Verification

Run from `pinvou3-app`:

```powershell
npm run lint:ui
npm run test:pinvou-mode-state
npm run test:design-runtime
npm run test:design-changes
npm run build:ui
npm run test:design-mode-entry-smoke
```

The Tauri dev app was running during iteration at `http://127.0.0.1:1420`.

## Follow-Up Notes

- The current secondary tabs are visual-only selectors for work/design categories; wire behavior when the product flow is defined.
- If the tab labels grow, check the composer on narrow widths because the secondary row uses horizontal overflow.
- Keep code provider icons as real brand assets. Do not replace them with generic or approximate icons.

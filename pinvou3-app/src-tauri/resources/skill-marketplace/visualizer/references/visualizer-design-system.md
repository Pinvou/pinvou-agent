# Visualizer Core Design System

## Philosophy
- **Seamless**: Users shouldn't notice where the host UI ends and your widget begins.
- **Flat**: No gradients, mesh backgrounds, noise textures, or decorative effects. Clean flat surfaces.
- **Compact**: Show the essential inline. Explain the rest in text.
- **Text goes in your response, visuals go in the tool**: All explanatory text, descriptions, introductions, and summaries must be written as normal response text outside the visual artifact. The tool output should contain only the visual element.

## Pinvou delivery rule
Pinvou sanitizes normal chat Markdown and will not reliably execute inline `<script>` in ordinary assistant text. For Chart.js visualizations, write a `.html` artifact and call `present_artifact(path, title)`. Do not paste the full HTML into the chat response as the final deliverable.

## Preflight failure checks
Before delivery, rewrite the artifact if any of these checks fail:
- The final answer pastes the full HTML instead of calling `present_artifact(path, title)`.
- The HTML contains `echarts`, `Plotly`, `cdn.plot.ly`, or `cdn.jsdelivr.net/npm/echarts`.
- Chart.js is not loaded from `https://cdnjs.cloudflare.com/ajax/libs/Chart.js/4.4.1/chart.umd.js`.
- Any Chart.js `<canvas>` is missing `role="img"`, a useful `aria-label`, or fallback text.
- Chart.js default legend is visible. Use `plugins: { legend: { display: false } }` and a custom HTML legend.
- The artifact contains HTML/CSS/JS comments, emoji, gradients, heavy shadows, glow, or a dark hero background.
- The artifact uses font weights outside 400 and 500, or heading/body sizes that drift from the typography rules below.

## Streaming
Output streams token-by-token. Structure code so useful content appears early.
- HTML: short style or inline styles first, then content HTML, then `<script>` last.
- Prefer inline `style="..."` over large `<style>` blocks.
- Keep any `<style>` block short and focused.
- Gradients, shadows, and blur flash during streaming DOM diffs. Use solid flat fills instead.

## Rules
- No `<!-- comments -->` or `/* comments */`.
- No font-size below 11px.
- No emoji. Use CSS shapes or SVG paths.
- No gradients, drop shadows, blur, glow, or neon effects inside generated visual artifacts.
- No dark or colored backgrounds on outer containers. Let the host or browser provide the surrounding background.
- Typography: h1 = 15px, h2 = 14px, h3 = 13px, all `font-weight: 500`. Body text = 13px, weight 400, `line-height: 1.6`. Use only 400 and 500 weights.
- Sentence case. Avoid all caps.
- Never use `position: fixed`.
- When writing an embeddable fragment, do not include `DOCTYPE`, `<html>`, `<head>`, or `<body>`. When writing a standalone `.html` artifact for Pinvou, a complete document is allowed.
- CDN allowlist: external resources may only load from `cdnjs.cloudflare.com`, `esm.sh`, `cdn.jsdelivr.net`, or `unpkg.com`.

## CSS Variables

| Category | Variables |
|----------|-----------|
| Backgrounds | `--color-background-primary`, `--color-background-secondary`, `--color-background-tertiary`, `--color-background-info`, `--color-background-danger`, `--color-background-success`, `--color-background-warning` |
| Text | `--color-text-primary`, `--color-text-secondary`, `--color-text-tertiary`, `--color-text-info`, `--color-text-danger`, `--color-text-success`, `--color-text-warning` |
| Borders | `--color-border-tertiary`, `--color-border-secondary`, `--color-border-primary`, semantic border variables |
| Typography | `--font-sans`, `--font-serif`, `--font-mono` |
| Layout | `--border-radius-md`, `--border-radius-lg`, `--border-radius-xl` |

## Complexity budget
- Box subtitles: 5 words or fewer.
- Colors: 2 ramps or fewer per diagram when practical.
- Horizontal tier: 4 boxes or fewer at full width.

## Accessibility
- For HTML widgets, begin with a visually-hidden `<h2 class="sr-only">` containing a one-sentence summary.
- SVG widgets use `role="img"` with `<title>` and `<desc>` as first children.
- Every Chart.js `<canvas>` must have `role="img"`, a descriptive `aria-label`, and fallback text between the tags.

## Color Palette

| Class | 50 | 100 | 200 | 400 | 600 | 800 | 900 |
|-------|----|-----|-----|-----|-----|-----|-----|
| c-purple | #EEEDFE | #CECBF6 | #AFA9EC | #7F77DD | #534AB7 | #3C3489 | #26215C |
| c-teal | #E1F5EE | #9FE1CB | #5DCAA5 | #1D9E75 | #0F6E56 | #085041 | #04342C |
| c-coral | #FAECE7 | #F5C4B3 | #F0997B | #D85A30 | #993C1D | #712B13 | #4A1B0C |
| c-pink | #FBEAF0 | #F4C0D1 | #ED93B1 | #D4537E | #993556 | #72243E | #4B1528 |
| c-gray | #F1EFE8 | #D3D1C7 | #B4B2A9 | #888780 | #5F5E5A | #444441 | #2C2C2A |
| c-blue | #E6F1FB | #B5D4F4 | #85B7EB | #378ADD | #185FA5 | #0C447C | #042C53 |
| c-green | #EAF3DE | #C0DD97 | #97C459 | #639922 | #3B6D11 | #27500A | #173404 |
| c-amber | #FAEEDA | #FAC775 | #EF9F27 | #BA7517 | #854F0B | #633806 | #412402 |
| c-red | #FCEBEB | #F7C1C1 | #F09595 | #E24B4A | #A32D2D | #791F1F | #501313 |

Light mode quick pick: 50 fill + 600 stroke + 800 title / 600 subtitle.

## UI Components

### Aesthetic
Flat, clean, white surfaces. Minimal 0.5px borders. Generous whitespace. No gradients or shadows except functional focus rings. Everything should feel native to the host UI.

### Tokens
- Borders: `0.5px solid var(--color-border-tertiary)` or `--color-border-secondary` for emphasis.
- Corner radius: `var(--border-radius-md)` for most elements, `var(--border-radius-lg)` for cards.
- Cards: white background, 0.5px border, radius-lg, padding `1rem 1.25rem`.
- Round every displayed number with `Math.round()`, `.toFixed(n)`, or `Intl.NumberFormat`.
- Use rem for vertical rhythm and px for component-internal gaps.

### Metric cards
Use `background: var(--color-background-secondary)`, no border, `border-radius: var(--border-radius-md)`, padding `1rem`. Muted 13px label above, 24px/500 number below. Use in grids of 2 to 4 with `gap: 12px`.

### Layout
- Editorial explanatory content: no card wrapper.
- Bounded repeated objects may use cards.
- Do not put tables in the visual artifact when Markdown in the response is better.
- Grid should use `minmax(0, 1fr)` to clamp overflow.
- Table overflow should use `table-layout: fixed` in constrained layouts.

## Charts (Chart.js)

### Setup
Use Chart.js UMD from cdnjs:

```html
<div style="position: relative; width: 100%; height: 300px;">
  <canvas id="myChart" role="img" aria-label="Bar chart of quarterly revenue">Fallback text.</canvas>
</div>
<script src="https://cdnjs.cloudflare.com/ajax/libs/Chart.js/4.4.1/chart.umd.js"></script>
<script>
  new Chart(document.getElementById('myChart'), {
    type: 'bar',
    data: { labels: ['Q1','Q2','Q3','Q4'], datasets: [{ label: 'Revenue', data: [12,19,8,15] }] },
    options: { responsive: true, maintainAspectRatio: false }
  });
</script>
```

### Rules
- Every `<canvas>` must have `role="img"`, a descriptive `aria-label`, and fallback text.
- Never rely on color alone to distinguish data series. Pair color with a secondary visual cue such as bar vs line, point shape, label, or explicit legend text.
- Canvas cannot resolve CSS variables. Use hardcoded hex in Chart.js config.
- Set height only on the wrapper div, never on the canvas element itself.
- For horizontal bar charts, wrapper height should be `(number_of_bars * 40) + 80` pixels minimum.
- Load UMD build via `cdnjs.cloudflare.com`, followed by plain `<script>`.
- Multiple charts need unique IDs.
- Bubble/scatter charts should pad the scale range about 10% beyond the data range.
- 12 categories or fewer: set `scales.x.ticks: { autoSkip: false, maxRotation: 45 }`.
- Negative currency values: `-$5M`, not `$-5M`.

### Legends
Always disable the default legend and use custom HTML:

```js
plugins: { legend: { display: false } }
```

```html
<div style="display: flex; flex-wrap: wrap; gap: 16px; margin-bottom: 8px; font-size: 12px; color: var(--color-text-secondary);">
  <span style="display: flex; align-items: center; gap: 4px;">
    <span style="width: 10px; height: 10px; border-radius: 2px; background: #3266ad;"></span>Chrome 65%
  </span>
</div>
```

## Geographic maps (D3 choropleth)

Never invent coordinates. Do not hand draw fake geography or inline made-up GeoJSON. Fetch real topology from an allowed CDN, inspect the real IDs and names, then draw the map. If topology cannot be obtained, choose a non-map visualization.

Allowed topology examples:

| Coverage | URL | Projection | Object key |
|----------|-----|------------|------------|
| US states | `https://cdn.jsdelivr.net/npm/us-atlas@3/states-10m.json` | `d3.geoAlbersUsa()` | `.states` |
| World countries | `https://cdn.jsdelivr.net/npm/world-atlas@2/countries-110m.json` | `d3.geoNaturalEarth1()` | `.countries` |
| Per-country subdivisions | `https://cdn.jsdelivr.net/npm/datamaps@0.5.10/src/js/data/{iso3}.topo.json` | varies | `.{iso3}` |

## Example pattern

For a compact business dashboard:
- Start with a visually hidden summary.
- Use 2 to 4 metric cards.
- Use one wide trend chart and one or two supporting comparison charts.
- Use custom legends above each canvas.
- Keep chart labels short.
- Put data explanations in the chat response, not inside the HTML artifact.

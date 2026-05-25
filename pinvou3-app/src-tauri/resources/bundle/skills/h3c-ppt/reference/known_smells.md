# Known Smells · 历史踩过的坑(脱敏)

When an audit flags something and the obvious fix doesn't work, check here first. These are problems that have actually broken decks in past projects, with the recipe that ended up working.

---

## S1. The "1 / 5" frozen page indicator

**Symptom.** The deck's bottom-center page indicator (rendered by the host shell, not by the slide itself) shows "1 / 5" or some other static value that never updates as you navigate between slides.

**Root cause.** The host shell's boot script reads the URL hash to decide which slide to load — `const m = location.hash.match(/p(\d+)/); if (m) go(parseInt(m[1]) - 1);`. When there's no `#pN` hash (which is the default — user just opened the file), `go()` is never called. The page indicator stays at whatever placeholder is hardcoded in the HTML.

**Fix.** Make `go()` always run on init:

```js
const m = location.hash.match(/p(\d+)/);
go(m ? parseInt(m[1]) - 1 : 0);   // always call go(); default to slide 0
```

The audit_structure.py check `INDEX INIT` catches this.

---

## S2. Hidden sub-18px text via decimal values

**Symptom.** A grep for `font-size:\s*\d+px` says all the font sizes are ≥ 18 px, but on the rendered page some text is visibly tiny. The audit's "minimum font size" check passes anyway.

**Root cause.** Someone wrote `font-size: 13.5px` (or 14.5, 15.5 — the half-integer ones are common). Integer-only regexes skip these.

**Fix.** audit_visual.py's `audit_decimal_px` check looks specifically for `\d+\.\d+px`. The remediation is to round each occurrence up to the nearest tier (typically 18 px). If the layout was visually relying on the smaller text, the design needs to change — not the font size.

---

## S3. Chapter numbers that jump on scene pages

**Symptom.** Walking through the deck in playback order, the chapter number in the top-left moves like `CH.03 → CH.05 → CH.03 → CH.05 …`. The user reports it as "CH 编号一会儿 3 一会儿 5,不知道啥意思".

**Root cause.** A "master page → scene page" pattern. The master pages were chaptered correctly. The scene pages (typically named `*-scene-*.html` or `*scene*.html`) were copy-pasted from a different section template and inherited the wrong chapter number. The author never noticed because each scene page in isolation looks fine.

**Fix.** Each `*scene*.html` page must carry the same `CH.NN` as the master immediately before it in the SLIDES order. audit_structure.py's `CH MISMATCH` check enumerates the violations in one pass; the fix is a one-line `CH.05` → `CH.03` per offending file.

There's a related variant — a single page in the wrong chapter altogether (e.g. `CH.13` next to a sequence of `CH.04` pages). Those have to be repaired manually after looking at the page's actual content to decide which chapter it belongs to.

---

## S4. Page-number divisor lagging the rebrand

**Symptom.** Most pages show "73 / 113" but a few stragglers show "02 / 92" or "47 / 86". The denominator is wrong on some pages.

**Root cause.** Pages got added or removed during a deck restructure, and most pages had their denominator updated, but a handful — usually section anchors or special-layout pages — got missed because they use a different page-number markup than the rest of the deck.

**Fix.** audit_structure.py's `PAGENUM MISMATCH` lists each page where X/Y doesn't match the SLIDES position / SLIDES length. Repair them one by one.

---

## S5. Bottom-right page number colliding with footer text

**Symptom.** The bottom-right page indicator is overlapping or adjacent to the footer's right-side note text ("technology partners to be selected with X" type of disclaimers). Sometimes the page number is unreadable because text is laying on top of it.

**Root cause.** Page-number widget is positioned at `right: 56px; bottom: 36px`. The footer uses `right: 96px` and stretches the full width with `display: flex; justify-content: space-between`, so the footer's right-side text reaches `right: 96px` — well inside the page-number's claimed area.

**Fix.** Add a global CSS override in `assets/base.css`:

```css
.slide .page-footer {
  right: 240px !important;   /* clear the 56-220 page-number zone */
}
```

And give the page-number widget a translucent backdrop so even if something else encroaches, the number stays readable:

```css
.slide-pagenum, .page-pagenum {
  bottom: 28px !important;
  right: 48px !important;
  padding: 6px 14px;
  background: rgba(0, 11, 26, 0.55);
  border-radius: 12px;
  backdrop-filter: blur(6px);
  z-index: 95;
}
```

audit_structure.py's `FOOTER-RIGHT` check looks for either the global override or sufficient inline values.

---

## S6. AI image generators garbling Chinese text

**Symptom.** You ask an image-generation model to put a Chinese-language UI on a screen in a photo — e.g. a price comparison table, a dashboard, a notification list. The output is photorealistic, but the Chinese characters are nonsense: `售米怀核对议表` instead of `集采价格对比表`, `Saving saving / Trachs / PHP / Shippinng bonds` mixed with broken pinyin, made-up characters that look like Chinese but aren't.

**Root cause.** Every current frontier image model — Gemini 2.5/3.x image, Imagen, Midjourney, DALL-E — generates Chinese characters as visual-pattern hallucinations rather than actual encoded characters. They have no glyph-correctness loss term in training. Increasing prompt clarity doesn't help; switching models often just changes the flavor of garbage.

**Reliable fix.** Don't ask the model to render Chinese. Use Pillow (Python Imaging Library) to overlay correct Chinese text onto the screen region of the otherwise photo-realistic image:

```python
from PIL import Image, ImageDraw, ImageFont

im = Image.open("scene-with-garbled-screen.png").convert("RGBA")
draw = ImageDraw.Draw(im)

# 1) Cover the garbled screen area with a clean panel
TL, TR, BR, BL = (365, 60), (920, 80), (920, 360), (365, 350)
draw.polygon([TL, TR, BR, BL], fill=(252, 253, 255, 255))

# 2) Draw correct Chinese text using a real CJK font
font = ImageFont.truetype("/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc", 19)
draw.text((TL[0] + 16, TL[1] + 12), "集采价格对比表", font=font, fill=(20, 25, 30))
# ... table rows etc.

im.save("scene-with-clean-screen.png")
```

If the screen has perspective distortion, use OpenCV's `getPerspectiveTransform` + `warpPerspective` to warp a clean-rendered Chinese panel onto the four screen corners. Either way: the model's job is to make the photo look right; PIL's job is to put the right text on the screen.

audit_assets.py's `AI-GIBBERISH` check looks for the characteristic mis-spelled tokens.

---

## S7. AI image platform watermarks left in delivered assets

**Symptom.** A demo screenshot in the deck shows the host platform's UI chrome — a "Try [Platform] Canvas" pill in the bottom-right, a "[Platform] PRO" badge in the top-left, a "Report unsafe content" link in the bottom-left.

**Root cause.** The image was screen-captured from the generation platform's web UI directly, without cropping out the platform's own chrome.

**Fix.** Either re-export via the platform's "download image" button (which usually strips chrome), or paint white rectangles over the chrome areas with Pillow before using the image. Watch out for the bottom strip — chrome buttons can extend ~60 px up from the bottom edge, so the rectangle has to cover that full band.

```python
from PIL import Image, ImageDraw
im = Image.open(p).convert("RGBA")
W, H = im.size
draw = ImageDraw.Draw(im)
draw.rectangle((0, 0, W, 75), fill=(255, 255, 255, 255))     # top chrome
draw.rectangle((0, H - 65, W, H), fill=(255, 255, 255, 255)) # bottom chrome
im.convert("RGB").save(p_clean, "JPEG", quality=92)
```

audit_assets.py's `AI-WATERMARK` check (OCR-based) catches the common phrases.

---

## S8. Demo screenshots with English placeholder text

**Symptom.** A product mockup screenshot shows a dashboard or product list where category icons / product names are rendered as English placeholders: `Oil`, `Egg`, `Bread`, `Milk`, `Apple`, `Detergent`. The rest of the deck is in Chinese.

**Root cause.** The image was generated by a model that fell back to English when it couldn't reliably render the intended Chinese category names.

**Fix.** Same recipe as S6 — use Pillow to paint over the English placeholders with the correct Chinese category labels. Match the placeholder's font color, weight, and position so it looks native to the underlying UI design.

audit_assets.py's `ENG-PLACEHOLDER` check (OCR-based) catches the common ones.

---

## S9. Legacy "dark + neon emoji" demo screenshots clashing with a redesigned deck

**Symptom.** Most of the deck has moved to a clean light-card aesthetic, but a handful of demo screenshots are still in the old style — dark background, neon highlights, cartoon emoji avatars (`👴 / 👨 / 👩`), big red "EMERGENCY CALL" buttons. The contrast makes the dark screenshots look broken next to the new ones.

**Fix.** Re-shoot the demo screenshots in the new style. If a re-shoot isn't feasible in the time available, at minimum:

- Don't put a dark screenshot next to a light one on the same page
- Crop the dark screenshot to remove the most jarring elements (the cartoon emojis, the lurid red buttons)

The project should maintain `<DECK_ROOT>/.audit/legacy_assets.txt` listing the dark-style filenames that need replacement — audit_assets.py's `LEGACY-IMG` check then catches them when they re-appear in new pages.

---

## S10. Inline `<style>` rules overriding the global design-system override

**Symptom.** You add a global CSS rule in `assets/base.css` to fix some issue across all slides (e.g. the S5 `.page-footer { right: 240px !important }` fix). After rebuilding, some pages still show the bug.

**Root cause.** Each slide carries its own inline `<style>` block. A rule like `.page-footer { right: 96px; }` in an inline block has CSS specificity (0, 1, 0), which is normally lower than `.slide .page-footer` at (0, 2, 0). But if the project's slides happened to use a more specific selector (e.g. `.slide.dark .page-footer`), the global override loses the cascade race.

**Fix.** Make the global override `!important`. The cascade only treats two `!important` declarations the same way, and specificity wins among them — so a `!important` on `.slide .page-footer` (0, 2, 0) beats a non-`!important` on `.slide.dark .page-footer` (0, 2, 0 + 1 class) even though the inline one is more specific.

```css
.slide .page-footer {
  right: 240px !important;
}
```

audit_structure.py's `FOOTER-RIGHT` check explicitly looks for `!important` in the global rule to mark the project as having the override in place.

---

## S11. Decks with > 100 inline slides exceeding browser memory

**Symptom.** After `build_mega.py`, the resulting `mega.html` is 25-30 MB and crashes the browser tab on lower-end devices, or takes 5+ seconds to first paint.

**Mitigations.**

- Aggressive image compression in `inline_images.py` — JPEG quality 60-65, max dimension 960 px (the original-quality images can stay in `assets/` for the non-inline version of the deck).
- Move very large slides to lazy-loaded sections — `inline_images.py` can be configured to write images out as separate files for these and the SLIDES array can use a regular URL instead of a data URI.
- For decks > 150 slides, consider splitting into multiple mega files (e.g. `mega-part-1.html`, `mega-part-2.html`).

No automated check for this — but if `build_mega.py` reports a file size > 50 MB, audit run_all.sh prints a warning.

---

## How to extend this list

When you find a new class of bug that:

- Repeats across decks
- Is hard to find by eye but easy to find with a script
- Has a clear remediation

…document it here (de-identified — no project names, no specific customer names, no exact byte counts that would identify a particular deck). The point is that the next person who hits the same problem can find this file and stop reinventing the wheel.

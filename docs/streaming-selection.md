# Selecting and copying streamed responses

Assistant output continues to render while text is selected. Both the shared
conversation timeline (including code mode and auxiliary panels) and legacy
chat bubbles commit the existing sanitized Markdown output through
`features/conversation/rich-text-dom.js`.

The commit reconciles matching elements and splices changed text instead of
replacing the message's HTML tree. Renderer-owned code copy buttons and live
regions retain their identity. A copy snapshots the current code before its
asynchronous clipboard write; later output cannot change that snapshot or
discard the visible success/failure feedback. For source-backed updates, source-positioned code and table-part identities prevent a removed code block’s controls or pending feedback from moving to another block at the same DOM position, whether Markdown resolution or a presentation filter removes it. Detached source diagnostics scan real HTML tag boundaries, excluding quoted attributes, comments and raw text. If provenance cannot be validated, removed blocks receive fresh controls rather than inheriting old feedback. Surviving source-backed code blocks can be adopted across changed ancestors, including newly inserted wrappers, so their own pending feedback remains visible. Table parts also retain their source identity, so a recycled cell cannot direct a selection into newly appended equal text. Ordinary matching siblings stay in place. Source corrections retain a block only when its opening remains in the common source prefix. A bounded last-frame cache skips repeated detached diagnostics for unchanged, closed, top-level Markdown code with verified payload/markup and no HTML, tables or presentation transform. A second bounded last-frame snapshot reuses the validated previous diagnostic during source appends, including open and nested code fences, after checking the actual prior text and ordered block ranges. Each current frame still receives a fresh diagnostic; this removes redundant previous-frame parsing without inferring ownership from equal code. Other source changes or mismatched DOM fall back to both diagnostics. Large open code therefore still has linear per-frame current-source parsing, sanitization and DOM indexing costs; active selections involving filtered or changing HTML may require additional retirement diagnostics. Inert template content is reconciled through its content fragment.

Native selection endpoints are indexed in DOM text coordinates, excluding copy
controls. Reconciliation and fade wrapping/cleanup restore those endpoints when
Markdown or syntax highlighting changes the underlying structure. Text alignment
accounts for multiple delimiter/whitespace edits in one frame, including inline
closures, table headers and loose lists. The selected text anchors both endpoints
together when retained, so repeated new output cannot extend the range. Text
alignment excludes syntax only when corresponding DOM links/images materialize.
The bridge carries the text snapshot used for each throttled HTML render. Actual parser tokens, the shared CRLF/Tab/container source maps and append-only source positions also identify retired HTML, entity, escape and formatting prefixes. Retained references and literal code continue to anchor positions. Semantic
correspondence also covers nested images with no change in image count. For newly materialized images, a detached diagnostic render annotates previous parser text and passes it through the same sanitizer, so removed or inert HTML cannot transfer ownership to an equal visible occurrence. Diagnostic resource attributes are removed and the resulting text must match the actual DOM; annotations never enter the message. Legacy presentation folds apply the previous frame’s same draft filtering to its diagnostic render, including when generation completes or its placeholder label changes. Source-positioned code identities in detached renders also collapse selections in code that becomes a hidden draft, while retaining identical text in other code blocks. Allocation is
bounded; large rewrites use whitespace-normalized neighboring text, and fallback endpoints cannot
reverse the selected range. Browser-decoded entities inside raw HTML CODE/PRE retire their old spellings and previously decoded values when further input changes the character, while retaining an unchanged decoded prefix. A detached render verifies EOF entities and HTML prefixes against their actual visible ownership, so hidden attributes, comments, inert templates or filtering elsewhere cannot retire adjacent prose. Markdown code retains literal entities. Source-owned code spaces and tabs remain selected even at the live output tail; only trailing layout whitespace outside code may contract. Each retained code, cell, caption or table endpoint (including implicit row-group/row element boundaries and cross-message endpoints) uses bounded alignment within its original part, even while that part grows or closes formatting. This disambiguates browser foster parenting that inserts new prose before an existing table, while source retirement remains authoritative. A contiguous native range includes text that the browser inserts between its retained endpoints. Appended text
does not automatically extend an existing selection. A selection entirely inside
removed syntax (including closing code fences, image alt, entities and incomplete reference
definitions and multiline titles identified by the existing Markdown parser) collapses at its original boundary instead of selecting unrelated
new output. Backward selections, ranges with an endpoint in another message, and distinct native endpoints sharing one text coordinate are preserved. Equal HTML only skips reconciliation when the source and presentation policy are also unchanged. Scroll-follow yields
while the conversation contains a noncollapsed selection, including the auxiliary panel’s parent follow effect; rendering continues.

This change retains the existing parser, sanitizer, streaming throttle and final
text flush. It introduces no dependency or platform-specific application branch.

Run the deterministic DOM checks and Chromium interaction regression with:

```bash
node --test pinvou3-app/tests/rich_text_dom.test.mjs
npm --prefix pinvou3-app run test:streaming-rich-text
```

The browser regression covers active paragraphs, open code fences, completion,
formatting changes (with forward/backward selections at each Markdown prefix),
backward/cross-message ranges, pointer clicks spanning a
render commit, pending/failed clipboard writes, delayed animation cleanup and the actual auxiliary panel’s selected reading position.

The virtualization smoke also covers retaining the selected reading position
while a growing live tail completes and migrates into measured history rows:

```bash
npm --prefix pinvou3-app run test:conversation-virtualization
```

On Linux with Python GI, GTK 3, WebKit2 4.1 and a display, also run:

```bash
PINVOU_NATIVE_WEBKIT=1 npm --prefix pinvou3-app run test:streaming-rich-text
```

This additionally loads the real React fixture in an offscreen native WebKitGTK
view. Both engines use a clipboard test double; system clipboard integration
is outside these rendering regressions.

import { useRef, useState } from 'react';

const LONGPRESS_MS = 350;
    const MOVE_CANCEL = 10;
    // Instant-move drag (move to project): entered as soon as the press moves
    // past the threshold. Does not rely on HTML5 DnD — WebKitGTK's in-page
    // drag stops delivering dragover/drop once the pointer stops moving (a
    // drop after hovering is silently discarded), so outside Chromium it
    // cannot be trusted; the pointer path is consistent across platforms.
    // { enabled, payload, onBegin(geom), onHover(key|null), onDrop(key|null, payload), onEnd() }
    // onBegin reports the grab geometry at the activation instant (isomorphic
    // to tear-off's info, driving the cursor-following ghost); onEnd is always
    // called after drop/cancel and is used to fold the ghost away.
    const useLongPressDrag = (kind, onPickUp, moveDrag) => {
      const startRef = useRef(null);
      const timerRef = useRef(null);
      const pickedRef = useRef(false);
      // Per-gesture state for the instant-move drag: after pointer capture,
      // move/up all land on the source element.
      const moveDragRef = useRef({ active: false, payload: null });
      // There is a timing window between the drag end and the click event (the
      // state is already reset), so suppression must go through a ref.
      const suppressClickRef = useRef(false);
      const [moveDragging, setMoveDragging] = useState(false);
      const moveDragEnabled = () => !!(moveDrag && moveDrag.enabled && moveDrag.payload !== undefined && moveDrag.payload !== null);
      // A native HTML5 drag starting while a press is pending means the
      // gesture became a session drag: browsers fire pointercancel and stop
      // delivering pointermove (so the >MOVE_CANCEL defense can never fire),
      // and without this coupling the 350ms timer survives the whole native
      // drag and spawns a tear-off mid-drag. The listener is { once } so it
      // self-removes on the first dragstart; when a press ends without one it
      // lingers until the next dragstart and then runs as a harmless no-op
      // (every branch is idempotent on cleared state).
      const clearPress = () => {
        if (timerRef.current) { clearTimeout(timerRef.current); timerRef.current = null; }
        startRef.current = null;
        document.body.style.userSelect = '';
      };
      const endMoveDrag = (drop) => {
        const session = moveDragRef.current;
        moveDragRef.current = { active: false, payload: null };
        setMoveDragging(false);
        suppressClickRef.current = true;
        document.body.style.userSelect = '';
        if (moveDrag && moveDrag.onHover) moveDrag.onHover(null);
        if (drop && moveDrag && moveDrag.onDrop) {
          const target = document.elementFromPoint(session.lastX, session.lastY);
          const dropTarget = target && target.closest
            ? target.closest('[data-project-drop-target]')
            : null;
          moveDrag.onDrop(dropTarget ? dropTarget.getAttribute('data-drop-key') : null, session.payload);
        }
        if (moveDrag && moveDrag.onEnd) moveDrag.onEnd();
      };
      const onPointerDown = (e) => {
        if (e.button !== 0 || !kind) return;
        // A fresh press means any pending click suppression is stale: when
        // pointer capture failed, the post-drag click never reached this
        // row's guardClick, so without this reset the flag would eat the next
        // legitimate click. The suppressed click always fires before the next
        // pointerdown, so clearing here is safe.
        suppressClickRef.current = false;
        // Session rows put the drag handlers on their label button, which is
        // itself marked data-drag-surface: presses on it start the long-press
        // drag. Every other button/input (pin, more, confirm…) lacks the
        // marker and is skipped so pressing an action never picks up the row.
        if (e.target && e.target.closest && e.target.closest('button:not([data-drag-surface]), input')) return;
        const rect = e.currentTarget.getBoundingClientRect();
        startRef.current = { x: e.clientX, y: e.clientY, rect, pointerId: e.pointerId, currentTarget: e.currentTarget };
        pickedRef.current = false;
        document.addEventListener('dragstart', clearPress, { capture: true, once: true });
        document.body.style.userSelect = 'none'; // 长按期间禁选,防止选中下方会话文字
        timerRef.current = setTimeout(() => {
          pickedRef.current = true;
          timerRef.current = null;
          // 客户端像素:按下点相对标签左上角的偏移 + 标签尺寸 + 起点 → 喂给 DOM avatar 锁定相对位置。
          const r = startRef.current.rect;
          const info = {
            dx: startRef.current.x - r.left,
            dy: startRef.current.y - r.top,
            w: r.width,
            h: r.height,
            startX: startRef.current.x,
            startY: startRef.current.y,
          };
          if (onPickUp) onPickUp(info);
        }, LONGPRESS_MS);
      };
      const onPointerMove = (e) => {
        const s = startRef.current;
        if (!s) {
          if (!moveDragRef.current.active) return;
          moveDragRef.current.lastX = e.clientX;
          moveDragRef.current.lastY = e.clientY;
          const target = document.elementFromPoint(e.clientX, e.clientY);
          const dropTarget = target && target.closest
            ? target.closest('[data-project-drop-target]')
            : null;
          if (moveDrag && moveDrag.onHover) {
            moveDrag.onHover(dropTarget ? dropTarget.getAttribute('data-drop-key') : null);
          }
          return;
        }
        if (pickedRef.current) return;
        if (Math.hypot(e.clientX - s.x, e.clientY - s.y) > MOVE_CANCEL) {
          // The instant-move drag takes priority (dragging a session in the
          // project view); without that mode the old semantics hold: a fast
          // move falls to native HTML5 drag / plain click cancellation.
          if (moveDragEnabled()) {
            const geom = {
              dx: s.x - s.rect.left,
              dy: s.y - s.rect.top,
              w: s.rect.width,
              h: s.rect.height,
              startX: s.x,
              startY: s.y,
            };
            clearPress();
            moveDragRef.current = { active: true, payload: moveDrag.payload, lastX: e.clientX, lastY: e.clientY };
            setMoveDragging(true);
            try { s.currentTarget.setPointerCapture(s.pointerId); } catch { /* edge cases like an already-released pointer: a capture failure does not affect hit-testing */ }
            if (moveDrag && moveDrag.onBegin) moveDrag.onBegin(geom);
            return;
          }
          clearPress();
        }
      };
      const onPointerUp = (e) => {
        if (moveDragRef.current.active) {
          moveDragRef.current.lastX = e.clientX;
          moveDragRef.current.lastY = e.clientY;
          endMoveDrag(true);
          return;
        }
        clearPress();
      };
      const onPointerCancel = () => {
        if (moveDragRef.current.active) { endMoveDrag(false); return; }
        clearPress();
      };
      const guardClick = (fn) => (e) => {
        if (pickedRef.current || suppressClickRef.current) {
          pickedRef.current = false;
          suppressClickRef.current = false;
          e.stopPropagation();
          if (e.preventDefault) e.preventDefault();
          return;
        }
        if (fn) fn(e);
      };
      return { handlers: { onPointerDown, onPointerMove, onPointerUp, onPointerCancel }, guardClick, moveDragging };
    };

    // ==========================================
    // Render
    // ==========================================

export { LONGPRESS_MS, MOVE_CANCEL, useLongPressDrag };

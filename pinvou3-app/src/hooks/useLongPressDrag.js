import { useRef } from 'react';

const LONGPRESS_MS = 350;
    const MOVE_CANCEL = 10;
    const useLongPressDrag = (kind, onPickUp) => {
      const startRef = useRef(null);
      const timerRef = useRef(null);
      const pickedRef = useRef(false);
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
      const onPointerDown = (e) => {
        if (e.button !== 0 || !kind) return;
        // Session rows put the drag handlers on their label button, which is
        // itself marked data-drag-surface: presses on it start the long-press
        // drag. Every other button/input (pin, more, confirm…) lacks the
        // marker and is skipped so pressing an action never picks up the row.
        if (e.target && e.target.closest && e.target.closest('button:not([data-drag-surface]), input')) return;
        const rect = e.currentTarget.getBoundingClientRect();
        startRef.current = { x: e.clientX, y: e.clientY, rect };
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
        if (!s || pickedRef.current) return;
        if (Math.hypot(e.clientX - s.x, e.clientY - s.y) > MOVE_CANCEL) clearPress();
      };
      const onPointerUp = () => clearPress();
      const guardClick = (fn) => (e) => {
        if (pickedRef.current) {
          pickedRef.current = false;
          e.stopPropagation();
          if (e.preventDefault) e.preventDefault();
          return;
        }
        if (fn) fn(e);
      };
      return { handlers: { onPointerDown, onPointerMove, onPointerUp, onPointerCancel: clearPress }, guardClick };
    };

    // ==========================================
    // Render
    // ==========================================

export { LONGPRESS_MS, MOVE_CANCEL, useLongPressDrag };

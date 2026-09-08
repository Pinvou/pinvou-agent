import { useRef, useState } from 'react';

const LONGPRESS_MS = 350;
    const MOVE_CANCEL = 10;
    // 即移拖拽(移动到项目):按下后立即移动超过阈值进入。不依赖 HTML5 DnD——
    // WebKitGTK 的页内拖放在指针停止移动后不再投递 dragover/drop(悬停后松手
    // 被静默丢弃),Chromium 之外不可依赖;指针路径全平台一致。
    // { enabled, payload, onHover(key|null), onDrop(key|null, payload) }
    const useLongPressDrag = (kind, onPickUp, moveDrag) => {
      const startRef = useRef(null);
      const timerRef = useRef(null);
      const pickedRef = useRef(false);
      // 即移拖拽会话内状态:pointer capture 之后的 move/up 都落在源元素上。
      const moveDragRef = useRef({ active: false, payload: null });
      // 拖拽结束到 click 事件之间存在时序窗口(state 已复位),抑制必须走 ref。
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
      };
      const onPointerDown = (e) => {
        if (e.button !== 0 || !kind) return;
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
          // 优先即移拖拽(项目视图里拖会话);没有该模式时维持旧语义:
          // 快速移动交给原生 HTML5 拖拽/普通点击取消。
          if (moveDragEnabled()) {
            clearPress();
            moveDragRef.current = { active: true, payload: moveDrag.payload, lastX: e.clientX, lastY: e.clientY };
            setMoveDragging(true);
            try { s.currentTarget.setPointerCapture(s.pointerId); } catch { /* 已释放等边缘:捕获失败不影响命中测试 */ }
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

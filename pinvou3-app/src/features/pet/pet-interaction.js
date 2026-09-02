import {
  availableTauriMonitors,
  createPhysicalPosition,
  currentTauriMonitor,
  getCurrentTauriWindow,
} from '../../platform/tauri/client.js';

const nativeWindowAdapter = {
  availableMonitors: availableTauriMonitors,
  currentMonitor: currentTauriMonitor,
  getCurrentWindow: getCurrentTauriWindow,
};

function requireWindowApi(adapter = nativeWindowAdapter) {
  const api = adapter;
  if (!api || typeof api.getCurrentWindow !== 'function') {
    throw new Error('Tauri window.getCurrentWindow is unavailable');
  }
  if (typeof api.currentMonitor !== 'function') {
    throw new Error('Tauri window.currentMonitor is unavailable');
  }
  return api;
}

export async function readPetDragContext(adapter) {
  const api = requireWindowApi(adapter);
  const win = api.getCurrentWindow();
  if (!win || typeof win.innerPosition !== 'function' || typeof win.innerSize !== 'function') {
    throw new Error('Tauri current window geometry API is unavailable');
  }
  const monitorPromise = Promise.resolve()
    .then(() => api.currentMonitor())
    .catch(() => null);
  const monitorsPromise = typeof api.availableMonitors === 'function'
    ? Promise.resolve().then(() => api.availableMonitors()).catch(() => [])
    : Promise.resolve([]);
  const [position, size, monitor, available] = await Promise.all([
    win.innerPosition(),
    win.innerSize(),
    monitorPromise,
    monitorsPromise,
  ]);
  const monitors = Array.isArray(available) && available.length > 0
    ? available
    : (monitor ? [monitor] : []);
  return {
    win,
    position,
    size,
    monitor,
    monitors,
  };
}

function monitorArea(monitor) {
  if (!monitor) return null;
  const source = monitor.workArea || monitor.work_area || monitor;
  const position = source.position;
  const size = source.size;
  const l = Number(position && position.x);
  const t = Number(position && position.y);
  const width = Number(size && size.width);
  const height = Number(size && size.height);
  if (![l, t, width, height].every(Number.isFinite) || width <= 0 || height <= 0) return null;
  return { l, t, r: l + width, b: t + height, monitor };
}

function monitorAreas(monitors) {
  return (Array.isArray(monitors) ? monitors : []).map(monitorArea).filter(Boolean);
}

function areaContains(area, x, y) {
  return x >= area.l && x < area.r && y >= area.t && y < area.b;
}

function clamp(value, low, high) {
  return Math.min(Math.max(value, low), Math.max(low, high));
}

function connectedMonitorArea(monitors, seed) {
  const areas = monitorAreas(monitors);
  if (areas.length === 0) return null;
  const seedArea = monitorArea(seed);
  let seedIndex = seed ? areas.findIndex((area) => area.monitor === seed) : -1;
  if (seedIndex < 0 && seedArea) {
    const cx = (seedArea.l + seedArea.r) / 2;
    const cy = (seedArea.t + seedArea.b) / 2;
    seedIndex = areas.findIndex((area) => areaContains(area, cx, cy));
  }
  if (seedIndex < 0) seedIndex = 0;
  const selected = new Set([seedIndex]);
  const queue = [seedIndex];
  while (queue.length > 0) {
    const index = queue.shift();
    const current = areas[index];
    for (let candidate = 0; candidate < areas.length; candidate += 1) {
      if (selected.has(candidate)) continue;
      const next = areas[candidate];
      const horizontalGap = Math.max(current.l - next.r, next.l - current.r, 0);
      const verticalGap = Math.max(current.t - next.b, next.t - current.b, 0);
      if (horizontalGap <= 1 && verticalGap <= 1) {
        selected.add(candidate);
        queue.push(candidate);
      }
    }
  }
  const group = [...selected].map((index) => areas[index]);
  return {
    l: Math.min(...group.map((area) => area.l)),
    t: Math.min(...group.map((area) => area.t)),
    r: Math.max(...group.map((area) => area.r)),
    b: Math.max(...group.map((area) => area.b)),
  };
}

function petLocalRect({
  size,
  alignment,
  verticalAlignment,
  viewportHeight,
  characterWidth,
  characterHeight,
  horizontalPadding,
  verticalPadding,
  localTop,
  localBottom,
}) {
  const windowWidth = Number(size && size.width);
  const windowHeight = Number(size && size.height);
  const width = Number(characterWidth);
  const height = Number(characterHeight);
  const horizontal = Number(horizontalPadding);
  const vertical = Number(verticalPadding);
  if (![windowWidth, width, height, horizontal].every(Number.isFinite)) return null;
  const left = alignment === 'left' ? horizontal : windowWidth - horizontal - width;
  const viewport = Number(viewportHeight);
  // 竖向优先用实测值(getBoundingClientRect):X11 下 outerSize 回读不可靠,
  // 用 windowHeight 反推的人物底边会偏高,拖拽起手时把人物钳上去。实测缺失
  // 时回退到 windowHeight - verticalPadding(Windows 上两者一致)。
  let top;
  let bottom;
  if (Number.isFinite(localTop) && Number.isFinite(localBottom)) {
    // 拖拽起手已经从真实 DOM 测得人物在 WebView 内的位置时，以实测为准。
    // X11 的异步 resize 会让外框尺寸与当前内容视口短暂不同，继续按
    // viewport/固定 padding 反推会在物理循环启动后把窗口向上钳一次。
    top = Number(localTop);
    bottom = Number(localBottom);
  } else if (Number.isFinite(viewport) && Number.isFinite(vertical)) {
    top = verticalAlignment === 'top' ? 0 : viewport - vertical - height;
    bottom = top + height;
  } else {
    bottom = Number.isFinite(windowHeight) && Number.isFinite(vertical)
      ? windowHeight - vertical
      : NaN;
    top = bottom - height;
  }
  if (![bottom, top].every(Number.isFinite)) return null;
  return { l: left, t: top, r: left + width, b: bottom };
}

export function petWindowBounds({
  monitors,
  monitor,
  clientOriginVerticalBounds,
  ...geometry
}) {
  const desktop = connectedMonitorArea(monitors, monitor);
  const pet = petLocalRect(geometry);
  if (!desktop || !pet) return null;
  const bounds = {
    l: desktop.l - pet.l,
    t: desktop.t - pet.t,
    r: desktop.r - pet.r,
    b: desktop.b - pet.b,
  };
  const clientTop = clientOriginVerticalBounds && clientOriginVerticalBounds.t;
  const clientBottom = clientOriginVerticalBounds && clientOriginVerticalBounds.b;
  if (Number.isFinite(clientTop) && Number.isFinite(clientBottom)) {
    bounds.t = Math.max(bounds.t, clientTop);
    bounds.b = Math.max(bounds.t, Math.min(bounds.b, clientBottom));
  }
  return bounds;
}

export function petElementHorizontalBounds({
  monitors,
  monitor,
  localLeft,
  localRight,
}) {
  const desktop = connectedMonitorArea(monitors, monitor);
  const left = Number(localLeft);
  const right = Number(localRight);
  if (!desktop || !Number.isFinite(left) || !Number.isFinite(right) || right <= left) return null;
  return {
    l: desktop.l - left,
    r: desktop.r - right,
  };
}

function clientOriginVerticalBounds(area, viewportHeight) {
  const height = Number(viewportHeight);
  if (!area || !Number.isFinite(height) || height <= 0) return null;
  return {
    t: area.t,
    b: Math.max(area.t, area.b - height),
  };
}

export function petClientOriginVerticalBounds({ monitor, viewportHeight }) {
  // 布局翻转看当前显示器边缘，不能用多屏外接矩形：异高/错位屏幕会把
  // 另一块屏的边缘误当成当前人物的触边位置。
  return clientOriginVerticalBounds(monitorArea(monitor), viewportHeight);
}

export function petConnectedClientOriginVerticalBounds({
  monitors,
  monitor,
  viewportHeight,
}) {
  // 最终硬钳制使用相连桌面的整体范围，允许人物继续跨到上下相邻屏幕；
  // 外接矩形中的空洞随后由 clampPetDragToDesktop 投影回真实显示器。
  return clientOriginVerticalBounds(
    connectedMonitorArea(monitors, monitor),
    viewportHeight,
  );
}

export function petAlignmentAtDragEdge({
  currentAlignment,
  x,
  tx,
  holding,
  bounds,
  threshold = 1,
}) {
  const current = currentAlignment === 'left' ? 'left' : 'right';
  if (!bounds) return current;
  const edgeX = holding && Number.isFinite(tx) ? tx : x;
  const margin = Number.isFinite(threshold) ? Math.max(0, threshold) : 1;
  if (!Number.isFinite(edgeX)) return current;
  if (edgeX <= bounds.l + margin) return 'left';
  if (edgeX >= bounds.r - margin) return 'right';
  return current;
}

export function petVerticalAlignmentAtDragEdge({
  currentAlignment,
  y,
  ty,
  holding,
  bounds,
  threshold = 1,
}) {
  const current = currentAlignment === 'top' ? 'top' : 'bottom';
  if (!bounds) return current;
  const edgeY = holding && Number.isFinite(ty) ? ty : y;
  const margin = Number.isFinite(threshold) ? Math.max(0, threshold) : 1;
  if (!Number.isFinite(edgeY)) return current;
  if (edgeY <= bounds.t + margin) return 'top';
  if (edgeY >= bounds.b - margin) return 'bottom';
  return current;
}

export function petMonitorAtPosition({ position, monitors, ...geometry }) {
  const pet = petLocalRect(geometry);
  if (!position || !pet) return null;
  const cx = Number(position.x) + (pet.l + pet.r) / 2;
  const cy = Number(position.y) + (pet.t + pet.b) / 2;
  if (![cx, cy].every(Number.isFinite)) return null;
  for (const monitor of Array.isArray(monitors) ? monitors : []) {
    const area = monitorArea(monitor);
    if (area && areaContains(area, cx, cy)) return monitor;
  }
  return null;
}

// 人物中心必须停在某块显示器的工作区内。
// petWindowBounds 用的是相连显示器的外接矩形——异高/错位/L 形布局下，
// 外接矩形会包含实际没有屏幕的空洞，宠物停进去就等于消失（重启才会被
// pet_window.rs 的 point_on_any_monitor 救回默认位置）。这里按各显示器
// 工作区的并集做二次判定，落进空洞就投影到最近的显示器边界。
export function clampPetDragToDesktop(state, { monitors, ...geometry }) {
  const pet = petLocalRect(geometry);
  const areas = monitorAreas(monitors);
  if (!state || !pet || areas.length === 0) return state;
  const { x, y } = state;
  if (![x, y].every(Number.isFinite)) return state;
  const offsetX = (pet.l + pet.r) / 2;
  const offsetY = (pet.t + pet.b) / 2;
  const cx = x + offsetX;
  const cy = y + offsetY;
  if (areas.some((area) => areaContains(area, cx, cy))) return state;
  let best = null;
  for (const area of areas) {
    const nx = clamp(cx, area.l, area.r - 1);
    const ny = clamp(cy, area.t, area.b - 1);
    const distance = (nx - cx) ** 2 + (ny - cy) ** 2;
    if (!best || distance < best.distance) best = { distance, cx: nx, cy: ny };
  }
  const next = { ...state, x: best.cx - offsetX, y: best.cy - offsetY };
  if (next.x !== x) {
    next.vx = 0;
    if (state.holding) {
      next.tx = next.x;
      next.lastTx = next.x;
    }
  }
  if (next.y !== y) {
    next.vy = 0;
    if (state.holding) {
      next.ty = next.y;
      next.lastTy = next.y;
    }
  }
  return next;
}

export function petEdgeAlignment({
  position,
  size,
  monitor,
  fallback = 'right',
  currentAlignment,
  characterWidth,
  horizontalPadding,
}) {
  if (!position || !size || !monitor) return fallback === 'left' ? 'left' : 'right';
  const monitorLeft = Number(monitor.position && monitor.position.x);
  const monitorWidth = Number(monitor.size && monitor.size.width);
  const windowLeft = Number(position.x);
  const windowWidth = Number(size.width);
  if (![monitorLeft, monitorWidth, windowLeft, windowWidth].every(Number.isFinite)) {
    return fallback === 'left' ? 'left' : 'right';
  }
  const hasCurrentAlignment = currentAlignment === 'left' || currentAlignment === 'right';
  const side = currentAlignment === 'left' ? 'left' : 'right';
  const width = Number(characterWidth);
  const padding = Number(horizontalPadding);
  const hasPetAnchor = Number.isFinite(width) && width >= 0
    && Number.isFinite(padding) && padding >= 0;
  const localLeft = side === 'left' ? padding : windowWidth - padding - width;
  if (hasCurrentAlignment && hasPetAnchor) {
    const petLeft = windowLeft + localLeft;
    const petRight = petLeft + width;
    const monitorRight = monitorLeft + monitorWidth;
    if (petLeft <= monitorLeft + 1) return 'left';
    if (petRight >= monitorRight - 1) return 'right';
    return side;
  }
  const localCenter = hasPetAnchor ? localLeft + width / 2 : windowWidth / 2;
  const petCenter = windowLeft + localCenter;
  const monitorCenter = monitorLeft + monitorWidth / 2;
  return petCenter <= monitorCenter ? 'left' : 'right';
}

export function rebasePetDragForAlignment(state, {
  from,
  to,
  windowWidth,
  characterWidth,
  horizontalPadding,
}) {
  const source = from === 'left' ? 'left' : 'right';
  const target = to === 'left' ? 'left' : 'right';
  const width = Number(windowWidth);
  const character = Number(characterWidth);
  const padding = Number(horizontalPadding);
  const anchorDistance = [width, character, padding].every(Number.isFinite)
    ? Math.max(0, width - character - padding * 2)
    : 0;
  const offset = source === target
    ? 0
    : (source === 'left' ? -anchorDistance : anchorDistance);
  const shifted = (value) => (Number.isFinite(value) ? value + offset : value);
  return {
    ...state,
    x: shifted(state.x),
    tx: shifted(state.tx),
    lastTx: shifted(state.lastTx),
  };
}

export function rebasePetDragForVerticalAlignment(state, {
  from,
  to,
  viewportHeight,
  characterHeight,
  verticalPadding,
  previousLocalTop,
  nextLocalTop,
}) {
  const source = from === 'top' ? 'top' : 'bottom';
  const target = to === 'top' ? 'top' : 'bottom';
  const viewport = Number(viewportHeight);
  const character = Number(characterHeight);
  const padding = Number(verticalPadding);
  const measuredPreviousTop = Number(previousLocalTop);
  const measuredNextTop = Number(nextLocalTop);
  const hasMeasuredShift = Number.isFinite(measuredPreviousTop)
    && Number.isFinite(measuredNextTop);
  const anchorDistance = [viewport, character, padding].every(Number.isFinite)
    ? Math.max(0, viewport - character - padding)
    : 0;
  const offset = source === target
    ? 0
    : (hasMeasuredShift
      ? measuredPreviousTop - measuredNextTop
      : (source === 'top' ? -anchorDistance : anchorDistance));
  const shifted = (value) => (Number.isFinite(value) ? value + offset : value);
  return {
    ...state,
    y: shifted(state.y),
    ty: shifted(state.ty),
    lastTy: shifted(state.lastTy),
  };
}

export function attachPetDragGeometry(state, { position }) {
  const pointerScale = Number.isFinite(state.pointerScale) ? state.pointerScale : 1;
  const currentCX = Number.isFinite(state.currentCX) ? state.currentCX : state.startCX;
  const currentCY = Number.isFinite(state.currentCY) ? state.currentCY : state.startCY;
  const tx = position.x + (currentCX - state.startCX) * pointerScale;
  const ty = position.y + (currentCY - state.startCY) * pointerScale;
  const next = {
    ...state,
    geometryReady: true,
    x: position.x,
    y: position.y,
    tx,
    ty,
    lastTx: state.releasePending ? position.x : tx,
    lastTy: state.releasePending ? position.y : ty,
  };

  return next;
}

export function releasePetDrag(state) {
  // 松手可能发生在 pointermove 后、下一 rAF 前，或 resize 同步暂停期间。
  // 保留一次严格跟手帧消费最新 tx/ty，然后立即停住，不能丢掉尾帧位移，
  // 也不能让历史速度在松手后继续推动或反弹窗口。
  return { ...state, releasePending: true };
}

export function setPetWindowPosition(win, x, y, positionFactory = createPhysicalPosition) {
  try {
    if (!win || typeof win.setPosition !== 'function') {
      throw new Error('Tauri window.setPosition is unavailable');
    }
    const position = positionFactory(Math.round(x), Math.round(y));
    return Promise.resolve(win.setPosition(position));
  } catch (error) {
    return Promise.reject(error);
  }
}

export function dragAnimationFromMotion(current, deltaX, deltaY) {
  const dx = Number.isFinite(deltaX) ? deltaX : 0;
  const dy = Number.isFinite(deltaY) ? deltaY : 0;
  const next = dx > 0 ? 'running-right' : 'running-left';
  // 迟滞：慢速拖动时相邻事件增量只有 ±1px，手抖的反向事件和真实换向
  // 无法靠单事件区分，反转方向必须要求更大的位移，否则方向会被噪声翻转。
  const threshold = current && next !== current ? 3 : 1;
  if (Math.abs(dx) < threshold || Math.abs(dx) < Math.abs(dy) * 0.35) return current;
  return next;
}

export function stepPetDrag(state) {
  let { x, y } = state;
  const { tx, ty } = state;
  const followPointer = state.holding || state.releasePending;
  if (followPointer) {
    // 人物位置就是鼠标目标，不保留速度、弹簧或惯性状态。
    x = tx;
    y = ty;
  }
  const next = clampPetDragToBounds({
    ...state,
    x,
    y,
    tx,
    ty,
    vx: 0,
    vy: 0,
    lastTx: tx,
    lastTy: ty,
  });
  if (state.releasePending) {
    next.holding = false;
    next.releasePending = false;
    next.vx = 0;
    next.vy = 0;
    next.stopped = true;
  }
  return next;
}

export function clampPetDragToBounds(state, bounds = state.bounds) {
  let { x, y, tx, ty, vx, vy } = state;
  if (bounds) {
    const { l, t, r, b } = bounds;
    if (x < l) {
      x = l;
      tx = l;
      vx = 0;
    }
    if (x > r) {
      x = r;
      tx = r;
      vx = 0;
    }
    if (y < t) {
      y = t;
      ty = t;
      vy = 0;
    }
    if (y > b) {
      y = b;
      ty = b;
      vy = 0;
    }
  }

  return {
    ...state,
    x,
    y,
    tx,
    ty,
    vx,
    vy,
    lastTx: tx,
    lastTy: ty,
    stopped: !state.holding,
  };
}

const MIN_SCALE = 0.5;
const MAX_SCALE = 1.2;
const BASE_WIDTH = 240;
const BASE_HEIGHT = 330;

export function scaleFromResizeDrag(current, deltaX, deltaY) {
  const base = Number.isFinite(current) ? current : 1;
  const scaleDelta = (
    deltaX * BASE_WIDTH + deltaY * BASE_HEIGHT
  ) / (BASE_WIDTH * BASE_WIDTH + BASE_HEIGHT * BASE_HEIGHT);
  const next = base + scaleDelta;
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, Math.round(next * 100) / 100));
}

export function petScreenAnchorFromRect({ position, rect, scaleFactor }) {
  if (!position || !rect) return null;
  const x = Number(position && position.x);
  const y = Number(position && position.y);
  const left = Number(rect && rect.left);
  const top = Number(rect && rect.top);
  const dpr = Number(scaleFactor);
  if (![x, y, left, top, dpr].every(Number.isFinite) || dpr <= 0) return null;
  return {
    x: x + left * dpr,
    y: y + top * dpr,
  };
}

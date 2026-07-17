function requireWindowApi(T) {
  const api = T && T.window;
  if (!api || typeof api.getCurrentWindow !== 'function') {
    throw new Error('Tauri window.getCurrentWindow is unavailable');
  }
  if (typeof api.currentMonitor !== 'function') {
    throw new Error('Tauri window.currentMonitor is unavailable');
  }
  return api;
}

export async function readPetDragContext(T) {
  const api = requireWindowApi(T);
  const win = api.getCurrentWindow();
  if (!win || typeof win.outerPosition !== 'function' || typeof win.outerSize !== 'function') {
    throw new Error('Tauri current window geometry API is unavailable');
  }
  const monitorPromise = Promise.resolve()
    .then(() => api.currentMonitor())
    .catch(() => null);
  const monitorsPromise = typeof api.availableMonitors === 'function'
    ? Promise.resolve().then(() => api.availableMonitors()).catch(() => [])
    : Promise.resolve([]);
  const [position, size, monitor, available] = await Promise.all([
    win.outerPosition(),
    win.outerSize(),
    monitorPromise,
    monitorsPromise,
  ]);
  const monitors = Array.isArray(available) && available.length > 0
    ? available
    : (monitor ? [monitor] : []);
  return { win, position, size, monitor, monitors };
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
  characterWidth,
  characterHeight,
  horizontalPadding,
  verticalPadding,
}) {
  const windowWidth = Number(size && size.width);
  const windowHeight = Number(size && size.height);
  const width = Number(characterWidth);
  const height = Number(characterHeight);
  const horizontal = Number(horizontalPadding);
  const vertical = Number(verticalPadding);
  if (![windowWidth, windowHeight, width, height, horizontal, vertical].every(Number.isFinite)) return null;
  const left = alignment === 'left' ? horizontal : windowWidth - horizontal - width;
  const bottom = windowHeight - vertical;
  return { l: left, t: bottom - height, r: left + width, b: bottom };
}

export function petWindowBounds({ monitors, monitor, ...geometry }) {
  const desktop = connectedMonitorArea(monitors, monitor);
  const pet = petLocalRect(geometry);
  if (!desktop || !pet) return null;
  return {
    l: desktop.l - pet.l,
    t: desktop.t - pet.t,
    r: desktop.r - pet.r,
    b: desktop.b - pet.b,
  };
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
  if (next.x !== x) next.vx = state.holding ? 0 : -state.vx * BOUNCE;
  if (next.y !== y) next.vy = state.holding ? 0 : -state.vy * BOUNCE;
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
    base: state.base ? { ...state.base, x: shifted(state.base.x) } : state.base,
    x: shifted(state.x),
    tx: shifted(state.tx),
    lastTx: shifted(state.lastTx),
  };
}

const RELEASE_VELOCITY = 0.18;

export function attachPetDragGeometry(state, { position, size, monitor }) {
  const dpr = Number.isFinite(state.dpr) ? state.dpr : 1;
  const currentCX = Number.isFinite(state.currentCX) ? state.currentCX : state.startCX;
  const currentCY = Number.isFinite(state.currentCY) ? state.currentCY : state.startCY;
  const tx = position.x + (currentCX - state.startCX) * dpr;
  const ty = position.y + (currentCY - state.startCY) * dpr;
  const next = {
    ...state,
    base: position,
    x: position.x,
    y: position.y,
    tx,
    ty,
    lastTx: tx,
    lastTy: ty,
  };

  if (monitor) {
    next.bounds = {
      l: monitor.position.x,
      t: monitor.position.y,
      r: monitor.position.x + monitor.size.width - size.width,
      b: monitor.position.y + monitor.size.height - size.height,
    };
  }
  if (state.released) {
    next.vx += (tx - next.x) * RELEASE_VELOCITY;
    next.vy += (ty - next.y) * RELEASE_VELOCITY;
    next.holding = false;
  }
  return next;
}

export function releasePetDrag(state) {
  const next = { ...state, released: true };
  if (!state.base) return next;
  if (!(state.physicsSteps > 0)) {
    next.vx += (state.tx - state.x) * RELEASE_VELOCITY;
    next.vy += (state.ty - state.y) * RELEASE_VELOCITY;
  }
  next.holding = false;
  return next;
}

export function setPetWindowPosition(T, win, x, y) {
  try {
    const api = T && T.window;
    if (!api || typeof api.PhysicalPosition !== 'function') {
      throw new Error('Tauri window.PhysicalPosition is unavailable');
    }
    if (!win || typeof win.setPosition !== 'function') {
      throw new Error('Tauri window.setPosition is unavailable');
    }
    const position = new api.PhysicalPosition(Math.round(x), Math.round(y));
    return Promise.resolve(win.setPosition(position));
  } catch (error) {
    return Promise.reject(error);
  }
}

const SPRING = 0.14;
const HOLDING_DAMPING = 0.74;
const INERTIA_DAMPING = 0.955;
const BOUNCE = 0.55;
const STOP_SPEED = 0.4;
const MAX_TILT = 16;
const HOLDING_DIRECT_FOLLOW = 0.85;
const REVERSE_VELOCITY_RETENTION = 0.15;

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
  let { x, y, vx, vy } = state;
  if (state.holding) {
    const targetDx = Number.isFinite(state.lastTx) ? state.tx - state.lastTx : 0;
    const targetDy = Number.isFinite(state.lastTy) ? state.ty - state.lastTy : 0;
    if (targetDx && vx && Math.sign(targetDx) !== Math.sign(vx)) {
      vx *= REVERSE_VELOCITY_RETENTION;
    }
    if (targetDy && vy && Math.sign(targetDy) !== Math.sign(vy)) {
      vy *= REVERSE_VELOCITY_RETENTION;
    }
    x += targetDx * HOLDING_DIRECT_FOLLOW;
    y += targetDy * HOLDING_DIRECT_FOLLOW;
    vx += (state.tx - x) * SPRING;
    vy += (state.ty - y) * SPRING;
    vx *= HOLDING_DAMPING;
    vy *= HOLDING_DAMPING;
  } else {
    vx *= INERTIA_DAMPING;
    vy *= INERTIA_DAMPING;
  }

  x += vx;
  y += vy;
  if (state.bounds) {
    const { l, t, r, b } = state.bounds;
    if (x < l) { x = l; vx = state.holding ? 0 : Math.abs(vx) * BOUNCE; }
    if (x > r) { x = r; vx = state.holding ? 0 : -Math.abs(vx) * BOUNCE; }
    if (y < t) { y = t; vy = state.holding ? 0 : Math.abs(vy) * BOUNCE; }
    if (y > b) { y = b; vy = state.holding ? 0 : -Math.abs(vy) * BOUNCE; }
  }

  return {
    ...state,
    x,
    y,
    vx,
    vy,
    lastTx: state.tx,
    lastTy: state.ty,
    tilt: Math.max(-MAX_TILT, Math.min(MAX_TILT, vx * 0.5)),
    stopped: !state.holding && Math.abs(vx) < STOP_SPEED && Math.abs(vy) < STOP_SPEED,
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

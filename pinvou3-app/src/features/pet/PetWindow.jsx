import React, {
  useEffect,
  useLayoutEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
} from 'react';
import { createPetActivationState, loadActivePet } from './pet-active.js';
import { loadImage } from './load-image.js';
import {
  buildAnimationSequence,
  PET_FRAME_H,
  PET_FRAME_W,
} from './pet-animation.js';
import {
  createPetCardUiState,
  normalizedPetReply,
  petCardUiReducer,
} from './pet-card-state.js';
import {
  attachPetDragGeometry,
  clampPetDragToDesktop,
  dragAnimationFromMotion,
  petAlignmentAtDragEdge,
  petEdgeAlignment,
  petElementHorizontalBounds,
  petMonitorAtPosition,
  petScreenAnchorFromRect,
  petWindowBounds,
  readPetDragContext,
  rebasePetDragForAlignment,
  releasePetDrag,
  scaleFromResizeDrag,
  setPetWindowPosition,
  stepPetDrag,
} from './pet-interaction.js';
import {
  applyActivitySnapshot,
  applyEvent,
  createPetState,
  deriveActivities,
  deriveAnimation,
  markSessionViewed,
  removeSessionActivity,
} from './pet-state.js';
import {
  acknowledgeScheduledNotice,
  formatScheduledNoticeBody,
  isScheduledSessionPayload,
  readScheduledNoticeAcknowledgedAt,
  selectLatestScheduledNotice,
} from './pet-scheduled-notice.js';
import { renderPetMarkdown } from './pet-markdown.js';
import {
  DEFAULT_PET_ID,
  normalizePetId,
  resolvePet,
} from './pet-registry.js';
import { useReducedMotion } from '../../hooks/useReducedMotion.js';
import './pet.css';

const TICK_MS = 600;
const DEFAULT_SCALE = 0.5;
const MAX_SCALE = 1.2;
const FIRST_AWAKE_MS = 8_000;
const PET_EDGE_PADDING = 24;
const PET_BOTTOM_PADDING = 8;
const PET_ACTIVITY_WINDOW_HEIGHT = 260;
const PET_FRAME_WIDTH = PET_FRAME_W;
const PET_FRAME_HEIGHT = PET_FRAME_H;

const PET_EVENTS = [
  'pet:turn_start', 'pet:turn_end',
  'chat:delta', 'chat:tool_start', 'chat:tool_end',
  'chat:user_input_required', 'chat:done',
];

const STATUS_LABEL = {
  waiting: '需要输入',
  failed: '遇到问题',
  review: '可以查看',
  running: '处理中',
};

const STATUS_SYMBOL = {
  waiting: '',
  failed: '!',
  review: '✓',
  running: '',
};

/** Codex v2 player: per-frame timings, three active cycles, then slow idle. */
function PetSprite({ pet, animation }) {
  const reducedMotion = useReducedMotion();
  const sequence = useMemo(
    () => buildAnimationSequence(animation, { reducedMotion }),
    [animation, reducedMotion],
  );
  const [frameIndex, setFrameIndex] = useState(0);

  useEffect(() => setFrameIndex(0), [sequence]);
  useEffect(() => {
    if (reducedMotion || sequence.frames.length <= 1) return undefined;
    const frame = sequence.frames[frameIndex] || sequence.frames[0];
    const timer = window.setTimeout(() => {
      setFrameIndex((current) => (
        current + 1 < sequence.frames.length ? current + 1 : sequence.loopStartIndex
      ));
    }, frame.durationMs);
    return () => window.clearTimeout(timer);
  }, [frameIndex, reducedMotion, sequence]);

  const frame = sequence.frames[frameIndex] || sequence.frames[0];
  return (
    <div
      className="pet-sprite"
      style={{
        width: PET_FRAME_WIDTH,
        height: PET_FRAME_HEIGHT,
        backgroundImage: `url(${pet.sheetUrl})`,
        backgroundPosition: `-${frame.column * PET_FRAME_WIDTH}px -${frame.row * PET_FRAME_HEIGHT}px`,
      }}
    />
  );
}

function PetActivityBody({ text, expanded = false }) {
  const source = String(text || '');
  const className = expanded
    ? 'pet-activity-body pet-activity-body-expanded'
    : 'pet-activity-body';

  return (
    <div
      className={className}
      dangerouslySetInnerHTML={{ __html: renderPetMarkdown(source) }}
    />
  );
}

export default function PetWindow({ allowResize = true, configuredScale = null }) {
  const startupScale = Number.isFinite(configuredScale)
    ? Math.min(MAX_SCALE, Math.max(DEFAULT_SCALE, configuredScale))
    : DEFAULT_SCALE;
  const stateRef = useRef(createPetState());
  const [activePet, setActivePet] = useState(null);
  const [activationFailed, setActivationFailed] = useState(false);
  const petActivationRef = useRef(createPetActivationState());
  const [baseAnimation, setBaseAnimation] = useState('idle');
  const [dragAnimation, setDragAnimation] = useState(null);
  const [hovered, setHovered] = useState(false);
  const [firstAwake, setFirstAwake] = useState(true);
  const [activities, setActivities] = useState([]);
  const [scheduledNotice, setScheduledNotice] = useState(null);
  const [cardUi, dispatchCardUi] = useReducer(
    petCardUiReducer,
    undefined,
    createPetCardUiState,
  );
  const [scale, setScale] = useState(startupScale);
  const [edgeAlign, setEdgeAlign] = useState('right');
  const activityListRef = useRef(null);
  const characterSlotRef = useRef(null);
  const activityCardRectRef = useRef(null);
  const activityHeightRef = useRef(null);
  const openingSessionRef = useRef(null);
  const openingScheduledRunRef = useRef(null);
  const scheduledNoticeRef = useRef(null);
  scheduledNoticeRef.current = scheduledNotice;
  const scaleRef = useRef(startupScale);
  scaleRef.current = scale;
  const edgeAlignRef = useRef(edgeAlign);
  edgeAlignRef.current = edgeAlign;
  const alignmentGeometryRef = useRef(null);

  const activateSelectedPet = async (id, startup = false) => {
    const committed = await loadActivePet(id, {
      state: petActivationRef.current,
      startup,
      defaultPetId: DEFAULT_PET_ID,
      normalizeId: normalizePetId,
      resolvePet,
      loadAtlas: (pet) => pet.atlas(),
      decodeImage: loadImage,
      commit: setActivePet,
      onActivationFailed: setActivationFailed,
      onError: (error, context) => {
        const phase = context.fallback ? 'startup fallback' : (startup ? 'startup' : 'switch');
        console.error(`[pet atlas] ${phase} load failed for ${context.petId}`, error);
      },
    });
    return committed;
  };

  const updateEdgeAlignment = (geometry, initial = false) => {
    if (!geometry) return;
    alignmentGeometryRef.current = geometry;
    const dpr = window.devicePixelRatio || 1;
    const next = petEdgeAlignment({
      ...geometry,
      fallback: edgeAlignRef.current,
      currentAlignment: initial ? undefined : edgeAlignRef.current,
      characterWidth: PET_FRAME_WIDTH * scaleRef.current * dpr,
      horizontalPadding: PET_EDGE_PADDING * dpr,
    });
    if (next !== edgeAlignRef.current) {
      edgeAlignRef.current = next;
      setEdgeAlign(next);
    }
  };

  // 活动卡可被人物右上角的徽标手动收起。收起纯粹是 CSS 隐藏——窗口尺寸
  // 完全不动（不缩放就不可能闪、人物不可能移位），徽标改显示活动数量；
  // 新活动到来保持收起，仅数字增长。窗口大小仍只跟随"有没有内容"。
  const [cardsCollapsed, setCardsCollapsed] = useState(false);
  const cardsCollapsedRef = useRef(cardsCollapsed);
  cardsCollapsedRef.current = cardsCollapsed;
  const activityBadgeCount = activities.length + (scheduledNotice ? 1 : 0);
  const activityVisible = activities.length > 0
    || !!scheduledNotice
    || (firstAwake && !!activePet);
  const activityVisibleRef = useRef(activityVisible);
  activityVisibleRef.current = activityVisible;

  const measureActivityCard = () => {
    const card = activityListRef.current?.querySelector('.pet-activity');
    if (!card) {
      activityCardRectRef.current = null;
      return;
    }
    const rect = card.getBoundingClientRect();
    activityCardRectRef.current = rect.width > 0
      ? { left: rect.left, right: rect.right }
      : null;
  };

  const animation = dragAnimation
    || (hovered ? 'jumping' : (baseAnimation !== 'idle' ? baseAnimation : (firstAwake ? 'waving' : 'idle')));

  const refresh = () => {
    const now = Date.now();
    setActivities(deriveActivities(stateRef.current, now));
    setBaseAnimation(deriveAnimation(stateRef.current, now) || 'idle');
  };

  useEffect(() => {
    const core = window.__TAURI__ && window.__TAURI__.core;
    if (!core) return undefined;
    let disposed = false;
    const requestSequence = petActivationRef.current.requestSequence;
    core.invoke('get_selected_pet').then(async (id) => {
      if (disposed || petActivationRef.current.requestSequence !== requestSequence) return;
      const committed = await activateSelectedPet(id, true);
      // 启动回退（目标图集坏、落到 lingling）后把持久化收敛到实际显示的
      // 宠物——与切换路径的回滚协议一致，否则设置页与桌宠永久分叉且每次
      // 重启重试坏 ID。本地打包资源的加载失败几乎不会自愈，重试无意义。
      // 仅在成功读到请求 ID 且确实发生回退时写回；读取失败分支不写回，
      // 避免因一次读失败就覆盖用户的有效选择。
      if (
        !disposed
        && committed
        && !petActivationRef.current.pendingId
        && normalizePetId(id) !== committed.id
      ) {
        core.invoke('set_selected_pet', {
          id: committed.id,
          // CAS：仅当持久化仍是启动时读到的那个(加载失败的)ID 才收敛,
          // 期间用户的新选择不允许被这次过期写覆盖。
          expectedCurrent: normalizePetId(id),
        }).catch(() => {});
      }
    }).catch((error) => {
      if (disposed || petActivationRef.current.requestSequence !== requestSequence) return;
      console.error('[pet atlas] failed to read selected pet; loading fallback', error);
      void activateSelectedPet(DEFAULT_PET_ID, true);
    });
    return () => { disposed = true; };
  }, []);

  useEffect(() => {
    const timer = window.setTimeout(() => setFirstAwake(false), FIRST_AWAKE_MS);
    return () => window.clearTimeout(timer);
  }, []);

  // Global Engine events drive activities. The main window supplies titles and
  // current busy flags because the pet intentionally does not duplicate Session.
  useEffect(() => {
    const T = window.__TAURI__;
    const ev = T && T.event;
    const core = T && T.core;
    if (!ev) return undefined;
    let disposed = false;
    let noticeRequest = 0;
    let scheduledRefreshTimer = 0;
    const unlisteners = [];

    const refreshScheduledNotice = async () => {
      if (!core) return;
      const request = ++noticeRequest;
      try {
        const tasks = await core.invoke('list_scheduled_tasks');
        const unreadTasks = (Array.isArray(tasks) ? tasks : [])
          .filter((task) => task && task.hasUnreadRuns);
        const entries = await Promise.all(unreadTasks.map(async (task) => {
          const runs = await core.invoke('list_scheduled_task_runs', { id: task.id, limit: 20 });
          return [task.id, Array.isArray(runs) ? runs : []];
        }));
        if (disposed || request !== noticeRequest) return;
        const next = selectLatestScheduledNotice(
          unreadTasks,
          Object.fromEntries(entries),
          readScheduledNoticeAcknowledgedAt(),
        );
        scheduledNoticeRef.current = next;
        setScheduledNotice(next);
      } catch (_) {
        // The task page remains the source of truth; a transient read failure
        // must not remove an already visible completion reminder.
      }
    };

    const scheduleNoticeRefresh = (delay = 0) => {
      window.clearTimeout(scheduledRefreshTimer);
      scheduledRefreshTimer = window.setTimeout(refreshScheduledNotice, delay);
    };

    const subscriptions = PET_EVENTS.map((name) => ev.listen(name, (event) => {
      if (isScheduledSessionPayload(event.payload)) {
        const status = String((event.payload && event.payload.status) || '').toLowerCase();
        if (name === 'chat:done' && status === 'completed' && !event.payload?.error) {
          scheduleNoticeRefresh(300);
        }
        return;
      }
      if (applyEvent(stateRef.current, name, event.payload, Date.now())) refresh();
    }));
    subscriptions.push(ev.listen('pet:selected_changed', async (event) => {
      const requested = event.payload && event.payload.selected_pet;
      const before = petActivationRef.current.activePet;
      const after = await activateSelectedPet(requested);
      // 激活失败（图集加载/解码不成，仍停留在旧宠）时，把已持久化的选择
      // 回滚到实际显示的宠物——否则设置页与桌宠外观会永久分叉，且重启
      // 会反复重试坏 ID。pendingId 非空说明有更新的请求在跑，此时不回滚。
      if (
        core
        && before
        && after === before
        && !petActivationRef.current.pendingId
        && normalizePetId(requested) !== before.id
      ) {
        core.invoke('set_selected_pet', {
          id: before.id,
          // CAS：仅当持久化仍是刚刚激活失败的目标 ID 才回滚。
          expectedCurrent: normalizePetId(requested),
        }).catch(() => {});
      }
    }));
    subscriptions.push(ev.listen('scheduled_task:run_updated', (event) => {
      const payload = event.payload || {};
      const run = payload.run || payload;
      if (String(run.status || '').toLowerCase() === 'completed') scheduleNoticeRefresh();
    }));
    subscriptions.push(ev.listen('pet:scheduled_notice_opened', (event) => {
      const payload = event.payload || {};
      const runId = String(payload.run_id || payload.runId || '');
      const current = scheduledNoticeRef.current;
      if (current && (!runId || current.runId === runId)) {
        acknowledgeScheduledNotice(current);
        scheduledNoticeRef.current = null;
        setScheduledNotice(null);
      }
      openingScheduledRunRef.current = null;
    }));
    subscriptions.push(ev.listen('pet:scheduled_notice_open_failed', () => {
      openingScheduledRunRef.current = null;
    }));
    subscriptions.push(ev.listen('pet:activity_snapshot', (event) => {
      const sessions = event.payload && event.payload.sessions;
      const chatSessions = (Array.isArray(sessions) ? sessions : [])
        .filter((session) => !isScheduledSessionPayload(session));
      applyActivitySnapshot(
        stateRef.current,
        chatSessions,
        event.payload && event.payload.sequence,
        Date.now(),
      );
      refresh();
    }));
    subscriptions.push(ev.listen('pet:session_viewed', (event) => {
      const sid = event.payload && (event.payload.session_id || event.payload.sessionId);
      if (openingSessionRef.current === String(sid || '')) openingSessionRef.current = null;
      dispatchCardUi({ type: 'dismiss', sessionId: String(sid || '') });
      if (markSessionViewed(stateRef.current, sid)) refresh();
    }));
    subscriptions.push(ev.listen('pet:session_unavailable', (event) => {
      const sid = event.payload && (event.payload.session_id || event.payload.sessionId);
      if (openingSessionRef.current === String(sid || '')) openingSessionRef.current = null;
      dispatchCardUi({ type: 'dismiss', sessionId: String(sid || '') });
      if (removeSessionActivity(stateRef.current, sid)) refresh();
    }));
    subscriptions.push(ev.listen('pet:reply_accepted', (event) => {
      const payload = event.payload || {};
      dispatchCardUi({
        type: 'reply-accepted',
        requestId: payload.request_id || payload.requestId,
      });
    }));
    subscriptions.push(ev.listen('pet:reply_failed', (event) => {
      const payload = event.payload || {};
      const sid = payload.session_id || payload.sessionId;
      dispatchCardUi({
        type: 'reply-failed',
        requestId: payload.request_id || payload.requestId,
        error: payload.error || '发送失败',
      });
      if (payload.unavailable && removeSessionActivity(stateRef.current, sid)) {
        dispatchCardUi({ type: 'dismiss', sessionId: String(sid || '') });
        refresh();
      }
    }));

    Promise.all(subscriptions).then((items) => {
      if (disposed) items.forEach((unlisten) => unlisten());
      else {
        unlisteners.push(...items);
        ev.emit('pet:request_snapshot').catch(() => {});
        refreshScheduledNotice();
      }
    }).catch(() => {});

    const timer = window.setInterval(refresh, TICK_MS);
    return () => {
      disposed = true;
      window.clearInterval(timer);
      window.clearTimeout(scheduledRefreshTimer);
      unlisteners.forEach((unlisten) => { try { unlisten(); } catch (_) {} });
    };
  }, []);

  useLayoutEffect(() => {
    const core = window.__TAURI__ && window.__TAURI__.core;
    if (!core) return undefined;
    // 人物锚点由 Rust 从当前窗口几何自行反推（前端此刻的 DOM 已经因卡片
    // 卸载而漂移，测量结果不可信），这里只需带上人物贴边方向。
    const invokeActivityVisible = (visible, activityHeight) => {
      core.invoke('set_pet_activity_visible', {
        visible,
        activityHeight,
        alignment: edgeAlignRef.current,
      }).catch(() => {});
    };
    const list = activityListRef.current;
    if (!activityVisible || !list) {
      activityCardRectRef.current = null;
      activityHeightRef.current = null;
      invokeActivityVisible(false, null);
      return undefined;
    }

    activityHeightRef.current = PET_ACTIVITY_WINDOW_HEIGHT;
    measureActivityCard();
    invokeActivityVisible(true, PET_ACTIVITY_WINDOW_HEIGHT);
    return undefined;
  }, [activityVisible]);

  useEffect(() => {
    const T = window.__TAURI__;
    if (!T || !T.window || !T.core) return undefined;
    const scaleRequest = Number.isFinite(configuredScale)
      ? T.core.invoke('set_pet_scale', {
        scale: startupScale,
        activityVisible: activityVisibleRef.current,
        activityHeight: activityHeightRef.current,
      })
      : T.core.invoke('get_pet_scale');
    scaleRequest.then((value) => {
      if (value > 0) setScale(value);
    }).catch(() => {});
    const win = T.window.getCurrentWindow();
    let saveTimer = 0;
    const unlisteners = [];
    readPetDragContext(T).then((geometry) => updateEdgeAlignment(geometry, true)).catch(() => {});
    win.onMoved(({ payload }) => {
      if (alignmentGeometryRef.current) {
        alignmentGeometryRef.current = { ...alignmentGeometryRef.current, position: payload };
      }
      window.clearTimeout(saveTimer);
      saveTimer = window.setTimeout(() => {
        T.core.invoke('save_pet_position', { x: payload.x, y: payload.y }).catch(() => {});
      }, 500);
    }).then((fn) => { unlisteners.push(fn); });
    win.onResized(({ payload }) => {
      if (alignmentGeometryRef.current) {
        alignmentGeometryRef.current = { ...alignmentGeometryRef.current, size: payload };
      }
      if (dragRef.current) dragRef.current.windowSize = payload;
    }).then((fn) => { unlisteners.push(fn); });
    return () => {
      window.clearTimeout(saveTimer);
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, []);

  const openMain = (sessionId = null) => {
    const core = window.__TAURI__ && window.__TAURI__.core;
    if (!core) return;
    const sid = String(sessionId || '').trim();
    if (sid && openingSessionRef.current === sid) return;
    if (sid) openingSessionRef.current = sid;
    core.invoke('open_main_from_pet', { sessionId: sid || null }).catch((error) => {
      if (openingSessionRef.current === sid) openingSessionRef.current = null;
      console.error('[pet navigation] failed', error);
    });
  };

  const openScheduledNotice = (event) => {
    event.stopPropagation();
    const core = window.__TAURI__ && window.__TAURI__.core;
    if (!core || !scheduledNotice) return;
    if (openingScheduledRunRef.current === scheduledNotice.runId) return;
    openingScheduledRunRef.current = scheduledNotice.runId;
    core.invoke('open_main_from_pet', {
      sessionId: null,
      scheduledRun: scheduledNotice,
    }).catch((error) => {
      openingScheduledRunRef.current = null;
      console.error('[pet scheduled navigation] failed', error);
    });
  };

  const dismissScheduledNotice = (event) => {
    event.preventDefault();
    event.stopPropagation();
    if (!scheduledNotice) return;
    acknowledgeScheduledNotice(scheduledNotice);
    scheduledNoticeRef.current = null;
    setScheduledNotice(null);
  };

  // Elastic drag: the window follows a spring, then continues with inertia and
  // edge bounce. Horizontal drag alone switches to the directional run rows.
  const tiltRef = useRef(null);
  const dragRef = useRef(null);
  const physRafRef = useRef(0);

  const stopPhysics = (expected = null) => {
    if (expected && dragRef.current !== expected) return;
    dragRef.current = null;
    cancelAnimationFrame(physRafRef.current);
    physRafRef.current = 0;
    if (tiltRef.current) tiltRef.current.style.transform = '';
  };

  const stepPhysics = () => {
    const drag = dragRef.current;
    if (!drag || !drag.base || !drag.win) return;
    const currentAlignment = edgeAlignRef.current;
    const monitorScale = Number(drag.pointerScale) || Number(drag.dpr) || 1;
    const metrics = {
      size: drag.windowSize,
      alignment: currentAlignment,
      characterWidth: PET_FRAME_WIDTH * scaleRef.current * monitorScale,
      characterHeight: PET_FRAME_HEIGHT * scaleRef.current * monitorScale,
      horizontalPadding: PET_EDGE_PADDING * monitorScale,
      verticalPadding: PET_BOTTOM_PADDING * monitorScale,
    };
    const activeMonitor = petMonitorAtPosition({
      position: { x: drag.x, y: drag.y },
      monitors: drag.monitors,
      ...metrics,
    });
    if (activeMonitor && activeMonitor !== drag.monitor) {
      drag.monitor = activeMonitor;
      const nextScale = Number(activeMonitor.scaleFactor);
      if (Number.isFinite(nextScale) && nextScale > 0) {
        drag.pointerScale = nextScale;
        drag.dpr = nextScale;
      }
    }
    drag.bounds = petWindowBounds({
      monitors: drag.monitors,
      monitor: drag.monitor,
      ...metrics,
    });
    Object.assign(drag, stepPetDrag(drag));
    if (drag.windowSize && drag.bounds) {
      const cardRect = activityVisibleRef.current
        && !cardsCollapsedRef.current
        && activityCardRectRef.current;
      const cardBounds = cardRect && petElementHorizontalBounds({
        monitors: drag.monitors,
        monitor: drag.monitor,
        localLeft: cardRect.left * monitorScale,
        localRight: cardRect.right * monitorScale,
      });
      const nextAlignment = petAlignmentAtDragEdge({
        currentAlignment,
        x: drag.x,
        tx: drag.tx,
        holding: drag.holding,
        bounds: cardBounds || drag.bounds,
      });
      if (nextAlignment !== currentAlignment) {
        Object.assign(drag, rebasePetDragForAlignment(drag, {
          from: currentAlignment,
          to: nextAlignment,
          windowWidth: drag.windowSize.width,
          characterWidth: metrics.characterWidth,
          horizontalPadding: metrics.horizontalPadding,
        }));
        edgeAlignRef.current = nextAlignment;
        setEdgeAlign(nextAlignment);
        window.requestAnimationFrame(measureActivityCard);
        drag.bounds = petWindowBounds({
          monitors: drag.monitors,
          monitor: drag.monitor,
          ...metrics,
          alignment: nextAlignment,
        });
      }
      // 对齐 rebase 会横向平移 x，所以空洞判定必须排在它后面、且在下游读取
      // drag.x/y 之前——否则 alignmentGeometryRef 会记到钳制前的旧坐标。
      Object.assign(drag, clampPetDragToDesktop(drag, {
        monitors: drag.monitors,
        ...metrics,
        alignment: edgeAlignRef.current,
      }));
      alignmentGeometryRef.current = {
        position: { x: drag.x, y: drag.y },
        size: drag.windowSize,
        monitor: drag.monitor,
      };
    }
    drag.physicsSteps = (drag.physicsSteps || 0) + 1;
    const T = window.__TAURI__;
    setPetWindowPosition(T, drag.win, drag.x, drag.y).catch((error) => {
      console.error('[pet drag] setPosition failed', error);
      stopPhysics(drag);
    });
    if (tiltRef.current) tiltRef.current.style.transform = `rotate(${drag.tilt}deg)`;
    if (drag.stopped) {
      stopPhysics(drag);
      return;
    }
    physRafRef.current = requestAnimationFrame(stepPhysics);
  };
  useEffect(() => () => stopPhysics(), []);

  const pressRef = useRef(null);
  const onPointerDown = (event) => {
    if (event.button !== 0) return;
    const core = window.__TAURI__ && window.__TAURI__.core;
    if (core) core.invoke('hide_pet_context_menu').catch(() => {});
    measureActivityCard();
    event.currentTarget.setPointerCapture(event.pointerId);
    pressRef.current = {
      x: event.screenX,
      y: event.screenY,
      lastX: event.screenX,
      lastY: event.screenY,
      moved: false,
    };
    const dpr = window.devicePixelRatio || 1;
    const previous = dragRef.current;
    const drag = {
      win: null,
      holding: true,
      startCX: event.screenX,
      startCY: event.screenY,
      currentCX: event.screenX,
      currentCY: event.screenY,
      dpr,
      released: false,
      physicsSteps: 0,
      base: null,
      x: 0,
      y: 0,
      tx: 0,
      ty: 0,
      vx: previous ? previous.vx : 0,
      vy: previous ? previous.vy : 0,
      bounds: null,
    };
    dragRef.current = drag;
    const T = window.__TAURI__;
    if (!T || !T.window) return;
    readPetDragContext(T)
      .then(({ win, position, size, monitor, monitors }) => {
        if (dragRef.current !== drag) return;
        drag.win = win;
        drag.windowSize = size;
        drag.monitor = monitor;
        drag.monitors = monitors;
        const monitorScale = Number(monitor && monitor.scaleFactor);
        if (Number.isFinite(monitorScale) && monitorScale > 0) {
          drag.pointerScale = monitorScale;
          drag.dpr = monitorScale;
        }
        Object.assign(drag, attachPetDragGeometry(drag, { position, size, monitor: null }));
        cancelAnimationFrame(physRafRef.current);
        physRafRef.current = requestAnimationFrame(stepPhysics);
      })
      .catch((error) => {
        console.error('[pet drag] read context failed', error);
        stopPhysics(drag);
      });
  };

  const onPointerMove = (event) => {
    const press = pressRef.current;
    const drag = dragRef.current;
    let motionX = 0;
    let motionY = 0;
    if (press) {
      const dx = event.screenX - press.x;
      const dy = event.screenY - press.y;
      if (Math.abs(dx) + Math.abs(dy) > 4) press.moved = true;
      motionX = event.screenX - press.lastX;
      motionY = event.screenY - press.lastY;
      setDragAnimation((current) => dragAnimationFromMotion(current, motionX, motionY));
      press.lastX = event.screenX;
      press.lastY = event.screenY;
    }
    if (drag && drag.holding) {
      drag.currentCX = event.screenX;
      drag.currentCY = event.screenY;
      if (drag.base) {
        const pointerScale = Number(drag.pointerScale) || Number(drag.dpr) || 1;
        drag.tx += motionX * pointerScale;
        drag.ty += motionY * pointerScale;
      }
    }
  };

  const finishPointer = (cancelled = false) => {
    const press = pressRef.current;
    pressRef.current = null;
    setDragAnimation(null);
    if (dragRef.current) Object.assign(dragRef.current, releasePetDrag(dragRef.current));
    if (!cancelled && press && !press.moved) openMain(null);
  };

  const resizeRef = useRef(null);
  const flushResizeScale = async (drag) => {
    if (drag.sending) return;
    const core = window.__TAURI__ && window.__TAURI__.core;
    if (!core) return;
    drag.sending = true;
    while (drag.pendingScale != null) {
      const pending = drag.pendingScale;
      const next = pending.scale;
      drag.pendingScale = null;
      try {
        const hasCharacterAnchor = Number.isFinite(drag.anchorX)
          && Number.isFinite(drag.anchorY);
        const actual = await core.invoke('set_pet_scale', {
          scale: next,
          anchor: hasCharacterAnchor ? 'character_top_left' : 'top_left',
          alignment: drag.alignment,
          anchorX: hasCharacterAnchor ? drag.anchorX : null,
          anchorY: hasCharacterAnchor ? drag.anchorY : null,
          activityVisible: activityVisibleRef.current,
          activityHeight: activityHeightRef.current,
          persist: pending.persist,
        });
        if (drag.pendingScale == null && resizeRef.current === drag && actual > 0) {
          scaleRef.current = actual;
          setScale(actual);
        }
      } catch (_) {
        drag.pendingScale = null;
        break;
      }
    }
    drag.sending = false;
    if (drag.ended && resizeRef.current === drag) resizeRef.current = null;
  };

  const queueResizeScale = (drag, next, persist) => {
    drag.pendingScale = { scale: next, persist };
    void flushResizeScale(drag);
  };

  const applyResizePointer = (drag, persist) => {
    if (!drag.ready) return;
    const next = scaleFromResizeDrag(
      drag.startScale,
      drag.latestX - drag.startX,
      drag.latestY - drag.startY,
    );
    if (next !== drag.currentScale) {
      drag.currentScale = next;
      scaleRef.current = next;
      setScale(next);
      queueResizeScale(drag, next, false);
    }
    if (persist) queueResizeScale(drag, drag.currentScale, true);
  };

  const onResizePointerDown = (event) => {
    event.preventDefault();
    event.stopPropagation();
    if (event.button !== 0) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    const drag = {
      pointerId: event.pointerId,
      startX: event.screenX,
      startY: event.screenY,
      latestX: event.screenX,
      latestY: event.screenY,
      startScale: scaleRef.current,
      currentScale: scaleRef.current,
      alignment: edgeAlignRef.current,
      anchorX: null,
      anchorY: null,
      ready: false,
      pendingScale: null,
      sending: false,
      ended: false,
    };
    resizeRef.current = drag;

    const T = window.__TAURI__;
    const rect = characterSlotRef.current?.getBoundingClientRect();
    Promise.resolve()
      .then(() => T?.window?.getCurrentWindow()?.outerPosition())
      .then((position) => petScreenAnchorFromRect({
        position,
        rect,
        scaleFactor: window.devicePixelRatio || 1,
      }))
      .catch(() => null)
      .then((anchor) => {
        if (resizeRef.current !== drag) return;
        drag.anchorX = anchor?.x ?? null;
        drag.anchorY = anchor?.y ?? null;
        drag.ready = true;
        applyResizePointer(drag, drag.ended);
      });
  };

  const onResizePointerMove = (event) => {
    const drag = resizeRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    event.preventDefault();
    event.stopPropagation();
    drag.latestX = event.screenX;
    drag.latestY = event.screenY;
    applyResizePointer(drag, false);
  };

  const onResizePointerUp = (event) => {
    const drag = resizeRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    event.preventDefault();
    event.stopPropagation();
    drag.latestX = event.screenX;
    drag.latestY = event.screenY;
    drag.ended = true;
    applyResizePointer(drag, true);
  };

  // 根节点只负责压掉 WebView 默认右键菜单（卡片/透明区不该冒出"检查/刷新"）；
  // 公仔菜单只在人物本体上触发，透明边距和活动卡不再误开。
  const suppressContextMenu = (event) => {
    event.preventDefault();
  };
  const onCharacterContextMenu = (event) => {
    event.preventDefault();
    event.stopPropagation();
    const core = window.__TAURI__ && window.__TAURI__.core;
    if (core) {
      core.invoke('show_pet_context_menu', {
        anchorX: event.clientX,
        anchorY: event.clientY,
      }).catch(() => {});
    }
  };

  const dismissActivity = (event, sessionId) => {
    event.preventDefault();
    event.stopPropagation();
    removeSessionActivity(stateRef.current, sessionId);
    setActivities(deriveActivities(stateRef.current));
    dispatchCardUi({ type: 'dismiss', sessionId });
  };

  const submitPetReply = async (activity) => {
    const text = normalizedPetReply(cardUi.draft);
    if (!text || cardUi.pendingRequestId) return;
    const requestId = `${Date.now()}-${Math.random().toString(36).slice(2)}`;
    dispatchCardUi({ type: 'submit-reply', requestId });
    const core = window.__TAURI__ && window.__TAURI__.core;
    if (!core) {
      dispatchCardUi({ type: 'reply-failed', requestId, error: '无法连接主窗口' });
      return;
    }
    try {
      await core.invoke('queue_pet_reply', {
        requestId,
        sessionId: activity.sessionId,
        text,
      });
    } catch (error) {
      dispatchCardUi({
        type: 'reply-failed',
        requestId,
        error: String(error && error.message ? error.message : error),
      });
    }
  };

  const resizeReplyInput = (element) => {
    element.style.height = '0';
    element.style.height = `${Math.min(element.scrollHeight, 52)}px`;
  };

  return (
    <div
      className={`pet-root pet-align-${edgeAlign}`}
      style={{ '--pet-character-width': `${PET_FRAME_WIDTH * scale}px` }}
      onContextMenu={suppressContextMenu}
    >
      {activityVisible && (
        <div
          ref={activityListRef}
          className={`pet-activities ${activityBadgeCount > 1 ? 'pet-activities-tray' : ''}${cardsCollapsed ? ' pet-activities--collapsed' : ''}`}
        >
          {scheduledNotice && (
            <div className="pet-activity-shell">
              <div
                className="pet-activity pet-activity-scheduled"
                onPointerDown={(event) => event.stopPropagation()}
              >
                <button
                  type="button"
                  className="pet-activity-open"
                  aria-label={`打开定时任务${scheduledNotice.taskName}的本次运行`}
                  onClick={openScheduledNotice}
                />
                <div className="pet-activity-main">
                  <div className="pet-activity-title-row">
                    <span className="pet-activity-title">定时任务已完成</span>
                    <span className="pet-scheduled-status" aria-label="已完成">✓</span>
                  </div>
                  <div className="pet-activity-body-row">
                    <span className="pet-activity-body">
                      {formatScheduledNoticeBody(scheduledNotice)}
                    </span>
                  </div>
                </div>
                <button
                  type="button"
                  className="pet-activity-close"
                  aria-label="关闭定时任务完成提醒"
                  onClick={dismissScheduledNotice}
                >
                  ×
                </button>
              </div>
            </div>
          )}
          {activities.length > 0 ? activities.slice(0, 6).map((activity) => {
            const expanded = cardUi.expandedSessionId === activity.sessionId;
            const replying = cardUi.replySessionId === activity.sessionId;
            const pending = replying && !!cardUi.pendingRequestId;
            return (
              <div key={activity.sessionId} className="pet-activity-shell">
                <div
                  className={`pet-activity pet-activity-${activity.status} ${expanded ? 'is-expanded' : ''}`}
                  onPointerDown={(event) => event.stopPropagation()}
                >
                  <button
                    type="button"
                    className="pet-activity-open"
                    aria-label={`打开${activity.title}对话`}
                    onClick={(event) => {
                      event.stopPropagation();
                      openMain(activity.sessionId);
                    }}
                  />
                  <div className="pet-activity-main">
                    <div className="pet-activity-title-row">
                      <span className="pet-activity-title">{activity.title}</span>
                      <span
                        className="pet-activity-status"
                        aria-label={STATUS_LABEL[activity.status]}
                      >
                        {STATUS_SYMBOL[activity.status]}
                      </span>
                      <button
                        type="button"
                        className="pet-activity-expand"
                        aria-label={expanded ? '收起回复' : '展开回复'}
                        aria-expanded={expanded}
                        data-hint={expanded ? '收起' : '展开'}
                        onClick={(event) => {
                          event.preventDefault();
                          event.stopPropagation();
                          dispatchCardUi({ type: 'toggle-expand', sessionId: activity.sessionId });
                        }}
                      >
                        <span className="pet-activity-expand-icon" aria-hidden="true">›</span>
                      </button>
                    </div>
                    <div className="pet-activity-body-row">
                      <PetActivityBody
                        text={activity.body}
                        expanded={expanded}
                      />
                    </div>
                  </div>
                  <button
                    type="button"
                    className="pet-activity-close"
                    aria-label={`关闭${activity.title}提醒`}
                    onClick={(event) => dismissActivity(event, activity.sessionId)}
                  >
                    ×
                  </button>
                  <button
                    type="button"
                    className="pet-activity-reply"
                    onClick={(event) => {
                      event.preventDefault();
                      event.stopPropagation();
                      dispatchCardUi({ type: 'open-reply', sessionId: activity.sessionId });
                    }}
                  >
                    回复
                  </button>
                </div>
                {replying && (
                  <form
                    className="pet-reply-composer"
                    onPointerDown={(event) => event.stopPropagation()}
                    onSubmit={(event) => {
                      event.preventDefault();
                      event.stopPropagation();
                      void submitPetReply(activity);
                    }}
                  >
                    <textarea
                      autoFocus
                      rows={1}
                      value={cardUi.draft}
                      disabled={pending}
                      aria-label={`回复${activity.title}`}
                      placeholder="输入回复…"
                      onChange={(event) => dispatchCardUi({
                        type: 'edit-reply',
                        text: event.target.value,
                      })}
                      onInput={(event) => resizeReplyInput(event.currentTarget)}
                      onKeyDown={(event) => {
                        event.stopPropagation();
                        if (event.key === 'Escape') {
                          event.preventDefault();
                          dispatchCardUi({ type: 'close-reply' });
                        } else if (event.key === 'Enter' && !event.shiftKey
                          && !event.nativeEvent.isComposing) {
                          event.preventDefault();
                          void submitPetReply(activity);
                        }
                      }}
                    />
                    <button
                      type="submit"
                      aria-label="发送回复"
                      disabled={pending || !normalizedPetReply(cardUi.draft)}
                    >
                      {pending ? '…' : (
                        <svg
                          className="pet-reply-send-icon"
                          viewBox="0 0 16 16"
                          aria-hidden="true"
                          focusable="false"
                        >
                          <path d="M8 13V3M8 3 3.75 7.25M8 3l4.25 4.25" />
                        </svg>
                      )}
                    </button>
                    {cardUi.error && <span className="pet-reply-error">{cardUi.error}</span>}
                  </form>
                )}
              </div>
            );
          }) : (!scheduledNotice && activePet && (
            <div className="pet-activity-shell">
              <div className="pet-activity pet-activity-awake">
                <button
                  type="button"
                  className="pet-activity-open"
                  aria-label="回到品悟"
                  onPointerDown={(event) => event.stopPropagation()}
                  onClick={(event) => { event.stopPropagation(); openMain(null); }}
                />
                <div className="pet-activity-main">
                  <div className="pet-activity-title-row">
                    <span className="pet-activity-title">{activePet.name}已就绪</span>
                  </div>
                  <div className="pet-activity-body-row">
                    <span className="pet-activity-body">点击回到品悟</span>
                  </div>
                </div>
              </div>
            </div>
          ))}
        </div>
      )}
      <div
        ref={characterSlotRef}
        className="pet-character-slot"
        style={{ width: PET_FRAME_WIDTH * scale, height: PET_FRAME_HEIGHT * scale }}
      >
        {activePet && (
          <div className="pet-stage" style={{ transform: `translateX(-50%) scale(${scale})` }}>
            <div
              className="pet-character"
              role="button"
              tabIndex={0}
              aria-label={`点击回到品悟，拖动${activePet.name}`}
              onContextMenu={onCharacterContextMenu}
              onPointerEnter={() => setHovered(true)}
              onPointerLeave={() => setHovered(false)}
              onPointerDown={onPointerDown}
              onPointerMove={onPointerMove}
              onPointerUp={() => finishPointer(false)}
              onPointerCancel={() => finishPointer(true)}
              onKeyDown={(event) => {
                if (event.key === 'Enter' || event.key === ' ') openMain(null);
              }}
            >
              <div className="pet-tilt" ref={tiltRef}>
                <PetSprite pet={activePet} animation={animation} />
              </div>
            </div>
          </div>
        )}
        {activePet && activityBadgeCount > 0 && (
          <button
            type="button"
            className={`pet-collapse-badge${cardsCollapsed ? ' pet-collapse-badge--count' : ''}`}
            aria-label={cardsCollapsed ? `展开 ${activityBadgeCount} 条活动` : '收起活动卡片'}
            title={cardsCollapsed ? '展开活动' : '收起活动'}
            onPointerDown={(event) => event.stopPropagation()}
            onClick={(event) => {
              event.stopPropagation();
              setCardsCollapsed((value) => !value);
            }}
          >
            {cardsCollapsed
              ? (activityBadgeCount > 99 ? '99+' : activityBadgeCount)
              : (
                <svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true" focusable="false">
                  <path d="M2.5 6h7" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" fill="none" />
                </svg>
              )}
          </button>
        )}
        {!activePet && activationFailed && (
          <button
            type="button"
            className="pet-activation-fallback"
            data-pet-activation-failed="true"
            aria-label="公仔加载失败，重试"
            onClick={() => { void activateSelectedPet(DEFAULT_PET_ID, true); }}
          >
            <span className="pet-activation-fallback-icon" aria-hidden="true">!</span>
            <span className="pet-activation-fallback-title">公仔加载失败</span>
            <span className="pet-activation-fallback-action">点击重试</span>
          </button>
        )}
        {allowResize && (
          <div
            className="pet-resize-grip"
            role="separator"
            aria-label="拖动调整公仔大小"
            title="拖动调整大小"
            onPointerDown={onResizePointerDown}
            onPointerMove={onResizePointerMove}
            onPointerUp={onResizePointerUp}
            onPointerCancel={onResizePointerUp}
          >
            <svg
              className="pet-resize-grip-icon"
              viewBox="0 0 16 16"
              aria-hidden="true"
              focusable="false"
            >
              <path d="M2 14H14V2" />
            </svg>
          </div>
        )}
      </div>
    </div>
  );
}

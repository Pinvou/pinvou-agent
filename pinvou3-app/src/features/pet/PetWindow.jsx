import React, {
  useEffect,
  useLayoutEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
} from 'react';
import { createPetActivationState, loadActivePet } from './pet-active.js';
import { isImeComposing } from '../../shared/ime-guard.mjs';
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
  clampPetDragToBounds,
  clampPetDragToDesktop,
  dragAnimationFromMotion,
  petAlignmentAtDragEdge,
  petEdgeAlignment,
  petElementHorizontalBounds,
  petMonitorAtPosition,
  petScreenAnchorFromRect,
  petVerticalAlignmentAtDragEdge,
  petClientOriginVerticalBounds,
  petConnectedClientOriginVerticalBounds,
  petWindowBounds,
  readPetDragContext,
  rebasePetDragForAlignment,
  rebasePetDragForVerticalAlignment,
  releasePetDrag,
  scaleFromResizeDrag,
  setPetWindowPosition,
  stepPetDrag,
} from './pet-interaction.js';
import {
  getCurrentTauriWindow,
  invokeTauri,
  isTauriAvailable,
  tauriCommands,
  tauriEvents,
} from '../../platform/tauri/client.js';
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
import { dict, initialSystemLanguage, TAG_TO_LANG } from '../../shared/i18n.js';
import './pet.css';

const TICK_MS = 600;
const DEFAULT_SCALE = 0.5;
const MAX_SCALE = 1.2;
const FIRST_AWAKE_MS = 8_000;
const PET_EDGE_PADDING = 24;
const PET_BOTTOM_PADDING = 8;
const PET_ACTIVITY_WINDOW_HEIGHT = 260;
// workArea 已给出无装饰窗口的真实 client 边界，只留 1px 消除整数取整抖动。
const PET_VERTICAL_FLIP_MARGIN = 1;
const PET_FRAME_WIDTH = PET_FRAME_W;
const PET_FRAME_HEIGHT = PET_FRAME_H;

const PET_EVENTS = [
  'pet:turn_start', 'pet:turn_end',
  'chat:delta', 'chat:tool_start', 'chat:tool_end',
  'chat:user_input_required', 'chat:done',
];

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

export default function PetWindow({
  allowResize = true,
  configuredScale = null,
  configuredVerticalAlignment = 'bottom',
}) {
  const startupScale = Number.isFinite(configuredScale)
    ? Math.min(MAX_SCALE, Math.max(DEFAULT_SCALE, configuredScale))
    : DEFAULT_SCALE;
  const stateRef = useRef(createPetState());
  const [language, setLanguage] = useState(initialSystemLanguage);
  const t = dict[language] || dict.zh;
  const petCopy = t.uiPet;
  // pet.html 的 <title>/<html lang> 是静态中文,按当前语言同步一次。
  useEffect(() => {
    const misc = t.uiPlatformMisc;
    if (!misc) return;
    document.title = misc.petTitle;
    if (misc.htmlLang) document.documentElement.lang = misc.htmlLang;
  }, [t]);
  const [activePet, setActivePet] = useState(null);
  const [activationFailed, setActivationFailed] = useState(false);
  const petActivationRef = useRef(createPetActivationState());
  const [baseAnimation, setBaseAnimation] = useState('idle');
  const [dragAnimation, setDragAnimation] = useState(null);
  const [hovered, setHovered] = useState(false);
  // 右键菜单改为窗口内 DOM 浮层(不再另起透明 webview:GB10/WebKitGTK 下
  // 新起第二个透明窗口会触发 malloc 堆损坏闪退,且被 GTK 钳到 200x200)。
  const [ctxMenu, setCtxMenu] = useState(null);
  const ctxMenuRef = useRef(null);
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
  const [edgeVAlign, setEdgeVAlign] = useState(
    configuredVerticalAlignment === 'top' ? 'top' : 'bottom',
  );
  const petRootRef = useRef(null);
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
  const edgeVAlignRef = useRef(edgeVAlign);
  edgeVAlignRef.current = edgeVAlign;
  const verticalAlignmentSaveTimerRef = useRef(0);

  const activateSelectedPet = async (id, startup = false) => {
    const committed = await loadActivePet(id, {
      state: petActivationRef.current,
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
      ? {
        left: rect.left,
        right: rect.right,
        top: rect.top,
        bottom: rect.bottom,
      }
      : null;
  };

  const animation = dragAnimation
    || (hovered ? 'jumping' : (baseAnimation !== 'idle' ? baseAnimation : (firstAwake ? 'waving' : 'idle')));
  const activePetName = activePet
    ? (t.uiPetSettings.pets[activePet.id]?.name || activePet.name)
    : '';

  useEffect(() => {
    if (!isTauriAvailable()) return undefined;
    let disposed = false;
    let unlisten = null;
    invokeTauri('get_settings').then((settings) => {
      if (!disposed) setLanguage(TAG_TO_LANG[settings?.language] || initialSystemLanguage());
    }).catch(() => {});
    tauriEvents.listen('ui:language_changed', (event) => {
      const next = event.payload?.language;
      if (!disposed && dict[next]) setLanguage(next);
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    }).catch(() => {});
    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, []);

  const refresh = () => {
    const now = Date.now();
    setActivities(deriveActivities(stateRef.current, now, petCopy));
    setBaseAnimation(deriveAnimation(stateRef.current, now) || 'idle');
  };

  useEffect(() => {
    const core = isTauriAvailable() ? tauriCommands : null;
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
    const ev = isTauriAvailable() ? tauriEvents : null;
    const core = isTauriAvailable() ? tauriCommands : null;
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
      if (applyEvent(stateRef.current, name, event.payload, Date.now(), petCopy)) refresh();
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
        petCopy,
      );
      refresh();
    }));
    subscriptions.push(ev.listen('pet:session_viewed', (event) => {
      const sid = event.payload && (event.payload.session_id || event.payload.sessionId);
      if (openingSessionRef.current === String(sid || '')) openingSessionRef.current = null;
      dispatchCardUi({ type: 'dismiss', sessionId: String(sid || '') });
      if (markSessionViewed(stateRef.current, sid, {
        completed: event.payload?.completed === true,
      })) refresh();
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
        error: payload.error || petCopy.sendFailed,
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
  }, [petCopy.sendFailed]);

  useLayoutEffect(() => {
    const core = isTauriAvailable() ? tauriCommands : null;
    if (!core) return undefined;
    // 人物锚点由 Rust 从当前窗口几何自行反推（前端此刻的 DOM 已经因卡片
    // 卸载而漂移，测量结果不可信），这里只需带上人物贴边方向。
    const invokeActivityVisible = (visible, activityHeight) => {
      core.invoke('set_pet_activity_visible', {
        visible,
        activityHeight,
        alignment: edgeAlignRef.current,
        verticalAlignment: edgeVAlignRef.current,
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
    if (!isTauriAvailable()) return undefined;
    const scaleRequest = Number.isFinite(configuredScale)
      ? invokeTauri('set_pet_scale', {
        scale: startupScale,
        activityVisible: activityVisibleRef.current,
        activityHeight: activityHeightRef.current,
        verticalAlignment: edgeVAlignRef.current,
      })
      : invokeTauri('get_pet_scale');
    scaleRequest.then((value) => {
      if (value > 0) setScale(value);
    }).catch(() => {});
    const win = getCurrentTauriWindow();
    let saveTimer = 0;
    let disposed = false;
    const unlisteners = [];
    readPetDragContext().then((geometry) => updateEdgeAlignment(geometry, true)).catch(() => {});
    // Linux/TAO 的 onMoved payload 来自 WM frame origin，而 setPosition 在 GTK
    // 实际按 client origin 移动。事件只当作“发生了移动”的通知；稳定后重新读
    // innerPosition，保证运行模型、持久化和下次恢复始终处于同一坐标域。
    win.onMoved(() => {
      window.clearTimeout(saveTimer);
      saveTimer = window.setTimeout(() => {
        Promise.resolve(win.innerPosition()).then((position) => {
          if (disposed) return;
          return invokeTauri('save_pet_position', {
            x: position.x,
            y: position.y,
            verticalAlignment: edgeVAlignRef.current,
          });
        }).catch(() => {});
      }, 500);
    }).then((fn) => { unlisteners.push(fn); });
    win.onResized(({ payload }) => {
      if (dragRef.current) {
        const activeDrag = dragRef.current;
        activeDrag.windowSize = payload;
        const resizeSyncToken = {};
        activeDrag.resizeSyncToken = resizeSyncToken;
        const positionQueue = positionQueueRef.current;
        if (positionQueue.requested && positionQueue.requested.drag === activeDrag) {
          positionQueue.requested = null;
        }
        const queuedMoveSettled = waitForPetPositionWrites();
        window.requestAnimationFrame(() => {
          measureActivityCard();
          queuedMoveSettled
            .then(() => win.innerPosition())
            .then((position) => {
              if (dragRef.current !== activeDrag
                || activeDrag.resizeSyncToken !== resizeSyncToken) return;
              if (activeDrag.geometryReady) {
                const dx = position.x - activeDrag.x;
                const dy = position.y - activeDrag.y;
                const shifted = (value, delta) => (
                  Number.isFinite(value) ? value + delta : value
                );
                activeDrag.x = position.x;
                activeDrag.y = position.y;
                activeDrag.tx = shifted(activeDrag.tx, dx);
                activeDrag.ty = shifted(activeDrag.ty, dy);
                activeDrag.lastTx = shifted(activeDrag.lastTx, dx);
                activeDrag.lastTy = shifted(activeDrag.lastTy, dy);
              }
              activeDrag.localRect = measurePetLocalRect(activeDrag);
              activeDrag.resizeSyncToken = null;
            })
            .catch(() => {
              if (dragRef.current === activeDrag
                && activeDrag.resizeSyncToken === resizeSyncToken) {
                activeDrag.resizeSyncToken = null;
              }
            });
        });
      } else {
        window.requestAnimationFrame(measureActivityCard);
      }
    }).then((fn) => { unlisteners.push(fn); });
    return () => {
      disposed = true;
      window.clearTimeout(saveTimer);
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, []);

  const openMain = (sessionId = null) => {
    const core = isTauriAvailable() ? tauriCommands : null;
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
    const core = isTauriAvailable() ? tauriCommands : null;
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

  // 按住时窗口严格跟手；松手消费尾帧后立即停住。
  const dragRef = useRef(null);
  const physRafRef = useRef(0);
  const positionQueueRef = useRef({ requested: null, inFlight: new Set() });

  const stopPhysics = (expected = null) => {
    if (expected && dragRef.current !== expected) return;
    dragRef.current = null;
    cancelAnimationFrame(physRafRef.current);
    physRafRef.current = 0;
  };

  const waitForPetPositionWrites = () => {
    const queue = positionQueueRef.current;
    const writes = [...queue.inFlight];
    return writes.length > 0 ? Promise.allSettled(writes) : Promise.resolve();
  };

  const queuePetWindowPosition = (drag) => {
    const queue = positionQueueRef.current;
    const x = Math.round(drag.x);
    const y = Math.round(drag.y);
    if (queue.requested
      && queue.requested.drag === drag
      && queue.requested.x === x
      && queue.requested.y === y) return;
    const job = { drag, x, y };
    queue.requested = job;
    // rAF 已经把同一帧的鼠标事件合并成最新坐标。这里必须立即提交，不能等待
    // 上一笔 GTK setPosition Promise；串行等待会让原生窗口阶梯式追赶鼠标，
    // 体感等同于重新加入弹簧。inFlight 只用于 resize/新拖拽前的读取屏障。
    const write = setPetWindowPosition(drag.win, x, y);
    queue.inFlight.add(write);
    void write
      .catch((error) => {
        if (queue.requested === job) queue.requested = null;
        console.error('[pet drag] setPosition failed', error);
        if (dragRef.current === drag) stopPhysics(drag);
      })
      .finally(() => {
        queue.inFlight.delete(write);
      });
  };

  const stepPhysics = () => {
    const drag = dragRef.current;
    if (!drag || !drag.geometryReady || !drag.win) return;
    if (drag.resizeSyncToken) {
      physRafRef.current = requestAnimationFrame(stepPhysics);
      return;
    }
    const currentAlignment = edgeAlignRef.current;
    const currentVAlign = edgeVAlignRef.current;
    let monitorScale = Number(drag.pointerScale) || 1;
    const measuredLocalRect = drag.localRect || measurePetLocalRect(drag);
    if (measuredLocalRect) drag.localRect = measuredLocalRect;
    const metrics = {
      size: drag.windowSize,
      alignment: currentAlignment,
      verticalAlignment: currentVAlign,
      viewportHeight: Number(drag.windowSize && drag.windowSize.height)
        || window.innerHeight * monitorScale,
      characterWidth: PET_FRAME_WIDTH * scaleRef.current * monitorScale,
      characterHeight: PET_FRAME_HEIGHT * scaleRef.current * monitorScale,
      horizontalPadding: PET_EDGE_PADDING * monitorScale,
      verticalPadding: PET_BOTTOM_PADDING * monitorScale,
      // 竖向 bounds 用起手实测的人物矩形(物理像素),规避 X11 outerSize 回读不准。
      localTop: measuredLocalRect ? measuredLocalRect.t : undefined,
      localBottom: measuredLocalRect ? measuredLocalRect.b : undefined,
    };
    // 必须先消费本帧最新目标，再用人物的新位置选择活动屏。否则“跨屏+松手”
    // 同帧发生时会按旧屏计算一次并立即停止，再也没有下一帧纠正边界/DPI。
    // 暂不钳制原始 overshoot，让后面的横/竖布局翻转先消费它。
    Object.assign(drag, stepPetDrag({ ...drag, bounds: null }));
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
        monitorScale = nextScale;
        drag.localRect = measurePetLocalRect(drag);
        metrics.viewportHeight = Number(drag.windowSize && drag.windowSize.height)
          || window.innerHeight * nextScale;
        metrics.characterWidth = PET_FRAME_WIDTH * scaleRef.current * nextScale;
        metrics.characterHeight = PET_FRAME_HEIGHT * scaleRef.current * nextScale;
        metrics.horizontalPadding = PET_EDGE_PADDING * nextScale;
        metrics.verticalPadding = PET_BOTTOM_PADDING * nextScale;
        metrics.localTop = drag.localRect ? drag.localRect.t : undefined;
        metrics.localBottom = drag.localRect ? drag.localRect.b : undefined;
      }
    }
    const windowVBounds = petClientOriginVerticalBounds({
      monitor: drag.monitor,
      viewportHeight: metrics.viewportHeight,
    });
    const desktopWindowVBounds = petConnectedClientOriginVerticalBounds({
      monitors: drag.monitors,
      monitor: drag.monitor,
      viewportHeight: metrics.viewportHeight,
    });
    const transitionBounds = petWindowBounds({
      monitors: drag.monitors,
      monitor: drag.monitor,
      ...metrics,
    });
    // 最终布局确定后才做 WM 可达范围钳制，否则一次大步事件会永久丢掉
    // 鼠标抓点偏移。
    if (drag.windowSize && transitionBounds) {
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
        bounds: cardBounds || transitionBounds,
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
      }
      // 宠物窗首次 map 前已保持无装饰，setPosition / innerPosition 统一使用
      // client origin；可达顶边就是工作区顶边，触边当帧翻转并 rebase 人物锚点。
      const nextVAlign = petVerticalAlignmentAtDragEdge({
        currentAlignment: currentVAlign,
        y: drag.y,
        ty: drag.ty,
        holding: drag.holding,
        bounds: windowVBounds || transitionBounds,
        threshold: PET_VERTICAL_FLIP_MARGIN * monitorScale,
      });
      if (nextVAlign !== currentVAlign) {
        const previousLocalRect = measurePetLocalRect(drag) || measuredLocalRect;
        edgeVAlignRef.current = nextVAlign;
        // 原生窗口坐标会在本物理帧内按新布局 rebase。React state 的 className
        // 更新若晚一帧，人物会先随窗口跳走、下一帧才回到锚点，肉眼就是一次
        // 卡顿/闪跳。先同步改 DOM class，确保本帧提交给合成器的布局与坐标一致；
        // state 随后接管声明式状态。
        if (petRootRef.current) {
          petRootRef.current.classList.remove(`pet-valign-${currentVAlign}`);
          petRootRef.current.classList.add(`pet-valign-${nextVAlign}`);
        }
        // class 已同步切换，此处只在翻转帧强制测一次新人物矩形。用真实 localTop
        // 差值 rebase，既守住鼠标抓点，也避免重新依赖 X11 不可靠的 outerSize。
        const nextLocalRect = petRootRef.current ? measurePetLocalRect(drag) : null;
        Object.assign(drag, rebasePetDragForVerticalAlignment(drag, {
          from: currentVAlign,
          to: nextVAlign,
          viewportHeight: metrics.viewportHeight,
          characterHeight: metrics.characterHeight,
          verticalPadding: metrics.verticalPadding,
          previousLocalTop: previousLocalRect ? previousLocalRect.t : undefined,
          nextLocalTop: nextLocalRect ? nextLocalRect.t : undefined,
        }));
        drag.localRect = nextLocalRect;
        metrics.verticalAlignment = nextVAlign;
        metrics.localTop = nextLocalRect ? nextLocalRect.t : undefined;
        metrics.localBottom = nextLocalRect ? nextLocalRect.b : undefined;
        setEdgeVAlign(nextVAlign);
        const core = isTauriAvailable() ? tauriCommands : null;
        if (core) {
          // 落盘不在翻转关键帧做；快速往返时只保存最终方向。
          window.clearTimeout(verticalAlignmentSaveTimerRef.current);
          verticalAlignmentSaveTimerRef.current = window.setTimeout(() => {
            core.invoke('save_pet_vertical_alignment', {
              alignment: edgeVAlignRef.current,
            }).catch(() => {});
          }, 250);
        }
        window.requestAnimationFrame(measureActivityCard);
      }
      drag.bounds = petWindowBounds({
        monitors: drag.monitors,
        monitor: drag.monitor,
        clientOriginVerticalBounds: desktopWindowVBounds,
        ...metrics,
        alignment: edgeAlignRef.current,
        verticalAlignment: edgeVAlignRef.current,
      });
      Object.assign(drag, clampPetDragToBounds(drag));
      // 对齐 rebase 会平移模型原点，空洞判定必须排在它后面。
      Object.assign(drag, clampPetDragToDesktop(drag, {
        monitors: drag.monitors,
        ...metrics,
        alignment: edgeAlignRef.current,
        verticalAlignment: edgeVAlignRef.current,
      }));
    }
    queuePetWindowPosition(drag);
    if (drag.stopped) {
      stopPhysics(drag);
      return;
    }
    physRafRef.current = requestAnimationFrame(stepPhysics);
  };
  useEffect(() => () => {
    stopPhysics();
    positionQueueRef.current.requested = null;
    window.clearTimeout(verticalAlignmentSaveTimerRef.current);
  }, []);

  // 起手时实测人物槽在窗口内的真实矩形(物理像素):X11 下 outerSize 回读不准,
  // 用它反推的人物底边偏高会让拖拽起手人物上跳。人物在窗口内的位置在一次拖拽
  // 中不变,测一次缓存即可,不必每帧测(避免强制重排)。
  const measurePetLocalRect = (drag) => {
    const el = characterSlotRef.current;
    if (!el) return null;
    const rect = el.getBoundingClientRect();
    if (!(rect.width > 0) || !(rect.height > 0)) return null;
    const sf = Number(drag && drag.pointerScale) || window.devicePixelRatio || 1;
    return {
      t: rect.top * sf,
      b: rect.bottom * sf,
    };
  };

  // 纯点击不启动拖拽物理，避免无位移操作触发一次无意义的窗口定位。
  const beginPetPhysics = (drag) => {
    if (!drag || drag.physicsRunning || !drag.geometryReady) return;
    drag.physicsRunning = true;
    drag.localRect = measurePetLocalRect(drag);
    cancelAnimationFrame(physRafRef.current);
    physRafRef.current = requestAnimationFrame(stepPhysics);
  };

  const pressRef = useRef(null);
  const onPointerDown = (event) => {
    if (event.button !== 0) return;
    // 右键菜单现为窗口内 DOM 浮层,开启时由捕获阶段 pointerdown 监听收起,
    // 无需再对已废弃的原生菜单窗口发 hide IPC。
    measureActivityCard();
    event.currentTarget.setPointerCapture(event.pointerId);
    pressRef.current = {
      x: event.screenX,
      y: event.screenY,
      lastX: event.screenX,
      lastY: event.screenY,
      moved: false,
    };
    const pointerScale = window.devicePixelRatio || 1;
    const previous = dragRef.current;
    const drag = {
      win: null,
      holding: true,
      startCX: event.screenX,
      startCY: event.screenY,
      currentCX: event.screenX,
      currentCY: event.screenY,
      pointerScale,
      didMove: false,
      geometryReady: false,
      x: 0,
      y: 0,
      tx: 0,
      ty: 0,
      vx: 0,
      vy: 0,
      bounds: null,
    };
    dragRef.current = drag;
    // 立刻停掉上一次拖拽遗留的动画帧,避免它作用到这次的新 drag 上。
    cancelAnimationFrame(physRafRef.current);
    physRafRef.current = 0;
    const positionQueue = positionQueueRef.current;
    positionQueue.requested = null;
    const previousPositionSettled = waitForPetPositionWrites();
    if (!isTauriAvailable()) {
      stopPhysics(drag);
      return;
    }
    previousPositionSettled.then(() => readPetDragContext())
      .then(({ win, position, size, monitor, monitors }) => {
        if (dragRef.current !== drag) return;
        drag.win = win;
        drag.windowSize = size;
        drag.monitor = monitor;
        drag.monitors = monitors;
        const monitorScale = Number(monitor && monitor.scaleFactor);
        if (Number.isFinite(monitorScale) && monitorScale > 0) {
          drag.pointerScale = monitorScale;
        }
        Object.assign(drag, attachPetDragGeometry(drag, {
          position,
        }));
        // 若按下后已经移动过(真拖动),几何就绪即启动物理;纯点击则不启动。
        if (drag.didMove) beginPetPhysics(drag);
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
      if (Math.abs(dx) + Math.abs(dy) > 4) {
        press.moved = true;
        if (drag) drag.didMove = true;
      }
      motionX = event.screenX - press.lastX;
      motionY = event.screenY - press.lastY;
      setDragAnimation((current) => dragAnimationFromMotion(current, motionX, motionY));
      press.lastX = event.screenX;
      press.lastY = event.screenY;
    }
    if (drag && drag.holding) {
      drag.currentCX = event.screenX;
      drag.currentCY = event.screenY;
      if (drag.geometryReady) {
        const pointerScale = Number(drag.pointerScale) || 1;
        drag.tx += motionX * pointerScale;
        drag.ty += motionY * pointerScale;
      }
    }
    // 越过拖动阈值才启动物理循环(纯点击不启动,避免点击触发 bounds 钳制上移)。
    if (press && press.moved) beginPetPhysics(drag);
  };

  const finishPointer = (cancelled = false) => {
    const press = pressRef.current;
    pressRef.current = null;
    setDragAnimation(null);
    const drag = dragRef.current;
    const isClick = !cancelled && press && !press.moved;
    if (cancelled || !press || !press.moved) {
      stopPhysics(drag);
      if (isClick) openMain(null);
      return;
    }
    if (drag) Object.assign(drag, releasePetDrag(drag));
  };

  const resizeRef = useRef(null);
  const flushResizeScale = async (drag) => {
    if (drag.sending) return;
    const core = isTauriAvailable() ? tauriCommands : null;
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
          verticalAlignment: edgeVAlignRef.current,
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

    const rect = characterSlotRef.current?.getBoundingClientRect();
    Promise.resolve()
      .then(() => getCurrentTauriWindow().innerPosition())
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
    // pet-root 铺满视口(100vw/100vh),clientX/Y 即窗口内局部坐标。菜单钳在
    // 窗口内(pet-root overflow:hidden 会裁掉溢出),避免半截跑到窗口外。
    const MENU_W = 88;
    const MENU_H = 32;
    const margin = 8;
    const x = Math.max(margin, Math.min(event.clientX, window.innerWidth - MENU_W - margin));
    const y = Math.max(margin, Math.min(event.clientY, window.innerHeight - MENU_H - margin));
    setCtxMenu({ x, y });
  };

  const hidePet = async () => {
    const core = isTauriAvailable() ? tauriCommands : null;
    try {
      if (core) await core.invoke('set_pet_enabled', { enabled: false });
    } finally {
      setCtxMenu(null);
    }
  };

  // 菜单开启时:窗口外(菜单以外)点击 / Esc / 失焦都收起。捕获阶段监听,
  // 命中菜单内部则不收,与旧独立菜单窗口的 blur/透明区收起语义一致。
  useEffect(() => {
    if (!ctxMenu) return undefined;
    const close = () => setCtxMenu(null);
    const onPointerDownAway = (event) => {
      if (ctxMenuRef.current && ctxMenuRef.current.contains(event.target)) return;
      close();
    };
    const onKeyDown = (event) => {
      if (event.key === 'Escape') close();
    };
    window.addEventListener('pointerdown', onPointerDownAway, true);
    window.addEventListener('keydown', onKeyDown);
    window.addEventListener('blur', close);
    return () => {
      window.removeEventListener('pointerdown', onPointerDownAway, true);
      window.removeEventListener('keydown', onKeyDown);
      window.removeEventListener('blur', close);
    };
  }, [ctxMenu]);

  const dismissActivity = (event, sessionId) => {
    event.preventDefault();
    event.stopPropagation();
    removeSessionActivity(stateRef.current, sessionId);
    setActivities(deriveActivities(stateRef.current, Date.now(), petCopy));
    dispatchCardUi({ type: 'dismiss', sessionId });
  };

  const submitPetReply = async (activity) => {
    const text = normalizedPetReply(cardUi.draft);
    if (!text || cardUi.pendingRequestId) return;
    const requestId = `${Date.now()}-${Math.random().toString(36).slice(2)}`;
    dispatchCardUi({ type: 'submit-reply', requestId });
    const core = isTauriAvailable() ? tauriCommands : null;
    if (!core) {
      dispatchCardUi({ type: 'reply-failed', requestId, error: petCopy.noMain });
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
        error: String(error && error.message ? error.message : error) || petCopy.sendFailed,
      });
    }
  };

  const resizeReplyInput = (element) => {
    element.style.height = '0';
    element.style.height = `${Math.min(element.scrollHeight, 52)}px`;
  };

  return (
    <div
      ref={petRootRef}
      className={`pet-root pet-align-${edgeAlign} pet-valign-${edgeVAlign}`}
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
                  aria-label={petCopy.openScheduled(scheduledNotice.taskName)}
                  onClick={openScheduledNotice}
                />
                <div className="pet-activity-main">
                  <div className="pet-activity-title-row">
                    <span className="pet-activity-title">{petCopy.scheduledDone}</span>
                    <span className="pet-scheduled-status" aria-label={petCopy.done}>✓</span>
                  </div>
                  <div className="pet-activity-body-row">
                    <span className="pet-activity-body">
                      {formatScheduledNoticeBody(scheduledNotice, t.langTag, petCopy.done)}
                    </span>
                  </div>
                </div>
                <button
                  type="button"
                  className="pet-activity-close"
                  aria-label={petCopy.closeScheduled}
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
                    aria-label={petCopy.openChat(activity.title)}
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
                        aria-label={petCopy[activity.status]}
                      >
                        {STATUS_SYMBOL[activity.status]}
                      </span>
                      <button
                        type="button"
                        className="pet-activity-expand"
                        aria-label={expanded ? petCopy.collapseReply : petCopy.expandReply}
                        aria-expanded={expanded}
                        data-hint={expanded ? petCopy.collapse : petCopy.expand}
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
                    aria-label={petCopy.closeNotice(activity.title)}
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
                    {petCopy.reply}
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
                      aria-label={petCopy.replyTo(activity.title)}
                      placeholder={petCopy.replyPlaceholder}
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
                          && !isImeComposing(event)) {
                          event.preventDefault();
                          void submitPetReply(activity);
                        }
                      }}
                    />
                    <button
                      type="submit"
                      aria-label={petCopy.sendReply}
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
                  aria-label={petCopy.back}
                  onPointerDown={(event) => event.stopPropagation()}
                  onClick={(event) => { event.stopPropagation(); openMain(null); }}
                />
                <div className="pet-activity-main">
                  <div className="pet-activity-title-row">
                    <span className="pet-activity-title">{petCopy.ready(activePetName)}</span>
                  </div>
                  <div className="pet-activity-body-row">
                    <span className="pet-activity-body">{petCopy.backHint}</span>
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
              aria-label={petCopy.drag(activePetName)}
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
              <PetSprite pet={activePet} animation={animation} />
            </div>
          </div>
        )}
        {activePet && activityBadgeCount > 0 && (
          <button
            type="button"
            className={`pet-collapse-badge${cardsCollapsed ? ' pet-collapse-badge--count' : ''}`}
            aria-label={cardsCollapsed ? petCopy.expandActivities(activityBadgeCount) : petCopy.collapseActivities}
            title={cardsCollapsed ? petCopy.expandActivity : petCopy.collapseActivity}
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
            aria-label={petCopy.loadFailedRetry(petCopy.loadFailed, petCopy.retry)}
            onClick={() => { void activateSelectedPet(DEFAULT_PET_ID, true); }}
          >
            <span className="pet-activation-fallback-icon" aria-hidden="true">!</span>
            <span className="pet-activation-fallback-title">{petCopy.loadFailed}</span>
            <span className="pet-activation-fallback-action">{petCopy.retry}</span>
          </button>
        )}
        {allowResize && (
          <div
            className="pet-resize-grip"
            role="separator"
            aria-label={petCopy.resize}
            title={petCopy.resizeTitle}
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
      {ctxMenu && (
        <div
          ref={ctxMenuRef}
          className="pet-context-menu"
          style={{ left: ctxMenu.x, top: ctxMenu.y }}
          onContextMenu={(event) => event.preventDefault()}
          onPointerDown={(event) => event.stopPropagation()}
        >
          <button type="button" className="pet-context-menu-item" onClick={hidePet}>
            {petCopy.hide}
          </button>
        </div>
      )}
    </div>
  );
}

import { useEffect, useMemo, useRef, useState } from 'react';
import { AlertTriangle, Check } from '../../components/icons.jsx';
import {
  buildPreviewSequence,
  PET_ATLAS_COLS,
  PET_FRAME_H,
  PET_FRAME_W,
} from './pet-animation.js';
import { loadImage } from './load-image.js';
import { PET_REGISTRY, normalizePetId } from './pet-registry.js';
import { useReducedMotion } from '../../hooks/useReducedMotion.js';
import './pet-settings.css';

const PET_IDS = Object.keys(PET_REGISTRY);

// 预览雪碧图跟随设置页紧凑卡尺寸展示；几何全部由协议常量推导，
// 不再与 pet-animation.js 双写。背景纵向 auto 以兼容 9/11 行图集。
const PREVIEW_SCALE = 0.3;
const PREVIEW_FRAME_W = PET_FRAME_W * PREVIEW_SCALE;
const PREVIEW_FRAME_H = PET_FRAME_H * PREVIEW_SCALE;

/** 悬停预览：挥手、跳跃、观察后回慢速 idle（序列由 pet-animation 提供，宠物无关）。 */
function PreviewSprite({ atlasUrl }) {
  const sequence = useMemo(() => buildPreviewSequence(), []);
  const [frameIndex, setFrameIndex] = useState(0);

  // eslint-disable-next-line react-hooks/set-state-in-effect -- 切换图集时同步重置预览帧,一次性镜像
  useEffect(() => setFrameIndex(0), [atlasUrl]);
  useEffect(() => {
    const frame = sequence.frames[frameIndex] || sequence.frames[0];
    const timer = window.setTimeout(() => {
      setFrameIndex((current) => (
        current + 1 < sequence.frames.length ? current + 1 : sequence.loopStartIndex
      ));
    }, frame.durationMs);
    return () => window.clearTimeout(timer);
  }, [frameIndex, sequence]);

  const frame = sequence.frames[frameIndex] || sequence.frames[0];
  return (
    <div
      className="pet-card-sprite"
      style={{
        width: PREVIEW_FRAME_W,
        height: PREVIEW_FRAME_H,
        backgroundImage: `url(${atlasUrl})`,
        backgroundSize: `${PET_ATLAS_COLS * PREVIEW_FRAME_W}px auto`,
        backgroundPosition: `-${frame.column * PREVIEW_FRAME_W}px -${frame.row * PREVIEW_FRAME_H}px`,
      }}
    />
  );
}

/**
 * 设置页「桌宠」卡内部：启用后直接展示三张可选宠物卡。
 * 选中状态以 bridge 的 selectedPetId 为唯一真相——本组件不做乐观更新，
 * 切换失败时 UI 自然停留在旧宠物上。
 */
export default function PetSettingsSection({ enabled, selectedPetId, t, onSelect }) {
  const reducedMotion = useReducedMotion();
  const [assets, setAssets] = useState({});
  const [hoveredId, setHoveredId] = useState(null);
  const aliveRef = useRef(true);
  useEffect(() => {
    // Fast Refresh 会先执行旧 effect 的 cleanup，再复用组件状态；必须在新
    // effect 挂载时恢复标志，否则后续封面/图集加载会被误判为卸载后的回调。
    aliveRef.current = true;
    return () => { aliveRef.current = false; };
  }, []);

  const currentId = normalizePetId(selectedPetId);

  // 封面与图集分两级进状态机：封面是轻量资源，先到先显示——图集 decode
  // 期间卡片必须一直露出封面（需求硬性要求），而不是空白占位。
  const loadPetCover = (id) => {
    setAssets((state) => ({
      ...state,
      [id]: { ...state[id], coverFailed: false },
    }));
    PET_REGISTRY[id].cover()
      .then(loadImage)
      .then((cover) => {
        if (!aliveRef.current) return;
        setAssets((state) => ({ ...state, [id]: { ...state[id], cover } }));
      })
      .catch(() => {
        if (!aliveRef.current) return;
        setAssets((state) => ({ ...state, [id]: { ...state[id], coverFailed: true } }));
      });
  };

  const loadPetAssets = (id) => {
    // 与在途加载去重:重试可重复点击,封面或图集任一在途时不再重复发请求。
    const current = assets[id];
    if (current && (current.status === 'loading' || current.atlasStatus === 'loading')) return;
    setAssets((state) => ({
      ...state,
      [id]: { ...state[id], status: 'loading', coverFailed: false, atlasStatus: 'loading' },
    }));
    const entry = PET_REGISTRY[id];
    entry.cover()
      .then(loadImage)
      .then((cover) => {
        if (!aliveRef.current) return;
        setAssets((state) => ({ ...state, [id]: { ...state[id], cover } }));
      })
      .catch(() => {
        if (!aliveRef.current) return;
        setAssets((state) => ({ ...state, [id]: { ...state[id], coverFailed: true } }));
      });
    entry.atlas()
      .then(loadImage)
      .then((atlas) => {
        if (!aliveRef.current) return;
        setAssets((state) => ({ ...state, [id]: { ...state[id], atlas, atlasStatus: 'ready', status: 'ready' } }));
        // 重试路径与 ensurePetAtlas 同一收口语义:加载期间用户已点击该卡
        // (重试在途时点击只能排队,ensurePetAtlas 因 atlasStatus=loading 早退、
        // 无人发起回调),完成即执行排队的选择,不丢点击。
        const pending = pendingSelectRef.current;
        if (pending === id) {
          pendingSelectRef.current = null;
          Promise.resolve(onSelect(id)).catch((error) => {
            console.error('[pet-selector] switch failed, keeping previous pet', error);
          });
        }
      })
      .catch(() => {
        if (!aliveRef.current) return;
        // 失败腿同样消费排队:排队的选择等不到就绪,丢弃并让用户看到失败态,
        // 不残留误触发(之后 hover 不会再无点击切换)。
        if (pendingSelectRef.current === id) pendingSelectRef.current = null;
        setAssets((state) => ({ ...state, [id]: { ...state[id], status: 'error', atlasStatus: 'error' } }));
      });
  };

  // 两级懒加载:封面(轻)随区域出现即载;图集(单张 1.7-2.2MB)只在 hover/聚焦/
  // 选中该卡时才拉取——原实现进设置页就把 3 只宠全量预载 ≈5.9MB。atlasStatus
  // 独立于封面状态,hover 预览与选中切换都要求 atlas ready。
  useEffect(() => {
    if (!enabled) return;
    PET_IDS.forEach((id) => {
      if (!assets[id]) loadPetCover(id);
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps -- 只在区域出现时按需补载缺失封面;依赖 assets 会造成加载-入库-再触发的循环
  }, [enabled]);

  // 当前选中宠的图集随区域出现预载(它随时会被桌宠窗口使用);其余宠 hover 才载。
  // 不把 entry 纳入依赖:图集失败会写新 entry,重跑会变成失败-重试死循环;
  // 主路径靠 ensurePetAtlas 在 !entry 时也照常发起(见下)来保证生效。
  useEffect(() => {
    // eslint-disable-next-line react-hooks/immutability -- ensurePetAtlas 声明在 effect 之后,仅为预载入口;运行时调用时已初始化
    if (enabled && currentId) ensurePetAtlas(currentId);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- 纳入 ensurePetAtlas/assets 会因图集失败写入而重触发
  }, [enabled, currentId]);

  const pendingSelectRef = useRef(null);
  const ensurePetAtlas = (id) => {
    const entry = assets[id];
    // entry 不存在时也要照常发起:预载 effect 首次运行时封面尚未入库,
    // 在此早退会让「当前宠图集随区域出现预载」在主路径上从不生效。
    if (entry && (entry.atlas || entry.atlasStatus === 'loading')) return;
    setAssets((state) => ({ ...state, [id]: { ...state[id], atlasStatus: 'loading' } }));
    PET_REGISTRY[id].atlas()
      .then(loadImage)
      .then((atlas) => {
        if (!aliveRef.current) return;
        setAssets((state) => ({ ...state, [id]: { ...state[id], atlas, atlasStatus: 'ready', status: 'ready' } }));
        // 加载期间用户已点击该卡:完成即执行排队的选择。
        const pending = pendingSelectRef.current;
        if (pending === id) {
          pendingSelectRef.current = null;
          Promise.resolve(onSelect(id)).catch((error) => {
            console.error('[pet-selector] switch failed, keeping previous pet', error);
          });
        }
      })
      .catch(() => {
        if (!aliveRef.current) return;
        if (pendingSelectRef.current === id) pendingSelectRef.current = null;
        setAssets((state) => ({ ...state, [id]: { ...state[id], atlasStatus: 'error' } }));
      });
  };

  const handleSelect = (id) => {
    ensurePetAtlas(id);
    if (id === currentId) {
      // 点击当前宠的早退也要清掉排队,否则旧排队会在其图集就绪后误触发切换。
      pendingSelectRef.current = null;
      return;
    }
    const entry = assets[id];
    // 两级懒加载:atlas 未就绪时点击一律排队(无论本次刚发起加载还是已在途),
    // 由 ensurePetAtlas 的完成/失败回调消费——用同一渲染闭包的旧快照判断
    // atlasStatus 会丢弃发起加载的那次点击(旧快照里它还是 undefined)。
    if (!entry || entry.status !== 'ready') {
      pendingSelectRef.current = id;
      return;
    }
    // 直接选择生效后清掉可能残留的旧排队,避免之后被在途回调误消费。
    pendingSelectRef.current = null;
    Promise.resolve(onSelect(id)).catch((error) => {
      console.error('[pet-selector] switch failed, keeping previous pet', error);
    });
  };

  if (!enabled) return null;

  return (
    <div className="mt-4">
      {/* biome-ignore lint/a11y/useSemanticElements: 卡片横滑轨道(region 地标语义保留给读屏),section 元素无横滑语义 */}
      <div
        id="pet-selector-panel"
        role="region"
        aria-label={t.uiPetSettings.choose}
        className="pet-card-track"
      >
        {PET_IDS.map((id) => {
          const pet = PET_REGISTRY[id];
          const localizedPet = t.uiPetSettings.pets[id] || pet;
          const entry = assets[id] || { status: 'loading' };
          const isSelected = id === currentId;
          const isReady = entry.status === 'ready';
          const showPreview = isReady && hoveredId === id && !reducedMotion;
          return (
            <div
              key={id}
              data-pet-id={id}
              className={`pet-card pet-card--light pet-card--dark ${isSelected ? 'pet-card--selected' : ''}`}
              style={{ '--pet-accent': pet.themeColor }}
            >
              <button
                type="button"
                className="pet-card-main"
                disabled={!entry.cover}
                aria-pressed={isSelected}
                onClick={() => handleSelect(id)}
                onMouseEnter={() => { setHoveredId(id); ensurePetAtlas(id); }}
                onMouseLeave={() => setHoveredId((value) => (value === id ? null : value))}
                onFocus={() => { setHoveredId(id); ensurePetAtlas(id); }}
                onBlur={() => setHoveredId((value) => (value === id ? null : value))}
              >
                {pet.placeholder && (
                  <span className="pet-card-flag pet-card-flag--dark">{t.uiPetSettings.placeholder}</span>
                )}
                <span className="pet-card-figure">
                  {/* 预览出现时隐藏封面而不是卸载：若 mousedown 按住的节点在
                      mouseup 前被拆出 DOM，浏览器不会合成 click——快速移入并
                      立即点击的用户会丢失这一下选择。 */}
                  {showPreview && <PreviewSprite atlasUrl={entry.atlas} />}
                  {entry.cover
                    ? (
                      <img
                        src={entry.cover}
                        alt=""
                        className="pet-card-cover"
                        style={showPreview ? { display: 'none' } : undefined}
                      />
                    )
                    : (!showPreview && <span className="pet-card-cover-blank" />)}
                  {entry.status === 'loading' && <span className="pet-card-ring" aria-hidden="true" />}
                  {isSelected && (
                    <span className="pet-card-badge" aria-hidden="true"><Check size={12} /></span>
                  )}
                </span>
                <span className="pet-card-name">{localizedPet.name}</span>
                <span className="pet-card-desc text-[#5F6368] dark:text-[#9AA0A6]">
                  {localizedPet.description}
                </span>
                {entry.status === 'loading' && (
                  <span className="pet-card-preparing">{t.uiPetSettings.preparing}</span>
                )}
              </button>
              {/* hover 预取的图集失败只写 atlasStatus,也必须给出错误展示与重试入口。 */}
              {(entry.status === 'error' || entry.atlasStatus === 'error' || entry.coverFailed) && (
                <div className="pet-card-error pet-card-error--dark">
                  <AlertTriangle size={14} />
                  <span>{entry.status === 'error' || entry.atlasStatus === 'error' ? t.uiPetSettings.animationFailed : t.uiPetSettings.coverFailed}</span>
                  <button type="button" className="pet-card-retry" onClick={() => loadPetAssets(id)}>
                    {t.uiPetSettings.retry}
                  </button>
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

import React, { useEffect, useMemo, useRef, useState } from 'react';
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
      [id]: { ...(state[id] || {}), coverFailed: false },
    }));
    PET_REGISTRY[id].cover()
      .then(loadImage)
      .then((cover) => {
        if (!aliveRef.current) return;
        setAssets((state) => ({ ...state, [id]: { ...(state[id] || {}), cover } }));
      })
      .catch(() => {
        if (!aliveRef.current) return;
        setAssets((state) => ({ ...state, [id]: { ...(state[id] || {}), coverFailed: true } }));
      });
  };

  const loadPetAssets = (id) => {
    setAssets((state) => ({
      ...state,
      [id]: { ...(state[id] || {}), status: 'loading', coverFailed: false },
    }));
    const entry = PET_REGISTRY[id];
    entry.cover()
      .then(loadImage)
      .then((cover) => {
        if (!aliveRef.current) return;
        setAssets((state) => ({ ...state, [id]: { ...(state[id] || {}), cover } }));
      })
      .catch(() => {
        if (!aliveRef.current) return;
        setAssets((state) => ({ ...state, [id]: { ...(state[id] || {}), coverFailed: true } }));
      });
    entry.atlas()
      .then(loadImage)
      .then((atlas) => {
        if (!aliveRef.current) return;
        setAssets((state) => ({ ...state, [id]: { ...(state[id] || {}), atlas, status: 'ready' } }));
      })
      .catch(() => {
        if (!aliveRef.current) return;
        setAssets((state) => ({ ...state, [id]: { ...(state[id] || {}), status: 'error' } }));
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
  }, [enabled]);

  // 当前选中宠的图集随区域出现预载(它随时会被桌宠窗口使用);其余宠 hover 才载。
  useEffect(() => {
    if (enabled && currentId) ensurePetAtlas(currentId);
  }, [enabled, currentId]);

  const pendingSelectRef = useRef(null);
  const ensurePetAtlas = (id) => {
    const entry = assets[id];
    if (!entry || entry.atlas || entry.atlasStatus === 'loading') return;
    setAssets((state) => ({ ...state, [id]: { ...(state[id] || {}), atlasStatus: 'loading' } }));
    PET_REGISTRY[id].atlas()
      .then(loadImage)
      .then((atlas) => {
        if (!aliveRef.current) return;
        setAssets((state) => ({ ...state, [id]: { ...(state[id] || {}), atlas, atlasStatus: 'ready', status: 'ready' } }));
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
        setAssets((state) => ({ ...state, [id]: { ...(state[id] || {}), atlasStatus: 'error' } }));
      });
  };

  const handleSelect = (id) => {
    ensurePetAtlas(id);
    if (id === currentId) return;
    const entry = assets[id];
    // 两级懒加载:atlas 未就绪时点击先排队,加载完成再切换(点击即意图,
    // 卡片 disabled 只反映封面可见性)。
    if (!entry || entry.status !== 'ready') {
      if (entry && entry.atlasStatus === 'loading') {
        pendingSelectRef.current = id; // 排队,atlas 完成回调执行切换
      }
      return;
    }
    Promise.resolve(onSelect(id)).catch((error) => {
      console.error('[pet-selector] switch failed, keeping previous pet', error);
    });
  };

  if (!enabled) return null;

  return (
    <div className="mt-4">
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
              {(entry.status === 'error' || entry.coverFailed) && (
                <div className="pet-card-error pet-card-error--dark">
                  <AlertTriangle size={14} />
                  <span>{entry.status === 'error' ? t.uiPetSettings.animationFailed : t.uiPetSettings.coverFailed}</span>
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

import React, { useEffect, useMemo, useRef, useState } from 'react';
import { AlertTriangle, Check, ChevronDown } from '../../components/icons.jsx';
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

// 预览雪碧图按协议帧规格 0.5 缩放展示；几何全部由协议常量推导，
// 不再与 pet-animation.js 双写。背景纵向 auto 以兼容 9/11 行图集。
const PREVIEW_SCALE = 0.5;
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
 * 设置页「桌宠」卡内部：当前宠物紧凑行 + 内联展开的三卡选择器。
 * 选中状态以 bridge 的 selectedPetId 为唯一真相——本组件不做乐观更新，
 * 切换失败时 UI 自然停留在旧宠物上。
 */
export default function PetSettingsSection({ isDark, enabled, selectedPetId, onSelect }) {
  const reducedMotion = useReducedMotion();
  const [expanded, setExpanded] = useState(false);
  const [assets, setAssets] = useState({});
  const [rowCover, setRowCover] = useState(null);
  const [hoveredId, setHoveredId] = useState(null);
  const aliveRef = useRef(true);
  useEffect(() => {
    // Fast Refresh 会先执行旧 effect 的 cleanup，再复用组件状态；必须在新
    // effect 挂载时恢复标志，否则后续封面/图集加载会被误判为卸载后的回调。
    aliveRef.current = true;
    return () => { aliveRef.current = false; };
  }, []);

  const currentId = normalizePetId(selectedPetId);
  const currentPet = PET_REGISTRY[currentId];

  // 关闭桌宠时选择器必须立即整体消失；重新启用只回紧凑行，不自动展开。
  useEffect(() => {
    if (!enabled) setExpanded(false);
  }, [enabled]);

  // 紧凑行缩略图：只加载当前宠物的轻量封面，不触碰其他资产。
  useEffect(() => {
    if (!enabled) return;
    let stale = false;
    setRowCover(null);
    PET_REGISTRY[currentId].cover()
      .then((url) => { if (!stale && aliveRef.current) setRowCover(url); })
      .catch(() => {});
    return () => { stale = true; };
  }, [enabled, currentId]);

  // 封面与图集分两级进状态机：封面是轻量资源，先到先显示——图集 decode
  // 期间卡片必须一直露出封面（需求硬性要求），而不是空白占位。
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

  // 展开时才懒加载三张封面与图集；失败的卡等待手动重试，不自动重拉。
  useEffect(() => {
    if (!expanded) return;
    PET_IDS.forEach((id) => {
      if (!assets[id]) loadPetAssets(id);
    });
  }, [expanded]);

  const handleSelect = (id) => {
    if (id === currentId) return;
    const entry = assets[id];
    if (!entry || entry.status !== 'ready') return;
    Promise.resolve(onSelect(id)).catch((error) => {
      console.error('[pet-selector] switch failed, keeping previous pet', error);
    });
  };

  if (!enabled) return null;

  return (
    <div className="mt-5">
      <button
        type="button"
        data-pet-selector-toggle="true"
        onClick={() => setExpanded((value) => !value)}
        aria-expanded={expanded}
        aria-controls="pet-selector-panel"
        className={`w-full flex items-center gap-3 rounded-2xl px-4 py-3 transition-colors ${
          isDark ? 'bg-[#131314] hover:bg-[#28292A]' : 'bg-white hover:bg-[#E8EDF3]'
        }`}
      >
        <span className={`pet-row-thumb ${isDark ? 'bg-[#28292A]' : 'bg-[#F0F4F9]'}`}>
          {rowCover && <img src={rowCover} alt="" className="pet-row-thumb-img" />}
        </span>
        <span className="min-w-0 flex-1 text-left">
          <span className={`block text-[12px] ${isDark ? 'text-[#9AA0A6]' : 'text-[#5F6368]'}`}>当前公仔</span>
          <span className="block text-[14px] font-medium truncate">{currentPet.name}</span>
        </span>
        <span className={`shrink-0 text-[13px] ${isDark ? 'text-[#A8C7FA]' : 'text-[#0B57D0]'}`}>更换</span>
        <ChevronDown
          size={16}
          className={`shrink-0 transition-transform ${expanded ? 'rotate-180' : ''} ${isDark ? 'text-[#9AA0A6]' : 'text-[#5F6368]'}`}
        />
      </button>

      {expanded && (
        <div
          id="pet-selector-panel"
          role="region"
          aria-label="选择公仔"
          className="pet-card-track mt-4"
        >
          {PET_IDS.map((id) => {
            const pet = PET_REGISTRY[id];
            const entry = assets[id] || { status: 'loading' };
            const isSelected = id === currentId;
            const isReady = entry.status === 'ready';
            const showPreview = isReady && hoveredId === id && !reducedMotion;
            return (
              <div
                key={id}
                data-pet-id={id}
                className={`pet-card ${isSelected ? 'pet-card--selected' : ''} ${
                  isDark ? 'pet-card--dark' : 'pet-card--light'
                }`}
                style={{ '--pet-accent': pet.themeColor }}
              >
                <button
                  type="button"
                  className="pet-card-main"
                  disabled={!isReady}
                  aria-pressed={isSelected}
                  onClick={() => handleSelect(id)}
                  onMouseEnter={() => setHoveredId(id)}
                  onMouseLeave={() => setHoveredId((value) => (value === id ? null : value))}
                  onFocus={() => setHoveredId(id)}
                  onBlur={() => setHoveredId((value) => (value === id ? null : value))}
                >
                  {pet.placeholder && (
                    <span className={`pet-card-flag ${isDark ? 'pet-card-flag--dark' : ''}`}>开发占位</span>
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
                  <span className="pet-card-name">{pet.name}</span>
                  <span className={`pet-card-desc ${isDark ? 'text-[#9AA0A6]' : 'text-[#5F6368]'}`}>
                    {pet.description}
                  </span>
                  {entry.status === 'loading' && (
                    <span className="pet-card-preparing">正在准备动画</span>
                  )}
                </button>
                {(entry.status === 'error' || entry.coverFailed) && (
                  <div className={`pet-card-error ${isDark ? 'pet-card-error--dark' : ''}`}>
                    <AlertTriangle size={14} />
                    <span>{entry.status === 'error' ? '动画加载失败' : '封面加载失败'}</span>
                    <button type="button" className="pet-card-retry" onClick={() => loadPetAssets(id)}>
                      重试
                    </button>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

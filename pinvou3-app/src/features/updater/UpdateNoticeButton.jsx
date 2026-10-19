import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { PinvouLogo } from '../../components/PinvouLogo.jsx';
import { RefreshCw, X } from '../../components/icons.jsx';
import { bridge } from '../../hooks/useBridge.js';

// Card bottom edge sits at 16px; `bottomInset` raises it above full-height
// bottom chrome (the compact shell's 56px tab bar).
const CARD_BASE_BOTTOM = 16;
const CARD_LEFT_MARGIN = 16;
// Composer-box anchor for the collision check below (ChatView's centered
// content box: input bar, queued overlay and banners all live inside it).
const COMPOSER_BOX_SELECTOR = '[data-testid="chat-composer-box"]';

export const UpdateNoticeButton = ({ bs, t, onShowChangelog, sidebarOffset = 0, bottomInset = 0, hidden = false, measureKey = '' }) => {
  // update-notice-logic.js 是 index.html 经典脚本;若未来改为延迟注入,这里
  // 必须容忍未就绪(无更新 UI 缺席一轮渲染即可),不能白屏。
  // 早退 return 必须放在全部 hooks 之后:logic 未就绪时直接 return 会跳过
  // 下方 useState/useEffect,违反 Rules of Hooks(下一轮 logic 就绪重渲染时
  // hooks 数量不一致,恰在本注释声称防御的场景崩溃)。
  const logic = window.UpdateNoticeLogic;
  const isPreview =
    !!logic && !bridge.available && logic.previewEnabled(window.location);
  const updateInfo = logic ? logic.updateInfoFor(bs, { preview: isPreview }) : null;
  const [closed, setClosed] = useState(false);
  const cardRef = useRef(null);
  // Extra bottom offset applied when the preferred slot would intersect the
  // chat composer box (see the layout effect below); 0 keeps the card docked.
  const [collisionLift, setCollisionLift] = useState(0);

  const updateVersionKey = logic && updateInfo ? logic.versionKey(updateInfo) : '';
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- reset "dismissed" when a new version arrives so the new card becomes visible again
    setClosed(false);
  }, [updateVersionKey]);

  // Collision guard for the offset slot. On narrow windows with the sidebar
  // open there is no free bottom band between the sidebar column and the
  // composer's centered box, so anchoring right of the sidebar would cover
  // composer controls (the legacy left-4 spot covers the sidebar footer
  // instead). Measure the card's CANDIDATE rect (as if not lifted) against
  // the composer box: on intersect, lift the card above the composer's top
  // edge — covering scrollable chat content is acceptable, blocking input
  // controls is not. Judging the candidate (not the current) position keeps
  // this oscillation-free; re-measured on view switches, resizes, offset
  // changes and composer height changes (the input bar grows when typing).
  useLayoutEffect(() => {
    let frame = 0;
    let composerObserver = null;
    const measure = () => {
      const card = cardRef.current;
      if (!card) return;
      const box = document.querySelector(COMPOSER_BOX_SELECTOR);
      if (!box) {
        setCollisionLift(0);
        return;
      }
      const baseBottom = CARD_BASE_BOTTOM + bottomInset;
      const candidateLeft = (sidebarOffset || 0) + CARD_LEFT_MARGIN;
      const candidateTop = window.innerHeight - baseBottom - card.offsetHeight;
      const candidateBottom = window.innerHeight - baseBottom;
      const boxRect = box.getBoundingClientRect();
      const intersects =
        candidateLeft < boxRect.right &&
        candidateLeft + card.offsetWidth > boxRect.left &&
        candidateTop < boxRect.bottom &&
        candidateBottom > boxRect.top;
      setCollisionLift(intersects ? Math.ceil(window.innerHeight - boxRect.top + 8) : 0);
    };
    const scheduleMeasure = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(measure);
    };
    const observeComposer = () => {
      const box = document.querySelector(COMPOSER_BOX_SELECTOR);
      if (box && composerObserver) composerObserver.observe(box);
      return !!box;
    };
    composerObserver = new ResizeObserver(scheduleMeasure);
    scheduleMeasure();
    if (!observeComposer()) {
      // ChatView (and its composer) lazy-mount after this component; retry
      // per frame for ~2s until the box shows up.
      let tries = 0;
      const retryObserve = () => {
        tries += 1;
        if (tries > 120 || observeComposer()) return;
        frame = requestAnimationFrame(retryObserve);
      };
      frame = requestAnimationFrame(retryObserve);
    }
    window.addEventListener('resize', scheduleMeasure);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener('resize', scheduleMeasure);
      if (composerObserver) composerObserver.disconnect();
    };
  }, [bottomInset, sidebarOffset, hidden, measureKey]);

  if (hidden || !logic || !updateInfo || closed) return null;

  const vm = logic.viewModel(bs, updateInfo, bs && bs.appVersion, {
    downloadInstall: t.downloadInstall,
    downloadInstallRestart: t.downloadInstallRestart,
    downloading: t.downloading,
    installing: t.installing,
    restartNow: t.restartNow,
    updateInstallerStarted: t.updateInstallerStarted,
    updateInstallFailed: t.updateInstallFailed,
  });

  const handleUpgrade = () => {
    if (isPreview) {
      return;
    }
    if (!bridge.available) return;
    if (vm.action === 'restart') bridge.updater.restartApp();
    else if (vm.action === 'download') bridge.updater.downloadAndInstallUpdate();
  };

  const handleShowChangelog = () => {
    if (onShowChangelog) onShowChangelog();
  };

  // Placement rules: anchor just right of the sidebar column (sidebarOffset
  // from the app shell; 0 keeps the legacy left-4 spot without a persistent
  // sidebar), `bottomInset` clears full-height bottom chrome (compact tab
  // bar), `hidden` keeps the card off the compact drawer while it covers the
  // sidebar, and collisionLift moves it above the composer when the slot
  // would intersect it (see the layout effect above).
  return (
    <div data-update-notice-card="true" className={`fixed z-[70] w-[260px] p-3.5 backdrop-blur-xl rounded-2xl border shadow-xl shrink-0 transition-all duration-300 bg-white/85 border-gray-200/60 text-gray-800 dark:bg-[#1c1c21]/85 dark:border-white/[0.06] dark:text-gray-200 dark:shadow-2xl`} style={{ left: (sidebarOffset || 0) + CARD_LEFT_MARGIN, bottom: CARD_BASE_BOTTOM + bottomInset + collisionLift }}>
      <div className="flex items-center gap-3 mb-3">
        <div className={`w-10 h-10 rounded-[10px] border shadow-inner flex items-center justify-center shrink-0 overflow-hidden relative transition-colors duration-300 bg-gradient-to-br from-gray-100 to-gray-50 border-gray-200/80 dark:bg-gradient-to-br dark:from-[#2c2c35] dark:to-[#1a1a20] dark:border-white/[0.08]`}>
          <PinvouLogo className="h-6 w-6" />
        </div>

        <div className="flex flex-col justify-center flex-1 min-w-0">
          <div className="flex items-center justify-between">
            <span className={`text-[13px] font-semibold tracking-wide transition-colors duration-300 text-gray-900 dark:text-gray-100`}>{t.newVersionFound}</span>
            <button
              type="button"
              onClick={() => setClosed(true)}
              className={`p-1 -mr-1 rounded-full transition-colors focus:outline-none text-gray-400 hover:text-gray-600 hover:bg-gray-100 dark:text-gray-500 dark:hover:text-gray-300 dark:hover:bg-white/[0.08]`}
              title={t.winClose}
            >
              <X size={14} />
            </button>
          </div>
          <span className={`text-[11px] font-mono px-1.5 py-0.5 rounded w-fit mt-0.5 transition-colors duration-300 text-gray-500 bg-gray-100 dark:text-gray-400 dark:bg-black/20`}>PINVOU v{vm.version}</span>
        </div>
      </div>

      {vm.error && (
        <div className="mb-3 text-[11px] leading-relaxed text-[#EA4335] break-words">{vm.error}</div>
      )}

      <div className="flex gap-2 text-xs font-medium">
        <button
          type="button"
          data-update-notes-button="true"
          onClick={handleShowChangelog}
          className={`flex-1 py-2 rounded-xl transition-all active:scale-[0.96] bg-gray-100 hover:bg-gray-200 text-gray-700 dark:bg-white/[0.06] dark:hover:bg-white/[0.1] dark:text-gray-200`}
        >
          {t.updateNotes}
        </button>
        <button
          type="button"
          onClick={handleUpgrade}
          disabled={vm.disabled}
          className="flex-1 py-2 bg-blue-600 hover:bg-blue-500 text-white rounded-xl transition-all active:scale-[0.96] flex justify-center items-center gap-1.5 shadow-sm shadow-blue-900/20 disabled:opacity-80 disabled:cursor-not-allowed"
        >
          {vm.downloading ? <span className="w-3.5 h-3.5 rounded-full border-2 border-white/70 border-t-transparent animate-spin" /> : <RefreshCw size={14} />}
          <span>{vm.label}</span>
        </button>
      </div>
    </div>
  );
};

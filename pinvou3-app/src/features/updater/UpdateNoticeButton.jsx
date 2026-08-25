import { useEffect, useState } from 'react';
import { PinvouLogo } from '../../components/PinvouLogo.jsx';
import { RefreshCw, X } from '../../components/icons.jsx';
import { bridge } from '../../hooks/useBridge.js';

export const UpdateNoticeButton = ({ bs, t, onShowChangelog }) => {
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

  const updateVersionKey = logic && updateInfo ? logic.versionKey(updateInfo) : '';
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- reset "dismissed" when a new version arrives so the new card becomes visible again
    setClosed(false);
  }, [updateVersionKey]);

  if (!logic || !updateInfo || closed) return null;

  const vm = logic.viewModel(bs, updateInfo, bs && bs.appVersion, {
    downloadInstall: t.downloadInstall,
    downloadInstallRestart: t.downloadInstallRestart,
    downloading: t.downloading,
    installing: t.installing,
    restartNow: t.restartNow,
    updateInstallerStarted: t.updateInstallerStarted,
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

  return (
    <div data-update-notice-card="true" className={`fixed left-4 bottom-4 z-[70] w-[260px] p-3.5 backdrop-blur-xl rounded-2xl border shadow-xl shrink-0 transition-all duration-300 bg-white/85 border-gray-200/60 text-gray-800 dark:bg-[#1c1c21]/85 dark:border-white/[0.06] dark:text-gray-200 dark:shadow-2xl`}>
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

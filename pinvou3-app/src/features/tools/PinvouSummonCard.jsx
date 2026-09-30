import { useEffect, useState } from 'react';
import { X } from '../../components/icons.jsx';
import { bridge } from '../../hooks/useBridge.js';
import { AcShieldCheck, AcSparkles } from './tool-common.jsx';

// Pin/Wu role palette (matches the artifact card): Pin = shield, orange
// #FF9500/#FF9F0A; Wu = sparkles, purple #5E5CE6. The 品/悟 glyphs are brand
// marks and stay untranslated. Returns { name, accentHex (raw inline-style
// color; Pin needs isDark), text (class), softBg (class), Icon }.
const pvRole = (isWu, isDark) => isWu
  ? { name: '悟', accentHex: '#5E5CE6', text: 'text-[#5E5CE6]',
      softBg: 'bg-[#5E5CE6]/[0.10] dark:bg-[#5E5CE6]/15', Icon: AcSparkles }
  : { name: '品', accentHex: isDark ? '#FF9F0A' : '#FF9500', text: 'text-[#FF9500] dark:text-[#FF9F0A]',
      softBg: 'bg-[#FF9500]/[0.10] dark:bg-[#FF9F0A]/15', Icon: AcShieldCheck };

// ==========================================
// PinvouSummonCard — summoned review (the user explicitly calls Pinvou).
// Introduces its persona (one primary + alternates), the trace, and issues
// colored by severity.
// ==========================================
// Per-item resolution rows, split by kind so each item offers the action that
// fits it instead of a one-size-fits-all yes/no:
//   recommendation (decision point / missing info only the user can settle)
//     -> adopt the suggestion / let the AI ask me;
//   issue.needs_verify (external fact the AI cannot know)
//     -> let the AI verify / I confirm it is fine;
//   other issues (artifact defects the AI can fix)
//     -> let the AI fix / accept as is (preselected for high severity).
// "Hand to AI" assembles targeted instructions from the chosen actions. This is
// a separate component so its useState calls keep a stable hook order.
const PinvouRows = ({ review, t, role }) => {
  const roleLabel = role || pvRole(false, false);
  const body = 'text-[#000] dark:text-[#fff]';
  const muted = 'text-[#3C3C43]/60 dark:text-[#EBEBF5]/60';
  // iOS semantic colors: high red / medium orange / low gray.
  const sevDot = (s) => s === 'high' ? '#FF3B30' : s === 'medium' ? '#FF9500' : '#C7C7CC';
  const rows = [
    ...(review.recommendations || []).map((x, i) => ({
      k: 'r' + i, raw: x, kind: 'rec', dot: '#FF9500',
      head: (x.topic ? x.topic + '：' : '') + t.pvSuggest + x.pick, sub: x.why,
    })),
    ...(review.issues || []).map((x, i) => ({
      k: 'i' + i, raw: x, kind: x.kind === 'needs_verify' ? 'verify' : 'fix',
      dot: sevDot(x.severity), sev: x.severity, nv: x.kind === 'needs_verify',
      head: x.text, sub: x.suggestion,
    })),
    ...(review.coverage || []).map((x, i) => ({
      k: 'c' + i, raw: x, kind: 'gap', dot: '#5E5CE6',
      sev: x.severity, head: x.dimension + (x.text ? '：' + x.text : ''), sub: x.suggestion,
    })),
  ];
  // Two choices per kind as [value, label]: the first asks the AI to act
  // (highlighted), the second means the user handles it (gray).
  const ACT = {
    rec: [['adopt', t.pvActAdopt], ['ask', t.pvActAsk]],
    verify: [['verify', t.pvActVerify], ['confirmed', t.pvActConfirmed]],
    fix: [['modify', t.pvActModify], ['accept', t.pvActAccept]],
    gap: [['fill', t.pvActFill], ['skip', t.pvActSkip]],
  };
  const ACTIVE = { adopt: 1, ask: 1, verify: 1, modify: 1, fill: 1 }; // actions handed to the AI
  const [res, setRes] = useState(() => {
    const m = {};
    rows.forEach(it => {
      let def = null;
      if (it.sev === 'high') def = it.kind === 'fix' ? 'modify' : it.kind === 'gap' ? 'fill' : null;
      m[it.k] = it.raw.resolution || def;
    });
    return m;
  });
  const setOne = (k, v) => setRes(p => ({ ...p, [k]: p[k] === v ? null : v }));
  const activeCount = rows.filter(it => ACTIVE[res[it.k]]).length;
  // iOS segmented-control style: selected + handed to AI = role-color fill
  // (background via style); selected but self-handled = gray fill;
  // unselected = outline.
  const chip = (on, active) => `text-[12px] px-2.5 py-1 rounded-full font-medium transition-all active:scale-[0.96] ${on
    ? (active ? 'text-white border border-transparent'
              : 'bg-black/[0.08] text-[#000] border border-transparent dark:bg-white/15 dark:text-[#fff]')
    : 'border border-black/[0.12] text-[#3C3C43]/80 hover:bg-black/5 dark:border-white/15 dark:text-[#EBEBF5]/70 dark:hover:bg-white/5'}`;
  const onResolve = () => {
    if (!bridge.available) return;
    // The modal's review is a deep copy from notify, so writing resolution on
    // it would never reach the original state. Pass the decisions by index to
    // the bridge, which writes them on state.pinvouModal.review and persists
    // them (this is what keeps resolutions from being lost).
    const resolutions = {
      recs: (review.recommendations || []).map((_, i) => res['r' + i] || 'pending'),
      issues: (review.issues || []).map((_, i) => res['i' + i] || 'pending'),
      coverage: (review.coverage || []).map((_, i) => res['c' + i] || 'pending'),
    };
    const actions = [];
    rows.forEach(it => {
      const a = res[it.k];
      if (a === 'modify') actions.push({ t: 'fix', text: it.head + (it.sub ? '（' + it.sub + '）' : '') });
      else if (a === 'verify') actions.push({ t: 'verify', text: it.head + (it.sub ? '（' + it.sub + '）' : '') });
      else if (a === 'adopt') actions.push({ t: 'adopt', topic: it.raw.topic || '', pick: it.raw.pick || '' });
      else if (a === 'ask') actions.push({ t: 'ask', topic: it.raw.topic || it.head });
      else if (a === 'fill') actions.push({ t: 'fill', dimension: it.raw.dimension || '', suggestion: it.raw.suggestion || '' });
    });
    bridge.interaction.resolvePinvouReview(resolutions, actions);
  };
  return (
    <div>
      <div className="space-y-2">
        {rows.map(it => {
          const decided = res[it.k];
          const passive = ['accept', 'confirmed', 'skip'].includes(decided);
          return (
            <div key={it.k} className={`rounded-[12px] px-3 py-2.5 transition-opacity ${passive ? 'opacity-40' : ''} bg-[#F2F2F7] dark:bg-white/[0.06]`}>
              <div className="flex gap-2.5">
                <span className="mt-[7px] w-[7px] h-[7px] rounded-full shrink-0" style={{ background: it.dot }} />
                <div className="flex-1 min-w-0">
                  <div className={`text-[14px] leading-relaxed ${body}`}>
                    {it.nv && <span className={`text-[10.5px] font-medium mr-1.5 px-1.5 py-px rounded-full align-[1px] bg-[#FFF8E1] text-[#B25000] dark:bg-[#FFD60A]/20 dark:text-[#FFD60A]`}>{t.pvNeedsVerify}</span>}
                    {it.head}
                  </div>
                  {it.sub && <div className={`text-[13px] mt-0.5 ${muted}`}>{it.sub}</div>}
                  <div className="flex gap-2 mt-2">
                    {ACT[it.kind].map(([v, label]) => (
                      <button type="button" key={v} onClick={() => setOne(it.k, v)} className={chip(decided === v, !!ACTIVE[v])}
                        style={decided === v && ACTIVE[v] ? { background: roleLabel.accentHex } : undefined}>{label}</button>
                    ))}
                  </div>
                </div>
              </div>
            </div>
          );
        })}
      </div>
      <div className="flex items-center gap-2 mt-4 pt-1">
        {activeCount > 0 && (
          <button type="button" onClick={onResolve}
            className="px-4 py-2 rounded-full text-[14px] font-semibold text-white active:scale-[0.97] transition-transform"
            style={{ background: roleLabel.accentHex }}>
            {t.pvHandToAi(activeCount)}
          </button>
        )}
        <button type="button" onClick={() => bridge.available && bridge.interaction.dismissPinvouReview()} title={t.pvSkipTitle}
          className={`px-4 py-2 rounded-full text-[14px] font-medium transition-colors text-[#3C3C43]/70 hover:bg-black/5 dark:text-[#EBEBF5]/70 dark:hover:bg-white/5`}>
          {t.pvSkip}
        </button>
      </div>
    </div>
  );
};

// Review loading state: local models take 5-30s and online models are usually
// faster. Show an iOS activity spinner, an elapsed timer and reassuring copy so
// the wait does not feel stuck.
const PinvouLoading = ({ isWu, isDark, t, isLocal }) => {
  const [secs, setSecs] = useState(0);
  useEffect(() => {
    const b = setInterval(() => setSecs(s => s + 1), 1000);
    return () => clearInterval(b);
  }, []);
  const role = pvRole(isWu, isDark);
  // isDark stays: the SVG circle stroke and Pin's raw accentHex still need it.
  const muted = 'text-[#3C3C43]/60 dark:text-[#EBEBF5]/60';
  return (
    <div className="py-8 flex flex-col items-center text-center">
      {/* iOS activity spinner: base ring + role-colored arc, constant rotation */}
      <svg aria-hidden="true" className="w-9 h-9" viewBox="0 0 24 24" fill="none" style={{ animation: 'tsSpinner 0.8s linear infinite' }}>
        <circle cx="12" cy="12" r="9" stroke={isDark ? 'rgba(255,255,255,.12)' : 'rgba(0,0,0,.08)'} strokeWidth="3" />
        <path d="M12 3a9 9 0 0 1 9 9" stroke={role.accentHex} strokeWidth="3" strokeLinecap="round" />
      </svg>
      <div className={`flex items-center gap-1.5 mt-4 text-[15px] font-semibold ${role.text}`}>
        <role.Icon className="w-[18px] h-[18px]" />
        <span>{isWu ? t.pvLoadingWu : t.pvLoadingPin}</span>
      </div>
      <div className={`text-[13px] mt-1.5 ${muted}`}>
        {isWu ? t.pvLoadingWuSub : t.pvLoadingPinSub}
        {secs > 0 && <span className="ml-1.5 tabular-nums opacity-70">{secs}s</span>}
      </div>
      <div className={`text-[12px] mt-1 ${muted}`} style={{ opacity: 0.6 }}>{t.pvLoadingHint(isLocal)}</div>
    </div>
  );
};

// Review result card (rendered inside the modal without an outer card frame;
// Pin = orange / Wu = purple, matching the artifact card).
export const PinvouSummonCard = ({ item, theme, t, isLocal }) => {
  const isDark = theme === 'dark';
  const isWu = !!item.coverage; // Wu = divergent (coverage); Pin = error check
  const role = pvRole(isWu, isDark);
  const muted = 'text-[#3C3C43]/60 dark:text-[#EBEBF5]/60';
  const body = 'text-[#000] dark:text-[#fff]';
  // isDark stays: Pin's accentHex and the PinvouLoading SVG stroke need it.
  if (item.loading) return <PinvouLoading isWu={isWu} isDark={isDark} t={t} isLocal={isLocal} />;
  if (item.error) return (
    <div className="py-2">
      <div className={`flex items-center gap-1.5 text-[15px] font-semibold ${role.text}`}><role.Icon className="w-[18px] h-[18px]" /><span>Pinvou {role.name}</span></div>
      <div className={`text-[14px] mt-2 text-[#FF3B30] dark:text-[#FF453A]`}>{t.pvFail}{item.error}</div>
    </div>
  );
  const r = item.review || {};
  if (r.dismissed) return (
    <div className={`py-2 flex items-center gap-1.5 text-[14px] ${muted}`}><role.Icon className="w-4 h-4" /><span>{'Pinvou · ' + role.name + ' · ' + t.pvSkipped}</span></div>
  );
  const personas = r.personas || [];
  const primary = personas.find(p => p && p.primary) || personas[0] || {};
  const alts = r.alternates || [];
  const hasRows = (r.recommendations || []).length > 0 || (r.issues || []).length > 0 || (r.coverage || []).length > 0;
  return (
    <div>
      <div className="flex items-center flex-wrap gap-x-2 gap-y-1 mb-2.5">
        <span className={`inline-flex items-center justify-center w-7 h-7 rounded-full ${role.softBg}`}>
          <role.Icon className={`w-[17px] h-[17px] ${role.text}`} />
        </span>
        <span className={`text-[16px] font-semibold ${body}`}>
          {'Pinvou · ' + role.name}
          {primary.label && <span className={`text-[14px] font-normal ${muted}`}> · {primary.label + t.pvPerspective}</span>}
        </span>
        {r.verdict === 'pass' && <span className={`text-[11px] font-semibold px-2 py-0.5 rounded-full bg-[#34C759]/15 text-[#248A3D] dark:bg-[#30D158]/20 dark:text-[#30D158]`}>{t.pvVerdictPass}</span>}
      </div>
      {alts.length > 0 && <div className={`text-[12px] -mt-1 mb-2 ${muted}`}>{t.pvAlsoInvolves} {alts.join(' / ')}</div>}
      {r.trace && <div className={`text-[14px] leading-relaxed mb-3 ${body}`}>{r.trace}</div>}
      {(r.framework || []).length > 0 && (
        <div className={`text-[12px] mb-3 px-3 py-2 rounded-[12px] leading-relaxed ${role.softBg} ${role.text}`}>
          <span className="opacity-70">{t.pvFramework} · {(r.framework || []).length}{t.pvDims}: </span>{(r.framework || []).join(' · ')}
        </div>
      )}
      {hasRows && <PinvouRows review={r} t={t} role={role} />}
    </div>
  );
};

// Global review modal: centered dialog over a frosted backdrop that blurs the
// app behind it. It is lazy-loaded because summoned reviews are rare and should
// stay out of the startup chunk.
export function PinvouSummonModal({ item, theme, t, isLocal }) {
  return (
    // biome-ignore lint/a11y/useKeyWithClickEvents: keyboard users close the dialog through its real close button
    // biome-ignore lint/a11y/noStaticElementInteractions: this is a pointer-only backdrop around an accessible dialog card
    <div className="fixed inset-0 z-[55] flex items-center justify-center p-6"
         style={{ background: theme === 'dark' ? 'rgba(0,0,0,.45)' : 'rgba(255,255,255,.35)', backdropFilter: 'blur(20px) saturate(140%)', WebkitBackdropFilter: 'blur(20px) saturate(140%)' }}
         onClick={() => { if (!item.loading) bridge.interaction.dismissPinvouReview(); }}>
      {/* Backdrop clicks cannot close the modal while loading: the summon call
          (direct to the model, 5-30s) is still running and its guard is held, so
          an accidental close would look like "flashes and ignores the next
          click for a while". Locking keeps the spinner visible until a result or
          error arrives. */}
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: background click-to-close layer; keyboard path handled by the top-right close button (a real button below) */}
      {/* biome-ignore lint/a11y/noStaticElementInteractions: background click-to-close layer; non-interactive container */}
      <div className="relative w-full max-w-[720px] overflow-hidden bg-white dark:bg-[#1C1C1E] rounded-[20px] shadow-[0_20px_60px_rgba(0,0,0,0.28)] ts-modal-in"
           onClick={(event) => event.stopPropagation()}
           style={{ fontFamily:'-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif' }}>
        {/* The close button is always present (including while loading): during
            loading it cancels the wait and closes; the guard discards the
            in-flight result. */}
        <button type="button" onClick={() => bridge.available && bridge.interaction.dismissPinvouReview()} aria-label={t.pvSkip}
          className="absolute top-3.5 right-3.5 z-10 w-7 h-7 flex items-center justify-center rounded-full bg-black/[0.06] dark:bg-white/10 text-[#8E8E93] hover:bg-black/10 dark:hover:bg-white/15 active:scale-90 transition-colors">
          <X size={16} />
        </button>
        <div className="max-h-[90vh] overflow-y-auto custom-scrollbar px-5 pt-5 pb-6">
          <PinvouSummonCard item={item} theme={theme} t={t} isLocal={isLocal} />
        </div>
      </div>
    </div>
  );
}

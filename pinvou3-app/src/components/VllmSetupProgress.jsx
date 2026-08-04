import React, { useEffect, useState } from 'react';

const VllmSetupProgress = ({ phase, attempt, isDark, t }) => {
      const [secs, setSecs] = useState(0);
      useEffect(() => { const id = setInterval(() => setSecs(s => s + 1), 1000); return () => clearInterval(id); }, []);
      const steps = [
        { key: 'authorizing', label: t.vllmStepAuth },
        { key: 'waiting', label: t.vllmStepWait },
        { key: 'ready', label: t.vllmStepReady },
      ];
      const order = { authorizing: 0, waiting: 1, ready: 2 };
      const cur = order[phase] != null ? order[phase] : 0;
      const mmss = Math.floor(secs / 60) + ':' + String(secs % 60).padStart(2, '0');
      const accent = isDark ? '#0A84FF' : '#007AFF';
      const muted = isDark ? '#8E8E8E' : '#757575';
      const bodyClr = isDark ? '#E3E3E3' : '#1F1F1F';
      return (
        <div className="py-1">
          <div className="space-y-2.5 mb-3">
            {steps.map((s, i) => {
              const done = i < cur, active = i === cur;
              return (
                <div key={s.key} className="flex items-center gap-2.5">
                  {done ? (
                    <span className="w-5 h-5 shrink-0 rounded-full flex items-center justify-center text-white text-[11px]" style={{ background: accent }}>✓</span>
                  ) : active ? (
                    <span className={`w-5 h-5 shrink-0 rounded-full border-[2.5px] border-t-transparent border-[#007AFF] dark:border-[#0A84FF]`}
                      style={{ animation: 'tsSpinner .8s linear infinite' }} />
                  ) : (
                    <span className="w-5 h-5 shrink-0 rounded-full border-2" style={{ borderColor: muted, opacity: .35 }} />
                  )}
                  <span className="text-[14px]" style={{ color: active ? bodyClr : muted, fontWeight: active ? 600 : 400, opacity: done ? .7 : 1 }}>{s.label}</span>
                </div>
              );
            })}
          </div>
          <div className="flex items-center gap-2 text-[12.5px]" style={{ color: muted }}>
            <span className="tabular-nums">{t.vllmElapsed} {mmss}</span>
            {phase === 'waiting' && attempt > 0 && <span style={{ opacity: .7 }}>· {t.vllmProbing(attempt)}</span>}
          </div>
          <div className="text-[12px] mt-1.5" style={{ color: muted, opacity: .7 }}>{t.vllmSetupRunning}</div>
        </div>
      );
    };

    /* ==========================================
       App — 1:1 matching 前端.html structure
       ========================================== */

export { VllmSetupProgress };

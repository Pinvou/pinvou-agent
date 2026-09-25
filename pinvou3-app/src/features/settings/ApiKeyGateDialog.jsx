import { PinvouLogo } from '../../components/PinvouLogo.jsx';

// API key gate: when a cloud model has no usable credential, cover only the
// chat surface and force configuration first. Sending with an empty key gets a
// 401 that used to surface as a silent non-response. credential_state
// "unavailable" is gated too: on macOS, denying the Keychain authorization
// prompt yields "unavailable", which fails the same way as "missing". Local
// vLLM and loopback OpenAI-compatible endpoints may run without auth. The
// settings page must stay reachable, otherwise "Configure" would leave the gate
// covering the only place where the key can be entered.
export function ApiKeyGateDialog({ onOpenModelSettings, t, theme }) {
  const dark = theme === 'dark';

  return (
    <div className="fixed inset-0 z-[57] flex items-center justify-center p-6" style={{ background: 'rgba(0,0,0,.5)' }}>
      <div className="w-full max-w-[400px] rounded-2xl p-6 ts-modal-in"
           style={{ background: dark ? '#1E1F20' : '#FFFFFF', color: dark ? '#E3E3E3' : '#1F1F1F', boxShadow: '0 12px 48px rgba(0,0,0,.35)' }}>
        <div className="flex items-center gap-2 mb-3">
          <PinvouLogo className="h-[22px] w-[22px] select-none" />
          <div className="text-[17px] font-semibold">{t.apiKeyGateTitle}</div>
        </div>
        <div className="text-[14px] leading-relaxed mb-4" style={{ opacity: .85 }}>{t.apiKeyGateDesc}</div>
        <div className="flex justify-end">
          <button type="button" onClick={onOpenModelSettings}
            className="h-9 px-4 rounded-lg text-[14px] font-medium text-white" style={{ background: '#0A84FF' }}>{t.apiKeyGateBtn}</button>
        </div>
      </div>
    </div>
  );
}

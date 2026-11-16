import claudeIcon from '../../brand-icons/claude.png';
import kimiIcon from '../../brand-icons/kimi-code.png';
import { CodexLogo } from '../../components/CodexLogo.jsx';
import { MonoLogo } from '../../components/MonoLogo.jsx';
import { resolveAppAssetUrl } from '../../shared/asset-url.mjs';

// Code-mode Pinvou identity mark: reuses the brand PNG's alpha channel as a
// monochrome mask (bg-current, black in light theme / white in dark — the
// same trick CodexLogo uses) so the code lane stays distinguishable from the
// blue work-mode brand image (PinvouLogo): one brand mark, one color per mode.
// brand-blue.png is RGBA, so it works as a mask unchanged.
const PINVOU_BRAND_URL = resolveAppAssetUrl('assets/brand/brand-blue.png');

function PinvouMonoLogo({ className = 'h-5 w-5', title }) {
  return <MonoLogo url={PINVOU_BRAND_URL} className={className} title={title} />;
}

export function AcpAgentLogo({ agentId = 'codex', className = 'h-5 w-5', title }) {
  if (agentId === 'pinvou') {
    return <PinvouMonoLogo className={className} title={title || '品悟'} />;
  }
  if (agentId === 'codex') {
    return <CodexLogo className={className} title={title || 'Codex'} />;
  }
  if (agentId === 'claude') {
    return (
      <img
        src={claudeIcon}
        alt={title || 'Claude Code'}
        title={title || 'Claude Code'}
        className={`${className} inline-block shrink-0 object-contain`}
      />
    );
  }
  if (agentId === 'kimi') {
    return (
      <img
        src={kimiIcon}
        alt={title || 'Kimi'}
        title={title || 'Kimi'}
        className={`${className} inline-block shrink-0 object-contain`}
      />
    );
  }
  return (
    <span
      role="img"
      aria-label={title || agentId}
      title={title || agentId}
      className={`${className} inline-flex items-center justify-center rounded-[30%] bg-gray-500 text-[0.62em] font-bold text-white`}
    >
      A
    </span>
  );
}

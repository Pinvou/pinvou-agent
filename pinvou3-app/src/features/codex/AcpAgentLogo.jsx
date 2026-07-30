import React from 'react';
import claudeIcon from '../../brand-icons/claude.png';
import kimiIcon from '../../brand-icons/kimi-code.png';
import { CodexLogo } from '../../components/CodexLogo.jsx';

export function AcpAgentLogo({ agentId = 'codex', className = 'h-5 w-5', title }) {
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

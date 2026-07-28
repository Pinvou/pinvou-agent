import React from 'react';
import { Briefcase, Code, Palette } from '../../components/icons.jsx';
import { CodexLogo } from '../../components/CodexLogo.jsx';
import { AcpAgentLogo } from '../codex/AcpAgentLogo.jsx';
import { IosSegmentedControl } from '../../components/IosControls.jsx';

const HOME_DESIGN_MODE_ENABLED = false;

const HOME_MODE_OPTIONS = [
  { key: 'work', labelKey: 'work', Icon: Briefcase, testId: 'home-mode-work' },
  { key: 'design', labelKey: 'design', Icon: Palette, testId: 'home-mode-design', enabled: HOME_DESIGN_MODE_ENABLED },
  { key: 'code', labelKey: 'code', Icon: Code, testId: 'home-mode-code' },
];

const CODE_AGENT_OPTIONS = [
  { key: 'codex', label: 'Codex', Logo: CodexLogo, enabled: true },
  { key: 'claude', label: 'Claude Code', enabled: true },
  { key: 'kimi', label: 'Kimi', enabled: true },
];

export function HomeModeSwitcher({
  mode,
  onChange,
  codeSupported = true,
  codeAgent = 'codex',
  onCodeAgentChange,
  isDark = false,
  copy = { work: '工作', design: '设计', code: '代码' },
}) {
  const visibleModes = HOME_MODE_OPTIONS
    .filter(option => option.enabled !== false && (option.key !== 'code' || codeSupported))
    .map(option => ({ ...option, label: copy[option.labelKey] }));
  const activeMode = visibleModes.some(option => option.key === mode) ? mode : 'work';
  const visibleCodeAgents = CODE_AGENT_OPTIONS.filter(
    option => option.enabled && (option.key === 'codex' || onCodeAgentChange),
  );

  return (
    <div data-testid="home-mode-switcher" className="mb-3 flex flex-col items-center gap-2.5">
      <IosSegmentedControl
        value={activeMode}
        onChange={onChange}
        segments={visibleModes}
        isDark={isDark}
        compact
        prominent
      />
      {activeMode === 'code' && (
        <div data-testid="code-agent-selector" className="flex items-center justify-center gap-2">
          {visibleCodeAgents.map(({ key, label, Logo }) => (
            <button
              key={key}
              type="button"
              data-testid={`code-agent-${key}`}
              aria-current={codeAgent === key ? 'true' : undefined}
              onClick={() => {
                if (onCodeAgentChange) onCodeAgentChange(key);
                if (onChange) onChange('code');
              }}
              className="relative flex h-8 items-center gap-2 px-3 text-[13px] font-medium text-gray-700 transition-colors dark:text-gray-200"
            >
              {Logo
                ? <Logo className="h-4 w-4" />
                : <AcpAgentLogo agentId={key} className="h-4 w-4" title={label} />}
              <span>{label}</span>
              {codeAgent === key && (
                <span
                  aria-hidden="true"
                  className="absolute inset-x-2 bottom-0 h-0.5 rounded-full bg-[#007AFF] dark:bg-[#0A84FF]"
                />
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

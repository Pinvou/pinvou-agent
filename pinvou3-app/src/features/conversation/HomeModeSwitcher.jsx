import React from 'react';
import { Briefcase, Code, Palette } from '../../components/icons.jsx';
import { CodexLogo } from '../../components/CodexLogo.jsx';
import { IosSegmentedControl } from '../../components/IosControls.jsx';

const HOME_DESIGN_MODE_ENABLED = false;

const HOME_MODE_OPTIONS = [
  { key: 'work', label: '工作', Icon: Briefcase, testId: 'home-mode-work' },
  { key: 'design', label: '设计', Icon: Palette, testId: 'home-mode-design', enabled: HOME_DESIGN_MODE_ENABLED },
  { key: 'code', label: '代码', Icon: Code, testId: 'home-mode-code' },
];

const CODE_AGENT_OPTIONS = [
  { key: 'codex', label: 'Codex', Logo: CodexLogo, enabled: true },
  { key: 'claude-code', label: 'Claude Code', enabled: false },
  { key: 'kimi-code', label: 'Kimi Code', enabled: false },
];

export function HomeModeSwitcher({
  mode,
  onChange,
  codeSupported = true,
  isDark = false,
}) {
  const visibleModes = HOME_MODE_OPTIONS.filter(option => (
    option.enabled !== false && (option.key !== 'code' || codeSupported)
  ));
  const activeMode = visibleModes.some(option => option.key === mode) ? mode : 'work';
  const visibleCodeAgents = CODE_AGENT_OPTIONS.filter(option => option.enabled);

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
              aria-current="true"
              onClick={() => onChange && onChange('code')}
              className="relative flex h-8 items-center gap-2 px-3 text-[13px] font-medium text-gray-700 transition-colors dark:text-gray-200"
            >
              {Logo ? <Logo className="h-4 w-4" /> : null}
              <span>{label}</span>
              <span
                aria-hidden="true"
                className="absolute inset-x-2 bottom-0 h-0.5 rounded-full bg-[#007AFF] dark:bg-[#0A84FF]"
              />
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

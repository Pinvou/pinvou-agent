import React from 'react';
import { Briefcase, Code } from '../../components/icons.jsx';
import { CodexLogo } from '../../components/CodexLogo.jsx';

export function HomeModeSwitcher({ mode, onChange, codeSupported = true }) {
  const isCode = mode === 'code';
  const optionClass = (active) => `h-9 px-4 rounded-full flex items-center gap-2 text-[13px] font-semibold transition-colors ${
    active
      ? 'bg-white text-[#1F1F1F] shadow-sm dark:bg-white/10 dark:text-white'
      : 'text-gray-500 hover:text-gray-800 hover:bg-black/[0.04] dark:text-gray-400 dark:hover:text-gray-200 dark:hover:bg-white/[0.05]'
  }`;

  return (
    <div data-testid="home-mode-switcher" className="mb-3 flex flex-col items-center gap-2.5">
      <div className="inline-flex items-center rounded-full border border-black/[0.06] dark:border-white/[0.08] bg-black/[0.04] dark:bg-white/[0.04] p-1">
        <button
          type="button"
          data-testid="home-mode-work"
          aria-pressed={!isCode}
          onClick={() => onChange && onChange('work')}
          className={optionClass(!isCode)}
        >
          <Briefcase size={15} /> 工作
        </button>
        {codeSupported && (
          <button
            type="button"
            data-testid="home-mode-code"
            aria-pressed={isCode}
            onClick={() => onChange && onChange('code')}
            className={optionClass(isCode)}
          >
            <Code size={15} /> 代码
          </button>
        )}
      </div>
      {isCode && (
        <div data-testid="code-agent-selector" className="flex items-center justify-center">
          <button
            type="button"
            aria-current="true"
            className="h-8 px-3 rounded-lg flex items-center gap-2 border-b-2 border-[#007AFF] text-[12px] font-medium text-gray-700 dark:text-gray-200"
          >
            <CodexLogo className="h-4 w-4" /> Codex
          </button>
        </div>
      )}
    </div>
  );
}

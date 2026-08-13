import React from 'react';

// 统一的 iOS 风格开关：收敛 SettingsView 内多处手写的「圆角轨道 + 白色圆形滑块」开关。
// 尺寸：
//   sm —— 工具/项目技能开关（轨道 34×20，滑块 16）
//   md —— 设置项开关（原 IOSSwitch，轨道 46×26，滑块 22）
const SIZE_STYLES = {
  sm: { track: 'h-5 w-[34px]', knob: 'h-4 w-4', on: 'translate-x-[16px]', off: 'translate-x-[2px]' },
  md: { track: 'h-[26px] w-[46px]', knob: 'h-[22px] w-[22px]', on: 'translate-x-[22px]', off: 'translate-x-[2px]' },
};

export function Toggle({ checked, onChange, disabled = false, size = 'sm', 'aria-label': ariaLabel, className = '' }) {
  const s = SIZE_STYLES[size] || SIZE_STYLES.sm;
  return (
    <button
      type="button"
      role="switch"
      aria-checked={!!checked}
      aria-label={ariaLabel}
      disabled={disabled}
      onClick={() => { if (onChange) onChange(!checked); }}
      className={`relative inline-flex shrink-0 items-center rounded-full transition-colors disabled:cursor-default ${s.track} ${disabled ? 'opacity-70' : ''} ${checked ? 'bg-[#34C759]' : 'bg-[#E5E5EA] dark:bg-[#39393D]'} ${className}`}
    >
      <span className={`inline-block rounded-full bg-white shadow transition-transform ${s.knob} ${checked ? s.on : s.off}`} />
    </button>
  );
}

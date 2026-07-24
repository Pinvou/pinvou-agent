import React from 'react';
import { Search, X } from './icons.jsx';

const searchFill = (isDark) => isDark ? 'rgba(118,118,128,.24)' : 'rgba(118,118,128,.12)';
const controlFill = (isDark) => isDark ? 'rgba(118,118,128,.18)' : 'rgba(118,118,128,.12)';

function IosSearchField({
  value,
  onChange,
  placeholder,
  isDark,
  className = '',
  inputClassName = '',
  onKeyDown,
  disabled = false,
  compact = false,
}) {
  return (
    <div className={`relative ${compact ? 'h-9' : 'h-12'} ${className}`}>
      <Search size={18} className="absolute left-3 top-1/2 -translate-y-1/2" style={{ color: '#8E8E93' }} />
      <input
        type="text"
        value={value}
        disabled={disabled}
        placeholder={placeholder}
        onChange={onChange}
        onKeyDown={onKeyDown}
        className={`h-full w-full rounded-[14px] border-none bg-transparent pl-10 pr-10 font-normal outline-none placeholder:text-[#8E8E93] disabled:cursor-default ${compact ? 'text-[13px]' : 'text-[16px]'} ${inputClassName}`}
        style={{ background: searchFill(isDark), color: isDark ? '#fff' : '#000' }}
      />
      {value ? (
        <button
          type="button"
          onClick={() => onChange && onChange({ target: { value: '' } })}
          className="absolute right-3 top-1/2 -translate-y-1/2"
          style={{ color: '#8E8E93' }}
        >
          <X size={16} />
        </button>
      ) : null}
    </div>
  );
}

function IosSegmentedControl({ value, onChange, segments, isDark, className = '', compact = false, prominent = false }) {
  const activeIndex = Math.max(0, segments.findIndex((segment) => segment.key === value));
  const segmentCount = Math.max(segments.length, 1);

  if (compact) {
    const heightClass = prominent ? 'h-10' : 'h-9';
    const radiusClass = prominent ? 'rounded-[16px]' : 'rounded-[14px]';
    const plateRadiusClass = prominent ? 'rounded-[12px]' : 'rounded-[10px]';
    const buttonClass = prominent
      ? 'h-8 min-w-[82px] gap-2 rounded-[12px] px-3.5 text-[14px]'
      : 'h-7 min-w-[68px] gap-1.5 rounded-[10px] px-3 text-[13px]';
    const iconSize = prominent ? 15 : 14;
    return (
      <div
        className={`relative grid ${heightClass} shrink-0 items-center overflow-hidden ${radiusClass} p-1 ${className}`}
        style={{
          gridTemplateColumns: `repeat(${segmentCount}, minmax(0, 1fr))`,
          background: controlFill(isDark),
          boxShadow: isDark ? 'inset 0 0 0 1px rgba(255,255,255,.06)' : 'inset 0 0 0 1px rgba(0,0,0,.06)',
        }}
      >
        <span
          aria-hidden="true"
          className={`absolute bottom-1 top-1 ${plateRadiusClass} transition-transform duration-200 ease-out`}
          style={{
            left: '4px',
            width: `calc((100% - 8px) / ${segmentCount})`,
            transform: `translateX(${activeIndex * 100}%)`,
            background: isDark ? '#3A3A3C' : '#fff',
            boxShadow: isDark ? 'none' : '0 1px 2px rgba(0,0,0,.10)',
          }}
        />
        {segments.map(({ key, label, Icon, title, count }) => {
          const selected = value === key;
          return (
            <button
              key={key}
              type="button"
              title={title || label}
              onClick={() => onChange && onChange(key)}
              className={`relative z-10 inline-flex items-center justify-center font-semibold transition-colors duration-200 ${buttonClass}`}
              style={{ color: selected ? (isDark ? '#fff' : '#1D1D1F') : (isDark ? 'rgba(235,235,245,.60)' : 'rgba(60,60,67,.60)') }}
            >
              {Icon ? <Icon size={iconSize} /> : null}
              {label ? <span>{label}</span> : null}
              {count != null ? (
                <span
                  className="ml-0.5 min-w-5 rounded-full px-1.5 py-0.5 text-center text-[11px] font-bold leading-none"
                  style={{ background: isDark ? '#0A84FF' : '#007AFF', color: '#fff' }}
                >
                  {count}
                </span>
              ) : null}
            </button>
          );
        })}
      </div>
    );
  }

  return (
    <div
      className={`inline-flex shrink-0 items-center gap-3 ${className}`}
      style={{
        background: 'transparent',
        boxShadow: 'none',
      }}
    >
      {segments.map(({ key, label, Icon, title, count }) => {
        const selected = value === key;
        return (
          <button
            key={key}
            type="button"
            title={title || label}
            onClick={() => onChange && onChange(key)}
            className="inline-flex h-9 items-center justify-center gap-2 px-3 text-[24px] font-normal tracking-tight transition-colors duration-200"
            style={{ color: selected ? (isDark ? 'rgba(255,255,255,.90)' : 'rgba(0,0,0,.90)') : (isDark ? 'rgba(235,235,245,.50)' : 'rgba(60,60,67,.42)') }}
            >
            {Icon ? <Icon size={15} /> : null}
            {label ? <span>{label}</span> : null}
          </button>
        );
      })}
    </div>
  );
}

export { IosSearchField, IosSegmentedControl };

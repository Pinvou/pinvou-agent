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

function IosSegmentedControl({
  value,
  onChange,
  segments,
  isDark,
  className = '',
  compact = false,
  prominent = false,
}) {
  const activeIndex = Math.max(0, segments.findIndex(segment => segment.key === value));

  if (compact && prominent) {
    return (
      <div
        className={`relative inline-grid h-10 shrink-0 items-center rounded-[16px] p-1 ${className}`}
        style={{
          gridTemplateColumns: `repeat(${segments.length}, minmax(82px, 1fr))`,
          background: controlFill(isDark),
          boxShadow: isDark
            ? 'inset 0 0 0 1px rgba(255,255,255,.06)'
            : 'inset 0 0 0 1px rgba(0,0,0,.06)',
        }}
      >
        <span
          aria-hidden="true"
          className="pointer-events-none absolute bottom-1 top-1 rounded-[12px] transition-transform duration-200 ease-out"
          style={{
            left: 4,
            width: `calc((100% - 8px) / ${segments.length})`,
            transform: `translateX(${activeIndex * 100}%)`,
            background: isDark ? '#3A3A3C' : '#fff',
            boxShadow: isDark ? 'none' : '0 1px 3px rgba(0,0,0,.12)',
          }}
        />
        {segments.map(({ key, label, Icon, title, testId }) => {
          const selected = value === key;
          return (
            <button
              key={key}
              type="button"
              title={title || label}
              data-testid={testId}
              aria-pressed={selected}
              onClick={() => onChange && onChange(key)}
              className="relative z-10 inline-flex h-8 min-w-[82px] items-center justify-center gap-2 rounded-[12px] px-3.5 text-[14px] font-semibold transition-colors"
              style={{
                color: selected
                  ? (isDark ? '#fff' : '#1D1D1F')
                  : (isDark ? 'rgba(235,235,245,.60)' : 'rgba(60,60,67,.60)'),
              }}
            >
              {Icon ? <Icon size={15} /> : null}
              {label ? <span>{label}</span> : null}
            </button>
          );
        })}
      </div>
    );
  }

  return (
    <div
      className={`inline-flex shrink-0 items-center ${compact ? 'h-9 gap-1 rounded-[14px] p-1' : 'gap-3 max-sm:gap-1'} ${className}`}
      style={{
        background: compact ? controlFill(isDark) : 'transparent',
        boxShadow: compact ? (isDark ? 'inset 0 0 0 1px rgba(255,255,255,.06)' : 'inset 0 0 0 1px rgba(0,0,0,.06)') : 'none',
      }}
    >
      {segments.map(({ key, label, Icon, title, count, testId }) => {
        const selected = value === key;
        return (
          <button
            key={key}
            type="button"
            title={title || label}
            data-testid={testId}
            aria-pressed={selected}
            onClick={() => onChange && onChange(key)}
            className={`inline-flex items-center justify-center whitespace-nowrap transition-colors ${compact ? 'h-7 gap-1.5 rounded-[10px] px-3 text-[13px] font-semibold' : 'h-9 gap-2 px-3 text-[24px] font-normal tracking-tight max-sm:h-8 max-sm:gap-1.5 max-sm:px-2 max-sm:text-[17px]'}`}
            style={compact
              ? (selected
                ? {
                    background: isDark ? '#3A3A3C' : '#fff',
                    color: isDark ? '#fff' : '#1D1D1F',
                    boxShadow: isDark ? 'none' : '0 1px 2px rgba(0,0,0,.10)',
                  }
                : { color: isDark ? 'rgba(235,235,245,.60)' : 'rgba(60,60,67,.60)' })
              : { color: selected ? (isDark ? 'rgba(255,255,255,.90)' : 'rgba(0,0,0,.90)') : (isDark ? 'rgba(235,235,245,.50)' : 'rgba(60,60,67,.42)') }}
            >
            {Icon ? <Icon size={compact ? 14 : 15} /> : null}
            {label ? <span>{label}</span> : null}
            {compact && count != null ? (
              <span
                className={`${compact ? 'ml-0.5 min-w-5 px-1.5 py-0.5 text-[11px]' : 'ml-1 min-w-7 px-2 py-1 text-[11px]'} rounded-full text-center font-bold leading-none`}
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

export { IosSearchField, IosSegmentedControl };

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

function IosSegmentedControl({ value, onChange, segments, isDark, className = '', compact = false }) {
  return (
    <div
      className={`inline-flex shrink-0 items-center gap-1 p-1 ${compact ? 'h-9 rounded-[14px]' : 'h-11 rounded-[15px]'} ${className}`}
      style={{
        background: controlFill(isDark),
        boxShadow: isDark ? 'inset 0 0 0 1px rgba(255,255,255,.06)' : 'inset 0 0 0 1px rgba(0,0,0,.06)',
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
            className={`inline-flex items-center justify-center font-semibold transition-colors ${compact ? 'h-7 gap-1.5 rounded-[10px] px-3 text-[13px]' : 'h-9 gap-2 rounded-[11px] px-4 text-[15px]'}`}
            style={selected
              ? {
                  background: isDark ? '#3A3A3C' : '#fff',
                  color: isDark ? '#fff' : '#1D1D1F',
                  boxShadow: isDark ? 'none' : '0 1px 2px rgba(0,0,0,.10)',
                }
              : { color: isDark ? 'rgba(235,235,245,.60)' : 'rgba(60,60,67,.60)' }}
          >
            {Icon ? <Icon size={compact ? 14 : 15} /> : null}
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

export { IosSearchField, IosSegmentedControl };

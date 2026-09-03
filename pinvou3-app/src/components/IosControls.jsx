import { Search, X } from './icons.jsx';

function IosSearchField({
  value,
  onChange,
  placeholder,
  className = '',
  inputClassName = '',
  onKeyDown,
  onClear,
  clearLabel,
  disabled = false,
  compact = false,
  inputRef,
  inputTestId,
  trailing,
}) {
  return (
    <div className={`relative ${compact ? 'h-9' : 'h-12'} ${className}`}>
      <Search size={18} className="absolute left-3 top-1/2 -translate-y-1/2" style={{ color: '#8E8E93' }} />
      <input
        ref={inputRef}
        type="text"
        data-testid={inputTestId}
        value={value}
        disabled={disabled}
        placeholder={placeholder}
        onChange={onChange}
        onKeyDown={onKeyDown}
        className={`h-full w-full rounded-[14px] border-none bg-[rgba(118,118,128,.12)] dark:bg-[rgba(118,118,128,.24)] text-[#000] dark:text-[#fff] pl-10 pr-10 font-normal outline-none placeholder:text-[#8E8E93] disabled:cursor-default ${compact ? 'text-[13px]' : 'text-[16px]'} ${inputClassName}`}
      />
      <div className="absolute right-3 top-1/2 flex -translate-y-1/2 items-center gap-1.5">
        {trailing || null}
        {value ? (
          <button
            type="button"
            aria-label={clearLabel}
            onClick={() => {
              if (onClear) onClear();
              if (onChange) onChange({ target: { value: '' } });
              if (inputRef && inputRef.current) inputRef.current.focus();
            }}
            style={{ color: '#8E8E93' }}
          >
            <X size={16} />
          </button>
        ) : null}
      </div>
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
    // isolate 让内部按钮的 z-10 只用于压在滑块之上，不会盖住 composer 弹层。
    return (
      <div
        className={`relative isolate grid ${heightClass} shrink-0 items-center overflow-hidden ${radiusClass} p-1 bg-[rgba(118,118,128,.12)] dark:bg-[rgba(118,118,128,.18)] ${className}`}
        style={{
          gridTemplateColumns: `repeat(${segmentCount}, minmax(0, 1fr))`,
          // isDark dynamic-value: 保留 (boxShadow)
          boxShadow: isDark ? 'inset 0 0 0 1px rgba(255,255,255,.06)' : 'inset 0 0 0 1px rgba(0,0,0,.06)',
        }}
      >
        <span
          aria-hidden="true"
          className={`absolute bottom-1 top-1 ${plateRadiusClass} transition-transform duration-200 ease-out bg-[#fff] dark:bg-[#3A3A3C]`}
          style={{
            left: '4px',
            width: `calc((100% - 8px) / ${segmentCount})`,
            transform: `translateX(${activeIndex * 100}%)`,
            // isDark dynamic-value: 保留 (boxShadow)
            boxShadow: isDark ? 'none' : '0 1px 2px rgba(0,0,0,.10)',
          }}
        />
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
              className={`relative z-10 inline-flex items-center justify-center font-semibold transition-colors duration-200 ${buttonClass} ` + (selected ? 'text-[#1D1D1F] dark:text-[#fff]' : 'text-[rgba(60,60,67,.60)] dark:text-[rgba(235,235,245,.60)]')}
            >
              {Icon ? <Icon size={iconSize} /> : null}
              {label ? <span>{label}</span> : null}
              {count == null ? null : (
                <span
                  className="ml-0.5 min-w-5 rounded-full px-1.5 py-0.5 text-center text-[11px] font-bold leading-none bg-[#007AFF] dark:bg-[#0A84FF] text-[#fff]"
                >
                  {count}
                </span>
              )}
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
        background: 'transparent',
        boxShadow: 'none',
      }}
    >
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
            className={`inline-flex items-center justify-center whitespace-nowrap transition-colors ${compact ? 'h-7 gap-1.5 rounded-[10px] px-3 text-[13px] font-semibold' : 'h-9 gap-2 px-3 text-[24px] font-normal tracking-tight max-sm:h-8 max-sm:gap-1.5 max-sm:px-2 max-sm:text-[17px]'} ` + (selected ? 'text-[rgba(0,0,0,.90)] dark:text-[rgba(255,255,255,.90)]' : 'text-[rgba(60,60,67,.42)] dark:text-[rgba(235,235,245,.50)]')}
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

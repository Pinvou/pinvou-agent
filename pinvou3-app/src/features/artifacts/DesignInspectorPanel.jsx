import React, { useEffect, useRef, useState } from 'react';

const rgbToHex = (value, fallback = '#000000') => {
  const raw = String(value || '').trim();
  if (/^#[0-9a-f]{6}$/i.test(raw)) return raw;
  const match = raw.match(/rgba?\((\d+),\s*(\d+),\s*(\d+)/i);
  if (!match) return fallback;
  return '#' + [match[1], match[2], match[3]]
    .map((part) => Math.max(0, Math.min(255, Number(part))).toString(16).padStart(2, '0'))
    .join('');
};

const pxNumber = (value) => {
  const n = parseFloat(String(value || '').replace('px', ''));
  return Number.isFinite(n) ? Math.round(n) : 0;
};

const cssValue = (style, key, fallback = '') => {
  const value = style && style[key];
  return value == null ? fallback : String(value);
};

const normalizeNumber = (value, unit = 'px') => {
  const n = parseFloat(String(value || '').replace(unit, ''));
  if (!Number.isFinite(n)) return '';
  return String(Math.round(n * 100) / 100);
};

const isHexColor = (value) => /^#[0-9a-f]{6}$/i.test(String(value || '').trim());

const COLOR_PRESETS = [
  '#000000', '#ffffff', '#8e8e93', '#d1d1d6',
  '#ff3b30', '#ff9500', '#ffcc00', '#34c759',
  '#007aff', '#5856d6', '#af52de', '#c9a84c',
  '#6b1c1c', '#f0d68a',
];

const FONT_PRESETS = [
  { label: '系统默认', labelKey: 'diFontSystem', group: 'System', value: 'system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif' },
  { label: '微软雅黑', group: 'Chinese', value: '"Microsoft YaHei", "PingFang SC", sans-serif' },
  { label: '苹方', group: 'Chinese', value: '"PingFang SC", "Microsoft YaHei", sans-serif' },
  { label: '宋体', group: 'Chinese', value: 'SimSun, "Songti SC", serif' },
  { label: '黑体', group: 'Chinese', value: 'SimHei, "Heiti SC", sans-serif' },
  { label: '楷体', group: 'Chinese', value: 'KaiTi, "Kaiti SC", serif' },
  { label: '仿宋', group: 'Chinese', value: 'FangSong, "FangSong SC", serif' },
  { label: '思源黑体', group: 'Chinese', value: '"Source Han Sans SC", "Noto Sans CJK SC", sans-serif' },
  { label: '思源宋体', group: 'Chinese', value: '"Source Han Serif SC", "Noto Serif CJK SC", serif' },
  { label: 'Arial', group: 'Latin', value: 'Arial, Helvetica, sans-serif' },
  { label: 'Helvetica', group: 'Latin', value: 'Helvetica, Arial, sans-serif' },
  { label: 'Inter', group: 'Latin', value: 'Inter, system-ui, sans-serif' },
  { label: 'Roboto', group: 'Latin', value: 'Roboto, Arial, sans-serif' },
  { label: 'Georgia', group: 'Latin', value: 'Georgia, "Times New Roman", serif' },
  { label: 'Times New Roman', group: 'Latin', value: '"Times New Roman", Times, serif' },
  { label: 'Monospace', group: 'Latin', value: '"SFMono-Regular", Consolas, "Liberation Mono", monospace' },
];

const normalizeFontFamily = (value) => String(value || '')
  .replace(/["']/g, '')
  .replace(/\s+/g, ' ')
  .trim()
  .toLowerCase();

const findFontPreset = (value) => {
  const normalized = normalizeFontFamily(value);
  if (!normalized) return null;
  return FONT_PRESETS.find((preset) => normalizeFontFamily(preset.value) === normalized || normalizeFontFamily(preset.label) === normalized) || null;
};

const shortElementLabel = (element) => {
  if (!element) return '';
  const selector = String(element.selector || '');
  const ownClass = String(element.className || '').trim().split(/\s+/).filter(Boolean)[0];
  if (ownClass) return `${String(element.tagName || '').toLowerCase()} .${ownClass}`.trim();
  const idMatches = selector.match(/#[A-Za-z0-9_-]+/g);
  if (idMatches && idMatches.length) return `${String(element.tagName || '').toLowerCase()} ${idMatches[idMatches.length - 1]}`.trim();
  const classMatches = selector.match(/\.[A-Za-z0-9_-]+/g);
  if (classMatches && classMatches.length) return `${String(element.tagName || '').toLowerCase()} ${classMatches[classMatches.length - 1]}`.trim();
  return String(element.tagName || selector || 'element').toLowerCase();
};

const describeSelectedElement = (element, L) => {
  if (!element) return { title: L.diNoSelection, subtitle: '', typeKey: '' };
  const tag = String(element.tagName || '').toLowerCase();
  const className = String(element.className || '').trim();
  const text = String(element.text || '').trim();
  const selector = String(element.selector || '');
  const lower = `${tag} ${className} ${selector}`.toLowerCase();
  let typeKey = 'element';
  if (tag === 'img' || tag === 'svg' || tag === 'canvas' || lower.includes('icon')) typeKey = 'graphic';
  else if (tag === 'button' || lower.includes('button') || lower.includes('btn')) typeKey = 'button';
  else if (tag === 'a') typeKey = 'link';
  else if (tag === 'input' || tag === 'textarea' || tag === 'select') typeKey = 'input';
  else if (tag === 'span' || tag === 'p' || tag === 'h1' || tag === 'h2' || tag === 'h3' || tag === 'h4' || tag === 'h5' || tag === 'h6' || text) typeKey = 'text';
  else if (lower.includes('card') || lower.includes('item')) typeKey = 'card';
  else if (tag === 'section' || tag === 'article' || tag === 'main' || tag === 'header' || tag === 'footer' || tag === 'div') typeKey = 'container';
  const readableClass = className.split(/\s+/).filter(Boolean)[0] || '';
  const fallbackTag = tag || L.diTypes.element;
  return {
    title: L.diSelected(L.diTypes[typeKey]),
    subtitle: readableClass ? `${readableClass} · ${fallbackTag}` : fallbackTag,
    typeKey,
  };
};

const DesignInspectorPanel = ({ isDark, t, selectedElement, changes = [], onApplyChange, onClearChanges, docked = false }) => {
  const L = t.uiArtifacts;
  const style = (selectedElement && selectedElement.computedStyle) || {};
  const [textDraft, setTextDraft] = useState('');
  const [changesExpanded, setChangesExpanded] = useState(false);
  const [fontMenuOpen, setFontMenuOpen] = useState(false);
  const [colorMenu, setColorMenu] = useState(null);
  const [colorDraft, setColorDraft] = useState('');
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const committedTextRef = useRef('');
  useEffect(() => {
    const next = selectedElement ? selectedElement.text || '' : '';
    setTextDraft(next);
    committedTextRef.current = next;
  }, [selectedElement && selectedElement.id]);

  useEffect(() => {
    setFontMenuOpen(false);
    setColorMenu(null);
    setDetailsOpen(false);
    setAdvancedOpen(false);
  }, [selectedElement && selectedElement.id, style.fontFamily]);

  const panelCls = docked
    ? `flex h-full w-full flex-col overflow-hidden ${isDark ? 'bg-[#1C1C1E] text-[#F5F5F7]' : 'bg-[#F5F5F7] text-[#1D1D1F]'}`
    : `w-full max-h-full overflow-y-auto rounded-[16px] border p-3 ${
      isDark
        ? 'border-white/10 bg-[#1E1F20] text-[#E3E3E3] shadow-xl shadow-black/30'
        : 'border-black/[0.08] bg-white text-[#1F1F1F] shadow-lg shadow-black/10'
    }`;
  const inputCls = `h-9 min-w-0 rounded-[12px] border px-3 text-[13px] outline-none transition-colors ${
    isDark
      ? 'border-white/10 bg-[#2C2C2E] text-[#F5F5F7] focus:border-[#0A84FF]/60'
      : 'border-black/[0.08] bg-white text-[#1D1D1F] focus:border-[#007AFF]/50'
  }`;
  const labelCls = `text-[12px] font-medium ${isDark ? 'text-[#A1A1AA]' : 'text-[#6E6E73]'}`;
  const sectionCls = `mt-3 rounded-[18px] border shadow-sm ${
    isDark
      ? 'border-white/[0.08] bg-[#2C2C2E] shadow-black/10'
      : 'border-black/[0.06] bg-white shadow-black/[0.03]'
  }`;
  const sectionTitleCls = `px-3.5 pt-3 text-[13px] font-semibold ${isDark ? 'text-[#F5F5F7]' : 'text-[#1D1D1F]'}`;
  const rowGridCls = 'grid grid-cols-2 gap-2.5 p-3.5';
  const selectCls = `${inputCls} appearance-none`;

  const commitText = () => {
    if (!selectedElement) return;
    if (textDraft !== committedTextRef.current) {
      const oldValue = committedTextRef.current;
      committedTextRef.current = textDraft;
      onApplyChange && onApplyChange({ type: 'text', oldValue, newValue: textDraft });
    }
  };
  const applyStyle = (property, oldValue, newValue) => {
    if (!selectedElement || String(oldValue == null ? '' : oldValue) === String(newValue == null ? '' : newValue)) return;
    onApplyChange && onApplyChange({ type: 'style', property, oldValue, newValue });
  };
  const applyTextStyle = (property, value) => applyStyle(property, cssValue(style, property), value);
  const applyPxStyle = (property, value) => {
    const raw = String(value || '').trim();
    if (raw === '') return;
    applyTextStyle(property, `${raw}px`);
  };
  const applyFontFamily = (value) => {
    const raw = String(value || '').trim();
    if (!raw) return;
    const preset = findFontPreset(raw);
    const nextValue = preset ? preset.value : raw;
    applyTextStyle('fontFamily', nextValue);
    setFontMenuOpen(false);
  };
  const openColorMenu = (property, fallback, allowClear) => {
    const current = rgbToHex(cssValue(style, property), fallback);
    setColorDraft(current);
    setColorMenu({ property, fallback, allowClear });
  };
  const applyColorValue = (property, fallback, value) => {
    const next = String(value || '').trim();
    if (!next) return;
    applyStyle(property, cssValue(style, property), next);
    setColorDraft(isHexColor(next) ? next : rgbToHex(next, fallback));
  };
  const submitColorDraft = () => {
    if (!colorMenu || !isHexColor(colorDraft)) return;
    applyColorValue(colorMenu.property, colorMenu.fallback, colorDraft);
  };
  const renderSection = (title, body, props = {}) => (
    <section className={sectionCls} data-testid={props.testId}>
      <div className={sectionTitleCls}>{title}</div>
      {body}
    </section>
  );
  const textField = (label, property, options = {}) => (
    <label className="flex flex-col gap-1">
      <span className={labelCls}>{label}</span>
      <input
        className={inputCls}
        defaultValue={cssValue(style, property)}
        onBlur={(e) => applyTextStyle(property, e.target.value)}
        placeholder={options.placeholder || ''}
      />
    </label>
  );
  const pxField = (label, property) => (
    <label className="flex flex-col gap-1">
      <span className={labelCls}>{label}</span>
      <input
        type="number"
        className={inputCls}
        defaultValue={normalizeNumber(cssValue(style, property))}
        onBlur={(e) => applyPxStyle(property, e.target.value)}
      />
    </label>
  );
  const selectField = (label, property, values) => (
    <label className="flex flex-col gap-1">
      <span className={labelCls}>{label}</span>
      <select className={selectCls} value={cssValue(style, property)} onChange={(e) => applyTextStyle(property, e.target.value)}>
        {values.map((option) => {
          const value = typeof option === 'string' ? option : option.value;
          const optionLabel = typeof option === 'string' ? option : option.label;
          return <option key={value} value={value}>{optionLabel}</option>;
        })}
      </select>
    </label>
  );
  const fontFamilyField = () => {
    const current = cssValue(style, 'fontFamily');
    const matched = findFontPreset(current);
    const displayLabel = matched ? (matched.labelKey ? L[matched.labelKey] : matched.label) : (current ? L.diFontCustom : L.diFontSystem);
    return (
      <div className="relative flex flex-col gap-1 sm:col-span-2">
        <span className={labelCls}>{L.diFont}</span>
        <button
          type="button"
          data-testid="design-font-family-input"
          className={`${inputCls} flex w-full items-center justify-between gap-2 text-left`}
          onClick={() => setFontMenuOpen((open) => !open)}
          style={{ fontFamily: matched ? matched.value : undefined }}
        >
          <span className="min-w-0 truncate">{displayLabel}</span>
          <span className={`shrink-0 text-[11px] ${isDark ? 'text-[#A8A8A8]' : 'text-[#777]'}`}>▼</span>
        </button>
        {fontMenuOpen && (
          <div className={`absolute left-0 right-0 top-[54px] z-30 max-h-64 overflow-y-auto rounded-[14px] border p-1.5 shadow-xl ${
            isDark ? 'border-white/10 bg-[#242528]' : 'border-black/10 bg-white'
          }`}>
            {FONT_PRESETS.map((preset) => (
              <button
                key={preset.label}
                type="button"
                data-testid="design-font-family-option"
                data-font-preset={preset.label}
                className={`flex w-full flex-col items-start rounded-[9px] px-2.5 py-2 text-left transition-colors ${
                  matched && matched.label === preset.label
                    ? (isDark ? 'bg-[#A8C7FA]/20' : 'bg-[#E8F0FE]')
                    : (isDark ? 'hover:bg-white/10' : 'hover:bg-black/5')
                }`}
                style={{ fontFamily: preset.value }}
                onMouseDown={(e) => {
                  e.preventDefault();
                  applyFontFamily(preset.value);
                }}
              >
                <span className="text-[13px] font-medium">{preset.labelKey ? L[preset.labelKey] : preset.label}</span>
              </button>
            ))}
          </div>
        )}
      </div>
    );
  };
  const colorField = (label, property, fallback = '#ffffff', testId, options = {}) => {
    const current = rgbToHex(cssValue(style, property), fallback);
    const open = colorMenu && colorMenu.property === property;
    return (
    <div className="relative col-span-2 flex min-w-0 items-center justify-between gap-3">
      <span className={`${labelCls} whitespace-nowrap`}>{label}</span>
      <input
        data-testid={testId}
        type="text"
        value={current}
        onInput={(e) => applyColorValue(property, fallback, e.currentTarget.value)}
        onChange={(e) => applyColorValue(property, fallback, e.target.value)}
        className="sr-only"
        tabIndex={-1}
        aria-hidden="true"
        readOnly={false}
      />
      <button
        type="button"
        data-testid={testId ? `${testId}-button` : undefined}
        onClick={() => open ? setColorMenu(null) : openColorMenu(property, fallback, options.allowClear !== false)}
        className={`flex h-9 w-14 shrink-0 items-center justify-center rounded-[13px] border transition-colors ${
          isDark
            ? 'border-white/10 bg-[#2C2C2E] hover:bg-[#3A3A3C]'
            : 'border-black/[0.08] bg-white hover:bg-[#F5F5F7]'
        }`}
        aria-haspopup="dialog"
        aria-expanded={open ? 'true' : 'false'}
      >
        <span className="h-5 w-8 rounded-[6px] border border-black/15 shadow-inner" style={{ background: current }} />
      </button>
      {open && (
        <div
          data-testid="design-color-popover"
          className={`absolute right-0 top-11 z-40 w-[236px] rounded-[18px] border p-3 shadow-2xl ${
            isDark ? 'border-white/10 bg-[#2C2C2E] text-[#F5F5F7]' : 'border-black/[0.08] bg-white text-[#1D1D1F]'
          }`}
        >
          <div className="flex items-center gap-3">
            <div className="h-12 w-12 rounded-full border border-black/10 shadow-inner" style={{ background: current }} />
            <div className="min-w-0 flex-1">
              <div className={`text-[11px] font-medium ${isDark ? 'text-[#A1A1AA]' : 'text-[#6E6E73]'}`}>{label}</div>
              <input
                data-testid="design-color-hex-input"
                value={colorDraft}
                onChange={(e) => setColorDraft(e.target.value)}
                onBlur={submitColorDraft}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') {
                    e.preventDefault();
                    submitColorDraft();
                    setColorMenu(null);
                  } else if (e.key === 'Escape') {
                    setColorMenu(null);
                  }
                }}
                className={`${inputCls} mt-1 h-8 w-full font-mono`}
                placeholder="#000000"
              />
            </div>
          </div>
          <div className="mt-3 grid grid-cols-7 gap-2">
            {COLOR_PRESETS.map((color) => (
              <button
                key={color}
                type="button"
                data-testid="design-color-preset"
                data-color={color}
                onClick={() => applyColorValue(property, fallback, color)}
                className={`h-7 w-7 rounded-full border transition-transform hover:scale-105 ${
                  current.toLowerCase() === color.toLowerCase()
                    ? 'border-[#007AFF] ring-2 ring-[#007AFF]/25'
                    : (isDark ? 'border-white/20' : 'border-black/10')
                }`}
                style={{ background: color }}
                aria-label={L.diPickColor(color)}
              />
            ))}
          </div>
          <div className="mt-3 flex items-center justify-between gap-2">
            <button
              type="button"
              disabled={options.allowClear === false}
              onClick={() => applyColorValue(property, fallback, 'transparent')}
              className={`h-8 rounded-full px-3 text-[12px] font-medium transition-colors ${
                options.allowClear === false
                  ? 'cursor-not-allowed opacity-40'
                  : (isDark ? 'bg-white/[0.08] hover:bg-white/[0.12]' : 'bg-black/[0.05] hover:bg-black/[0.08]')
              }`}
            >
              {L.diClear}
            </button>
            <button
              type="button"
              onClick={() => {
                submitColorDraft();
                setColorMenu(null);
              }}
              className="h-8 rounded-full bg-[#007AFF] px-3 text-[12px] font-semibold text-white hover:bg-[#006EE6]"
            >
              {L.diDone}
            </button>
          </div>
        </div>
      )}
    </div>
    );
  };
  const groupedChanges = changes.reduce((acc, change) => {
    const key = change.groupId || `${change.selector || 'unknown'}:${change.id}`;
    if (!acc[key]) acc[key] = { key, label: change.groupLabel || change.elementLabel || change.selector || L.diChangeFallback, items: [] };
    acc[key].items.push(change);
    return acc;
  }, {});
  const selectedSummary = describeSelectedElement(selectedElement, L);
  const hasTextContent = String(selectedElement && selectedElement.text || '').trim().length > 0;
  const isTextElement = selectedSummary.typeKey === 'text' || hasTextContent;
  const hasValue = (property) => {
    const raw = cssValue(style, property).trim();
    return raw && !['auto', 'normal', 'none', 'initial', 'unset', '0px', '0', 'rgba(0, 0, 0, 0)'].includes(raw);
  };

  if (!selectedElement) {
    return (
      <div data-testid="design-inspector-panel" className={panelCls}>
        <div className={docked ? 'p-4 text-[13px] font-semibold' : 'text-[13px] font-semibold'}>{L.diSelectHint}</div>
      </div>
    );
  }

  return (
    <div data-testid="design-inspector-panel" className={panelCls}>
      <div className={`${docked ? 'min-h-0 flex-1 overflow-y-auto p-4' : ''}`}>
        <div className="flex items-center justify-between gap-3">
          <div className="min-w-0">
            <div className="text-[14px] font-semibold truncate" data-testid="design-selected-element" title={selectedSummary.subtitle}>
              {selectedSummary.title}
            </div>
            <div className={`mt-0.5 text-[12px] truncate ${isDark ? 'text-[#8E8E93]' : 'text-[#757575]'}`}>
              {selectedSummary.subtitle} · {Math.round(selectedElement.rect?.width || 0)} × {Math.round(selectedElement.rect?.height || 0)}
            </div>
            <button
              type="button"
              data-testid="design-selected-details-toggle"
              onClick={() => setDetailsOpen((value) => !value)}
              className={`mt-1 text-[11px] font-medium ${isDark ? 'text-[#A8C7FA] hover:text-[#C2D7FB]' : 'text-[#0B57D0] hover:text-[#174EA6]'}`}
            >
              {detailsOpen ? L.diCollapseDetails : L.diViewDetails}
            </button>
          </div>
          {changes.length > 0 && (
            <button
              type="button"
              onClick={onClearChanges}
              data-testid="design-clear-changes"
              className={`shrink-0 h-8 px-3 rounded-full text-[12px] font-semibold transition-colors ${
                isDark ? 'bg-white/[0.08] hover:bg-white/[0.12] text-[#F5F5F7]' : 'bg-black/[0.05] hover:bg-black/[0.08] text-[#3C3C43]'
              }`}
            >
              {L.diClearChanges}
            </button>
          )}
        </div>
        {detailsOpen && (
          <div data-testid="design-selected-details" className={`mt-2 rounded-[10px] p-2 text-[11px] leading-relaxed ${isDark ? 'bg-white/[0.04] text-[#A8A8A8]' : 'bg-black/[0.035] text-[#757575]'}`}>
            <div className="font-semibold">{shortElementLabel(selectedElement)}</div>
            <div className="mt-1 break-all">{selectedElement.selector || selectedElement.className || L.diNoLocation}</div>
          </div>
        )}

        {renderSection(L.diSecCommon, (
          <div className={rowGridCls}>
            {isTextElement && (
              <label className="flex flex-col gap-1 col-span-2">
                <span className={labelCls}>{L.diText}</span>
                <input
                  data-testid="design-text-input"
                  className={inputCls}
                  value={textDraft}
                  onChange={(e) => setTextDraft(e.target.value)}
                  onBlur={commitText}
                  onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); e.currentTarget.blur(); } }}
                />
              </label>
            )}
            {fontFamilyField()}
            <label className="flex flex-col gap-1">
              <span className={labelCls}>{L.diFontSize}</span>
              <input
                data-testid="design-font-size-input"
                type="number"
                min="1"
                className={inputCls}
                defaultValue={pxNumber(style.fontSize)}
                onBlur={(e) => onApplyChange && onApplyChange({ type: 'style', property: 'fontSize', oldValue: style.fontSize || '', newValue: `${e.target.value || 0}px` })}
              />
            </label>
            {textField(L.diFontWeight, 'fontWeight')}
            {selectField(L.diAlign, 'textAlign', [
              { value: 'start', label: L.diOptDefault },
              { value: 'left', label: L.diAlignLeft },
              { value: 'center', label: L.diOptCenter },
              { value: 'right', label: L.diAlignRight },
              { value: 'justify', label: L.diAlignJustify },
            ])}
            {colorField(L.diTextColor, 'color', '#000000', 'design-color-input', { allowClear: false })}
          </div>
        ), { testId: 'design-section-common' })}

        {renderSection(L.diSecAppearance, (
          <div className={rowGridCls}>
            {colorField(L.diBgColor, 'backgroundColor', '#ffffff', 'design-background-input')}
            {colorField(L.diBorderColor, 'borderTopColor', '#000000')}
            <label className="flex flex-col gap-1">
              <span className={labelCls}>{L.diRadius}</span>
              <input
                data-testid="design-radius-input"
                type="number"
                min="0"
                className={inputCls}
                defaultValue={pxNumber(style.borderRadius || style.borderTopLeftRadius)}
                onBlur={(e) => onApplyChange && onApplyChange({ type: 'style', property: 'borderRadius', oldValue: style.borderRadius || '', newValue: `${e.target.value || 0}px` })}
              />
            </label>
            <label className="flex flex-col gap-1">
              <span className={labelCls}>{L.diOpacity}</span>
              <input
                type="number"
                min="0"
                max="100"
                className={inputCls}
                defaultValue={Math.round(Number(cssValue(style, 'opacity', '1')) * 100)}
                onBlur={(e) => applyTextStyle('opacity', String(Math.max(0, Math.min(100, Number(e.target.value || 0))) / 100))}
              />
            </label>
            {hasValue('backgroundImage') && textField(L.diBgImage, 'backgroundImage')}
          </div>
        ), { testId: 'design-section-appearance' })}

        {renderSection(L.diSecSize, (
          <div className={rowGridCls}>
            {pxField(L.diWidth, 'width')}
            {pxField(L.diHeight, 'height')}
          </div>
        ), { testId: 'design-section-size' })}

        <section className={sectionCls} data-testid="design-section-advanced">
          <button
            type="button"
            data-testid="design-advanced-toggle"
            onClick={() => setAdvancedOpen((value) => !value)}
            className="flex w-full items-center justify-between px-3.5 py-3 text-left"
          >
            <span className={`text-[13px] font-semibold ${isDark ? 'text-[#F5F5F7]' : 'text-[#1D1D1F]'}`}>{L.diSecAdvanced}</span>
            <span className={`text-[12px] ${isDark ? 'text-[#A1A1AA]' : 'text-[#6E6E73]'}`}>{advancedOpen ? L.diCollapse : L.diExpand}</span>
          </button>
          {advancedOpen && (
            <div data-testid="design-advanced-content">
              <div className={rowGridCls}>
                {pxField(L.diLineHeight, 'lineHeight')}
                {pxField(L.diLetterSpacing, 'letterSpacing')}
                {textField(L.diBgImage, 'backgroundImage')}
                {textField(L.diBgSize, 'backgroundSize', { placeholder: 'cover / contain' })}
                {textField(L.diBgPosition, 'backgroundPosition')}
                {selectField(L.diRepeat, 'backgroundRepeat', [
                  { value: 'repeat', label: L.diRepeat },
                  { value: 'no-repeat', label: L.diRepeatNone },
                  { value: 'repeat-x', label: L.diRepeatX },
                  { value: 'repeat-y', label: L.diRepeatY },
                ])}
                {pxField(L.diMinWidth, 'minWidth')}
                {pxField(L.diMaxWidth, 'maxWidth')}
                {pxField(L.diMinHeight, 'minHeight')}
                {pxField(L.diMaxHeight, 'maxHeight')}
                {pxField(L.diMarginTop, 'marginTop')}
                {pxField(L.diMarginRight, 'marginRight')}
                {pxField(L.diMarginBottom, 'marginBottom')}
                {pxField(L.diMarginLeft, 'marginLeft')}
                {pxField(L.diPaddingTop, 'paddingTop')}
                {pxField(L.diPaddingRight, 'paddingRight')}
                {pxField(L.diPaddingBottom, 'paddingBottom')}
                {pxField(L.diPaddingLeft, 'paddingLeft')}
                {pxField(L.diGap, 'gap')}
                {pxField(L.diRowGap, 'rowGap')}
                {pxField(L.diColumnGap, 'columnGap')}
                {selectField(L.diDisplay, 'display', [
                  { value: 'block', label: L.diDisplayBlock },
                  { value: 'flex', label: L.diDisplayFlex },
                  { value: 'grid', label: L.diDisplayGrid },
                  { value: 'inline', label: L.diDisplayInline },
                  { value: 'inline-block', label: L.diDisplayInlineBlock },
                  { value: 'none', label: L.diOptHidden },
                ])}
                {selectField(L.diDirection, 'flexDirection', [
                  { value: 'row', label: L.diDirRow },
                  { value: 'row-reverse', label: L.diDirRowReverse },
                  { value: 'column', label: L.diDirColumn },
                  { value: 'column-reverse', label: L.diDirColumnReverse },
                ])}
                {selectField(L.diJustify, 'justifyContent', [
                  { value: 'normal', label: L.diOptDefault },
                  { value: 'flex-start', label: L.diOptStart },
                  { value: 'center', label: L.diOptCenter },
                  { value: 'flex-end', label: L.diOptEnd },
                  { value: 'space-between', label: L.diJustifyBetween },
                  { value: 'space-around', label: L.diJustifyAround },
                ])}
                {selectField(L.diAlignItems, 'alignItems', [
                  { value: 'normal', label: L.diOptDefault },
                  { value: 'stretch', label: L.diStretch },
                  { value: 'flex-start', label: L.diOptStart },
                  { value: 'center', label: L.diOptCenter },
                  { value: 'flex-end', label: L.diOptEnd },
                ])}
                {selectField(L.diOverflow, 'overflow', [
                  { value: 'visible', label: L.diOptVisible },
                  { value: 'hidden', label: L.diOptHidden },
                  { value: 'clip', label: L.diOverflowClip },
                  { value: 'scroll', label: L.diOverflowScroll },
                  { value: 'auto', label: L.diOptAuto },
                ])}
                {selectField(L.diPosition, 'position', [
                  { value: 'static', label: L.diOptDefault },
                  { value: 'relative', label: L.diPosRelative },
                  { value: 'absolute', label: L.diPosAbsolute },
                  { value: 'fixed', label: L.diPosFixed },
                  { value: 'sticky', label: L.diPosSticky },
                ])}
                {pxField(L.diTop, 'top')}
                {pxField(L.diRight, 'right')}
                {pxField(L.diBottom, 'bottom')}
                {pxField(L.diLeft, 'left')}
                {textField(L.diZIndex, 'zIndex')}
                {selectField(L.diVisibility, 'visibility', [
                  { value: 'visible', label: L.diOptVisible },
                  { value: 'hidden', label: L.diOptHidden },
                  { value: 'collapse', label: L.diVisCollapse },
                ])}
                {textField(L.diCursor, 'cursor')}
              </div>
            </div>
          )}
        </section>

        {changes.length > 0 && (
          <div data-testid="design-changes-log" className={`mt-3 rounded-[12px] p-2 text-[11px] ${isDark ? 'bg-black/20' : 'bg-black/[0.035]'}`}>
            <button
              type="button"
              onClick={() => setChangesExpanded((value) => !value)}
              data-testid="design-changes-toggle"
              className="flex w-full items-center justify-between text-left font-semibold"
            >
              <span>{L.diChangesLog(changes.length)}</span>
              <span>{changesExpanded ? L.diCollapse : L.diExpand}</span>
            </button>
            {changesExpanded && (
              <div className="mt-1 max-h-48 overflow-y-auto space-y-2">
                {Object.values(groupedChanges).slice(-8).map((group) => (
                  <div key={group.key} className={`rounded-[8px] p-2 ${isDark ? 'bg-white/[0.04]' : 'bg-white'}`}>
                    <div className="mb-1 truncate font-semibold">{group.label} · {group.items.length}</div>
                    {group.items.slice(-6).map((change) => (
                      <div key={change.id} className="truncate">
                        {change.type === 'text' ? 'text' : change.property}: {change.oldValue || L.diEmpty} -&gt; {change.newValue || L.diEmpty}
                      </div>
                    ))}
                  </div>
                ))}
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
};

export { DesignInspectorPanel, rgbToHex, pxNumber };

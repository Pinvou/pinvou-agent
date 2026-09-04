// 空态欢迎语下方的场景/模板卡片入口（design lane 并入 work 后取代输入区上方的
// SubModePicker/PersonalWorkbenchTemplatePicker 三段堆叠，输入区只留工作/代码切换器）。
// 点场景卡 = 选择/再点取消该场景；点模板卡 = 把模板完整提示词填进输入框供编辑。

function SceneCardGrid({ items, activeKey, onSelect }) {
  return (
    <div
      data-testid="scene-card-grid"
      className="mt-8 grid grid-cols-2 gap-3 sm:grid-cols-4"
    >
      {items.map((item) => {
        const selected = item.key === activeKey;
        const ItemIcon = item.Icon;
        return (
          <button
            key={item.key}
            type="button"
            data-testid={`scene-card-${item.key}`}
            aria-pressed={selected}
            onClick={() => onSelect && onSelect(item.key)}
            className={`group flex flex-col items-center gap-2.5 rounded-2xl border px-3 py-4 text-center transition-all duration-200 ${
              selected
                ? 'border-blue-400/60 bg-blue-50/70 shadow-sm dark:border-blue-500/40 dark:bg-blue-500/10'
                : 'border-slate-200/60 bg-slate-50/40 hover:border-blue-200 hover:bg-blue-50/40 dark:border-[#3A3A3C]/50 dark:bg-[#2A2B2D]/30 dark:hover:border-[#555] dark:hover:bg-[#2A2B2D]/60'
            }`}
          >
            {ItemIcon && (
              <span className={`flex h-9 w-9 items-center justify-center rounded-xl transition-colors ${
                selected
                  ? 'bg-blue-500/15 text-blue-600 dark:bg-blue-400/15 dark:text-blue-300'
                  : 'bg-black/[0.04] text-slate-500 group-hover:text-blue-600 dark:bg-white/[0.06] dark:text-slate-400 dark:group-hover:text-blue-300'
              }`}>
                <ItemIcon size={18} />
              </span>
            )}
            <span className={`text-[13px] font-medium transition-colors ${
              selected
                ? 'text-blue-700 dark:text-blue-300'
                : 'text-slate-700 dark:text-slate-300'
            }`}>
              {item.label}
            </span>
          </button>
        );
      })}
    </div>
  );
}

function TemplateCardGrid({ templates, selectedIndex, onSelect, copy }) {
  return (
    <div
      data-testid="personal-workbench-template-cards"
      className="mt-3 grid grid-cols-2 gap-2.5 sm:grid-cols-3 md:grid-cols-4"
    >
      {templates.map((template, index) => {
        const selected = selectedIndex === index;
        const label = copy?.workbenchTemplates?.[template.id] || template.title;
        return (
          <button
            key={template.id}
            type="button"
            data-testid={`personal-workbench-template-${index}`}
            aria-pressed={selected}
            onClick={() => onSelect && onSelect(index)}
            className={`rounded-xl border px-3 py-2.5 text-[13px] font-medium transition-all duration-200 ${
              selected
                ? 'border-blue-400/60 bg-blue-50/70 text-blue-700 shadow-sm dark:border-blue-500/40 dark:bg-blue-500/10 dark:text-blue-300'
                : 'border-slate-200/60 bg-white/60 text-slate-600 hover:border-blue-200 hover:text-blue-700 dark:border-[#3A3A3C]/50 dark:bg-[#2A2B2D]/30 dark:text-slate-300 dark:hover:border-[#555] dark:hover:text-blue-300'
            }`}
          >
            {label}
          </button>
        );
      })}
    </div>
  );
}

export { SceneCardGrid, TemplateCardGrid };

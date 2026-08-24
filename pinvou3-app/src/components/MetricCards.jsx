
const WidgetCard = ({ title, children }) => (
  <div className="rounded-[24px] p-8 flex flex-col transition-shadow hover:shadow-md bg-[#F0F4F9] dark:bg-[#1E1F20]">
    <div className="text-[14px] font-medium tracking-wide mb-6 text-[#0B57D0] dark:text-[#A8C7FA]">{title}</div>
    <div className="flex-1 flex flex-col">{children}</div>
  </div>
);

const ProgressBar = ({ label, value, subValue, percentage, color = '#0B57D0' }) => (
  <div>
    <div className="flex justify-between items-end mb-2">
      <span className="text-[14px] text-[#1F1F1F] dark:text-[#E3E3E3]">{label}</span>
      <div className="text-right">
        <span className="text-[16px] font-medium text-[#1F1F1F] dark:text-[#E3E3E3]">{value}</span>
        {subValue && <span className="text-[13px] ml-2 text-[#444746] dark:text-[#C4C7C5]">{subValue}</span>}
      </div>
    </div>
    <div className="h-2 w-full rounded-full overflow-hidden bg-[#E1E5EA] dark:bg-[#333537]">
      <div
        className="h-full rounded-full transition-all duration-1000 ease-out bg-[#C4C7C5] dark:bg-[#444746]"
        style={{ width: `${percentage > 0 ? percentage : 100}%`, backgroundColor: percentage > 0 ? color : undefined }}
      />
    </div>
  </div>
);

const ListRow = ({ label, value, border = true }) => (
  <div className="flex justify-between items-center px-4 py-3 relative">
    <span className="text-[14px] text-[#1F1F1F] dark:text-[#E3E3E3]">{label}</span>
    <span className="text-[14px] font-mono text-[#444746] dark:text-[#C4C7C5]">{value}</span>
    {border && <div className="absolute bottom-0 right-4 left-4 h-[1px] bg-black/5 dark:bg-white/5" />}
  </div>
);

export { ListRow, ProgressBar, WidgetCard };

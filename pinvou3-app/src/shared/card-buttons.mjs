// Canonical action-button classes for tool/plan cards (tool-renderers), the
// YOLO first-switch confirm card, and the artifacts panel chrome. This used to
// be mirrored verbatim in shared/yolo-confirm-card.jsx (the shared layer must
// not import features); both sides now import from here.
const cardBtnCls = (variant) => {
  const base = 'px-3 py-1.5 rounded-full text-[13px] font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed';
  if (variant === 'primary') return `${base} bg-[#0B57D0] text-white hover:bg-[#0A4BB8] dark:bg-[#A8C7FA] dark:text-[#062E6F] dark:hover:bg-[#C2DBFF]`;
  // 危险确认（如首切 YOLO）：红底白字，深浅色同配色（红色在两种主题下对比度都够）。
  if (variant === 'danger') return `${base} bg-[#C5221F] text-white hover:bg-[#A50E0E]`;
  return `${base} bg-white text-[#1F1F1F] hover:bg-[#E1E5EA] border border-black/10 dark:border-transparent dark:bg-[#333537] dark:text-[#E3E3E3] dark:hover:bg-[#444746]`;
};

export { cardBtnCls };

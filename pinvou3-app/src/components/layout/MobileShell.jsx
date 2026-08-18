import React from 'react';
import { createPortal } from 'react-dom';
import { Edit2, Menu } from '../icons.jsx';

// 移动壳层（仅 Web 紧凑视口渲染，桌面窗口不受影响）：
// 顶部栏（☰ 会话抽屉 + 标题 + 新对话）、底部主导航 Tab、「更多」底部面板。
// 主导航收敛为 对话/卡池/运行状态 三个 Tab，其余入口全部走「更多」；
// 会话列表复用现有侧栏抽屉（max-sm 下是 overlay），不重建第二套列表。

const MobileTopBar = ({ theme, t, title, onMenu, onNewChat }) => {
  const btnCls = 'w-10 h-10 shrink-0 rounded-full flex items-center justify-center transition-colors text-[#444746] hover:bg-[#E1E5EA] dark:text-[#E3E3E3] dark:hover:bg-[#333537]';
  return (
    <div data-testid="mobile-top-bar" className="h-12 shrink-0 flex items-center gap-1 px-2 bg-[#F0F4F9] dark:bg-[#1E1F20]">
      <button type="button" data-testid="mobile-navigation-open"
        aria-label={t.uiComponents.openNavigation} onClick={onMenu} className={btnCls}>
        <Menu size={20} />
      </button>
      <span className="flex-1 min-w-0 truncate text-center text-[15px] font-medium px-1">{title}</span>
      {onNewChat ? (
        <button type="button" title={t.newChat} aria-label={t.newChat} onClick={onNewChat} className={btnCls}>
          <Edit2 size={18} />
        </button>
      ) : (
        <span className="w-10 h-10 shrink-0" />
      )}
    </div>
  );
};

const MobileTabBar = ({ theme, tabs }) => {
  return (
    <div data-testid="mobile-tab-bar" className="h-14 shrink-0 flex items-stretch border-t bg-[#F0F4F9] border-black/10 dark:bg-[#1E1F20] dark:border-white/10">
      {tabs.map(tab => (
        <button key={tab.key} type="button" data-testid={`mobile-tab-${tab.key}`} onClick={tab.onClick}
          className="flex-1 min-w-0 flex flex-col items-center justify-center gap-1 select-none">
          <span className={`relative flex items-center justify-center w-12 h-7 rounded-full transition-colors ${tab.active
            ? 'bg-[#D3E3FD] text-[#0B57D0] dark:bg-[#A8C7FA] dark:text-[#041E49]'
            : 'text-[#444746] dark:text-[#C4C7C5]'}`}>
            {tab.icon}
            {tab.dot && <span className="absolute -top-0.5 right-1 w-2 h-2 rounded-full bg-[#EA4335]" />}
          </span>
          <span className={`text-[11px] leading-none ${tab.active
            ? 'text-[#0B57D0] font-semibold dark:text-[#E3E3E3]'
            : 'text-[#5F6368] dark:text-[#9AA0A6]'}`}>{tab.label}</span>
        </button>
      ))}
    </div>
  );
};

const MobileMoreSheet = ({ theme, title, items, onClose }) => {
  return createPortal(
    <div data-testid="mobile-more-sheet" className="fixed inset-0 z-[70] flex flex-col justify-end" onClick={onClose}>
      <div className="absolute inset-0 bg-black/40" />
      <div onClick={e => e.stopPropagation()}
        className="relative rounded-t-[20px] px-4 pt-3 pb-[max(16px,env(safe-area-inset-bottom))] bg-white text-[#1F1F1F] dark:bg-[#1E1F20] dark:text-[#E3E3E3]">
        <div className="mx-auto mb-3 h-1 w-9 rounded-full bg-black/15 dark:bg-white/20" />
        <div className="mb-2 px-1 text-[13px] font-semibold text-[#5F6368] dark:text-[#9AA0A6]">{title}</div>
        <div className="grid grid-cols-4 gap-2 pb-1">
          {items.map(item => (
            <button key={item.key} type="button" data-testid={`mobile-more-${item.key}`} onClick={item.onClick}
              className={`flex flex-col items-center gap-1.5 rounded-2xl px-1 py-3 transition-colors ${item.active
                ? 'bg-[#D3E3FD]/60 dark:bg-[#A8C7FA]/15'
                : 'active:bg-black/[0.06] dark:active:bg-white/10'}`}>
              <span className="relative flex h-11 w-11 items-center justify-center rounded-full bg-[#F0F4F9] text-[#444746] dark:bg-[#333537] dark:text-[#E3E3E3]">
                {item.icon}
                {item.dot && <span className="absolute top-0 right-0 w-2.5 h-2.5 rounded-full border-2 border-[#ffffff] bg-[#EA4335] dark:border-[#1E1F20]" />}
              </span>
              <span className="text-[11px] leading-tight text-center">{item.label}</span>
            </button>
          ))}
        </div>
      </div>
    </div>,
    document.body
  );
};

export { MobileTopBar, MobileTabBar, MobileMoreSheet };

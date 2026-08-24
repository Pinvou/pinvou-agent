import React from 'react';

// 懒加载视图 chunk 拉取失败的整页兜底:React.lazy 的 rejected promise 会被
// 永久缓存,断网恢复后切视图仍会抛错——没有边界就是白屏+锁死到手动刷新。
// 边界提供「重新加载页面」入口(reload 会重新执行 import,绕开缓存)。
// 文案走 i18n 三语(uiMainApp.viewLoadFailed/viewReload),不引入单语言硬编码。
export class ViewErrorBoundary extends React.Component {
  constructor(props) {
    super(props);
    this.state = { error: null };
  }

  static getDerivedStateFromError(error) {
    return { error };
  }

  componentDidCatch(error, info) {
    console.error(
      '[view] lazy chunk load failed',
      error && error.stack ? error.stack : error,
      info && info.componentStack ? info.componentStack : info,
    );
  }

  render() {
    if (this.state.error) {
      const copy = (this.props.t && this.props.t.uiMainApp) || {};
      return (
        <div className="flex-1 flex flex-col w-full h-full relative z-10 px-16 py-12">
          <div className="max-w-[800px] rounded-2xl border p-5 bg-white border-[#DDE3EA] text-[#1F1F1F] dark:bg-[#1F2023] dark:border-[#333537] dark:text-[#E8EAED]">
            <div className="text-[18px] font-semibold mb-2">{copy.viewLoadFailed}</div>
            <div className="text-[13px] leading-relaxed text-[#444746] dark:text-[#C4C7C5]">
              {String((this.state.error && this.state.error.message) || this.state.error)}
            </div>
            <button
              type="button"
              onClick={() => window.location.reload()}
              className="mt-4 px-4 h-9 rounded-full text-[13px] font-semibold text-white bg-[#0B57D0] dark:bg-[#0A84FF]"
            >
              {copy.viewReload}
            </button>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}

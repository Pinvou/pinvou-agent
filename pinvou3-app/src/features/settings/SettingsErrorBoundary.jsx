import React from 'react';

export class SettingsErrorBoundary extends React.Component {
  constructor(props) {
    super(props);
    this.state = { error: null };
  }

  static getDerivedStateFromError(error) {
    return { error };
  }

  render() {
    if (this.state.error) {
      const settingsCopy = this.props.t.uiSettingsDetail;
      return (
        <div className="flex-1 flex flex-col w-full h-full relative z-10 px-16 py-12">
          <div className={`max-w-[800px] rounded-2xl border p-5 bg-white border-[#DDE3EA] text-[#1F1F1F] dark:bg-[#1F2023] dark:border-[#333537] dark:text-[#E8EAED]`}>
            <div className="text-[18px] font-semibold mb-2">{settingsCopy.settingsLoadFailed}</div>
            <div className={`text-[13px] leading-relaxed text-[#444746] dark:text-[#C4C7C5]`}>
              {String((this.state.error && this.state.error.message) || this.state.error)}
            </div>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}

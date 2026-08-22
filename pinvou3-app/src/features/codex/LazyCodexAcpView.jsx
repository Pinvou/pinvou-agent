import React, { lazy, Suspense } from 'react';

const loadCodexAcpWorkspace = () => import('./CodexAcpView.jsx')
  .then(module => ({ default: module.CodexAcpView }));

// The lazy chunk loads over the network on Web builds. A failed import must
// not unmount the whole app, and a retry must issue a fresh dynamic import.
class CodexAcpLoadErrorBoundary extends React.Component {
  constructor(props) {
    super(props);
    this.state = { error: null };
  }

  static getDerivedStateFromError(error) {
    return { error };
  }

  componentDidCatch(error, info) {
    console.error(
      '[codex] ACP workspace render failed',
      error && error.stack ? error.stack : error,
      info && info.componentStack ? info.componentStack : info,
    );
  }

  render() {
    if (this.state.error) {
      const copy = (this.props.t && this.props.t.uiCodex) || {};
      return (
        <div className="relative z-10 flex flex-1 items-center justify-center px-6">
          <div className="max-w-[440px] rounded-2xl border p-5 bg-white border-[#DDE3EA] text-[#1F1F1F] dark:bg-[#1F2023] dark:border-[#333537] dark:text-[#E8EAED]">
            <div className="text-[15px] font-semibold mb-2">{copy.viewLoadFailed}</div>
            <button
              type="button"
              className="mt-3 rounded-lg border border-[#DDE3EA] px-3 py-1.5 text-[13px] hover:bg-[#F5F7FA] dark:border-[#333537] dark:hover:bg-[#26282B]"
              onClick={() => this.props.onRetry()}
            >
              {copy.viewRetry}
            </button>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}

export function CodexAcpView({ t, ...props }) {
  const [attempt, setAttempt] = React.useState(0);
  // A per-attempt component ensures the retry remounts the lazy element and
  // re-runs the loader instead of replaying the failed import result.
  const Workspace = React.useMemo(
    () => lazy(loadCodexAcpWorkspace),
    [attempt],
  );
  return (
    <CodexAcpLoadErrorBoundary
      key={attempt}
      t={t}
      onRetry={() => setAttempt(value => value + 1)}
    >
      <Suspense fallback={(
        <div className="relative z-10 flex flex-1 items-center justify-center text-sm text-gray-500 dark:text-gray-300">
          {t.uiCodex.viewLoading}
        </div>
      )}>
        <Workspace key={attempt} t={t} {...props} />
      </Suspense>
    </CodexAcpLoadErrorBoundary>
  );
}

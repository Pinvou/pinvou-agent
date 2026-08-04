import React, { lazy, Suspense } from 'react';

const CodexAcpWorkspace = lazy(() => import('./CodexAcpView.jsx')
  .then(module => ({ default: module.CodexAcpView })));

export function CodexAcpView({ t, ...props }) {
  return (
    <Suspense fallback={(
      <div className="relative z-10 flex flex-1 items-center justify-center text-sm text-gray-500 dark:text-gray-300">
        {t.uiCodex.checking}
      </div>
    )}>
      <CodexAcpWorkspace t={t} {...props} />
    </Suspense>
  );
}

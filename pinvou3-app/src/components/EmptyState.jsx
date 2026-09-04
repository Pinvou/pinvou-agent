// Empty state for lists / panels: centered title + optional icon container + optional hint and action.
// tools / knowledge / remote-knowledge / conversation previously hand-rolled isomorphic blocks (up to 4 verbatim copies in one file).
// Per-view spacing and icon-container styling are aligned via className props; when the differences are too large,
// callers should still extract locally within their feature instead of forcing this component to fit.

/**
 * @param {{
 *   icon?: React.ReactNode,
 *   title: React.ReactNode,
 *   hint?: React.ReactNode,
 *   action?: React.ReactNode,
 *   className?: string,        // extra classes on the outer container (spacing, text-color tokens, ...)
 *   iconClassName?: string,    // icon container classes (size/radius/background); ignored without icon
 *   titleClassName?: string,
 *   hintClassName?: string,
 *   testId?: string,
 * }} props
 */
export function EmptyState({
  icon,
  title,
  hint,
  action,
  className = '',
  iconClassName = '',
  titleClassName = '',
  hintClassName = '',
  testId,
}) {
  return (
    <div data-testid={testId} className={`text-center ${className}`}>
      {icon ? <div className={`mx-auto grid place-items-center ${iconClassName}`}>{icon}</div> : null}
      <p className={titleClassName}>{title}</p>
      {hint ? <p className={hintClassName}>{hint}</p> : null}
      {action || null}
    </div>
  );
}

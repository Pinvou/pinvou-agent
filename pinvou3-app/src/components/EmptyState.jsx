// Empty state for lists / panels: centered title + optional icon container + optional hint and action.
// tools / knowledge / remote-knowledge / conversation previously hand-rolled isomorphic blocks (up to 4 verbatim copies in one file).
// Per-view spacing and icon-container styling are aligned via className props; when the differences are too large,
// callers should still extract locally within their feature instead of forcing this component to fit.

/**
 * @param {object} props - Empty state content and styling hooks.
 * @param {React.ReactNode} [props.icon] - Optional leading icon node.
 * @param {React.ReactNode} props.title - Primary title line.
 * @param {React.ReactNode} [props.hint] - Optional secondary hint line.
 * @param {React.ReactNode} [props.action] - Optional action node rendered under the hint.
 * @param {string} [props.className] - Extra classes on the outer container (spacing, text-color tokens, ...).
 * @param {string} [props.iconClassName] - Icon container classes (size/radius/background); ignored without icon.
 * @param {string} [props.titleClassName] - Extra classes for the title line.
 * @param {string} [props.hintClassName] - Extra classes for the hint line.
 * @param {string} [props.testId] - Test hook for the outer container.
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

// 列表 / 面板空态：居中标题 + 可选图标容器 + 可选提示与操作。此前 tools / knowledge /
// remote-knowledge / conversation 等各自手写同构块（同文件内最多 4 份逐字重复）。
// 各视图的留白与图标容器样式不同，经 className props 对齐原样式；差异过大时
// 调用方仍应做特征内本地抽取而不是硬塞进本组件。

/**
 * @param {{
 *   icon?: React.ReactNode,
 *   title: React.ReactNode,
 *   hint?: React.ReactNode,
 *   action?: React.ReactNode,
 *   className?: string,        // 外层容器追加类（留白、文字色 token 等）
 *   iconClassName?: string,    // 图标容器类（尺寸/圆角/底色），空 icon 时忽略
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

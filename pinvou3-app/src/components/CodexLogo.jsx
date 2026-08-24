import openaiIcon from '../brand-icons/openai.svg';

// 有 title 时作为图片暴露(role="img" + aria-label,掩码图无法内嵌 <title>);
// 无 title 时视为装饰性图形,对辅助技术隐藏。拆成两个分支让 role 与
// aria-label 静态可判定,避免条件 role 触发 useAriaPropsSupportedByRole。
export function CodexLogo({ className = 'h-4 w-4', title }) {
  const maskStyle = {
    WebkitMaskImage: `url("${openaiIcon}")`,
    maskImage: `url("${openaiIcon}")`,
    WebkitMaskPosition: 'center',
    maskPosition: 'center',
    WebkitMaskRepeat: 'no-repeat',
    maskRepeat: 'no-repeat',
    WebkitMaskSize: 'contain',
    maskSize: 'contain',
  };
  const cls = `inline-block shrink-0 bg-current ${className}`;
  if (title) {
    return <span role="img" aria-label={title} className={cls} style={maskStyle} />;
  }
  return <span aria-hidden="true" className={cls} style={maskStyle} />;
}

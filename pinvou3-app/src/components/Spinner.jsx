// 收敛此前 15+ 处手写的圆环加载指示。两种视觉形态：
//   arc —— 细环 + 同色系亮弧（border-current/20 border-t-current 一族）
//   top —— 实色环 + 顶部透明缺口（border-X border-t-transparent 一族）
// 颜色经 tone 枚举提供（避免 className 覆盖与默认色打架）；特殊配色继续用内联 span。
const TONES = {
  arc: {
    current: 'border-current/20 border-t-current',
    brand: 'border-blue-500/20 border-t-blue-500',
  },
  top: {
    brand: 'border-blue-500 border-t-transparent',
    ios: 'border-[#007AFF] border-t-transparent dark:border-[#0A84FF]',
    inverse: 'border-white/70 border-t-transparent',
    web: 'border-[#8AB4F8] border-t-transparent',
  },
};

/**
 * @param {{ size?: number, variant?: 'arc'|'top', tone?: string, className?: string }} props
 */
export function Spinner({ size = 14, variant = 'top', tone, className = '' }) {
  const variants = TONES[variant] || TONES.top;
  const toneClass = variants[tone] || (variant === 'arc' ? variants.current : variants.brand);
  return (
    <span
      aria-hidden="true"
      className={`inline-block shrink-0 animate-spin rounded-full border-2 motion-reduce:animate-none ${toneClass} ${className}`}
      style={{ width: size, height: size }}
    />
  );
}

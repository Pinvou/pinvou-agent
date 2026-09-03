// 状态小圆点：transcript / 工具行 / 会话列表此前 12+ 处逐字重复同一组
// 「1.5px 圆点 + 语义色」span（灰待机 / 蓝脉冲运行 / 红失败 / 翠绿完成 / 琥珀等待）。
// tone 枚举到具体色值（避免 className 覆盖与默认色打架）；色板之外的特例继续内联。
const TONES = {
  idle: 'bg-gray-300 dark:bg-gray-600',
  run: 'bg-blue-500 animate-pulse',
  fail: 'bg-red-500',
  ok: 'bg-emerald-500',
  okPulse: 'bg-emerald-500 animate-pulse',
  warn: 'bg-amber-500',
};

/**
 * @param {{ tone?: keyof typeof TONES, size?: 'sm'|'md', className?: string }} props
 */
export function StatusDot({ tone = 'idle', size = 'sm', className = '' }) {
  const sizeClass = size === 'md' ? 'w-2 h-2' : 'w-1.5 h-1.5';
  return (
    <span
      aria-hidden="true"
      className={`${sizeClass} shrink-0 rounded-full ${TONES[tone] || TONES.idle} ${className}`}
    />
  );
}

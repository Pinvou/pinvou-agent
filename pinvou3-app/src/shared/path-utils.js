// 路径工具：此前 codex 视图 / 工具商店 / 设置 / reader 各自内联 split(/[\\/]/).pop()
// 的 basename 提取，尾斜杠与 Windows 分隔符处理互有漂移，收敛到本模块。

/**
 * 取路径最后一段。'\\' 与 '/' 都视为分隔符（Windows 路径）。
 * @param {string|null} path - 文件 / 目录路径；空值按空串处理
 * @param {{ collapseTrailing?: boolean, fallback?: string }} [opts]
 *   collapseTrailing —— 先剥掉末尾分隔符（目录路径取目录名本身，如 /a/b/ → b）；
 *   fallback —— 结果为空串时的占位（如 t.unknownDirectory）。
 */
export function pathBasename(path, { collapseTrailing = false, fallback = '' } = {}) {
  let value = String(path || '');
  if (collapseTrailing) {
    let end = value.length;
    while (end > 0 && (value[end - 1] === '/' || value[end - 1] === '\\')) end -= 1;
    value = value.slice(0, end);
  }
  const segments = value.split(/[\\/]/);
  return segments[segments.length - 1] || fallback;
}

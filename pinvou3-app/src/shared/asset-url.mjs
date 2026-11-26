export function resolveAppAssetUrl(path, baseUrl = import.meta.env.BASE_URL) {
  const rawPath = String(path || '').trim();
  if (!rawPath || /^(?:[a-z][a-z\d+.-]*:|\/\/|#)/i.test(rawPath)) {
    return rawPath;
  }

  let base = String(baseUrl || '/');
  // 去掉结尾的斜杠（原 /\/+$/ 正则被 sonarjs 判为回溯风险；循环等价，
  // 逐段 slice 最坏 O(n²)，实际入参是短配置串，开销可忽略）。
  while (base.endsWith('/')) base = base.slice(0, -1);
  const normalizedBase = `${base}/`;
  return `${normalizedBase}${rawPath.replace(/^\/+/, '')}`;
}

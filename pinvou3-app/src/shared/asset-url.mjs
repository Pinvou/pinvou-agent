export function resolveAppAssetUrl(path, baseUrl = import.meta.env.BASE_URL) {
  const rawPath = String(path || '').trim();
  if (!rawPath || /^(?:[a-z][a-z\d+.-]*:|\/\/|#)/i.test(rawPath)) {
    return rawPath;
  }

  const normalizedBase = `${String(baseUrl || '/').replace(/\/+$/, '')}/`;
  return `${normalizedBase}${rawPath.replace(/^\/+/, '')}`;
}

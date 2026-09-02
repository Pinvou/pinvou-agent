export function isInternalBlankPageUrl(value) {
  return typeof value === 'string' && /^about:blank(?:#.*)?$/.test(value);
}

export function browserAddressValue(value) {
  if (isInternalBlankPageUrl(value)) return '';
  return typeof value === 'string' ? value : '';
}

export function browserTabLabel(tab, emptyLabel) {
  if (isInternalBlankPageUrl(tab?.url)) return emptyLabel;
  return tab?.title || tab?.url || emptyLabel;
}

export function shouldShowNativeBrowserSurface({
  statusResolved,
  running,
  url,
  suspended,
}) {
  return !!statusResolved
    && !!running
    && !suspended
    && !isInternalBlankPageUrl(url);
}

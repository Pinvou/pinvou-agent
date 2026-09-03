/**
 * 深浅色偏好的纯逻辑,主窗口/撕离窗/阅读器共用。
 *
 * 产品口径:
 * - 首次安装(无任何已保存偏好)跟随系统深浅;
 * - 系统偏好无法判断(matchMedia 不可用/查询失败)时按浅色;
 * - 用户在设置里显式选择后,system/light/dark 三档持久化,light/dark 不再跟随系统。
 *
 * 运行中监听系统切换见 hooks/useSystemDarkMode.js;持久化在桌面端走
 * settings.json 的 color_scheme 字段(prefs.rs),Web 端存浏览器 localStorage。
 */

/** Web 端浏览器本地存储 key;桌面端不使用(走 settings.json)。 */
export const COLOR_SCHEME_STORAGE_KEY = 'pinvou.web.theme';

/**
 * 任意落盘/存储值归一为 'system' | 'light' | 'dark';未知/缺失一律回跟随系统。
 * @param {string | null | undefined} value 落盘 color_scheme / localStorage / bridge 兜底对象里的任意值。
 * @returns {'system' | 'light' | 'dark'} 归一后的深浅偏好。
 */
export function normalizeColorScheme(value) {
  return value === 'light' || value === 'dark' ? value : 'system';
}

/** 系统当前是否深色。matchMedia 不可用时返回 false —— 判不出即浅色,是产品口径而非降级。 */
export function systemPrefersDark() {
  return typeof window !== 'undefined'
    && typeof window.matchMedia === 'function'
    && window.matchMedia('(prefers-color-scheme: dark)').matches;
}

/**
 * 已归一的偏好 → 实际渲染主题。
 * @param {string} scheme 'system' | 'light' | 'dark'
 * @param {boolean} [systemDark] 系统是否深色;缺省时现场检测一次,供无监听的一次性场景(阅读器)复用。
 * @returns {'light' | 'dark'} 实际渲染主题;system 跟随系统,判不出浅色。
 */
export function resolveTheme(scheme, systemDark = systemPrefersDark()) {
  if (scheme === 'light') return 'light';
  if (scheme === 'dark') return 'dark';
  return systemDark ? 'dark' : 'light';
}

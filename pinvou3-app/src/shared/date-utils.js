

// Intl.DateTimeFormat construction is expensive, and the sidebar calls
// formatSessionDate/formatDateGroupLabel for every session on each App
// render. Cache formatters by locale|opts (closed key space: three
// languages × two opts variants).
const formatterCache = new Map();
function cachedFormatter(locale, optsKey, opts) {
  const key = `${locale}|${optsKey}`;
  let fmt = formatterCache.get(key);
  if (!fmt) {
    fmt = new Intl.DateTimeFormat(locale, opts);
    formatterCache.set(key, fmt);
  }
  return fmt;
}

function formatSessionDate(ts, language) {
      if (!ts) return '';
      const d = new Date(ts);
      const now = new Date();
      let diff = now - d;
      if (diff < 0) diff = 0; // 时钟漂移/未来时间戳 → 当「刚刚」,不出现负数
      const L = {
        zh: { justNow: '刚刚', minsAgo: n => `${n} 分钟前`, hoursAgo: n => `${n} 小时前`, yesterday: '昨天', daysAgo: n => `${n} 天前`, locale: 'zh-CN' },
        ja: { justNow: 'たった今', minsAgo: n => `${n} 分前`, hoursAgo: n => `${n} 時間前`, yesterday: '昨日', daysAgo: n => `${n} 日前`, locale: 'ja-JP' },
        en: { justNow: 'Just now', minsAgo: n => `${n}m ago`, hoursAgo: n => `${n}h ago`, yesterday: 'Yesterday', daysAgo: n => `${n}d ago`, locale: 'en-US' },
      }[language] || { justNow: 'Just now', minsAgo: n => `${n}m ago`, hoursAgo: n => `${n}h ago`, yesterday: 'Yesterday', daysAgo: n => `${n}d ago`, locale: 'en-US' };
      // 今天之内:相对时间(刚刚 / X分钟前 / X小时前)——比绝对钟点更直观传达「多近」,
      // 且不同时间天然有区分度(不会同日多条都一样)。
      if (diff < 60000) return L.justNow;                                  // < 60 秒
      if (diff < 3600000) return L.minsAgo(Math.floor(diff / 60000));      // < 60 分钟
      if (diff < 86400000) return L.hoursAgo(Math.floor(diff / 3600000));  // < 24 小时
      if (diff < 172800000) return L.yesterday;                           // 24~47 小时
      const days = Math.floor(diff / 86400000);
      if (days < 7) return L.daysAgo(days);                               // 2~6 天
      // ≥7 天:日期;跨年带年份(只写月日会有歧义)
      return d.getFullYear() === now.getFullYear()
        ? cachedFormatter(L.locale, 'md', { month: 'short', day: 'numeric' }).format(d)
        : cachedFormatter(L.locale, 'ymd', { year: 'numeric', month: 'short', day: 'numeric' }).format(d);
    }

    // 侧栏任务列表按日期堆叠:本地日历日 key(YYYY-MM-DD),无时间戳归 'unknown'
    function localDateKey(ts) {
      if (!ts) return 'unknown';
      const d = new Date(ts);
      if (Number.isNaN(d.getTime())) return 'unknown';
      return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
    }

    // 日期组标题:今天/昨天用相对词,其余落本地化日期(跨年带年份)
    function formatDateGroupLabel(key, language) {
      const L = {
        zh: { today: '今天', yesterday: '昨天', unknown: '时间未知', locale: 'zh-CN' },
        ja: { today: '今日', yesterday: '昨日', unknown: '日時不明', locale: 'ja-JP' },
        en: { today: 'Today', yesterday: 'Yesterday', unknown: 'Unknown time', locale: 'en-US' },
      }[language] || { today: 'Today', yesterday: 'Yesterday', unknown: 'Unknown time', locale: 'en-US' };
      if (key === 'unknown') return L.unknown;
      if (key === localDateKey(Date.now())) return L.today;
      if (key === localDateKey(Date.now() - 86400000)) return L.yesterday;
      const [y, m, d] = key.split('-').map(Number);
      const date = new Date(y, m - 1, d);
      return y === new Date().getFullYear()
        ? cachedFormatter(L.locale, 'md', { month: 'short', day: 'numeric' }).format(date)
        : cachedFormatter(L.locale, 'ymd', { year: 'numeric', month: 'short', day: 'numeric' }).format(date);
    }

    // Absolute local date / datetime (YYYY-MM-DD / YYYY-MM-DD HH:mm). artifacts /
    // scheduled / knowledge previously each inlined padStart assembly of the same shapes; consolidated here.
    // `missing` is the sentinel for empty / invalid timestamps (views originally returned '' / '—' / copy).
    function formatLocalDate(ts, missing = '') {
      const key = localDateKey(ts);
      return key === 'unknown' ? missing : key;
    }
    function formatLocalDateTime(ts, missing = '') {
      const key = localDateKey(ts);
      if (key === 'unknown') return missing;
      const d = new Date(ts);
      return `${key} ${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
    }

export { formatSessionDate, localDateKey, formatDateGroupLabel, formatLocalDate, formatLocalDateTime };

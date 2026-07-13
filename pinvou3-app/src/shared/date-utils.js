

function formatSessionDate(ts, language) {
      if (!ts) return '';
      const d = new Date(typeof ts === 'number' ? ts : ts);
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
      const opts = d.getFullYear() === now.getFullYear()
        ? { month: 'short', day: 'numeric' }
        : { year: 'numeric', month: 'short', day: 'numeric' };
      return d.toLocaleDateString(L.locale, opts);
    }

    // ==========================================
    // Search View (Gemini Style)
    // ==========================================

export { formatSessionDate };

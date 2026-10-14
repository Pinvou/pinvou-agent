// Shared inline status chip for the settings feature. Three historical pill/badge
// implementations (composer-shared statusBadge, SettingsView Tag, ProvidersSection badge)
// rendered the same kind of tiny inline status label with subtly different chrome; this
// component keeps every variant's class output byte-identical to its original so swapping
// them in is visually neutral.
//   variant="dot"   - dot pill (composer: connected/builtin tags). tone: 'green' | 'blue'.
//   variant="tag"   - SettingsView list tag. tone: 'green' | 'gray'.
//   variant="badge" - ProvidersSection provider badges: free-form tone class string
//                     (passing a class keeps the original tone palette local to that view).
const DOT_TONES = {
  green: {
    chip: 'text-[#34C759] bg-[#34C759]/10',
    dot: 'bg-[#34C759]',
  },
  blue: {
    chip: 'text-[#007AFF] dark:text-[#5AC8FA] bg-[#007AFF]/10 dark:bg-[#0A84FF]/15',
    dot: 'bg-[#007AFF] dark:bg-[#5AC8FA]',
  },
};

const TAG_TONES = {
  green: 'bg-[#34C759]/15 text-[#248A3D]',
  gray: 'bg-[#E5E5EA] text-[#636366] dark:bg-white/[0.08] dark:text-[#C7C7CC]',
};

export function StatusChip({ variant = 'tag', tone = 'green', toneClass = '', children }) {
  if (variant === 'dot') {
    const dotTone = DOT_TONES[tone] || DOT_TONES.green;
    return (
      <span className={`shrink-0 inline-flex items-center gap-1 text-[10px] font-semibold ${dotTone.chip} px-2 py-0.5 rounded-full leading-none`}>
        <span className={`w-1.5 h-1.5 rounded-full ${dotTone.dot}`} />
        {children}
      </span>
    );
  }
  if (variant === 'badge') {
    return <span className={`rounded-md px-1.5 py-0.5 text-[10px] font-semibold ${toneClass}`}>{children}</span>;
  }
  return <span className={`shrink-0 text-[12px] px-2 py-0.5 rounded-md ${TAG_TONES[tone] || TAG_TONES.green}`}>{children}</span>;
}

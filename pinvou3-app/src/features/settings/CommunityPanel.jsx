import { useState } from 'react';
import { Check, Copy, ExternalLink, MessageCircle } from '../../components/icons.jsx';
import { copyClipboardText } from '../../shared/clipboard.js';

/** @param {{ label: string }} props - Localized placeholder label. */
function QrPlaceholder({ label }) {
  return (
    <div
      data-testid="community-qr-placeholder"
      className="relative flex h-[196px] w-[196px] shrink-0 items-center justify-center overflow-hidden rounded-[22px] border border-black/[0.08] bg-white p-4 shadow-sm dark:border-white/[0.10]"
    >
      <div aria-hidden="true" className="absolute inset-4 grid grid-cols-5 grid-rows-5 gap-1 opacity-15">
        {Array.from({ length: 25 }, (_, index) => (
          <span
            key={index}
            className={`${[0, 1, 4, 5, 6, 8, 9, 12, 14, 16, 18, 20, 21, 23, 24].includes(index) ? 'bg-[#1C1C1E]' : 'bg-transparent'} rounded-[2px]`}
          />
        ))}
      </div>
      <span className="relative rounded-full bg-white/95 px-3 py-1.5 text-[12px] font-semibold text-[#636366] shadow-sm dark:bg-[#F2F2F7]">
        {label}
      </span>
    </div>
  );
}

/**
 * @param {{
 *   copy: Record<string, string>,
 *   groupName: string,
 *   groupNumber: string,
 *   qrImageSrc: string,
 *   onOpenDiscussions: () => void,
 * }} props - Community content and actions.
 */
export function CommunityPanel({ copy, groupName, groupNumber, qrImageSrc, onOpenDiscussions }) {
  const [copied, setCopied] = useState(false);

  const copyGroupNumber = async () => {
    if (!groupNumber) return;
    const success = await copyClipboardText(groupNumber);
    if (!success) return;
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1600);
  };

  return (
    <div data-community-panel="true" className="space-y-4">
      <section className="flex items-center gap-6 rounded-[24px] bg-gradient-to-br from-[#EAF7FF] to-[#F3F0FF] p-6 max-sm:flex-col dark:from-[#102B3A] dark:to-[#22213B]">
        {qrImageSrc ? (
          <img
            data-testid="community-qr-image"
            src={qrImageSrc}
            alt={copy.communityQrAlt}
            className="h-[224px] w-[224px] shrink-0 rounded-[22px] bg-white object-contain shadow-sm max-sm:h-auto max-sm:w-full max-sm:max-w-[280px]"
          />
        ) : (
          <QrPlaceholder label={copy.communityQrPlaceholder} />
        )}
        <div className="min-w-0 flex-1 max-sm:text-center">
          <div className="mb-3 inline-flex items-center gap-1.5 rounded-full bg-white/80 px-2.5 py-1 text-[12px] font-semibold text-[#007AFF] dark:bg-white/[0.08] dark:text-[#64D2FF]">
            <MessageCircle size={14} />
            {copy.communityChannelTag}
          </div>
          <h2 data-testid="community-group-name" className="text-[20px] font-semibold leading-6">{groupName}</h2>
          <p className="mt-2 text-[13px] leading-5 text-[#636366] dark:text-[#C7C7CC]">{copy.communityQrHint}</p>
          <div className="mt-4 flex items-center gap-2 max-sm:justify-center">
            <span className="text-[13px] text-[#8A8A8E] dark:text-[#98989D]">{copy.communityGroupLabel}</span>
            <span data-testid="community-group-number" className="text-[14px] font-semibold tabular-nums">
              {groupNumber || copy.communityGroupPending}
            </span>
            <button
              type="button"
              data-testid="community-copy-group"
              onClick={copyGroupNumber}
              disabled={!groupNumber}
              aria-label={copy.communityCopyGroup}
              className="flex h-8 w-8 items-center justify-center rounded-full text-[#007AFF] hover:bg-[#007AFF]/10 disabled:cursor-not-allowed disabled:opacity-30 dark:text-[#64D2FF]"
            >
              {copied ? <Check size={16} /> : <Copy size={16} />}
            </button>
          </div>
          {copied && <div role="status" className="mt-1 text-[12px] text-[#248A3D] dark:text-[#30D158]">{copy.communityCopied}</div>}
        </div>
      </section>

      <div className="rounded-[18px] bg-white px-4 py-3 text-[12px] leading-5 text-[#636366] dark:bg-[#2C2C2E] dark:text-[#C7C7CC]">
        {copy.communitySupportNotice}
      </div>

      <button
        type="button"
        onClick={onOpenDiscussions}
        className="flex min-h-[58px] w-full items-center justify-between gap-3 rounded-[18px] bg-white px-4 py-3 text-left text-[15px] text-[#007AFF] hover:bg-black/[0.035] dark:bg-[#2C2C2E] dark:text-[#64D2FF] dark:hover:bg-white/[0.05]"
      >
        <span className="font-semibold">{copy.communityDiscussions}</span>
        <ExternalLink size={17} />
      </button>
    </div>
  );
}

import { useState } from 'react';
import { Check, Copy, ExternalLink, MessageCircle } from '../../components/icons.jsx';
import { copyClipboardText } from '../../shared/clipboard.js';
import { COMMUNITY_QQ_GROUP_NAME, COMMUNITY_QQ_GROUP_NUMBER, COMMUNITY_QQ_QR_IMAGE_SRC } from './community-config.js';

/**
 * @param {{
 *   copy: Record<string, string>,
 *   onOpenDiscussions: () => void,
 * }} props - Community content and actions. The QQ group identity constants are
 * owned by this panel (community-config.js) and rendered directly; the sole
 * caller (SettingsView) never passed overrides, so the prop slots were dead
 * surface. onOpenDiscussions stays injected so the external-url bridge seam is
 * wired by the caller.
 */
export function CommunityPanel({ copy, onOpenDiscussions }) {
  const [copied, setCopied] = useState(false);

  const copyGroupNumber = async () => {
    const success = await copyClipboardText(COMMUNITY_QQ_GROUP_NUMBER);
    if (!success) return;
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1600);
  };

  return (
    <div data-community-panel="true" className="space-y-4">
      <section className="flex items-center gap-6 rounded-[24px] bg-gradient-to-br from-[#EAF7FF] to-[#F3F0FF] p-6 max-sm:flex-col dark:from-[#102B3A] dark:to-[#22213B]">
        <img
          data-testid="community-qr-image"
          src={COMMUNITY_QQ_QR_IMAGE_SRC}
          alt={copy.communityQrAlt}
          className="h-[224px] w-[224px] shrink-0 rounded-[22px] bg-white object-contain shadow-sm max-sm:h-auto max-sm:w-full max-sm:max-w-[280px]"
        />
        <div className="min-w-0 flex-1 max-sm:text-center">
          <div className="mb-3 inline-flex items-center gap-1.5 rounded-full bg-white/80 px-2.5 py-1 text-[12px] font-semibold text-[#007AFF] dark:bg-white/[0.08] dark:text-[#64D2FF]">
            <MessageCircle size={14} />
            {copy.communityChannelTag}
          </div>
          <h2 data-testid="community-group-name" className="text-[20px] font-semibold leading-6">{COMMUNITY_QQ_GROUP_NAME}</h2>
          <p className="mt-2 text-[13px] leading-5 text-[#636366] dark:text-[#C7C7CC]">{copy.communityQrHint}</p>
          <div className="mt-4 flex items-center gap-2 max-sm:justify-center">
            <span className="text-[13px] text-[#8A8A8E] dark:text-[#98989D]">{copy.communityGroupLabel}</span>
            <span data-testid="community-group-number" className="text-[14px] font-semibold tabular-nums">
              {COMMUNITY_QQ_GROUP_NUMBER}
            </span>
            <button
              type="button"
              data-testid="community-copy-group"
              onClick={copyGroupNumber}
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

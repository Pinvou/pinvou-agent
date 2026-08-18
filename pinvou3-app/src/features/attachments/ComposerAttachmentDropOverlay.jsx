import React from 'react';
import { AttachmentDropOverlay } from './AttachmentDropOverlay.jsx';
import { useAttachmentDrop } from './useAttachmentDrop.js';

export function ComposerAttachmentDropOverlay({ enabled, onFiles, dark, variant, copy }) {
  const active = useAttachmentDrop({ enabled, onFiles });
  return (
    <AttachmentDropOverlay
      active={active}
      dark={dark}
      variant={variant}
      releaseLabel={copy.dropRelease}
      webTitle={copy.dropWebTitle}
      webHint={copy.dropWebHint}
    />
  );
}

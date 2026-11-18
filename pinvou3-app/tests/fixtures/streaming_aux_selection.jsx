import React from 'react';
import { createRoot } from 'react-dom/client';

let snapshot = {
  chatItems: [
    { id: 'user', type: 'user', text: 'Question' },
    { id: 'assistant', type: 'assistant', streaming: true,
      text: Array.from({ length: 20 }, (_, index) => `Line ${index} selected paragraph.`).join('\n\n') },
  ],
  busy: true,
  queued: [],
};
const listeners = new Set();
window.TauriBridge = {
  available: true,
  auxChat: { ensure: async () => 'aux-fixture', snapshot: () => snapshot },
  state: {
    subscribeMany(domains, callback) {
      if (domains.includes('chat')) listeners.add(callback);
      return () => listeners.delete(callback);
    },
  },
};
// The bridge must exist before the real panel's module captures its adapter.
const { AuxChatPanel } = await import('../../src/features/aux-chat/AuxChatPanel.jsx');
const t = { uiAuxChat: {}, uiConversation: { elapsed: ms => String(ms) } };
createRoot(document.getElementById('root')).render(
  <AuxChatPanel sessionId="streaming-fixture" t={t} theme="light" />,
);
window.auxSelectionFixture = {
  grow() {
    const [user, assistant] = snapshot.chatItems;
    snapshot = { ...snapshot, chatItems: [user, { ...assistant,
      text: `${assistant.text}\n\n${'More output.\n\n'.repeat(10)}` }] };
    for (const listener of listeners) listener();
  },
};

import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { flushSync } from 'react-dom';
import { ConversationTimeline } from '../../src/features/conversation/ConversationTimeline.jsx';

const host = document.getElementById('root');
const root = createRoot(host);
let updateTurns = null;
let updateBusy = null;
const scrollElementRef = { current: null };
const followOutputRef = { current: true };

let statefulMountSequence = 0;
function StatefulProbe({ id }) {
  const [expanded, setExpanded] = useState(false);
  const [mountId] = useState(() => ++statefulMountSequence);
  return (
    <button
      type="button"
      data-stateful-probe={id}
      data-mount-id={mountId}
      aria-pressed={expanded}
      onClick={() => setExpanded(value => !value)}
      style={{ minHeight: 620 }}
    >
      {expanded ? 'expanded' : 'collapsed'}
    </button>
  );
}

function renderPerformanceItem(item) {
  if (!item.stateful) return undefined;
  return <StatefulProbe id={item.id} />;
}

function PerformanceTimeline() {
  const [visibleTurns, setVisibleTurns] = useState([]);
  const [busy, setBusy] = useState(false);
  updateTurns = setVisibleTurns;
  updateBusy = setBusy;
  return (
    <div
      ref={scrollElementRef}
      style={{ height: 800, overflowY: 'auto', overflowAnchor: 'none' }}
    >
      <div data-testid="timeline-header" style={{ height: 96 }} />
      <ConversationTimeline
        turns={visibleTurns}
        sessionId="performance-session"
        scrollElementRef={scrollElementRef}
        followOutputRef={followOutputRef}
        busy={busy}
        turnGapPx={28}
        renderItem={renderPerformanceItem}
      />
    </div>
  );
}

flushSync(() => root.render(<PerformanceTimeline />));

function turns(count, suffix = '') {
  return Array.from({ length: count }, (_, index) => ({
    id: `turn-${index}`,
    status: 'completed',
    userText: `User message ${index}`,
    items: [{
      id: `answer-${index}`,
      type: 'agent_message',
      status: 'completed',
      text: `Answer ${index}: Markdown text for the performance experiment with **bold**, \`code\` and a list.${suffix}`,
    }],
  }));
}

function commit(nextTurns) {
  const startedAt = performance.now();
  flushSync(() => updateTurns(nextTurns));
  return performance.now() - startedAt;
}

function commitConversation(nextTurns, nextBusy) {
  const startedAt = performance.now();
  flushSync(() => {
    updateBusy(nextBusy);
    updateTurns(nextTurns);
  });
  return performance.now() - startedAt;
}

function nextPaint() {
  return new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
}

function renderedTurnIndexes() {
  return [...host.querySelectorAll('[data-conversation-turn]')]
    .map(element => Number(String(element.dataset.conversationTurn || '').replace('turn-', '')))
    .filter(Number.isFinite);
}

window.__PINVOU_TIMELINE_PERFORMANCE__ = {
  runInitialLongMount() {
    const container = document.createElement('div');
    document.body.append(container);
    const initialRoot = createRoot(container);
    const initialScrollRef = { current: null };
    const initialFollowRef = { current: true };
    flushSync(() => initialRoot.render(
      <div ref={initialScrollRef} style={{ height: 800, overflowY: 'auto', overflowAnchor: 'none' }}>
        <ConversationTimeline
          turns={turns(1000)}
          sessionId="initial-long-session"
          scrollElementRef={initialScrollRef}
          followOutputRef={initialFollowRef}
        />
      </div>,
    ));
    const result = {
      virtualized: Boolean(container.querySelector('[data-conversation-virtual-timeline]')),
      nodes: container.querySelectorAll('*').length,
    };
    flushSync(() => initialRoot.unmount());
    container.remove();
    return result;
  },
  async run(count) {
    followOutputRef.current = true;
    flushSync(() => updateTurns([]));
    scrollElementRef.current.scrollTop = 0;
    const baseTurns = turns(count);
    const mountMs = commit(baseTurns);
    scrollElementRef.current.scrollTop = scrollElementRef.current.scrollHeight;
    await nextPaint();
    const bottomDistanceAfterMount = scrollElementRef.current.scrollHeight
      - scrollElementRef.current.clientHeight
      - scrollElementRef.current.scrollTop;
    const nodes = host.querySelectorAll('*').length;
    const shortContentVisibility = count <= 80
      ? getComputedStyle(host.querySelector('[data-conversation-turn]')).contentVisibility
      : null;
    const updatedTurns = baseTurns.slice();
    const tail = updatedTurns[updatedTurns.length - 1];
    updatedTurns[updatedTurns.length - 1] = {
      ...tail,
      items: [{ ...tail.items[0], text: `${tail.items[0].text} tail-update` }],
    };
    const tailUpdateMs = commit(updatedTurns);
    await nextPaint();
    const tailUpdated = [...host.querySelectorAll('[data-conversation-turn]')]
      .some(element => element.dataset.conversationTurn === `turn-${count - 1}` && element.textContent.includes('tail-update'));
    const bottomIndexes = renderedTurnIndexes();
    const bottomRows = [...host.querySelectorAll('[data-conversation-virtual-row]')];
    const virtualContentVisibility = count > 80
      ? [...host.querySelectorAll('[data-conversation-turn]')].map(element => getComputedStyle(element).contentVisibility)
      : [];
    const measuredGap = bottomRows.length > 1
      ? Number(bottomRows.at(-1).dataset.conversationVirtualStart)
        - Number(bottomRows.at(-2).dataset.conversationVirtualStart)
        - Number(bottomRows.at(-2).dataset.conversationVirtualSize)
      : null;
    let maxResidentNodes = host.querySelectorAll('*').length;
    // Match the real Chat/Codex scroll handlers: a user-initiated upward scroll
    // disables bottom following before the viewport moves.
    followOutputRef.current = false;
    scrollElementRef.current.scrollTop = 0;
    await nextPaint();
    const topScrollTop = scrollElementRef.current.scrollTop;
    const topIndexes = renderedTurnIndexes();
    const firstTurn = host.querySelector('[data-conversation-turn="turn-0"]');
    const firstRow = firstTurn?.closest('[data-conversation-virtual-row]');
    const firstRowTransform = firstRow ? getComputedStyle(firstRow).transform : '';
    const topTransformOffset = firstRowTransform && firstRowTransform !== 'none'
      ? new DOMMatrixReadOnly(firstRowTransform).m42
      : null;
    maxResidentNodes = Math.max(maxResidentNodes, host.querySelectorAll('*').length);
    let maxResidentVirtualRows = host.querySelectorAll('[data-conversation-virtual-row]').length;
    scrollElementRef.current.scrollTop = scrollElementRef.current.scrollHeight / 2;
    await nextPaint();
    const middleScrollTop = scrollElementRef.current.scrollTop;
    const middleIndexes = renderedTurnIndexes();
    maxResidentNodes = Math.max(maxResidentNodes, host.querySelectorAll('*').length);
    maxResidentVirtualRows = Math.max(
      maxResidentVirtualRows,
      bottomRows.length,
      host.querySelectorAll('[data-conversation-virtual-row]').length,
    );
    followOutputRef.current = true;
    return {
      count,
      mountMs,
      tailUpdateMs,
      tailUpdated,
      nodes,
      maxResidentNodes,
      maxResidentVirtualRows,
      virtualRows: Number(host.querySelector('[data-conversation-virtual-timeline]')?.dataset.conversationVirtualRowCount || 0),
      scrollHeight: scrollElementRef.current.scrollHeight,
      topIndexes,
      topScrollTop,
      middleIndexes,
      middleScrollTop,
      bottomIndexes,
      bottomDistanceAfterMount,
      measuredGap,
      shortContentVisibility,
      virtualContentVisibility,
      topTransformOffset,
      scrollMargin: Number(host.querySelector('[data-conversation-virtual-timeline]')?.dataset.conversationScrollMargin || 0),
    };
  },
  async runCompletionMigration() {
    const baseTurns = turns(100);
    baseTurns[baseTurns.length - 1] = {
      ...baseTurns[baseTurns.length - 1],
      status: 'running',
      completedAt: null,
      items: [{ id: 'stateful-tail', type: 'custom', status: 'running', stateful: true }],
    };
    commitConversation(baseTurns, true);
    scrollElementRef.current.scrollTop = scrollElementRef.current.scrollHeight;
    await nextPaint();
    const beforeButton = host.querySelector('[data-stateful-probe="stateful-tail"]');
    beforeButton.click();
    await nextPaint();
    const before = {
      button: beforeButton,
      mountId: beforeButton.dataset.mountId,
    };
    const completedTurns = baseTurns.slice();
    completedTurns[completedTurns.length - 1] = {
      ...baseTurns[baseTurns.length - 1],
      status: 'completed',
      completedAt: Date.now(),
      items: [{ ...baseTurns[baseTurns.length - 1].items[0], status: 'completed' }],
    };
    commitConversation(completedTurns, false);
    const immediateButton = host.querySelector('[data-stateful-probe="stateful-tail"]');
    const immediateScrollHeight = scrollElementRef.current.scrollHeight;
    const immediateBottomDistance = immediateScrollHeight
      - scrollElementRef.current.clientHeight
      - scrollElementRef.current.scrollTop;
    await nextPaint();
    const afterButton = host.querySelector('[data-stateful-probe="stateful-tail"]');
    return {
      sameElement: before.button === immediateButton && immediateButton === afterButton,
      statePreserved: afterButton?.getAttribute('aria-pressed') === 'true',
      mountPreserved: afterButton?.dataset.mountId === before.mountId,
      immediateBottomDistance,
      bottomDistance: scrollElementRef.current.scrollHeight
        - scrollElementRef.current.clientHeight
        - scrollElementRef.current.scrollTop,
    };
  },
};

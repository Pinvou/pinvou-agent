// Serializes React UI publication behind the native browser child's hide ACK.
// A channel is latest-wins; a session change also invalidates every pending
// publication, even when it came from another channel.
export function createNativeSurfaceTransitionGate({
  acquireHide,
  getContext,
  onError = () => {},
}) {
  if (typeof acquireHide !== 'function') throw new TypeError('acquireHide must be a function');
  if (typeof getContext !== 'function') throw new TypeError('getContext must be a function');

  const revisions = new Map();
  const channelTails = new Map();
  let disposed = false;

  const issue = (channel) => {
    const revision = (revisions.get(channel) || 0) + 1;
    revisions.set(channel, revision);
    return { channel, revision };
  };

  const isCurrent = (ticket, context, guardSession) => {
    if (disposed || revisions.get(ticket.channel) !== ticket.revision) return false;
    if (!guardSession) return true;
    return (getContext() || {}).sessionId === context.sessionId;
  };

  const release = (lease) => {
    try {
      lease?.release?.();
    } catch (error) {
      onError(error);
    }
  };

  const publishCurrent = (ticket, context, guardSession, publish, lease) => {
    if (!isCurrent(ticket, context, guardSession)) {
      release(lease);
      return false;
    }

    let result;
    try {
      result = publish({
        context,
        isCurrent: () => isCurrent(ticket, context, guardSession),
      });
    } catch (error) {
      release(lease);
      throw error;
    }

    if (result && typeof result.then === 'function') {
      return Promise.resolve(result).then(
        (value) => {
          release(lease);
          return value === false ? false : true;
        },
        (error) => {
          release(lease);
          throw error;
        },
      );
    }

    release(lease);
    return result === false ? false : true;
  };

  return {
    invalidate(channel = 'default') {
      issue(channel);
    },

    dispose() {
      disposed = true;
      revisions.clear();
      channelTails.clear();
    },

    run(publish, {
      channel = 'default',
      hideMode = 'visible',
      guardSession = true,
      serialize = false,
    } = {}) {
      if (typeof publish !== 'function') throw new TypeError('publish must be a function');
      const ticket = issue(channel);
      const context = { ...(getContext() || {}) };
      const shouldHide = hideMode === 'workspace'
        ? !!context.hasWorkspace
        : hideMode !== 'none' && !!context.visible;

      if (!shouldHide && !serialize) {
        return publishCurrent(ticket, context, guardSession, publish, null);
      }

      const hideReady = shouldHide
        ? Promise.resolve().then(() => acquireHide(context.sessionId))
        : Promise.resolve(null);
      const predecessor = serialize
        ? (channelTails.get(channel) || Promise.resolve())
        : Promise.resolve();
      const task = Promise.all([
        hideReady,
        predecessor.catch(() => false),
      ]).then(([lease]) => publishCurrent(
        ticket,
        context,
        guardSession,
        publish,
        lease,
      ));
      const settled = task.catch((error) => {
        onError(error);
        return false;
      });
      if (serialize) {
        channelTails.set(channel, settled);
        void settled.finally(() => {
          if (channelTails.get(channel) === settled) channelTails.delete(channel);
        });
      }
      return settled;
    },
  };
}

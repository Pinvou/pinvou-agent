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
          return value !== false;
        },
        (error) => {
          release(lease);
          throw error;
        },
      );
    }

    release(lease);
    return result !== false;
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
      const context = { ...getContext() };
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

// Once a guarded publication callback has started, it may have queued React
// state even when it later returns false or throws (for example, a session RPC
// can fail after the route was changed). Keep the native hide lease until a
// layout commit has acknowledged those queued mutations on every outcome.
// The transition gate itself owns the distinct path where a stale ticket means
// the publication callback was never invoked.
export async function settleBrowserUiPublicationAfterCommit({
  publish,
  waitForCommit,
  onSettled = () => {},
}) {
  if (typeof publish !== 'function') throw new TypeError('publish must be a function');
  if (typeof waitForCommit !== 'function') {
    throw new TypeError('waitForCommit must be a function');
  }
  if (typeof onSettled !== 'function') throw new TypeError('onSettled must be a function');

  let value;
  let publicationRejected = false;
  let publicationError;
  try {
    value = await publish();
  } catch (error) {
    publicationRejected = true;
    publicationError = error;
  }

  try {
    await waitForCommit();
  } finally {
    onSettled();
  }

  if (publicationRejected) throw publicationError;
  return value;
}

// Native hide IPC can fail transiently while the platform host is being
// attached/detached. Retry exactly once, but only while the caller's captured
// visibility intent is still current. This guard is what prevents an old hide
// from landing after a newer show or session switch.
export async function hideNativeSurfaceWithRetry({
  hide,
  isCurrent,
  onError = () => {},
  waitBeforeRetry = () => Promise.resolve(),
}) {
  if (typeof hide !== 'function') throw new TypeError('hide must be a function');
  if (typeof isCurrent !== 'function') throw new TypeError('isCurrent must be a function');
  if (typeof onError !== 'function') throw new TypeError('onError must be a function');
  if (typeof waitBeforeRetry !== 'function') {
    throw new TypeError('waitBeforeRetry must be a function');
  }

  try {
    return await hide({ attempt: 1 });
  } catch (error) {
    const willRetry = isCurrent();
    onError(error, { attempt: 1, willRetry });
    if (!willRetry) throw error;

    await waitBeforeRetry();
    if (!isCurrent()) throw error;

    try {
      return await hide({ attempt: 2 });
    } catch (retryError) {
      onError(retryError, { attempt: 2, willRetry: false });
      throw retryError;
    }
  }
}

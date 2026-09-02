export function createPetActivationState(activePet = null) {
  return {
    requestSequence: 0,
    pendingId: null,
    activePet,
  };
}

function reportLoadError(onError, error, petId, fallback) {
  if (typeof onError === 'function') onError(error, { petId, fallback });
}

/**
 * Resolve, load, decode, and atomically commit one pet selection.
 * All environment-specific work is injected so request ordering is unit-testable.
 */
export async function loadActivePet(requestedId, {
  state,
  defaultPetId = 'lingling',
  normalizeId,
  resolvePet,
  loadAtlas,
  decodeImage,
  commit,
  onActivationFailed,
  onError,
}) {
  const petId = normalizeId(requestedId);
  if (state.pendingId === petId) {
    return state.activePet;
  }
  if (state.activePet?.id === petId) {
    // 重选当前宠物必须作废仍在途的其他切换：持久化层此刻已经写回当前
    // 宠物，若旧请求稍后完成并提交，显示与持久化会分叉。
    if (state.pendingId !== null) {
      state.requestSequence += 1;
      state.pendingId = null;
    }
    if (state.pendingId === null && typeof onActivationFailed === 'function') {
      onActivationFailed(false);
    }
    return state.activePet;
  }

  const requestId = ++state.requestSequence;
  state.pendingId = petId;

  const isLatest = () => requestId === state.requestSequence;
  const attempt = async (id) => {
    const metadata = resolvePet(id);
    const sheetUrl = await loadAtlas(metadata);
    if (typeof sheetUrl !== 'string' || !sheetUrl.trim()) {
      throw new Error(`Pet atlas loader returned an invalid URL for ${id}`);
    }
    await decodeImage(sheetUrl);
    return { ...metadata, sheetUrl };
  };
  const commitIfLatest = (pet) => {
    if (!isLatest()) return state.activePet;
    state.activePet = pet;
    state.pendingId = null;
    commit(pet);
    if (typeof onActivationFailed === 'function') onActivationFailed(false);
    return pet;
  };

  try {
    return commitIfLatest(await attempt(petId));
  } catch (error) {
    reportLoadError(onError, error, petId, false);
    // 回退默认宠物的条件是「当前什么都没显示」而非 startup 标志：
    // 事件先于启动读取到达且加载失败时，同样不能让窗口永远空白。
    if (!isLatest() || state.activePet !== null || petId === defaultPetId) {
      if (isLatest()) state.pendingId = null;
      if (isLatest() && typeof onActivationFailed === 'function') {
        onActivationFailed(state.activePet === null);
      }
      return state.activePet;
    }
  }

  state.pendingId = defaultPetId;
  try {
    return commitIfLatest(await attempt(defaultPetId));
  } catch (error) {
    reportLoadError(onError, error, defaultPetId, true);
    if (isLatest()) {
      state.pendingId = null;
      if (typeof onActivationFailed === 'function') {
        onActivationFailed(state.activePet === null);
      }
    }
    return state.activePet;
  }
}

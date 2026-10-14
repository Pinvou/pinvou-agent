/**
 * personas feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim classic-script artifact; strict mode is part of the payload
  "use strict";
  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting statements would diverge from the artifact
  const registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["personas"] = function (context) {let pinvouSharedtauriPersonasCache = null;
function pinvouSharedtauriPersonas() {
  if (!pinvouSharedtauriPersonasCache) pinvouSharedtauriPersonasCache = window.PinvouBridgeShared.create("tauriPersonas", { state, notify, personaPoolCache: { get value() { return personaPoolCache; }, set value(v) { personaPoolCache = v; } }, invoke, deletedPersonaIds, sessionStates, lastEquippedSid: { get value() { return lastEquippedSid; }, set value(v) { lastEquippedSid = v; } }, runOnSession, addChatItem, mountedCollectionDraftTarget: { get value() { return mountedCollectionDraftTarget; }, set value(v) { mountedCollectionDraftTarget = v; } }, ensureSession, mountedCollectionUpdate: { get value() { return mountedCollectionUpdate; }, set value(v) { mountedCollectionUpdate = v; } }, addSystemItem, bt, timeStr, isDefaultChatTitle, personaPlaceholderTitles, personaSyncSeq: { get value() { return personaSyncSeq; }, set value(v) { personaSyncSeq = v; } }, itemIdSeq: { get value() { return context.itemIdSeq; }, set value(v) { context.itemIdSeq = v; } } });
  return pinvouSharedtauriPersonasCache;
}


    const state = context.state;
    const sessionStates = context.sessionStates;
    const notify = context.notify;
    const invoke = context.invoke;
    const bt = context.bt;
    const addSystemItem = context.addSystemItem;
    const addChatItem = context.addChatItem;
    const timeStr = context.timeStr;
    const ensureSession = context.ensureSession;
    const runOnSession = context.runOnSession;
    const personaPlaceholderTitles = context.personaPlaceholderTitles;
    const isDefaultChatTitle = context.isDefaultChatTitle;
    let personaPoolCache = [];
  // ── 卡片池: 专家面具加持 ─────────────────────────────────────────
  // 懒加载全部专家卡(1078 张),前端缓存供 facet/搜索。只拉一次。
async function loadPersonas() { return pinvouSharedtauriPersonas().loadPersonas(); }

  // ── 用户自创卡 CRUD(写盘后刷新缓存) ──
async function createPersona(input) { return pinvouSharedtauriPersonas().createPersona(input); }
async function updatePersona(personaId, input) { return pinvouSharedtauriPersonas().updatePersona(personaId, input); }
async function deletePersona(personaId) { return pinvouSharedtauriPersonas().deletePersona(personaId); }
  // 给当前 session 加持一张专家面具。后端存 persona_id + 每 turn 注入人设;
  // 前端记 activePersona(挂件) + 发一条系统消息播报。
  // 取专家显示名(兼容 Side A 的 cn_name / Side B 的 name)。
function personaName(p) { return pinvouSharedtauriPersonas().personaName(p); }
  // 记一条卡牌事件到时间线 sidecar(pos=当前 messages 数),并落盘。重载历史时按 pos 插回。
function recordPersonaEvent(ev) { return pinvouSharedtauriPersonas().recordPersonaEvent(ev); }
async function equipPersona(personaId) { return pinvouSharedtauriPersonas().equipPersona(personaId); }
  // 摘下当前 session 的专家面具。
  async function unequipPersona() {
    if (!state.activeSessionId) return;
    // 入口捕获触发会话：await 期间切走，卸下播报不得写进别的会话（审计）。
    const sid = state.activeSessionId;
    try { await invoke("unequip_persona", { sessionId: state.activeSessionId }); } catch { /* 忽略,前端照样摘 */ }
    if (sid !== state.activeSessionId) return; // 已切走：不写当前显示
    personaSyncSeq++; // 权威写前 bump：作废在途 syncActivePersona 的旧快照(审计补充)
    // 旧专家同样在写点复核：await 窗口内若已被 equip 换成新卡,播报新卡,
    // 入口捕获的陈旧值会重复播报早已卸下的旧卡(二审补充)。
    const prev = state.activePersona;
    state.activePersona = null;
    if (prev) { addChatItem({ type: "system", text: bt("personaUnequipped") + personaName(prev), time: timeStr() }); recordPersonaEvent({ kind: "unequip", name: personaName(prev) }); }
    notify();
  }
  // 挂件还原的请求序号 + 最近成功加持的目标会话：
  // - syncActivePersona 仅 sid 校验挡不住 A→B→A 的 ABA 与同会话乱序(慢响应
  //   返回 null 会把刚加持的挂件覆盖掉,且无人再纠正)。序号在每次 sync 发起
  //   与 equip/unequip 权威写时递增,旧快照一律作废(审计补充)。
  // - lastEquippedSid 供 equip 后紧随的播报(如卡牌制造者引导卡)定向回
  //   发起会话——equip 的 await 窗口用户可能已切走。
  const deletedPersonaIds = new Set();
  let personaSyncSeq = 0;
  let lastEquippedSid = null;
  // 切换/重载 session 后,从后端拉该 session 的加持状态还原挂件(backend 是真相)。
  async function syncActivePersona() {
    if (!state.activeSessionId) { state.activePersona = null; return; }
    const sid = state.activeSessionId;
    const seq = ++personaSyncSeq;
    try {
      // invoke 形状保持原样（协议指纹按文本计算）；发起瞬间 activeSessionId === sid。
      const persona = await invoke("get_active_persona", { sessionId: state.activeSessionId }) || null;
      if (sid !== state.activeSessionId || seq !== personaSyncSeq) return; // 已切走或被权威写/新 sync 作废
      state.activePersona = persona && !deletedPersonaIds.has(persona.id) ? persona : null;
    } catch { /* 旧 session 无加持,忽略 */ }
  }
  // 在【指定 session】追加卡牌制造者引导卡并落 sidecar(持久化,重载按 pos 插回)。
  // 默认定向最近一次成功 equip 的目标会话：AI 造卡链路 equip→intro 之间用户
  // 切走时,intro 必须仍落在发起(已加持)会话,而不是写进切走后的当前显示
  // (错误会话被插卡是持久化污染,不可自愈)。显式传 sid 可覆盖(审计补充)。
function postCardCreatorIntro(sid) { return pinvouSharedtauriPersonas().postCardCreatorIntro(sid); }


function applyMountedCollections(value) { return pinvouSharedtauriPersonas().applyMountedCollections(value); }
  let mountedCollectionUpdate = Promise.resolve();
  let mountedCollectionDraftTarget = null;
function mountedCollectionTargetAtEnqueue() { return pinvouSharedtauriPersonas().mountedCollectionTargetAtEnqueue(); }
function updateMountedCollections(command, args) { return pinvouSharedtauriPersonas().updateMountedCollections(command, args); }
  // 添加知识集；已挂载但停用时重新启用，不覆盖其他挂载项。
async function mountCollection(collectionId) { return pinvouSharedtauriPersonas().mountCollection(collectionId); }
async function setCollectionEnabled(collectionId, enabled) { return pinvouSharedtauriPersonas().setCollectionEnabled(collectionId, enabled); }
async function removeCollection(collectionId) { return pinvouSharedtauriPersonas().removeCollection(collectionId); }
  // 兼容旧入口：摘下当前对话的全部知识集挂载。
  async function unmountCollection() {
    if (!state.activeSessionId) {
      applyMountedCollections([]);
      applyMountedRemoteCollections([]);
      notify();
      return;
    }
    // 所有更新在本同步片段内入队，确保它们捕获同一个点击时 session；随后即使用户
    // 切换会话，远程逐项移除和本地清空也不会误作用到新会话。
    const pending = normalizeMountedRemoteCollections(state.mountedRemoteCollections).map(function (entry) {
      return updateMountedRemoteCollections("session_remove_mounted_remote_collection", {
        serverId: entry.serverId,
        collectionId: entry.collectionId,
      });
    });
    pending.push(updateMountedCollections("session_unmount_collection", null));
    return Promise.all(pending);
  }
  function normalizeMountedRemoteCollections(value) {
    if (!Array.isArray(value)) return [];
    const seen = Object.create(null);
    return value.map(function (entry) {
      if (!entry || !entry.serverId || entry.collectionId == null) return null;
      const key = String(entry.serverId) + ":" + String(entry.collectionId);
      if (seen[key]) return null;
      seen[key] = true;
      return { serverId: entry.serverId, collectionId: entry.collectionId, enabled: entry.enabled !== false };
    }).filter(Boolean);
  }
  function applyMountedRemoteCollections(value) {
    state.mountedRemoteCollections = normalizeMountedRemoteCollections(value);
    return state.mountedRemoteCollections;
  }
  function updateMountedRemoteCollections(command, args) {
    const requestedTarget = mountedCollectionTargetAtEnqueue();
    mountedCollectionUpdate = mountedCollectionUpdate.catch(function () {}).then(async function () {
      const sessionId = await requestedTarget.promise;
      if (!sessionId) return null;
      try {
        const saved = await invoke(command, Object.assign({ sessionId }, args || {}));
        const normalized = normalizeMountedRemoteCollections(saved);
        if (state.activeSessionId === sessionId) {
          applyMountedRemoteCollections(normalized);
          notify();
        }
        return normalized;
      } catch (e) {
        addSystemItem(bt("mountCollectionFailed") + e);
        return null;
      }
    });
    if (requestedTarget.draft) {
      mountedCollectionUpdate = mountedCollectionUpdate.finally(function () {
        requestedTarget.pending -= 1;
        if (requestedTarget.pending === 0 && mountedCollectionDraftTarget === requestedTarget) mountedCollectionDraftTarget = null;
      });
    }
    return mountedCollectionUpdate;
  }
  async function mountRemoteCollection(serverId, collectionId) {
    return updateMountedRemoteCollections("session_add_mounted_remote_collection", { serverId, collectionId });
  }
  async function setRemoteCollectionEnabled(serverId, collectionId, enabled) {
    return updateMountedRemoteCollections("session_set_mounted_remote_collection_enabled", { serverId, collectionId, enabled: !!enabled });
  }
  async function removeRemoteCollection(serverId, collectionId) {
    return updateMountedRemoteCollections("session_remove_mounted_remote_collection", { serverId, collectionId });
  }
  // 切换/重载 session 后从后端还原挂载状态(backend 是真相;仅驻内存,重启后为 null)。
  async function syncLocalMountedCollection() {
    if (!state.activeSessionId) { applyMountedCollections([]); return; }
    const sessionId = state.activeSessionId;
    try {
      const snapshot = await invoke("session_mounted_collections_snapshot", { sessionId });
      if (state.activeSessionId !== sessionId) return;
      if (snapshot && Array.isArray(snapshot.collections)) { applyMountedCollections(snapshot); return; }
      const mounted = await invoke("session_mounted_collections", { sessionId });
      if (state.activeSessionId !== sessionId) return;
      if (Array.isArray(mounted)) { applyMountedCollections(mounted); return; }
      const legacy = await invoke("session_mounted_collection", { sessionId });
      if (state.activeSessionId !== sessionId) return;
      applyMountedCollections(legacy == null ? [] : [legacy]);
    } catch {
      try {
        const cid = await invoke("session_mounted_collection", { sessionId });
        if (state.activeSessionId !== sessionId) return;
        applyMountedCollections(cid == null ? [] : [cid]);
      } catch { if (state.activeSessionId === sessionId) applyMountedCollections([]); }
    }
  }
  async function syncMountedCollection() {
    await syncLocalMountedCollection();
    if (!state.activeSessionId) { applyMountedRemoteCollections([]); return; }
    const sessionId = state.activeSessionId;
    try {
      const mounted = await invoke("session_mounted_remote_collections", { sessionId });
      if (state.activeSessionId === sessionId) applyMountedRemoteCollections(mounted);
    } catch {
      if (state.activeSessionId === sessionId) applyMountedRemoteCollections([]);
    }
  }

  function getPersonas() { return personaPoolCache; }
    return {
      loadPersonas,
      getPersonas,
      createPersona,
      updatePersona,
      deletePersona,
      recordPersonaEvent,
      equipPersona,
      unequipPersona,
      postCardCreatorIntro,
      syncActivePersona,
      mountCollection,
      setCollectionEnabled,
      removeCollection,
      unmountCollection,
      mountRemoteCollection,
      setRemoteCollectionEnabled,
      removeRemoteCollection,
      syncMountedCollection
    };
  };
})(window);

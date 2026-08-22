/**
 * personas feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 */
(function (root) {
  "use strict";
  var registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["personas"] = function (context) {
    var state = context.state;
    var notify = context.notify;
    var invoke = context.invoke;
    var bt = context.bt;
    var addSystemItem = context.addSystemItem;
    var addChatItem = context.addChatItem;
    var timeStr = context.timeStr;
    var ensureSession = context.ensureSession;
    var runOnSession = context.runOnSession;
    var personaPlaceholderTitles = context.personaPlaceholderTitles;
    var isDefaultChatTitle = context.isDefaultChatTitle;
    var personaPoolCache = [];
  // ── 卡片池: 专家面具加持 ─────────────────────────────────────────
  // 懒加载全部专家卡(1078 张),前端缓存供 facet/搜索。只拉一次。
  async function loadPersonas() {
    if (state.personaPool.loadState === "ready" || state.personaPool.loadState === "loading") return;
    await refreshPersonas();
  }
  // 强制重拉卡牌列表(自创卡增删改后调,让池子立即反映)。
  async function refreshPersonas() {
    state.personaPool.loadState = "loading"; notify();
    try {
      personaPoolCache = await invoke("list_personas");
      state.personaPool.loadState = "ready";
    } catch (e) {
      personaPoolCache = []; state.personaPool.loadState = "error";
      console.warn("list_personas failed", e);
    }
    notify();
  }
  // ── 用户自创卡 CRUD(写盘后刷新缓存) ──
  async function createPersona(input) {
    var sum = await invoke("create_persona", { input: input });
    await refreshPersonas();
    return sum;
  }
  async function updatePersona(personaId, input) {
    var sum = await invoke("update_persona", { personaId: personaId, input: input });
    await refreshPersonas();
    // 若改的正是当前 session 加持的卡, 同步挂件显示
    if (state.activePersona && state.activePersona.id === personaId) { state.activePersona = sum; notify(); }
    return sum;
  }
  async function deletePersona(personaId) {
    await invoke("delete_persona", { personaId: personaId });
    await refreshPersonas();
  }
  // 给当前 session 加持一张专家面具。后端存 persona_id + 每 turn 注入人设;
  // 前端记 activePersona(挂件) + 发一条系统消息播报。
  // 取专家显示名(兼容 Side A 的 cn_name / Side B 的 name)。
  function personaName(p) {
    if (!p) return "";
    // 内置卡名按 UI 语言显示(personas-i18n.js overlay),中文兜底;自制卡不翻
    var lang = state.settings && state.settings.language;
    var L = lang === "en" ? "en" : lang === "ja" ? "ja" : null;
    var tr = L && p.source !== "user" && window.PERSONA_I18N && window.PERSONA_I18N[p.id] && window.PERSONA_I18N[p.id][L];
    if (tr && tr.name) return tr.name;
    return (p.name || p.cn_name) || "";
  }
  // 记一条卡牌事件到时间线 sidecar(pos=当前 messages 数),并落盘。重载历史时按 pos 插回。
  function recordPersonaEvent(ev) {
    if (!state.activeSessionId) return;
    ev.pos = state.messages.length;
    state.personaEvents.push(ev);
    var sid = state.activeSessionId;
    var snapshot = JSON.parse(JSON.stringify(state.personaEvents));
    invoke("save_session_persona_events", { sessionId: sid, events: snapshot }).catch(function () {});
  }
  async function equipPersona(personaId) {
    if (!state.activeSessionId) {
      // 草稿态加卡 → 先物化 session(lazy session)。用返回值判空：切走场景
      // ensureSession 返回 null 但 activeSessionId 非空，会把卡加进新会话。
      var materialized = await ensureSession();
      if (!materialized) return; // 物化失败/切走,放弃
    }
    // 入口捕获触发会话：await 期间用户可能切走，UI 写入不得落进别的会话
    // （错误会话被重命名/插卡是持久化污染，不可自愈）。后端已按发起会话
    // 定向；切走后放弃前端播报，挂件靠 syncActivePersona 恢复（审计）。
    var sid = state.activeSessionId;
    try {
      // invoke 形状保持原样（协议指纹按文本计算）；发起瞬间 activeSessionId === sid。
      var card = await invoke("equip_persona", { sessionId: state.activeSessionId, personaId: personaId });
      lastEquippedSid = sid; // 成功加持的目标会话(即使已切走)：供紧随其后的引导卡定向(审计补充)
      if (sid !== state.activeSessionId) return card; // 已切走：不写当前显示
      // 标题仍是默认占位(三语哨兵,见 isDefaultChatTitle)→ 用卡牌名命名(无论草稿态物化还是遗留空会话;
      // 用户已主动改名 / 已被首条消息命名的会话不动)。决策:卡牌优先于首条消息。
      var m = state.sessions.find(function (s) { return s.id === sid; });
      // 标题还是默认值 / 仍是卡牌占位(换卡场景)→ 用(新)卡牌名命名,并标记为占位。
      // 占位名会被首条用户消息覆盖(见 persistMessages*),让同卡会话靠对话内容区分。
      if (m && (isDefaultChatTitle(m.title) || personaPlaceholderTitles[sid])) {
        var newTitle = personaName(card);
        if (newTitle) {
          try { await invoke("rename_session", { id: sid, title: newTitle }); } catch (_) {}
          if (sid !== state.activeSessionId) return card; // rename 挂起期间切走:放弃后续 UI 写入(审计补充)
          m.title = newTitle;
          personaPlaceholderTitles[sid] = true;
        }
      }
      // 同 session 换了一张不同的卡 → 先弹一条"已卸下旧专家",再弹新加持。
      // 旧专家在写点复核而非入口捕获：同会话快速连续换卡时,入口值可能已被
      // 上一次 equip 的权威写覆盖,陈旧值会播报错误的"已卸下"(二审补充)。
      var prev = state.activePersona;
      if (prev && prev.id !== card.id) {
        addChatItem({ type: "system", text: bt("personaUnequipped") + personaName(prev), time: timeStr() });
        recordPersonaEvent({ kind: "unequip", name: personaName(prev) });
      }
      personaSyncSeq++; // 权威写前 bump：作废在途 syncActivePersona 的旧快照(审计补充)
      state.activePersona = card;
      addChatItem({ type: "persona_equip", card: card, time: timeStr() });
      recordPersonaEvent({ kind: "equip", card: card });
      notify();
      return card;
    } catch (e) {
      // 失败也守住归属：失败气泡只落发起会话,不得插进 await 窗口内切到的
      // 会话(addSystemItem 随消息流持久化);同时作废 lastEquippedSid——失败
      // equip 后紧随的引导卡不得回退定向到历史成功 equip 的无关会话(二审补充)。
      lastEquippedSid = null;
      if (sid === state.activeSessionId) addSystemItem(bt("equipFailed") + e);
      return null;
    }
  }
  // 摘下当前 session 的专家面具。
  async function unequipPersona() {
    if (!state.activeSessionId) return;
    // 入口捕获触发会话：await 期间切走，卸下播报不得写进别的会话（审计）。
    var sid = state.activeSessionId;
    try { await invoke("unequip_persona", { sessionId: state.activeSessionId }); } catch (e) { /* 忽略,前端照样摘 */ }
    if (sid !== state.activeSessionId) return; // 已切走：不写当前显示
    personaSyncSeq++; // 权威写前 bump：作废在途 syncActivePersona 的旧快照(审计补充)
    // 旧专家同样在写点复核：await 窗口内若已被 equip 换成新卡,播报新卡,
    // 入口捕获的陈旧值会重复播报早已卸下的旧卡(二审补充)。
    var prev = state.activePersona;
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
  var personaSyncSeq = 0;
  var lastEquippedSid = null;
  // 切换/重载 session 后,从后端拉该 session 的加持状态还原挂件(backend 是真相)。
  async function syncActivePersona() {
    if (!state.activeSessionId) { state.activePersona = null; return; }
    var sid = state.activeSessionId;
    var seq = ++personaSyncSeq;
    try {
      // invoke 形状保持原样（协议指纹按文本计算）；发起瞬间 activeSessionId === sid。
      var persona = await invoke("get_active_persona", { sessionId: state.activeSessionId }) || null;
      if (sid !== state.activeSessionId || seq !== personaSyncSeq) return; // 已切走或被权威写/新 sync 作废
      state.activePersona = persona;
    } catch (e) { /* 旧 session 无加持,忽略 */ }
  }
  // 在【指定 session】追加卡牌制造者引导卡并落 sidecar(持久化,重载按 pos 插回)。
  // 默认定向最近一次成功 equip 的目标会话：AI 造卡链路 equip→intro 之间用户
  // 切走时,intro 必须仍落在发起(已加持)会话,而不是写进切走后的当前显示
  // (错误会话被插卡是持久化污染,不可自愈)。显式传 sid 可覆盖(审计补充)。
  function postCardCreatorIntro(sid) {
    var target = sid || lastEquippedSid || state.activeSessionId;
    if (!target) return;
    runOnSession(target, function () {
      addChatItem({ type: "card_creator_intro", time: "" });
      recordPersonaEvent({ kind: "card_creator_intro" });
      notify();
    });
  }

  // ── 多知识库挂载(会话级粘连,仿 persona) ──
  function normalizeMountedCollections(value) {
    if (!Array.isArray(value)) return [];
    var seen = Object.create(null);
    return value.map(function (entry) {
      if (entry == null) return null;
      var collectionId = typeof entry === "object"
        ? (entry.collectionId != null ? entry.collectionId : entry.collection_id)
        : entry;
      if (collectionId == null || seen[String(collectionId)]) return null;
      seen[String(collectionId)] = true;
      return { collectionId: collectionId, enabled: typeof entry === "object" ? entry.enabled !== false : true };
    }).filter(Boolean);
  }
  function applyMountedCollections(value) {
    var hasSnapshot = value && !Array.isArray(value) && Array.isArray(value.collections);
    var revision = hasSnapshot ? Number(value.revision || 0) : Number(state.mountedCollectionsRevision || 0);
    if (hasSnapshot && revision < Number(state.mountedCollectionsRevision || 0)) {
      return normalizeMountedCollections(state.mountedCollections);
    }
    var normalized = normalizeMountedCollections(hasSnapshot ? value.collections : value);
    state.mountedCollections = normalized;
    state.mountedCollectionsRevision = revision;
    var firstEnabled = normalized.find(function (entry) { return entry.enabled; });
    state.mountedCollection = firstEnabled ? firstEnabled.collectionId : null;
    return normalized;
  }
  var mountedCollectionUpdate = Promise.resolve();
  var mountedCollectionDraftTarget = null;
  function mountedCollectionTargetAtEnqueue() {
    if (state.activeSessionId) return { draft: false, promise: Promise.resolve(state.activeSessionId) };
    var draftEpoch = Number(state.draftEpoch || 0);
    if (!mountedCollectionDraftTarget || mountedCollectionDraftTarget.epoch !== draftEpoch || mountedCollectionDraftTarget.failed) {
      var target = { draft: true, epoch: draftEpoch, failed: false, pending: 0, promise: null };
      target.promise = Promise.resolve().then(async function () {
        // Navigation before draft materialization cancels this batch instead of
        // silently retargeting it to the newly active session.
        if (state.activeSessionId) return null;
        var sessionId = await ensureSession();
        if (!sessionId) target.failed = true;
        return sessionId;
      });
      mountedCollectionDraftTarget = target;
    }
    mountedCollectionDraftTarget.pending += 1;
    return mountedCollectionDraftTarget;
  }
  function updateMountedCollections(command, args) {
    var requestedTarget = mountedCollectionTargetAtEnqueue();
    mountedCollectionUpdate = mountedCollectionUpdate.catch(function () {}).then(async function () {
      // The target is captured at click time. Rapid draft actions share one
      // materialization promise and remain bound to that session after navigation.
      var sessionId = await requestedTarget.promise;
      if (!sessionId) return null;
      try {
        var saved = await invoke(command, Object.assign({ sessionId: sessionId }, args || {}));
        var normalized = normalizeMountedCollections(saved && saved.collections);
        if (state.activeSessionId === sessionId) {
          applyMountedCollections(saved);
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
        if (requestedTarget.pending === 0 && mountedCollectionDraftTarget === requestedTarget) {
          mountedCollectionDraftTarget = null;
        }
      });
    }
    return mountedCollectionUpdate;
  }
  // 添加知识集；已挂载但停用时重新启用，不覆盖其他挂载项。
  async function mountCollection(collectionId) {
    if (collectionId == null) return null;
    var saved = await updateMountedCollections("session_add_mounted_collection", { collectionId: collectionId });
    return saved ? collectionId : null;
  }
  async function setCollectionEnabled(collectionId, enabled) {
    return updateMountedCollections("session_set_mounted_collection_enabled", {
      collectionId: collectionId,
      enabled: !!enabled,
    });
  }
  async function removeCollection(collectionId) {
    return updateMountedCollections("session_remove_mounted_collection", { collectionId: collectionId });
  }
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
    var pending = normalizeMountedRemoteCollections(state.mountedRemoteCollections).map(function (entry) {
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
    var seen = Object.create(null);
    return value.map(function (entry) {
      if (!entry || !entry.serverId || entry.collectionId == null) return null;
      var key = String(entry.serverId) + ":" + String(entry.collectionId);
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
    var requestedTarget = mountedCollectionTargetAtEnqueue();
    mountedCollectionUpdate = mountedCollectionUpdate.catch(function () {}).then(async function () {
      var sessionId = await requestedTarget.promise;
      if (!sessionId) return null;
      try {
        var saved = await invoke(command, Object.assign({ sessionId: sessionId }, args || {}));
        var normalized = normalizeMountedRemoteCollections(saved);
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
    return updateMountedRemoteCollections("session_add_mounted_remote_collection", { serverId: serverId, collectionId: collectionId });
  }
  async function setRemoteCollectionEnabled(serverId, collectionId, enabled) {
    return updateMountedRemoteCollections("session_set_mounted_remote_collection_enabled", { serverId: serverId, collectionId: collectionId, enabled: !!enabled });
  }
  async function removeRemoteCollection(serverId, collectionId) {
    return updateMountedRemoteCollections("session_remove_mounted_remote_collection", { serverId: serverId, collectionId: collectionId });
  }
  // 切换/重载 session 后从后端还原挂载状态(backend 是真相;仅驻内存,重启后为 null)。
  async function syncLocalMountedCollection() {
    if (!state.activeSessionId) { applyMountedCollections([]); return; }
    var sessionId = state.activeSessionId;
    try {
      var snapshot = await invoke("session_mounted_collections_snapshot", { sessionId: sessionId });
      if (state.activeSessionId !== sessionId) return;
      if (snapshot && Array.isArray(snapshot.collections)) { applyMountedCollections(snapshot); return; }
      var mounted = await invoke("session_mounted_collections", { sessionId: sessionId });
      if (state.activeSessionId !== sessionId) return;
      if (Array.isArray(mounted)) { applyMountedCollections(mounted); return; }
      var legacy = await invoke("session_mounted_collection", { sessionId: sessionId });
      if (state.activeSessionId !== sessionId) return;
      applyMountedCollections(legacy == null ? [] : [legacy]);
    } catch (e) {
      try {
        var cid = await invoke("session_mounted_collection", { sessionId: sessionId });
        if (state.activeSessionId !== sessionId) return;
        applyMountedCollections(cid == null ? [] : [cid]);
      } catch (_) { if (state.activeSessionId === sessionId) applyMountedCollections([]); }
    }
  }
  async function syncMountedCollection() {
    await syncLocalMountedCollection();
    if (!state.activeSessionId) { applyMountedRemoteCollections([]); return; }
    var sessionId = state.activeSessionId;
    try {
      var mounted = await invoke("session_mounted_remote_collections", { sessionId: sessionId });
      if (state.activeSessionId === sessionId) applyMountedRemoteCollections(mounted);
    } catch (_) {
      if (state.activeSessionId === sessionId) applyMountedRemoteCollections([]);
    }
  }

  function getPersonas() { return personaPoolCache; }
    return {
      loadPersonas: loadPersonas,
      getPersonas: getPersonas,
      createPersona: createPersona,
      updatePersona: updatePersona,
      deletePersona: deletePersona,
      recordPersonaEvent: recordPersonaEvent,
      equipPersona: equipPersona,
      unequipPersona: unequipPersona,
      postCardCreatorIntro: postCardCreatorIntro,
      syncActivePersona: syncActivePersona,
      mountCollection: mountCollection,
      setCollectionEnabled: setCollectionEnabled,
      removeCollection: removeCollection,
      unmountCollection: unmountCollection,
      mountRemoteCollection: mountRemoteCollection,
      setRemoteCollectionEnabled: setRemoteCollectionEnabled,
      removeRemoteCollection: removeRemoteCollection,
      syncMountedCollection: syncMountedCollection
    };
  };
})(window);

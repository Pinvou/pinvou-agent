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
    var personaPlaceholderTitles = context.personaPlaceholderTitles;
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
      await ensureSession(); // 草稿态加卡 → 先物化 session(lazy session)
      if (!state.activeSessionId) return; // 物化失败,放弃
    }
    var prev = state.activePersona; // 换卡前的旧专家(同 session 切换时先播报卸下)
    try {
      var card = await invoke("equip_persona", { sessionId: state.activeSessionId, personaId: personaId });
      // 标题仍是默认值「新对话」→ 用卡牌名命名(无论草稿态物化还是遗留空会话;
      // 用户已主动改名 / 已被首条消息命名的会话不动)。决策:卡牌优先于首条消息。
      var sid = state.activeSessionId;
      var m = state.sessions.find(function (s) { return s.id === sid; });
      // 标题还是默认值 / 仍是卡牌占位(换卡场景)→ 用(新)卡牌名命名,并标记为占位。
      // 占位名会被首条用户消息覆盖(见 persistMessages*),让同卡会话靠对话内容区分。
      if (m && (m.title === "新对话" || m.title === "New chat" || personaPlaceholderTitles[sid])) {
        var newTitle = personaName(card);
        if (newTitle) {
          try { await invoke("rename_session", { id: sid, title: newTitle }); } catch (_) {}
          m.title = newTitle;
          personaPlaceholderTitles[sid] = true;
        }
      }
      // 同 session 换了一张不同的卡 → 先弹一条"已卸下旧专家",再弹新加持。
      if (prev && prev.id !== card.id) {
        addChatItem({ type: "system", text: bt("personaUnequipped") + personaName(prev), time: timeStr() });
        recordPersonaEvent({ kind: "unequip", name: personaName(prev) });
      }
      state.activePersona = card;
      addChatItem({ type: "persona_equip", card: card, time: timeStr() });
      recordPersonaEvent({ kind: "equip", card: card });
      notify();
      return card;
    } catch (e) { addSystemItem(bt("equipFailed") + e); return null; }
  }
  // 摘下当前 session 的专家面具。
  async function unequipPersona() {
    if (!state.activeSessionId) return;
    var prev = state.activePersona;
    try { await invoke("unequip_persona", { sessionId: state.activeSessionId }); } catch (e) { /* 忽略,前端照样摘 */ }
    state.activePersona = null;
    if (prev) { addChatItem({ type: "system", text: bt("personaUnequipped") + personaName(prev), time: timeStr() }); recordPersonaEvent({ kind: "unequip", name: personaName(prev) }); }
    notify();
  }
  // 切换/重载 session 后,从后端拉该 session 的加持状态还原挂件(backend 是真相)。
  async function syncActivePersona() {
    if (!state.activeSessionId) { state.activePersona = null; return; }
    try {
      state.activePersona = await invoke("get_active_persona", { sessionId: state.activeSessionId }) || null;
    } catch (e) { /* 旧 session 无加持,忽略 */ }
  }

  // ── 知识库挂载(会话级粘连,仿 persona) ──
  // 给当前对话挂一个知识集;草稿态先物化 session(同 equipPersona)。挂上后每条消息
  // 发送前后端自动检索注入(commands::chat)。返回挂载的 id 或 null(失败)。
  async function mountCollection(collectionId) {
    if (collectionId == null) return null;
    if (!state.activeSessionId) {
      await ensureSession();
      if (!state.activeSessionId) return null;
    }
    try {
      await invoke("session_mount_collection", { sessionId: state.activeSessionId, collectionId: collectionId });
      state.mountedCollection = collectionId;
      notify();
      return collectionId;
    } catch (e) { addSystemItem("挂载知识集失败: " + e); return null; }
  }
  // 摘下当前对话的知识集挂载。
  async function unmountCollection() {
    if (!state.activeSessionId) { state.mountedCollection = null; notify(); return; }
    try { await invoke("session_unmount_collection", { sessionId: state.activeSessionId }); } catch (e) { /* 前端照样摘 */ }
    state.mountedCollection = null;
    notify();
  }
  // 切换/重载 session 后从后端还原挂载状态(backend 是真相;仅驻内存,重启后为 null)。
  async function syncMountedCollection() {
    if (!state.activeSessionId) { state.mountedCollection = null; return; }
    try {
      var cid = await invoke("session_mounted_collection", { sessionId: state.activeSessionId });
      state.mountedCollection = (cid == null) ? null : cid;
    } catch (e) { state.mountedCollection = null; }
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
      syncActivePersona: syncActivePersona,
      mountCollection: mountCollection,
      unmountCollection: unmountCollection,
      syncMountedCollection: syncMountedCollection
    };
  };
})(window);

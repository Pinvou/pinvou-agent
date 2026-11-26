(function () {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim copy of a classic script; strict mode is the payload
  "use strict";

  const shellCleanupFailed = {
    zh: "⚠️ 部分后台任务未能停止，可在后台任务列表中逐个停止。",
    en: "⚠️ Some background tasks could not be stopped. You can stop them individually from the background task list.",
    ja: "⚠️ 一部のバックグラウンドタスクを停止できませんでした。バックグラウンドタスク一覧から個別に停止できます。",
  };

  // 与 bridge 层 bt() 约定一致:settings.language 存的是 tag(zh-Hans/en/ja),
  // 非 en/ja 一律回退中文。
  function shellCleanupFailedText(language) {
    return language === "en" ? shellCleanupFailed.en
      : language === "ja" ? shellCleanupFailed.ja
      : shellCleanupFailed.zh;
  }

  // Backend error strings the desktop computer-use feature surfaces
  // verbatim: localizeKnownError maps command rejections
  // (platform/tauri/bridge/computer_use.js) and the virtual-display status
  // projection maps the settings-page diagnostic detail through the same
  // table (localizeVirtualDisplayDetail). Exact equality first;
  // unrecognized messages then fall through to the marker-based table
  // below, and a miss there returns null so the bridge passes the original
  // error through untouched.
  const computerUseKnownErrors = {
    "computer use has no backend on this operating system": {
      zh: "当前平台没有电脑使用后端，无法开启此功能。",
      en: "Computer use is not available on this platform: there is no computer-use backend for this operating system.",
      ja: "このプラットフォームにはコンピュータ操作のバックエンドがないため、この機能は利用できません。",
    },
    "computer use is disabled; enable it in settings before granting control": {
      zh: "电脑使用已在设置中停用，请先开启再授予控制。",
      en: "Computer use is disabled in settings; enable it before granting control.",
      ja: "コンピュータ操作は設定で無効になっています。制御を許可する前に設定で有効にしてください。",
    },
    "computer use is stopped; resume it before granting control": {
      zh: "电脑使用已停止，请先恢复（关闭后再开启）再授予控制。",
      en: "Computer use is stopped; turn it off and back on before granting control.",
      ja: "コンピュータ操作は停止中です。オフにして再度オンにしてから制御を許可してください。",
    },
  };

  // The expired-confirmation errors embed the confirm_id
  // ("confirmation request no longer exists (unknown or expired): cu-…"),
  // so exact equality cannot match them; these match on a stable marker
  // phrase instead (pinned by tests on both sides of the bridge).
  const computerUseKnownErrorMarkers = [
    {
      marker: "unknown or expired",
      zh: "该确认请求已过期或不存在；请重新发起操作，确认弹窗会再次出现。",
      en: "This confirmation request no longer exists (unknown or expired); retry the action to raise a fresh confirmation.",
      ja: "この確認リクエストは存在しないか有効期限が切れています。操作を再試行すると新しい確認が表示されます。",
    },
    // The macOS virtual-display copy embeds live values (seconds, vendor and
    // product ids, serial numbers), so exact equality cannot match; these
    // match on stable marker phrases. Each marker is pinned against the
    // backend source by tests/computer_use_bridge.test.mjs — a wording
    // change on either side must update both in the same change.
    {
      marker: "did not become active within",
      zh: "虚拟显示器未能在创建预算内激活：若 macOS 曾为新显示器弹出「屏幕镜像」询问，该询问已随本次释放一起关闭——请新开会话，并在 控制中心 → 屏幕镜像（若系统已记住选择则在 系统设置 → 显示器）中为该显示器选择「扩展」。",
      en: "The virtual display did not become active within the creation budget; if macOS showed a Screen Mirroring alert for the new display, that alert closed when the session released the display — start a new session and choose \"Extend\" for it in Control Center → Screen Mirroring (or System Settings → Displays if the choice was remembered).",
      ja: "仮想ディスプレイが作成予算内でアクティブになりませんでした：macOS が新しいディスプレイの「画面をミラーリング」確認を表示していた場合、その確認はセッションがディスプレイを解放した時点で閉じています——新しいセッションを開始し、コントロールセンター → 画面をミラーリング（選択が記憶されている場合はシステム設定 → ディスプレイ）で「拡張」を選択してください。",
    },
    {
      marker: "came up as part of a mirror set",
      zh: "虚拟显示器以镜像集合成员启动，而不是独立桌面：镜像会把你的桌面内容复制到它上面，驱动或截取它等于驱动或截取你的桌面。请新开会话，并在 控制中心 → 屏幕镜像（若系统已记住选择则在 系统设置 → 显示器）中为该显示器选择「扩展」。",
      en: "The virtual display came up as part of a mirror set, not as a separate desktop: the set duplicates your desktop onto it, so driving or capturing it would drive and capture your desktop. Start a new session and choose \"Extend\" for the display in Control Center → Screen Mirroring (or System Settings → Displays if the choice was remembered).",
      ja: "仮想ディスプレイが独立したデスクトップではなくミラーセットの一部として起動しました：ミラーリングはあなたのデスクトップをこのディスプレイに複製するため、操作やキャプチャはあなたのデスクトップへの操作になります。新しいセッションを開始し、コントロールセンター → 画面をミラーリング（選択が記憶されている場合はシステム設定 → ディスプレイ）で「拡張」を選択してください。",
    },
    {
      marker: "became the main display while the user's own",
      zh: "虚拟显示器成了主显示器，而你的显示器仍在线：菜单栏和新窗口已移到它上面，驱动或截取它等于驱动或截取你的桌面。请新开会话，并在 macOS 询问时为该显示器选择「扩展」（控制中心 → 屏幕镜像）；若系统已记住排列，请在 系统设置 → 显示器 中把你的显示器设为主显示器。",
      en: "The virtual display became the main display while your own displays are active: their menu bar and new windows moved onto it, so driving or capturing it would drive and capture your desktop. Start a new session and choose \"Extend\" for the display when macOS asks (Control Center → Screen Mirroring); if the arrangement was remembered, set your own display as the main display in System Settings → Displays.",
      ja: "あなたのディスプレイがアクティブなまま、仮想ディスプレイがメインディスプレイになりました：メニューバーと新しいウィンドウがそちらへ移動しているため、操作やキャプチャはあなたのデスクトップへの操作になります。新しいセッションを開始し、macOS が尋ねたときにこのディスプレイへ「拡張」を選択してください（コントロールセンター → 画面をミラーリング）。配置が記憶されている場合は、システム設定 → ディスプレイで自分のディスプレイをメインに設定してください。",
    },
    {
      marker: "rejected every identity slot",
      zh: "macOS 拒绝了虚拟显示器的全部身份槽位（每个未结束的并发会话各占一块，上限 4 块）：常见原因是其他 Pinvou 虚拟显示器仍在运行——退出本应用可立即全部释放（停止其他会话的电脑使用要等其会话结束后才生效），然后新开会话。",
      en: "macOS rejected every virtual display identity slot (one per concurrent session that has not ended, up to four): the usual cause is other Pinvou virtual displays still alive — quit the app to release them all immediately (stopping the other sessions' computer use only helps once their sessions end), then start a new session.",
      ja: "macOS が仮想ディスプレイの識別スロットをすべて拒否しました（終了していない同時実行セッションごとに 1 つ、最大 4 つ）：原因は通常、他の Pinvou 仮想ディスプレイがまだ動作していることです——このアプリを終了するとすべて即時に解放されます（他のセッションのコンピュータ操作を停止しても、そのセッションが終わって初めて反映されます）。その後、新しいセッションを開始してください。",
    },
    // The API-absence probes are the user-facing "this macOS cannot host a
    // virtual display" condition: the probe verdict is the settings page's
    // "unavailable" detail line, and a creation with the preference on
    // falls back to the physical target in the same breath (resolve_target),
    // so the copy states that fallback. The creation-phase counterpart
    // ("has no setter to name the display") is deliberately NOT mapped: it
    // only fires when the probe passed yet creation still cannot proceed,
    // where no fallback exists and a localized "sessions fall back" claim
    // would be false.
    {
      marker: "private macos virtual display api not present on this macos",
      zh: "此 macOS 版本没有隔离虚拟显示器所依赖的私有显示器 API，无法创建虚拟显示器：会话会改用你的真实屏幕。可尝试升级 macOS；若确认不支持，可在设置中关闭「优先使用隔离的虚拟显示器」，让此提示不再出现。",
      en: "This macOS build lacks the private display API the isolated virtual display depends on, so no virtual display can be created: sessions fall back to your real screen. Consider updating macOS; if it still does not provide the API, turn off \"Prefer isolated virtual display\" in settings to dismiss this hint.",
      ja: "この macOS ビルドには隔離仮想ディスプレイが依存するプライベートディスプレイ API がないため、仮想ディスプレイは作成できません：セッションはあなたの実際の画面に切り替わります。macOS の更新を検討してください。それでも対応していない場合は、設定で「隔離された仮想ディスプレイを優先」をオフにするとこの表示は消えます。",
    },
    {
      // Covers both probe shapes (plural "selectors" and singular
      // "selector") via the shared prefix.
      marker: "is missing the selector",
      zh: "此 macOS 版本的私有显示器 API 缺少隔离虚拟显示器所需的选择器，无法创建虚拟显示器：会话会改用你的真实屏幕。可尝试升级 macOS；若确认不支持，可在设置中关闭「优先使用隔离的虚拟显示器」。",
      en: "This macOS build's private display API is missing selectors the isolated virtual display needs, so no virtual display can be created: sessions fall back to your real screen. Consider updating macOS; if it still lacks them, turn off \"Prefer isolated virtual display\" in settings.",
      ja: "この macOS ビルドのプライベートディスプレイ API には隔離仮想ディスプレイに必要なセレクタが欠けているため、仮想ディスプレイは作成できません：セッションはあなたの実際の画面に切り替わります。macOS の更新を検討してください。それでも欠けている場合は、設定で「隔離された仮想ディスプレイを優先」をオフにしてください。",
    },
  ];

  function computerUseKnownErrorText(message, language) {
    const known = computerUseKnownErrors[String(message || "")];
    if (known) {
      return language === "en" ? known.en
        : language === "ja" ? known.ja
        : known.zh;
    }
    const text = String(message || "").toLowerCase();
    for (const entry of computerUseKnownErrorMarkers) {
      if (text.includes(entry.marker)) {
        return language === "en" ? entry.en
          : language === "ja" ? entry.ja
          : entry.zh;
      }
    }
    return null;
  }

  // Runtime-owned user-role turns may arrive with their trailing turn metadata
  // flattened into the same text block. Keep that transport detail out of UI projections.
  function inputProvenanceFromText(value) {
    const text = String(value || "").trim();
    if (!text) return "";
    const lowerText = text.toLowerCase();
    const openingTag = "<turn_meta>";
    const closingTag = "</turn_meta>";
    const openingIndex = lowerText.lastIndexOf(openingTag);
    const closingIndex = lowerText.indexOf(closingTag, openingIndex + openingTag.length);
    const hasTrailingMetadata = openingIndex > 0 && text[openingIndex - 1] === "\n" &&
      closingIndex >= 0 && !text.slice(closingIndex + closingTag.length).trim();
    const metadata = openingIndex === 0
      ? text
      : (hasTrailingMetadata ? text.slice(openingIndex, closingIndex + closingTag.length) : "");
    if (!metadata) return "";
    const match = metadata.match(/(?:^|\n)Input provenance:\s*([a-z0-9_-]+)/i);
    return match && match[1] ? match[1].toLowerCase() : "";
  }

  function isInternalUserMessageProvenance(provenance) {
    return ["runtime", "subagent_handoff", "shell_completion"].includes(provenance);
  }

  function consumeLeadingInternalRuntimeEnvelope(value) {
    const text = String(value || "").trim();
    const opening = text.match(/^<codewhale:runtime_event\b[^>]*\bvisibility=(["'])internal\1[^>]*>/i);
    if (!opening) return null;
    const remainder = text.slice(opening[0].length);
    const closing = remainder.match(/<\/codewhale:runtime_event\s*>/i);
    if (!closing) return null;
    return remainder.slice(closing.index + closing[0].length).trim();
  }

  function containsOnlyInternalRuntimeMetadata(value) {
    let remainder = String(value || "").trim();
    let foundEnvelope = false;
    while (remainder) {
      const afterEnvelope = consumeLeadingInternalRuntimeEnvelope(remainder);
      if (afterEnvelope === null) break;
      foundEnvelope = true;
      remainder = afterEnvelope;
    }
    if (!foundEnvelope) return false;
    if (!remainder || /^<turn_meta_unchanged\s*\/>$/i.test(remainder)) return true;
    const lowerRemainder = remainder.toLowerCase();
    return lowerRemainder.startsWith("<turn_meta>") && lowerRemainder.endsWith("</turn_meta>");
  }

  function userMessageInputProvenance(blocks) {
    const textBlocks = Array.isArray(blocks) ? blocks : [];
    for (let i = 0; i < textBlocks.length; i++) {
      const block = textBlocks[i];
      if (!block || block.type !== "text") continue;
      const provenance = inputProvenanceFromText(block.text);
      if (provenance) return provenance;
    }
    return "";
  }

  // CodeWhale may persist a prompt-cache delta as a user-role message so the
  // model keeps the effective session context. It is runtime metadata, not a
  // user turn. Match the complete canonical shape only: a user's mention or
  // quotation of the tag must remain visible.
  function isContextUpdateRuntimeMessage(value) {
    const text = String(value || "").trim().replaceAll("\r\n", "\n");
    const prefix = [
      "<context_update>",
      "Session context changed since it was pinned; the pinned system prompt is unchanged. Delta (+ added, - removed):",
      "",
    ].join("\n");
    return text.startsWith(prefix) && text.endsWith("\n</context_update>");
  }

  // Sum of the context_update boundary counts already handed out to a
  // session's rendered user bubbles (runtimeContextUpdatesBefore). The
  // committed-transcript resync (chat:transcript_committed payload
  // context_updates_total, computed Rust-side by count_context_updates)
  // recomputes the trailing pending count as total − consumed: the live
  // delivery never announces engine drift deltas (they produce no
  // chat:user_message), so the cumulative total is the only trustworthy
  // live source, and the resync absorbs every live-path divergence at once
  // (drift deltas, duplicate admissions, failed sends, deferred drops,
  // edit_last splices).
  function runtimeContextUpdatesConsumedTotal(chatItems) {
    let consumed = 0;
    for (const item of chatItems || []) {
      if (item && item.type === "user" && typeof item.runtimeContextUpdatesBefore === "number") {
        consumed += item.runtimeContextUpdatesBefore;
      }
    }
    return consumed;
  }

  function resyncRuntimeContextUpdatesPending(buffer, chatItems, total) {
    if (!buffer) return;
    const totalNormalized = Number(total) || 0;
    // 记下最近一次权威总数：压缩基线重置用它在气泡 vintage 清零后立即
    // 重播种待消费余量（见 heal 注释的时序推导）。
    buffer.runtimeContextUpdatesLastTotal = totalNormalized;
    buffer.runtimeContextUpdatesPending =
      Math.max(0, totalNormalized - runtimeContextUpdatesConsumedTotal(chatItems));
  }

  // 压缩真重写（messages_before > messages_after）后的计数基线重置（r7 M2）：
  // 旧漂移增量随转写改写被摘除，而客户端气泡仍携带压缩前的
  // runtimeContextUpdatesBefore 老 vintage——权威重同步 pending = total −
  // consumed 会被老消费量钳在 0，此后每个新增量都补不回来（fork/rewind
  // 序号整体低计，恰是计数器要修的形态）。prune-only（before === after，
  // 转写未被改写）不动基线：增量与气泡仍一一对应，重置反而抬高 pending。
  // 重播种（r8）：底座在 CompactionCompleted **之前**先发 SessionUpdated
  // （persist 已携带压缩后总数跑过一次重同步），所以此时 buffer 上的
  // lastTotal 就是压缩后的真实存量——待消费直接重播种为 lastTotal（而非
  // 清零）让序号即时收敛：清零会让「压缩完成 → 下一次 persist」的窗口里
  // fork 序号按 0 个增量折算，点了分叉会少保留被点击轮次之前的存量增量。
  // 旧后端从未发过总数（lastTotal 缺失）保持零基线不变。返回是否有改动
  // （调用方决定是否 notify）。
  function healRuntimeContextUpdatesVintageAfterCompaction(buffer, chatItems, before, after) {
    if (!Number.isFinite(Number(before)) || !Number.isFinite(Number(after)) || Number(after) >= Number(before)) return false;
    let healed = false;
    if (buffer) {
      const lastTotal = Number(buffer.runtimeContextUpdatesLastTotal);
      const seeded = Number.isFinite(lastTotal) ? lastTotal : 0;
      if (buffer.runtimeContextUpdatesPending !== seeded) healed = true;
      buffer.runtimeContextUpdatesPending = seeded;
    }
    for (const item of chatItems || []) {
      if (!item || item.type !== "user" || !item.runtimeContextUpdatesBefore) continue;
      item.runtimeContextUpdatesBefore = 0;
      healed = true;
    }
    return healed;
  }

  // ── CodeWhale compaction checkpoint ────────────────────────────────
  // Compaction commits its summary into the saved history as a role="user"
  // message (the model reads it as a user-history item, Codex parity), so the
  // engine's context survives a reload. It is engine-written runtime state,
  // not a user turn: a projection that renders it as typed input shows the
  // model-facing summary under the user's name. Recognition mirrors the
  // foundation's structural predicates (CodeWhale
  // crates/tui/src/history_recognition.rs) rather than a substring scan,
  // because a user may legitimately paste or quote the summary header.
  const COMPACTION_SUMMARY_MARKERS = [
    "Another language model started to solve this problem",
    "Conversation Summary (Auto-Generated)",
  ];
  const COMPACTION_CHECKPOINT_PROVENANCE = "<!-- codewhale.compaction-checkpoint.v1 -->";

  function isCheckpointTextBox(block) {
    return !!block && block.type === "text" && block.cache_control == null;
  }

  // Loose form (`is_compaction_checkpoint_message`): the first text block
  // begins with the summary header (current or pre-v0.9.6 legacy), and the
  // only other block, if any, is the provenance marker.
  function isCompactionCheckpointBlocks(blocks) {
    if (!Array.isArray(blocks) || blocks.length === 0 || blocks.length > 2) return false;
    const first = blocks[0];
    if (!isCheckpointTextBox(first) || typeof first.text !== "string") return false;
    if (COMPACTION_SUMMARY_MARKERS.every(marker => !first.text.startsWith(marker))) return false;
    return blocks.length === 1 ||
      (isCheckpointTextBox(blocks[1]) && blocks[1].text === COMPACTION_CHECKPOINT_PROVENANCE);
  }

  // Stamped form (`is_generated_compaction_checkpoint`): summary block plus
  // the engine-written provenance block. Typed input cannot produce this
  // shape — the marker is its own content block — so it is an unambiguous
  // checkpoint.
  //
  // Display reads ONLY this form as a checkpoint. The bare form matches a
  // user turn that merely pastes the summary header; the foundation still
  // reads it loosely in a history with no stamped carrier because a missed
  // carrier would stack a second summary, but hiding a user's own message
  // has no such correctness need behind it. A pre-stamp bare carrier turns
  // stamped on the engine's next restore and disappears from the bubbles
  // then.
  function isGeneratedCompactionCheckpointBlocks(blocks) {
    return Array.isArray(blocks) && blocks.length === 2 && isCompactionCheckpointBlocks(blocks);
  }

  function isInternalRuntimeUserMessage(value) {
    return isContextUpdateRuntimeMessage(value) ||
      containsOnlyInternalRuntimeMetadata(value) ||
      isInternalUserMessageProvenance(inputProvenanceFromText(value));
  }

  function isKnownInternalMetadataBlock(value) {
    if (isInternalRuntimeUserMessage(value)) return true;
    const text = String(value || "").trim().toLowerCase();
    return /^<turn_meta_unchanged\s*\/>$/.test(text) ||
      (text.startsWith("<turn_meta>") && text.endsWith("</turn_meta>"));
  }

  // A persisted user message is internal only when every non-empty text block
  // is known runtime metadata. A canonical context update may be accompanied
  // by other internal metadata, but any ordinary text keeps the user turn.
  function isInternalUserMessage(blocks) {
    const textBlocks = Array.isArray(blocks) ? blocks : [];
    if (isInternalUserMessageProvenance(userMessageInputProvenance(textBlocks))) return true;
    // The stamped checkpoint is engine-written by construction, so every
    // consumer (display text, edit-last boundary, auto-title) reads it as
    // runtime metadata. The bare form matches pasted headers and stays
    // displayable.
    if (isGeneratedCompactionCheckpointBlocks(textBlocks)) return true;
    let foundInternal = false;
    for (let i = 0; i < textBlocks.length; i++) {
      const block = textBlocks[i];
      if (!block || block.type !== "text") continue;
      const text = String(block.text || "");
      if (!text.trim()) continue;
      if (!isKnownInternalMetadataBlock(text)) return false;
      foundInternal = true;
    }
    return foundInternal;
  }

  window.PinvouBridgeMessages = Object.freeze({
    isInternalRuntimeUserMessage,
    // canonical <context_update> shape matcher: the fork/rewind ordinal
    // counter (bridge-shared-helpers / chat-events) counts exactly these
    // dropped runtime messages — see forkTurnEntries.js.
    isContextUpdateRuntimeMessage,
    // committed-transcript resync of the ordinal counter (tauri lane):
    // pending = payload.context_updates_total − consumed-on-bubbles.
    runtimeContextUpdatesConsumedTotal,
    resyncRuntimeContextUpdatesPending,
    healRuntimeContextUpdatesVintageAfterCompaction,
    isInternalUserMessage,
    isKnownInternalMetadataBlock,
    isInternalUserMessageProvenance,
    userMessageInputProvenance,
    // CodeWhale compaction checkpoint (see the family above): the two
    // structural predicates and their literals, exported for the replay and
    // for parity tests with the ESM twin (src/shared/internal-message.mjs).
    isCompactionCheckpointBlocks,
    isGeneratedCompactionCheckpointBlocks,
    COMPACTION_SUMMARY_MARKERS,
    COMPACTION_CHECKPOINT_PROVENANCE,
    computerUseKnownErrorText,
    // Error texts the gate missed (gateway/proxy custom bodies, raw
    // provider messages) do not become model-service notices but still get
    // displayed as bare strings / red text. Redact unconditionally in
    // front of every display surface: classification may miss, credentials
    // must not. Returned as-is when the helper is missing (falls back to
    // the existing behavior).
    redactRawError: function (error, state) {
      const text = String(error == null ? "" : error);
      if (!text) return text;
      const helper = window.PinvouModelServiceErrors;
      if (!helper || typeof helper.redactTechnicalDetail !== "function") return text;
      return helper.redactTechnicalDetail(text, state && state.settings && state.settings.language);
    },

    modelServiceUserError: function (payload, state) {
      payload = payload || {};
      state = state || {};
      const raw = payload && (payload.user_error || payload.userError);
      const error = payload && payload.error;
      if (!raw && !error) return null;
      const helper = window.PinvouModelServiceErrors;
      if (!helper || typeof helper.build !== "function") return null;
      if (!raw && typeof helper.isModelServiceError === "function" && !helper.isModelServiceError(error)) {
        return null;
      }
      const language = state.settings && state.settings.language;
      const userError = helper.build(raw || error, {
        language,
        providerLabel: helper.providerLabelFromState(state, language),
        terminal: payload.terminal !== false,
      });
      // The structured base code/category forwarded by the forwarder is
      // kept on the user card: the streaming path currently emits mostly
      // the generic "transient", so this is controlled semantics retained
      // for diagnostics only and does not participate in gating or
      // classification.
      if (typeof (payload && payload.code) === "string" && payload.code) userError.errorCode = payload.code;
      if (typeof (payload && payload.category) === "string" && payload.category) userError.errorCategory = payload.category;
      return userError;
    },

    addModelServiceErrorNotice: function (payload, state, addSystemItem, legacyConversationOnly, terminalRecord) {
      payload = payload || {};
      state = state || {};
      const helper = window.PinvouModelServiceErrors;
      payload.terminal = !!legacyConversationOnly;
      const userError = window.PinvouBridgeMessages.modelServiceUserError(payload, state);
      if (!helper || !userError) return false;
      const notice = helper.noticeText(userError);
      const chatItems = Array.isArray(state.chatItems) ? state.chatItems : [];
      const nextDetail = userError && userError.technicalDetail;
      // Dedup keys on the error identity (kind + technical detail), not
      // the final wording: within one turn a transient notice (recoverable
      // wording) followed by done (terminal wording) always differs in
      // text, so exact-text dedup would miss and produce two contradictory
      // bubbles for one error.
      const existing = chatItems.find(function (item) {
        if (!item || !item.turnErrorNotice || !item.userError) return false;
        if (item.userError.kind !== userError.kind) return false;
        const existingDetail = item.userError.technicalDetail;
        if (existingDetail || nextDetail) return existingDetail === nextDetail;
        return true;
      });
      // Hiding the bubble on terminal requires a timeline terminal record
      // that actually exists and carries an error (the timeline card takes
      // over). recordTurnCompleted writes no record when openStart/turnId
      // is missing; hiding anyway would make the error fully invisible
      // (silent-swallow regression).
      const timelineTakesOver = !!legacyConversationOnly
        && !!(terminalRecord && terminalRecord.error);
      let target = existing || null;
      if (target) {
        target.text = notice;
        target.userError = userError;
        if (timelineTakesOver) target.legacyConversationOnly = true;
      } else {
        target = { turnErrorNotice: true, legacyConversationOnly: timelineTakesOver, userError };
        addSystemItem(notice, target);
      }
      // On terminal takeover, hide the turn's other model-service transient
      // bubbles too (of a different identity, e.g. a network idle timeout
      // followed by a billing done): their "will keep retrying" wording
      // contradicts the terminal "has stopped". The bridge clears
      // turnErrorNotice items on every send, so everything still present
      // belongs to the current turn and the previous turn is untouched.
      if (timelineTakesOver) {
        chatItems.forEach(function (item) {
          if (item && item !== target && item.turnErrorNotice && item.userError && !item.legacyConversationOnly) {
            item.legacyConversationOnly = true;
          }
        });
      }
      payload.user_error = userError;
      payload.userError = userError;
      return true;
    },
    // When a successful chat:done (no error) arrives, the turn's transient
    // model-service bubbles ("will keep retrying") are stale: the turn has
    // recovered, and an outdated claim must not outlive the recovery it
    // described. Only items carrying a userError are hidden (their wording
    // promises future retries); bare-string fallbacks are statements about
    // errors that did happen and keep the existing behavior. Terminal
    // errors do not go through this path — addModelServiceErrorNotice
    // already upgrades/hides them by identity.
    settleModelServiceErrorNotices: function (state) {
      state = state || {};
      const chatItems = Array.isArray(state.chatItems) ? state.chatItems : [];
      let settled = false;
      chatItems.forEach(function (item) {
        if (item && item.turnErrorNotice && item.userError && !item.legacyConversationOnly) {
          item.legacyConversationOnly = true;
          settled = true;
        }
      });
      return settled;
    },
    showShellCleanupFailure: function (payload, state, addSystemItem) {
      if (!payload || !payload.shell_cleanup_failed) return;
      const notice = shellCleanupFailedText(state.settings && state.settings.language);
      const existing = state.chatItems.find(function (item) {
        return item && item.turnErrorNotice && item.text === notice;
      });
      if (existing) {
        existing.legacyConversationOnly = true;
      } else {
        addSystemItem(notice, {
          turnErrorNotice: true,
          legacyConversationOnly: true,
        });
      }
    },
  });
})();

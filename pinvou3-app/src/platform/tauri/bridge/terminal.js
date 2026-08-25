/** Shell polling and terminal output normalization for bridge tool cards. */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim copy of a classic-script artifact; strict mode is part of the payload
  "use strict";
  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting the statement would diverge from the artifact
  const registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry.terminal = function (context) {
    const state = context.state;
    const notify = context.notify;
    const invoke = context.invoke;
    const bt = context.bt;
    const runSyncOnSession = context.runSyncOnSession;
    const addChatItem = context.addChatItem;
    let shellNotifyTimer = null;
    const shellPollState = Object.create(null);
  function updateToolItem(toolId, output, success) {
    for (let i = 0; i < state.chatItems.length; i++) {
      if (state.chatItems[i].type === "tool" && state.chatItems[i].toolId === toolId) {
        state.chatItems[i].output = output;
        state.chatItems[i].success = success;
        state.chatItems[i].state = success ? "done" : "failed";
        delete state.chatItems[i]._terminalParser;
        return state.chatItems[i];
      }
    }
    return null;
  }

  function isShellExecutionTool(name) {
    return ["exec_shell", "exec_shell_wait", "exec_wait", "task_shell_start", "task_shell_wait", "shell", "Bash"].includes(name);
  }

  function utf8Length(text) {
    try { return new TextEncoder().encode(String(text || "")).length; }
    catch { return String(text || "").length; }
  }

  // Shell snapshots are a tail view, not an append-only byte stream; the tail
  // is normalized by the later normalizeTerminalTail (mergeTerminalChunk-based)
  // below. An earlier ANSI-stripping variant of the same name was dead code
  // (same-scope redeclaration meant the later function always won) and was
  // removed while fixing the duplicate declaration.

  function formatShellSnapshot(job) {
    function section(raw, total, kind) {
      raw = String(raw || "");
      const visibleRaw = raw.replace(/^\.\.\.\s*/, "");
      const omitted = /^\.\.\./.test(raw) || Number(total || 0) > utf8Length(visibleRaw);
      let body = normalizeTerminalTail(visibleRaw);
      if (omitted) body = bt("shellOutputOmitted")(kind) + "\n" + body;
      return body;
    }
    const stdout = section(job.stdout_tail, job.stdout_len, "stdout");
    const stderr = section(job.stderr_tail, job.stderr_len, "stderr");
    const parts = [];
    if (stdout) parts.push(stdout);
    if (stderr) parts.push((stdout ? "[STDERR]\n" : "") + stderr);
    if (String(job.status || "").toLowerCase() !== "running") {
      const code = job.exit_code == null ? bt("shellUnknownExit") : String(job.exit_code);
      parts.push(bt("shellTaskFinished")(code));
    }
    return parts.join("\n");
  }

  function shellCommandForItem(item) {
    return item && item.args && typeof item.args.command === "string" ? item.args.command : "";
  }

  function shellSnapshotKey(job) {
    return JSON.stringify([
      job.id, job.status, job.exit_code, job.stdout_len, job.stderr_len,
      job.stdout_tail, job.stderr_tail,
    ]);
  }

  function terminalShellHistoryMatch(item, job) {
    if (!item || item.type !== "tool" || item.taskId || item.state === "running" ||
        !isShellExecutionTool(item.name) || shellCommandForItem(item) !== String(job.command || "")) {
      return false;
    }
    const output = normalizeTerminalTail(String(item.output || ""));
    if (output.includes(String(job.id || "")) && job.id) return true;
    const evidence = [job.stdout_tail, job.stderr_tail].map(function (raw) {
      return normalizeTerminalTail(String(raw || "").replace(/^\.\.\.\s*/, "")).trim();
    }).filter(Boolean);
    if (evidence.length) return evidence.every(function (text) { return output.includes(text); });
    return /\(no output\)|no output|无输出|出力なし/i.test(output);
  }

  function applyShellSnapshots(sid, jobs) {
    let anyRunning = false;
    let changed = false;
    const runningCommandCounts = {};
    (jobs || []).forEach(function (job) {
      if (String(job.status || "").toLowerCase() !== "running") return;
      const command = String(job.command || "");
      runningCommandCounts[command] = (runningCommandCounts[command] || 0) + 1;
    });
    runSyncOnSession(sid, function () {
      (jobs || []).forEach(function (job) {
        const status = String(job.status || "").toLowerCase();
        const running = status === "running";
        if (running) anyRunning = true;
        let item = state.chatItems.find(function (it) {
          return it.type === "tool" && it.taskId === job.id;
        });
        if (!item && running) {
          const command = String(job.command || "");
          const candidates = state.chatItems.filter(function (it) {
            return it.type === "tool" && isShellExecutionTool(it.name) && !it.taskId &&
              it.state === "running" && shellCommandForItem(it) === command;
          });
          // Command text is only a temporary bridge until tool_end exposes the
          // task id. Never guess when identical commands are concurrent.
          if (runningCommandCounts[command] === 1 && candidates.length === 1) item = candidates[0];
        }
        if (!item && !running) {
          item = state.chatItems.find(function (it) {
            return terminalShellHistoryMatch(it, job);
          });
          if (item) item.shellHistoryReconciled = true;
        }
        // A detached job may have been started by a subagent, so no matching
        // top-level tool card exists. Completed jobs must also get a card: the
        // first poll may happen after a short detached process already exited.
        if (!item) {
          item = {
            type: "tool", toolId: "shell-task:" + job.id, name: "exec_shell",
            args: { command: job.command || "" }, output: null, success: null,
            state: running ? "running" : "failed", shellSnapshot: true,
          };
          addChatItem(item);
          changed = true;
        }
        const snapshotKey = shellSnapshotKey(job);
        if (item.shellSnapshotKey === snapshotKey) return;
        item.taskId = job.id;
        item.sessionId = sid;
        item.shellStatus = job.status;
        item.exitCode = job.exit_code;
        item.elapsedMs = job.elapsed_ms;
        if (!item.shellHistoryReconciled || item.output == null || running) {
          item.output = formatShellSnapshot(job);
        }
        item.state = running ? "running" : (status === "completed" ? "done" : "failed");
        item.success = running ? null : status === "completed";
        item.shellSnapshotKey = snapshotKey;
        changed = true;
      });
    });
    if (changed) notify();
    return anyRunning;
  }

  function scheduleShellPoll(sid, immediate) {
    if (!sid) return;
    if (!shellPollState[sid]) shellPollState[sid] = {
      timer: null, inFlight: false, waitBudget: 0,
    };
    const poll = shellPollState[sid];
    poll.waitBudget = Math.max(poll.waitBudget, 12);
    if (poll.timer || poll.inFlight) return;
    poll.timer = setTimeout(function () { runShellPoll(sid); }, immediate ? 0 : 250);
  }

  async function runShellPoll(sid) {
    const poll = shellPollState[sid];
    if (!poll || poll.inFlight) return;
    poll.timer = null;
    poll.inFlight = true;
    let running = false;
    try {
      const jobs = await invoke("list_shell_tasks", { sessionId: sid });
      running = applyShellSnapshots(sid, Array.isArray(jobs) ? jobs : []);
      if (!running) poll.waitBudget = Math.max(0, poll.waitBudget - 1);
    } catch (error) {
      console.warn("shell task polling failed", error);
      poll.waitBudget = Math.max(0, poll.waitBudget - 1);
    } finally {
      poll.inFlight = false;
    }
    if (running || poll.waitBudget > 0) {
      poll.timer = setTimeout(function () { runShellPoll(sid); }, 250);
    } else {
      delete shellPollState[sid];
    }
  }

  async function cancelTrackedShellTask(sessionId, taskId) {
    const sid = sessionId || state.activeSessionId;
    if (!sid || !taskId) return;
    try {
      await invoke("cancel_shell_task", { sessionId: sid, taskId });
    } finally {
      scheduleShellPoll(sid, true);
    }
  }
  function scheduleShellNotify() {
    if (shellNotifyTimer != null) return;
    shellNotifyTimer = window.setTimeout(function () {
      shellNotifyTimer = null;
      notify();
    }, 50);
  }

  function markBackgroundToolItem(toolId, sessionId, taskId, fallbackOutput) {
    for (let i = 0; i < state.chatItems.length; i++) {
      const item = state.chatItems[i];
      if (item.type !== "tool" || item.toolId !== toolId) continue;
      if (!item.liveOutput && fallbackOutput != null) item.output = fallbackOutput;
      item.success = null;
      item.state = "running";
      item.background = true;
      item.sessionId = sessionId || state.activeSessionId;
      item.taskId = taskId;
      return true;
    }
    return false;
  }

  function finishBackgroundToolItem(toolId, payload) {
    for (let i = 0; i < state.chatItems.length; i++) {
      const item = state.chatItems[i];
      if (item.type !== "tool" || item.toolId !== toolId) continue;
      const status = payload.status || "Failed";
      const success = status === "Completed";
      item.success = success;
      item.state = success ? "done" : "failed";
      item.background = false;
      item.shellStatus = status;
      item.exitCode = payload.exit_code;
      item.output = reconcileBackgroundTerminalOutput(item.output, payload);
      delete item._terminalParser;
      return true;
    }
    return false;
  }

  const MAX_PENDING_TERMINAL_SEQUENCE_CHARS = 16 * 1024;
  function rememberPendingTerminalSequence(parserState, input, start) {
    const pending = input.slice(start);
    // A malformed unterminated OSC/DCS sequence must not bypass the live
    // output tail limit and grow renderer memory without bound.
    parserState.pendingAnsi = pending.length <= MAX_PENDING_TERMINAL_SEQUENCE_CHARS ? pending : "";
  }

  // eslint-disable-next-line sonarjs/cognitive-complexity -- state machine parses terminal sequences byte by byte; refactoring needs its own regression pass, kept as-is for now
  function stripTerminalSequences(text, parserState) {
    const input = String((parserState.pendingAnsi || "") + (text || ""));
    parserState.pendingAnsi = "";
    let clean = "";
    for (let i = 0; i < input.length; i++) {
      if (input[i] !== "\x1B") {
        clean += input[i];
        continue;
      }
      if (i + 1 >= input.length) {
        rememberPendingTerminalSequence(parserState, input, i);
        break;
      }

      const kind = input[i + 1];
      if (kind === "[") {
        let csiEnd = i + 2;
        let malformedCsi = false;
        while (csiEnd < input.length) {
          // ANSI sequences are scanned bytewise; charCode is the protocol byte value, codePointAt adds nothing here.
          const csiCode = input.charCodeAt(csiEnd); // eslint-disable-line unicorn/prefer-code-point
          if (csiCode >= 0x40 && csiCode <= 0x7e) break;
          if (csiCode < 0x20 || csiCode > 0x3f) {
            malformedCsi = true;
            break;
          }
          csiEnd += 1;
        }
        if (malformedCsi) {
          i += 1;
          continue;
        }
        if (csiEnd >= input.length) {
          rememberPendingTerminalSequence(parserState, input, i);
          break;
        }
        i = csiEnd; // eslint-disable-line sonarjs/updated-loop-counter -- cursor advance skipping the entire CSI sequence
        continue;
      }

      // OSC/DCS/SOS/PM/APC are terminated by ST (ESC \); OSC also accepts BEL.
      if (["]", "P", "X", "^", "_"].includes(kind)) {
        let stringEnd = i + 2;
        let terminated = false;
        while (stringEnd < input.length) {
          if (kind === "]" && input[stringEnd] === "\x07") {
            terminated = true;
            break;
          }
          if (input[stringEnd] === "\x1B" && input[stringEnd + 1] === "\\") {
            stringEnd += 1;
            terminated = true;
            break;
          }
          stringEnd += 1;
        }
        if (!terminated) {
          rememberPendingTerminalSequence(parserState, input, i);
          break;
        }
        i = stringEnd; // eslint-disable-line sonarjs/updated-loop-counter -- cursor advance skipping the entire OSC/DCS sequence
        continue;
      }

      // Generic two-or-more-byte escape sequence: optional intermediate
      // bytes followed by a final byte.
      let escapeEnd = i + 1;
      while (escapeEnd < input.length) {
        // ANSI sequences are scanned bytewise; charCode is the protocol byte value, codePointAt adds nothing here.
        const escapeCode = input.charCodeAt(escapeEnd); // eslint-disable-line unicorn/prefer-code-point
        if (escapeCode < 0x20 || escapeCode > 0x2f) break;
        escapeEnd += 1;
      }
      if (escapeEnd >= input.length) {
        rememberPendingTerminalSequence(parserState, input, i);
        break;
      }
      // ANSI sequences are scanned bytewise; charCode is the protocol byte value, codePointAt adds nothing here.
      const finalCode = input.charCodeAt(escapeEnd); // eslint-disable-line unicorn/prefer-code-point
      // eslint-disable-next-line sonarjs/updated-loop-counter -- cursor advance skipping the entire escape sequence
      if (finalCode >= 0x30 && finalCode <= 0x7e) i = escapeEnd;
    }
    return clean;
  }

  function terminalParserState(item, stream) {
    if (!item._terminalParser) {
      Object.defineProperty(item, "_terminalParser", {
        value: {},
        writable: true,
        configurable: true,
      });
    }
    const key = stream === "stderr" ? "stderr" : "stdout";
    if (!item._terminalParser[key]) {
      item._terminalParser[key] = { pendingCR: false, pendingAnsi: "" };
    }
    return item._terminalParser[key];
  }

  // A standalone carriage return resets the current terminal line. WinGet
  // uses this for progress frames, so keep the newest frame instead of
  // appending hundreds of nearly identical lines.
  function mergeTerminalChunk(previous, chunk, parserState, prefix) {
    let output = String(previous == null ? "" : previous);
    const clean = stripTerminalSequences(chunk, parserState);
    let i = 0;
    if (parserState.pendingCR && clean) {
      if (clean[0] === "\n") {
        output += "\n";
        i = 1;
      } else {
        output = output.slice(0, output.lastIndexOf("\n") + 1);
      }
      parserState.pendingCR = false;
    }
    let needsPrefix = !!prefix;
    for (; i < clean.length; i++) {
      const ch = clean[i];
      if (ch === "\r") {
        if (clean[i + 1] === "\n") {
          output += "\n";
          i += 1;
        } else if (i + 1 >= clean.length) {
          parserState.pendingCR = true;
        } else {
          output = output.slice(0, output.lastIndexOf("\n") + 1);
        }
      } else if (ch === "\b") {
        const lineStart = output.lastIndexOf("\n") + 1;
        if (output.length > lineStart) output = output.slice(0, -1);
      } else {
        if (needsPrefix) {
          output += prefix;
          needsPrefix = false;
        }
        output += ch;
      }
    }
    return output;
  }

  function mergeTerminalTail(previous, tail) {
    const output = String(previous == null ? "" : previous);
    const suffix = String(tail == null ? "" : tail);
    if (!suffix) return output;
    if (!output) return suffix;
    if (output.includes(suffix)) return output;

    const maxOverlap = Math.min(output.length, suffix.length);
    for (let overlap = maxOverlap; overlap > 0; overlap--) {
      if (output.slice(-overlap) === suffix.slice(0, overlap)) {
        return output + suffix.slice(overlap);
      }
    }
    return output + (output.endsWith("\n") || suffix.startsWith("\n") ? "" : "\n") + suffix;
  }

  function normalizeTerminalTail(tail, prefix) {
    if (!tail) return "";
    return mergeTerminalChunk(
      "",
      tail,
      { pendingCR: false, pendingAnsi: "" },
      prefix || ""
    );
  }

  function reconcileBackgroundTerminalOutput(previous, payload) {
    let output = String(previous == null ? "" : previous);
    output = mergeTerminalTail(output, normalizeTerminalTail(payload.stdout_tail, ""));
    output = mergeTerminalTail(output, normalizeTerminalTail(payload.stderr_tail, "[STDERR] "));
    return output;
  }

  // Live shell output is display-only. The completed tool result remains the
  // authoritative value written to conversation history/model context.
  function appendToolItemOutput(toolId, content, stream) {
    const chunk = typeof content === "string" ? content : String(content == null ? "" : content);
    if (!chunk) return false;
    for (let i = 0; i < state.chatItems.length; i++) {
      const item = state.chatItems[i];
      if (item.type !== "tool" || item.toolId !== toolId) continue;
      const parserState = terminalParserState(item, stream);
      let output = mergeTerminalChunk(
        item.output,
        chunk,
        parserState,
        stream === "stderr" ? "[STDERR] " : ""
      );
      // A verbose long-running process must not grow renderer memory without
      // bound. Completion replaces this tail with the normal full result.
      const maxLiveChars = 128 * 1024;
      if (output.length > maxLiveChars) output = "…\n" + output.slice(-maxLiveChars);
      item.output = output;
      item.liveOutput = true;
      return true;
    }
    return false;
  }


    return {
      updateToolItem,
      isShellExecutionTool,
      utf8Length,
      formatShellSnapshot,
      shellCommandForItem,
      shellSnapshotKey,
      terminalShellHistoryMatch,
      applyShellSnapshots,
      scheduleShellPoll,
      runShellPoll,
      cancelTrackedShellTask,
      scheduleShellNotify,
      markBackgroundToolItem,
      finishBackgroundToolItem,
      rememberPendingTerminalSequence,
      stripTerminalSequences,
      terminalParserState,
      mergeTerminalChunk,
      mergeTerminalTail,
      normalizeTerminalTail,
      reconcileBackgroundTerminalOutput,
      appendToolItemOutput
    };
  };
})(window);

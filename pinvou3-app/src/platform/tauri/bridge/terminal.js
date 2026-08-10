/** Shell polling and terminal output normalization for bridge tool cards. */
(function (root) {
  "use strict";
  var registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry.terminal = function (context) {
    var state = context.state;
    var notify = context.notify;
    var invoke = context.invoke;
    var bt = context.bt;
    var runSyncOnSession = context.runSyncOnSession;
    var addChatItem = context.addChatItem;
    var shellNotifyTimer = null;
    var shellPollState = Object.create(null);
  function updateToolItem(toolId, output, success) {
    for (var i = 0; i < state.chatItems.length; i++) {
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
    return ["exec_shell", "exec_shell_wait", "exec_wait", "task_shell_start", "task_shell_wait", "shell", "Bash"].indexOf(name) >= 0;
  }

  function utf8Length(text) {
    try { return new TextEncoder().encode(String(text || "")).length; }
    catch (_) { return String(text || "").length; }
  }

  // Shell snapshots are a tail view, not an append-only byte stream. Normalize
  // terminal control sequences and state omissions explicitly instead of
  // pretending the visible tail is the complete log.
  function normalizeTerminalTail(text) {
    var value = String(text || "")
      .replace(/\x1b\][^\x07]*(?:\x07|\x1b\\)/g, "")
      .replace(/\x1b\[[0-?]*[ -\/]*[@-~]/g, "");
    var out = [];
    value.split("\n").forEach(function (line) {
      // After splitting on LF, a normal Windows CRLF line still ends in CR.
      // Remove that delimiter first; only an *internal* CR means a terminal
      // progress line overwrote earlier content on the same row.
      var visible = line.endsWith("\r") ? line.slice(0, -1) : line;
      var overwriteAt = visible.lastIndexOf("\r");
      if (overwriteAt >= 0) visible = visible.slice(overwriteAt + 1);
      while (visible.indexOf("\x08") >= 0) {
        visible = visible.replace(/[^\x08]\x08/g, "").replace(/^\x08+/, "");
      }
      out.push(visible);
    });
    return out.join("\n");
  }

  function formatShellSnapshot(job) {
    function section(raw, total, kind) {
      raw = String(raw || "");
      var visibleRaw = raw.replace(/^\.\.\.\s*/, "");
      var omitted = /^\.\.\./.test(raw) || Number(total || 0) > utf8Length(visibleRaw);
      var body = normalizeTerminalTail(visibleRaw);
      if (omitted) body = bt("shellOutputOmitted")(kind) + "\n" + body;
      return body;
    }
    var stdout = section(job.stdout_tail, job.stdout_len, "stdout");
    var stderr = section(job.stderr_tail, job.stderr_len, "stderr");
    var parts = [];
    if (stdout) parts.push(stdout);
    if (stderr) parts.push((stdout ? "[STDERR]\n" : "") + stderr);
    if (String(job.status || "").toLowerCase() !== "running") {
      var code = job.exit_code == null ? bt("shellUnknownExit") : String(job.exit_code);
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
    var output = normalizeTerminalTail(String(item.output || ""));
    if (output.indexOf(String(job.id || "")) >= 0 && job.id) return true;
    var evidence = [job.stdout_tail, job.stderr_tail].map(function (raw) {
      return normalizeTerminalTail(String(raw || "").replace(/^\.\.\.\s*/, "")).trim();
    }).filter(Boolean);
    if (evidence.length) return evidence.every(function (text) { return output.indexOf(text) >= 0; });
    return /\(no output\)|no output|无输出|出力なし/i.test(output);
  }

  function applyShellSnapshots(sid, jobs) {
    var anyRunning = false;
    var changed = false;
    var runningCommandCounts = {};
    (jobs || []).forEach(function (job) {
      if (String(job.status || "").toLowerCase() !== "running") return;
      var command = String(job.command || "");
      runningCommandCounts[command] = (runningCommandCounts[command] || 0) + 1;
    });
    runSyncOnSession(sid, function () {
      (jobs || []).forEach(function (job) {
        var status = String(job.status || "").toLowerCase();
        var running = status === "running";
        if (running) anyRunning = true;
        var item = state.chatItems.find(function (it) {
          return it.type === "tool" && it.taskId === job.id;
        });
        if (!item && running) {
          var command = String(job.command || "");
          var candidates = state.chatItems.filter(function (it) {
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
        var snapshotKey = shellSnapshotKey(job);
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
    var poll = shellPollState[sid] || (shellPollState[sid] = {
      timer: null, inFlight: false, waitBudget: 0,
    });
    poll.waitBudget = Math.max(poll.waitBudget, 12);
    if (poll.timer || poll.inFlight) return;
    poll.timer = setTimeout(function () { runShellPoll(sid); }, immediate ? 0 : 250);
  }

  async function runShellPoll(sid) {
    var poll = shellPollState[sid];
    if (!poll || poll.inFlight) return;
    poll.timer = null;
    poll.inFlight = true;
    var running = false;
    try {
      var jobs = await invoke("list_shell_tasks", { sessionId: sid });
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
    var sid = sessionId || state.activeSessionId;
    if (!sid || !taskId) return;
    try {
      await invoke("cancel_shell_task", { sessionId: sid, taskId: taskId });
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
    for (var i = 0; i < state.chatItems.length; i++) {
      var item = state.chatItems[i];
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
    for (var i = 0; i < state.chatItems.length; i++) {
      var item = state.chatItems[i];
      if (item.type !== "tool" || item.toolId !== toolId) continue;
      var status = payload.status || "Failed";
      var success = status === "Completed";
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

  var MAX_PENDING_TERMINAL_SEQUENCE_CHARS = 16 * 1024;
  function rememberPendingTerminalSequence(parserState, input, start) {
    var pending = input.slice(start);
    // A malformed unterminated OSC/DCS sequence must not bypass the live
    // output tail limit and grow renderer memory without bound.
    parserState.pendingAnsi = pending.length <= MAX_PENDING_TERMINAL_SEQUENCE_CHARS ? pending : "";
  }

  function stripTerminalSequences(text, parserState) {
    var input = String((parserState.pendingAnsi || "") + (text || ""));
    parserState.pendingAnsi = "";
    var clean = "";
    for (var i = 0; i < input.length; i++) {
      if (input[i] !== "\x1b") {
        clean += input[i];
        continue;
      }
      if (i + 1 >= input.length) {
        rememberPendingTerminalSequence(parserState, input, i);
        break;
      }

      var kind = input[i + 1];
      if (kind === "[") {
        var csiEnd = i + 2;
        var malformedCsi = false;
        while (csiEnd < input.length) {
          var csiCode = input.charCodeAt(csiEnd);
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
        i = csiEnd;
        continue;
      }

      // OSC/DCS/SOS/PM/APC are terminated by ST (ESC \); OSC also accepts BEL.
      if (kind === "]" || kind === "P" || kind === "X" || kind === "^" || kind === "_") {
        var stringEnd = i + 2;
        var terminated = false;
        while (stringEnd < input.length) {
          if (kind === "]" && input[stringEnd] === "\x07") {
            terminated = true;
            break;
          }
          if (input[stringEnd] === "\x1b" && input[stringEnd + 1] === "\\") {
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
        i = stringEnd;
        continue;
      }

      // Generic two-or-more-byte escape sequence: optional intermediate
      // bytes followed by a final byte.
      var escapeEnd = i + 1;
      while (escapeEnd < input.length) {
        var escapeCode = input.charCodeAt(escapeEnd);
        if (escapeCode < 0x20 || escapeCode > 0x2f) break;
        escapeEnd += 1;
      }
      if (escapeEnd >= input.length) {
        rememberPendingTerminalSequence(parserState, input, i);
        break;
      }
      var finalCode = input.charCodeAt(escapeEnd);
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
    var key = stream === "stderr" ? "stderr" : "stdout";
    if (!item._terminalParser[key]) {
      item._terminalParser[key] = { pendingCR: false, pendingAnsi: "" };
    }
    return item._terminalParser[key];
  }

  // A standalone carriage return resets the current terminal line. WinGet
  // uses this for progress frames, so keep the newest frame instead of
  // appending hundreds of nearly identical lines.
  function mergeTerminalChunk(previous, chunk, parserState, prefix) {
    var output = String(previous == null ? "" : previous);
    var clean = stripTerminalSequences(chunk, parserState);
    var i = 0;
    if (parserState.pendingCR && clean) {
      if (clean[0] === "\n") {
        output += "\n";
        i = 1;
      } else {
        output = output.slice(0, output.lastIndexOf("\n") + 1);
      }
      parserState.pendingCR = false;
    }
    var needsPrefix = !!prefix;
    for (; i < clean.length; i++) {
      var ch = clean[i];
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
        var lineStart = output.lastIndexOf("\n") + 1;
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
    var output = String(previous == null ? "" : previous);
    var suffix = String(tail == null ? "" : tail);
    if (!suffix) return output;
    if (!output) return suffix;
    if (output.indexOf(suffix) >= 0) return output;

    var maxOverlap = Math.min(output.length, suffix.length);
    for (var overlap = maxOverlap; overlap > 0; overlap--) {
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
    var output = String(previous == null ? "" : previous);
    output = mergeTerminalTail(output, normalizeTerminalTail(payload.stdout_tail, ""));
    output = mergeTerminalTail(output, normalizeTerminalTail(payload.stderr_tail, "[STDERR] "));
    return output;
  }

  // Live shell output is display-only. The completed tool result remains the
  // authoritative value written to conversation history/model context.
  function appendToolItemOutput(toolId, content, stream) {
    var chunk = typeof content === "string" ? content : String(content == null ? "" : content);
    if (!chunk) return false;
    for (var i = 0; i < state.chatItems.length; i++) {
      var item = state.chatItems[i];
      if (item.type !== "tool" || item.toolId !== toolId) continue;
      var parserState = terminalParserState(item, stream);
      var output = mergeTerminalChunk(
        item.output,
        chunk,
        parserState,
        stream === "stderr" ? "[STDERR] " : ""
      );
      // A verbose long-running process must not grow renderer memory without
      // bound. Completion replaces this tail with the normal full result.
      var maxLiveChars = 128 * 1024;
      if (output.length > maxLiveChars) output = "…\n" + output.slice(-maxLiveChars);
      item.output = output;
      item.liveOutput = true;
      return true;
    }
    return false;
  }


    return {
      updateToolItem: updateToolItem,
      isShellExecutionTool: isShellExecutionTool,
      utf8Length: utf8Length,
      normalizeTerminalTail: normalizeTerminalTail,
      formatShellSnapshot: formatShellSnapshot,
      shellCommandForItem: shellCommandForItem,
      shellSnapshotKey: shellSnapshotKey,
      terminalShellHistoryMatch: terminalShellHistoryMatch,
      applyShellSnapshots: applyShellSnapshots,
      scheduleShellPoll: scheduleShellPoll,
      runShellPoll: runShellPoll,
      cancelTrackedShellTask: cancelTrackedShellTask,
      scheduleShellNotify: scheduleShellNotify,
      markBackgroundToolItem: markBackgroundToolItem,
      finishBackgroundToolItem: finishBackgroundToolItem,
      rememberPendingTerminalSequence: rememberPendingTerminalSequence,
      stripTerminalSequences: stripTerminalSequences,
      terminalParserState: terminalParserState,
      mergeTerminalChunk: mergeTerminalChunk,
      mergeTerminalTail: mergeTerminalTail,
      normalizeTerminalTail: normalizeTerminalTail,
      reconcileBackgroundTerminalOutput: reconcileBackgroundTerminalOutput,
      appendToolItemOutput: appendToolItemOutput
    };
  };
})(window);

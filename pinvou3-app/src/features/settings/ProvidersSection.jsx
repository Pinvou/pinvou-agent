// 设置页「ACP 管理」（模型设置页内子页，原「Provider 管理」分节）：三 Agent 标签页
// + Provider 卡片（切换/编辑/删除）、导入导出、env 冲突警告、CLI 状态区（安装/更新/卸载）。

import React, { useCallback, useEffect, useRef, useState } from 'react';
import {
  AlertTriangle, ChevronDown, Download, Edit2, Lock, Plus, RefreshCw,
  Terminal, Trash2, Upload, X,
} from '../../components/icons.jsx';
import { invokeTauri, isTauriAvailable, tauriEvents } from '../../platform/tauri/client.js';
import {
  markAcpModelsProbePending,
  reseedDraftControlsAfterProviderSwitch,
} from '../codex/acp-draft-controls.js';
import { ProviderFormModal } from './ProviderFormModal.jsx';

const AGENTS = [
  { key: 'codex', agentId: 'codex' },
  { key: 'claude', agentId: 'claude' },
  { key: 'kimi', agentId: 'kimi' },
];

function WireBadge({ wireApi, agent, copy }) {
  // codex 的记录归一为 openai 但写入器固定 responses，徽标如实显示 Responses；
  // kimi 原生协议显示 Kimi 原生（此前都会误标成 Anthropic 兼容）。
  const label = agent === 'codex'
    ? copy.wireResponses
    : wireApi === 'kimi'
      ? copy.wireKimi
      : wireApi === 'openai'
        ? copy.wireOpenai
        : copy.wireAnthropic;
  const openaiFamily = agent === 'codex' || wireApi === 'openai';
  return (
    <span className={`shrink-0 rounded-md px-1.5 py-0.5 text-[10px] font-semibold ${openaiFamily ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400' : 'bg-violet-500/10 text-violet-600 dark:text-violet-400'}`}>
      {label}
    </span>
  );
}

function KeyDot({ hasCredential, copy }) {
  return (
    <span className={`inline-flex items-center gap-1 text-[11px] ${hasCredential ? 'text-emerald-600 dark:text-emerald-400' : 'text-amber-600 dark:text-amber-400'}`}>
      <Lock size={12} />
      {hasCredential ? copy.keyOk : copy.keyNone}
    </span>
  );
}

// 模块级缓存：设置页关闭导致组件卸载后仍存活，重开时同步水合（无加载闪烁），
// 随后后台静默刷新。键 = agent id；仅在本次 app 运行期内有效。
const PROVIDER_SECTION_CACHE = new Map();

// 安装进度缓存：command 事件只在安装开始时发一次，设置面板/App 关闭重开后
// 组件重新订阅事件流时已收不到 command——缓存让重开后面板仍能显示「执行
// 命令」与「最新输出」，后续事件继续刷新最新行。log 含 agent；phase 同步
// 缓存（结束态保留供重开查看，安装结束不再自动收起）。
const INSTALL_LOG_CACHE = { log: null, phase: null };

export function ProvidersSection({ t }) {
  const copy = t.uiAcpProviders;
  const [activeAgent, setActiveAgent] = useState('codex');
  const [view, setView] = useState(() => (PROVIDER_SECTION_CACHE.get('codex') || {}).view || null);
  const [status, setStatus] = useState(() => (PROVIDER_SECTION_CACHE.get('codex') || {}).status || null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [formOpen, setFormOpen] = useState(false);
  const [editing, setEditing] = useState(null);
  const [deleteConfirm, setDeleteConfirm] = useState(null);
  const [uninstallConfirm, setUninstallConfirm] = useState(null);
  const [busy, setBusy] = useState('');
  // busy 按 Agent 隔离：busy 记录触发时的标签页（后端安装/升级本就按 Agent
  // 互斥），切换标签页后其他 Agent 的按钮不被全局 busy 堵住。busyAgent 在
  // clearBusy 时清空，busyOnAgent 是「当前标签页有操作进行中」的派生值。
  const [busyAgent, setBusyAgent] = useState(null);
  const runBusy = op => {
    setBusy(op);
    setBusyAgent(activeAgent);
  };
  const clearBusy = () => {
    setBusy('');
    setBusyAgent(null);
  };
  const busyOnAgent = busy !== '' && busyAgent === activeAgent;
  const [notice, setNotice] = useState('');
  const [exportOpen, setExportOpen] = useState(false);
  const [exportJson, setExportJson] = useState('');
  const [importOpen, setImportOpen] = useState(false);
  const [importJson, setImportJson] = useState('');
  const [loginWaiting, setLoginWaiting] = useState(false);
  const [loginCode, setLoginCode] = useState('');
  const loginPollRef = useRef(null);
  const noticeTimer = useRef(null);
  // CLI 区「重新检测」局部 busy：recheck=true 会强制后端重新 spawn --version
  // 探测（最长 15s），期间必须给按钮转圈反馈，否则看起来「按了没反应」。
  // recheckingAgent 按标签页隔离：按一个只转当前标签页的按钮（后端本就只
  // 重探测该 Agent，其他标签页不该显示「在检测」）。
  const [rechecking, setRechecking] = useState(false);
  const [recheckingAgent, setRecheckingAgent] = useState(null);
  const recheckingOnAgent = rechecking && recheckingAgent === activeAgent;
  // 安装进度展示：installLog = { agent, command, line }（实际命令行 + 输出最新
  // 一行，来自 acp:install-progress 事件）；installPhase = checking/installing/
  // done/failed/cancelled 阶段标签。结束态保留展示（不自动收起），下次安装
  // 或重开窗口从缓存恢复。
  const [installLog, setInstallLog] = useState(INSTALL_LOG_CACHE.log);
  const [installPhase, setInstallPhase] = useState(() => INSTALL_LOG_CACHE.phase);
  // 阶段同步进模块缓存：关设置窗口重开后恢复结束态/进行中态展示。
  useEffect(() => {
    INSTALL_LOG_CACHE.phase = installPhase;
  }, [installPhase]);
  // 安装中状态从 status 派生而非本地 busy：设置页关闭重开后仍能恢复进行中 UI。
  const installing = Boolean(status && status.installing);

  const stopLoginPoll = () => {
    if (loginPollRef.current) {
      window.clearInterval(loginPollRef.current);
      loginPollRef.current = null;
    }
  };

  const startLoginPoll = agent => {
    stopLoginPoll();
    loginPollRef.current = window.setInterval(async () => {
      try {
        const next = await invokeTauri('get_acp_agent_status', { agentId: agent, recheck: true });
        setStatus(next);
        if (!next || !next.login_in_progress || next.authenticated) {
          stopLoginPoll();
          setLoginWaiting(false);
          setLoginCode('');
          refresh(true);
        }
      } catch {
        stopLoginPoll();
        setLoginWaiting(false);
      }
    }, 1500);
  };

  const startLogin = async () => {
    runBusy('login:' + activeAgent);
    setError('');
    try {
      const next = await invokeTauri('login_acp_agent', { agentId: activeAgent });
      setStatus(next);
      if (next && next.login_in_progress) {
        setLoginWaiting(true);
        startLoginPoll(activeAgent);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      clearBusy();
    }
  };

  const submitLoginCode = async () => {
    runBusy('login:' + activeAgent);
    try {
      const next = await invokeTauri('submit_acp_agent_login_code', {
        agentId: activeAgent,
        code: loginCode.trim(),
      });
      setStatus(next);
      setLoginCode('');
    } catch (e) {
      setError(String(e));
    } finally {
      clearBusy();
    }
  };

  const doLogout = async () => {
    runBusy('logout:' + activeAgent);
    try {
      await invokeTauri('logout_acp_agent', { agent: activeAgent });
      notify(copy.loggedOut);
      await refresh(true);
    } catch (e) {
      setError(String(e));
    } finally {
      clearBusy();
    }
  };

  const notify = message => {
    setNotice(message);
    if (noticeTimer.current) window.clearTimeout(noticeTimer.current);
    noticeTimer.current = window.setTimeout(() => setNotice(''), 3200);
  };

  // recheck=true 强制后端忽略探测缓存重新检测 CLI：安装/卸载/切换后的状态
  // 必须反映真实环境（读缓存会出现「装上了却没有卸载按钮」）。
  // quiet=true 跳过 loading spinner（后台轮询/标签页切换用，页面不闪烁）。
  // 缓存抬到模块级（PROVIDER_SECTION_CACHE）：设置页关闭重开/切换标签页都
  // 先显示缓存、后台静默更新，不再闪加载态。
  // loadAgent 按 agent 加载并写缓存，仅当该 agent 仍是当前标签页时更新展示；
  // 进入页面即三路并行加载（见下方 effect），切标签页全部读缓存秒开。
  const activeAgentRef = useRef(activeAgent);
  activeAgentRef.current = activeAgent;
  const loadAgent = useCallback(async (agent, recheck = false, quiet = false) => {
    if (!quiet && agent === activeAgentRef.current) setLoading(true);
    try {
      const [nextView, nextStatus] = await Promise.all([
        invokeTauri('list_acp_providers', { agent }),
        invokeTauri('get_acp_agent_status', recheck
          ? { agentId: agent, recheck: true }
          : { agentId: agent }),
      ]);
      PROVIDER_SECTION_CACHE.set(agent, { view: nextView, status: nextStatus });
      if (agent === activeAgentRef.current) {
        setView(nextView);
        setStatus(nextStatus);
      }
    } catch (e) {
      if (agent === activeAgentRef.current) setError(String(e));
    } finally {
      if (!quiet && agent === activeAgentRef.current) setLoading(false);
    }
  }, []);
  const refresh = useCallback(
    (recheck = false, quiet = false) => loadAgent(activeAgentRef.current, recheck, quiet),
    [loadAgent],
  );

  // CLI 区「重新检测」：强制后端重探测并给按钮转圈反馈（recheck 探测最长 15s）。
  const recheck = async () => {
    setRechecking(true);
    setRecheckingAgent(activeAgent);
    try {
      await refresh(true);
    } finally {
      setRechecking(false);
      setRecheckingAgent(null);
    }
  };

  useEffect(() => {
    setError('');
    // 进入页面/切换标签页：三个 Agent 并行加载。当前标签页有缓存先展示，
    // 其余后台静默刷新；无缓存的标签页照常显示加载态。
    for (const agent of AGENTS) {
      const cached = PROVIDER_SECTION_CACHE.get(agent.key);
      const hasCache = Boolean(cached && (cached.view || cached.status));
      if (agent.key === activeAgent && hasCache) {
        setView(cached.view || null);
        setStatus(cached.status || null);
      }
      if (agent.key === activeAgent && !hasCache) {
        setView(null);
        setStatus(null);
      }
      loadAgent(agent.key, false, agent.key !== activeAgent);
    }
  }, [loadAgent, activeAgent]);

  // 标签页切换/卸载时停掉登录轮询，避免把状态写到别的 Agent 上
  useEffect(() => {
    return () => {
      stopLoginPoll();
      setLoginWaiting(false);
    };
  }, [activeAgent]);

  // 安装中轮询：installing 期间每 2 秒读一次状态。installing 标志来自后端
  // 内存里的安装锁，与 CLI 探测无关——用缓存读（不 recheck）+ quiet（不闪
  // loading），避免每 2 秒真跑一遍 --version 探测造成的页面抖动。
  // 本地 busy 也触发轮询：安装命令从发起到后端登记安装锁之间有探测延迟，
  // 靠轮询把 status.installing 接上（取消按钮随之出现）。
  const installInFlight = installing || busy === 'install:' + activeAgent;
  useEffect(() => {
    if (!installInFlight) return undefined;
    const timer = window.setInterval(() => {
      refresh(false, true);
    }, 2000);
    return () => window.clearInterval(timer);
  }, [installInFlight, refresh]);

  // 安装进度事件：后端逐行推送（kind=command 为实际执行的命令行，stdout/stderr
  // 为输出最新一行，80ms 限流）。只消费当前标签页 Agent 的事件，避免切页串扰。
  useEffect(() => {
    if (!isTauriAvailable()) return undefined;
    let unlisten = null;
    tauriEvents
      .listen('acp:install-progress', event => {
        const payload = event.payload || {};
        if (payload.agent !== activeAgentRef.current) return;
        if (payload.kind === 'command') {
          setInstallPhase('installing');
          applyInstallLog({ agent: payload.agent, command: payload.value });
        } else if (payload.value) {
          applyInstallLog({ agent: payload.agent, line: payload.value });
        }
      })
      .then(fn => {
        unlisten = fn;
      });
    return () => {
      if (unlisten) unlisten();
    };
  }, []);

  // 设置面板/App 关闭重开后恢复安装进度卡：
  // 1) 模块缓存里有（关设置窗口，App 未退出）→ 恢复缓存阶段（进行中/结束态）；
  // 2) 缓存空但 status 带进度（App 重启且安装仍在进行）→ 恢复 installing，
  //    command/latest 行取自后端 status（installCommand/installLatestLine）。
  useEffect(() => {
    const cachedLog = INSTALL_LOG_CACHE.log;
    if (cachedLog && cachedLog.agent === activeAgent && INSTALL_LOG_CACHE.phase) {
      setInstallPhase(INSTALL_LOG_CACHE.phase);
      return;
    }
    if (status && status.installing && status.install_command) {
      setInstallPhase('installing');
      applyInstallLog(prev => ({
        ...(prev || {}),
        agent: activeAgent,
        command: status.install_command,
        line: status.install_latest_line || (prev && prev.line),
      }));
    }
  }, [status, activeAgent]);

  useEffect(() => {
    return () => {
      if (noticeTimer.current) window.clearTimeout(noticeTimer.current);
    };
  }, []);

  const switchTo = async (providerId, official) => {
    runBusy(official ? 'official' : providerId);
    try {
      if (official) {
        await invokeTauri('switch_acp_provider_official', { agent: activeAgent });
      } else {
        await invokeTauri('switch_acp_provider', { agent: activeAgent, providerId });
      }
      // 草稿态的模型/配置快照来自旧 Provider 的会话上报：用新 Provider 的
      // 模型重写（选择器立即可见且名字正确）；恢复官方/无模型时失效快照。
      // Claude 按真实会话形态 seed 别名列表（default/sonnet/opus/haiku/fable，
      // 显示名为槽位映射值），草稿下拉里五个选项与真实会话一致。
      const switchedRecord = official
        ? null
        : (view && view.providers || []).find(provider => provider.id === providerId) || null;
      // kimi 未填 model 时 writer 会兜底写 KIMI_DEFAULT_MODEL（kimi-k3，
      // providers/mod.rs），seed 用同一兜底值而不是失效快照；
      // codex 无确定默认，仍失效。
      const switchedModel = switchedRecord
        ? switchedRecord.model || (activeAgent === 'kimi' ? 'kimi-k3' : null)
        : null;
      let modelEntries = null;
      let currentModel = switchedModel;
      if (switchedRecord && activeAgent === 'kimi' && switchedModel) {
        // kimi 上报的模型选项 id 是 config.toml 的 models 表名（pv-xxx-main）：
        // seed 必须用同一形态，否则会被中转激活时的 pv-* 过滤器清掉
        modelEntries = [{ id: `${providerId}-main`, name: switchedModel }];
        currentModel = `${providerId}-main`;
      }
      if (switchedRecord && activeAgent === 'claude' && switchedRecord.modelSlots) {
        const slots = switchedRecord.modelSlots;
        const fallbackName = switchedModel || slots.sonnet || slots.opus || '';
        if (fallbackName) {
          const nameFor = alias => (alias === 'default' ? fallbackName : (slots[alias] || fallbackName));
          modelEntries = ['default', 'sonnet', 'opus', 'haiku', 'fable']
            .map(alias => ({ id: alias, name: nameFor(alias) }));
          currentModel = 'default';
        }
      }
      reseedDraftControlsAfterProviderSwitch(activeAgent, currentModel, modelEntries);
      // reseed 只是即时占位：打一次性探针标记，对话页草稿态会真实连接一次
      // ACP 拉取新 Provider 的模型列表覆盖占位，之后恢复懒加载。
      markAcpModelsProbePending(activeAgent);
      notify(official ? copy.restoredOfficial : copy.switchOk);
      await refresh(true);
    } catch (e) {
      setError(String(e));
    } finally {
      clearBusy();
    }
  };

  const remove = async () => {
    runBusy('delete');
    // 点击确认即关闭弹窗：失败时不留滞，错误直接显示在红错区（与卸载一致）。
    setDeleteConfirm(null);
    try {
      // 删除当前 Provider 会触发后端自动恢复官方：官方默认模型无法预知，
      // 快照失效（传 null）；删非当前 Provider 不影响草稿快照
      const deletingCurrent = Boolean(view && view.currentProviderId === deleteConfirm.id);
      await invokeTauri('delete_acp_provider', { agent: activeAgent, providerId: deleteConfirm.id });
      if (deletingCurrent) {
        reseedDraftControlsAfterProviderSwitch(activeAgent, null);
        // 删除当前 Provider 即回到官方：同样探一次拿官方真实模型列表。
        markAcpModelsProbePending(activeAgent);
      }
      notify(copy.deleted);
      await refresh(true);
    } catch (e) {
      setError(String(e));
    } finally {
      clearBusy();
    }
  };

  // 安装日志更新/清空同步模块级缓存：设置面板关闭重开后「执行命令」行不丢
  //（command 事件只在安装开始时发一次，重挂载后收不到）。
  const applyInstallLog = patch => {
    setInstallLog(prev => {
      const next = { ...(prev || {}), ...patch };
      INSTALL_LOG_CACHE.log = next;
      return next;
    });
  };
  const resetInstallLog = () => {
    INSTALL_LOG_CACHE.log = null;
    setInstallLog(null);
  };

  const installOrUpdate = async action => {
    runBusy('install:' + activeAgent);
    resetInstallLog();
    // 自检中：后端开始前跑预检（脚本源可达性/坏残留），通过后第一条
    // acp:install-progress command 事件切换为「下载安装中」。
    setInstallPhase('checking');
    try {
      // 安装命令要等装完才返回；先刷新一次让 status.installing 变 true
      // （取消按钮立即可见、安装中轮询启动），再收口安装结果。
      const pending = invokeTauri('install_acp_agent', { agent: activeAgent, action: action || null });
      await refresh(true);
      await pending;
      // 结束态保留展示（不自动收起）：关设置窗口重开仍能看到结果。
      setInstallPhase('done');
    } catch (e) {
      // 后端把取消写成失败退出；这里按结构化标记收口为「已取消」，不重复报
      // 红错（不依赖中文文案子串，复审低危 5）。
      if (String(e).includes('install_cancelled')) {
        setInstallPhase('cancelled');
      } else {
        setInstallPhase('failed');
        setError(String(e));
      }
    } finally {
      clearBusy();
      await refresh(true);
    }
  };

  // 取消安装：后端按登记的 pid 杀安装进程树，等待侧以「安装已取消」收尾
  const cancelInstall = async () => {
    runBusy('cancel-install:' + activeAgent);
    try {
      await invokeTauri('cancel_acp_agent_install', { agent: activeAgent });
      // 已取消阶段保留展示（不自动收起）：重开窗口仍能看到结果。
      setInstallPhase('cancelled');
      notify(copy.installCancelled);
      await refresh(true);
    } catch (e) {
      setError(String(e));
    } finally {
      clearBusy();
    }
  };

  const uninstall = async () => {
    runBusy('uninstall:' + activeAgent);
    // 点击确认即关闭弹窗：后端失败（如运行中会话拦截/文件被占用）时弹窗
    // 不留滞，错误直接显示在红错区——否则弹窗盖住错误，看起来「点了没反应」。
    setUninstallConfirm(null);
    try {
      const next = await invokeTauri('uninstall_acp_agent', {
        agent: activeAgent,
        cleanup: uninstallConfirm.cleanup,
      });
      // 另一渠道仍有安装时后端通过 status.error 告知（「已卸载但仍有另一份」）
      if (next && next.error) {
        setError(String(next.error));
      } else {
        notify(copy.uninstallDone);
      }
      await refresh(true);
    } catch (e) {
      setError(String(e));
    } finally {
      clearBusy();
    }
  };

  const doExport = async () => {
    try {
      setExportJson(await invokeTauri('export_acp_providers', { agent: activeAgent }));
      setExportOpen(true);
    } catch (e) {
      setError(String(e));
    }
  };

  const doImport = async () => {
    runBusy('import');
    try {
      const result = await invokeTauri('import_acp_providers', {
        agent: activeAgent,
        json: importJson,
      });
      setImportOpen(false);
      setImportJson('');
      notify(copy.importDone(result));
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      clearBusy();
    }
  };

  const runInstallAction =
    status && status.installed
      ? status.update_available || status.update_required
        ? 'update'
        : null
      : status
        ? 'install'
        : null;

  // 安装进度阶段标签；进度卡只显示当前标签页 Agent 的日志（checking 阶段尚无
  // 日志时按 busy 匹配，避免切页后把别的 Agent 的进行中态渲染出来）。
  const phaseLabel = {
    checking: copy.phaseChecking,
    installing: copy.phaseInstalling,
    done: copy.phaseDone,
    failed: copy.phaseFailed,
    cancelled: copy.phaseCancelled,
  }[installPhase];
  const showInstallLog =
    Boolean(installPhase) &&
    (installLog ? installLog.agent === activeAgent : busy === 'install:' + activeAgent);

  const cardStyle = `rounded-[20px] p-4 bg-[#F0F4F9] dark:bg-white/[0.05]`;
  const badge = (label, tone) => (
    <span className={`rounded-md px-1.5 py-0.5 text-[10px] font-semibold ${tone}`}>{label}</span>
  );
  const activeBadge = badge(copy.current, 'bg-[#007AFF]/15 text-[#007AFF] dark:text-[#64B5F6]');
  const officialBadge = badge(copy.official, 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400');

  const providers = (view && view.providers) || [];
  const isCurrent = id => (view && view.currentProviderId) === id;
  // 改动 1：进程 env 优先于配置文件——env 冲突存在时「当前/官方」徽标降格，
  // 避免「显示已切换、实际请求没走该 Provider」的误导。
  const envOverrides = (view && view.envConflicts && view.envConflicts.length > 0) || false;
  const currentBadge = envOverrides
    ? badge(copy.currentOverriddenByEnv, 'bg-amber-500/15 text-amber-600 dark:text-amber-400')
    : activeBadge;
  const officialBadgeEffective = envOverrides
    ? badge(copy.currentOverriddenByEnv, 'bg-amber-500/15 text-amber-600 dark:text-amber-400')
    : officialBadge;
  // 未启用的 Provider：灰色「未启用」标注，与当前/官方状态一眼可辨
  const inactiveBadge = badge(copy.notEnabled, 'bg-black/[0.06] text-gray-500 dark:bg-white/[0.08] dark:text-gray-400');

  return (
    <div data-testid="acp-providers-section" className="space-y-5">
      <p className="text-[13px] opacity-70 leading-relaxed">{copy.sectionDesc}</p>

      {/* Agent 标签页 */}
      <div className="flex gap-1.5">
        {AGENTS.map(agent => (
          <button
            key={agent.key}
            data-testid={`acp-agent-tab-${agent.key}`}
            onClick={() => setActiveAgent(agent.key)}
            className={`h-9 px-4 rounded-full text-[12px] font-semibold transition-colors ${
              activeAgent === agent.key
                ? 'bg-[#007AFF] text-white'
                : 'bg-[#F0F4F9] text-[#5F6368] dark:bg-white/[0.08] dark:text-[#C7C7CC]'
            }`}
          >
            {copy[`agent${agent.key[0].toUpperCase()}${agent.key.slice(1)}`]}
          </button>
        ))}
      </div>

      {/* 顶部提示区：红错 + 绿色通知统一吸顶（滚动到任何操作位置都可见）；
          同容器避免两者同时出现时各自吸顶互相重叠 */}
      {(error || notice) && (
        <div className="sticky top-0 z-10 space-y-2">
          {error && (
            <div
              data-testid="acp-providers-error"
              className={`rounded-xl px-3 py-2.5 text-[12px] text-red-500 shadow-lg backdrop-blur-md bg-[#FEF2F2]/95 dark:bg-[#2A1A1A]/95`}
            >
              {error}
              <button onClick={refresh} className="ml-2 underline">{copy.retry}</button>
            </div>
          )}
          {notice && (
            <div className={`rounded-xl px-3 py-2.5 text-[12px] shadow-lg backdrop-blur-md bg-[#F0FDF4]/95 text-emerald-700 dark:bg-[#0F2A1A]/95 dark:text-emerald-300`}>
              {notice}
            </div>
          )}
        </div>
      )}

      {/* env 冲突警告 */}
      {view && view.configUnreadable && (
        <div data-testid="acp-providers-unreadable-warning" className={`rounded-xl px-3 py-2.5 bg-red-500/[0.08] dark:bg-red-500/[0.1]`}>
          <div className="flex items-center gap-1.5 text-[12px] font-semibold text-red-600 dark:text-red-300">
            <AlertTriangle size={14} />
            {copy.configUnreadable}
          </div>
          <p className="mt-1 text-[11px] leading-relaxed opacity-80">{copy.configUnreadableDesc}</p>
        </div>
      )}

      {view && view.envConflicts && view.envConflicts.length > 0 && (
        <div data-testid="acp-providers-env-warning" className={`rounded-xl px-3 py-2.5 bg-amber-500/[0.12] dark:bg-amber-500/[0.1]`}>
          <div className="flex items-center gap-1.5 text-[12px] font-semibold text-amber-600 dark:text-amber-300">
            <AlertTriangle size={14} />
            {copy.envConflictTitle}
          </div>
          <p className="mt-1 text-[11px] leading-relaxed opacity-80">{copy.envConflictDesc}</p>
          {/* env 来源的生效值：非密值明文展示，凭据类只显示「已设置」（改动 5） */}
          {view.envEffectiveEntries && view.envEffectiveEntries.length > 0 ? (
            <div className="mt-1.5 space-y-1">
              {view.envEffectiveEntries.map(entry => (
                <div key={entry.key} className="flex items-baseline gap-2 min-w-0">
                  <span className="shrink-0 text-[11px] font-medium opacity-60">{entry.key}</span>
                  <span className="min-w-0 flex-1 truncate font-mono text-[11px] opacity-90">
                    {entry.secret ? copy.secretSet : entry.value}
                  </span>
                </div>
              ))}
            </div>
          ) : (
            <div className="mt-1.5 font-mono text-[11px] opacity-80">{view.envConflicts.join(', ')}</div>
          )}
        </div>
      )}

      {/* 外部中转配置（配置文件归因不到当前 Provider）：全局提示而非每卡黄标，
          避免用户误以为有人动过自己的配置 */}
      {view && view.externalActive && (
        <div className={`rounded-xl px-3 py-2.5 bg-amber-500/[0.12] dark:bg-amber-500/[0.1]`}>
          <div className="flex items-center gap-1.5 text-[12px] font-semibold text-amber-600 dark:text-amber-300">
            <AlertTriangle size={14} />
            {copy.external}
          </div>
          <p className="mt-1 text-[11px] leading-relaxed opacity-80">{copy.externalDesc}</p>
        </div>
      )}

      {/* 改动 2：生效中配置只读区（F4 可见化）——值来自实际 CLI 配置文件，
          不含任何凭据；官方登录态或配置不可解析时为空不渲染。 */}
      {view && view.effectiveEntries && view.effectiveEntries.length > 0 && (
        <div data-testid="acp-providers-effective" className={`rounded-[20px] p-4 bg-[#F0F4F9] dark:bg-white/[0.05]`}>
          <div className="text-[13px] font-semibold">{copy.effectiveTitle}</div>
          <div className="mt-2 space-y-1">
            {view.effectiveEntries.map(entry => (
              <div key={entry.key} className="flex items-baseline gap-2 min-w-0">
                <span className="shrink-0 text-[11px] font-medium opacity-60">{entry.key}</span>
                <span className="min-w-0 flex-1 truncate font-mono text-[11px] opacity-90">{entry.value}</span>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* 工具栏 */}
      <div className="flex items-center gap-2">
        <button
          data-testid="acp-provider-add"
          onClick={() => { setEditing(null); setFormOpen(true); }}
          className="h-9 px-4 rounded-full bg-[#007AFF] text-white text-[12px] font-semibold inline-flex items-center gap-1.5"
        >
          <Plus size={14} />
          {copy.addProvider}
        </button>
        <button
          data-testid="acp-provider-export"
          onClick={doExport}
          disabled={providers.length === 0}
          className="h-9 px-3 rounded-full text-[12px] font-medium border border-black/[0.08] dark:border-white/[0.12] inline-flex items-center gap-1.5 disabled:opacity-40"
        >
          <Download size={13} />
          {copy.export}
        </button>
        <button
          data-testid="acp-provider-import"
          onClick={() => setImportOpen(true)}
          className="h-9 px-3 rounded-full text-[12px] font-medium border border-black/[0.08] dark:border-white/[0.12] inline-flex items-center gap-1.5"
        >
          <Upload size={13} />
          {copy.import}
        </button>
        <button
          data-testid="acp-provider-refresh"
          onClick={refresh}
          className="h-9 px-3 rounded-full text-[12px] font-medium border border-black/[0.08] dark:border-white/[0.12] inline-flex items-center gap-1.5"
        >
          <RefreshCw size={13} className={loading ? 'animate-spin' : ''} />
          {copy.recheck}
        </button>
      </div>

      {/* Provider 列表 */}
      {loading && !providers.length ? (
        <div className="text-[13px] opacity-60">{copy.loading}</div>
      ) : providers.length === 0 ? (
        <div className={`${cardStyle} text-center py-8`}>
          <div className="text-[14px] font-medium opacity-80">{copy.empty}</div>
          <div className="mt-1 text-[12px] opacity-50">{copy.emptyHint}</div>
        </div>
      ) : (
        <div className="space-y-2.5">
          {providers.map(provider => (
            <div key={provider.id} data-testid={`acp-provider-card-${provider.id}`} className={cardStyle}>
              <div className="flex items-start gap-3">
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2 flex-wrap">
                    <span className="text-[14px] font-semibold">{provider.name}</span>
                    {isCurrent(provider.id) ? currentBadge : inactiveBadge}
                  </div>
                  <div className="mt-1 font-mono text-[11px] opacity-60 truncate">{provider.baseUrl}</div>
                  <div className="mt-2 flex items-center gap-2 flex-wrap">
                    {provider.model && (
                      <span className="rounded-md px-1.5 py-0.5 text-[10px] font-medium bg-black/[0.05] dark:bg-white/[0.08]">{provider.model}</span>
                    )}
                    <WireBadge wireApi={provider.wireApi} agent={activeAgent} copy={copy} />
                    <KeyDot hasCredential={provider.hasCredential} copy={copy} />
                  </div>
                </div>
                <div className="flex shrink-0 items-center gap-1.5">
                  {!isCurrent(provider.id) && (
                    <button
                      data-testid={`acp-provider-switch-${provider.id}`}
                      onClick={() => switchTo(provider.id, false)}
                      disabled={busyOnAgent || !provider.hasCredential}
                      title={provider.hasCredential ? undefined : copy.apiKeyHint}
                      className="h-8 px-3 rounded-full bg-[#007AFF] text-white text-[11px] font-semibold disabled:opacity-40 inline-flex items-center gap-1"
                    >
                      {busy === provider.id && <RefreshCw size={11} className="animate-spin" />}
                      {copy.switch}
                    </button>
                  )}
                  <button
                    data-testid={`acp-provider-edit-${provider.id}`}
                    onClick={() => { setEditing(provider); setFormOpen(true); }}
                    className="h-8 w-8 rounded-full flex items-center justify-center border border-black/[0.08] dark:border-white/[0.12]"
                    aria-label={copy.edit}
                  >
                    <Edit2 size={13} />
                  </button>
                  <button
                    data-testid={`acp-provider-delete-${provider.id}`}
                    onClick={() => setDeleteConfirm(provider)}
                    className="h-8 w-8 rounded-full flex items-center justify-center border border-black/[0.08] dark:border-white/[0.12] hover:text-red-500"
                    aria-label={copy.delete}
                  >
                    <Trash2 size={13} />
                  </button>
                </div>
              </div>
            </div>
          ))}
        </div>
      )}

      {/* 官方登录态：官方生效时仅显示徽标；Provider 生效时提供「切回官方」 */}
      {view && (view.officialActive ? providers.length > 0 : true) && (
        <div className="flex items-center gap-2 text-[12px]">
          {officialBadgeEffective}
          {!view.officialActive && (
            <button
              data-testid="acp-provider-switch-official"
              onClick={() => switchTo(null, true)}
              disabled={busyOnAgent}
              className="h-8 px-3 rounded-full border border-black/[0.08] dark:border-white/[0.12] text-[11px] font-semibold disabled:opacity-40 inline-flex items-center gap-1"
            >
              {busy === 'official' && <RefreshCw size={11} className="animate-spin" />}
              {copy.restoreOfficial}
            </button>
          )}
        </div>
      )}

      {/* CLI 状态区 */}
      <div className={`${cardStyle} space-y-2`}>
        <div className="flex items-center gap-1.5 text-[13px] font-semibold">
          <Terminal size={14} />
          {copy.cliSection}
        </div>
        {!status ? (
          <div className="text-[12px] opacity-60">{copy.loading}</div>
        ) : (
          <>
            <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-[12px] opacity-80">
              <span>{copy.version}: {status.version || copy.notInstalled}</span>
              <span className={status.authenticated ? 'text-emerald-600 dark:text-emerald-400' : 'text-amber-600 dark:text-amber-400'}>
                {status.authenticated ? copy.authenticated : copy.notAuthenticated}
              </span>
            </div>
            <div className="flex items-center gap-2">
              {runInstallAction && (
                <button
                  data-testid="acp-cli-install-update"
                  onClick={() => installOrUpdate(status.install_action)}
                  disabled={installing || busyOnAgent}
                  className="h-8 px-3.5 rounded-full bg-[#007AFF] text-white text-[11px] font-semibold disabled:opacity-50"
                >
                  {installing || busy === 'install:' + activeAgent ? copy.installing : runInstallAction === 'update' ? copy.update : copy.install}
                </button>
              )}
              {installing && (
                <button
                  data-testid="acp-cli-install-cancel"
                  onClick={cancelInstall}
                  disabled={busy === 'cancel-install:' + activeAgent}
                  className="h-8 px-3.5 rounded-full border border-red-500/30 text-red-500 text-[11px] font-semibold disabled:opacity-50"
                >
                  {copy.cancelInstall}
                </button>
              )}
              {status.installed && !status.authenticated && !loginWaiting && (
                <button
                  data-testid="acp-cli-login"
                  onClick={startLogin}
                  disabled={busyOnAgent}
                  className="h-8 px-3.5 rounded-full bg-[#007AFF] text-white text-[11px] font-semibold disabled:opacity-50"
                >
                  {copy.login}
                </button>
              )}
              {/* 登出的是官方账号：中转生效时禁用并提示，避免误当「登出中转」
                  把可切回官方的凭据登没了（kimi 走 provider remove，同规则） */}
              {status.installed && status.authenticated && (
                <button
                  data-testid="acp-cli-logout"
                  onClick={doLogout}
                  disabled={busyOnAgent || Boolean(view && !view.officialActive)}
                  title={view && !view.officialActive ? copy.logoutRelayDisabled : undefined}
                  className="h-8 px-3.5 rounded-full border border-black/[0.08] dark:border-white/[0.12] text-[11px] font-semibold disabled:opacity-50"
                >
                  {busy === 'logout:' + activeAgent ? copy.saving : copy.logout}
                </button>
              )}
              {status.installed && (
                <button
                  data-testid="acp-cli-uninstall"
                  onClick={() => setUninstallConfirm({ cleanup: false })}
                  disabled={busyOnAgent}
                  className="h-8 px-3.5 rounded-full border border-red-500/30 text-red-500 text-[11px] font-semibold disabled:opacity-50"
                >
                  {busy === 'uninstall:' + activeAgent ? copy.uninstallBusy : copy.uninstall}
                </button>
              )}
              <button
                onClick={recheck}
                disabled={recheckingOnAgent || busyOnAgent}
                className="h-8 px-3 rounded-full text-[11px] font-medium border border-black/[0.08] dark:border-white/[0.12] inline-flex items-center gap-1.5 disabled:opacity-50"
              >
                {recheckingOnAgent && <RefreshCw size={11} className="animate-spin" />}
                {recheckingOnAgent ? copy.rechecking : copy.recheck}
              </button>
            </div>
            {/* 安装进度：阶段标签 + 实际执行的命令行 + 输出最新一行（等宽截断） */}
            {showInstallLog && (
              <div data-testid="acp-cli-install-progress" className={`rounded-xl px-3 py-2.5 space-y-1.5 bg-black/[0.03] dark:bg-white/[0.05]`}>
                <div className="flex items-center gap-2 text-[12px] font-semibold">
                  {installPhase === 'checking' || installPhase === 'installing' ? (
                    <RefreshCw size={12} className="animate-spin" />
                  ) : installPhase === 'done' ? (
                    <span className="text-emerald-500">✓</span>
                  ) : installPhase === 'failed' ? (
                    <span className="text-red-500">✕</span>
                  ) : null}
                  {phaseLabel}
                </div>
                {installLog && installLog.command && (
                  <div className="flex items-center gap-1.5 font-mono text-[11px] opacity-70 min-w-0">
                    <span className="shrink-0">{copy.installCmd}:</span>
                    <span className="truncate" title={installLog.command}>{installLog.command}</span>
                  </div>
                )}
                {installLog && installLog.line && (
                  <div className="flex items-center gap-1.5 font-mono text-[11px] opacity-70 min-w-0">
                    <span className="shrink-0">{copy.installLatest}:</span>
                    <span className="truncate" title={installLog.line}>{installLog.line}</span>
                  </div>
                )}
              </div>
            )}
            {/* 登录引导：URL + （claude 的）授权码输入，轮询直到完成/失败 */}
            {loginWaiting && status && status.login_in_progress && (
              <div className={`rounded-xl px-3 py-2.5 space-y-2 bg-black/[0.03] dark:bg-white/[0.05]`}>
                <div className="flex items-center gap-2 text-[12px]">
                  <RefreshCw size={12} className="animate-spin" />
                  <span>{copy.loginWaiting}</span>
                  {status.login_url && (
                    <button
                      data-testid="acp-cli-login-open-url"
                      onClick={() => invokeTauri('open_acp_agent_login_url', { agentId: activeAgent }).catch(e => setError(String(e)))}
                      className="h-7 px-3 rounded-full bg-[#007AFF] text-white text-[11px] font-semibold"
                    >
                      {copy.openLoginUrl}
                    </button>
                  )}
                </div>
                {status.login_input_required && (
                  <div className="flex items-center gap-2">
                    <input
                      data-testid="acp-cli-login-code"
                      value={loginCode}
                      onChange={event => setLoginCode(event.target.value)}
                      placeholder={copy.loginCodePlaceholder}
                      className="h-8 flex-1 rounded-lg px-2.5 font-mono text-[12px] outline-none bg-black/[0.05] dark:bg-white/[0.07]"
                    />
                    <button
                      data-testid="acp-cli-login-submit"
                      onClick={submitLoginCode}
                      disabled={!loginCode.trim()}
                      className="h-8 px-3.5 rounded-full bg-[#007AFF] text-white text-[11px] font-semibold disabled:opacity-40"
                    >
                      {copy.submitCode}
                    </button>
                  </div>
                )}
              </div>
            )}
          </>
        )}
      </div>

      {/* 新增/编辑弹窗 */}
      {formOpen && (
        <ProviderFormModal
          agent={activeAgent}
          copy={copy}
          initial={editing}
          onClose={() => setFormOpen(false)}
          onSaved={() => refresh()}
        />
      )}

      {/* 删除确认 */}
      {deleteConfirm && (
        <div className="fixed inset-0 z-[110] flex items-center justify-center bg-black/45 backdrop-blur-[14px] animate-in fade-in duration-200" onClick={() => setDeleteConfirm(null)}>
          <div onClick={event => event.stopPropagation()} className={`w-[min(400px,calc(100vw-24px))] rounded-[24px] p-6 bg-white text-[#1F1F1F] dark:bg-[#1E1F20] dark:text-[#E8EAED]`}>
            <h3 className="text-[16px] font-semibold">{copy.deleteTitle}</h3>
            <p className="mt-2 text-[13px] leading-relaxed opacity-75">{copy.deleteDesc(deleteConfirm.name)}</p>
            <div className="mt-6 flex justify-end gap-2">
              <button onClick={() => setDeleteConfirm(null)} className="h-9 px-4 rounded-full text-[13px] font-semibold border border-black/[0.08] dark:border-white/[0.12]">{copy.cancel}</button>
              <button data-testid="acp-provider-delete-confirm" onClick={remove} disabled={busyOnAgent} className="h-9 px-4 rounded-full bg-red-500 text-white text-[13px] font-semibold disabled:opacity-50">{copy.deleteConfirm}</button>
            </div>
          </div>
        </div>
      )}

      {/* 卸载确认 */}
      {uninstallConfirm && (
        <div className="fixed inset-0 z-[110] flex items-center justify-center bg-black/45 backdrop-blur-[14px] animate-in fade-in duration-200" onClick={() => setUninstallConfirm(null)}>
          <div onClick={event => event.stopPropagation()} className={`w-[min(420px,calc(100vw-24px))] rounded-[24px] p-6 bg-white text-[#1F1F1F] dark:bg-[#1E1F20] dark:text-[#E8EAED]`}>
            <h3 className="text-[16px] font-semibold">{copy.uninstallTitle.replace('{agent}', copy[`agent${activeAgent[0].toUpperCase()}${activeAgent.slice(1)}`])}</h3>
            <p className="mt-2 text-[13px] leading-relaxed opacity-75">{copy.uninstallDesc}</p>
            <label className="mt-4 flex items-start gap-2 text-[12px] opacity-80 cursor-pointer">
              <input
                data-testid="acp-uninstall-cleanup"
                type="checkbox"
                checked={uninstallConfirm.cleanup}
                onChange={event => setUninstallConfirm(current => ({ ...current, cleanup: event.target.checked }))}
                className="mt-0.5"
              />
              <span>
                {copy.uninstallCleanup}
                <span className="block text-[11px] opacity-60">{copy.uninstallCleanupHint}</span>
              </span>
            </label>
            <div className="mt-6 flex justify-end gap-2">
              <button onClick={() => setUninstallConfirm(null)} className="h-9 px-4 rounded-full text-[13px] font-semibold border border-black/[0.08] dark:border-white/[0.12]">{copy.cancel}</button>
              <button data-testid="acp-uninstall-confirm" onClick={uninstall} disabled={busyOnAgent} className="h-9 px-4 rounded-full bg-red-500 text-white text-[13px] font-semibold disabled:opacity-50">
                {busy === 'uninstall:' + activeAgent ? copy.uninstallBusy : copy.uninstall}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* 导出（含明文 key 警告） */}
      {exportOpen && (
        <div className="fixed inset-0 z-[110] flex items-center justify-center bg-black/45 backdrop-blur-[14px] animate-in fade-in duration-200" onClick={() => setExportOpen(false)}>
          <div onClick={event => event.stopPropagation()} className={`w-[min(560px,calc(100vw-24px))] rounded-[24px] p-6 bg-white text-[#1F1F1F] dark:bg-[#1E1F20] dark:text-[#E8EAED]`}>
            <div className="flex items-start gap-2">
              <AlertTriangle size={17} className="mt-0.5 shrink-0 text-amber-500" />
              <div className="min-w-0 flex-1">
                <h3 className="text-[15px] font-semibold">{copy.exportWarningTitle}</h3>
                <p className="mt-1 text-[12px] leading-relaxed opacity-75">{copy.exportWarningDesc}</p>
              </div>
              <button onClick={() => setExportOpen(false)} className="h-8 w-8 rounded-full flex items-center justify-center hover:bg-black/[0.06] dark:hover:bg-white/[0.08]"><X size={15} /></button>
            </div>
            <textarea
              data-testid="acp-provider-export-json"
              readOnly
              value={exportJson}
              onFocus={event => event.target.select()}
              className="mt-4 h-56 w-full rounded-xl p-3 font-mono text-[11px] outline-none bg-black/[0.05] dark:bg-white/[0.06] resize-none custom-scrollbar"
            />
            <div className="mt-4 flex justify-end gap-2">
              <button onClick={() => setExportOpen(false)} className="h-9 px-4 rounded-full text-[13px] font-semibold border border-black/[0.08] dark:border-white/[0.12]">{copy.cancel}</button>
              {/* 不自动复制到剪贴板：明文 key 意外粘贴到聊天/网页有泄露风险，
                  改为全选让用户主动复制 */}
              <button
                data-testid="acp-provider-export-select"
                onClick={event => { event.currentTarget.closest('div').previousElementSibling.select(); }}
                className="h-9 px-4 rounded-full bg-[#007AFF] text-white text-[13px] font-semibold"
              >
                {copy.selectAll}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* 导入 */}
      {importOpen && (
        <div className="fixed inset-0 z-[110] flex items-center justify-center bg-black/45 backdrop-blur-[14px] animate-in fade-in duration-200" onClick={() => setImportOpen(false)}>
          <div onClick={event => event.stopPropagation()} className={`w-[min(560px,calc(100vw-24px))] rounded-[24px] p-6 bg-white text-[#1F1F1F] dark:bg-[#1E1F20] dark:text-[#E8EAED]`}>
            <h3 className="text-[15px] font-semibold">{copy.import}</h3>
            {/* 导入同样可能含明文 key：来源信任警示（复审低危 6） */}
            <div className="mt-2 rounded-xl border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-[12px] leading-relaxed">
              <span className="font-semibold">{copy.importWarningTitle}</span>
              <span className="opacity-70"> — {copy.importWarningDesc}</span>
            </div>
            <textarea
              data-testid="acp-provider-import-json"
              value={importJson}
              onChange={event => setImportJson(event.target.value)}
              placeholder='[{ "name": "...", "baseUrl": "...", "apiKey": "..." }]'
              className="mt-3 h-52 w-full rounded-xl p-3 font-mono text-[11px] outline-none bg-black/[0.05] dark:bg-white/[0.06] resize-none custom-scrollbar"
            />
            <div className="mt-4 flex justify-end gap-2">
              <button onClick={() => setImportOpen(false)} className="h-9 px-4 rounded-full text-[13px] font-semibold border border-black/[0.08] dark:border-white/[0.12]">{copy.cancel}</button>
              <button
                data-testid="acp-provider-import-confirm"
                onClick={doImport}
                disabled={busyOnAgent || !String(importJson || '').trim()}
                className="h-9 px-4 rounded-full bg-[#007AFF] text-white text-[13px] font-semibold disabled:opacity-50"
              >
                {busy === 'import' ? copy.saving : copy.import}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

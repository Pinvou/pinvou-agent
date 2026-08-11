import React, { useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import {
  AlertTriangle,
  Award,
  ChevronRight,
  Clock,
  Copy,
  Link,
  LineChart,
  Plus,
  Send,
  User,
  X,
} from '../../components/icons.jsx';
import { bridge } from '../../hooks/useBridge.js';
import fallbackUserAvatarUrl from '../../avatars/avatar-07.svg';

const IOS_BLUE = '#007AFF';
const IOS_RED = '#FF3B30';
const IOS_PURPLE = '#AF52DE';

const todayMetrics = [
  {
    id: 'needs_me',
    label: '待我处理',
    value: '0',
    note: '暂无任务',
    icon: AlertTriangle,
    gradient: 'linear-gradient(135deg,#fb923c,#f97316)',
    glow: '0 8px 16px -4px rgba(255,149,0,0.42)',
  },
  {
    id: 'waiting_peer',
    label: '等待对方',
    value: '0',
    note: '暂无任务',
    icon: Send,
    gradient: 'linear-gradient(135deg,#c084fc,#a855f7)',
    glow: '0 8px 16px -4px rgba(175,82,222,0.42)',
  },
];

const overviewMetrics = [
  {
    id: 'high_risk',
    label: '高风险',
    value: '0',
    note: '暂无风险',
    icon: AlertTriangle,
    gradient: 'linear-gradient(135deg,#f87171,#ef4444)',
    glow: '0 8px 16px -4px rgba(255,59,48,0.42)',
    danger: true,
  },
  {
    id: 'timeout_soon',
    label: '即将超时',
    value: '0',
    note: '暂无超时',
    icon: Clock,
    gradient: 'linear-gradient(135deg,#fbbf24,#f59e0b)',
    glow: '0 8px 16px -4px rgba(255,149,0,0.42)',
    warning: true,
  },
];

const recognitionTrend = [
  { label: '一', value: 0 },
  { label: '二', value: 0 },
  { label: '三', value: 0 },
  { label: '四', value: 0 },
  { label: '五', value: 0 },
  { label: '六', value: 0 },
  { label: '日', value: 0, today: true },
];

const pendingActions = [];

const waitingActions = [];

const buildMockSessionPayload = (item, config) => {
  const title = item.title || config?.title || '协作任务';
  const peer = item.peer || '同事';
  const status = item.status || '处理中';
  const category = config?.title || '工作台';
  const promptByCategory = {
    待我处理: `我需要处理「${title}」，先帮我确认现在卡在哪里，以及我下一步应该怎么做。`,
    等待对方: `我发起的「${title}」还在等 ${peer}，帮我看看是否需要催办，以及怎么催比较合适。`,
  };
  const assistantIntroByCategory = {
    待我处理: `这项任务现在轮到你确认。我会把协作上下文限制在摘要和必要产物范围内，不展示原始文件。当前状态是「${status}」。`,
    等待对方: `这项任务已发给 ${peer}，当前状态是「${status}」。你可以在工作台轻点「催一下」，也可以在这里让我生成更克制的提醒话术。`,
  };
  const followUpByCategory = {
    待我处理: `下一步只需要你确认共享范围：当前问题摘要可以共享，原始文件和客户隐私字段不共享。确认后 Pinvou 会继续推进。`,
    等待对方: `等待原因看起来是对方还没有确认可共享的材料范围。建议发送一句短催办：这个任务今天需要流转，麻烦先确认能共享的摘要范围。`,
  };

  return {
    mockKey: item.id,
    title,
    turns: [
      { role: 'user', text: promptByCategory[category] || `帮我查看「${title}」的协作状态。` },
      { role: 'assistant', text: assistantIntroByCategory[category] || `已定位到「${title}」，当前状态是「${status}」。` },
      { role: 'assistant', text: followUpByCategory[category] || '你可以在这个原对话里继续追问或补充材料。' },
    ],
  };
};

const sheetConfigs = {
  needs_me: {
    title: '待我处理',
    countLabel: count => `${count} 项需要处理`,
    items: pendingActions,
  },
  waiting_peer: {
    title: '等待对方',
    countLabel: count => `${count} 项等待反馈`,
    items: waitingActions,
  },
};

const cx = (...parts) => parts.filter(Boolean).join(' ');

const taskStatusLabel = status => {
  switch (status) {
    case 'todo': return '待处理';
    case 'needs_me': return '需要处理';
    case 'waiting_delivery': return '等待送达';
    case 'delivered': return '已送达';
    case 'accepted': return '已接受';
    case 'rejected': return '已拒绝';
    case 'completed': return '已完成';
    case 'delivery_failed': return '发送失败';
    default: return status || '处理中';
  }
};

const formatTaskTime = value => {
  if (!value) return '刚刚';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return '刚刚';
  return date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
};

const taskContextLabel = task => {
  const context = task && task.taskContext;
  if (!context) return '未带上下文';
  const messages = Number(context.messageCount || 0);
  const artifacts = Number(context.artifactCount || 0);
  const parts = ['完整上下文'];
  if (messages > 0) parts.push(`${messages} 条消息`);
  if (artifacts > 0) parts.push(`${artifacts} 个产物`);
  return parts.join(' · ');
};

const incomingTaskToItem = task => ({
  id: task.taskId,
  title: task.title || '协作任务',
  peer: task.fromDisplayName || task.fromPeerId || '同事',
  updatedAt: formatTaskTime(task.updatedAt || task.createdAt),
  status: taskStatusLabel(task.status),
  contextLabel: taskContextLabel(task),
  realTask: task,
  acceptAction: true,
  rejectAction: true,
});

const outgoingTaskToItem = task => ({
  id: task.taskId,
  title: task.title || '协作任务',
  peer: task.toDisplayName || task.toPeerId || '同事',
  updatedAt: formatTaskTime(task.updatedAt || task.createdAt),
  status: taskStatusLabel(task.status),
  contextLabel: taskContextLabel(task),
  realTask: task,
});

const localTaskToItem = task => ({
  id: task.taskId,
  title: task.title || '本地任务',
  peer: '我',
  updatedAt: formatTaskTime(task.updatedAt || task.createdAt),
  status: taskStatusLabel(task.status),
  contextLabel: '本地任务',
  realTask: task,
  localTask: true,
  completeAction: task.status !== 'completed',
});

const localAvatarOnly = value => {
  if (!value || typeof value !== 'string') return null;
  const trimmed = value.trim();
  if (/^(https?:)?\/\//i.test(trimmed)) return null;
  return trimmed;
};

const getUserAvatarUrl = bs => {
  const profile = bs?.memory?.profile || {};
  const identity = profile.identity || {};
  return (
    localAvatarOnly(identity.avatar_url) ||
    localAvatarOnly(identity.avatarUrl) ||
    localAvatarOnly(profile.avatar_url) ||
    localAvatarOnly(profile.avatarUrl) ||
    fallbackUserAvatarUrl
  );
};

const roleLabel = role => {
  const normalized = String(role || '').toLowerCase();
  if (normalized === 'owner' || normalized === 'admin') return 'owner';
  return 'member';
};

const statusLabel = status => {
  if (status === 'online') return '在线';
  if (status === 'pending') return '待加入';
  return '离线';
};

const buildCollaborationMembers = (configState, peers) => {
  const identity = configState?.identity || {};
  const projectMembers = Array.isArray(configState?.project?.members) ? configState.project.members : [];
  const selfPeerId = identity.peerId || identity.peer_id || '';
  const items = [];
  const seen = new Set();
  const pushMember = member => {
    if (!member) return;
    const key = member.peerId || member.memberId || member.name;
    if (!key || seen.has(key)) return;
    seen.add(key);
    items.push(member);
  };

  const selfProjectMember = projectMembers.find(member => member && member.memberId === 'me');
  if (identity.name || selfProjectMember) {
    pushMember({
      memberId: (selfProjectMember && selfProjectMember.memberId) || 'me',
      peerId: selfPeerId,
      name: identity.name || selfProjectMember?.name || '我',
      displayName: identity.name || selfProjectMember?.name || '我',
      role: roleLabel(selfProjectMember?.role || 'owner'),
      status: 'online',
      isSelf: true,
      source: 'self',
    });
  }

  projectMembers.forEach(member => {
    if (!member || member.memberId === 'me') return;
    pushMember({
      memberId: member.memberId,
      peerId: member.peerId || '',
      name: member.name || '协作成员',
      displayName: member.name || '协作成员',
      role: roleLabel(member.role),
      status: member.status || 'offline',
      source: 'invite',
    });
  });

  (Array.isArray(peers) ? peers : []).forEach(peer => {
    if (!peer || !peer.peerId || peer.peerId === selfPeerId) return;
    pushMember({
      memberId: peer.peerId,
      peerId: peer.peerId,
      name: peer.displayName || peer.peerId,
      displayName: peer.displayName || peer.peerId,
      role: 'member',
      status: peer.status || 'offline',
      lastSeenAt: peer.lastSeenAt || peer.last_seen_at || '',
      source: 'relay',
    });
  });

  return items;
};

const appleCardClass = 'relative overflow-hidden rounded-[32px] border border-slate-200/70 bg-[#FCFCFD] shadow-[0_24px_60px_-24px_rgba(15,23,42,0.32),0_10px_28px_-18px_rgba(15,23,42,0.22),inset_0_1px_1px_0_rgba(255,255,255,0.95)] backdrop-blur-2xl transition-all duration-500 ease-out hover:-translate-y-1 hover:scale-[1.01] hover:shadow-[0_30px_72px_-24px_rgba(15,23,42,0.38),0_14px_34px_-20px_rgba(15,23,42,0.26),inset_0_1px_1px_0_rgba(255,255,255,0.95)] active:scale-[0.97] dark:border-white/10 dark:bg-[#1C1C1E]/60 dark:shadow-[0_10px_40px_-10px_rgba(0,0,0,0.5),inset_0_1px_1px_0_rgba(255,255,255,0.15),inset_0_0_0_1px_rgba(255,255,255,0.05)]';

const SectionHeader = ({ title, action, icon: Icon, color }) => (
  <div className="mb-6 flex items-center justify-between px-2">
    <h2 className="flex items-center gap-2 text-2xl font-bold tracking-normal text-gray-900 dark:text-white">
      {Icon && <Icon size={25} style={{ color }} />}
      {title}
    </h2>
    {action && (
      <button
        type="button"
        className="inline-flex items-center gap-1 text-sm font-semibold text-[#007AFF] transition-opacity hover:opacity-70"
      >
        {action}
      </button>
    )}
  </div>
);

const IconTile = ({ metric, compact = false }) => {
  const Icon = metric.icon;

  return (
    <div
      className={cx(
        'grid shrink-0 place-items-center text-white',
        compact ? 'h-10 w-10 rounded-[14px]' : 'h-12 w-12 rounded-[18px]'
      )}
      style={{ background: metric.gradient, boxShadow: metric.glow }}
    >
      <Icon
        size={compact ? 21 : 24}
        className={cx(
          'transition-transform duration-300 group-hover:scale-110',
          metric.id === 'waiting_peer' && 'group-hover:-translate-y-1 group-hover:translate-x-1',
          metric.danger && 'group-hover:rotate-12'
        )}
      />
    </div>
  );
};

const TodayMetricCard = ({ metric, onOpen }) => (
  <button
    type="button"
    onClick={onOpen}
    aria-label={`${metric.label} ${metric.value}`}
    className={cx(
      appleCardClass,
      'group flex aspect-[4/3] cursor-pointer flex-col justify-between p-6 text-left md:h-52 md:aspect-auto',
      metric.success && 'bg-gradient-to-br from-[#FCFCFD] to-green-50/80 dark:from-[#1C1C1E]/80 dark:to-green-900/10'
    )}
  >
    <IconTile metric={metric} />
    <div className="mt-auto">
      <span className="mb-1 block text-5xl font-black leading-none tracking-normal text-gray-900 dark:text-white">
        {metric.value}
      </span>
      <h3 className="text-base font-bold text-gray-800 dark:text-gray-100">{metric.label}</h3>
      <p className="mt-1 text-xs font-medium text-gray-500 dark:text-gray-400">{metric.note}</p>
    </div>
  </button>
);

const OverviewMetricCard = ({ metric }) => (
  <div
    aria-label={`${metric.label} ${metric.value}`}
    className={cx(
      appleCardClass,
      'group flex min-h-[150px] flex-col justify-between p-5 text-left',
      metric.danger && 'bg-gradient-to-b from-[#FCFCFD] to-red-50/80 dark:from-[#1C1C1E]/80 dark:to-red-900/10',
      metric.warning && 'bg-gradient-to-b from-[#FCFCFD] to-orange-50/80 dark:from-[#1C1C1E]/80 dark:to-orange-900/10'
    )}
  >
    {(metric.danger || metric.warning) && (
      <div
        className={cx(
          'absolute right-4 top-4 h-2 w-2 rounded-full animate-pulse',
          metric.danger ? 'bg-red-500 shadow-[0_0_8px_rgba(255,59,48,0.8)]' : 'bg-orange-500 shadow-[0_0_8px_rgba(255,149,0,0.8)]'
        )}
      />
    )}
    <IconTile metric={metric} compact />
    <div className="mt-3">
      <span
        className="mb-1 block text-[34px] font-black leading-none tracking-normal text-gray-900 dark:text-white"
        style={{ color: metric.danger ? IOS_RED : undefined }}
      >
        {metric.value}
      </span>
      <h3 className="text-[14px] font-bold leading-5 text-gray-800 dark:text-gray-100">{metric.label}</h3>
      <p
        className="mt-0.5 text-[11px] font-medium leading-4 text-gray-500 dark:text-gray-400"
        style={{ color: metric.danger ? IOS_RED : undefined }}
      >
        {metric.note}
      </p>
    </div>
  </div>
);

const PerformanceCard = ({ completedCount = 0, totalCount = 0, recognitionCount = 0 }) => {
  const value = totalCount > 0 ? Math.round((completedCount / totalCount) * 100) : 0;
  const circumference = 339.29;
  const strokeDashoffset = circumference - circumference * (value / 100);
  const statusLabel = totalCount > 0 && value >= 80 ? '表现出色' : '暂无数据';
  const statusActive = totalCount > 0 && value >= 80;

  return (
    <div
      aria-label={`最近表现 ${value}%`}
      className={cx(appleCardClass, 'group flex min-h-[360px] flex-col p-6 text-left md:col-span-3 md:p-8')}
    >
      <div className="relative z-10 mb-6 flex items-start justify-between gap-4">
        <div className="min-w-0">
          <div className="mb-2 inline-flex w-fit items-center gap-1.5 rounded-full border border-black/5 bg-white/60 px-3 py-1.5 text-xs font-bold text-gray-600 shadow-sm backdrop-blur-md dark:border-white/10 dark:bg-white/10 dark:text-gray-300">
            <Clock size={15} />
            近 7 天
          </div>
          <h3 className="text-2xl font-black leading-tight tracking-normal text-gray-900 dark:text-white">最近表现</h3>
          <p className="mt-1.5 max-w-[560px] text-sm font-medium leading-relaxed text-gray-500 dark:text-gray-400">
            近 7 天共完成 {completedCount} 项任务，获得 {recognitionCount} 次认可。
          </p>
        </div>
      </div>

      <div className="relative z-10 mt-auto grid grid-cols-1 gap-4 md:grid-cols-2 md:gap-6">
        <div className="relative flex min-h-[230px] flex-col items-center justify-center overflow-hidden rounded-[24px] border border-white/50 bg-white/40 p-6 shadow-sm transition-colors hover:bg-white/60 dark:border-white/10 dark:bg-white/[0.05] dark:hover:bg-white/10">
          <p className="absolute left-6 top-5 text-sm font-bold text-gray-500 dark:text-gray-400">任务完成率</p>

          <div className="relative mt-6 flex items-center justify-center">
            <svg className="h-32 w-32 -rotate-90" viewBox="0 0 128 128" aria-hidden="true">
              <defs>
                <filter id="workbench-progress-glow" x="-20%" y="-20%" width="140%" height="140%">
                  <feGaussianBlur stdDeviation="3" result="blur" />
                  <feComposite in="SourceGraphic" in2="blur" operator="over" />
                </filter>
                <linearGradient id="workbench-progress-gradient" x1="0%" y1="0%" x2="100%" y2="100%">
                  <stop offset="0%" stopColor="#34C759" />
                  <stop offset="100%" stopColor="#30D158" />
                </linearGradient>
              </defs>
              <circle cx="64" cy="64" r="54" stroke="currentColor" className="text-black/5 dark:text-white/[0.06]" strokeWidth="12" fill="transparent" />
              <circle
                cx="64"
                cy="64"
                r="54"
                stroke="url(#workbench-progress-gradient)"
                strokeWidth="12"
                fill="transparent"
                strokeDasharray={circumference}
                strokeDashoffset={strokeDashoffset}
                strokeLinecap="round"
                filter="url(#workbench-progress-glow)"
                style={{ transition: 'stroke-dashoffset 1.5s cubic-bezier(0.34, 1.56, 0.64, 1)' }}
              />
            </svg>
            <div className="absolute flex items-baseline">
              <span className="text-4xl font-black tracking-normal text-gray-900 dark:text-white">{value}</span>
              <span className="text-2xl font-bold text-gray-500 dark:text-gray-400">%</span>
            </div>
          </div>
        </div>

        <div className="relative flex min-h-[230px] flex-col justify-between rounded-[24px] border border-white/50 bg-white/40 p-6 shadow-sm transition-colors hover:bg-white/60 dark:border-white/10 dark:bg-white/[0.05] dark:hover:bg-white/10">
          <div className="flex items-start justify-between gap-4">
            <div>
              <p className="mb-0.5 text-sm font-bold text-gray-500 dark:text-gray-400">收到认可</p>
              <div className="flex items-baseline gap-1">
                <span className="text-3xl font-black tracking-normal text-gray-900 dark:text-white">{recognitionCount}</span>
                <span className="text-sm font-bold text-gray-500 dark:text-gray-400">次</span>
              </div>
            </div>
            <div className="grid h-10 w-10 place-items-center rounded-full bg-blue-100 text-blue-500 shadow-sm dark:bg-blue-500/20">
              <Award size={18} />
            </div>
          </div>

          <div className="mt-4 flex h-24 items-end justify-between gap-2 border-t border-black/5 pt-2 dark:border-white/5">
            {recognitionTrend.map(item => (
              <div key={item.label} className="flex h-full w-full flex-col items-center justify-end gap-1.5">
                <div className="flex h-[80%] w-full items-end overflow-hidden rounded-full bg-black/5 dark:bg-white/[0.06]">
                  <div
                    className="w-full rounded-full bg-gradient-to-t from-blue-400 to-blue-500 shadow-sm transition-opacity hover:opacity-80"
                    style={{ height: `${item.value}%` }}
                  />
                </div>
                <span className={cx('text-[10px] font-bold', item.today ? 'text-gray-900 dark:text-white' : 'text-gray-400')}>
                  {item.label}
                </span>
              </div>
            ))}
          </div>
        </div>
      </div>

      <div
        className={cx(
          'relative z-10 mt-6 inline-flex w-fit items-center gap-2 rounded-lg border px-3 py-1.5',
          statusActive
            ? 'border-green-500/20 bg-green-500/10 dark:bg-green-500/20'
            : 'border-gray-300/60 bg-gray-100/70 dark:border-white/10 dark:bg-white/10'
        )}
      >
        <span className="relative flex h-2.5 w-2.5">
          {statusActive && <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-green-500 opacity-75" />}
          <span className={cx('relative inline-flex h-2.5 w-2.5 rounded-full', statusActive ? 'bg-green-500 shadow-[0_0_8px_rgba(52,199,89,0.8)]' : 'bg-gray-400')} />
        </span>
        <span className={cx('text-xs font-bold tracking-wide', statusActive ? 'text-green-600 dark:text-green-400' : 'text-gray-500 dark:text-gray-400')}>{statusLabel}</span>
      </div>
    </div>
  );
};

const StatusActionSheet = ({ config, actionFeedback, onClose, onOpenChat, onOpenTask, onCreateTaskGuide, onQuickAction, onAcceptTask, onRejectTask, onCompleteTask }) => {
  if (!config) return null;
  const items = config.items || [];

  const handleOpenChat = (item) => {
    onClose();
    if (item.realTask) {
      if (onOpenTask) {
        onOpenTask(item.realTask);
        return;
      }
      if (onOpenChat) {
        onOpenChat({
          mockKey: item.id,
          title: item.title,
          turns: [
            { role: 'user', text: `帮我查看协作任务「${item.title}」。` },
            { role: 'assistant', text: item.realTask.instruction || `当前状态是「${item.status}」。` },
          ],
        });
      }
      return;
    }
    if (onOpenChat) onOpenChat(buildMockSessionPayload(item, config));
  };

  const sheet = (
    <div className="fixed inset-0 z-[70] flex items-end justify-center bg-black/20 p-3 backdrop-blur-[6px] md:items-center" onClick={onClose}>
      <div
        className="w-full max-w-[440px] overflow-hidden rounded-[26px] border border-black/[0.06] bg-[#F2F2F7]/95 shadow-[0_24px_70px_rgba(15,23,42,0.26)] backdrop-blur-2xl dark:border-white/10 dark:bg-[#101012]/95 dark:shadow-[0_24px_70px_rgba(0,0,0,0.62)]"
        onClick={event => event.stopPropagation()}
      >
        <div className="flex items-start justify-between gap-4 px-5 pt-4 pb-3">
          <div className="min-w-0">
            <h3 className="truncate text-[20px] font-bold tracking-normal text-gray-950 dark:text-white">{config.title}</h3>
            <p className="mt-0.5 text-[13px] font-medium text-gray-500 dark:text-gray-400">{config.countLabel(items.length)}</p>
          </div>
          <button
            type="button"
            aria-label="关闭"
            onClick={onClose}
            className="grid h-7 w-7 place-items-center rounded-full bg-black/[0.06] text-gray-400 transition-colors hover:bg-black/[0.10] active:bg-black/[0.14] dark:bg-white/[0.10] dark:text-gray-500 dark:hover:bg-white/[0.14]"
          >
            <X size={15} />
          </button>
        </div>

        <div className="px-3 pb-3">
          {items.length === 0 ? (
            <div className="rounded-[20px] bg-white px-5 py-5 shadow-[0_1px_2px_rgba(15,23,42,0.06)] ring-1 ring-black/[0.04] dark:bg-[#1C1C1E] dark:ring-white/10">
              <div className="flex items-start gap-3">
                <div className="grid h-11 w-11 shrink-0 place-items-center rounded-[16px] bg-blue-50 text-[#007AFF] dark:bg-blue-500/15 dark:text-[#0A84FF]">
                  <Send size={20} />
                </div>
                <div className="min-w-0 flex-1">
                  <div className="text-[16px] font-bold text-gray-950 dark:text-white">还没有任务</div>
                  <p className="mt-1 text-[13px] leading-relaxed text-gray-500 dark:text-gray-400">
                    在聊天输入框输入 @对象 加任务内容，就可以创建任务。
                  </p>
                  <div className="mt-3 space-y-2">
                    <div className="rounded-[14px] bg-gray-100 px-3 py-2 text-[13px] font-semibold text-gray-700 dark:bg-white/10 dark:text-gray-200">
                      @我 今天背完 20 个英语单词
                    </div>
                    <div className="rounded-[14px] bg-gray-100 px-3 py-2 text-[13px] font-semibold text-gray-700 dark:bg-white/10 dark:text-gray-200">
                      @同事 帮我 review 这份方案
                    </div>
                  </div>
                </div>
              </div>
              <button
                type="button"
                onClick={() => {
                  onClose();
                  if (onCreateTaskGuide) onCreateTaskGuide();
                }}
                className="mt-4 flex h-11 w-full items-center justify-center gap-2 rounded-[16px] bg-[#007AFF] text-[15px] font-bold text-white transition-opacity hover:opacity-90 active:opacity-70 dark:bg-[#0A84FF]"
              >
                去聊天创建任务
                <ChevronRight size={16} />
              </button>
            </div>
          ) : (
            <div className="overflow-hidden rounded-[20px] bg-white shadow-[0_1px_2px_rgba(15,23,42,0.06)] ring-1 ring-black/[0.04] dark:bg-[#1C1C1E] dark:ring-white/10">
            {items.map(item => (
            <button
              key={item.id}
              type="button"
              onClick={() => handleOpenChat(item)}
              className="flex min-h-[82px] w-full items-center justify-between gap-3 px-4 py-3 text-left transition-colors hover:bg-gray-50 active:bg-gray-100 dark:hover:bg-white/[0.06] dark:active:bg-white/10"
            >
              <div className="min-w-0">
                <div className="truncate text-[16px] font-semibold text-gray-950 dark:text-white">{item.title}</div>
                <div className="mt-1 truncate text-[13px] font-medium text-gray-500 dark:text-gray-400">
                  {item.peer} · {item.updatedAt} · {item.status}
                </div>
                {item.contextLabel && (
                  <div className="mt-1 truncate text-[12px] font-semibold text-[#007AFF] dark:text-[#0A84FF]">
                    {item.contextLabel}
                  </div>
                )}
              </div>
              <div className="flex shrink-0 items-center gap-2">
                {item.action && (
                  <span
                    role="button"
                    tabIndex={0}
                    onClick={event => {
                      event.stopPropagation();
                      onQuickAction(item);
                    }}
                    onKeyDown={event => {
                      if (event.key === 'Enter' || event.key === ' ') {
                        event.preventDefault();
                        event.stopPropagation();
                        onQuickAction(item);
                      }
                    }}
                    className={cx(
                      'rounded-full px-3 py-1 text-[12px] font-semibold transition-colors',
                      actionFeedback[item.id]
                        ? 'bg-green-100 text-green-700 dark:bg-green-500/15 dark:text-green-300'
                        : 'bg-blue-50 text-[#007AFF] hover:bg-blue-100 dark:bg-blue-500/15 dark:text-[#0A84FF] dark:hover:bg-blue-500/20'
                    )}
                  >
                    {actionFeedback[item.id] ? item.doneLabel : item.action}
                  </span>
                )}
                {item.acceptAction && (
                  <span
                    role="button"
                    tabIndex={0}
                    onClick={event => {
                      event.stopPropagation();
                      onAcceptTask(item);
                    }}
                    onKeyDown={event => {
                      if (event.key === 'Enter' || event.key === ' ') {
                        event.preventDefault();
                        event.stopPropagation();
                        onAcceptTask(item);
                      }
                    }}
                    className="rounded-full bg-green-50 px-3 py-1 text-[12px] font-semibold text-green-700 transition-colors hover:bg-green-100 dark:bg-green-500/15 dark:text-green-300 dark:hover:bg-green-500/20"
                  >
                    接受
                  </span>
                )}
                {item.completeAction && (
                  <span
                    role="button"
                    tabIndex={0}
                    onClick={event => {
                      event.stopPropagation();
                      onCompleteTask(item);
                    }}
                    onKeyDown={event => {
                      if (event.key === 'Enter' || event.key === ' ') {
                        event.preventDefault();
                        event.stopPropagation();
                        onCompleteTask(item);
                      }
                    }}
                    className="rounded-full bg-blue-50 px-3 py-1 text-[12px] font-semibold text-[#007AFF] transition-colors hover:bg-blue-100 dark:bg-blue-500/15 dark:text-[#0A84FF] dark:hover:bg-blue-500/20"
                  >
                    完成
                  </span>
                )}
                {item.rejectAction && (
                  <span
                    role="button"
                    tabIndex={0}
                    onClick={event => {
                      event.stopPropagation();
                      onRejectTask(item);
                    }}
                    onKeyDown={event => {
                      if (event.key === 'Enter' || event.key === ' ') {
                        event.preventDefault();
                        event.stopPropagation();
                        onRejectTask(item);
                      }
                    }}
                    className="rounded-full bg-red-50 px-3 py-1 text-[12px] font-semibold text-red-600 transition-colors hover:bg-red-100 dark:bg-red-500/15 dark:text-red-300 dark:hover:bg-red-500/20"
                  >
                    拒绝
                  </span>
                )}
                <ChevronRight size={19} className="text-gray-300 dark:text-gray-500" />
              </div>
            </button>
            ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
  return createPortal(sheet, document.body);
};

const StartCollaborationModal = ({ initialName, loading, onClose, onSubmit }) => {
  const [name, setName] = useState(initialName || '');
  const canSubmit = name.trim().length > 0 && !loading;
  const modal = (
    <div className="fixed inset-0 z-[80] flex items-center justify-center bg-black/25 px-4 backdrop-blur-[10px]" onClick={onClose}>
      <form
        className="w-full max-w-[390px] overflow-hidden rounded-[28px] border border-black/[0.06] bg-[#F2F2F7]/95 shadow-[0_28px_80px_rgba(15,23,42,0.30)] backdrop-blur-2xl dark:border-white/10 dark:bg-[#101012]/95"
        onClick={event => event.stopPropagation()}
        onSubmit={event => {
          event.preventDefault();
          if (canSubmit) onSubmit(name.trim());
        }}
      >
        <div className="px-5 pt-5 pb-4">
          <div className="mx-auto grid h-14 w-14 place-items-center rounded-[20px] bg-[#007AFF] text-white shadow-[0_14px_30px_-12px_rgba(0,122,255,0.8)]">
            <User size={26} />
          </div>
          <h2 className="mt-4 text-center text-[22px] font-black tracking-normal text-gray-950 dark:text-white">开始协作</h2>
          <p className="mx-auto mt-1 max-w-[300px] text-center text-[13px] leading-relaxed text-gray-500 dark:text-gray-400">
            设置一个显示名，其他在线成员可以在 @ 列表里看到你。
          </p>
          <label className="mt-5 block">
            <span className="mb-2 block px-1 text-[12px] font-semibold text-gray-500 dark:text-gray-400">显示名</span>
            <input
              autoFocus
              value={name}
              onChange={event => setName(event.target.value)}
              maxLength={40}
              placeholder="例如：张三"
              className="h-12 w-full rounded-[16px] border border-black/[0.06] bg-white px-4 text-[16px] font-semibold text-gray-950 outline-none transition-shadow placeholder:text-gray-400 focus:shadow-[0_0_0_4px_rgba(0,122,255,0.16)] dark:border-white/10 dark:bg-[#1C1C1E] dark:text-white"
            />
          </label>
        </div>
        <div className="grid grid-cols-2 border-t border-black/[0.06] dark:border-white/10">
          <button
            type="button"
            onClick={onClose}
            disabled={loading}
            className="h-12 text-[16px] font-semibold text-gray-500 transition-colors hover:bg-black/[0.03] active:bg-black/[0.06] dark:text-gray-400 dark:hover:bg-white/[0.06]"
          >
            取消
          </button>
          <button
            type="submit"
            disabled={!canSubmit}
            className="h-12 border-l border-black/[0.06] text-[16px] font-bold text-[#007AFF] transition-colors hover:bg-black/[0.03] active:bg-black/[0.06] disabled:text-gray-400 dark:border-white/10 dark:text-[#0A84FF] dark:hover:bg-white/[0.06]"
          >
            {loading ? '连接中...' : '开始协作'}
          </button>
        </div>
      </form>
    </div>
  );
  return createPortal(modal, document.body);
};

const InviteMembersModal = ({ members, loading, invite, error, copied, onClose, onCopy, onCreateInvite }) => {
  const modal = (
    <div className="fixed inset-0 z-[80] flex items-center justify-center bg-black/25 px-4 backdrop-blur-[10px]" onClick={onClose}>
      <div
        className="w-full max-w-[520px] overflow-hidden rounded-[28px] border border-black/[0.06] bg-[#F2F2F7]/95 shadow-[0_28px_80px_rgba(15,23,42,0.30)] backdrop-blur-2xl dark:border-white/10 dark:bg-[#101012]/95"
        onClick={event => event.stopPropagation()}
      >
        <div className="flex items-start justify-between gap-4 px-5 pt-5">
          <div>
            <div className="grid h-12 w-12 place-items-center rounded-[18px] bg-[#007AFF] text-white shadow-[0_14px_30px_-12px_rgba(0,122,255,0.8)]">
              <Link size={22} />
            </div>
            <h2 className="mt-4 text-[22px] font-black tracking-normal text-gray-950 dark:text-white">邀请成员</h2>
            <p className="mt-1 max-w-[360px] text-[13px] leading-relaxed text-gray-500 dark:text-gray-400">
              对方打开链接后可唤起 Pinvou；如果浏览器没有反应，也可以复制页面上的邀请口令，在 Pinvou 工作台的“加入邀请”里粘贴。
            </p>
          </div>
          <button
            type="button"
            aria-label="关闭"
            onClick={onClose}
            className="grid h-9 w-9 shrink-0 place-items-center rounded-full bg-black/[0.05] text-gray-500 transition-colors hover:bg-black/[0.08] dark:bg-white/10 dark:text-gray-300 dark:hover:bg-white/15"
          >
            <X size={18} />
          </button>
        </div>

        <div className="px-5 py-5">
          <div className="rounded-[20px] border border-black/[0.06] bg-white p-3 dark:border-white/10 dark:bg-[#1C1C1E]">
            <div className="mb-2 px-1 text-[12px] font-semibold text-gray-500 dark:text-gray-400">邀请链接</div>
            <div className="flex items-center gap-2">
              <div className="min-w-0 flex-1 truncate rounded-[14px] bg-gray-100 px-3 py-2 text-[13px] font-medium text-gray-700 dark:bg-black/25 dark:text-gray-200">
                {loading ? '生成中...' : (invite?.url || '暂未生成')}
              </div>
              <button
                type="button"
                onClick={invite?.url ? onCopy : onCreateInvite}
                disabled={loading}
                className="grid h-10 w-10 shrink-0 place-items-center rounded-full bg-[#007AFF] text-white transition-colors hover:bg-[#006FE6] disabled:cursor-not-allowed disabled:opacity-50"
                title={invite?.url ? '复制邀请链接' : '生成邀请链接'}
              >
                {invite?.url ? <Copy size={18} /> : <Plus size={18} />}
              </button>
            </div>
            {copied && <div className="mt-2 px-1 text-[12px] font-semibold text-green-600 dark:text-green-400">已复制</div>}
            {error && <div className="mt-2 px-1 text-[12px] font-semibold text-red-500">{error}</div>}
          </div>

          <div className="mt-4 rounded-[20px] border border-black/[0.06] bg-white p-3 dark:border-white/10 dark:bg-[#1C1C1E]">
            <div className="mb-2 px-1 text-[12px] font-semibold text-gray-500 dark:text-gray-400">当前成员</div>
            <div className="max-h-52 overflow-y-auto">
              {members.map(member => (
                <div key={member.memberId || member.peerId || member.name} className="flex items-center justify-between gap-3 rounded-[14px] px-2 py-2">
                  <div className="flex min-w-0 items-center gap-2">
                    <div className="grid h-8 w-8 shrink-0 place-items-center rounded-full bg-gray-100 text-gray-600 dark:bg-white/10 dark:text-gray-200">
                      <User size={16} />
                    </div>
                    <div className="min-w-0">
                      <div className="truncate text-[13px] font-bold text-gray-950 dark:text-white">{member.isSelf ? `${member.displayName}（我）` : member.displayName}</div>
                      <div className="text-[11px] font-medium text-gray-500 dark:text-gray-400">{member.role}</div>
                    </div>
                  </div>
                  <span className={cx('shrink-0 rounded-full px-2 py-1 text-[11px] font-bold', member.status === 'online' ? 'bg-green-500/10 text-green-600 dark:text-green-400' : 'bg-gray-500/10 text-gray-500 dark:text-gray-400')}>
                    {statusLabel(member.status)}
                  </span>
                </div>
              ))}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
  return createPortal(modal, document.body);
};

const JoinInviteModal = ({ loading, error, initialToken, initialDisplayName, onClose, onSubmit }) => {
  const [token, setToken] = useState(initialToken || '');
  const [displayName, setDisplayName] = useState(initialDisplayName || '');
  useEffect(() => {
    if (initialToken) setToken(initialToken);
  }, [initialToken]);
  useEffect(() => {
    if (initialDisplayName) setDisplayName(initialDisplayName);
  }, [initialDisplayName]);
  const canSubmit = token.trim().length > 0 && displayName.trim().length > 0 && !loading;
  const modal = (
    <div className="fixed inset-0 z-[80] flex items-center justify-center bg-black/25 px-4 backdrop-blur-[10px]" onClick={onClose}>
      <form
        className="w-full max-w-[440px] overflow-hidden rounded-[28px] border border-black/[0.06] bg-[#F2F2F7]/95 shadow-[0_28px_80px_rgba(15,23,42,0.30)] backdrop-blur-2xl dark:border-white/10 dark:bg-[#101012]/95"
        onClick={event => event.stopPropagation()}
        onSubmit={event => {
          event.preventDefault();
          if (canSubmit) onSubmit({ token: token.trim(), displayName: displayName.trim() });
        }}
      >
        <div className="px-5 pt-5 pb-4">
          <div className="mx-auto grid h-14 w-14 place-items-center rounded-[20px] bg-[#007AFF] text-white shadow-[0_14px_30px_-12px_rgba(0,122,255,0.8)]">
            <Link size={25} />
          </div>
          <h2 className="mt-4 text-center text-[22px] font-black tracking-normal text-gray-950 dark:text-white">加入协作</h2>
          <label className="mt-5 block">
            <span className="mb-2 block px-1 text-[12px] font-semibold text-gray-500 dark:text-gray-400">邀请链接或 token</span>
            <input
              autoFocus
              value={token}
              onChange={event => setToken(event.target.value)}
              placeholder="粘贴对方发来的邀请链接"
              className="h-12 w-full rounded-[16px] border border-black/[0.06] bg-white px-4 text-[14px] font-semibold text-gray-950 outline-none transition-shadow placeholder:text-gray-400 focus:shadow-[0_0_0_4px_rgba(0,122,255,0.16)] dark:border-white/10 dark:bg-[#1C1C1E] dark:text-white"
            />
          </label>
          <label className="mt-4 block">
            <span className="mb-2 block px-1 text-[12px] font-semibold text-gray-500 dark:text-gray-400">昵称</span>
            <input
              value={displayName}
              onChange={event => setDisplayName(event.target.value)}
              maxLength={40}
              placeholder="例如：张三"
              className="h-12 w-full rounded-[16px] border border-black/[0.06] bg-white px-4 text-[16px] font-semibold text-gray-950 outline-none transition-shadow placeholder:text-gray-400 focus:shadow-[0_0_0_4px_rgba(0,122,255,0.16)] dark:border-white/10 dark:bg-[#1C1C1E] dark:text-white"
            />
          </label>
          {error && <div className="mt-3 rounded-[14px] bg-red-500/10 px-3 py-2 text-[12px] font-semibold text-red-500">{error}</div>}
        </div>
        <div className="grid grid-cols-2 border-t border-black/[0.06] dark:border-white/10">
          <button type="button" onClick={onClose} disabled={loading} className="h-12 text-[16px] font-semibold text-gray-500 transition-colors hover:bg-black/[0.03] active:bg-black/[0.06] dark:text-gray-400 dark:hover:bg-white/[0.06]">取消</button>
          <button type="submit" disabled={!canSubmit} className="h-12 border-l border-black/[0.06] text-[16px] font-bold text-[#007AFF] transition-colors hover:bg-black/[0.03] active:bg-black/[0.06] disabled:text-gray-400 dark:border-white/10 dark:text-[#0A84FF] dark:hover:bg-white/[0.06]">
            {loading ? '加入中...' : '加入协作'}
          </button>
        </div>
      </form>
    </div>
  );
  return createPortal(modal, document.body);
};

const TaskRecipientModal = ({ members, onClose, onSelect }) => {
  const modal = (
    <div className="fixed inset-0 z-[80] flex items-center justify-center bg-black/25 px-4 backdrop-blur-[10px]" onClick={onClose}>
      <div className="w-full max-w-[420px] overflow-hidden rounded-[28px] border border-black/[0.06] bg-[#F2F2F7]/95 shadow-[0_28px_80px_rgba(15,23,42,0.30)] backdrop-blur-2xl dark:border-white/10 dark:bg-[#101012]/95" onClick={event => event.stopPropagation()}>
        <div className="flex items-center justify-between px-5 py-4">
          <div>
            <h2 className="text-[20px] font-black tracking-normal text-gray-950 dark:text-white">新建任务</h2>
            <p className="mt-0.5 text-[12px] font-medium text-gray-500 dark:text-gray-400">选择接收人后进入聊天补充任务内容。</p>
          </div>
          <button type="button" aria-label="关闭" onClick={onClose} className="grid h-9 w-9 place-items-center rounded-full bg-black/[0.05] text-gray-500 dark:bg-white/10 dark:text-gray-300">
            <X size={18} />
          </button>
        </div>
        <div className="max-h-[360px] overflow-y-auto px-3 pb-4">
          {members.map(member => (
            <button
              key={member.peerId || member.memberId || member.displayName}
              type="button"
              onClick={() => onSelect(member)}
              className="flex w-full items-center justify-between gap-3 rounded-[18px] px-3 py-3 text-left transition-colors hover:bg-white active:bg-gray-100 dark:hover:bg-white/10 dark:active:bg-white/15"
            >
              <div className="flex min-w-0 items-center gap-3">
                <div className="grid h-10 w-10 shrink-0 place-items-center rounded-full bg-white text-gray-600 shadow-sm dark:bg-white/10 dark:text-gray-200">
                  <User size={18} />
                </div>
                <div className="min-w-0">
                  <div className="truncate text-[15px] font-bold text-gray-950 dark:text-white">{member.displayName}</div>
                  <div className="text-[12px] font-medium text-gray-500 dark:text-gray-400">{statusLabel(member.status)}</div>
                </div>
              </div>
              <ChevronRight size={18} className="text-gray-300 dark:text-gray-500" />
            </button>
          ))}
        </div>
      </div>
    </div>
  );
  return createPortal(modal, document.body);
};

export const CollaborationView = ({ theme, bs, pendingInvite, onPendingInviteConsumed, onOpenChat, onOpenTask, onCreateTaskGuide, onStartCollaboration, onOpenAbilityPool }) => {
  const userAvatarUrl = getUserAvatarUrl(bs);
  const collaborationBridge = bridge.collaboration || {};
  const [activeSheetId, setActiveSheetId] = useState(null);
  const [actionFeedback, setActionFeedback] = useState({});
  const [startingCollaboration, setStartingCollaboration] = useState(false);
  const [startModalOpen, setStartModalOpen] = useState(false);
  const [inviteModalOpen, setInviteModalOpen] = useState(false);
  const [inviteLoading, setInviteLoading] = useState(false);
  const [inviteError, setInviteError] = useState('');
  const [inviteLink, setInviteLink] = useState(null);
  const [inviteCopied, setInviteCopied] = useState(false);
  const [joinModalOpen, setJoinModalOpen] = useState(false);
  const [joinLoading, setJoinLoading] = useState(false);
  const [joinError, setJoinError] = useState('');
  const [joinInitialToken, setJoinInitialToken] = useState('');
  const [taskRecipientOpen, setTaskRecipientOpen] = useState(false);
  const [taskHint, setTaskHint] = useState('');
  const collaboration = bs?.collaboration || {};
  const configState = collaboration.configState || {};
  const members = buildCollaborationMembers(configState, collaboration.peers);
  const taskReceivers = members.filter(member => !member.isSelf && member.peerId && member.status !== 'pending');
  const hasCollaborators = taskReceivers.length > 0;
  const incomingTasks = Array.isArray(collaboration.incomingTasks) ? collaboration.incomingTasks : [];
  const outgoingTasks = Array.isArray(collaboration.outgoingTasks) ? collaboration.outgoingTasks : [];
  const localTasks = Array.isArray(collaboration.localTasks) ? collaboration.localTasks : [];
  const collaborationReady = !!configState.identityRegistered && !!configState.projectConfigured;
  const visibleLocalTasks = localTasks.filter(task => task.status !== 'completed').map(localTaskToItem);
  const needsMeItems = incomingTasks.filter(task => task.status === 'needs_me').map(incomingTaskToItem).concat(visibleLocalTasks);
  const waitingPeerItems = outgoingTasks
    .filter(task => task.status === 'waiting_delivery' || task.status === 'delivered' || task.status === 'delivery_failed')
    .map(outgoingTaskToItem);
  const allTasks = incomingTasks.concat(outgoingTasks, localTasks);
  const completedCount = allTasks.filter(task => task.status === 'completed').length;
  const deliveryFailedCount = outgoingTasks.filter(task => task.status === 'delivery_failed').length;
  const dynamicMetrics = todayMetrics.map(metric => {
    if (metric.id === 'needs_me') {
      const note = visibleLocalTasks.length > 0 && incomingTasks.length === 0
        ? '本地任务'
        : (collaboration.connected ? '需要确认' : (collaboration.reason || '协作未连接'));
      return { ...metric, value: String(needsMeItems.length), note };
    }
    if (metric.id === 'waiting_peer') {
      return { ...metric, value: String(waitingPeerItems.length), note: waitingPeerItems.length > 0 ? '待反馈' : '暂无任务' };
    }
    return metric;
  });
  const dynamicOverviewMetrics = overviewMetrics.map(metric => {
    if (metric.id === 'high_risk') {
      return { ...metric, value: String(deliveryFailedCount), note: deliveryFailedCount > 0 ? '发送失败' : '暂无风险' };
    }
    if (metric.id === 'timeout_soon') {
      return { ...metric, value: '0', note: '暂无超时' };
    }
    return metric;
  });
  const dynamicSheetConfigs = {
    ...sheetConfigs,
    needs_me: { ...sheetConfigs.needs_me, items: needsMeItems },
    waiting_peer: { ...sheetConfigs.waiting_peer, items: waitingPeerItems },
  };
  const activeSheetConfig = dynamicSheetConfigs[activeSheetId] || null;

  useEffect(() => {
    const url = pendingInvite?.url || '';
    if (!url) return;
    setJoinInitialToken(url);
    setJoinError('');
    setJoinModalOpen(true);
    if (onPendingInviteConsumed) onPendingInviteConsumed();
  }, [pendingInvite, onPendingInviteConsumed]);

  const handleQuickAction = item => {
    setActionFeedback(previous => ({ ...previous, [item.id]: true }));
  };
  const handleAcceptTask = item => {
    if (!item?.id || !bridge.available || !collaborationBridge.collaborationAcceptTask) return;
    setActionFeedback(previous => ({ ...previous, [item.id]: true }));
    collaborationBridge.collaborationAcceptTask(item.id).catch(error => {
      console.warn('[collaboration] accept task failed', error);
    });
  };
  const handleRejectTask = item => {
    if (!item?.id || !bridge.available || !collaborationBridge.collaborationRejectTask) return;
    setActionFeedback(previous => ({ ...previous, [item.id]: true }));
    collaborationBridge.collaborationRejectTask(item.id).catch(error => {
      console.warn('[collaboration] reject task failed', error);
    });
  };
  const handleCompleteTask = item => {
    if (!item?.id || !bridge.available || !collaborationBridge.collaborationCompleteLocalTask) return;
    setActionFeedback(previous => ({ ...previous, [item.id]: true }));
    collaborationBridge.collaborationCompleteLocalTask(item.id).catch(error => {
      console.warn('[collaboration] complete local task failed', error);
    });
  };
  const handleStartCollaboration = () => {
    if (startingCollaboration) return;
    if (!bridge.available || !collaborationBridge.collaborationStart) {
      if (onOpenAbilityPool) onOpenAbilityPool();
      return;
    }
    if (onStartCollaboration) {
      onStartCollaboration();
      return;
    }
    setStartModalOpen(true);
  };
  const submitStartCollaboration = (name) => {
    const trimmed = String(name || '').trim();
    if (!trimmed) return;
    setStartingCollaboration(true);
    collaborationBridge.collaborationStart({ name: trimmed })
      .then(() => setStartModalOpen(false))
      .catch(error => {
        console.warn('[collaboration] start failed', error);
      })
      .finally(() => setStartingCollaboration(false));
  };
  const handleOpenInvite = () => {
    setInviteModalOpen(true);
    setInviteCopied(false);
    setInviteError('');
    if (inviteLink) return;
    handleCreateInvite();
  };
  const handleCreateInvite = () => {
    if (!bridge.available || !collaborationBridge.collaborationCreateInvite || inviteLoading) return;
    setInviteLoading(true);
    setInviteError('');
    collaborationBridge.collaborationCreateInvite()
      .then(invite => setInviteLink(invite))
      .catch(error => setInviteError(String(error && error.message ? error.message : error).slice(0, 220)))
      .finally(() => setInviteLoading(false));
  };
  const handleCopyInvite = async () => {
    const url = inviteLink?.url || '';
    if (!url) return;
    try {
      await navigator.clipboard.writeText(url);
      setInviteCopied(true);
      window.setTimeout(() => setInviteCopied(false), 1600);
    } catch (error) {
      setInviteError('复制失败，请手动选择链接复制');
    }
  };
  const handleJoinInvite = request => {
    if (!bridge.available || !collaborationBridge.collaborationJoinInvite || joinLoading) return;
    setJoinLoading(true);
    setJoinError('');
    collaborationBridge.collaborationJoinInvite(request)
      .then(() => setJoinModalOpen(false))
      .catch(error => setJoinError(String(error && error.message ? error.message : error).slice(0, 220)))
      .finally(() => setJoinLoading(false));
  };
  const handleNewTask = () => {
    setTaskHint('');
    if (!hasCollaborators) {
      setTaskHint('请先邀请协作成员');
      return;
    }
    setTaskRecipientOpen(true);
  };
  const handleSelectTaskRecipient = member => {
    setTaskRecipientOpen(false);
    if (onCreateTaskGuide) onCreateTaskGuide(member.displayName || member.name || '');
  };

  if (!collaborationReady && visibleLocalTasks.length === 0) {
    return (
      <div className={cx('relative z-10 h-full overflow-y-auto transition-colors duration-700', theme === 'dark' ? 'bg-black' : 'bg-white')}>
        <main className="mx-auto flex min-h-full w-full max-w-3xl flex-col justify-center px-6 py-16">
          <div className="rounded-[32px] border border-slate-200 bg-white p-8 shadow-sm dark:border-white/10 dark:bg-[#1C1C1E]">
            <div className="grid h-16 w-16 place-items-center rounded-3xl bg-blue-600 text-white">
              <User size={30} />
            </div>
            <h1 className="mt-6 text-3xl font-black tracking-normal text-gray-900 dark:text-white">开始协作</h1>
            <p className="mt-2 text-[15px] leading-relaxed text-gray-500 dark:text-gray-400">
              输入名字后会创建本机协作身份并连接 relay，在线成员会出现在聊天输入框的 @ 列表里。
            </p>
            <button
              type="button"
              onClick={handleStartCollaboration}
              disabled={startingCollaboration}
              className="mt-6 rounded-full bg-blue-600 px-5 py-2.5 text-[14px] font-semibold text-white transition-colors hover:bg-blue-700"
            >
              {startingCollaboration ? '连接中...' : '开始协作'}
            </button>
            <button
              type="button"
              onClick={() => {
                setJoinError('');
                setJoinModalOpen(true);
              }}
              className="ml-3 mt-6 rounded-full bg-gray-100 px-5 py-2.5 text-[14px] font-semibold text-gray-700 transition-colors hover:bg-gray-200 dark:bg-white/10 dark:text-gray-200 dark:hover:bg-white/15"
            >
              加入邀请
            </button>
          </div>
        </main>
        {startModalOpen && (
          <StartCollaborationModal
            initialName={configState.identity?.name || ''}
            loading={startingCollaboration}
            onClose={() => {
              if (!startingCollaboration) setStartModalOpen(false);
            }}
            onSubmit={submitStartCollaboration}
          />
        )}
        {joinModalOpen && (
          <JoinInviteModal
            loading={joinLoading}
            error={joinError}
            initialToken={joinInitialToken}
            initialDisplayName={configState.identity?.name || ''}
            onClose={() => {
              if (!joinLoading) setJoinModalOpen(false);
            }}
            onSubmit={handleJoinInvite}
          />
        )}
      </div>
    );
  }

  return (
    <div className={cx('no-scrollbar relative z-10 h-full overflow-y-auto transition-colors duration-700', theme === 'dark' ? 'bg-black' : 'bg-white')}>
      <main className="relative z-10 mx-auto w-full max-w-5xl px-6 pt-16 pb-24">
        <header className="mb-12 flex items-end justify-between gap-4 px-2">
          <div className="min-w-0">
            <h1 className="truncate text-4xl font-black tracking-normal text-gray-900 dark:text-white md:text-5xl">工作台</h1>
            {taskHint && <p className="mt-2 text-[13px] font-semibold text-[#007AFF] dark:text-[#0A84FF]">{taskHint}</p>}
          </div>

          <div className="flex shrink-0 items-center gap-2">
            <button
              type="button"
              onClick={handleOpenInvite}
              className={cx(
                'inline-flex h-10 items-center gap-2 rounded-full px-4 text-[13px] font-bold transition-colors',
                hasCollaborators
                  ? 'bg-gray-100 text-gray-700 hover:bg-gray-200 dark:bg-white/10 dark:text-gray-200 dark:hover:bg-white/15'
                  : 'bg-[#007AFF] text-white hover:bg-[#006FE6]'
              )}
            >
              <Link size={16} />
              邀请成员
            </button>
            <button
              type="button"
              onClick={handleNewTask}
              className={cx(
                'inline-flex h-10 items-center gap-2 rounded-full px-4 text-[13px] font-bold transition-colors',
                hasCollaborators
                  ? 'bg-[#007AFF] text-white hover:bg-[#006FE6]'
                  : 'cursor-not-allowed bg-gray-100 text-gray-400 dark:bg-white/10 dark:text-gray-500'
              )}
            >
              <Plus size={16} />
              新建任务
            </button>
            <button
              type="button"
              aria-label="当前用户头像"
              className="grid h-12 w-12 place-items-center overflow-hidden rounded-full bg-gradient-to-br from-gray-100 to-gray-200 shadow-md ring-2 ring-white transition-all hover:scale-110 active:scale-95 dark:from-gray-800 dark:to-gray-900 dark:ring-gray-800"
            >
              <img src={userAvatarUrl} alt="当前用户头像" className="h-full w-full object-cover" />
            </button>
          </div>
        </header>

        <section className="mb-12">
          <SectionHeader title="协作成员" icon={User} color={IOS_BLUE} />
          {!hasCollaborators ? (
            <div className="rounded-[28px] border border-black/[0.06] bg-[#F2F2F7]/70 p-6 shadow-sm dark:border-white/10 dark:bg-white/[0.06]">
              <div className="grid h-12 w-12 place-items-center rounded-[18px] bg-white text-[#007AFF] shadow-sm dark:bg-white/10 dark:text-[#0A84FF]">
                <User size={22} />
              </div>
              <h3 className="mt-4 text-[20px] font-black tracking-normal text-gray-950 dark:text-white">还没有协作成员</h3>
              <p className="mt-1 max-w-[520px] text-[13px] leading-relaxed text-gray-500 dark:text-gray-400">
                邀请成员加入你的 Pinvou 协作空间后，可以在聊天中 @TA 分派任务。
              </p>
              <button
                type="button"
                onClick={handleOpenInvite}
                className="mt-5 inline-flex h-10 items-center gap-2 rounded-full bg-[#007AFF] px-4 text-[13px] font-bold text-white transition-colors hover:bg-[#006FE6]"
              >
                <Link size={16} />
                邀请成员
              </button>
            </div>
          ) : (
            <div className="overflow-hidden rounded-[24px] border border-black/[0.06] bg-white shadow-sm dark:border-white/10 dark:bg-[#1C1C1E]">
              {members.map(member => (
                <div key={member.memberId || member.peerId || member.displayName} className="flex min-h-[68px] items-center justify-between gap-3 border-b border-black/[0.05] px-4 py-3 last:border-b-0 dark:border-white/10">
                  <div className="flex min-w-0 items-center gap-3">
                    <div className="grid h-10 w-10 shrink-0 place-items-center rounded-full bg-gray-100 text-gray-600 dark:bg-white/10 dark:text-gray-200">
                      <User size={18} />
                    </div>
                    <div className="min-w-0">
                      <div className="truncate text-[15px] font-bold text-gray-950 dark:text-white">{member.isSelf ? `${member.displayName}（我）` : member.displayName}</div>
                      <div className="mt-0.5 text-[12px] font-medium text-gray-500 dark:text-gray-400">{member.role}</div>
                    </div>
                  </div>
                  <span className={cx('shrink-0 rounded-full px-2.5 py-1 text-[11px] font-bold', member.status === 'online' ? 'bg-green-500/10 text-green-600 dark:text-green-400' : 'bg-gray-500/10 text-gray-500 dark:text-gray-400')}>
                    {statusLabel(member.status)}
                  </span>
                </div>
              ))}
            </div>
          )}
        </section>

        <section className="mb-12">
          <SectionHeader title="今日待办" icon={Clock} color={IOS_BLUE} />
          <div className="grid grid-cols-1 gap-6 sm:grid-cols-2">
            {dynamicMetrics.map(metric => (
              <TodayMetricCard
                key={metric.id}
                metric={metric}
                onOpen={dynamicSheetConfigs[metric.id] ? () => setActiveSheetId(metric.id) : undefined}
              />
            ))}
          </div>
        </section>

        <section>
          <SectionHeader title="数据概览" icon={LineChart} color={IOS_PURPLE} />
          <div className="grid grid-cols-1 gap-6 md:grid-cols-5">
            <div className="grid grid-cols-2 gap-6 self-start md:col-span-2">
              {dynamicOverviewMetrics.map(metric => (
                <OverviewMetricCard key={metric.id} metric={metric} />
              ))}
            </div>
            <PerformanceCard completedCount={completedCount} totalCount={allTasks.length} recognitionCount={0} />
          </div>
        </section>
      </main>
      <StatusActionSheet
        config={activeSheetConfig}
        actionFeedback={actionFeedback}
        onClose={() => setActiveSheetId(null)}
        onOpenChat={onOpenChat}
        onOpenTask={onOpenTask}
        onCreateTaskGuide={onCreateTaskGuide}
        onQuickAction={handleQuickAction}
        onAcceptTask={handleAcceptTask}
        onRejectTask={handleRejectTask}
        onCompleteTask={handleCompleteTask}
      />
      {inviteModalOpen && (
        <InviteMembersModal
          members={members}
          loading={inviteLoading}
          invite={inviteLink}
          error={inviteError}
          copied={inviteCopied}
          onClose={() => setInviteModalOpen(false)}
          onCopy={handleCopyInvite}
          onCreateInvite={handleCreateInvite}
        />
      )}
      {joinModalOpen && (
        <JoinInviteModal
          loading={joinLoading}
          error={joinError}
          initialToken={joinInitialToken}
          initialDisplayName={configState.identity?.name || ''}
          onClose={() => {
            if (!joinLoading) setJoinModalOpen(false);
          }}
          onSubmit={handleJoinInvite}
        />
      )}
      {taskRecipientOpen && (
        <TaskRecipientModal
          members={taskReceivers}
          onClose={() => setTaskRecipientOpen(false)}
          onSelect={handleSelectTaskRecipient}
        />
      )}
    </div>
  );
};

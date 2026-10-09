import { requireCondition } from '../errors.mjs';

// Ordinary single-record edges from M. Cross-record transitions require a
// composition command as well; membership in a graph never grants eligibility.
export const ENTITY_EDGES = Object.freeze({
  artifact: { uploading: ['validating'], validating: ['valid', 'rejected'], valid: ['quarantined'] },
  release: { draft: ['assembled'], assembled: ['closed', 'cancelled'] },
  releaseTarget: { draft: ['in_review', 'cancelled'], in_review: ['approved', 'rejected'], approved: ['revoked'] },
  deployment: { draft: ['in_review'], in_review: ['scheduled', 'rejected'], scheduled: ['active', 'paused', 'withdrawn', 'superseded'],
    active: ['paused', 'withdrawn', 'superseded'], paused: ['active', 'withdrawn', 'superseded'] },
  rollout: { draft: ['running', 'aborted'], running: ['paused', 'completed', 'aborted'], paused: ['running', 'aborted'] },
  supplyChain: { pending: ['approved', 'rejected'], approved: ['expired', 'revoked'] },
  selectionScope: { unactivated: ['active'] },
  authorization: { available: ['consumed', 'cancelled', 'expired'] },
  workflow: { ongoing: ['succeeded', 'failed', 'cancelled', 'qualification_lost'] },
  staged: { waiting: ['activating', 'invalidated'], activating: ['activated', 'cancelled', 'failed'] },
  task: { pending: ['observing', 'cancelled'], observing: ['completed', 'failed', 'cancelled'] },
  protection: { idle: ['preparing'], preparing: ['executing', 'idle', 'reconciliation_required'],
    executing: ['idle', 'repair_protected', 'reconciliation_required'], reconciliation_required: ['idle'], repair_protected: ['executing'] },
});

export const TRANSACTION_EDGES = Object.freeze({
  preinstall: { authorization_consumed: ['staging_started'], staging_started: ['staging_verified'], staging_verified: ['staging_completed'] },
  install: { authorization_consumed: ['helper_plan_started', 'execution_ready'], helper_plan_started: ['execution_ready'],
    execution_ready: ['installer_started'], installer_started: ['reconciling'], reconciling: ['installation_verified'],
    installation_verified: ['health_check_started'], health_check_started: ['succeeded'] },
  activate: { authorization_consumed: ['activation_ready'], activation_ready: ['activation_started'],
    activation_started: ['reconciling'], reconciling: ['installation_verified'], installation_verified: ['health_check_started'], health_check_started: ['succeeded'] },
});

export const SESSION_EDGES = Object.freeze({
  decision_received: ['no_update', 'update_offered'], update_offered: ['deferred', 'user_confirmed', 'download_started', 'staged_verified', 'download_resume_context'],
  deferred: ['update_offered', 'user_confirmed', 'download_started', 'download_resume_context'],
  user_confirmed: ['download_started'], download_resume_context: ['download_started'],
  download_started: ['download_succeeded', 'download_failed'], download_succeeded: ['verification_started'],
  verification_started: ['verification_succeeded', 'verification_failed'],
  verification_succeeded: ['preflight_started'], staged_verified: ['preflight_started'],
  preflight_started: ['preflight_succeeded', 'preflight_failed'], preflight_succeeded: ['permission_checked'],
  permission_checked: ['permission_granted', 'permission_denied'], permission_granted: ['preparation_ready', 'preparation_started'],
  preparation_started: ['writers_frozen', 'preparation_failed'], writers_frozen: ['backup_started', 'preparation_ready'],
  backup_started: ['backup_succeeded', 'backup_failed'], backup_succeeded: ['preparation_ready'],
  preparation_ready: ['authorization_requested'], authorization_requested: ['authorized', 'authorization_failed', 'coordination_aborted'],
  authorized: ['authorization_consumed', 'cancelled_before_install', 'authorization_expired'],
});

export const SESSION_TERMINALS = Object.freeze(['no_update', 'download_failed', 'verification_failed', 'preflight_failed',
  'permission_denied', 'preparation_failed', 'backup_failed', 'authorization_failed', 'coordination_aborted',
  'authorization_consumed', 'cancelled_before_install', 'authorization_expired', 'cancelled', 'expired', 'channel_changed']);
export const TRANSACTION_TERMINALS = Object.freeze(['staging_completed', 'staging_failed', 'staging_cancelled',
  'succeeded', 'abandoned_before_install', 'cancelled_before_install', 'failed_manual_repair_required']);

export function assertEdge(graph, from, to) {
  requireCondition(Object.hasOwn(graph, from) && graph[from].includes(to), 'MODEL_EDGE_INVALID');
}

export function assertEntityEdge(kind, from, to) {
  requireCondition(Object.hasOwn(ENTITY_EDGES, kind), 'MODEL_KIND_INVALID');
  assertEdge(ENTITY_EDGES[kind], from, to);
}

function freezeGraph(value) {
  for (const child of Object.values(value)) if (child !== null && typeof child === 'object') freezeGraph(child);
  Object.freeze(value);
}
for (const graph of [ENTITY_EDGES, TRANSACTION_EDGES, SESSION_EDGES]) freezeGraph(graph);

import { join, basename } from 'node:path';
import { randomUUID } from 'node:crypto';
import { requestSession, listSessions } from './client.mjs';
import { enroll, readEnrollment } from './supervision.mjs';
import { readJson, writeJson, sessionDir } from './store.mjs';
import { dependencyConfiguration } from './dependency-observer.mjs';

export async function followDependencies(root, id, { project, taskIds, notify = false, maxNotifications = 10, scope }) {
  const state = await requestSession(root, id, '/status');
  if (!state.capabilities?.includes('dependencies')) return { sessionId: id, status: 'needs_reload', nextAction: 'Reload Pi at idle, then repeat follow.' };
  const config = await dependencyConfiguration({ project, taskIds, notify, maxNotifications });
  if (scope !== undefined) await enroll(root, id, scope);
  if (!readEnrollment(root, id)?.enabled) return { sessionId: id, status: 'needs_enrollment', nextAction: 'Supply --scope with the existing delegated limits.' };
  const observer = await requestSession(root, id, '/dependencies', config, 5000, state.instanceId);
  return { ...observer, status: 'configured', usage: notify ? 'May wake this Pi model; initial snapshot plus later changes, bounded by notification cap.' : 'Observe only; no model messages.' };
}

export async function dependencyStatus(root, id, check = false) {
  const state = await requestSession(root, id, '/status');
  if (!state.capabilities?.includes('dependencies')) return { sessionId: id, status: 'needs_reload' };
  return requestSession(root, id, check ? '/dependencies/check' : '/dependencies', check ? {} : undefined, 2500, state.instanceId);
}

export async function unfollowDependencies(root, id) {
  const dir = sessionDir(root, id);
  if (!readJson(join(dir, 'dependencies.json'))) return { sessionId: id, status: 'not_following' };
  // The durable cancellation marker works even when the endpoint is unreachable.
  writeJson(join(dir, 'dependencies.pause.json'), { sessionId: id, nonce: randomUUID(), pausedAt: new Date().toISOString() });
  try { await requestSession(root, id, '/dependencies/pause', {}); }
  catch { return { sessionId: id, status: 'paused', endpointConfirmed: false }; }
  return { sessionId: id, status: 'paused', endpointConfirmed: true };
}

export async function doctor(root, selectedId) {
  const rows = await listSessions(root);
  const chosen = rows.filter((r) => selectedId ? r.sessionId === selectedId : r.live || readEnrollment(root, r.sessionId)?.enabled);
  if (selectedId && !chosen.length) return { status: 'not_registered', sessionId: selectedId, nextAction: 'Load the Pi extension and use list to obtain its exact session ID.' };
  const sessions = await Promise.all(chosen.map(async (r) => {
    const base = { sessionId: r.sessionId, name: r.label || basename(r.cwd || '') || r.sessionId,
      cwd: r.cwd, live: r.live, runtimeState: r.state };
    if (!r.live) return { ...base, readiness: 'offline', nextAction: 'Inspect the original Pi process; no automatic restart.' };
    if (!r.capabilities?.includes('dependencies')) return { ...base, readiness: 'needs_reload', nextAction: 'Run /reload when idle to load dependency observation.' };
    const observer = await requestSession(root, r.sessionId, '/dependencies');
    const handoff = r.capabilities.includes('handoff') ? await requestSession(root, r.sessionId, '/handoff?budget=16384') : null;
    const enabled = Boolean(readEnrollment(root, r.sessionId)?.enabled);
    return { ...base, readiness: !enabled ? 'needs_enrollment' : observer.phase,
      handoff: handoff?.status || 'unavailable', observer: { phase: observer.phase, notify: observer.notify,
        taskIds: observer.taskIds, pending: observer.pending, notifications: observer.notifications,
        maxNotifications: observer.maxNotifications, checkedAt: observer.checkedAt,
        lastDelivery: observer.lastAlert ? { id: observer.lastAlert.id, status: observer.lastAlert.receiptStatus } : null },
      nextAction: !enabled ? 'Use follow with --scope, or enroll first.' :
        observer.phase === 'notification_limit' ? 'Unfollow, then explicitly follow again only if more model wakes are authorized.' :
        ['source_error', 'source_identity_changed', 'storage_error', 'control_state_unavailable', 'delivery_unknown', 'awaiting_delivery'].includes(observer.phase) ?
          'Inspect dependencies evidence/error before further action; unknown sends are not retried.' :
        ['not_configured', 'needs_refollow', 'scope_changed', 'paused'].includes(observer.phase) ?
          'Use follow with the intended project/tasks; --notify explicitly enables bounded wake messages.' :
          'Observation is configured. dependencies shows evidence; unfollow pauses it.' };
  }));
  return { status: 'diagnosed', sessions };
}

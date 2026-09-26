import { join, basename, resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
import { requestSession, listSessions } from './client.mjs';
import { enroll, readEnrollment } from './supervision.mjs';
import { readJson, registry, writeJson, sessionDir } from './store.mjs';
import { dependencyConfiguration, findOwnerSubscription } from './dependency-observer.mjs';
import { runtimeInfo } from './activation.mjs';

export async function followDependencies(root, id, { project, taskIds, notify = false, maxNotifications = 10, scope }) {
  let state;
  try {
    state = await requestSession(root, id, '/status');
  } catch (error) {
    // The common mistake is passing the managed run id (a child's EDDA_SESSION_ID)
    // instead of the Pi channel session id. Name the identity it expected.
    if (/no reachable registered owner/.test(error.message)) {
      throw new Error(`No registered session '${id}'. follow takes the Pi channel session id — read it from 'edda-pi run-status <runId>' (the sessionId field) or the launch output — not the managed run id (EDDA_SESSION_ID).`);
    }
    throw error;
  }
  if (!state.capabilities?.includes('dependencies')) return { sessionId: id, status: 'needs_reload', nextAction: 'Reload Pi at idle, then repeat follow.' };
  const config = await dependencyConfiguration({ project, taskIds, notify, maxNotifications });
  if (scope !== undefined) await enroll(root, id, scope);
  if (!readEnrollment(root, id)?.enabled) return { sessionId: id, status: 'needs_enrollment', nextAction: 'Supply --scope with the existing delegated limits.' };
  const observer = await requestSession(root, id, '/dependencies', config, 5000, state.instanceId);
  return { ...observer, status: 'configured', usage: notify ? 'May wake this Pi model; initial snapshot plus later changes, bounded by notification cap.' : 'Observe only; no model messages.' };
}

export async function dependencyStatus(root, id, check = false) {
  let state;
  try { state = await requestSession(root, id, '/status'); }
  catch (error) {
    // Offline: project the persisted subscription (owner-scoped when one owns the
    // session), so status still resolves without a second subscription store.
    const owner = findOwnerSubscription(root, id);
    const record = readJson(join(owner.dir ?? sessionDir(root, id), 'dependencies.json'));
    if (!record) {
      if (!owner.dir && owner.unreadable.length) {
        return { sessionId: id, status: 'unavailable', unreadable: owner.unreadable,
          nextAction: 'An owner subscription record is unreadable and was not repaired; inspect the owner-lifecycle directory.' };
      }
      throw error;
    }
    return { sessionId: id, status: 'offline', scope: owner.dir ? 'owner' : 'session', phase: record.phase,
      project: record.project, taskIds: record.taskIds, notify: record.notify,
      changeSequence: record.sequence || 0, handledSequence: record.handledSequence || 0,
      notifications: record.notifications || 0, checkedAt: record.checkedAt,
      ...(owner.unreadable.length ? { unreadable: owner.unreadable } : {}),
      notice: 'Read-only persisted subscription projection; the live session was unreachable.' };
  }
  if (!state.capabilities?.includes('dependencies')) return { sessionId: id, status: 'needs_reload' };
  return requestSession(root, id, check ? '/dependencies/check' : '/dependencies', check ? {} : undefined, 2500, state.instanceId);
}

export async function unfollowDependencies(root, id) {
  const owner = findOwnerSubscription(root, id);
  const dir = owner.dir ?? sessionDir(root, id);
  if (!readJson(join(dir, 'dependencies.json'))) {
    if (!owner.dir && owner.unreadable.length) {
      return { sessionId: id, status: 'unavailable', unreadable: owner.unreadable,
        nextAction: 'An owner subscription record is unreadable and was not repaired; inspect the owner-lifecycle directory.' };
    }
    return { sessionId: id, status: 'not_following' };
  }
  // The durable cancellation marker works even when the endpoint is unreachable.
  // It carries the same identity the observer's pauseToken() validates.
  writeJson(join(dir, 'dependencies.pause.json'), owner.dir
    ? { ownerRef: owner.ownerRef, nonce: randomUUID(), pausedAt: new Date().toISOString() }
    : { sessionId: id, nonce: randomUUID(), pausedAt: new Date().toISOString() });
  const scope = owner.dir ? 'owner' : 'session';
  try { await requestSession(root, id, '/dependencies/pause', {}); }
  catch { return { sessionId: id, status: 'paused', endpointConfirmed: false, scope }; }
  return { sessionId: id, status: 'paused', endpointConfirmed: true, scope };
}

export async function doctor(root, selectedId) {
  const rows = await listSessions(root);
  // Registry health is read independently of the session inventory so a record
  // that is unreadable AND has no recoverable identity is still named here.
  const health = registry(root).map(({ dir, state, owner, stateError, ownerError }) => {
    const error = stateError || ownerError;
    if (!error) return null;
    return { dir, sessionId: owner?.sessionId || state?.sessionId || null, record: error.record, message: error.message };
  }).filter(Boolean);
  const unreadable = health.map(({ sessionId, record }) => ({ sessionId, record }));
  const enrolled = (id) => { try { return Boolean(readEnrollment(root, id)?.enabled); } catch { return false; } };
  const chosen = rows.filter((r) => selectedId ? r.sessionId === selectedId : r.live || enrolled(r.sessionId));
  if (selectedId && !chosen.length) {
    let target = null;
    try { target = sessionDir(root, selectedId); } catch { /* not a valid session selector */ }
    if (target && health.some((entry) => entry.dir === target)) return { status: 'record_unavailable', sessionId: selectedId, unreadable,
      nextAction: 'The record is unreadable and was not repaired; it was preserved as-is.' };
    return { status: 'not_registered', sessionId: selectedId, unreadable,
      nextAction: 'Load the Pi extension and use list to obtain its exact session ID.' };
  }
  const installed = runtimeInfo(root);
  const pathKey = (path) => process.platform === 'win32' ? resolve(path).toLowerCase() : resolve(path);
  const knownChannels = [installed.channel.path, ...installed.releases.filter((v) => v.verified).map((v) => v.channel)].map(pathKey);
  const sessions = await Promise.all(chosen.map(async (r) => {
    const base = { sessionId: r.sessionId, name: r.label || basename(r.cwd || '') || r.sessionId,
      cwd: r.cwd, live: r.live, runtimeState: r.state };
    if (r.error?.code === 'record_unavailable') return { ...base, readiness: 'record_unavailable',
      error: { ...r.error },
      nextAction: 'The record was preserved. Resume the original Pi conversation with the installed extension; use /edda-session-recover there to preserve damaged state and reconnect only after the old owner is dead. For older loaded extensions use edda-pi recover SESSION_ID --instance OLD_INSTANCE_UUID, then reopen the same Pi session with the installed extension.' };
    if (!r.live) return { ...base, readiness: 'offline', nextAction: 'Inspect the original Pi process; no automatic restart.' };
    if (typeof r.integration?.modulePath !== 'string' || !knownChannels.includes(pathKey(r.integration.modulePath))) {
      return { ...base, readiness: 'stale_extension', loadedModule: r.integration?.modulePath || null,
        nextAction: 'This live session loaded an unverified/stale channel. Preserve its work; reopen the SAME Pi session with the installed extension from edda-pi runtime-info, without restarting other sessions.' };
    }
    if (!r.capabilities?.includes('dependencies')) return { ...base, readiness: 'needs_reload', nextAction: 'Run /reload when idle to load dependency observation.' };
    const observer = await requestSession(root, r.sessionId, '/dependencies');
    const handoff = r.capabilities.includes('handoff') ? await requestSession(root, r.sessionId, '/handoff?budget=16384') : null;
    const enabled = enrolled(r.sessionId);
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
  return { status: 'diagnosed', sessions, unreadable };
}

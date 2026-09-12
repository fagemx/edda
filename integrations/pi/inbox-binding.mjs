import { digest } from './store.mjs';

export function assertCurrentEvent(event, state, handoff, policy) {
  if (!state.live || state.state !== 'idle' || event.sessionId !== state.sessionId || event.instanceId !== state.instanceId) {
    throw new Error('Inbox response requires the original live idle instance');
  }
  if (policy?.enabled === false || (event.binding.scopeDigest !== null &&
    (!policy?.enabled || typeof policy.scope !== 'string' || digest(policy.scope) !== event.binding.scopeDigest))) {
    throw new Error('Inbox scope changed or supervision paused');
  }
  if ((handoff.manifestRevision ?? null) !== event.binding.manifestRevision ||
    (handoff.manifest?.runId ?? null) !== event.binding.runId) throw new Error('Inbox manifest binding changed');
  if (event.report) {
    if (handoff.status !== 'ready' || handoff.report?.reportId !== event.report.reportId ||
      handoff.workEpoch !== event.report.workEpoch) throw new Error('Inbox report is no longer current');
  } else if (state.inbox?.localEpoch !== event.localEpoch) throw new Error('Inbox work epoch changed');
}
export function authorizationMatches(event, record) {
  return event.kind === 'decision_request' && event.binding.runId && event.binding.manifestRevision &&
    digest(JSON.stringify(record.binding)) === digest(JSON.stringify(event.binding)) &&
    record.sessionId === event.sessionId && record.requestedAction === event.report.decision.requestedAction &&
    record.resource === event.report.decision.resource;
}
export function checkAuthorization(store, event, id) {
  const record = store.read('authorizations', id);
  if (!record || store.read('revocations', id) || !authorizationMatches(event, record.data)) {
    throw new Error('Authorization is missing, revoked or does not match this exact request/scope');
  }
  return record;
}

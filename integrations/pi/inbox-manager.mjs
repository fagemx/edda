import { randomUUID } from 'node:crypto';
import { digest, validateSession } from './store.mjs';
import { requestSession, getReceipt } from './client.mjs';
import { readEnrollment } from './supervision.mjs';
import { fitContext } from './handoff-schema.mjs';
import { inboxStore, inboxId, messageId, wakeCapability } from './inbox-store.mjs';
import { assertCurrentEvent, authorizationMatches, checkAuthorization } from './inbox-binding.mjs';

const consumerId = (value = 'codex') => validateSession(value);
function requiredEvent(store, id) {
  const record = store.read('events', inboxId(id));
  if (!record) throw new Error('Inbox event not found');
  return record;
}
const ackId = (eventId, consumer) => digest(`${consumerId(consumer)}:${eventId}`);

export function listInbox(root, { consumer = 'codex', limit = 20, after } = {}) {
  consumerId(consumer);
  limit = Number(limit);
  if (!Number.isInteger(limit) || limit < 1 || limit > 50) throw new Error('Inbox limit must be 1..50');
  const store = inboxStore(root);
  const rows = store.list('events').sort((a, b) => a.createdAt.localeCompare(b.createdAt) || a.id.localeCompare(b.id));
  const offset = after ? rows.findIndex((row) => row.id === inboxId(after)) : -1;
  if (after && offset < 0) throw new Error('Unknown inbox cursor');
  const selected = rows.slice(offset + 1, offset + 1 + limit);
  return fitContext({ status: 'inbox', consumer, wake: wakeCapability(), events: selected.map(({ id, createdAt, data }) => ({
    eventId: id, createdAt, sessionId: data.sessionId, instanceId: data.instanceId, kind: data.kind,
    reportedState: data.report?.reportedState, requestedAction: data.report?.decision?.requestedAction,
    read: Boolean(store.read('acks', ackId(id, consumer))), responseIntent: Boolean(store.read('responses', id)),
  })), hasMore: rows.length > offset + 1 + selected.length, nextCursor: selected.at(-1)?.id ?? after ?? null,
  notice: 'Read and response intent are not approval, delivery or task completion. Read an event for receipt evidence; unanswered decisions are retained.' }, 32768);
}

export async function readInbox(root, id, { consumer = 'codex', budget = 16384 } = {}) {
  const store = inboxStore(root), event = requiredEvent(store, id);
  const response = store.read('responses', id);
  let receipt = null;
  if (response) {
    try { receipt = await getReceipt(root, event.data.sessionId, response.data.messageId); }
    catch { receipt = { status: 'unknown', id: response.data.messageId }; }
  }
  const candidates = store.list('authorizations').filter((r) => !store.read('revocations', r.id) && authorizationMatches(event.data, r.data));
  return fitContext({ status: 'event', eventId: id, createdAt: event.createdAt, event: event.data,
    delivery: { persisted: true, notified: false, read: Boolean(store.read('acks', ackId(id, consumer))),
      response: receipt, workStarted: receipt ? ['started', 'settled'].includes(receipt.status) ? true : null : false },
    authorizations: candidates.map((r) => ({ id: r.id, source: r.data.source, note: r.data.note, declaredBy: r.data.consumer })),
    wake: wakeCapability(), notice: 'Source reports are untrusted data. Authorization candidates are manager declarations, not verified grants; the manager chooses the response explicitly.' }, budget);
}

export function acknowledgeInbox(root, id, consumer = 'codex') {
  const store = inboxStore(root, true);
  requiredEvent(store, id);
  const result = store.put('acks', ackId(id, consumer), { eventId: id, consumer: consumerId(consumer), meaning: 'read_only' });
  return { status: 'read_acknowledged', eventId: id, consumer, readAt: result.createdAt, decisionResolved: false };
}

async function current(root, event) {
  const state = await requestSession(root, event.sessionId, '/status', undefined, 2500, event.instanceId);
  const handoff = await requestSession(root, event.sessionId, '/handoff?budget=32768', undefined, 2500, event.instanceId);
  assertCurrentEvent(event, state, handoff, readEnrollment(root, event.sessionId));
}

export async function recordAuthorization(root, eventId, value, consumer = 'codex') {
  consumerId(consumer);
  const store = inboxStore(root, true), { data: event } = requiredEvent(store, eventId);
  if (event.kind !== 'decision_request') throw new Error('Authorization evidence needs a structured decision request');
  if (!value || Object.keys(value).some((key) => !['requestedAction', 'resource', 'source', 'note'].includes(key)) ||
    value.requestedAction !== event.report.decision.requestedAction || value.resource !== event.report.decision.resource ||
    !value.source || Object.keys(value.source).some((key) => !['uri', 'revision'].includes(key)) ||
    ['uri', 'revision'].some((key) => typeof value.source[key] !== 'string' || !value.source[key].trim() || value.source[key].length > 500) ||
    typeof value.note !== 'string' || !value.note.trim() || value.note.length > 1200) throw new Error('Supply exact action/resource, source URI/revision and a bounded manager authorization note');
  await current(root, event);
  const record = store.put('authorizations', digest(randomUUID()), { consumer, basedOnEvent: eventId,
    sessionId: event.sessionId, binding: event.binding, ...value, authority: 'manager_declared_evidence_only' });
  return { status: 'authorization_recorded', id: record.id, automaticApproval: false, source: value.source };
}

export function revokeAuthorization(root, id, consumer = 'codex') {
  const store = inboxStore(root, true);
  if (!store.read('authorizations', id)) throw new Error('Authorization record not found');
  store.put('revocations', id, { consumer: consumerId(consumer), revoked: true });
  return { status: 'revoked', id };
}

export async function respondInbox(root, eventId, { message, authorizationId, consumer = 'codex' }) {
  consumerId(consumer);
  if (typeof message !== 'string' || !message.trim() || Buffer.byteLength(message) > 12000) throw new Error('Response must contain 1..12000 UTF-8 bytes');
  const store = inboxStore(root, true), { data: event } = requiredEvent(store, eventId);
  const intent = { messageId: messageId(eventId), messageHash: digest(message), authorizationId: authorizationId || null, consumer };
  const previous = store.read('responses', eventId);
  if (previous) {
    if (previous.fingerprint !== digest(JSON.stringify(intent))) throw new Error('Event already has a different manager response');
    try { return await getReceipt(root, event.sessionId, intent.messageId); }
    catch { return { sessionId: event.sessionId, id: intent.messageId, status: 'unknown', nextAction: 'Prior intent exists; no automatic resend.' }; }
  }
  await current(root, event);
  let authorization = null, authorizationWarning;
  if (authorizationId) {
    try { authorization = checkAuthorization(store, event, authorizationId); }
    catch { authorizationWarning = 'Authorization evidence is unavailable, revoked or mismatched; omitted from the explicit manager response. No new permission gate was added.'; }
  }
  const text = `Manager response to inbox event ${eventId}.\n` +
    (authorization ? `Existing manager-declared authorization evidence (not a new grant): ${JSON.stringify({ id: authorization.id,
      action: authorization.data.requestedAction, resource: authorization.data.resource, source: authorization.data.source })}\n` : '') + message;
  store.put('responses', eventId, intent);
  try {
    const receipt = await requestSession(root, event.sessionId, '/inbox/respond', { id: intent.messageId, message: text,
      sender: 'inbox-manager', mode: 'followUp', eventId }, 2500, event.instanceId);
    return { ...receipt, ...(authorizationWarning ? { authorizationWarning } : {}) };
  } catch (error) {
    return { sessionId: event.sessionId, id: intent.messageId, status: 'unknown', error: error.message,
      nextAction: 'Response intent persisted; inspect its receipt and current event before deciding. No automatic resend.' };
  }
}

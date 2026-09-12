import { mkdirSync, readdirSync, existsSync, writeFileSync, unlinkSync } from 'node:fs';
import { join } from 'node:path';
import { digest, privateRoot, readJson, writeJson, validateSession } from './store.mjs';
import { inspectSession, requestSession, getReceipt, managementContext } from './client.mjs';
import { fitContext } from './handoff-schema.mjs';

const folder = (root) => join(root, 'supervision');
const recordPath = (root, id) => join(folder(root), `${digest(validateSession(id))}.json`);
export function readEnrollment(root, id) {
  const value = readJson(recordPath(root, id));
  if (value && value.sessionId !== id) throw new Error('Enrollment identity mismatch');
  return value;
}
const now = () => new Date().toISOString();
function record(root, id) {
  const value = readJson(recordPath(root, id));
  if (!value || value.sessionId !== id) throw new Error('Session is not enrolled for supervision');
  return value;
}
async function locked(root, id, action) {
  const path = `${recordPath(root, id)}.lock`;
  writeFileSync(path, '', { flag: 'wx', mode: 0o600 });
  try { return await action(); } finally { unlinkSync(path); }
}
export async function enroll(root, id, scope) {
  if (typeof scope !== 'string' || !scope.trim() || scope.length > 8000) throw new Error('An explicit bounded supervision scope is required');
  const state = await requestSession(root, id, '/status');
  privateRoot(root);
  mkdirSync(folder(root), { recursive: true, mode: 0o700 });
  return locked(root, id, async () => {
  const previous = readJson(recordPath(root, id));
  const value = { ...previous, sessionId: id, cwd: state.cwd, scope, enabled: true,
    enrolledAt: previous?.enrolledAt || now(), updatedAt: now(), action: 'enrolled',
    cursor: previous?.cursor || null };
  writeJson(recordPath(root, id), value);
  return value;
  });
}
export async function watch(root, { withConversation = false } = {}) {
  if (!existsSync(folder(root))) return [];
  const records = readdirSync(folder(root)).filter((name) => /^[0-9a-f]{64}\.json$/.test(name))
    .map((name) => readJson(join(folder(root), name))).filter((r) => r.enabled);
  return Promise.all(records.map(async (r) => {
    const supervision = withConversation ? r : { sessionId: r.sessionId, cwd: r.cwd, enabled: r.enabled,
      action: r.action, cursor: r.cursor, requiresBriefBeforeDecision: true };
    try {
      if (!withConversation) {
        const view = await managementContext(root, r.sessionId);
        const h = view.handoff;
        return { supervision, state: view.state, handoff: { status: h.status, attention: h.attention,
          runId: h.manifest?.runId, manifestRevision: h.manifestRevision, reportedState: h.report?.reportedState,
          stage: h.report?.stage, reportId: h.report?.reportId }, assessment: h.attention };
      }
      const view = await inspectSession(root, r.sessionId, { after: r.cursor || undefined, limit: 20 });
      return { supervision, ...view };
    } catch (error) {
      return { supervision, assessment: 'inspection_failed_do_not_send', error: error.message };
    }
  }));
}

export async function managementBrief(root, id, budget = 16384) {
  const view = await managementContext(root, id, 32768);
  const policy = readJson(recordPath(root, id));
  return fitContext({ ...view.handoff, sessionId: id, instanceId: view.state.instanceId,
    runtimeState: view.state.state, supervisorScope: policy?.scope || null,
    scopeNotice: 'supervisorScope is the local enrollment policy; manifest scope is declared controller context. Neither is resolved into a new grant here.' }, budget);
}
export async function checkpoint(root, id, { cursor, action, note }) {
  if (!['observed', 'working', 'waiting_user', 'complete', 'paused'].includes(action)) throw new Error('Invalid checkpoint action');
  if (typeof note !== 'string' || !note.trim() || note.length > 4000) throw new Error('Checkpoint needs a bounded evidence note');
  if (action !== 'paused' && (typeof cursor !== 'string' || !cursor || cursor.length > 200)) throw new Error('Checkpoint requires an observed conversation cursor');
  return locked(root, id, async () => {
  const value = record(root, id);
  if (action === 'paused') {
    // Cancellation must remain available when the runtime/history is gone.
    // Keep the last verified cursor instead of accepting a new unverified one.
    const paused = { ...value, action, note, enabled: false, updatedAt: now() };
    writeJson(recordPath(root, id), paused);
    return paused;
  }
  // Verify the cursor belongs to the currently inspectable branch.
  const view = await inspectSession(root, id, { after: cursor, limit: 1 });
  if (action === 'complete' && (!view.state.live || view.state.state !== 'idle')) throw new Error('Completion checkpoint requires a live idle session and evidence note');
  if (action === 'complete' && view.conversation.headCursor !== cursor) throw new Error('New unread activity prevents completion; inspect the latest head');
  const next = { ...value, cursor, action, note, updatedAt: now(),
    enabled: !['complete', 'paused'].includes(action) };
  writeJson(recordPath(root, id), next);
  return next;
  });
}
function replyId(id, cursor) {
  const h = digest(`edda-manager-v1:${id}:${cursor}`);
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-5${h.slice(13, 16)}-a${h.slice(17, 20)}-${h.slice(20, 32)}`;
}
export async function reply(root, id, { to, message }) {
  if (typeof to !== 'string' || !to) throw new Error('Reply requires the conversation cursor you inspected');
  if (typeof message !== 'string' || !message.trim() || Buffer.byteLength(message) > 16384) throw new Error('Reply must contain 1..16384 bytes');
  // One controller decision per cursor; do not race two supervising invocations.
  return locked(root, id, async () => {
    const r = record(root, id);
    if (!r.enabled) throw new Error('Supervision is paused');
    const idempotencyId = replyId(id, to);
    if (r.lastReply?.cursor === to) {
      if (r.lastReply.messageHash !== digest(message)) throw new Error('This cursor already has a different reply; inspect before deciding again');
      try { return await getReceipt(root, id, idempotencyId); }
      catch { return { sessionId: id, id: idempotencyId, status: 'unknown', error: 'Prior attempt has no receipt; do not automatically retry' }; }
    }
    const view = await inspectSession(root, id, { limit: 1 });
    if (!view.state.live || view.state.state !== 'idle') throw new Error('Managed reply requires an idle, live session');
    if (view.conversation.headCursor !== to) throw new Error('New conversation activity appeared; read it before replying');
    // A write-ahead intent survives controller interruption. It never grants
    // broader authority than r.scope, which the supervising agent must read.
    const next = { ...r, updatedAt: now(), lastReply: { cursor: to, id: idempotencyId,
      messageHash: digest(message), attemptedAt: now() }, action: 'reply_attempted' };
    writeJson(recordPath(root, id), next);
    return await requestSession(root, id, '/messages', { id: idempotencyId, message,
      sender: 'codex-manager', mode: 'followUp' }, 2500, view.state.instanceId);
  });
}

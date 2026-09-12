import { readJson, registry, sessionDir, validateId, defaultRoot } from './store.mjs';
import { join } from 'node:path';
import { findTranscript, readTranscript } from './conversation.mjs';

export async function requestSession(root, sessionId, path, body, timeoutMs = 2500, expectedInstance) {
  const owner = readJson(join(sessionDir(root, sessionId), 'owner.json'));
  if (!owner || owner.sessionId !== sessionId || !Number.isInteger(owner.port) || owner.port < 1 || owner.port > 65535 ||
      typeof owner.token !== 'string' || !/^[0-9a-f]{64}$/.test(owner.token)) throw new Error('Session has no reachable registered owner');
  validateId(owner.instanceId);
  if (expectedInstance && owner.instanceId !== expectedInstance) throw new Error('Session instance changed; inspect again before sending');
  let response;
  try {
    response = await fetch(`http://127.0.0.1:${owner.port}${path}`, {
      method: body ? 'POST' : 'GET', redirect: 'error', signal: AbortSignal.timeout(timeoutMs),
      headers: { authorization: `Bearer ${owner.token}`, 'x-edda-instance': owner.instanceId, 'content-type': 'application/json' },
      body: body ? JSON.stringify(body) : undefined,
    });
  } catch {
    throw new Error(body ? `Delivery outcome unknown; query receipt ${body.id} before retrying with the SAME ID` : 'Session endpoint unreachable');
  }
  const result = await response.json();
  if (!response.ok) throw new Error(result.error || `Channel HTTP ${response.status}`);
  if (result.sessionId !== sessionId || (path === '/status' && result.instanceId !== owner.instanceId)) throw new Error('Session response identity mismatch');
  return result;
}

export async function inspectSession(root, sessionId, options = {}) {
  let state;
  try { state = await requestSession(root, sessionId, '/status'); }
  catch {
    const stored = readJson(join(sessionDir(root, sessionId), 'state.json'));
    if (!stored || stored.sessionId !== sessionId) throw new Error('No registered session state');
    state = { ...stored, live: false, state: 'unreachable' };
  }
  let conversation;
  if (state.live && state.capabilities?.includes('conversation')) {
    const params = new URLSearchParams({ limit: String(options.limit || 20) });
    if (options.after) params.set('after', options.after);
    conversation = await requestSession(root, sessionId, `/conversation?${params}`, undefined, 2500, state.instanceId);
  } else {
    const path = await findTranscript(sessionId, state.cwd, options);
    conversation = { sessionId, instanceId: state.instanceId, ...await readTranscript(path, options) };
  }
  return { state, conversation, assessment: state.state === 'idle' ? 'read_reply_before_deciding' :
    state.state === 'waiting_user' ? 'structured_user_prompt' : state.live ? 'runtime_active_not_task_completion' : 'offline_inspect_only' };
}

export async function listSessions(root = defaultRoot()) {
  return Promise.all(registry(root).map(async ({ state, owner }) => {
    const sessionId = owner?.sessionId || state.sessionId;
    try { return await requestSession(root, sessionId, '/status'); }
    catch {
      return { ...state, sessionId, instanceId: owner?.instanceId || state?.instanceId,
        live: false, state: owner ? 'unreachable' : (state?.state === 'stopped' ? 'stopped' : 'unreachable') };
    }
  }));
}

export async function getReceipt(root, sessionId, id) {
  validateId(id);
  try { return await requestSession(root, sessionId, `/receipts/${id}`); }
  catch {
    const stored = readJson(join(sessionDir(root, sessionId), 'receipts', `${id.toLowerCase()}.json`));
    if (!stored) throw new Error('Receipt not found; do not assume a new ID is safe');
    const { fingerprint, envelopeHash, ...publicRecord } = stored;
    return { ...publicRecord, status: ['settled', 'failed', 'unknown'].includes(stored.status) ? stored.status : 'unknown',
      lastRecordedStatus: stored.status, live: false };
  }
}

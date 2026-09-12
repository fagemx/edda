import { readJson, registry, sessionDir, validateId, defaultRoot } from './store.mjs';
import { join } from 'node:path';

export async function requestSession(root, sessionId, path, body, timeoutMs = 2500) {
  const owner = readJson(join(sessionDir(root, sessionId), 'owner.json'));
  if (!owner || owner.sessionId !== sessionId || !Number.isInteger(owner.port) || owner.port < 1 || owner.port > 65535 ||
      typeof owner.token !== 'string' || !/^[0-9a-f]{64}$/.test(owner.token)) throw new Error('Session has no reachable registered owner');
  validateId(owner.instanceId);
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

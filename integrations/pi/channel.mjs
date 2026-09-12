import { createServer } from 'node:http';
import { randomBytes, randomUUID, timingSafeEqual } from 'node:crypto';
import { openStore, digest, validateSession, validateId } from './store.mjs';
import { createHandoff } from './handoff.mjs';
import { createDependencyObserver } from './dependency-observer.mjs';
import { readEnrollment } from './supervision.mjs';
import { createInboxProducer } from './inbox-producer.mjs';
import { inboxStore, inboxId, messageId } from './inbox-store.mjs';
import { assertCurrentEvent } from './inbox-binding.mjs';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const integrationVersion = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8')).version;

const terminal = new Set(['settled', 'failed', 'unknown']);
const now = () => new Date().toISOString();
const fail = (message, status = 400) => Object.assign(new Error(message), { status });

function validateMessage(body) {
  validateId(body.id);
  if (typeof body.message !== 'string' || !body.message.trim() || Buffer.byteLength(body.message) > 16384) throw fail('Message must contain 1..16384 bytes');
  if (typeof body.sender !== 'string' || !/^[\p{L}\p{N}_.@ /:-]{1,80}$/u.test(body.sender)) throw fail('Invalid sender label');
  if (!['followUp', 'steer'].includes(body.mode)) throw fail('Mode must be followUp or steer');
  return { id: body.id.toLowerCase(), message: body.message, sender: body.sender, mode: body.mode };
}

async function jsonBody(req) {
  let bytes = 0;
  const chunks = [];
  for await (const chunk of req) {
    bytes += chunk.length;
    if (bytes > 24576) throw fail('Request too large', 413);
    chunks.push(chunk);
  }
  const body = JSON.parse(Buffer.concat(chunks).toString('utf8'));
  if (!body || Array.isArray(body) || typeof body !== 'object') throw fail('Expected JSON object');
  return body;
}

export async function startChannel({ root, sessionId, cwd, label = '', deliver, getConversation, heartbeatMs = 5000,
  dependencyCommand, dependencyPollMs = 60000 }) {
  validateSession(sessionId);
  const instanceId = randomUUID();
  const token = randomBytes(32).toString('hex');
  const owner = { version: 1, sessionId, instanceId, pid: process.pid, token, port: null };
  const store = openStore(root, sessionId, owner);
  let handoff;
  let dependencies;
  let inbox;
  try { handoff = createHandoff(store.dir, sessionId, instanceId); }
  catch (error) { store.release(); throw error; }
  let closed = false;
  let storageError = false;
  let busy = false;
  let waiting = false;
  let lastStopReason = null;
  const tools = new Map();
  const state = { version: 1, sessionId, instanceId, pid: process.pid, cwd, label: String(label).slice(0, 120),
    state: 'idle', startedAt: now(), heartbeatAt: now(), lastProgressAt: now(), lastEvent: 'session_start',
    toolNames: [], unsettledMessages: 0 };
  let receipts;
  try {
    receipts = new Map(store.receipts().map((r) => {
      if (!terminal.has(r.status) && r.instanceId !== instanceId) {
        const recovered = { ...r, status: 'unknown', lastRecordedStatus: r.status, updatedAt: now() };
        store.putReceipt(recovered);
        return [r.id, recovered];
      }
      return [r.id, r];
    }));
  }
  catch (error) { store.release(); throw error; }
  const save = () => {
    state.heartbeatAt = now();
    // These are channel receipts, NOT Pi's runtime queue occupancy.
    state.unsettledMessages = [...receipts.values()].filter((r) => r.instanceId === instanceId && !terminal.has(r.status)).length;
    store.state(state);
  };
  const update = (receipt, status) => {
    const next = { ...receipt, status, updatedAt: now() };
    store.putReceipt(next);
    receipts.set(next.id, next);
    return next;
  };
  const publicReceipt = ({ fingerprint, envelopeHash, ...rest }) => rest;
  const recompute = () => {
    state.state = closed ? 'stopped' : storageError ? 'unavailable' : waiting ? 'waiting_user' : tools.size ? 'executing_tool' : busy ? 'running' : 'idle';
    state.toolNames = [...new Set(tools.values())];
  };
  const channel = {
    sessionId, instanceId,
    snapshot: () => ({ ...state, toolNames: [...state.toolNames], live: !closed,
      integration: { version: integrationVersion, modulePath: fileURLToPath(import.meta.url), releaseId: process.env.EDDA_PI_RELEASE_ID || null },
      capabilities: ['send', 'receipts', 'handoff', 'dependencies', 'inbox', ...(getConversation ? ['conversation'] : [])],
      inbox: inbox?.status() }),
    get dependencies() { return dependencies; },
    handoffContext: (budget) => handoff.context(channel.snapshot(), budget),
    reportHandoff(id, value) {
      if (closed || storageError) throw fail('Channel unavailable', 503);
      inbox.reconcile();
      const p = readEnrollment(root, sessionId);
      const receipt = handoff.report(id, value, p?.enabled && typeof p.scope === 'string' ? digest(p.scope) : null);
      inbox.reconcile();
      return receipt;
    },
    event(name, data = {}) {
      if (closed) return;
      if (name === 'agent_start') { busy = true; lastStopReason = null; inbox.begin(); }
      if (name === 'ui_prompt_start') waiting = true;
      if (name === 'ui_prompt_end') waiting = false;
      if (name === 'tool_execution_start') tools.set(data.toolCallId, data.toolName);
      if (name === 'tool_execution_end') tools.delete(data.toolCallId);
      if (name === 'assistant_end') { lastStopReason = data.stopReason; inbox.assistant(data.text); }
      state.lastEvent = name;
      state.lastProgressAt = now();
      recompute();
      save();
      handoff.event(name);
    },
    messageStarted(text) {
      if (closed || typeof text !== 'string') return;
      const receipt = [...receipts.values()].find((r) => r.instanceId === instanceId && !terminal.has(r.status) && r.envelopeHash === digest(text));
      if (receipt) update(receipt, 'started');
      channel.event('message_start');
    },
    settled() {
      if (closed) return;
      busy = false;
      tools.clear();
      for (const r of receipts.values()) {
        if (r.instanceId === instanceId && r.status === 'started') update(r, ['error', 'aborted'].includes(lastStopReason) ? 'failed' : 'settled');
      }
      channel.event('agent_settled');
      inbox.settled();
    },
    async close() {
      if (closed) return;
      closed = true;
      clearInterval(heartbeat);
      try {
        await dependencies?.close();
        for (const r of receipts.values()) {
          if (r.instanceId === instanceId && !terminal.has(r.status)) update(r, 'unknown');
        }
        recompute();
        save();
      } finally {
        server.closeAllConnections();
        await new Promise((resolve) => server.close(resolve));
        store.release();
      }
    },
  };

  function submitMessage(body, requireIdle = false) {
    const message = validateMessage(body);
    const fingerprint = digest(JSON.stringify(message));
    const prior = receipts.get(message.id);
    if (prior) {
      if (prior.fingerprint !== fingerprint) throw fail('Message ID conflict', 409);
      return publicReceipt(prior);
    }
    if (closed || storageError) throw fail('Channel unavailable', 503);
    if (waiting) throw fail('Pi is waiting for a user answer; message delivery refused', 409);
    if (requireIdle && channel.snapshot().state !== 'idle') throw fail('Receiver is no longer idle', 409);
    if (receipts.size >= 1000) throw fail('Receipt capacity reached; archive this session before sending more', 409);
    const envelope = `[Edda message ${message.id} from ${message.sender}]\n${message.message}`;
    const receipt = { id: message.id, sessionId, instanceId, sender: message.sender, mode: message.mode,
      status: 'accepted', fingerprint, envelopeHash: digest(envelope), createdAt: now(), updatedAt: now() };
    store.putReceipt(receipt);
    receipts.set(receipt.id, receipt);
    try {
      deliver(envelope, { deliverAs: message.mode, expandPromptTemplates: false });
      const current = receipts.get(receipt.id);
      if (current.status === 'accepted') update(current, 'unconfirmed');
    } catch { update(receipts.get(receipt.id), 'unknown'); }
    save();
    return publicReceipt(receipts.get(receipt.id));
  }

  const server = createServer(async (req, res) => {
    const reply = (status, value) => {
      res.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store', 'x-content-type-options': 'nosniff' });
      res.end(JSON.stringify(value));
    };
    try {
      const auth = req.headers.authorization || '';
      const expected = `Bearer ${token}`;
      if (req.headers.origin || Buffer.byteLength(auth) !== Buffer.byteLength(expected) || !timingSafeEqual(Buffer.from(auth), Buffer.from(expected))) throw fail('Unauthorized', 401);
      if (req.headers['x-edda-instance'] !== instanceId) throw fail('Wrong session instance', 409);
      if (req.method === 'GET' && req.url === '/status') return reply(200, channel.snapshot());
      if (req.method === 'POST' && req.url === '/inbox/respond') {
        if (req.headers['content-type'] !== 'application/json') throw fail('Expected application/json', 415);
        const body = await jsonBody(req), records = inboxStore(root);
        const event = records.read('events', inboxId(body.eventId));
        if (!event) throw fail('Inbox event not found', 404);
        if (validateId(body.id) !== messageId(body.eventId)) throw fail('Inbox response must use its event message identity', 409);
        assertCurrentEvent(event.data, channel.snapshot(), channel.handoffContext(32768), readEnrollment(root, sessionId));
        return reply(200, submitMessage(body, true));
      }
      if (req.method === 'GET' && req.url === '/dependencies') return reply(200, dependencies.status());
      if (req.method === 'POST' && req.url?.startsWith('/dependencies')) {
        if (closed || storageError) throw fail('Channel unavailable', 503);
        if (req.headers['content-type'] !== 'application/json') throw fail('Expected application/json', 415);
        const body = await jsonBody(req);
        if (req.url === '/dependencies') return reply(200, await dependencies.configure(body));
        if (req.url === '/dependencies/pause') return reply(200, await dependencies.pause());
        if (req.url === '/dependencies/check') { void dependencies.check(); return reply(200, dependencies.status()); }
        throw fail('Unknown dependency operation', 404);
      }
      if (req.method === 'GET' && req.url?.startsWith('/handoff?')) {
        const params = new URL(req.url, 'http://127.0.0.1').searchParams;
        return reply(200, channel.handoffContext(params.get('budget') || 16384));
      }
      if (req.method === 'POST' && req.url === '/handoff/manifest') {
        if (closed || storageError) throw fail('Channel unavailable', 503);
        if (req.headers['content-type'] !== 'application/json') throw fail('Expected application/json', 415);
        const body = await jsonBody(req);
        inbox.reconcile();
        handoff.prepare(body.manifest, body.expectedRevision, channel.snapshot());
        return reply(200, channel.handoffContext());
      }
      if (req.method === 'GET' && req.url?.startsWith('/conversation?') && getConversation) {
        const params = new URL(req.url, 'http://127.0.0.1').searchParams;
        return reply(200, { ...getConversation({ after: params.get('after') || undefined, limit: params.get('limit') || 20 }),
          sessionId, instanceId, source: 'pi_runtime', branchEvidence: 'current Pi branch' });
      }
      if (req.method === 'GET' && req.url?.startsWith('/receipts/')) {
        const receipt = receipts.get(validateId(req.url.slice('/receipts/'.length)));
        if (!receipt) throw fail('Receipt not found', 404);
        return reply(200, publicReceipt(receipt));
      }
      if (req.method !== 'POST' || req.url !== '/messages') throw fail('Unknown channel operation', 404);
      if (req.headers['content-type'] !== 'application/json') throw fail('Expected application/json', 415);
      return reply(200, submitMessage(await jsonBody(req)));
    } catch (error) {
      reply(error.status || 400, { error: error.status ? error.message : 'Invalid request or channel storage unavailable' });
    }
  });
  server.requestTimeout = 5000;
  server.headersTimeout = 5000;
  server.timeout = 5000;
  let heartbeat;
  try {
    try {
      inbox = createInboxProducer({ root, sessionId, instanceId, evidence: () => handoff.outboundEvidence(),
        context: () => channel.handoffContext(32768), policy: () => readEnrollment(root, sessionId) });
    } catch (error) {
      // Telemetry setup never disables the original channel or task execution.
      inbox = { status: () => ({ status: 'storage_error', error: error.message, wake: { status: 'unsupported', notified: false } }),
        begin() {}, assistant() {}, settled() {}, reconcile() {} };
    }
    dependencies = createDependencyObserver({ dir: store.dir, sessionId, instanceId,
      policy: () => readEnrollment(root, sessionId), runtime: () => channel.snapshot(),
      manifestRevision: () => handoff.currentRevision(), send: (body) => submitMessage(body, true),
      receipt: (id) => receipts.get(id), command: dependencyCommand, pollMs: dependencyPollMs });
    await new Promise((resolve, reject) => {
      server.once('error', reject);
      server.listen(0, '127.0.0.1', resolve);
    });
    owner.port = server.address().port;
    store.owner(owner);
    save();
    heartbeat = setInterval(() => {
      try { save(); inbox.reconcile(); }
      catch { storageError = true; recompute(); }
    }, heartbeatMs);
    heartbeat.unref();
    server.unref();
    return channel;
  } catch (error) {
    server.close();
    store.release();
    throw error;
  }
}

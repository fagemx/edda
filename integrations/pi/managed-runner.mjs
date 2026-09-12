import { spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { randomBytes, timingSafeEqual } from 'node:crypto';
import { existsSync, unlinkSync, realpathSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { managedDir, verifyRelease, rpcFrames, inside, findPiEntry } from './managed-store.mjs';
import { readJson, writeJson, validateId, digest } from './store.mjs';
import { requestSession, getReceipt } from './client.mjs';
import { messageId } from './inbox-store.mjs';

const [rootArg, runArg, serviceArg, mode] = process.argv.slice(2);
const root = resolve(rootArg), runId = validateId(runArg), serviceId = validateId(serviceArg);
const dir = managedDir(root, runId), config = readJson(join(dir, 'config.json'));
if (!config || config.version !== 1 || config.runId !== runId || config.root !== root) throw new Error('Invalid managed run configuration');
verifyRelease(config.release);
const prior = readJson(join(dir, 'state.json'));
if (prior?.serviceId !== serviceId || prior.phase !== 'launch_requested') throw new Error('Launch generation is no longer current');
const resume = mode === '--resume';
if (mode && !resume) throw new Error('Unknown runner mode');
const token = randomBytes(32).toString('hex');
let state = { ...prior, runId, serviceId, runnerPid: process.pid, phase: 'starting',
  release: config.release, piVersion: null, expectedPiVersion: config.pi.version, updatedAt: new Date().toISOString() };
let child, stopped = false, server, heartbeat, rpcError, observed, stateSequence = 0;
const save = () => { state.updatedAt = new Date().toISOString(); writeJson(join(dir, 'state.json'), state); };
const initialId = messageId(digest(`${runId}:initial`));
save();

async function childExit() {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  child.stdin.end();
  const deadline = Date.now() + 2000;
  while (child.exitCode === null && child.signalCode === null && Date.now() < deadline) await delay(50);
  if (child.exitCode === null && child.signalCode === null) {
    // This is the actual ChildProcess owned by this runner, never a saved PID.
    child.kill();
    const killedDeadline = Date.now() + 5000;
    while (child.exitCode === null && child.signalCode === null && Date.now() < killedDeadline) await delay(50);
    if (child.exitCode === null && child.signalCode === null) throw new Error('Owned Pi did not exit; preserve run for inspection');
  }
}
async function cleanup() {
  clearInterval(heartbeat);
  await childExit();
  const ownerPath = join(dir, 'owner.json'), owner = readJson(ownerPath);
  if (owner?.serviceId === serviceId) unlinkSync(ownerPath);
  if (server?.listening) await new Promise((resolveClose) => server.close(resolveClose));
}
async function snapshot() {
  let piState = null, receipt = state.initialReceipt || null;
  if (state.sessionId) {
    try { piState = await requestSession(root, state.sessionId, '/status', undefined, 2000, state.instanceId); } catch { /* stale/offline stays visible */ }
    if (state.initialAttemptedAt) {
      try { receipt = await getReceipt(root, state.sessionId, initialId); } catch { receipt = { id: initialId, status: 'unknown' }; }
    }
  }
  return { ...state, live: true, status: state.phase, pi: piState,
    sessionPersisted: Boolean(state.sessionFile && existsSync(state.sessionFile)), initialReceipt: receipt,
    notice: 'Runner readiness is not work completion. Resume never replays the initial prompt; inbox delivery does not imply a manager decision.' };
}

try {
  state.piVersion = findPiEntry(config.pi.entry).version;
  save();
  const env = { ...process.env, EDDA_PI_CHANNEL_DIR: root, EDDA_PI_RELEASE_ID: config.release.id,
    EDDA_SESSION_ID: runId, EDDA_SESSION_LABEL: `managed-pi-${runId.slice(0, 8)}` };
  delete env.EDDA_PROJECT_ID;
  if (config.agentDir) env.PI_CODING_AGENT_DIR = config.agentDir;
  const model = resume && prior.model ? prior.model : { provider: config.provider, id: config.model };
  const args = [config.pi.entry, '--mode', 'rpc', '--no-extensions', '-e', join(config.release.path, 'extension.mjs'),
    '--session-dir', join(dir, 'sessions'), ...(resume ? ['--session', prior.sessionFile] : []),
    ...(model.provider ? ['--provider', model.provider] : []), ...(model.id ? ['--model', model.id] : []),
    ...((resume ? prior.thinkingLevel : config.thinking) ? ['--thinking', resume ? prior.thinkingLevel : config.thinking] : []),
    ...(config.noTools ? ['--no-tools'] : []), ...config.extensions.flatMap((path) => ['-e', path])];
  if (config.noSkills) args.push('--no-skills', '--no-prompt-templates');
  if (config.tools) args.push('--tools', config.tools.join(','));
  child = spawn(process.execPath, args, { cwd: config.project, env, windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] });
  state.childPid = child.pid; save();
  child.on('error', (error) => { rpcError = error; });
  child.stdin.on('error', (error) => { rpcError = error; });
  child.stderr.on('data', () => { state.stderrObserved = true; });
  child.stdout.on('data', rpcFrames((event) => {
    if (event.type === 'response' && event.command === 'get_state') {
      if (!event.success || !event.data?.sessionId || !event.data?.sessionFile) throw new Error('Pi did not supply persisted session identity');
      observed = event.data;
      if (state.sessionId && observed.sessionId !== state.sessionId) throw new Error('Managed Pi changed session identity');
      if (!inside(join(dir, 'sessions'), observed.sessionFile)) throw new Error('Pi session file is outside owned storage');
      state.sessionId = observed.sessionId; state.sessionFile = observed.sessionFile;
      state.model = observed.model ? { provider: observed.model.provider, id: observed.model.id } : null;
      state.thinkingLevel = observed.thinkingLevel || null;
    }
    if (['agent_start', 'agent_settled', 'tool_execution_start', 'tool_execution_end', 'extension_error'].includes(event.type)) {
      state.lastEvent = event.type; state.lastProgressAt = new Date().toISOString();
    }
    if (event.type === 'message_end' && event.message?.role === 'assistant' && event.message.usage) {
      // Aggregate observable accounting only; no raw messages or reasoning logs.
      const usage = event.message.usage;
      state.usage = { tokens: (state.usage?.tokens || 0) + (Number(usage.totalTokens) || 0),
        reportedCost: (state.usage?.reportedCost || 0) + (Number(usage.cost?.total) || 0) };
    }
    if (event.type === 'message_end' && event.message?.role === 'assistant') {
      state.lastModelStopReason = event.message.stopReason;
      const error = event.message.errorMessage;
      if (typeof error === 'string' && error) {
        state.modelError = { category: /credit|balance|funds/i.test(error) ? 'credit_or_quota' :
          /auth|api.?key|401|403/i.test(error) ? 'authentication' : /timeout|timed.out/i.test(error) ? 'timeout' : 'provider_error',
          httpStatus: error.match(/\b[45][0-9]{2}\b/)?.[0] || null };
      }
    }
  }, (error) => { rpcError = error; }));
  const queryState = () => child.stdin.write(JSON.stringify({ type: 'get_state', id: `managed-state-${++stateSequence}` }) + '\n');
  queryState();
  const deadline = Date.now() + 25000;
  let piState;
  while (Date.now() < deadline) {
    if (rpcError) throw rpcError;
    if (child.exitCode !== null || child.signalCode !== null) throw new Error('Pi exited during startup; inspect installation/provider configuration');
    if (observed) {
      try { piState = await requestSession(root, observed.sessionId, '/status'); } catch { /* registration in progress */ }
      if (piState?.live) break;
    }
    await delay(100);
  }
  if (!piState?.live || !['inbox', 'handoff', 'receipts'].every((cap) => piState.capabilities?.includes(cap)) ||
    piState.integration?.releaseId !== config.release.id || piState.integration?.version !== config.release.version ||
    realpathSync(piState.integration.modulePath) !== realpathSync(join(config.release.path, 'channel.mjs'))) {
    throw new Error('Pi registration/version handshake failed; no initial task was sent');
  }
  state.instanceId = piState.instanceId; state.integration = piState.integration; state.phase = 'ready'; save();
  server = createServer(async (req, res) => {
    const reply = (code, body) => { res.writeHead(code, { 'content-type': 'application/json', 'cache-control': 'no-store' }); res.end(JSON.stringify(body)); };
    try {
      const supplied = req.headers.authorization || '', expected = `Bearer ${token}`;
      if (req.headers.origin || Buffer.byteLength(supplied) !== Buffer.byteLength(expected) ||
        !timingSafeEqual(Buffer.from(supplied), Buffer.from(expected)) || req.headers['x-edda-instance'] !== serviceId) throw new Error('Unauthorized managed request');
      if (req.method === 'GET' && req.url === '/status') return reply(200, await snapshot());
      if (req.method !== 'POST' || req.url !== '/stop') return reply(404, { error: 'Unknown managed operation' });
      if (stopped) throw new Error('Stop already in progress');
      const current = await requestSession(root, state.sessionId, '/status', undefined, 2000, state.instanceId);
      const abort = req.headers['x-edda-abort'] === 'true';
      if (current.state !== 'idle' && !abort) throw new Error('Pi is working; idle stop refused without interrupting it. Use explicit --abort to stop owned work.');
      stopped = true; state.phase = 'stopping'; save();
      await childExit();
      state.phase = 'stopped'; state.stopMode = abort ? 'explicit_abort' : 'idle'; save();
      reply(200, { runId, serviceId, status: 'stopped', sessionId: state.sessionId, sessionFile: state.sessionFile, interrupted: abort, initialReplayed: false });
    } catch (error) { reply(409, { error: error.message }); }
  });
  server.requestTimeout = 5000; server.headersTimeout = 5000;
  await new Promise((resolveListen, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolveListen); });
  writeJson(join(dir, 'owner.json'), { runId, serviceId, pid: process.pid, port: server.address().port, token });
  if (!resume && config.prompt !== null && !state.initialAttemptedAt) {
    state.initialAttemptedAt = new Date().toISOString(); state.initialReceipt = { id: initialId, status: 'unknown' }; save();
    try { state.initialReceipt = await requestSession(root, state.sessionId, '/messages', { id: initialId, message: config.prompt, sender: 'managed-launch', mode: 'followUp' }, 2500, state.instanceId); }
    catch { /* intent remains unknown; never replay */ }
    save();
  }
  heartbeat = setInterval(() => { if (!stopped) { queryState(); save(); } }, 2000);
  while (!stopped && child.exitCode === null && child.signalCode === null && !rpcError) await delay(200);
  if (!stopped) { state.phase = rpcError ? 'failed' : 'exited'; state.error = rpcError?.message || 'Pi process exited'; save(); }
} catch (error) {
  state.phase = 'failed'; state.error = error.message; save();
} finally { await cleanup(); }

import { createServer } from 'node:http';
import { randomBytes, timingSafeEqual } from 'node:crypto';
import { openSync, closeSync, unlinkSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { validateId, readJson, writeJson } from './store.mjs';
import { loadSupervisor, pauseToken } from './supervisor-store.mjs';
import { createSupervisorEngine } from './supervisor-engine.mjs';

const [rootArg, idArg, generation] = process.argv.slice(2);
const root = resolve(rootArg), id = validateId(idArg), serviceId = validateId(generation);
const { dir, config } = loadSupervisor(root, id), lifecycle = join(dir, 'lifecycle.json');
const prior = readJson(lifecycle);
if (prior?.serviceId !== serviceId || prior.phase !== 'launch_requested') throw new Error('Supervisor launch generation changed');
const lock = join(dir, 'service.lock'), fd = openSync(lock, 'wx', 0o600);
writeFileSync(fd, JSON.stringify({ id, serviceId, pid: process.pid }));
const token = randomBytes(32).toString('hex');
let closing = false, engine, server;
const active = () => !closing && pauseToken(dir) === prior.expectedPauseToken;
const life = (phase, error) => writeJson(lifecycle, { ...prior, id, serviceId, pid: process.pid, phase, error: error || null, updatedAt: new Date().toISOString() });
function summary() {
  const state = engine.snapshot();
  return { supervisorId: id, serviceId, pid: process.pid, live: true, phase: active() ? state.phase : 'pausing',
    attention: state.attention || null, tasks: state.tasks?.map(({ id, status, assignee }) => ({ id, status, assignee })),
    workers: state.workers, managerRunId: state.managerRunId || null, pending: state.pending,
    decisions: state.decisions, responses: state.responses,
    recentCases: Object.entries(state.cases).slice(-5).map(([packetId, value]) => ({ packetId, taskId: value.taskId,
      status: value.status, action: value.proposal?.action, managerReceipt: value.receipt?.status, workerReceipt: value.workerReceipt?.status,
      reason: value.proposal?.reason || value.error || null })),
    acceptance: 'unverified', operatorNotified: false, updatedAt: state.updatedAt };
}
try {
  life('starting');
  engine = createSupervisorEngine(root, id, active);
  server = createServer((req, res) => {
    const supplied = req.headers.authorization || '', expected = `Bearer ${token}`;
    const valid = !req.headers.origin && req.headers['x-edda-instance'] === serviceId && Buffer.byteLength(supplied) === Buffer.byteLength(expected) && timingSafeEqual(Buffer.from(supplied), Buffer.from(expected));
    const ok = valid && req.method === 'GET' && req.url === '/status';
    res.writeHead(ok ? 200 : 403, { 'content-type': 'application/json', 'cache-control': 'no-store' });
    res.end(JSON.stringify(ok ? summary() : { error: 'Unauthorized or unknown supervisor operation' }));
  });
  server.requestTimeout = 5000; server.headersTimeout = 5000;
  await new Promise((resolveListen, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolveListen); });
  writeJson(join(dir, 'owner.json'), { id, serviceId, pid: process.pid, port: server.address().port, token });
  life('running');
  while (active()) {
    await engine.tick();
    if (engine.snapshot().phase === 'tasks_done') break;
    if (active()) await delay(config.pollMs);
  }
  life(engine.snapshot().phase === 'tasks_done' ? 'completed' : 'paused');
} catch (error) { life('failed', error.message); }
finally {
  closing = true;
  if (server?.listening) await new Promise((resolveClose) => server.close(resolveClose));
  const owner = readJson(join(dir, 'owner.json'));
  if (owner?.serviceId === serviceId) unlinkSync(join(dir, 'owner.json'));
  closeSync(fd); unlinkSync(lock);
}

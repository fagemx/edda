import test from 'node:test';
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { spawnSync, spawn } from 'node:child_process';
import { AgentManager } from '../src/manager.js';
import { ManagerStore } from '../src/store.js';
import { parseConfig, selectionRevision, hash } from '../src/config.js';
import { EddaWorkflowLedger, WorkflowLocks, eddaRunner, type WorkflowLedger, type LedgerNote } from '../src/edda-workflow.js';
import type { WorkAction, WorkBinding } from '../src/workflow-contracts.js';
import type { PiAdapter, SendRequest } from '../src/contracts.js';
import { parseWorkAction } from '../src/workflow.js';

function fixture() {
  const root = mkdtempSync(join(tmpdir(), 'manager-work-integration-')), instanceId = randomUUID(), taskKey = randomUUID();
  const config = parseConfig({ version: 1, projects: [{ id: 'p', name: 'Project' }], agents: [
    { id: 'a', name: 'Agent', projectId: 'p', role: 'manager', registryRoot: root, workspace: root, sessionId: 'original' }],
    works: [1, 2].map((taskId) => ({ id: `w${taskId}`, projectId: 'p', taskId, workspace: root, ownerAgentId: 'a' })) });
  const rows = new Map<number, LedgerNote[]>(); let loseAppend = false, effects = 0;
  const ledger: WorkflowLedger = {
    task: async (b) => ({ id: b.taskId, key: `${taskKey}-${b.taskId}`, title: 'Task', status: 'running', receipt: null, updatedAt: 'now' }),
    notes: async (b) => structuredClone(rows.get(b.taskId) ?? []),
    append: async (b, text) => { const list = rows.get(b.taskId) ?? []; list.push({ id: randomUUID(), at: 'now', text }); rows.set(b.taskId, list);
      if (loseAppend) { loseAppend = false; throw new Error('append result lost'); } },
  };
  const adapter: PiAdapter = { observe: async () => ({ state: 'idle', instanceId, observedAt: 'now', heartbeatAt: null, lastProgressAt: null, lastEvent: null,
    source: 'live', stale: false, reason: null, degraded: null, model: null, usage: null, capabilities: { send: true, conversation: true }, latestMessage: null, ownerMailbox: null }),
    conversation: async () => { throw new Error('unused'); }, send: async (b, r) => { effects++; return { id: r.operationId, instanceId, sessionId: b.sessionId, status: 'settled' }; }, receipt: async () => null };
  let store = new ManagerStore(root), manager = new AgentManager(config, store, adapter, { ledger, locks: new WorkflowLocks(join(root, 'locks')) });
  const send = (operationId = randomUUID()): SendRequest => ({ operationId, selectionRevision: selectionRevision(config.agents[0]!), instanceId, basisCursor: null, mode: 'followUp', message: 'Review exact SHA' });
  const initialize = async (id: string) => {
    const v = (await manager.works.list()).works.find((w) => w.id === id)!;
    return manager.works.act(id, { kind: 'initialize', actionId: randomUUID(), revision: v.revision, nextStep: 'Assign' });
  };
  return { root, config, rows, ledger, adapter, send, initialize, get manager() { return manager; }, get effects() { return effects; }, lose() { loseAppend = true; },
    restart(mode?: 'target' | 'store') {
      store.close(); store = new ManagerStore(mode === 'store' ? join(root, 'new-store') : root);
      manager = new AgentManager(mode === 'target' ? { ...config, agents: [{ ...config.agents[0]!, sessionId: 'replacement' }] } : config, store, adapter, { ledger, locks: new WorkflowLocks(join(root, 'locks')) });
    }, async close() { await manager.stop(); store.close(); rmSync(root, { recursive: true, force: true }); } };
}
test('lost pre-effect intent never recovers onto a changed target or replacement transport store', async () => {
  for (const mode of ['target', 'store'] as const) {
    const f = fixture();
    try {
      const v = await f.initialize('w1'), a: WorkAction = { kind: 'assign', actionId: randomUUID(), revision: v.revision, agentId: 'a', nextStep: 'Review', send: f.send() };
      f.lose(); await assert.rejects(() => f.manager.works.act('w1', a), /lost/); f.restart(mode);
      assert.equal((await f.manager.works.list()).works[0]!.deliveryStatus, 'unknown');
      if (mode === 'target') await assert.rejects(() => f.manager.works.act('w1', a), /綁定已變更/);
      else assert.equal((await f.manager.works.act('w1', a)).deliveryStatus, 'unknown');
      assert.equal(f.effects, 0);
    } finally { await f.close(); }
  }
});
test('concurrent different works cannot claim the same transport ID or project another receipt', async () => {
  const f = fixture();
  try {
    const first = await f.initialize('w1'), second = await f.initialize('w2'), op = randomUUID();
    const a: WorkAction = { kind: 'assign', actionId: randomUUID(), revision: first.revision, agentId: 'a', nextStep: 'One', send: f.send(op) };
    const b: WorkAction = { kind: 'assign', actionId: randomUUID(), revision: second.revision, agentId: 'a', nextStep: 'Two', send: { ...f.send(op), message: 'Different work' } };
    const result = await Promise.allSettled([f.manager.works.act('w1', a), f.manager.works.act('w2', b)]);
    assert.equal(result.filter((r) => r.status === 'fulfilled').length, 1); assert.equal(f.effects, 1);
    f.restart(); const views = (await f.manager.works.list()).works;
    assert.equal(views.filter((v) => v.stage === 'awaiting_delivery').length, 1); assert.equal(views.filter((v) => v.deliveryOperationId === null).length, 1);
    // An external writer ignoring locks cannot make a different request adopt
    // that existing operation's receipt, even with an otherwise valid chain.
    const winner = result[0]!.status === 'fulfilled' ? 1 : 2, loser = winner === 1 ? 2 : 1;
    const assigned = JSON.parse(f.rows.get(winner)![1]!.text.slice('edda.manager-work.v1 '.length)) as Record<string, unknown>;
    const losingAction = winner === 1 ? b : a;
    assigned.taskKey = (await f.ledger.task(f.config.works![loser - 1]!)).key; assigned.previous = f.rows.get(loser)![0]!.id;
    assigned.action = parseWorkAction(losingAction); assigned.fingerprint = hash(JSON.stringify(assigned.action));
    f.rows.get(loser)!.push({ id: randomUUID(), at: 'now', text: 'edda.manager-work.v1 ' + JSON.stringify(assigned) });
    f.restart(); const bad = (await f.manager.works.list()).works[loser - 1]!;
    assert.match(bad.error!, /身分不一致/); assert.equal(bad.deliveryStatus, null);
  } finally { await f.close(); }
});
test('invalid external ledger transitions and target shapes fail closed', async () => {
  const f = fixture();
  try {
    await f.initialize('w1');
    const first = f.rows.get(1)![0]!, raw = JSON.parse(first.text.slice('edda.manager-work.v1 '.length)) as Record<string, unknown>;
    const invalid = { kind: 'accept', actionId: randomUUID(), revision: 'invalid', evidence: 'Invented' };
    raw.action = parseWorkAction(invalid); raw.fingerprint = hash(JSON.stringify(raw.action));
    first.text = 'edda.manager-work.v1 ' + JSON.stringify(raw); f.restart();
    assert.match((await f.manager.works.list()).works[0]!.error!, /不合法/);
  } finally { await f.close(); }
});
test('work snapshot returns within bounded time while a source is pending', async () => {
  const f = fixture(); let release!: () => void;
  const original = f.ledger.task; const gate = new Promise<void>((r) => { release = r; });
  f.ledger.task = async (b) => { if (b.taskId === 1) await gate; return original(b); };
  try {
    const started = Date.now(), snapshot = await f.manager.works.list();
    assert.ok(Date.now() - started < 2500); assert.match(snapshot.works[0]!.error!, /正在取得/); assert.equal(snapshot.works[1]!.error, null);
    release();
  } finally { release(); await f.close(); }
});
test('real temporary Edda ledger handles empty filtered log and preserves literal note arguments', async (t) => {
  const executable = process.platform === 'win32' ? 'edda.exe' : 'edda';
  if (spawnSync(executable, ['--version'], { windowsHide: true }).status !== 0) { t.skip('Installed Edda unavailable'); return; }
  const root = mkdtempSync(join(tmpdir(), 'manager-edda-ledger-')), run = eddaRunner(executable);
  try {
    await run(root, ['init', '--no-hooks']); await run(root, ['task', 'new', 'Real fixture']);
    const ledger = new EddaWorkflowLedger(run), b: WorkBinding = { id: 'w', projectId: 'p', taskId: 1, workspace: root, ownerAgentId: 'a' };
    assert.equal((await ledger.task(b)).status, 'ready'); assert.deepEqual(await ledger.notes(b), []);
    const marker = 'edda.manager-work.v1 ' + JSON.stringify({ text: '中文 "quote"; $(echo secret) --flag\nnew line' });
    await ledger.append(b, marker); assert.equal((await ledger.notes(b))[0]!.text, marker); assert.equal((await ledger.task(b)).status, 'ready');
  } finally { rmSync(root, { recursive: true, force: true }); }
});
test('task mutex is shared across processes and released on process death', async () => {
  const root = mkdtempSync(join(tmpdir(), 'manager-work-lock-')), module = new URL('../src/edda-workflow.js', import.meta.url).href;
  const script = `import { WorkflowLocks } from ${JSON.stringify(module)}; await new WorkflowLocks(process.argv[1]).run('same-task', async () => { console.log('locked'); await new Promise(() => setInterval(() => {}, 1000)); });`;
  const child = spawn(process.execPath, ['--input-type=module', '-e', script, root], { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
  try {
    await new Promise<void>((resolve, reject) => { const timer = setTimeout(() => reject(new Error('lock child timeout')), 10000); child.once('error', reject); child.stdout.once('data', () => { clearTimeout(timer); resolve(); }); });
    await assert.rejects(() => new WorkflowLocks(root).run('same-task', async () => 'not allowed'), /另一個管理者/);
    const exited = new Promise<void>((resolve) => child.once('exit', () => resolve())); child.kill(); await exited;
    assert.equal(await new WorkflowLocks(root).run('same-task', async () => 'released'), 'released');
  } finally {
    if (child.exitCode === null && child.signalCode === null) { const exited = new Promise<void>((resolve) => child.once('exit', () => resolve())); child.kill(); await exited; }
    rmSync(root, { recursive: true, force: true });
  }
});

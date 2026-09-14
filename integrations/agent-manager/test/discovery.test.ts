import test from 'node:test';
import assert from 'node:assert/strict';
import { randomBytes, randomUUID } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { AgentManager } from '../src/manager.js';
import { ManagerStore } from '../src/store.js';
import { parseConfig, parseRegisterCandidate } from '../src/config.js';
import { candidateId, dedupeRuns, projectCandidates, rootLabel } from '../src/discovery.js';
import { ChannelAdapter, defaultPiRoot } from '../src/pi-adapter.js';
import { serve } from '../src/http.js';
import { type AgentObservation, type DiscoveryReport, type DiscoveredRun, type PiAdapter, type RecordDegradation } from '../src/contracts.js';

interface Channel { snapshot(): { instanceId: string }; close(): Promise<void> }
interface ChannelModule { startChannel(options: { root: string; sessionId: string; cwd: string; deliver: () => void; getConversation: () => unknown }): Promise<Channel> }

const running = (): AgentObservation => ({ state: 'idle', instanceId: randomUUID(), observedAt: new Date().toISOString(), heartbeatAt: null, lastProgressAt: null, lastEvent: null,
  source: 'live', stale: false, reason: null, degraded: null, model: null, usage: null, capabilities: { conversation: false, send: false }, latestMessage: null, ownerMailbox: null });

function fixture() {
  const root = mkdtempSync(join(tmpdir(), 'manager-discovery-')), workspace = mkdtempSync(join(tmpdir(), 'manager-ws-')), broken = join(root, 'broken');
  const config = parseConfig({ version: 1, projects: [{ id: 'p', name: 'Project' }], agents: [
    { id: 'known', name: 'Known', role: 'worker', projectId: 'p', registryRoot: root, workspace, sessionId: 'known-session' }] });
  const runs: DiscoveredRun[] = [
    { registryRoot: root, sessionId: 'known-session', runId: null, instanceId: randomUUID(), state: 'running', live: true, source: 'live', workspace, lastProgressAt: null, reason: null },
    { registryRoot: root, sessionId: 'new-session', runId: null, instanceId: null, state: 'stopped', live: false, source: 'recorded', workspace, lastProgressAt: null, reason: 'stopped' },
    { registryRoot: root, sessionId: 'new-session', runId: null, instanceId: randomUUID(), state: 'idle', live: true, source: 'live', workspace, lastProgressAt: '2026-01-01T00:00:00.000Z', reason: null },
  ];
  const report: DiscoveryReport = { runs, failures: [{ registryRoot: broken, message: '此來源的 session 清單目前無法讀取；其他來源不受影響。' }] };
  const calls = { discover: [] as string[][], observe: 0, send: 0 };
  const adapter: PiAdapter = {
    discover: async (roots) => { calls.discover.push([...roots]); return report; },
    observe: async () => { calls.observe++; return running(); },
    conversation: async () => { throw new Error('unused'); },
    send: async () => { calls.send++; throw new Error('must not send'); },
    receipt: async () => null,
  };
  const store = new ManagerStore(root), manager = new AgentManager(config, store, adapter);
  return { root, workspace, broken, config, report, calls, adapter, store, manager };
}

test('registering a run whose managed record pins an owner mailbox keeps that root internal', async () => {
  const f = fixture();
  try {
    f.adapter.observe = async () => ({ ...running(), ownerMailbox: { ref: 'assistant/x', root: f.root } });
    const id = candidateId({ registryRoot: f.root, sessionId: 'new-session' });
    const view = await f.manager.registerCandidate({ candidateId: id, id: 'mailbox-run', name: 'Mailbox run', role: 'worker', projectId: 'p' });
    // The response is a browser projection; only the internal view keeps the path.
    assert.equal(view.ownerMailbox?.root, null);
    assert.equal(f.manager.agentViews().find((a) => a.id === 'mailbox-run')?.ownerMailbox?.root, f.root);
    assert.ok(!JSON.stringify(view).includes(f.root));
  } finally { await f.manager.stop(); f.store.close(); }
});

test('candidate projection deduplicates by session, marks configured runs and never leaks the registry root', () => {
  const root = join(tmpdir(), 'manager-projection-registry'), workspace = join(tmpdir(), 'manager-projection-ws'), other = join(tmpdir(), 'manager-projection-other');
  const agents = parseConfig({ version: 1, projects: [{ id: 'p', name: 'P' }], agents: [
    { id: 'known', name: 'Known', role: 'worker', projectId: 'p', registryRoot: root, workspace, sessionId: 's1' }] }).agents;
  const report: DiscoveryReport = { runs: [
    { registryRoot: root, sessionId: 's1', runId: null, instanceId: null, state: 'idle', live: false, source: 'recorded', workspace, lastProgressAt: null, reason: 'offline' },
    { registryRoot: root, sessionId: 's2', runId: null, instanceId: null, state: 'stopped', live: false, source: 'recorded', workspace, lastProgressAt: null, reason: 'stopped' },
    { registryRoot: root, sessionId: 's2', runId: null, instanceId: 'i', state: 'running', live: true, source: 'live', workspace, lastProgressAt: null, reason: null },
  ], failures: [{ registryRoot: other, message: 'unreadable' }] };
  const view = projectCandidates(report, agents);
  assert.equal(view.candidates.length, 2);
  assert.deepEqual(view.candidates.map((c) => [c.sessionId, c.live, c.configuredAgentId]), [['s2', true, null], ['s1', false, 'known']]);
  assert.deepEqual(view.issues, [{ label: rootLabel(other), message: 'unreadable' }]);
  assert.ok(!JSON.stringify(view).includes(root));
  assert.ok(!JSON.stringify(view).includes(basename(root)));
  assert.ok(!JSON.stringify(view).includes('registryRoot'));
});

test('opaque candidate ids keep the session and run namespaces distinct', () => {
  const root = join(tmpdir(), 'manager-id-namespace');
  assert.notEqual(candidateId({ registryRoot: root, sessionId: 'shared-id' }), candidateId({ registryRoot: root, runId: 'shared-id' }));
  assert.equal(candidateId({ registryRoot: root, sessionId: 'shared-id' }), candidateId({ registryRoot: root, sessionId: 'shared-id', runId: 'other' }));
});

test('the same session in two registry roots stays two root-scoped candidates', () => {
  const first = join(tmpdir(), 'manager-dedupe-first'), second = join(tmpdir(), 'manager-dedupe-second'), workspace = join(tmpdir(), 'manager-dedupe-ws');
  const report: DiscoveryReport = { runs: [
    { registryRoot: first, sessionId: 's', runId: null, instanceId: null, state: 'idle', live: false, source: 'recorded', workspace, lastProgressAt: null, reason: 'offline' },
    { registryRoot: second, sessionId: 's', runId: null, instanceId: null, state: 'idle', live: false, source: 'recorded', workspace, lastProgressAt: null, reason: 'offline' },
  ], failures: [] };
  const view = projectCandidates(report, []);
  assert.equal(view.candidates.length, 2);
  assert.deepEqual(view.candidates.map((candidate) => candidate.id).sort(), [
    candidateId({ registryRoot: first, sessionId: 's' }), candidateId({ registryRoot: second, sessionId: 's' })].sort());
});

test('candidate identity matches configuration on (registry root, session), not session id alone', async () => {
  const root = mkdtempSync(join(tmpdir(), 'manager-identity-root-')), other = mkdtempSync(join(tmpdir(), 'manager-identity-other-')), workspace = mkdtempSync(join(tmpdir(), 'manager-identity-ws-'));
  const config = parseConfig({ version: 1, projects: [{ id: 'p', name: 'P' }], agents: [
    { id: 'known', name: 'Known', role: 'worker', projectId: 'p', registryRoot: root, workspace, sessionId: 'shared' }] });
  try {
    // A same-id run under another configured root is not the configured agent.
    const elsewhere: DiscoveryReport = { runs: [{ registryRoot: other, sessionId: 'shared', runId: null, instanceId: null,
      state: 'idle', live: false, source: 'recorded', workspace, lastProgressAt: null, reason: 'offline' }], failures: [] };
    assert.equal(projectCandidates(elsewhere, config.agents).candidates[0]?.configuredAgentId, null);
    const matching: DiscoveryReport = { runs: [{ registryRoot: root, sessionId: 'shared', runId: null, instanceId: null,
      state: 'idle', live: false, source: 'recorded', workspace, lastProgressAt: null, reason: 'offline' }], failures: [] };
    assert.equal(projectCandidates(matching, config.agents).candidates[0]?.configuredAgentId, 'known');
    // The manager's registration refusal uses the same identity, so the run in the
    // other root can still be explicitly added as a second (registryRoot, sessionId).
    const adapter: PiAdapter = { discover: async () => elsewhere, observe: async () => running(),
      conversation: async () => { throw new Error('unused'); }, send: async () => { throw new Error('unused'); }, receipt: async () => null };
    const store = new ManagerStore(root), manager = new AgentManager(config, store, adapter);
    try {
      const view = await manager.registerCandidate({ candidateId: candidateId({ registryRoot: other, sessionId: 'shared' }), id: 'second', name: 'Second', role: 'worker', projectId: 'p' });
      assert.equal(view.id, 'second');
      assert.equal(manager.binding('second').registryRoot, other);
    } finally { await manager.stop(); store.close(); }
  } finally { rmSync(root, { recursive: true, force: true }); rmSync(other, { recursive: true, force: true }); rmSync(workspace, { recursive: true, force: true }); }
});

test('manager discovery is read-only, bounded to configured roots and isolates failures', async () => {
  const f = fixture();
  try {
    const view = await f.manager.candidates();
    assert.equal(f.calls.discover.length, 1);
    assert.deepEqual(f.calls.discover[0], [f.root]);
    assert.equal(f.calls.observe, 0); assert.equal(f.calls.send, 0);
    assert.equal(view.candidates.find((c) => c.sessionId === 'known-session')?.configuredAgentId, 'known');
    assert.equal(view.candidates.find((c) => c.sessionId === 'new-session')?.configuredAgentId, null);
    assert.equal(view.issues[0]?.label, rootLabel(f.broken));
    assert.ok(!JSON.stringify(view).includes('registryRoot'));
    assert.ok(!JSON.stringify(view).includes(f.root));
  } finally { await f.manager.stop(); f.store.close(); rmSync(f.root, { recursive: true, force: true }); rmSync(f.workspace, { recursive: true, force: true }); }
});

test('explicit registration adds the run to the running process without a send or config write', async () => {
  const f = fixture();
  try {
    const id = candidateId({ registryRoot: f.root, sessionId: 'new-session' });
    const view = await f.manager.registerCandidate({ candidateId: id, id: 'added', name: 'Added worker', role: 'worker', projectId: 'p' });
    assert.equal(view.id, 'added'); assert.equal(view.workspace, f.workspace);
    assert.equal(f.manager.binding('added').sessionId, 'new-session');
    assert.equal(f.manager.binding('added').registryRoot, f.root);
    assert.equal(f.calls.observe, 1); assert.equal(f.calls.send, 0);
    assert.ok(!JSON.stringify(view).includes('registryRoot'));
    assert.ok(!JSON.stringify(view).includes(f.root));
    const after = await f.manager.candidates();
    assert.equal(after.candidates.find((c) => c.sessionId === 'new-session')?.configuredAgentId, 'added');
    await assert.rejects(() => f.manager.registerCandidate({ candidateId: id, id: 'again', name: 'Again', role: 'worker', projectId: 'p' }), /已在管理清單/);
  } finally { await f.manager.stop(); f.store.close(); rmSync(f.root, { recursive: true, force: true }); rmSync(f.workspace, { recursive: true, force: true }); }
});

test('registration rejects an unknown candidate, a duplicate id, a wrong project and an invalid role', async () => {
  const f = fixture();
  try {
    await assert.rejects(() => f.manager.registerCandidate({ candidateId: 'f'.repeat(24), id: 'x', name: 'X', role: 'worker', projectId: 'p' }), /找不到這個候選/);
    await assert.rejects(() => f.manager.registerCandidate({ candidateId: candidateId({ registryRoot: f.root, sessionId: 'new-session' }), id: 'known', name: 'X', role: 'worker', projectId: 'p' }), /識別碼已存在/);
    await assert.rejects(() => f.manager.registerCandidate({ candidateId: candidateId({ registryRoot: f.root, sessionId: 'new-session' }), id: 'x', name: 'X', role: 'worker', projectId: 'missing' }), /專案未登記/);
    assert.throws(() => parseRegisterCandidate({ candidateId: 'f'.repeat(24), id: 'x', name: 'X', role: 'owner', projectId: 'p' }), /角色不正確/);
    assert.throws(() => parseRegisterCandidate({ candidateId: 'not-hex', id: 'x', name: 'X', role: 'worker', projectId: 'p' }), /識別碼格式/);
    assert.equal(f.calls.send, 0);
  } finally { await f.manager.stop(); f.store.close(); rmSync(f.root, { recursive: true, force: true }); rmSync(f.workspace, { recursive: true, force: true }); }
});

test('candidate HTTP endpoints require auth and keep registryRoot out of the response', async () => {
  const f = fixture(), token = randomBytes(32).toString('hex');
  const gateway = await serve(f.manager, token); const auth = { authorization: `Bearer ${token}` };
  try {
    assert.equal((await fetch(`${gateway.origin}/api/candidates`)).status, 401);
    const listResponse = await fetch(`${gateway.origin}/api/candidates`, { headers: auth });
    assert.equal(listResponse.status, 200);
    const list = await listResponse.text();
    assert.ok(!list.includes('registryRoot')); assert.ok(!list.includes(f.root));
    const id = candidateId({ registryRoot: f.root, sessionId: 'new-session' });
    const register = await fetch(`${gateway.origin}/api/candidates/register`, { method: 'POST',
      headers: { ...auth, 'content-type': 'application/json' }, body: JSON.stringify({ candidateId: id, id: 'added', name: 'Added', role: 'worker', projectId: 'p' }) });
    assert.equal(register.status, 200);
    const body = await register.text();
    assert.ok(!body.includes('registryRoot')); assert.ok(!body.includes(f.root));
    assert.equal((await fetch(`${gateway.origin}/api/candidates/register`, { method: 'POST',
      headers: { ...auth, 'content-type': 'application/json' }, body: JSON.stringify({ candidateId: 'x', id: 'y', name: 'Y', role: 'worker', projectId: 'p' }) })).status, 400);
  } finally { await f.manager.stop(); await gateway.close(); f.store.close(); rmSync(f.root, { recursive: true, force: true }); rmSync(f.workspace, { recursive: true, force: true }); }
});

test('real Pi session listing discovers live and offline runs read-only', async () => {
  const api = await import(pathToFileURL(join(defaultPiRoot(), 'channel.mjs')).href) as ChannelModule;
  const base = mkdtempSync(join(tmpdir(), 'manager-discovery-pi-')), registry = join(base, 'registry'), workspace = join(base, 'project');
  mkdirSync(registry); mkdirSync(workspace);
  const adapter = await ChannelAdapter.create();
  let first: Channel | undefined, second: Channel | undefined;
  try {
    first = await api.startChannel({ root: registry, sessionId: 'live-session', cwd: workspace, deliver: () => {}, getConversation: () => ({ entries: [], cursor: null, headCursor: null, hasMore: false }) });
    second = await api.startChannel({ root: registry, sessionId: 'other-session', cwd: workspace, deliver: () => {}, getConversation: () => ({ entries: [], cursor: null, headCursor: null, hasMore: false }) });
    const sessionDir = readdirSync(registry).filter((name) => /^[0-9a-f]{64}$/.test(name));
    const owners = sessionDir.map((name) => readFileSync(join(registry, name, 'owner.json'), 'utf8')).sort();
    const report = await adapter.discover([registry]);
    assert.deepEqual(report.failures, []);
    assert.deepEqual(report.runs.map((run) => [run.sessionId, run.live, run.workspace]).sort(), [['live-session', true, workspace], ['other-session', true, workspace]]);
    assert.deepEqual(sessionDir.map((name) => readFileSync(join(registry, name, 'owner.json'), 'utf8')).sort(), owners);
    await second.close(); second = undefined;
    const offline = await adapter.discover([registry]);
    const stopped = offline.runs.find((run) => run.sessionId === 'other-session');
    assert.equal(stopped?.live, false); assert.equal(stopped?.state, 'stopped'); assert.ok(stopped?.reason);
  } finally {
    await first?.close(); await second?.close();
    rmSync(base, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

test('managed inventory and its live session merge into one candidate', () => {
  const root = join(tmpdir(), 'manager-merge-registry'), workspace = join(tmpdir(), 'manager-merge-ws'), runId = '11111111-1111-4111-8111-111111111111';
  const report: DiscoveryReport = { runs: [
    { registryRoot: root, sessionId: 's3', runId, instanceId: null, state: 'unknown', live: false, source: 'recorded', workspace, lastProgressAt: '2026-01-01T00:00:00.000Z', reason: 'recorded inventory' },
    { registryRoot: root, sessionId: 's3', runId: null, instanceId: 'i', state: 'running', live: true, source: 'live', workspace, lastProgressAt: null, reason: null },
    { registryRoot: root, sessionId: null, runId: '22222222-2222-4222-8222-222222222222', instanceId: null, state: 'stopped', live: false, source: 'recorded', workspace: null, lastProgressAt: null, reason: 'no session yet' },
  ], failures: [] };
  const view = projectCandidates(report, []);
  assert.equal(view.candidates.length, 2);
  const merged = view.candidates.find((c) => c.sessionId === 's3');
  assert.equal(merged?.runId, runId); assert.equal(merged?.live, true); assert.equal(merged?.state, 'running');
  const orphan = view.candidates.find((c) => c.sessionId === null);
  assert.equal(orphan?.runId, '22222222-2222-4222-8222-222222222222'); assert.equal(orphan?.workspace, null);
});

test('discovery root set unions the effective default root with the configured roots', async () => {
  const root = mkdtempSync(join(tmpdir(), 'manager-discovery-roots-')), fallback = join(root, 'default');
  const config = parseConfig({ version: 1, projects: [{ id: 'p', name: 'P' }], agents: [
    { id: 'a', name: 'A', role: 'worker', projectId: 'p', registryRoot: root, workspace: root, sessionId: 's' }] });
  const calls: string[][] = [];
  const adapter: PiAdapter = { defaultRegistryRoot: () => fallback, discover: async (roots) => { calls.push([...roots]); return { runs: [], failures: [] }; },
    observe: async () => { throw new Error('unused'); }, conversation: async () => { throw new Error('unused'); },
    send: async () => { throw new Error('unused'); }, receipt: async () => null };
  const store = new ManagerStore(root), manager = new AgentManager(config, store, adapter);
  try { await manager.candidates(); assert.deepEqual(calls[0], [fallback, root]); }
  finally { await manager.stop(); store.close(); rmSync(root, { recursive: true, force: true }); }
});

test('a managed run without a session is visible but cannot be registered', async () => {  const root = mkdtempSync(join(tmpdir(), 'manager-discovery-orphan-')), runId = randomUUID();
  const config = parseConfig({ version: 1, projects: [{ id: 'p', name: 'P' }], agents: [] });
  const adapter: PiAdapter = { discover: async () => ({ runs: [{ registryRoot: root, sessionId: null, runId, instanceId: null, state: 'stopped', live: false,
    source: 'recorded', workspace: root, lastProgressAt: null, reason: 'no session yet' }], failures: [] }),
    observe: async () => { throw new Error('unused'); }, conversation: async () => { throw new Error('unused'); },
    send: async () => { throw new Error('unused'); }, receipt: async () => null };
  const store = new ManagerStore(root), manager = new AgentManager(config, store, adapter);
  try {
    const view = await manager.candidates();
    assert.equal(view.candidates[0]?.sessionId, null); assert.equal(view.candidates[0]?.runId, runId);
    await assert.rejects(() => manager.registerCandidate({ candidateId: candidateId({ registryRoot: root, runId }), id: 'x', name: 'X', role: 'worker', projectId: 'p' }), /尚無可綁定的 session/);
  } finally { await manager.stop(); store.close(); rmSync(root, { recursive: true, force: true }); }
});

test('discovery isolates one unreadable root while a healthy root still lists', async () => {
  const api = await import(pathToFileURL(join(defaultPiRoot(), 'channel.mjs')).href) as ChannelModule;
  const base = mkdtempSync(join(tmpdir(), 'manager-discovery-isolate-')), root = join(base, 'registry'), workspace = join(base, 'project'), broken = join(base, 'broken');
  mkdirSync(root); mkdirSync(workspace);
  writeFileSync(broken, 'not a registry directory');
  const adapter = await ChannelAdapter.create();
  let channel: Channel | undefined;
  try {
    channel = await api.startChannel({ root, sessionId: 'healthy', cwd: workspace, deliver: () => {}, getConversation: () => ({ entries: [], cursor: null, headCursor: null, hasMore: false }) });
    const report = await adapter.discover([root, broken]);
    assert.deepEqual(report.runs.map((run) => run.sessionId), ['healthy']);
    assert.deepEqual(report.failures.map((failure) => failure.registryRoot), [broken]);
  } finally {
    await channel?.close();
    rmSync(base, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

test('discovery reports roots dropped past the bounded root cap', async () => {
  const adapter = await ChannelAdapter.create();
  const base = mkdtempSync(join(tmpdir(), 'manager-discovery-cap-'));
  try {
    const roots = Array.from({ length: 33 }, (_, index) => join(base, `root-${index}`));
    const report = await adapter.discover(roots);
    const limit = report.failures.find((failure) => failure.message.includes('上限'));
    assert.ok(limit, 'the dropped root is reported instead of silently ignored');
    assert.equal(limit?.registryRoot, roots[32]);
  } finally { rmSync(base, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }); }
});

test('registration carries the managed run id when the candidate has one', async () => {
  const root = mkdtempSync(join(tmpdir(), 'manager-discovery-runid-')), workspace = mkdtempSync(join(tmpdir(), 'manager-discovery-runid-ws-')), runId = randomUUID();
  const config = parseConfig({ version: 1, projects: [{ id: 'p', name: 'P' }], agents: [] });
  const adapter: PiAdapter = { discover: async () => ({ runs: [{ registryRoot: root, sessionId: 's', runId, instanceId: 'i', state: 'running', live: true,
    source: 'live', workspace, lastProgressAt: null, reason: null }], failures: [] }), observe: async () => running(),
    conversation: async () => { throw new Error('unused'); }, send: async () => { throw new Error('unused'); }, receipt: async () => null };
  const store = new ManagerStore(root), manager = new AgentManager(config, store, adapter);
  try {
    await manager.registerCandidate({ candidateId: candidateId({ registryRoot: root, sessionId: 's' }), id: 'x', name: 'X', role: 'worker', projectId: 'p' });
    assert.equal(manager.binding('x').runId, runId);
  } finally { await manager.stop(); store.close(); rmSync(root, { recursive: true, force: true }); rmSync(workspace, { recursive: true, force: true }); }
});

test('a run merge keeps a recorded degradation unless a live row clears it', () => {
  const root = 'C:/registry', degraded: RecordDegradation = { code: 'record_unavailable', record: 'state.json', message: '無法讀取', recovery: 'inspect' };
  const recorded = (over: Partial<DiscoveredRun> = {}): DiscoveredRun => ({ registryRoot: root, sessionId: 's', runId: null, instanceId: null,
    state: 'unknown', live: false, source: 'recorded', workspace: null, lastProgressAt: null, reason: 'offline', ...over });
  const liveRow = (over: Partial<DiscoveredRun> = {}): DiscoveredRun => ({ ...recorded(), live: true, source: 'live', state: 'idle', reason: null, ...over });
  // A live preferred row is evidence the record is usable, so it clears the other
  // row's recorded degradation.
  assert.equal(dedupeRuns({ runs: [liveRow(), recorded({ degraded })], failures: [] })[0]?.degraded, null);
  assert.equal(dedupeRuns({ runs: [recorded({ degraded }), liveRow()], failures: [] })[0]?.degraded, null);
  // Between two recorded rows the degradation is a real fact and is kept.
  assert.equal(dedupeRuns({ runs: [recorded({ degraded }), recorded()], failures: [] })[0]?.degraded?.code, 'record_unavailable');
  // A live preferred row keeps its own degradation.
  assert.equal(dedupeRuns({ runs: [liveRow({ degraded }), recorded({ degraded: null })], failures: [] })[0]?.degraded?.code, 'record_unavailable');
});

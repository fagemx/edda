import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { AgentManager } from '../src/manager.js';
import { ManagerStore } from '../src/store.js';
import { parseConfig, hash } from '../src/config.js';
import { EddaWorkflowLedger, WorkflowLocks, eddaRunner, type WorkflowLedger, type LedgerNote } from '../src/edda-workflow.js';
import { ContinuationService, type NativeRunner } from '../src/continuation.js';
import type { NativeCapsuleInput, NativeRestore, PortableBundle } from '../src/continuation-contracts.js';
import type { PiAdapter } from '../src/contracts.js';
import type { WorkBinding } from '../src/workflow-contracts.js';

class Ledger implements WorkflowLedger {
  entries: LedgerNote[] = []; crash = false;
  async task(b: WorkBinding) { return { id: b.taskId, key: `evt_source${b.taskId}`, title: 'Continue work', status: 'running', receipt: null, updatedAt: '2026-09-13T00:00:00Z' }; }
  async notes() { return this.entries; }
  async append(_b: WorkBinding, text: string) { this.entries.push({ id: `evt_${randomUUID().replaceAll('-', '')}`, text, at: new Date().toISOString() }); if (this.crash) { this.crash = false; throw new Error('crash after append'); } }
}
const input: NativeCapsuleInput = { capsule_version: 1, state: { title: 'Next stage', goal: 'Finish delivery', current: 'Implementation saved', next_action: 'Read scoped review', open_questions: [] } };
function fixture(root: string, ledger: WorkflowLedger = new Ledger(), workspace = root, taskId = 7) {
  const storage = join(root, randomUUID());
  const config = parseConfig({ version: 1, projects: [{ id: 'p', name: 'Project' }], agents: [{ id: 'owner', name: 'Owner', projectId: 'p', role: 'manager', registryRoot: storage, workspace, sessionId: randomUUID() }], works: [{ id: 'w', projectId: 'p', taskId, workspace, ownerAgentId: 'owner' }] });
  const adapter = { observe: async () => { throw new Error('unused'); }, conversation: async () => { throw new Error('unused'); }, send: async () => { throw new Error('no sends allowed'); }, receipt: async () => null } satisfies PiAdapter;
  const store = new ManagerStore(storage), manager = new AgentManager(config, store, adapter, { ledger, locks: new WorkflowLocks(join(root, 'locks')) });
  let closed = false;
  return { manager, store, storage, config, adapter, close: async () => { if (!closed) { closed = true; await manager.stop(); store.close(); } } };
}
function nativeFixture() {
  const capsules = new Map<string, NativeRestore>(); let saves = 0, dropResponse = false, failRead = false;
  const calls: string[][] = [];
  const run: NativeRunner = async (_workspace, args) => {
    calls.push(args);
    if (args.at(-1) === '--help') return { ok: true, stdout: '--file --json CAPSULE_ID --out' };
    if (args[1] === 'save') {
      saves++; const data = JSON.parse(readFileSync(args[3]!, 'utf8')) as NativeCapsuleInput;
      const id = `cap_${saves}`, event = `evt_saved${saves}`;
      capsules.set(id, { data_authority: 'data_only', local_event_id: event, origin_event_id: event, imported: false, legacy_partial: false, warnings: [], capsule: {
        capsule_version: 1, capsule_id: id, created_at: '2026-09-13T00:00:00Z', source: {}, repository: { portable_repo_id: 'repo_fixture' },
        state: { title: '', summary: '', goal: '', current: '', hypotheses: [], rejected: [], open_questions: [], ...data.state }, git: { head_sha: 'a'.repeat(40), dirty_paths_truncated: false }, references: data.references ?? {}, truncation: [],
      } });
      return { ok: !dropResponse, stdout: dropResponse ? '' : JSON.stringify({ status: 'SAVED_LOCAL', data_authority: 'data_only', capsule_id: id }) };
    }
    if (args[1] === 'restore') return { ok: !failRead && capsules.has(args[2]!), stdout: failRead ? '' : JSON.stringify(capsules.get(args[2]!)) };
    if (args[1] === 'export') {
      const capsule = capsules.get(args[2]!)!.capsule, bytes = JSON.stringify(capsule);
      const bundle: PortableBundle = { bundle_version: 1, portable_repo_id: 'repo_fixture', origin_capsule_id: capsule.capsule_id, origin_event_id: `evt_saved${saves}`, capsule_sha256: hash(bytes), capsule_bytes_hex: Buffer.from(bytes).toString('hex'), bundle_sha256: hash(bytes), data_authority: 'data_only' };
      writeFileSync(args[4]!, JSON.stringify(bundle)); return { ok: true, stdout: 'EXPORTED' };
    }
    throw new Error(`Unexpected native command ${args[1]}`);
  };
  return { run, calls, saves: () => saves, drop: () => { dropResponse = true; }, failRead: (value: boolean) => { failRead = value; } };
}

test('native save wrapper keeps durable intent, native task references and same-ID publication after restart without another save', async () => {
  const root = mkdtempSync(join(tmpdir(), 'continuity-wrapper-')), ledger = new Ledger(), f = fixture(root, ledger), native = nativeFixture();
  let replacement: { manager: AgentManager; store: ManagerStore } | undefined;
  try {
    const service = new ContinuationService(f.manager, { run: native.run, tempRoot: join(root, 'temporary'), locks: new WorkflowLocks(join(root, 'locks')) });
    const revision = (await f.manager.works.continuationSnapshot('w')).view.revision, request = { actionId: randomUUID(), revision, input };
    const [first, duplicate] = await Promise.all([service.publish('w', request), service.publish('w', request)]);
    assert.equal(native.saves(), 1); assert.equal(first.operation?.status, 'attached'); assert.deepEqual(first.context, duplicate.context);
    assert.deepEqual(first.context?.capsule.references, { task_ids: ['7'], event_ids: ['evt_source7'] });
    assert.equal(first.work.continuity?.capsuleId, first.context?.capsule.capsule_id);
    assert.equal(first.bundle?.data_authority, 'data_only'); assert.ok(!JSON.stringify(first.context).includes(root));
    await f.close();
    const store = new ManagerStore(f.storage), manager = new AgentManager(f.config, store, f.adapter, { ledger, locks: new WorkflowLocks(join(root, 'locks')) }); replacement = { manager, store };
    const restarted = new ContinuationService(manager, { run: native.run, tempRoot: join(root, 'temporary'), locks: new WorkflowLocks(join(root, 'locks')) });
    await restarted.publish('w', request); assert.equal(native.saves(), 1);
    await assert.rejects(restarted.publish('w', { ...request, input: { ...input, state: { ...input.state, next_action: 'Different' } } }), /不同內容/);
    assert.equal(native.calls.filter(c => c[1] === 'save' && c.at(-1) !== '--help').length, 1);
  } finally { await f.close(); if (replacement) { await replacement.manager.stop(); replacement.store.close(); } rmSync(root, { recursive: true, force: true }); }
});

test('unknown native effect never replays and blocks blind new save; known capsule recovers failed readback and attachment', async () => {
  for (const known of [false, true]) {
    const root = mkdtempSync(join(tmpdir(), 'continuity-recovery-')), f = fixture(root), native = nativeFixture();
    try {
      const service = new ContinuationService(f.manager, { run: native.run, locks: new WorkflowLocks(join(root, 'locks')) });
      const request = { actionId: randomUUID(), revision: (await f.manager.works.continuationSnapshot('w')).view.revision, input };
      if (known) native.failRead(true); else native.drop();
      if (known) await assert.rejects(service.publish('w', request), /指定原生/); else assert.equal((await service.publish('w', request)).operation?.status, 'unknown');
      native.failRead(false);
      const recovered = await service.publish('w', request); assert.equal(native.saves(), 1); assert.equal(recovered.operation?.status, known ? 'attached' : 'unknown');
      if (!known) await assert.rejects(service.publish('w', { ...request, actionId: randomUUID() }), /上一份/);
    } finally { await f.close(); rmSync(root, { recursive: true, force: true }); }
  }
});

test('unavailable capability and oversized input do not write a fallback or invoke native save', async () => {
  const root = mkdtempSync(join(tmpdir(), 'continuity-unavailable-')), f = fixture(root); let calls = 0;
  try {
    const service = new ContinuationService(f.manager, { run: async () => { calls++; return { ok: false, stdout: '' }; } });
    const request = { actionId: randomUUID(), revision: (await f.manager.works.continuationSnapshot('w')).view.revision, input };
    await assert.rejects(service.publish('w', request), /不支援原生/); assert.equal(calls, 1);
    await assert.rejects(service.publish('w', { ...request, input: { ...input, state: { ...input.state, current: 'x'.repeat(9000) } } }), /上限/);
    assert.equal(calls, 1); assert.equal((await service.latest('w')).operation, null);
    await service.stop(); await assert.rejects(service.publish('w', request), /正在停止/);
  } finally { await f.close(); rmSync(root, { recursive: true, force: true }); }
});

test('native Edda continuation exports/imports between isolated clones, preserving warnings and native origin; takeover uses existing owner action', { skip: !process.env.EDDA_CONTINUITY_TEST_EXE }, async () => {
  const executable = process.env.EDDA_CONTINUITY_TEST_EXE!, root = mkdtempSync(join(tmpdir(), 'continuity-native-')), source = join(root, 'source'), destination = join(root, 'destination');
  const run = (cwd: string, command: string, args: string[]) => execFileSync(command, args, { cwd, encoding: 'utf8', windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  const resources: ReturnType<typeof fixture>[] = [];
  try {
    mkdirSync(source); run(source, 'git', ['init']); run(source, 'git', ['config', 'user.name', 'Fixture']); run(source, 'git', ['config', 'user.email', 'fixture@example.com']);
    writeFileSync(join(source, 'file.txt'), 'initial'); run(source, 'git', ['add', '.']); run(source, 'git', ['commit', '-m', 'initial']);
    run(root, 'git', ['clone', '--no-hardlinks', source, destination]);
    run(source, 'git', ['remote', 'add', 'origin', 'https://example.com/native/continuity.git']); run(destination, 'git', ['remote', 'set-url', 'origin', 'https://example.com/native/continuity.git']);
    for (const dir of [source, destination]) { run(dir, executable, ['init', '--no-hooks']); run(dir, executable, ['task', 'new', 'Continuation fixture']); }
    const ledger = new EddaWorkflowLedger(eddaRunner(executable)), src = fixture(root, ledger, source, 1), dst = fixture(root, ledger, destination, 1); resources.push(src, dst);
    const opts = { executable, tempRoot: join(root, 'temporary'), locks: new WorkflowLocks(join(root, 'continuity-locks')) }, a = new ContinuationService(src.manager, opts), b = new ContinuationService(dst.manager, opts);
    const publication = await a.publish('w', { actionId: randomUUID(), revision: (await src.manager.works.continuationSnapshot('w')).view.revision, input });
    assert.ok(publication.bundle); assert.equal(publication.context?.data_authority, 'data_only');
    const imported = await b.import('w', { actionId: randomUUID(), revision: (await dst.manager.works.continuationSnapshot('w')).view.revision, bundle: publication.bundle! });
    assert.equal(imported.context?.origin_event_id, publication.context?.origin_event_id); assert.equal(imported.context?.imported, true);
    assert.ok(imported.context?.warnings.some(w => w.includes('offline bundle'))); assert.ok(imported.context?.warnings.some(w => w.includes('dirty')));
    let work = await dst.manager.works.act('w', { kind: 'initialize', actionId: randomUUID(), revision: imported.work.revision, nextStep: 'Review restored context' });
    const takeover = { actionId: randomUUID(), revision: work.revision, capsuleId: imported.context!.capsule.capsule_id, ownerAgentId: 'owner', environmentEvidence: 'Required tools verified; dirty scaffolding understood', releaseEvidence: 'Fixture source has no actor' };
    await assert.rejects(b.takeover('w', { ...takeover, releaseEvidence: '' }), /空或過長/);
    work = await b.takeover('w', takeover); const again = await b.takeover('w', takeover);
    assert.equal(work.confirmedActionId, takeover.actionId); assert.equal(again.confirmedActionId, takeover.actionId); assert.equal(work.stage, 'ready');
    assert.equal(work.history.filter(e => e.kind === 'handoff_owner').length, 1);
    assert.equal(run(source, 'git', ['rev-parse', 'HEAD']), run(destination, 'git', ['rev-parse', 'HEAD']));
    await assert.rejects(b.takeover('w', { ...takeover, actionId: randomUUID() }), /更新/);
    // The native product owns missing-commit and dirty warnings; neither is
    // silently upgraded into a manager-side blanket refusal.
    writeFileSync(join(source, 'file.txt'), 'second commit'); run(source, 'git', ['add', 'file.txt']); run(source, 'git', ['commit', '-m', 'second']);
    const later = await a.publish('w', { actionId: randomUUID(), revision: (await src.manager.works.continuationSnapshot('w')).view.revision, input });
    const laterImported = await b.import('w', { actionId: randomUUID(), revision: (await dst.manager.works.continuationSnapshot('w')).view.revision, bundle: later.bundle! });
    assert.ok(laterImported.context?.warnings.includes('saved commit is absent from the current clone'));
    run(destination, 'git', ['remote', 'set-url', 'origin', 'https://example.com/different/project.git']);
    const refused = await b.import('w', { actionId: randomUUID(), revision: laterImported.work.revision, bundle: later.bundle! });
    assert.equal(refused.operation?.nativeStatus, 'refused'); assert.equal(refused.operation?.status, 'unknown'); assert.match(refused.operation!.notice, /儲存庫不同/);
    assert.ok(!refused.operation!.notice.includes(root));
  } finally { for (const f of resources) await f.close(); rmSync(root, { recursive: true, force: true }); }
});

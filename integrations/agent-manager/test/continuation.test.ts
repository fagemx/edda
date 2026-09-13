import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync, readdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { AgentManager } from '../src/manager.js';
import { ManagerStore } from '../src/store.js';
import { parseConfig, hash } from '../src/config.js';
import { EddaWorkflowLedger, WorkflowLocks, eddaRunner, type WorkflowLedger, type LedgerNote } from '../src/edda-workflow.js';
import { ContinuationService, nativeContinuityRunner, type NativeRunner } from '../src/continuation.js';
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
  const capsules = new Map<string, NativeRestore>(); let saves = 0, dropResponse = false, failRead = false, failList = false;
  const calls: string[][] = [];
  const run: NativeRunner = async (_workspace, args, context) => {
    calls.push(args);
    if (args.at(-1) === '--help') return { ok: true, stdout: '--file --json CAPSULE_ID --out' };
    if (args[1] === 'save') {
      saves++; const data = JSON.parse(readFileSync(args[3]!, 'utf8')) as NativeCapsuleInput;
      const id = `cap_${saves}`, event = `evt_saved${saves}`;
      capsules.set(id, { data_authority: 'data_only', local_event_id: event, origin_event_id: event, imported: false, legacy_partial: false, warnings: [], capsule: {
        capsule_version: 1, capsule_id: id, created_at: '2026-09-13T00:00:00Z', source: context ? { actor: context.actor } : {}, repository: { portable_repo_id: 'repo_fixture' },
        state: { title: '', summary: '', goal: '', current: '', hypotheses: [], rejected: [], open_questions: [], ...data.state }, git: { head_sha: 'a'.repeat(40), dirty_paths_truncated: false }, references: data.references ?? {}, truncation: [],
      } });
      return { ok: !dropResponse, stdout: dropResponse ? '' : JSON.stringify({ status: 'SAVED_LOCAL', data_authority: 'data_only', capsule_id: id }) };
    }
    if (args[1] === 'restore') return { ok: !failRead && capsules.has(args[2]!), stdout: failRead ? '' : JSON.stringify(capsules.get(args[2]!)) };
    if (args[1] === 'list') return { ok: !failList, stdout: failList ? '' : JSON.stringify({ data_authority: 'data_only', capsules: [...capsules.values()], warnings: [] }) };
    if (args[1] === 'export') {
      const capsule = capsules.get(args[2]!)!.capsule, bytes = JSON.stringify(capsule);
      const bundle: PortableBundle = { bundle_version: 1, portable_repo_id: 'repo_fixture', origin_capsule_id: capsule.capsule_id, origin_event_id: `evt_saved${saves}`, capsule_sha256: hash(bytes), capsule_bytes_hex: Buffer.from(bytes).toString('hex'), bundle_sha256: hash(bytes), data_authority: 'data_only' };
      writeFileSync(args[4]!, JSON.stringify(bundle)); return { ok: true, stdout: 'EXPORTED' };
    }
    throw new Error(`Unexpected native command ${args[1]}`);
  };
  return { run, calls, capsules, saves: () => saves, drop: () => { dropResponse = true; }, failRead: (value: boolean) => { failRead = value; }, failList: (value: boolean) => { failList = value; } };
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

test('lost stdout and known readback failure recover from latest without original browser or duplicate native save', async () => {
  for (const known of [false, true]) {
    const root = mkdtempSync(join(tmpdir(), 'continuity-recovery-')), f = fixture(root), native = nativeFixture();
    try {
      const service = new ContinuationService(f.manager, { run: native.run, locks: new WorkflowLocks(join(root, 'locks')) });
      const request = { actionId: randomUUID(), revision: (await f.manager.works.continuationSnapshot('w')).view.revision, input };
      if (known) native.failRead(true); else { native.drop(); native.failList(true); }
      if (known) await assert.rejects(service.publish('w', request), /指定原生/); else assert.equal((await service.publish('w', request)).operation?.status, 'unknown');
      native.failRead(false); native.failList(false);
      const recovered = await service.latest('w'); assert.equal(native.saves(), 1); assert.equal(recovered.operation?.status, 'attached'); assert.equal(recovered.operation?.actionId, request.actionId);
      await service.publish('w', request); assert.equal(native.saves(), 1);
    } finally { await f.close(); rmSync(root, { recursive: true, force: true }); }
  }
});

test('exact recovery rejects another capsule or altered state despite actor match, then preserves the original action', async () => {
  const root = mkdtempSync(join(tmpdir(), 'continuity-exact-recovery-')), f = fixture(root), native = nativeFixture();
  try {
    const service = new ContinuationService(f.manager, { run: native.run, locks: new WorkflowLocks(join(root, 'locks')) });
    const request = { actionId: randomUUID(), revision: (await f.manager.works.continuationSnapshot('w')).view.revision, input };
    native.drop(); native.failList(true); const pending = await service.publish('w', request); assert.equal(pending.operation?.status, 'unknown');
    const original = native.capsules.get('cap_1')!;
    const altered = structuredClone(original); altered.capsule.capsule_id = 'cap_other'; altered.capsule.state.next_action = 'Different'; native.capsules.set('cap_other', altered);
    await assert.rejects(service.recover('w', { actionId: request.actionId, capsuleId: 'cap_other' }), /不一致/);
    const recovered = await service.recover('w', { actionId: request.actionId, capsuleId: 'cap_1' });
    assert.equal(recovered.operation?.status, 'attached'); assert.equal(recovered.operation?.actionId, request.actionId); assert.equal(native.saves(), 1);
  } finally { await f.close(); rmSync(root, { recursive: true, force: true }); }
});

test('crash after native save before attachment recovers from persistent known capsule through latest', async () => {
  const root = mkdtempSync(join(tmpdir(), 'continuity-attach-recovery-')), ledger = new Ledger(), f = fixture(root, ledger), native = nativeFixture();
  try {
    const service = new ContinuationService(f.manager, { run: native.run, locks: new WorkflowLocks(join(root, 'locks')) });
    const request = { actionId: randomUUID(), revision: (await f.manager.works.continuationSnapshot('w')).view.revision, input };
    ledger.crash = true; await assert.rejects(service.publish('w', request), /crash/);
    const recovered = await service.latest('w'); assert.equal(recovered.operation?.status, 'attached'); assert.equal(recovered.operation?.actionId, request.actionId);
    assert.equal(ledger.entries.length, 1); assert.equal(native.saves(), 1);
  } finally { await f.close(); rmSync(root, { recursive: true, force: true }); }
});

test('nonplaintext recovery proof verifies native Unicode truncation and rejects changed prefixes', async () => {
  const root = mkdtempSync(join(tmpdir(), 'continuity-hash-proof-')), f = fixture(root), native = nativeFixture();
  try {
    const service = new ContinuationService(f.manager, { run: native.run, locks: new WorkflowLocks(join(root, 'locks')) });
    const data: NativeCapsuleInput = { ...input, state: { ...input.state, title: '界'.repeat(161), current: 'x'.repeat(4001), open_questions: ['y'.repeat(1001)] } };
    const request = { actionId: randomUUID(), revision: (await f.manager.works.continuationSnapshot('w')).view.revision, input: data };
    native.drop(); native.failList(true); await service.publish('w', request);
    const original = native.capsules.get('cap_1')!;
    original.capsule.state.title = '界'.repeat(160); original.capsule.state.current = 'x'.repeat(4000); original.capsule.state.open_questions = ['y'.repeat(1000)];
    original.capsule.truncation = [{ field: 'state.title', omitted_chars: 1, omitted_items: 0 }, { field: 'state.current', omitted_chars: 1, omitted_items: 0 }, { field: 'state.open_questions[0]', omitted_chars: 1, omitted_items: 0 }];
    const altered = structuredClone(original); altered.capsule.capsule_id = 'cap_altered'; altered.capsule.state.current = 'z'.repeat(4000); native.capsules.set('cap_altered', altered);
    await assert.rejects(service.recover('w', { actionId: request.actionId, capsuleId: 'cap_altered' }), /不一致/);
    assert.equal((await service.recover('w', { actionId: request.actionId, capsuleId: 'cap_1' })).operation?.status, 'attached');
    const stored = f.store.setting(`continuity:operation:${request.actionId}`)!;
    assert.ok(!stored.includes('intendedInput')); assert.ok(!stored.includes('界')); assert.ok(!stored.includes('Read scoped review')); assert.ok(stored.includes('inputProof'));
  } finally { await f.close(); rmSync(root, { recursive: true, force: true }); }
});

test('refused request values, unknown property names and bundle hex never enter manager SQLite or WAL', async () => {
  const root = mkdtempSync(join(tmpdir(), 'continuity-refused-privacy-')), f = fixture(root), native = nativeFixture();
  const marker = 'sk-abcdefghijklmnopqrstuvwxyz012345', encodedMarker = Buffer.from(marker).toString('hex');
  try {
    const run: NativeRunner = async (cwd, args, context) => args.at(-1) === '--help' ? native.run(cwd, args, context) : { ok: false, stdout: JSON.stringify({ data_authority: 'data_only', status: args[1] === 'save' ? 'SAVE_FAILED' : 'refused', error: `secret content refused in ${args[1] === 'save' ? 'continuity input' : 'capsule bytes'} (kind: test)` }) };
    const service = new ContinuationService(f.manager, { run, locks: new WorkflowLocks(join(root, 'locks')) });
    const revision = (await f.manager.works.continuationSnapshot('w')).view.revision;
    for (const state of [{ ...input.state, goal: marker }, { ...input.state, [marker]: 'unknown field' }]) {
      const request = { actionId: randomUUID(), revision, input: { ...input, state } };
      const result = await service.publish('w', request); assert.equal(result.operation?.status, 'failed');
      const stored = f.store.setting(`continuity:operation:${request.actionId}`)!; assert.ok(!stored.includes(marker)); assert.ok(!stored.includes(encodedMarker)); assert.ok(stored.includes('inputProof'));
    }
    const actionId = randomUUID();
    const result = await service.import('w', { actionId, revision, bundle: { capsule_bytes_hex: encodedMarker, capsule_sha256: marker, origin_capsule_id: marker } as unknown as PortableBundle }); assert.equal(result.operation?.status, 'failed');
    const stored = f.store.setting(`continuity:operation:${actionId}`)!; assert.ok(!stored.includes(marker)); assert.ok(!stored.includes(encodedMarker)); assert.ok(stored.includes('bundleProof'));
    for (const name of readdirSync(f.storage).filter(name => name.startsWith('manager.sqlite'))) {
      const bytes = readFileSync(join(f.storage, name)); assert.equal(bytes.includes(Buffer.from(marker)), false); assert.equal(bytes.includes(Buffer.from(encodedMarker)), false);
    }
  } finally { await f.close(); rmSync(root, { recursive: true, force: true }); }
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
    let saveCalls = 0; const actual = nativeContinuityRunner(executable);
    const lossy: NativeRunner = async (cwd, args, context) => { const result = await actual(cwd, args, context); if (args[1] === 'save' && args.at(-1) !== '--help' && ++saveCalls === 1) return { ok: false, stdout: '' }; return result; };
    let importCalls = 0;
    const lossyImport: NativeRunner = async (cwd, args, context) => { const result = await actual(cwd, args, context); if (args[1] === 'import' && args.at(-1) !== '--help' && ++importCalls === 1) return { ok: false, stdout: '' }; return result; };
    const opts = { executable, tempRoot: join(root, 'temporary'), locks: new WorkflowLocks(join(root, 'continuity-locks')) }, a = new ContinuationService(src.manager, { ...opts, run: lossy }), b = new ContinuationService(dst.manager, { ...opts, run: lossyImport });
    const publication = await a.publish('w', { actionId: randomUUID(), revision: (await src.manager.works.continuationSnapshot('w')).view.revision, input });
    assert.ok(publication.bundle); assert.equal(publication.context?.data_authority, 'data_only'); assert.equal(saveCalls, 1); assert.match(publication.context!.capsule.source.actor!, /^manager:/);
    const imported = await b.import('w', { actionId: randomUUID(), revision: (await dst.manager.works.continuationSnapshot('w')).view.revision, bundle: publication.bundle! });
    assert.equal(imported.context?.origin_event_id, publication.context?.origin_event_id); assert.equal(imported.context?.imported, true); assert.equal(importCalls, 1);
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
    assert.equal(refused.operation?.nativeStatus, 'refused'); assert.equal(refused.operation?.status, 'failed'); assert.match(refused.operation!.notice, /儲存庫不同/);
    assert.ok(!refused.operation!.notice.includes(root));
    run(destination, 'git', ['remote', 'set-url', 'origin', 'https://example.com/native/continuity.git']);
    const corrected = await b.import('w', { actionId: randomUUID(), revision: refused.work.revision, bundle: later.bundle! }); assert.equal(corrected.operation?.status, 'attached');
    const malformed = await a.publish('w', { actionId: randomUUID(), revision: (await src.manager.works.continuationSnapshot('w')).view.revision, input: { ...input, state: { ...input.state, unsupported: 'typo' } } as NativeCapsuleInput });
    assert.equal(malformed.operation?.status, 'failed'); assert.equal(malformed.operation?.nativeStatus, 'SAVE_FAILED');
    const fixed = await a.publish('w', { actionId: randomUUID(), revision: malformed.work.revision, input }); assert.equal(fixed.operation?.status, 'attached');
    // Synthetic rejected key, never a real credential. Check logical rows AND
    // current database/WAL bytes so deleting plaintext after write cannot pass.
    const rejectedSecret = 'sk-abcdefghijklmnopqrstuvwxyz012345', secretHex = Buffer.from(rejectedSecret).toString('hex');
    const secretRequest = { actionId: randomUUID(), revision: fixed.work.revision, input: { ...input, state: { ...input.state, goal: rejectedSecret } } };
    const rejected = await a.publish('w', secretRequest); assert.equal(rejected.operation?.status, 'failed'); assert.equal(rejected.operation?.nativeStatus, 'SAVE_FAILED');
    function canonical(value: unknown): string {
      const sort = (v: unknown): unknown => Array.isArray(v) ? v.map(sort) : v && typeof v === 'object' ? Object.fromEntries(Object.entries(v).sort(([a], [b]) => a.localeCompare(b)).map(([k, item]) => [k, sort(item)])) : v;
      return JSON.stringify(sort(value));
    }
    const capsule = JSON.parse(Buffer.from(publication.bundle!.capsule_bytes_hex, 'hex').toString('utf8')) as { state: { goal: string } }; capsule.state.goal = rejectedSecret;
    const capsuleBytes = canonical(capsule), badBundle = { ...publication.bundle!, capsule_bytes_hex: Buffer.from(capsuleBytes).toString('hex'), capsule_sha256: hash(capsuleBytes) };
    const { bundle_sha256: _digest, ...bundleContent } = badBundle; badBundle.bundle_sha256 = hash(canonical(bundleContent));
    const badImportId = randomUUID(), rejectedImport = await b.import('w', { actionId: badImportId, revision: corrected.work.revision, bundle: badBundle });
    assert.equal(rejectedImport.operation?.status, 'failed'); assert.match(rejectedImport.operation!.notice, /敏感資訊/);
    for (const [f, actionId] of [[src, secretRequest.actionId], [dst, badImportId]] as const) {
      const stored = f.store.setting(`continuity:operation:${actionId}`)!;
      assert.ok(!stored.includes(rejectedSecret)); assert.ok(!stored.includes(secretHex)); assert.ok(!stored.includes('intendedInput')); assert.ok(!stored.includes('intendedBundle')); assert.ok(!stored.includes('capsule_bytes_hex'));
      for (const name of readdirSync(f.storage).filter(name => name.startsWith('manager.sqlite'))) {
        const bytes = readFileSync(join(f.storage, name)); assert.equal(bytes.includes(Buffer.from(rejectedSecret)), false); assert.equal(bytes.includes(Buffer.from(secretHex)), false);
      }
    }
    assert.equal((await a.publish('w', { actionId: randomUUID(), revision: rejected.work.revision, input })).operation?.status, 'attached');
    assert.equal((await b.import('w', { actionId: randomUUID(), revision: rejectedImport.work.revision, bundle: publication.bundle! })).operation?.status, 'attached');
  } finally { for (const f of resources) await f.close(); rmSync(root, { recursive: true, force: true }); }
});

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm, mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';
import { startChannel } from './channel.mjs';
import { adoptSession, selectSession, discoverDependencies } from './adoption.mjs';
import { readEnrollment } from './supervision.mjs';
import { digest } from './store.mjs';
import { MAX_CONTINUITY_CONTEXT_BYTES } from './continuity.mjs';

const metadata = { role: 'controller', doneWhen: ['Synthetic checks pass'], scope: {
  allowed: ['Observe synthetic fixtures'], excluded: ['Production changes'], reserved: ['New spend'],
  authorityRefs: [{ uri: 'fixture://operator', revision: 'v1' }],
} };
async function fixture(t) {
  const project = await mkdtemp(join(tmpdir(), 'edda-adoption-test-'));
  const root = join(project, 'private');
  const command = { file: process.execPath, args: [fileURLToPath(new URL('./fixtures/edda-capsule-reader.mjs', import.meta.url))] };
  const put = (id, after = [], patch = {}) => writeFile(join(project, `task-${id}.json`), JSON.stringify({
    task_id: id, title: `Synthetic task ${id}`, created_event_id: `evt-${id}`, status: 'ready', after,
    scope_paths: ['fixture/**'], attempts: 0, receipt: null, brief_ref: 'brief.json', ...patch,
  }));
  await put(1, [2]); await put(2);
  await writeFile(join(project, 'brief.json'), JSON.stringify(metadata));
  const messages = [];
  let channel;
  channel = await startChannel({ root, sessionId: randomUUID(), cwd: project, dependencyCommand: command,
    deliver: (text) => { messages.push(text); channel.messageStarted(text); channel.settled(); } });
  t.after(async () => { await channel.close(); await rm(project, { recursive: true, force: true }); });
  const options = { id: '1', eddaCommand: command, scope: 'Only synthetic fixture observation; no spend.' };
  const adopt = (extra = {}) => adoptSession(root, channel.sessionId.slice(0, 8), { ...options, ...extra });
  const capsuleState = (summary = 'Synthetic restored summary') => ({ title: 'Continuity fixture', summary, goal: 'Continue synthetic task',
    current: 'Mid-task', hypotheses: [], rejected: [], open_questions: ['Is the boundary intact?'],
    next_action: 'Run the focused Node gates.' });
  const putCapsule = async (id, { summary, warnings = [] } = {}) => {
    await writeFile(join(project, `capsule-${id}.json`), JSON.stringify({ data_authority: 'data_only',
      local_event_id: 'evt-capsule-local', origin_event_id: 'evt-capsule-origin', imported: false, legacy_partial: false,
      capsule: { capsule_version: 1, capsule_id: id, created_at: '2026-09-13T00:00:00Z', source: {},
        repository: { portable_repo_id: 'repo_fixture', display_hint: 'fixture/repo' },
        state: capsuleState(summary), git: { branch: 'main', head_sha: 'abc123', detached: false, tree_dirty: false, dirty_paths_truncated: false },
        references: {} }, warnings }));
    await writeFile(join(project, 'capsules-list.json'), JSON.stringify({ data_authority: 'data_only',
      capsules: [{ capsule: { capsule_id: id } }], warnings: [] }));
    return id;
  };
  return { project, root, command, put, putCapsule, channel, messages, options, adopt };
}

test('unique prefix includes offline collisions and exact identity wins', () => {
  const rows = [{ sessionId: '12345678-aa', live: true }, { sessionId: '12345678-bb', live: false }];
  assert.equal(selectSession(rows, '12345678').status, 'ambiguous_session');
  assert.equal(selectSession(rows, '12345678-aa').state, rows[0]);
  assert.equal(selectSession(rows, '123').status, 'invalid_selector');
  assert.equal(selectSession(rows, 'notfound').status, 'not_registered');
});

test('discover bounded transitive diamond plus explicit review root; cycles/missing/overflow fail', async (t) => {
  const f = await fixture(t);
  await f.put(1, [2, 3]); await f.put(2, [4]); await f.put(3, [4]); await f.put(4); await f.put(5, [1]);
  const graph = await discoverDependencies(f.project, ['1', '5'], f.command);
  assert.deepEqual(graph.taskIds, ['1', '2', '3', '4', '5']);
  assert.equal(graph.edges.length, 5);
  await f.put(4, [1]);
  await assert.rejects(discoverDependencies(f.project, ['1'], f.command), /cycle/i);
  await f.put(4, [99]);
  await assert.rejects(discoverDependencies(f.project, ['1'], f.command), /Cannot read task 99/);
  for (let i = 1; i <= 9; i++) await f.put(i, i < 9 ? [i + 1] : []);
  await assert.rejects(discoverDependencies(f.project, ['1'], f.command), /eight/i);
});

test('preview, missing context, invalid cap and busy session make no managed changes', async (t) => {
  const f = await fixture(t);
  const preview = await f.adopt({ preview: true });
  assert.equal(preview.status, 'preview');
  assert.deepEqual(preview.dependencies.taskIds, ['1', '2']);
  assert.equal(readEnrollment(f.root, f.channel.sessionId), null);
  assert.equal(f.channel.handoffContext().status, 'not_prepared');
  await assert.rejects(f.adopt({ maxNotifications: 0 }), /cap/);
  await writeFile(join(f.project, 'brief.json'), 'ordinary prose');
  assert.equal((await f.adopt()).status, 'needs_context');
  assert.equal(readEnrollment(f.root, f.channel.sessionId), null);
  f.channel.event('agent_start');
  assert.equal((await f.adopt()).status, 'busy');
  assert.equal(f.messages.length, 0);
});

test('adopt prepares/enrolls/follows without starting work; repeat status changes preserve cap', async (t) => {
  const f = await fixture(t);
  const result = await f.adopt();
  assert.equal(result.status, 'adopted');
  assert.equal(result.workStarted, false);
  assert.equal(f.channel.handoffContext().status, 'ready');
  await f.channel.dependencies.check();
  assert.equal(f.messages.length, 0);
  await f.adopt({ notify: true, maxNotifications: 1 });
  await f.channel.dependencies.check();
  assert.equal(f.messages.length, 1);
  const original = f.channel.dependencies.status();
  await f.put(1, [2], { status: 'done', receipt: 'Changes Requested' });
  const again = await f.adopt({ notify: true, maxNotifications: 1 });
  await f.channel.dependencies.check();
  assert.equal(again.manifestReused, true);
  assert.equal(f.channel.dependencies.status().phase, 'notification_limit');
  assert.equal(f.channel.dependencies.status().lastAlert.id, original.lastAlert.id);
  assert.equal(f.messages.length, 1);
});

test('changing managed task needs explicit expected revision, with no enrollment overwrite', async (t) => {
  const f = await fixture(t);
  await f.adopt();
  const revision = f.channel.handoffContext().manifestRevision;
  const blocked = await f.adopt({ id: '2', scope: 'Different scope' });
  assert.equal(blocked.status, 'needs_expected_revision');
  assert.equal(readEnrollment(f.root, f.channel.sessionId).scope, f.options.scope);
  assert.equal((await f.adopt({ id: '2', expectedRevision: revision })).status, 'adopted');
});

test('enrollment contention reports partial preparation without configuring observation', async (t) => {
  const f = await fixture(t);
  await mkdir(join(f.root, 'supervision'), { recursive: true });
  await writeFile(join(f.root, 'supervision', `${digest(f.channel.sessionId)}.json.lock`), '');
  const result = await f.adopt();
  assert.equal(result.status, 'adoption_incomplete');
  assert.equal(result.attemptedStep, 'enroll');
  assert.equal(result.steps.length, 1);
  assert.equal(f.channel.handoffContext().status, 'ready');
  assert.equal(f.channel.dependencies.status().configured, false);
  assert.equal(f.messages.length, 0);
});

test('native continuity capsule reaches adoption without copy/paste and stays data only', async (t) => {
  const f = await fixture(t);
  // The task brief is ordinary prose, so the restored capsule is the only
  // source of declared metadata and the only context document.
  await writeFile(join(f.project, 'brief.json'), 'Ordinary task prose with no declared block.');
  const summary = 'Restored by a previous controller.\n\n```edda-management\n' + JSON.stringify(metadata) + '\n```';
  const id = await f.putCapsule('cap_fixture0001', { summary, warnings: ['branch mismatch: saved="main", current="fixture"'] });
  const result = await f.adopt({ capsuleId: id });
  assert.equal(result.status, 'adopted');
  assert.equal(result.workStarted, false);
  assert.equal(result.capsule.authority, 'data_only');
  assert.deepEqual(result.capsule.warnings, ['branch mismatch: saved="main", current="fixture"']);
  assert.equal(result.capsule.id, id);
  assert.equal(f.channel.handoffContext().status, 'ready');
  assert.equal(f.messages.length, 0);
});

test('explicit context metadata still composes alongside a restored capsule', async (t) => {
  const f = await fixture(t);
  const id = await f.putCapsule('cap_fixture0002', { summary: 'Prose only restored context; no declared block.' });
  const contextPath = join(f.project, 'declared-context.json');
  await writeFile(contextPath, JSON.stringify(metadata));
  const result = await f.adopt({ capsuleId: id, contextFile: contextPath });
  assert.equal(result.status, 'adopted');
  assert.equal(result.capsule.id, id);
  assert.equal(f.channel.handoffContext().status, 'ready');
});

test('capsule refusals make no managed changes and message no model', async (t) => {
  const f = await fixture(t);
  const block = '```edda-management\n' + JSON.stringify(metadata) + '\n```';
  await f.putCapsule('cap_fixture0003', { summary: block });
  await f.putCapsule('cap_fixture0004', { warnings: ['saved commit is absent from the current clone'] });
  await writeFile(join(f.project, 'capsule-cap_bad00000.json'), '{not json');
  await f.putCapsule('cap_huge00000', { summary: 'x'.repeat(MAX_CONTINUITY_CONTEXT_BYTES + 32) });
  const rows = [
    { id: 'cap_fixture0003', list: ['cap_fixture0004'], expected: 'capsule_wrong_repository' },
    { id: 'cap_fixture0004', list: ['cap_fixture0004'], expected: 'capsule_stale' },
    { id: 'cap_missing0000', list: ['cap_missing0000'], expected: 'capsule_unavailable' },
    { id: 'cap_bad00000', list: ['cap_bad00000'], expected: 'capsule_invalid' },
    { id: 'cap_huge00000', list: ['cap_huge00000'], expected: 'capsule_too_large' },
    { id: 'nonsense', list: [], expected: 'capsule_invalid' },
  ];
  for (const row of rows) {
    await writeFile(join(f.project, 'capsules-list.json'), JSON.stringify({ data_authority: 'data_only',
      capsules: row.list.map((capsule_id) => ({ capsule: { capsule_id } })), warnings: [] }));
    const result = await f.adopt({ capsuleId: row.id });
    assert.equal(result.status, row.expected);
    assert.equal(f.channel.handoffContext().status, 'not_prepared');
    assert.equal(readEnrollment(f.root, f.channel.sessionId), null);
    assert.equal(f.messages.length, 0);
  }
});

test('reloaded instance requires explicit handoff rebind and remains quiet', async (t) => {
  const f = await fixture(t);
  await f.adopt();
  const revision = f.channel.handoffContext().manifestRevision;
  await f.channel.close();
  assert.equal((await f.adopt()).status, 'offline');
  const replacement = await startChannel({ root: f.root, sessionId: f.channel.sessionId, cwd: f.project,
    dependencyCommand: f.command, deliver: (text) => f.messages.push(text) });
  try {
    assert.equal((await f.adopt()).status, 'needs_expected_revision');
    assert.equal(replacement.dependencies.status().phase, 'needs_refollow');
    assert.equal((await f.adopt({ expectedRevision: revision })).status, 'adopted');
    assert.equal(f.messages.length, 0);
  } finally { await replacement.close(); }
});

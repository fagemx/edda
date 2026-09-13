import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { validateCapsuleId, parseCapsuleEnvelope, capsuleContextDocument, restoreCapsuleContext, MAX_CONTINUITY_CONTEXT_BYTES } from './continuity.mjs';
import { composeHandoff } from './compose.mjs';

const metadata = () => ({ role: 'controller', doneWhen: ['Fixture checks pass'], scope: {
  allowed: ['Continue the synthetic fixture task'], excluded: ['No production changes'], reserved: ['New spend'],
  authorityRefs: [{ uri: 'fixture://operator', revision: 'v1' }],
} });
const task = () => ({ task_id: 17, title: 'Continue fixture work', created_event_id: 'evt-fixture-17',
  status: 'ready', after: [], scope_paths: ['src/fixture/**'], plan_id: 'plan-demo', work_unit_ref: 'W17',
  brief_ref: 'brief.md', attempts: 0, session_id: null, receipt: null });
const CAPSULE_ID_VALUE = 'cap_abcdef0123456789';

function state(overrides = {}) {
  return { title: 'Synthetic continuity', summary: 'Restored summary', goal: 'Finish the fixture',
    current: 'Halfway', hypotheses: [], rejected: [], open_questions: ['Is the boundary intact?'],
    next_action: 'Run the focused Node gates.', ...overrides };
}
function envelope({ id = CAPSULE_ID_VALUE, warnings = [], capsuleState = state(), repository = { portable_repo_id: 'repo_fixture', display_hint: 'fixture/repo' } } = {}) {
  return {
    data_authority: 'data_only', local_event_id: 'evt-local-1', origin_event_id: 'evt-origin-1',
    imported: false, legacy_partial: false,
    capsule: { capsule_version: 1, capsule_id: id, created_at: '2026-09-13T00:00:00Z',
      source: { actor: 'fixture' }, repository,
      state: capsuleState,
      git: { branch: 'main', head_sha: 'abc123', detached: false, tree_dirty: false, dirty_paths_truncated: false },
      references: {} },
    warnings,
  };
}
const command = { file: process.execPath, args: [fileURLToPath(new URL('./fixtures/edda-capsule-reader.mjs', import.meta.url))] };

async function fixture(t, { capsuleId = CAPSULE_ID_VALUE, capsuleText = JSON.stringify(envelope()), list = true, writeCapsule = true } = {}) {
  const project = await mkdtemp(join(tmpdir(), 'edda-continuity-test-'));
  t.after(() => rm(project, { recursive: true, force: true }));
  await writeFile(join(project, 'task-17.json'), JSON.stringify(task()));
  await writeFile(join(project, 'capsules-list.json'), JSON.stringify({ data_authority: 'data_only',
    capsules: list ? [{ capsule: { capsule_id: capsuleId } }] : [], warnings: [] }));
  if (writeCapsule) await writeFile(join(project, `capsule-${capsuleId}.json`), capsuleText);
  return { project, root: join(project, 'private-cache'), options: { project, id: '17', root: join(project, 'private-cache'), eddaCommand: command } };
}

test('capsule IDs are canonical lowercase identifiers', () => {
  assert.equal(validateCapsuleId('cap_abc123'), 'cap_abc123');
  for (const bad of ['cap_ABC', 'CAP_abc', 'cap-abc', 'cap_', '', 'cap_abc!', 42, null]) assert.throws(() => validateCapsuleId(bad));
});

test('envelope validation requires data_only identity and complete bounded state', () => {
  const parsed = parseCapsuleEnvelope(envelope(), CAPSULE_ID_VALUE);
  assert.equal(parsed.authority, 'data_only');
  assert.equal(parsed.localEventId, 'evt-local-1');
  assert.throws(() => parseCapsuleEnvelope({ ...envelope(), data_authority: 'execute' }, CAPSULE_ID_VALUE), /data_only/);
  assert.throws(() => parseCapsuleEnvelope(envelope(), 'cap_other000'), /identity/);
  assert.throws(() => parseCapsuleEnvelope(envelope({ id: 'cap_other000' }), CAPSULE_ID_VALUE), /identity/);
  assert.throws(() => parseCapsuleEnvelope({ ...envelope(), capsule: { ...envelope().capsule, capsule_version: 2 } }, CAPSULE_ID_VALUE), /version/);
  assert.throws(() => parseCapsuleEnvelope({ ...envelope(), capsule: { ...envelope().capsule, state: state({ goal: 7 }) } }, CAPSULE_ID_VALUE), /state.goal/);
  assert.throws(() => parseCapsuleEnvelope({ ...envelope(), warnings: [1] }, CAPSULE_ID_VALUE), /warnings/);
});

test('restored document keeps provenance, warnings and a line-start management block', () => {
  const block = '```edda-management\n' + JSON.stringify(metadata()) + '\n```';
  const parsed = parseCapsuleEnvelope(envelope({ warnings: ['branch mismatch: saved="main", current="feature"'],
    capsuleState: state({ summary: block }) }), CAPSULE_ID_VALUE);
  const document = capsuleContextDocument(parsed, 'deadbeef');
  assert.match(document, /DATA AUTHORITY: data_only/);
  assert.match(document, /WARNING: branch mismatch/);
  assert.match(document, /REPOSITORY: repo_fixture/);
  assert.match(document, /NATIVE RESTORE DIGEST: sha256:deadbeef/);
  assert.match(document, /NEXT ACTION \(DATA ONLY\):\nRun the focused Node gates\./);
  assert.ok(document.includes('\n```edda-management\n'));
});

test('successful restore lists the repository, reads the exact capsule and stays data only', async (t) => {
  const f = await fixture(t, { capsuleText: JSON.stringify(envelope({ warnings: ['saved checkout was dirty'] })) });
  const restored = await restoreCapsuleContext({ project: f.project, capsuleId: CAPSULE_ID_VALUE, eddaCommand: command });
  assert.equal(restored.status, 'restored');
  assert.equal(restored.capsule.authority, 'data_only');
  assert.deepEqual(restored.capsule.warnings, ['saved checkout was dirty']);
  assert.equal(restored.capsule.repository.portableRepoId, 'repo_fixture');
  assert.match(restored.capsule.revision, /^sha256:[0-9a-f]{64}$/);
  assert.ok(Buffer.byteLength(restored.document) <= MAX_CONTINUITY_CONTEXT_BYTES);
});

test('wrong repository, unavailable, malformed, oversize and stale capsules refuse clearly', async (t) => {
  const wrong = await fixture(t, { list: false });
  assert.equal((await restoreCapsuleContext({ project: wrong.project, capsuleId: CAPSULE_ID_VALUE, eddaCommand: command })).status, 'capsule_wrong_repository');

  const missing = await fixture(t, { writeCapsule: false });
  assert.equal((await restoreCapsuleContext({ project: missing.project, capsuleId: CAPSULE_ID_VALUE, eddaCommand: command })).status, 'capsule_unavailable');

  const malformed = await fixture(t, { capsuleText: '{not json' });
  assert.equal((await restoreCapsuleContext({ project: malformed.project, capsuleId: CAPSULE_ID_VALUE, eddaCommand: command })).status, 'capsule_invalid');

  const oversize = await fixture(t, { capsuleText: JSON.stringify(envelope({ capsuleState: state({ summary: 'x'.repeat(MAX_CONTINUITY_CONTEXT_BYTES + 16) }) })) });
  assert.equal((await restoreCapsuleContext({ project: oversize.project, capsuleId: CAPSULE_ID_VALUE, eddaCommand: command })).status, 'capsule_too_large');

  const stale = await fixture(t, { capsuleText: JSON.stringify(envelope({ warnings: ['saved commit is absent from the current clone'] })) });
  assert.equal((await restoreCapsuleContext({ project: stale.project, capsuleId: CAPSULE_ID_VALUE, eddaCommand: command })).status, 'capsule_stale');

  const dangling = await restoreCapsuleContext({ project: t.name, capsuleId: 'not-a-capsule', eddaCommand: command });
  assert.equal(dangling.status, 'capsule_invalid');
});

test('compose consumes a capsule block and a capsule alongside explicit metadata', async (t) => {
  const block = '```edda-management\n' + JSON.stringify(metadata()) + '\n```';
  const embedded = await fixture(t, { capsuleText: JSON.stringify(envelope({ capsuleState: state({ summary: block }) })) });
  const result = await composeHandoff({ ...embedded.options, capsuleId: CAPSULE_ID_VALUE });
  assert.equal(result.status, 'ready');
  assert.equal(result.manifest.role, 'controller');
  assert.equal(result.capsule.authority, 'data_only');
  assert.equal(result.source.contextStatus, 'native_capsule');
  assert.equal(result.source.capsuleStatus, 'native_capsule');
  assert.equal(result.source.capsuleRef.id, CAPSULE_ID_VALUE);

  const supplemental = await fixture(t);
  const contextPath = join(supplemental.project, 'context.json');
  await writeFile(contextPath, JSON.stringify(metadata()));
  const combined = await composeHandoff({ ...supplemental.options, contextFile: contextPath, capsuleId: CAPSULE_ID_VALUE });
  assert.equal(combined.status, 'ready');
  assert.equal(combined.source.contextStatus, 'explicit_context');
  assert.equal(combined.capsule.id, CAPSULE_ID_VALUE);
  const snapshot = await (await import('node:fs/promises')).readFile(combined.source.contextSource.uri, 'utf8');
  assert.match(snapshot, /edda-management/);
  assert.match(snapshot, /RESTORED STATE/);
});

test('compose returns the capsule refusal without a ready manifest', async (t) => {
  const f = await fixture(t, { list: false });
  const result = await composeHandoff({ ...f.options, capsuleId: CAPSULE_ID_VALUE, output: join(f.project, 'never.json') });
  assert.equal(result.status, 'capsule_wrong_repository');
  assert.match(result.capsuleError, /repository/);
  assert.equal(result.capsuleId, CAPSULE_ID_VALUE);
  await assert.rejects((await import('node:fs/promises')).access(join(f.project, 'never.json')));
});

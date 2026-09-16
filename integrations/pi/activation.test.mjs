import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, rm, writeFile, readdir, symlink, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import { runtimeInfo, listManagedRuns } from './activation.mjs';
import { installRuntime } from './managed-store.mjs';

const exec = promisify(execFile), cli = fileURLToPath(new URL('./cli.mjs', import.meta.url));
async function fixture(t) {
  const dir = await mkdtemp(join(tmpdir(), 'edda-activation-test-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  return dir;
}
async function record(root, id, values = {}) {
  const dir = join(root, 'managed', id);
  await mkdir(dir, { recursive: true });
  await writeFile(join(dir, 'config.json'), JSON.stringify({ version: 1, runId: id, root: resolve(root), project: root,
    prompt: 'SECRET-PROMPT', ...values.config }));
  await writeFile(join(dir, 'state.json'), JSON.stringify({ runId: id, phase: 'stopped', sessionId: randomUUID(),
    updatedAt: '2026-09-13T00:00:00.000Z', error: 'SECRET-ERROR', token: 'SECRET-TOKEN', ...values.state }));
}

test('runtime-info and runs are read-only and publish honest capabilities', async (t) => {
  const dir = await fixture(t), root = join(dir, 'absent');
  const info = runtimeInfo(root);
  assert.equal(info.registryRoot, root);
  assert.equal(info.capabilities.automaticProcessRestart, false);
  assert.equal(info.capabilities.automaticOwnerWake, false);
  assert.equal(info.capabilities.optInProcessRecovery, true);
  assert.deepEqual(info.capabilities.unattendedRecovery, { inProcess: 'opt-in', hostRestart: false, systemAutostart: false });
  assert.equal(info.capabilities.managedFork, false);
  assert.match(await readFile(info.guide, 'utf8'), /new session/i);
  assert.deepEqual(listManagedRuns(root).runs, []);
  assert.deepEqual(await readdir(dir), []);
  const result = await exec(process.execPath, [cli, 'runtime-info'], { env: { ...process.env, EDDA_PI_CHANNEL_DIR: root }, cwd: dir })
    .catch((error) => { assert.equal(error.code, 2); return error; });
  assert.equal(JSON.parse(result.stdout).registryRoot, root);
  assert.deepEqual(await readdir(dir), []);
});

test('runtime-info names the installed extension and digest-verified releases', async (t) => {
  const dir = await fixture(t), root = join(dir, 'registry');
  const info = runtimeInfo(root);
  assert.match(info.extension.path, /extension\.mjs$/);
  assert.match(info.extension.digest, /^[0-9a-f]{64}$/);
  assert.equal(info.extension.version, info.version);
  // Drift is checked against the channel module a live session reports, not the extension.
  assert.match(info.channel.path, /channel\.mjs$/);
  assert.match(info.channel.digest, /^[0-9a-f]{64}$/);
  assert.equal(info.channel.version, info.version);
  assert.deepEqual(info.handOpened.argv, ['pi', '-e', info.extension.path]);
  assert.ok(info.handOpened.load.includes(`"${info.extension.path}"`), 'load command quotes the path');
  assert.match(info.handOpened.ownerRef, /EDDA_OWNER_REF/);
  assert.deepEqual(info.releases, []);
  assert.equal(info.releasesHasMore, false);
  assert.deepEqual(await readdir(dir), []); // the releases scan is read-only
  const release = installRuntime(root), after = runtimeInfo(root);
  assert.equal(after.releases.length, 1);
  assert.equal(after.releases[0].id, release.id);
  assert.equal(after.releases[0].verified, true);
  assert.equal(after.releases[0].version, info.version);
  assert.equal(after.releases[0].extension, join(release.path, 'extension.mjs'));
  assert.equal(after.releases[0].channel, join(release.path, 'channel.mjs'));
  assert.match(await readFile(info.guide, 'utf8'), /Hand-opened sessions are not wired automatically/);
});

test('run rows classify continuity from the owner reference', async (t) => {
  const root = await fixture(t), owned = randomUUID(), unowned = randomUUID();
  await record(root, owned, { config: { owner: 'assistant/x' } });
  await record(root, unowned);
  const runs = listManagedRuns(root).runs;
  assert.equal(runs.find((r) => r.runId === owned).continuity, 'owner-bound');
  assert.equal(runs.find((r) => r.runId === unowned).continuity, 'session-addressed');
});

test('run discovery preserves corrupt records and does not leak prompt/token/provider error', async (t) => {
  const root = await fixture(t), good = randomUUID(), bad = randomUUID();
  await record(root, good); await record(root, bad);
  const badFile = join(root, 'managed', bad, 'state.json');
  await writeFile(badFile, '{ SECRET-BROKEN-JSON');
  const result = listManagedRuns(root);
  assert.equal(result.runs.length, 2);
  const healthy = result.runs.find((r) => r.runId === good), corrupt = result.runs.find((r) => r.runId === bad);
  assert.equal(healthy.recordedPhase, 'stopped');
  assert.equal(healthy.observedLive, null);
  assert.equal(corrupt.recordedPhase, 'unknown');
  assert.ok(corrupt.error);
  assert.doesNotMatch(JSON.stringify(result), /SECRET/);
  assert.equal(await readFile(badFile, 'utf8'), '{ SECRET-BROKEN-JSON');
});

test('run discovery keeps config identity when state.json is NUL bytes', async (t) => {
  const root = await fixture(t), good = randomUUID(), bad = randomUUID();
  await record(root, good);
  await record(root, bad, { config: { release: { id: 'a'.repeat(64) } } });
  const badFile = join(root, 'managed', bad, 'state.json');
  await writeFile(badFile, Buffer.alloc(1887, 0));
  const before = await readFile(badFile);
  const result = listManagedRuns(root);
  assert.equal(result.runs.length, 2);
  const healthy = result.runs.find((r) => r.runId === good), corrupt = result.runs.find((r) => r.runId === bad);
  assert.equal(healthy.error, null);
  assert.equal(corrupt.project, root);
  assert.equal(corrupt.releaseId, 'a'.repeat(64));
  assert.equal(corrupt.error.code, 'record_unavailable');
  assert.equal(corrupt.error.record, 'state.json');
  assert.doesNotMatch(JSON.stringify(result), /SECRET/);
  assert.deepEqual(await readFile(badFile), before);
});

test('runs paginate deterministically, validate input and do not traverse nested registries', async (t) => {
  const root = await fixture(t), ids = [randomUUID(), randomUUID(), randomUUID()].sort();
  for (const id of ids) await record(root, id);
  await record(join(root, 'unselected'), randomUUID());
  const first = listManagedRuns(root, { limit: 2 });
  assert.deepEqual(first.runs.map((r) => r.runId), ids.slice(0, 2));
  assert.equal(first.hasMore, true); assert.equal(first.nextAfter, ids[1]);
  const next = listManagedRuns(root, { after: first.nextAfter, limit: 2 });
  assert.deepEqual(next.runs.map((r) => r.runId), ids.slice(2));
  assert.equal(next.hasMore, false); assert.equal(next.nextAfter, null);
  assert.throws(() => listManagedRuns(root, { limit: 0 }), /limit/);
  assert.throws(() => listManagedRuns(root, { limit: 101 }), /limit/);
  assert.throws(() => listManagedRuns(root, { after: '../outside' }), /UUID/);
});

test('linked or mismatched run records remain unknown instead of being adopted', async (t) => {
  const root = await fixture(t), wrong = randomUUID(), linked = randomUUID(), outside = join(root, 'outside');
  await record(root, wrong, { config: { root: 'other-registry' } });
  await mkdir(outside);
  await symlink(outside, join(root, 'managed', linked), process.platform === 'win32' ? 'junction' : 'dir');
  assert.ok(listManagedRuns(root).runs.every((r) => r.error && r.sessionId === null));
});

test('installed-style help and argument errors keep machine-readable discovery usable', async (t) => {
  const root = await fixture(t), env = { ...process.env, EDDA_PI_CHANNEL_DIR: join(root, 'registry') };
  const help = (await exec(process.execPath, [cli, '--help'], { env, cwd: root })).stdout;
  assert.match(help, /edda-pi runs/); assert.match(help, /edda-pi runtime-info/);
  assert.match((await exec(process.execPath, [cli, '--version'], { env })).stdout, /^edda-pi \d+\.\d+\.\d+/);
  assert.deepEqual(JSON.parse((await exec(process.execPath, [cli, 'runs'], { env, cwd: root })).stdout).runs, []);
  await assert.rejects(exec(process.execPath, [cli, 'launch'], { env }), (error) => {
    assert.match(error.stderr, /requires --project/); assert.doesNotMatch(error.stderr, /Run ID:/); return true;
  });
  assert.deepEqual(await readdir(root), []);
});

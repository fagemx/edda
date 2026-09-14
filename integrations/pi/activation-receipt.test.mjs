import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, rm, writeFile, readdir, stat, cp } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { createServer } from 'node:http';
import { join, resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import { RECEIPT_VERSION, releaseIdentity, activationReceipt, evaluateCoherence } from './activation-receipt.mjs';
import { installRuntime } from './managed-store.mjs';

const exec = promisify(execFile);
const packageDir = fileURLToPath(new URL('.', import.meta.url));
const cli = fileURLToPath(new URL('./activation-receipt.mjs', import.meta.url));

async function fixture(t) {
  const dir = await mkdtemp(join(tmpdir(), 'edda-activation-receipt-test-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  return dir;
}

async function snapshot(dir) {
  const entries = [];
  async function walk(current, prefix) {
    let names;
    try { names = await readdir(current, { withFileTypes: true }); }
    catch (error) { if (error.code === 'ENOENT') return; throw error; }
    for (const entry of names.sort((left, right) => left.name.localeCompare(right.name))) {
      const relative = prefix ? `${prefix}/${entry.name}` : entry.name, full = join(current, entry.name);
      if (entry.isDirectory()) { entries.push(`${relative}/`); await walk(full, relative); }
      else { const info = await stat(full); entries.push(`${relative}:${info.size}:${info.mtimeMs}`); }
    }
  }
  await walk(dir, '');
  return entries;
}

function legs(mutate) {
  const head = 'a'.repeat(40);
  const value = {
    repository: { observed: true, root: '/repo', headRevision: head, headRef: 'refs/heads/main', dirty: false,
      piPackagingRevision: 'b'.repeat(40), error: null },
    edda: { observed: true, binary: 'edda', version: '0.6.1', revision: head.slice(0, 12), dirtyBuild: false,
      builtAt: '2026-09-13', raw: `edda 0.6.1 (${head.slice(0, 12)} 2026-09-13)`, error: null },
    pi: { observed: true, installedReleaseId: 'c'.repeat(64), repoReleaseId: 'c'.repeat(64), error: null },
    manager: { observed: true, root: '/manager',
      configured: { mergeCommit: head, headSha: head, sourceWorktree: '/src', entrypoint: '/entry' },
      configuredAt: '2026-09-13T00:00:00.000Z',
      running: { pid: 1, instanceId: 'instance', origin: 'http://127.0.0.1:1', startedAt: '2026-09-13T00:00:00.000Z' },
      pidAlive: true, health: 'ok', service: { version: 1, startedAt: '2026-09-13T00:00:00.000Z', agents: 1 }, error: null },
  };
  if (mutate) mutate(value);
  return value;
}

test('releaseIdentity is byte-equivalent to installRuntime for real and synthetic sources', async (t) => {
  const dir = await fixture(t);

  const realIdentity = releaseIdentity(packageDir);
  const realRelease = installRuntime(join(dir, 'registry-real'), packageDir);
  assert.equal(realIdentity.id, realRelease.id);
  assert.equal(realIdentity.version, realRelease.version);
  assert.equal(realIdentity.version, '0.8.0');

  const source = join(dir, 'synthetic');
  await mkdir(source);
  await writeFile(join(source, 'package.json'), JSON.stringify({ name: '@edda/pi-session-channel', version: '0.0.0-test' }));
  await writeFile(join(source, 'managed-runner.mjs'), 'export const runner = true;\n');
  await writeFile(join(source, 'extension.mjs'), 'export const extension = true;\n');
  await writeFile(join(source, 'helper.mjs'), 'export const helper = true;\n');
  await writeFile(join(source, 'helper.test.mjs'), '// excluded test file\n');
  await writeFile(join(source, 'smoke.mjs'), '// excluded smoke file\n');
  await writeFile(join(source, 'install.ps1'), 'Write-Host ok\n');
  await writeFile(join(source, 'getting-started.md'), '# guide\n');
  const synthetic = releaseIdentity(source);
  assert.deepEqual(Object.keys(synthetic.files), ['extension.mjs', 'getting-started.md', 'helper.mjs', 'install.ps1', 'managed-runner.mjs', 'package.json']);
  assert.ok(!('helper.test.mjs' in synthetic.files) && !('smoke.mjs' in synthetic.files));
  assert.equal(synthetic.id, installRuntime(join(dir, 'registry-synthetic'), source).id);
  assert.equal(synthetic.version, '0.0.0-test');

  const incomplete = join(dir, 'incomplete');
  await mkdir(incomplete);
  await writeFile(join(incomplete, 'package.json'), '{}');
  assert.throws(() => releaseIdentity(incomplete), /Incomplete runtime source/);
});

test('activationReceipt is strictly read-only over an absent and a seeded registry', async (t) => {
  const dir = await fixture(t);
  const empty = join(dir, 'empty-registry');
  await mkdir(empty);
  const beforeEmpty = await snapshot(empty);
  const emptyReceipt = await activationReceipt({ root: empty, repo: join(dir, 'not-a-repo'),
    managerRoot: join(dir, 'no-manager'), runGit: () => { throw new Error('no git'); },
    runVersion: () => { throw new Error('no edda'); }, fetch: async () => ({ version: 1, startedAt: 'x', agents: 0 }) });
  assert.deepEqual(await snapshot(empty), beforeEmpty);
  assert.equal(existsSync(join(empty, 'releases')), false);
  assert.equal(emptyReceipt.coherence.status, 'partial');
  assert.equal(emptyReceipt.repository.observed, false);
  assert.equal(emptyReceipt.edda.observed, false);

  const seeded = join(dir, 'seeded-registry');
  const releaseId = 'd'.repeat(64), runId = randomUUID();
  await mkdir(join(seeded, 'releases', releaseId), { recursive: true });
  await writeFile(join(seeded, 'releases', releaseId, 'release.json'), JSON.stringify({ version: 1, id: releaseId, files: {} }));
  await mkdir(join(seeded, 'managed', runId), { recursive: true });
  await writeFile(join(seeded, 'managed', runId, 'config.json'), JSON.stringify({ version: 1, runId,
    root: resolve(seeded), project: dir, release: { id: releaseId, path: join(seeded, 'releases', releaseId) } }));
  const beforeSeeded = await snapshot(seeded);
  const seededReceipt = await activationReceipt({ root: seeded, repo: join(dir, 'not-a-repo'),
    managerRoot: join(dir, 'no-manager'), runGit: () => { throw new Error('no git'); },
    runVersion: () => { throw new Error('no edda'); }, fetch: async () => ({ version: 1, startedAt: 'x', agents: 0 }) });
  assert.deepEqual(await snapshot(seeded), beforeSeeded);
  assert.deepEqual(seededReceipt.pi.pinnedReleaseIds.map((entry) => entry.id), [releaseId]);
  assert.equal(seededReceipt.pi.pinnedReleaseIds[0].version, '1');
  assert.deepEqual(seededReceipt.pi.relevantReleaseIds, [releaseId]);
});

test('evaluateCoherence is coherent, drift and fail-closed across unit cases', () => {
  const coherent = evaluateCoherence(legs());
  assert.equal(coherent.status, 'coherent');
  assert.deepEqual(coherent.findings, []);
  assert.equal(coherent.observedLegs, 4);
  assert.equal(coherent.requiredLegs, 4);

  const eddaMismatch = evaluateCoherence(legs((value) => { value.edda.revision = 'f'.repeat(12); }));
  assert.equal(eddaMismatch.status, 'drift');
  assert.deepEqual(eddaMismatch.findings.map((finding) => finding.code), ['edda_revision_mismatch']);
  assert.equal(eddaMismatch.observedLegs, 4);

  const piDrift = evaluateCoherence(legs((value) => { value.pi.installedReleaseId = 'd'.repeat(64); }));
  assert.equal(piDrift.status, 'drift');
  assert.deepEqual(piDrift.findings.map((finding) => finding.code), ['pi_content_drift']);

  const configuredMismatch = evaluateCoherence(legs((value) => { value.manager.configured.headSha = 'e'.repeat(40); }));
  assert.equal(configuredMismatch.status, 'drift');
  assert.deepEqual(configuredMismatch.findings.map((finding) => finding.code), ['manager_configured_revision_mismatch']);
  assert.equal(configuredMismatch.findings[0].leg, 'manager');

  const unhealthy = evaluateCoherence(legs((value) => { value.manager.health = 'unreachable'; }));
  assert.equal(unhealthy.status, 'drift');
  assert.deepEqual(unhealthy.findings.map((finding) => finding.code), ['manager_not_healthy']);
  assert.equal(unhealthy.findings[0].detail, 'unreachable');

  const absentOwner = evaluateCoherence(legs((value) => { value.manager.running = null; }));
  assert.equal(absentOwner.status, 'drift');
  assert.deepEqual(absentOwner.findings.map((finding) => finding.code), ['manager_absent_owner']);

  const partial = evaluateCoherence(legs((value) => { value.manager.observed = false; }));
  assert.equal(partial.status, 'partial');
  assert.deepEqual(partial.findings, []);
  assert.equal(partial.observedLegs, 3);

  const unknown = evaluateCoherence(legs((value) => { for (const leg of Object.values(value)) leg.observed = false; }));
  assert.equal(unknown.status, 'unknown');
  assert.deepEqual(unknown.findings, []);
  assert.equal(unknown.observedLegs, 0);
});

async function startService(t, token, body) {
  const server = createServer((request, response) => {
    if (request.method !== 'GET' || request.url !== '/api/service') { response.writeHead(404); response.end(); return; }
    if (request.headers.authorization !== `Bearer ${token}`) { response.writeHead(401); response.end(); return; }
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end(JSON.stringify(body));
  });
  await new Promise((ready) => server.listen(0, '127.0.0.1', ready));
  t.after(() => new Promise((closed) => server.close(closed)));
  return `http://127.0.0.1:${server.address().port}`;
}

async function git(cwd, args) {
  return (await exec('git', args, { cwd, timeout: 30000 })).stdout.trim();
}

async function activationFixture(t) {
  const dir = await fixture(t);
  const repo = join(dir, 'repo');
  await mkdir(join(repo, 'integrations'), { recursive: true });
  await cp(packageDir, join(repo, 'integrations', 'pi'), { recursive: true });
  await git(repo, ['init']);
  await git(repo, ['add', '-A']);
  await git(repo, ['-c', 'user.email=worker@example.invalid', '-c', 'user.name=worker', 'commit', '-m', 'fixture']);
  const head = await git(repo, ['rev-parse', 'HEAD']);

  const token = '9f7c1a'.repeat(10) + 'dead';
  const origin = await startService(t, token, { version: 1, startedAt: '2026-09-13T00:00:00.000Z', agents: 2 });
  const managerRoot = join(dir, 'manager');
  await mkdir(managerRoot, { recursive: true });
  const release = { version: 1, mergeCommit: head, headSha: head, sourceWorktree: repo,
    entrypoint: join(repo, 'integrations', 'agent-manager', 'dist', 'src', 'cli.js') };
  await writeFile(join(managerRoot, 'release.json'), JSON.stringify(release));
  await writeFile(join(managerRoot, 'owner.json'), JSON.stringify({ version: 1, pid: process.pid, instanceId: randomUUID(),
    origin, token, configDigest: 'f'.repeat(64), startedAt: '2026-09-13T00:00:00.000Z' }));

  const fakeEdda = join(dir, 'fake-edda.mjs');
  await writeFile(fakeEdda, `process.stdout.write(${JSON.stringify(`edda 0.6.1 (${head.slice(0, 12)} 2026-09-13)`)} + '\\n');\n`);
  return { dir, repo, head, managerRoot, registryRoot: join(dir, 'registry'), token, release, fakeEdda, origin };
}

test('end-to-end fixture observes all four legs coherent, and --check exits 0', async (t) => {
  const { repo, head, managerRoot, registryRoot, token, fakeEdda } = await activationFixture(t);
  const receipt = await activationReceipt({ root: registryRoot, repo, managerRoot,
    runVersion: () => `edda 0.6.1 (${head.slice(0, 12)} 2026-09-13)`,
    fetch: async (url, options) => {
      assert.equal(options.headers.Authorization, `Bearer ${token}`);
      return { version: 1, startedAt: '2026-09-13T00:00:00.000Z', agents: 2 };
    } });
  assert.equal(receipt.receiptVersion, RECEIPT_VERSION);
  assert.equal(receipt.repository.headRevision, head);
  assert.equal(receipt.edda.revision, head.slice(0, 12));
  assert.equal(receipt.pi.installedReleaseId, receipt.pi.repoReleaseId);
  assert.equal(receipt.manager.health, 'ok');
  assert.equal(receipt.manager.pidAlive, true);
  assert.equal(receipt.coherence.status, 'coherent');
  assert.deepEqual(receipt.coherence.findings, []);

  const ok = await exec(process.execPath, [cli, '--check', '--json', '--repo', repo, '--registry-root', registryRoot,
    '--manager-root', managerRoot, '--edda-bin', fakeEdda], { timeout: 30000 });
  const parsed = JSON.parse(ok.stdout);
  assert.equal(parsed.coherence.status, 'coherent');
  assert.equal(parsed.receiptVersion, 1);
});

test('drift fixture fails closed: --check exits 2 and the finding is visible', async (t) => {
  const { repo, head, managerRoot, registryRoot, release, fakeEdda } = await activationFixture(t);
  await writeFile(join(managerRoot, 'release.json'), JSON.stringify({ ...release, headSha: '0'.repeat(40) }));
  const receipt = await activationReceipt({ root: registryRoot, repo, managerRoot,
    runVersion: () => `edda 0.6.1 (${head.slice(0, 12)} 2026-09-13)`,
    fetch: async () => ({ version: 1, startedAt: '2026-09-13T00:00:00.000Z', agents: 2 }) });
  assert.equal(receipt.coherence.status, 'drift');
  assert.deepEqual(receipt.coherence.findings.map((finding) => finding.code), ['manager_configured_revision_mismatch']);

  const outcome = await exec(process.execPath, [cli, '--check', '--json', '--repo', repo, '--registry-root', registryRoot,
    '--manager-root', managerRoot, '--edda-bin', fakeEdda], { timeout: 30000 }).catch((error) => error);
  assert.equal(outcome.code, 2);
  const parsed = JSON.parse(outcome.stdout);
  assert.equal(parsed.coherence.status, 'drift');
  assert.ok(parsed.coherence.findings.some((finding) => finding.code === 'manager_configured_revision_mismatch'));
});

test('the receipt never leaks the agent-manager owner token', async (t) => {
  const dir = await fixture(t);
  const managerRoot = join(dir, 'manager'), token = 'a1b2c3d4'.repeat(8);
  await mkdir(managerRoot, { recursive: true });
  await writeFile(join(managerRoot, 'release.json'), JSON.stringify({ version: 1, mergeCommit: null, headSha: null,
    sourceWorktree: dir, entrypoint: join(dir, 'entry.js') }));
  await writeFile(join(managerRoot, 'owner.json'), JSON.stringify({ version: 1, pid: process.pid, instanceId: randomUUID(),
    origin: 'http://127.0.0.1:1', token, configDigest: 'e'.repeat(64), startedAt: '2026-09-13T00:00:00.000Z' }));
  let seen = null;
  const receipt = await activationReceipt({ root: join(dir, 'registry'), repo: join(dir, 'no-repo'), managerRoot,
    runGit: () => { throw new Error('no git'); }, runVersion: () => { throw new Error('no edda'); },
    fetch: async (url, options) => { seen = options.headers.Authorization; return { version: 1, startedAt: 'x', agents: 0 }; } });
  assert.equal(seen, `Bearer ${token}`);
  assert.deepEqual(Object.keys(receipt.manager.running).sort(), ['instanceId', 'origin', 'pid', 'startedAt']);
  assert.doesNotMatch(JSON.stringify(receipt), new RegExp(token));
  assert.doesNotMatch(JSON.stringify(receipt), /configDigest|"token"/);
});

test('an absent environment stays partial or unknown and creates no path', async (t) => {
  const dir = await fixture(t);
  const before = await snapshot(dir);
  const receipt = await activationReceipt({ root: join(dir, 'registry'), repo: join(dir, 'no-repo'),
    managerRoot: join(dir, 'no-manager'), eddaBin: 'edda-does-not-exist',
    runGit: () => { throw new Error('no git'); }, runVersion: () => { throw new Error('no edda'); },
    fetch: async () => { throw new Error('unreachable'); } });
  assert.ok(['partial', 'unknown'].includes(receipt.coherence.status));
  assert.equal(receipt.repository.observed, false);
  assert.equal(receipt.manager.observed, false);
  assert.equal(receipt.pi.observed, true);
  assert.ok(receipt.repository.error && receipt.manager.error && receipt.edda.error);
  assert.deepEqual(await snapshot(dir), before);
  assert.equal(existsSync(join(dir, 'registry')), false);
  assert.equal(existsSync(join(dir, 'no-manager')), false);
});

test('unknown or malformed CLI flags exit 2 on stderr without a receipt', async (t) => {
  const dir = await fixture(t);
  for (const args of [['--nope'], ['--repo'], ['--timeout', 'abc']]) {
    const outcome = await exec(process.execPath, [cli, ...args], { cwd: dir }).catch((error) => error);
    assert.equal(outcome.code, 2);
    assert.ok(outcome.stderr.length > 0);
    assert.equal(outcome.stdout, '');
  }
});

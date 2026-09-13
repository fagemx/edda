// Exercise the installed npm shim, not imports from the checkout. No paid models.
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, cp, rename, writeFile, readFile, rm, access } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { randomUUID } from 'node:crypto';
import { setTimeout as delay } from 'node:timers/promises';

const exec = promisify(execFile), source = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
if (args.length && (args.length !== 2 || args[0] !== '--pi-entry')) throw new Error('Usage: node package-smoke.mjs [--pi-entry actual-Pi-cli.js]');
const root = await mkdtemp(join(tmpdir(), 'edda-package-smoke-'));
const prefix = join(root, 'install'), registry = join(root, 'registry'), project = join(root, 'project'), staged = join(root, 'source');
const env = { ...process.env, EDDA_PI_CHANNEL_DIR: registry, PI_OFFLINE: '1', PI_TELEMETRY: '0' };
const npmCandidates = [process.env.npm_execpath, join(dirname(process.execPath), 'node_modules/npm/bin/npm-cli.js'),
  join(dirname(process.execPath), '../lib/node_modules/npm/bin/npm-cli.js')].filter(Boolean);
let npm;
for (const candidate of npmCandidates) { try { await access(candidate); npm = candidate; break; } catch { /* next installed npm */ } }
if (!npm) throw new Error('Installed npm CLI not found');
const shim = join(prefix, process.platform === 'win32' ? 'edda-pi' : 'bin/edda-pi');
const npmRun = (a) => exec(process.execPath, [npm, ...a], { cwd: root, env, timeout: 90000, windowsHide: true });
async function cli(a, allowUnavailable = false) {
  // npm installs a POSIX shim alongside .cmd on Windows; Git Bash executes that
  // real shim. The independent fresh-user drill also exercises edda-pi.cmd.
  try {
    const out = await exec(process.platform === 'win32' ? 'bash' : shim, process.platform === 'win32' ? [shim, ...a] : a,
      { cwd: project, env, timeout: 45000, windowsHide: true });
    return a[0] === '--version' ? out.stdout.trim() : JSON.parse(out.stdout);
  } catch (error) {
    if (allowUnavailable && error.code === 2 && error.stdout) return JSON.parse(error.stdout);
    throw new Error(`Installed command ${a[0]} failed: ${error.stderr || error.message}`);
  }
}
async function until(fn) {
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) { const result = await fn(); if (result) return result; await delay(150); }
  throw new Error('Timed out waiting for installed-client receipt');
}
const runId = randomUUID();
let launched = false, passed = false, receipt;
try {
  await mkdir(project); await mkdir(prefix); await cp(source, staged, { recursive: true });
  const pack = JSON.parse((await npmRun(['pack', staged, '--pack-destination', root, '--ignore-scripts', '--json'])).stdout)[0];
  const files = pack.files.map((f) => f.path);
  for (const required of ['cli.mjs', 'activation.mjs', 'managed-runner.mjs', 'private-directory.ps1', 'getting-started.md']) assert.ok(files.includes(required), required);
  assert.ok(!files.some((name) => name.includes('.test.') || name.includes('smoke') || name.startsWith('fixtures/')));
  await npmRun(['install', '--global', '--prefix', prefix, '--ignore-scripts', '--no-audit', '--no-fund', join(root, pack.filename)]);
  await rename(staged, join(root, 'source-unavailable'));
  assert.match(await cli(['--version']), /^edda-pi 0\.8\.0$/);
  const info = await cli(['runtime-info'], true);
  assert.ok(info.cli.startsWith(prefix));
  assert.match(await readFile(info.guide, 'utf8'), /new session/i);
  assert.deepEqual((await cli(['runs'])).runs, []);

  const agentDir = join(root, 'agent'); await mkdir(agentDir);
  await writeFile(join(agentDir, 'settings.json'), JSON.stringify({ retry: { enabled: false }, compaction: { enabled: false } }));
  let piEntry = args[1];
  if (!piEntry) {
    const pkg = join(root, 'pi'); await mkdir(join(pkg, 'dist/bundle'), { recursive: true });
    await writeFile(join(pkg, 'package.json'), JSON.stringify({ type: 'module', name: '@earendil-works/pi-coding-agent', version: 'fixture' }));
    piEntry = join(pkg, 'dist/bundle/cli.js'); await cp(join(source, 'fixtures/managed-pi.mjs'), piEntry);
  }
  const prompt = join(root, 'task.txt'); await writeFile(prompt, 'PACKAGE_FIRST');
  const launchArgs = ['launch', '--project', project, '--run-id', runId, '--pi-entry', piEntry, '--agent-dir', agentDir, '--prompt-file', prompt];
  if (args.length) launchArgs.push('--provider', 'edda-offline-test', '--model', 'echo', '--no-tools', '--extension', join(source, 'fixtures/offline-provider.mjs'));
  else launchArgs.push('--provider', 'fixture', '--model', 'echo');
  launched = true;
  const first = await cli(launchArgs);
  assert.equal(first.live, true);
  const sessionId = first.sessionId;
  await until(async () => (await cli(['run-status', runId])).initialReceipt?.status === 'settled');
  const before = (await cli(['conversation', sessionId, '--limit', '50'])).conversation;
  assert.ok(before.entries.some((e) => e.role === 'assistant'));
  assert.equal((await cli(['run-stop', runId])).status, 'stopped');
  const stopped = await cli(['run-status', runId], true);
  assert.equal(stopped.initialReceipt.status, 'settled');
  assert.equal(stopped.initialReceipt.live, false);
  const persisted = await cli(['run-conversation', runId, '--limit', '50']);
  assert.equal(persisted.evidenceSource, 'persisted_session');
  assert.equal(persisted.conversation.headCursor, before.headCursor);
  assert.equal(persisted.conversation.entries.filter((e) => e.role === 'user' && e.text?.includes('PACKAGE_FIRST')).length, 1);
  const inventory = await cli(['runs']);
  assert.equal(inventory.runs[0].runId, runId);
  assert.equal(inventory.runs[0].sessionId, sessionId);
  assert.equal(inventory.runs[0].recordedPhase, 'stopped');
  const resumed = await cli(['run-resume', inventory.runs[0].runId]);
  assert.equal(resumed.sessionId, sessionId);
  assert.equal(resumed.release.id, first.release.id);
  assert.equal((await cli(['conversation', sessionId, '--limit', '50'])).conversation.headCursor, before.headCursor);
  const messageId = randomUUID();
  await cli(['send', sessionId, '--id', messageId, '--message', 'PACKAGE_SECOND']);
  await until(async () => (await cli(['receipt', sessionId, '--id', messageId])).status === 'settled');
  const after = (await cli(['conversation', sessionId, '--limit', '50'])).conversation;
  assert.equal(after.entries.filter((e) => e.role === 'user' && e.text?.includes('PACKAGE_FIRST')).length, 1);
  assert.equal(after.entries.filter((e) => e.role === 'user' && e.text?.includes('PACKAGE_SECOND')).length, 1);
  assert.equal((await cli(['runs'])).runs.length, 1);
  assert.ok((await cli(['inbox'])).events.length > 0);
  receipt = { passed: true, actualPi: Boolean(args.length), modelCallsPaid: false, installedShim: true,
    sourceUnavailable: true, cwdIndependent: true, sameSessionResume: true, initialReplayed: false,
    explicitContinuation: true, packageIntegrity: pack.integrity, sessionId, runId, releaseId: first.release.id };
  passed = true;
} finally {
  if (launched) {
    const state = await cli(['run-status', runId], true);
    if (state.live) await cli(['run-stop', runId, '--abort']);
  }
  if (passed) { console.log(JSON.stringify(receipt, null, 2)); await rm(root, { recursive: true, force: true }); }
  else console.error(`Failure evidence preserved: ${root}`);
}

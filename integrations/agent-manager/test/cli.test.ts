import test from 'node:test';
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { ManagerStore } from '../src/store.js';
import type { AgentBinding, SendRequest } from '../src/contracts.js';
const exec = promisify(execFile), cli = fileURLToPath(new URL('../src/cli.js', import.meta.url));
interface Result { origin: string; pid: number; reused?: boolean; recovered?: boolean }
const runCli = async (root: string, command: string, extra: string[] = []): Promise<Result> => JSON.parse((await exec(process.execPath,
  [cli, command, '--root', root, '--port', '0', ...extra], { windowsHide: true, timeout: 30000 })).stdout.trim()) as Result;
async function stopped(root: string): Promise<void> {
  for (let i = 0; i < 100 && existsSync(join(root, 'service.lock')); i++) await delay(50);
  assert.equal(existsSync(join(root, 'service.lock')), false);
}
async function dead(pid: number): Promise<void> {
  for (let i = 0; i < 100; i++) {
    try { process.kill(pid, 0); }
    catch (error) { if ((error as NodeJS.ErrnoException).code === 'ESRCH') return; throw error; }
    await delay(50);
  }
  assert.fail('Test service did not exit');
}

test('CLI process survives launcher, reconnects, stops without touching agents and restarts same store', async () => {
  const root = mkdtempSync(join(tmpdir(), 'manager-cli-'));
  writeFileSync(join(root, 'config.json'), JSON.stringify({ version: 1, projects: [], agents: [] }));
  const run = async (command: string) => JSON.parse((await exec(process.execPath, [cli, command, '--root', root, '--port', '0'], { windowsHide: true, timeout: 30000 })).stdout.trim()) as { origin?: string; reused?: boolean; agentsStopped?: boolean };
  let active = false;
  try {
    const first = await run('start'); active = true;
    assert.ok(first.origin?.startsWith('http://127.0.0.1:'));
    assert.equal((await run('start')).reused, true);
    assert.equal((await run('stop')).agentsStopped, false); active = false;
    for (let i = 0; i < 30 && (await import('node:fs')).existsSync(join(root, 'service.lock')); i++) await delay(100);
    const second = await run('start'); active = true; assert.ok(second.origin);
    await run('stop'); active = false;
    for (let i = 0; i < 30 && (await import('node:fs')).existsSync(join(root, 'service.lock')); i++) await delay(100);
  } finally {
    if (active) { await run('stop'); await delay(500); }
    rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

test('concurrent launchers converge on one authenticated service', async () => {
  const root = mkdtempSync(join(tmpdir(), 'manager-concurrent-'));
  writeFileSync(join(root, 'config.json'), JSON.stringify({ version: 1, projects: [], agents: [] }));
  try {
    const results = await Promise.all(Array.from({ length: 4 }, () => runCli(root, 'start')));
    assert.equal(new Set(results.map(r => r.pid)).size, 1);
    assert.equal(new Set(results.map(r => r.origin)).size, 1);
    assert.equal((await runCli(root, 'status')).pid, results[0]!.pid);
  } finally {
    if (existsSync(join(root, 'owner.json'))) { await runCli(root, 'stop'); await stopped(root); }
    rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

test('start reclaims an abruptly killed service once, preserving config, native state and pending sends', async () => {
  const root = mkdtempSync(join(tmpdir(), 'manager-crash-')), piRoot = join(root, 'pi');
  mkdirSync(piRoot);
  // The process uses the real manager/store with an observable native transport.
  writeFileSync(join(piRoot, 'store.mjs'), 'export const privateRoot = root => root;');
  writeFileSync(join(piRoot, 'client.mjs'), `import { appendFileSync } from 'node:fs'; import { join } from 'node:path';
    export async function requestSession(root, sessionId, path) {
      appendFileSync(join(root, 'native-calls.jsonl'), JSON.stringify({ path }) + '\\n');
      return { live: true, sessionId, cwd: root, instanceId: '11111111-1111-4111-8111-111111111111', state: 'idle', capabilities: ['send'], heartbeatAt: new Date().toISOString() };
    }
    export async function getReceipt() { return null; }`);
  writeFileSync(join(piRoot, 'managed-client.mjs'), 'export async function managedStatus() { return {}; }');
  const binding: AgentBinding = { id: 'a', name: 'Preserved agent', role: 'manager', projectId: 'p', registryRoot: root,
    workspace: root, sessionId: 'native-session', runId: null, summaryFile: null };
  const configText = JSON.stringify({ version: 1, projects: [{ id: 'p', name: 'Preserved project' }], agents: [binding], refreshMs: 1000 }, null, 2) + '\n';
  writeFileSync(join(root, 'config.json'), configText);
  const nativeState = '{"sessionId":"native-session","transcript":"keep exactly"}\n';
  writeFileSync(join(root, 'native-state.json'), nativeState);
  const extra = ['--pi-root', piRoot];
  let active = false;
  try {
    const first = await runCli(root, 'start', extra); active = true;
    const oldOwner = readFileSync(join(root, 'owner.json'), 'utf8');
    const request: SendRequest = { operationId: randomUUID(), selectionRevision: 'a'.repeat(64), instanceId: '11111111-1111-4111-8111-111111111111',
      basisCursor: null, mode: 'followUp', message: 'Durable intent before interruption' };
    const store = new ManagerStore(root);
    try { store.putSetting('preserved-setting', 'keep'); store.begin(binding, request); }
    finally { store.close(); }
    // This PID belongs to the service just launched by this test, not a stale
    // record offered to the production recovery path.
    process.kill(first.pid, 'SIGKILL'); await dead(first.pid); active = false;
    assert.equal(readFileSync(join(root, 'owner.json'), 'utf8'), oldOwner);
    const restarted = await Promise.all(Array.from({ length: 4 }, () => runCli(root, 'start', extra))); active = true;
    assert.equal(new Set(restarted.map(r => r.pid)).size, 1);
    assert.equal(restarted.filter(r => r.recovered).length, 1);
    assert.notEqual(readFileSync(join(root, 'owner.json'), 'utf8'), oldOwner);
    assert.equal(readFileSync(join(root, 'config.json'), 'utf8'), configText);
    assert.equal(readFileSync(join(root, 'native-state.json'), 'utf8'), nativeState);
    const reopened = new ManagerStore(root);
    try {
      assert.equal(reopened.setting('preserved-setting'), 'keep');
      assert.equal(reopened.operation(request.operationId)?.status, 'unknown');
      assert.equal(reopened.operation(request.operationId)?.message, request.message);
      assert.equal(reopened.target(request.operationId)?.sessionId, binding.sessionId);
      assert.equal(reopened.operations().length, 1);
      assert.ok(reopened.events().some(event => event.operationId === request.operationId));
    } finally { reopened.close(); }
    const calls = readFileSync(join(root, 'native-calls.jsonl'), 'utf8').trim().split('\n').map(line => JSON.parse(line) as { path: string });
    assert.ok(calls.length >= 2);
    assert.ok(calls.every(call => call.path === '/status'), 'Recovery must not replay a send or alter native agents');
  } finally {
    if (active) { await runCli(root, 'stop', extra); await stopped(root); }
    rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

test('recovery refuses mismatched generations, mismatched PIDs, and live or reused PIDs without changing records', async () => {
  const exited = Number((await exec(process.execPath, ['-e', 'console.log(process.pid)'], { windowsHide: true })).stdout.trim());
  await dead(exited);
  for (const scenario of ['generation', 'pid', 'live'] as const) {
    const root = mkdtempSync(join(tmpdir(), 'manager-refuse-'));
    const config = JSON.stringify({ version: 1, projects: [], agents: [] });
    const held = { pid: scenario === 'live' ? process.pid : exited, instanceId: randomUUID() };
    const current = { version: 1, ...held, instanceId: scenario === 'generation' ? randomUUID() : held.instanceId,
      pid: scenario === 'pid' ? process.pid : held.pid, origin: 'http://127.0.0.1:0', token: 'a'.repeat(64), configDigest: 'b'.repeat(64), startedAt: new Date().toISOString() };
    const lockText = JSON.stringify(held), ownerText = JSON.stringify(current);
    writeFileSync(join(root, 'config.json'), config); writeFileSync(join(root, 'service.lock'), lockText); writeFileSync(join(root, 'owner.json'), ownerText);
    try {
      for (const command of ['start', 'recover']) {
        await assert.rejects(runCli(root, command), scenario !== 'live' ? /Owner generation mismatch/ : command === 'recover' ? /Cannot prove service owner is dead/ : /fetch failed/);
        assert.equal(readFileSync(join(root, 'service.lock'), 'utf8'), lockText);
        assert.equal(readFileSync(join(root, 'owner.json'), 'utf8'), ownerText);
        assert.equal(readFileSync(join(root, 'config.json'), 'utf8'), config);
        assert.equal(existsSync(join(root, 'manager.sqlite')), false);
        assert.equal(existsSync(join(root, 'service.log')), false);
      }
    } finally { rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }); }
  }
});

test('start also recovers a dead service that never published readiness', async () => {
  const root = mkdtempSync(join(tmpdir(), 'manager-unready-'));
  const exited = Number((await exec(process.execPath, ['-e', 'console.log(process.pid)'], { windowsHide: true })).stdout.trim());
  await dead(exited);
  writeFileSync(join(root, 'config.json'), JSON.stringify({ version: 1, projects: [], agents: [] }));
  writeFileSync(join(root, 'service.lock'), JSON.stringify({ pid: exited, instanceId: randomUUID() }));
  try { assert.equal((await runCli(root, 'start')).recovered, true); }
  finally {
    if (existsSync(join(root, 'owner.json'))) { await runCli(root, 'stop'); await stopped(root); }
    rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

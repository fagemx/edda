// Usage: node integrations/pi/pi-smoke.mjs /absolute/path/to/pi/dist/bundle/cli.js
// A real Pi subprocess with an offline fixture provider; never contacts a model.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, rm, mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';
import { setTimeout as delay } from 'node:timers/promises';
import { listSessions, requestSession, getReceipt } from './client.mjs';

const entry = process.argv[2];
if (!entry) throw new Error('Supply the installed Pi JavaScript CLI entry path');
const root = await mkdtemp(join(tmpdir(), 'edda-pi-smoke-'));
const registry = join(root, 'channel');
const extension = fileURLToPath(new URL('./extension.mjs', import.meta.url));
const provider = fileURLToPath(new URL('./fixtures/offline-provider.mjs', import.meta.url));
await mkdir(join(root, 'agent'));
const child = spawn(process.execPath, [resolve(entry), '--mode', 'rpc', '--no-session',
  '--no-extensions', '-e', extension, '-e', provider, '--no-skills', '--no-prompt-templates',
  '--no-themes', '--no-tools', '--provider', 'edda-offline-test', '--model', 'echo'], {
  cwd: root, windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'],
  env: { ...process.env, PI_CODING_AGENT_DIR: join(root, 'agent'), EDDA_PI_CHANNEL_DIR: registry },
});
let stderr = '';
let buffered = '';
let processError;
const events = [];
child.on('error', (error) => { processError = error; });
child.stderr.on('data', (chunk) => { stderr = (stderr + chunk).slice(-8000); });
child.stdout.on('data', (chunk) => {
  buffered += chunk.toString('utf8');
  let end;
  while ((end = buffered.indexOf('\n')) !== -1) {
    const line = buffered.slice(0, end);
    buffered = buffered.slice(end + 1);
    try { events.push(JSON.parse(line)); } catch { /* Pi may emit startup diagnostics. */ }
  }
});
async function until(fn, label, timeoutMs = 25000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (processError) throw processError;
    if (child.exitCode !== null) throw new Error(`Pi exited ${child.exitCode}: ${stderr}`);
    const value = await fn();
    if (value) return value;
    await delay(100);
  }
  throw new Error(`Timed out: ${label}. Pi stderr: ${stderr}`);
}
try {
  // Some RPC runtimes bind their extension UI during the initial state exchange.
  child.stdin.write(JSON.stringify({ id: 'initial', type: 'get_state' }) + '\n');
  const session = await until(async () => (await listSessions(registry)).find((row) => row.live), 'registration');
  const firstId = randomUUID();
  const first = await requestSession(registry, session.sessionId, '/messages', {
    id: firstId, message: 'CHANNEL_SMOKE_ONE', sender: 'codex-smoke', mode: 'followUp',
  });
  assert.ok(['queued', 'started', 'settled'].includes(first.status));
  await until(async () => (await getReceipt(registry, session.sessionId, firstId)).status === 'settled', 'first settlement');
  const secondId = randomUUID();
  await requestSession(registry, session.sessionId, '/messages', {
    id: secondId, message: 'CHANNEL_SMOKE_TWO', sender: 'codex-smoke', mode: 'steer',
  });
  await until(async () => (await getReceipt(registry, session.sessionId, secondId)).status === 'settled', 'second settlement');
  const messages = events.filter((event) => event.type === 'message_end' && event.message?.role === 'assistant');
  assert.equal(messages.length, 2);
  assert.ok(messages[0].message.content.some((p) => p.text?.includes('OFFLINE_ACK:') && p.text.includes('CHANNEL_SMOKE_ONE')));
  assert.ok(messages[1].message.content.some((p) => p.text?.includes('CHANNEL_SMOKE_TWO')));
  assert.ok(messages.every((m) => m.message.provider === 'edda-offline-test'));
  const final = await requestSession(registry, session.sessionId, '/status');
  assert.equal(final.sessionId, session.sessionId);
  assert.equal(final.state, 'idle');
  console.log(JSON.stringify({ passed: true, actualPi: true, sessionId: session.sessionId,
    messagesSettled: 2, sameSession: true, provider: 'offline fixture', paidCalls: 0 }, null, 2));
} finally {
  // Only the subprocess created above is stopped; no running user session is touched.
  if (child.exitCode === null) {
    child.kill();
    await new Promise((resolve) => child.once('exit', resolve));
  }
  await rm(root, { recursive: true, force: true });
}

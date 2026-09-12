import test from 'node:test';
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
const exec = promisify(execFile), cli = fileURLToPath(new URL('../src/cli.js', import.meta.url));

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

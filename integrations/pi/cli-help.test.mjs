import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const cli = fileURLToPath(new URL('./cli.mjs', import.meta.url));
const run = (args) => spawnSync(process.execPath, [cli, ...args], { encoding: 'utf8' });

test('per-verb --help prints that verb usage and exits 0', () => {
  for (const verb of ['launch', 'follow', 'run-status']) {
    const result = run([verb, '--help']);
    assert.equal(result.status, 0, `${verb} --help exit status (stderr: ${result.stderr})`);
    assert.match(result.stdout, new RegExp(`Usage: edda-pi ${verb}\\b`));
  }
  const follow = run(['follow', '--help']).stdout;
  assert.match(follow, /--project PATH/);
  assert.match(follow, /--tasks ID,ID/);
  assert.match(run(['launch', '--help']).stdout, /--owner REF/);
});

test('an unknown verb with --help names where the supported usage lives', () => {
  const result = run(['not-a-verb', '--help']);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /Unknown command 'not-a-verb'/);
  assert.match(result.stderr, /edda-pi --help/);
});

test('top-level --help still prints the full guide and exits 0', () => {
  const result = run(['--help']);
  assert.equal(result.status, 0);
  assert.match(result.stdout, /Edda Pi session channel/);
  assert.match(result.stdout, /edda-pi launch --project PATH/);
});

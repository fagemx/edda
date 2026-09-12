import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { ExperimentPi } from './fork-smoke-runtime.mjs';
const entry = process.argv[2];
if (!entry) throw new Error('Supply installed Pi entry');
const root = mkdtempSync(join(tmpdir(), 'edda-fork-tools-'));
writeFileSync(join(root, 'CONTRACT.md'), 'Fixture only.');
writeFileSync(join(root, 'cards.mjs'), '// fixture');
writeFileSync(join(root, '.fork-bundle.json'), JSON.stringify({ module: 'cards', receipts: root, acceptance: join(root, 'unused.mjs') }));
const pi = new ExperimentPi({ entry, cwd: root, dir: join(root, 'sessions'), provider: 'edda-fork-fixture', model: 'echo', thinking: 'off',
  tools: ['fork_read', 'fork_submit'],
  extensions: ['fork-provider.mjs', 'fork-bundle-tools.mjs'].map((name) => fileURLToPath(new URL(`./fixtures/${name}`, import.meta.url))) });
try {
  await pi.ready(); await pi.prompt('TOOLS', 15000);
  const result = JSON.parse((await pi.rpc('get_last_assistant_text')).text);
  assert.deepEqual(result.tools, ['fork_read', 'fork_submit']);
  console.log(JSON.stringify({ passed: true, actualPi: true, providerVisibleTools: result.tools, builtinsAbsent: true, root }));
} finally { await pi.stop(); }

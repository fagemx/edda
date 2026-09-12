import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync, readdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import bundleTools from './fixtures/fork-bundle-tools.mjs';

test('experimental submission gives real failure feedback and writes only the assigned output', async () => {
  const root = mkdtempSync(join(tmpdir(), 'edda-fork-bundle-')), prior = process.cwd();
  const receipts = join(root, 'receipts'); mkdirSync(receipts);
  const acceptance = join(root, 'acceptance.mjs');
  writeFileSync(acceptance, "import assert from 'node:assert/strict'; import {pathToFileURL} from 'node:url'; import {join} from 'node:path'; const {answer}=await import(pathToFileURL(join(process.argv[2],'cards.mjs'))); assert.equal(answer(),42); console.log('PASS');\n");
  writeFileSync(join(root, 'cards.mjs'), '// stub\n');
  writeFileSync(join(root, 'CONTRACT.md'), 'Return 42.');
  writeFileSync(join(root, '.fork-bundle.json'), JSON.stringify({ module: 'cards', acceptance, receipts }));
  const tools = new Map(); let before, active;
  try {
    process.chdir(root);
    bundleTools({ on: (name, fn) => { if (name === 'before_agent_start') before = fn; },
      setActiveTools: (names) => { active = names; }, registerTool: (tool) => tools.set(tool.name, tool) });
    before(); assert.deepEqual(active, ['fork_read', 'fork_submit']);
    const read = JSON.parse((await tools.get('fork_read').execute()).content[0].text);
    assert.equal(read.contract, 'Return 42.'); assert.equal(read.module, 'cards');
    const submit = async (code) => JSON.parse((await tools.get('fork_submit').execute('id', { code })).content[0].text);
    assert.equal((await submit('export function answer(){return 0;}')).accepted, false);
    assert.equal((await submit('export function answer(){return 42;}')).accepted, true);
    assert.equal(JSON.parse(readFileSync(join(receipts, 'submission-2.json'), 'utf8')).accepted, true);
    assert.equal(readFileSync(join(root, 'CONTRACT.md'), 'utf8'), 'Return 42.');
    await assert.rejects(() => submit('x'.repeat(65537)), /Invalid experimental output/);
    assert.equal((await submit('export function answer(){return 42;}')).accepted, true);
    assert.equal((await submit('export function answer(){return 99;}')).stopped, true);
    assert.match(readFileSync(join(root, 'cards.mjs'), 'utf8'), /return 42/);
    assert.deepEqual(readdirSync(root).sort(), ['.fork-bundle.json', 'CONTRACT.md', 'acceptance.mjs', 'cards.mjs', 'receipts'].sort());
  } finally { process.chdir(prior); rmSync(root, { recursive: true, force: true }); }
});

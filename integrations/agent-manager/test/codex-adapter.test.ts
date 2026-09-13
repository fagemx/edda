import test from 'node:test';
import assert from 'node:assert/strict';
import { appendFileSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { CodexAdapter } from '../src/codex-adapter.js';
import { ManagerStore } from '../src/store.js';
import type { AgentBinding } from '../src/contracts.js';

const at = '2026-09-13T01:00:00.000Z';
const row = (type: string, payload: unknown): string => JSON.stringify({ timestamp: at, type, payload }) + '\n';
function fixture() {
  const root = mkdtempSync(join(tmpdir(), 'codex-observer-'));
  const file = join(root, 'rollout.jsonl');
  const binding: AgentBinding = { id: 'codex', name: 'Codex', projectId: 'project', role: 'worker', registryRoot: root,
    runId: null, sessionId: 'thread-1', workspace: root, summaryFile: null, transport: 'codex', transcriptFile: file };
  const header = row('session_meta', { id: binding.sessionId, session_id: binding.sessionId, cwd: root, base_instructions: 'PRIVATE_INSTRUCTIONS' });
  writeFileSync(file, header);
  const store = new ManagerStore(join(root, 'store'));
  return { root, file, binding, header, store, adapter: new CodexAdapter(store), close() { store.close(); rmSync(root, { recursive: true, force: true }); } };
}

test('Codex lifecycle, public-only projection, append restart and stable event IDs', async () => {
  const f = fixture();
  try {
    appendFileSync(f.file, row('event_msg', { type: 'task_started', turn_id: 'turn-1' }) +
      row('response_item', { type: 'reasoning', text: 'PRIVATE_REASONING' }) +
      row('response_item', { type: 'message', role: 'assistant', channel: 'analysis', content: [{ type: 'output_text', text: 'PRIVATE_ANALYSIS' }] }) +
      row('response_item', { type: 'function_call', arguments: 'PRIVATE_ARGS' }) +
      row('event_msg', { type: 'item_completed', item: { type: 'McpToolCall', arguments: 'PRIVATE_ARGS', result: 'PRIVATE_RESULT' } }) +
      row('response_item', { type: 'message', role: 'assistant', channel: 'final', content: [{ type: 'output_text', text: '<b>Public literal text</b>' }] }));
    const first = await f.adapter.observe(f.binding);
    assert.equal(first.state, 'running'); assert.equal(first.source, 'recorded'); assert.equal(first.heartbeatAt, null);
    assert.equal(first.capabilities.send, false); assert.equal(first.latestMessage?.text, '<b>Public literal text</b>');
    assert.ok(!JSON.stringify(first).includes('PRIVATE'));
    const reopened = new ManagerStore(join(f.root, 'store'));
    const second = await new CodexAdapter(reopened).observe(f.binding);
    reopened.close();
    assert.deepEqual(second.sessionEvidence, first.sessionEvidence);
    appendFileSync(f.file, row('event_msg', { type: 'task_complete', turn_id: 'turn-1', last_agent_message: 'NOT_CANONICAL_DELIVERY' }));
    const done = await f.adapter.observe(f.binding);
    assert.equal(done.state, 'idle'); assert.equal(done.sessionEvidence?.events.length, 2);
    assert.equal(done.sessionEvidence?.events[0]?.id, first.sessionEvidence?.events[0]?.id);
    appendFileSync(f.file, row('event_msg', { type: 'turn_aborted', turn_id: 'turn-2', reason: 'interrupted' }) +
      row('event_msg', { type: 'item_completed', item: { type: 'SubAgentActivity', agent_thread_id: 'child-1', agent_path: '/private/path' } }));
    const aborted = await f.adapter.observe(f.binding);
    assert.equal(aborted.state, 'unknown'); assert.equal(aborted.sessionEvidence?.events.at(-1)?.childSessionId, 'child-1');
    assert.ok(!JSON.stringify(aborted).includes('/private/path'));
    const conversation = await f.adapter.conversation(f.binding);
    assert.equal(conversation.entries.length, 1);
    assert.equal((await f.adapter.conversation(f.binding, conversation.cursor!)).entries.length, 0);
    await assert.rejects(f.adapter.send(), { code: 'UNSUPPORTED' }); assert.equal(await f.adapter.receipt(), null);
  } finally { f.close(); }
});

test('Codex partial and corrupt lines recover, oversized records bounded, truncation changes incarnation', async () => {
  const f = fixture();
  try {
    const partial = row('event_msg', { type: 'task_started', turn_id: 'turn-1' });
    appendFileSync(f.file, partial.slice(0, -1));
    assert.equal((await f.adapter.observe(f.binding)).sessionEvidence?.events.length, 0);
    appendFileSync(f.file, '\nNOT JSON\n');
    const recovered = await f.adapter.observe(f.binding);
    assert.equal(recovered.sessionEvidence?.events.length, 1); assert.equal(recovered.sessionEvidence?.historyComplete, false);
    appendFileSync(f.file, 'x'.repeat(900_000));
    await f.adapter.observe(f.binding);
    appendFileSync(f.file, '\n' + row('event_msg', { type: 'task_complete', turn_id: 'turn-1' }));
    const skipped = await f.adapter.observe(f.binding);
    assert.equal(skipped.state, 'idle'); assert.match(skipped.reason!, /紀錄/);
    writeFileSync(f.file, f.header + row('event_msg', { type: 'task_started', turn_id: 'new-turn' }));
    const reset = await f.adapter.observe(f.binding);
    assert.notEqual(reset.instanceId, recovered.instanceId); assert.equal(reset.sessionEvidence?.events.length, 1);
    assert.equal(reset.sessionEvidence?.events[0]?.turnId, 'new-turn');
    writeFileSync(f.file, f.header + row('event_msg', { type: 'task_started', turn_id: 'alt-turn' }));
    const rewritten = await f.adapter.observe(f.binding);
    assert.notEqual(rewritten.instanceId, reset.instanceId);
    assert.equal(rewritten.sessionEvidence?.events[0]?.turnId, 'alt-turn');
    await assert.rejects(f.adapter.conversation(f.binding, 'old-cursor'), { code: 'STALE_CURSOR' });
  } finally { f.close(); }
});

test('Codex selected identity/workspace, missing source and path links are isolated', async () => {
  const f = fixture();
  try {
    await assert.rejects(f.adapter.observe({ ...f.binding, sessionId: 'other' }), { code: 'IDENTITY_MISMATCH' });
    await assert.rejects(f.adapter.observe({ ...f.binding, workspace: join(f.root, 'other') }), { code: 'IDENTITY_MISMATCH' });
    await assert.rejects(f.adapter.observe({ ...f.binding, transcriptFile: 'relative.jsonl' }), { code: 'INVALID_SOURCE' });
    await assert.rejects(f.adapter.observe({ ...f.binding, transcriptFile: join(f.root, 'missing') }), { code: 'SOURCE_UNAVAILABLE' });
    const linkedRoot = join(f.root, 'linked');
    symlinkSync(join(f.root, 'store'), linkedRoot, process.platform === 'win32' ? 'junction' : 'dir');
    writeFileSync(join(f.root, 'store', 'rollout.jsonl'), f.header);
    await assert.rejects(f.adapter.observe({ ...f.binding, transcriptFile: join(linkedRoot, 'rollout.jsonl') }), { code: 'INVALID_SOURCE' });
    assert.equal((await f.adapter.observe(f.binding)).source, 'recorded');
  } finally { f.close(); }
});

test('Codex tail bootstrap reports incomplete history and caps projections', async () => {
  const f = fixture();
  try {
    const records: string[] = [];
    for (let i = 0; i < 300; i++) records.push(row('event_msg', { type: 'task_started', turn_id: `turn-${i}` }),
      row('response_item', { type: 'message', role: 'user', content: [{ type: 'input_text', text: `public-${i}:` + 'x'.repeat(4000) }] }));
    appendFileSync(f.file, records.join(''));
    const observed = await f.adapter.observe(f.binding);
    assert.equal(observed.sessionEvidence?.historyComplete, false);
    assert.equal(observed.sessionEvidence?.events.length, 128);
    assert.equal((await f.adapter.conversation(f.binding)).entries.length, 64);
    assert.ok(!JSON.stringify(observed).includes('PRIVATE_INSTRUCTIONS'));
  } finally { f.close(); }
});

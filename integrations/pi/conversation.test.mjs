import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm, mkdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
import { projectEntry, pageConversation, readTranscript, findTranscript } from './conversation.mjs';

const entry = (id, parentId, role, content) => ({ type: 'message', id, parentId, message: { role, content } });
test('conversation projection exposes replies and tool evidence, never private reasoning or arguments', () => {
  const projected = projectEntry(entry('a', null, 'assistant', [
    { type: 'thinking', thinking: 'private reasoning' }, { type: 'text', text: 'Need approval for test database' },
    { type: 'toolCall', name: 'bash', id: 'tool1', arguments: { token: 'secret' } },
  ]));
  assert.equal(projected.text, 'Need approval for test database');
  assert.deepEqual(projected.toolCalls, [{ id: 'tool1', name: 'bash' }]);
  assert.ok(!JSON.stringify(projected).includes('secret'));
  assert.ok(!JSON.stringify(projected).includes('private reasoning'));
});

test('cursor pagination does not skip unread replies; stale branch cursor refuses', () => {
  const entries = ['a', 'b', 'c'].map((id, i) => projectEntry(entry(id, i ? ['a', 'b'][i - 1] : null, 'assistant', id)));
  const page = pageConversation(entries, { after: 'a', limit: 1 });
  assert.equal(page.cursor, 'b');
  assert.equal(page.headCursor, 'c');
  assert.equal(page.hasMore, true);
  assert.equal(pageConversation(entries, { after: 'b' }).entries[0].text, 'c');
  assert.throws(() => pageConversation(entries, { after: 'abandoned' }), /not on this branch/);
});

test('legacy reader verifies identity and follows persisted ancestry excluding abandoned branch', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'edda-transcript-test-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const sid = randomUUID();
  const safe = `--${resolve(root).replace(/^[/\\]/, '').replace(/[/\\:]/g, '-')}--`;
  const dir = join(root, 'sessions', safe);
  await mkdir(dir, { recursive: true });
  const path = join(dir, `2026-09-12_${sid}.jsonl`);
  const header = { type: 'session', id: sid, cwd: root };
  const entries = [header, entry('a', null, 'user', 'continue'), entry('old', 'a', 'assistant', 'abandoned'),
    entry('b', 'a', 'assistant', 'Need explicit test-DB approval')];
  await writeFile(path, entries.map((e) => JSON.stringify(e)).join('\n') + '\n');
  assert.equal(await findTranscript(sid, root, { agentDir: root }), path);
  const result = await readTranscript(path);
  assert.deepEqual(result.entries.map((e) => e.id), ['a', 'b']);
  assert.equal(result.source, 'pi_transcript');
  await writeFile(path, JSON.stringify({ ...header, id: randomUUID() }) + '\n');
  await assert.rejects(findTranscript(sid, root, { agentDir: root }), /identity/);
});

test('partial writes and broken ancestry never become a misleading completed response', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'edda-transcript-bad-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const path = join(root, 'test.jsonl');
  await writeFile(path, JSON.stringify({ type: 'session', id: randomUUID(), cwd: root }) + '\n{"type":');
  await assert.rejects(readTranscript(path), /incomplete/);
  await writeFile(path, '{}\n' + JSON.stringify(entry('a', 'missing', 'assistant', 'done')) + '\n');
  await assert.rejects(readTranscript(path), /ancestry/);
});

import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { writeJson } from './store.mjs';

// Bounded fault model of the observed NTFS behaviour (GH-715 and the 2026-09-13
// managed-registry incident): a file whose data was written but never flushed
// reads back as all NUL once the metadata records its length. `durable` is what a
// reader sees; `pending` is written-but-unflushed data. A flush promotes pending
// to durable, and a rename moves whatever is durable, so an unflushed rename moves
// zeros. This is a model, not a power loss: it pins the durability barrier.
function crashModel() {
  const durable = new Map(), pending = new Map();
  const dirsFlushed = [];
  return {
    durable,
    dirsFlushed,
    open(path) { pending.set(path, ''); durable.set(path, ''); return { path }; },
    write(fd, text) { pending.set(fd.path, text); durable.set(fd.path, '\0'.repeat(text.length)); },
    flush(fd) { durable.set(fd.path, pending.get(fd.path) ?? ''); },
    close() {},
    rename(from, to) {
      durable.set(to, durable.get(from) ?? ''); pending.set(to, pending.get(from) ?? '');
      durable.delete(from); pending.delete(from);
    },
    unlink(path) { durable.delete(path); pending.delete(path); },
    flushDir(dir) { dirsFlushed.push(dir); },
  };
}

test('writeJson flushes the tmp before rename: a crash leaves the complete record, not NUL', () => {
  const io = crashModel();
  writeJson('C:/registry/managed/run/state.json', { runId: 'run', phase: 'running' }, false, io);
  const text = io.durable.get('C:/registry/managed/run/state.json');
  assert.ok(text && text.length > 0, 'the record was renamed into place');
  assert.ok(!text.split('').every((c) => c === '\0'), 'the renamed record must not be all NUL');
  assert.deepEqual(JSON.parse(text), { runId: 'run', phase: 'running' });
  assert.equal(io.dirsFlushed.length, 1, 'the renamed record is followed by a parent-directory flush');
});

test('the exclusive write is flushed too, so a crash cannot leave an all-NUL record', () => {
  const io = crashModel();
  writeJson('C:/registry/owner.json', { instanceId: 'i' }, true, io);
  const text = io.durable.get('C:/registry/owner.json');
  // Unobservable without the flush: the model leaves a direct, unflushed write zero-filled.
  assert.ok(text && !text.split('').every((c) => c === '\0'), 'the exclusive record must not be all NUL');
  assert.deepEqual(JSON.parse(text), { instanceId: 'i' });
  assert.equal(io.dirsFlushed.length, 1, 'a create also flushes its parent directory');
});

test('the legacy tmp+rename without a flush reproduces the all-NUL record', () => {
  const io = crashModel();
  // The pre-correction algorithm: write the tmp, rename, never flush.
  const fd = io.open('state.json.tmp');
  io.write(fd, '{"phase":"running"}\n');
  io.rename('state.json.tmp', 'C:/registry/managed/run/state.json');
  const text = io.durable.get('C:/registry/managed/run/state.json');
  assert.ok(text.length > 0 && text.split('').every((c) => c === '\0'),
    'an unflushed rename reproduces the all-NUL record the incident observed');
});

test('the real fileIo path (fsync + parent-directory flush) writes a parseable record', () => {
  const dir = mkdtempSync(join(tmpdir(), 'pi-state-durability-'));
  try {
    const path = join(dir, 'state.json');
    writeJson(path, { runId: 'run', phase: 'running' }); // default fileIo: real fsyncSync + flushDir
    assert.deepEqual(JSON.parse(readFileSync(path, 'utf8')), { runId: 'run', phase: 'running' });
    assert.ok(!readdirSync(dir).some((name) => name.endsWith('.tmp')), 'no tmp file left behind');
    const created = join(dir, 'owner.json');
    writeJson(created, { instanceId: 'i' }, true); // exclusive branch, also real fileIo
    assert.deepEqual(JSON.parse(readFileSync(created, 'utf8')), { instanceId: 'i' });
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

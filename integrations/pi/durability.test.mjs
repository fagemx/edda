import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { writeJson, RecordWriteError } from './store.mjs';

// Bounded fault model of the observed NTFS behaviour (GH-715 and the 2026-09-13
// managed-registry incident): a file whose data was written but never flushed
// reads back as all NUL once the metadata records its length. `durable` is what a
// reader sees; `pending` is written-but-unflushed data. A flush promotes pending
// to durable, and a rename moves whatever is durable, so an unflushed rename moves
// zeros. This is a model, not a power loss: it pins the durability barrier.
//
// `flush` can be replaced with a no-op, which turns the *production* `writeJson`
// into the pre-1191 algorithm (writeFileSync never fsyncs) without a second copy
// of the writer: that is how the legacy reproduction below stays product code.
function crashModel({ flush = true } = {}) {
  const durable = new Map(), pending = new Map();
  const dirsFlushed = [], sleeps = [];
  return {
    durable,
    dirsFlushed,
    sleeps,
    // No real waiting in tests; the retry policy is asserted through call counts.
    sleep(ms) { sleeps.push(ms); },
    open(path) { pending.set(path, ''); durable.set(path, ''); return { path }; },
    write(fd, text) { pending.set(fd.path, text); durable.set(fd.path, '\0'.repeat(text.length)); },
    flush(fd) { if (flush) durable.set(fd.path, pending.get(fd.path) ?? ''); },
    close() {},
    rename(from, to) {
      durable.set(to, durable.get(from) ?? ''); pending.set(to, pending.get(from) ?? '');
      durable.delete(from); pending.delete(from);
    },
    unlink(path) { durable.delete(path); pending.delete(path); },
    flushDir(dir) { dirsFlushed.push(dir); },
  };
}

function errno(code, op = 'rename') {
  const error = new Error(`${code}: operation not permitted, ${op}`);
  error.code = code;
  return error;
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

test('the pre-1191 algorithm — the same writeJson with the flush disabled — reproduces the all-NUL record', () => {
  const io = crashModel({ flush: false });
  // Production `writeJson` on the production path, with only the durability
  // barrier removed: this is exactly the pre-1191 writer (writeFileSync + rename).
  writeJson('C:/registry/managed/run/state.json', { phase: 'running' }, false, io);
  const text = io.durable.get('C:/registry/managed/run/state.json');
  assert.ok(text.length > 0 && text.split('').every((c) => c === '\0'),
    'an unflushed rename reproduces the all-NUL record the incident observed');
});

test('a transient EPERM/EBUSY on the Windows replace is retried until the record lands', () => {
  const io = crashModel();
  let calls = 0;
  const realRename = io.rename;
  io.rename = (from, to) => {
    calls += 1;
    if (calls <= 2) throw errno(calls === 1 ? 'EPERM' : 'EBUSY');
    realRename(from, to);
  };
  writeJson('C:/registry/managed/run/state.json', { runId: 'run', phase: 'running' }, false, io);
  assert.equal(calls, 3, 'the replace was retried past the two transient failures');
  assert.deepEqual(JSON.parse(io.durable.get('C:/registry/managed/run/state.json')), { runId: 'run', phase: 'running' });
  assert.deepEqual(io.sleeps, [15, 35], 'the bounded backoff pauses were used');
});

test('an exhausted retry throws a typed record_write_failed and never fakes success', () => {
  const io = crashModel();
  let calls = 0;
  io.rename = () => { calls += 1; throw errno('EPERM'); };
  assert.throws(
    () => writeJson('C:/registry/managed/run/state.json', { runId: 'run' }, false, io),
    (error) => error instanceof RecordWriteError
      && error.code === 'record_write_failed'
      && error.errno === 'EPERM'
      && error.record === 'state.json',
  );
  assert.equal(calls, 6, 'the bounded window was fully used before giving up');
  assert.ok(!io.durable.has('C:/registry/managed/run/state.json'),
    'no partial record is presented as the completed write');
});

test('a non-retryable rename failure propagates unchanged and is not retried', () => {
  const io = crashModel();
  let calls = 0;
  io.rename = () => { calls += 1; throw errno('ENOSPC'); };
  assert.throws(() => writeJson('C:/registry/managed/run/state.json', {}, false, io), (error) => error.code === 'ENOSPC');
  assert.equal(calls, 1);
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

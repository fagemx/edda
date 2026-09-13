import test from 'node:test';
import assert from 'node:assert/strict';
import { writeJson } from './store.mjs';

// Bounded fault model of the observed NTFS behaviour (GH-715 and the 2026-09-13
// managed-registry incident): a hard interruption can leave a file whose recorded
// length is the new one but whose data pages were never flushed, so it reads back
// as all NUL. This model renames a tmp whose data was not flushed to a
// zero-filled target; a flushed tmp keeps its bytes. It is a model, not a power
// loss: it exists to pin the durability barrier `writeJson` must hold.
function crashModel() {
  const files = new Map(), flushed = new Set();
  return {
    files,
    open: (path) => { files.set(path, ''); return { path }; },
    write: (fd, text) => { files.set(fd.path, text); },
    flush: (fd) => { flushed.add(fd.path); },
    close: () => {},
    rename: (from, to) => {
      const text = files.get(from) ?? '';
      files.set(to, flushed.has(from) ? text : '\0'.repeat(text.length));
      files.delete(from);
    },
    unlink: (path) => { files.delete(path); },
    flushDir: () => {},
  };
}

test('writeJson flushes the tmp before rename: a crash leaves the complete record, not NUL', () => {
  const io = crashModel();
  writeJson('C:/registry/managed/run/state.json', { runId: 'run', phase: 'running' }, false, io);
  const text = io.files.get('C:/registry/managed/run/state.json');
  assert.ok(text && text.length > 0, 'the record was renamed into place');
  assert.ok(!text.split('').every((c) => c === '\0'), 'the renamed record must not be all NUL');
  assert.deepEqual(JSON.parse(text), { runId: 'run', phase: 'running' });
});

test('the exclusive write is flushed too, so a crash cannot leave an all-NUL record', () => {
  const io = crashModel();
  writeJson('C:/registry/owner.json', { instanceId: 'i' }, true, io);
  const text = io.files.get('C:/registry/owner.json');
  assert.ok(text && !text.split('').every((c) => c === '\0'));
  assert.deepEqual(JSON.parse(text), { instanceId: 'i' });
});

test('the legacy tmp+rename without a flush reproduces the all-NUL record', () => {
  const io = crashModel();
  // The pre-correction algorithm: write the tmp, rename, never flush.
  const fd = io.open('state.json.tmp');
  io.write(fd, '{"phase":"running"}\n');
  io.rename('state.json.tmp', 'C:/registry/managed/run/state.json');
  const text = io.files.get('C:/registry/managed/run/state.json');
  assert.ok(text.length > 0 && text.split('').every((c) => c === '\0'),
    'an unflushed rename reproduces the all-NUL record the incident observed');
});

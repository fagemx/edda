import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { randomUUID } from 'node:crypto';
import { createOwnerMailbox } from './owner-mailbox.mjs';
import { startChannel } from './channel.mjs';

const exec = promisify(execFile);
const cli = (name) => ({ file: process.execPath, args: [fileURLToPath(new URL(`./fixtures/${name}`, import.meta.url))] });
const fixture = () => cli('edda-return.mjs');
const owner = 'assistant/project';

async function postReturn(cwd, { work, session = 'controller-1', status = 'done', result = 'ok', deliverable = 'out.md' }) {
  await exec(process.execPath, [fixture().args[0], 'return', 'post', '--owner', owner, '--work', work,
    '--status', status, '--result', result, '--deliverable', deliverable, '--session', session],
  { cwd, windowsHide: true, encoding: 'utf8' });
}
async function project(t) {
  const cwd = await mkdtemp(join(tmpdir(), 'edda-owner-mailbox-'));
  t.after(() => rm(cwd, { recursive: true, force: true }));
  return cwd;
}

test('bind records the owner and holder; the same session is idempotent', async (t) => {
  const cwd = await project(t);
  const mailbox = createOwnerMailbox({ owner, sessionId: 'session-a', cwd, command: fixture() });
  assert.deepEqual(await mailbox.bind(), { status: 'bound', holder: 'current', replaced: false });
  assert.deepEqual(mailbox.state(), { owner, status: 'bound', replaced: false });
  const again = createOwnerMailbox({ owner, sessionId: 'session-a', cwd, command: fixture() });
  assert.deepEqual(await again.bind(), { status: 'bound', holder: 'current', replaced: false });
});

test('replacement rebinds explicitly from the persisted owner record', async (t) => {
  const cwd = await project(t);
  const a = createOwnerMailbox({ owner, sessionId: 'session-a', cwd, command: fixture() });
  assert.equal((await a.bind()).status, 'bound');
  const b = createOwnerMailbox({ owner, sessionId: 'session-b', cwd, command: fixture() });
  assert.deepEqual(await b.bind(), { status: 'replaced', holder: 'current', replaced: true });
  assert.equal(b.state().status, 'replaced');
});

test('claim is exactly once and a superseded session is refused', async (t) => {
  const cwd = await project(t);
  const a = createOwnerMailbox({ owner, sessionId: 'session-a', cwd, command: fixture() });
  await a.bind();
  await postReturn(cwd, { work: 'job-1' });
  const first = await a.claim();
  assert.equal(first.status, 'ok');
  assert.equal(first.returns.length, 1);
  assert.equal(first.returns[0].work, 'job-1');
  assert.deepEqual(await a.claim(), { status: 'empty', returns: [] });

  await postReturn(cwd, { work: 'job-2' });
  const b = createOwnerMailbox({ owner, sessionId: 'session-b', cwd, command: fixture() });
  await b.bind();
  assert.deepEqual(await a.claim(), { status: 'superseded', returns: [] });
  const claimed = await b.claim();
  assert.equal(claimed.status, 'ok');
  assert.equal(claimed.returns[0].work, 'job-2');
});

test('a duplicate replay keeps the same content hash and is idempotent', async (t) => {
  const cwd = await project(t);
  const a = createOwnerMailbox({ owner, sessionId: 'session-a', cwd, command: fixture() });
  await a.bind();
  await postReturn(cwd, { work: 'job-1' });
  await postReturn(cwd, { work: 'job-1' });
  const claimed = await a.claim();
  assert.equal(claimed.status, 'ok');
  assert.equal(claimed.returns.length, 1);
});

test('malformed, partial or absent output fails closed', async (t) => {
  const cwd = await project(t);
  const broken = createOwnerMailbox({ owner, sessionId: 'session-a', cwd, command: cli('edda-return-broken.mjs') });
  assert.deepEqual(await broken.claim(), { status: 'unavailable', returns: [] });
  const partial = createOwnerMailbox({ owner, sessionId: 'session-a', cwd, command: cli('edda-return-partial.mjs') });
  assert.deepEqual(await partial.claim(), { status: 'unavailable', returns: [] });
  const missing = createOwnerMailbox({ owner, sessionId: 'session-a', cwd,
    command: { file: 'edda-binary-that-does-not-exist-xyz', args: [] } });
  assert.deepEqual(await missing.bind(), { status: 'unavailable', replaced: false });
  assert.deepEqual(await missing.claim(), { status: 'unavailable', returns: [] });
});

test('owner validation rejects bad input', async (t) => {
  const cwd = await project(t);
  for (const bad of ['', 'x'.repeat(201), 'bad\u0000owner', 'bad\u001fowner', 42, null]) {
    assert.throws(() => createOwnerMailbox({ owner: bad, sessionId: 'session-a', cwd, command: fixture() }), /Owner reference/);
  }
});

test('a channel with an owner binds on start and claims exactly once', async (t) => {
  const cwd = await mkdtemp(join(tmpdir(), 'edda-owner-mailbox-'));
  const root = join(cwd, 'private');
  const channel = await startChannel({ root, sessionId: randomUUID(), cwd, ownerRef: owner, ownerCommand: fixture(), deliver() {} });
  t.after(async () => { await channel.close(); await rm(cwd, { recursive: true, force: true }); });
  assert.deepEqual(channel.snapshot().owner, { owner, status: 'bound', replaced: false });
  assert.equal(channel.snapshot().returnOwner, null);
  await postReturn(cwd, { work: 'job-channel', session: 'controller-c' });
  const first = await channel.claimOwnerReturns();
  assert.equal(first.status, 'ok');
  assert.equal(first.returns.length, 1);
  assert.equal(first.returns[0].work, 'job-channel');
  assert.deepEqual(await channel.claimOwnerReturns(), { status: 'empty', returns: [] });
});

test('a channel without an owner reports disabled owner returns', async (t) => {
  const cwd = await mkdtemp(join(tmpdir(), 'edda-owner-mailbox-'));
  const root = join(cwd, 'private');
  const channel = await startChannel({ root, sessionId: randomUUID(), cwd, deliver() {} });
  t.after(async () => { await channel.close(); await rm(cwd, { recursive: true, force: true }); });
  assert.equal(channel.snapshot().owner, null);
  assert.deepEqual(await channel.claimOwnerReturns(), { status: 'disabled', returns: [] });
});

test('one explicit owner mailbox root is shared by two sibling project directories', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'edda-owner-mailbox-shared-'));
  const assistantDir = join(root, 'assistant'), controllerDir = join(root, 'controllers', 'job-1');
  await mkdir(assistantDir, { recursive: true }); await mkdir(controllerDir, { recursive: true });
  const shared = join(root, 'owner-mailbox');
  const prior = process.env.EDDA_RETURN_ROOT;
  process.env.EDDA_RETURN_ROOT = shared;
  t.after(async () => {
    if (prior === undefined) delete process.env.EDDA_RETURN_ROOT; else process.env.EDDA_RETURN_ROOT = prior;
    await rm(root, { recursive: true, force: true });
  });
  const assistant = createOwnerMailbox({ owner, sessionId: 'session-a', cwd: assistantDir, command: fixture() });
  assert.equal((await assistant.bind()).status, 'bound');
  await postReturn(controllerDir, { work: 'job-shared' });
  const claimed = await assistant.claim();
  assert.equal(claimed.status, 'ok');
  assert.equal(claimed.returns[0].work, 'job-shared');
});

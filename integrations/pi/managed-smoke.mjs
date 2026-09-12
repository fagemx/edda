// Real Pi lifecycle dogfood. Default offline; --live uses explicit configured provider/model.
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, rm, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';
import { setTimeout as delay } from 'node:timers/promises';
import { launchManaged, managedStatus, stopManaged, resumeManaged } from './managed-client.mjs';
import { requestSession, getReceipt } from './client.mjs';
import { listInbox, readInbox, respondInbox } from './inbox-manager.mjs';
import { alive } from './managed-store.mjs';

const [entry, mode, provider, model] = process.argv.slice(2);
if (mode && mode !== '--live') throw new Error('Unknown smoke mode');
if (mode === '--live' && (!provider || !model)) throw new Error('--live requires explicit provider and model');
const root = await mkdtemp(join(tmpdir(), 'edda-managed-smoke-'));
const registry = join(root, 'registry'), project = join(root, 'project'), agentDir = join(root, 'agent');
await mkdir(project); await mkdir(agentDir);
let runId, sessionId, passed = false;
const prompt = mode === '--live' ? 'This is one bounded acceptance fixture for Edda issue #1157. Work only in the current temporary directory. Create smoke-result.txt containing exactly MANAGED_PI_OK followed by a newline, verify its contents, then reply MANAGED_PI_DONE and stop. Do not touch other directories, start subagents, access the network, install dependencies, or ask for another approval. The operator explicitly authorized this fixture.' : 'ASK_OFFLINE_PERMISSION';
async function until(fn, label, timeout = 30000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const value = await fn(); if (value) return value;
    await delay(200);
  }
  throw new Error(`Timed out: ${label}`);
}
try {
  const launch = await launchManaged(registry, { runId: randomUUID(), project, piEntry: entry, prompt,
    provider: mode === '--live' ? provider : 'edda-offline-test', model: mode === '--live' ? model : 'echo', thinking: mode === '--live' ? 'low' : undefined,
    ...(mode === '--live' ? {} : { agentDir, noTools: true, extensions: [fileURLToPath(new URL('./fixtures/offline-provider.mjs', import.meta.url))] }) });
  runId = launch.runId; sessionId = launch.sessionId;
  assert.equal(launch.live, true);
  assert.equal(launch.pi.integration.releaseId, launch.release.id);
  assert.ok(launch.pi.capabilities.includes('inbox'));
  await until(async () => {
    const status = await managedStatus(registry, runId);
    if (status.initialReceipt?.status === 'failed' || status.modelError ||
      (status.lastEvent === 'extension_error' && status.initialReceipt?.status === 'unconfirmed')) throw new Error(`Model invocation failed: ${JSON.stringify(status.modelError || { status: status.lastEvent || 'failed' })}`);
    return status.initialReceipt?.status === 'settled';
  }, 'initial task settlement', mode === '--live' ? 120000 : 30000);
  const afterWork = await managedStatus(registry, runId);
  assert.equal(afterWork.sessionPersisted, true);
  const inbox = listInbox(registry), eventId = inbox.events.at(-1).eventId;
  const item = await readInbox(registry, eventId);
  if (mode === '--live') {
    assert.equal(await readFile(join(project, 'smoke-result.txt'), 'utf8'), 'MANAGED_PI_OK\n');
    assert.ok(item.event.excerpt.text.includes('MANAGED_PI_DONE'));
  } else {
    assert.ok(item.event.excerpt.text.includes('APPROVE_OFFLINE_TASK'));
    const reply = await respondInbox(registry, eventId, { message: 'APPROVE_OFFLINE_TASK' });
    await until(async () => (await getReceipt(registry, sessionId, reply.id)).status === 'settled', 'same-session response');
  }
  const beforeStop = await requestSession(registry, sessionId, '/conversation?limit=50');
  assert.equal((await stopManaged(registry, runId)).status, 'stopped');
  const resumed = await resumeManaged(registry, runId);
  assert.equal(resumed.live, true);
  assert.equal(resumed.sessionId, sessionId);
  assert.deepEqual(resumed.model, afterWork.model);
  assert.equal(resumed.thinkingLevel, afterWork.thinkingLevel);
  assert.notEqual(resumed.serviceId, launch.serviceId);
  assert.notEqual(resumed.instanceId, launch.instanceId);
  const afterResume = await requestSession(registry, sessionId, '/conversation?limit=50');
  assert.equal(afterResume.headCursor, beforeStop.headCursor);
  assert.equal(afterResume.entries.filter((e) => e.role === 'user' && e.text?.includes(prompt)).length, 1);
  assert.ok(listInbox(registry).events.some((e) => e.eventId === eventId));
  passed = true;
  console.log(JSON.stringify({ passed: true, actualPi: true, liveModel: mode === '--live', sessionId,
    pinnedRuntime: true, reconnect: true, sameSessionRecovery: true, initialReplayed: false,
    inboxPreserved: true, model: afterWork.model, usage: afterWork.usage || null }, null, 2));
} finally {
  if (runId) {
    const state = await managedStatus(registry, runId);
    if (!passed) {
      const evidence = { runId, phase: state.phase, runnerLive: state.live, piState: state.pi?.state,
        lastEvent: state.lastEvent, model: state.model, modelError: state.modelError, initialReceipt: state.initialReceipt,
        usage: state.usage, error: state.error };
      await writeFile(join(root, 'failure-evidence.json'), JSON.stringify(evidence, null, 2));
      console.log(JSON.stringify({ failed: true, evidenceRoot: root, ...evidence }));
    }
    if (state.live) await stopManaged(registry, runId, { abort: state.pi?.state !== 'idle' });
    const final = await managedStatus(registry, runId);
    if (alive(final.runnerPid) || alive(final.childPid)) throw new Error(`Owned fixture still active; preserve ${root} and run ${runId} for inspection`);
  }
  if (passed) await rm(root, { recursive: true, force: true });
}

// Durable per-run opt-in recovery for managed Pi runs.
//
// The installed entry has no automatic process restart. This module adds the one
// bounded exception the product actually asked for: an operator enrolls a
// *specific, owner-bound* run once, and a later owner-bound session start may
// resume that run and send exactly one deterministic reconnect message. It never
// guesses, never revives an intentional stop or a live writer, never repairs a
// corrupt record, and never adds a scheduler, service, timer or second store.
//
// The policy file is `<managedDir(root, runId)>/recovery.json`; the effective
// run/session/owner identity always comes from the existing runner records.
import { execFile } from 'node:child_process';
import { lstatSync, readdirSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';
import { digest, readJson, readRecord, validateId, writeJson } from './store.mjs';
import { managedDir } from './managed-store.mjs';
import { managedStatus, resumeManaged } from './managed-client.mjs';
import { readEnrollment, validateScope } from './supervision.mjs';
import { requestSession } from './client.mjs';
import { messageId } from './inbox-store.mjs';

const exec = promisify(execFile);
const POLICY_VERSION = 1;
const MAX_ATTEMPTS = 10;
const MAX_COOLDOWN_MS = 86_400_000;
const MAX_RECONNECT_BYTES = 1200;
const RUN_DIR = /^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/;

export const recoveryPath = (root, runId) => join(managedDir(root, validateId(runId)), 'recovery.json');

function clipText(text, limit) {
  const bytes = Buffer.from(text);
  if (bytes.length <= limit) return text;
  let end = limit;
  while (end > 0 && (bytes[end] & 0xc0) === 0x80) end--;
  return bytes.subarray(0, end).toString('utf8');
}

// Read one run's durable policy. A missing file is `null`; a corrupt file throws
// the typed `record_unavailable` error so callers never guess or repair it.
export function readRecovery(root, runId) {
  runId = validateId(runId);
  const value = readJson(recoveryPath(root, runId));
  if (value === null) return null;
  if (value.runId !== runId) throw new Error('Recovery policy identity mismatch');
  return value;
}

// Bounded listing: only UUID-named directories directly under `<root>/managed`
// are inspected, and only their `recovery.json`. No recursive scan, no access
// outside the managed directory. A corrupt policy is surfaced as a per-run
// error entry rather than hidden or repaired.
export function listRecovery(root) {
  root = resolve(root);
  const base = join(root, 'managed');
  let names;
  try {
    const info = lstatSync(base);
    if (!info.isDirectory() || info.isSymbolicLink()) throw new Error('Managed directory must not be a link');
    names = readdirSync(base).filter((name) => RUN_DIR.test(name)).sort();
  } catch (error) {
    if (error.code === 'ENOENT') return [];
    throw error;
  }
  return names.map((runId) => {
    const { value, error } = readRecord(join(base, runId, 'recovery.json'));
    if (error) return { runId, policy: null, error: { ...error } };
    if (!value || value.runId !== runId) return null;
    return { runId, policy: value, error: null };
  }).filter(Boolean);
}

// Enroll one owner-bound run. Refuses an unowned run and names the exact fix;
// re-enrolling keeps the existing attempt history and re-enables the policy.
export async function enrollRecovery(root, runId, { scope, maxAttempts, cooldownMs } = {}) {
  runId = validateId(runId);
  validateScope(scope);
  const attempts = maxAttempts === undefined || maxAttempts === null ? 3 : Number(maxAttempts);
  if (!Number.isInteger(attempts) || attempts < 1 || attempts > MAX_ATTEMPTS) throw new Error(`--max-attempts must be an integer 1..${MAX_ATTEMPTS}`);
  const cooldown = cooldownMs === undefined || cooldownMs === null ? 60_000 : Number(cooldownMs);
  if (!Number.isInteger(cooldown) || cooldown < 0 || cooldown > MAX_COOLDOWN_MS) throw new Error(`--cooldown-ms must be an integer 0..${MAX_COOLDOWN_MS}`);

  // Existing run identity comes from the runner record, never a caller argument.
  const status = await managedStatus(root, runId);
  if (status.continuity !== 'owner-bound' || typeof status.owner !== 'string' || !status.owner) {
    throw new Error(`Run '${runId}' is not owner-bound, so recovery cannot be enrolled. Adopt it with `
      + `'edda-pi owner adopt --run ${runId} --owner <ref>'; if its pinned runtime has no adoption endpoint, `
      + `stop it and run 'edda-pi run-resume ${runId} --runtime current' to re-pin, then adopt again.`);
  }
  const sessionId = status.sessionId;
  if (typeof sessionId !== 'string' || !/^[a-zA-Z0-9][a-zA-Z0-9_.:-]{0,199}$/.test(sessionId)) {
    throw new Error(`Run '${runId}' has no recorded session identity, so its continuation cannot be bounded; inspect run-status before enrolling.`);
  }

  // Re-enrolling must not erase prior attempts. A corrupt policy is preserved,
  // not overwritten.
  let previous = null;
  try { previous = readRecovery(root, runId); }
  catch (error) {
    if (error.code === 'record_unavailable') {
      throw new Error(`Existing recovery record for '${runId}' is unreadable and was not repaired; inspect ${recoveryPath(root, runId)} before re-enrolling.`);
    }
    throw error;
  }
  const timestamp = new Date().toISOString();
  const policy = {
    version: POLICY_VERSION,
    runId,
    sessionId,
    owner: status.owner,
    ownerRoot: typeof status.ownerRoot === 'string' ? status.ownerRoot : null,
    returnOwner: typeof status.returnOwner === 'string' ? status.returnOwner : null,
    scope,
    maxAttempts: attempts,
    cooldownMs: cooldown,
    enabled: true,
    enrolledAt: typeof previous?.enrolledAt === 'string' ? previous.enrolledAt : timestamp,
    updatedAt: timestamp,
    attempts: Array.isArray(previous?.attempts) ? previous.attempts : [],
  };
  writeJson(recoveryPath(root, runId), policy);
  return policy;
}

// Revoke is terminal until an explicit re-enroll; the attempt history is kept.
export function revokeRecovery(root, runId, { reason } = {}) {
  runId = validateId(runId);
  const policy = readRecovery(root, runId);
  if (!policy) throw new Error(`Run '${runId}' has no recovery enrollment to revoke`);
  const next = { ...policy, enabled: false, updatedAt: new Date().toISOString(),
    revokedAt: new Date().toISOString(),
    revokedReason: typeof reason === 'string' && reason.trim() ? clipText(reason, 1000) : null };
  writeJson(recoveryPath(root, runId), next);
  return next;
}

// A durable policy must carry integer limits and parseable attempt times. A
// malformed shape must surface as attention, never silently disable the attempt
// cap or the cooldown.
function policyShapeValid(policy) {
  if (!Number.isInteger(policy.maxAttempts) || policy.maxAttempts < 1) return false;
  if (!Number.isInteger(policy.cooldownMs) || policy.cooldownMs < 0) return false;
  if (policy.attempts !== undefined && !Array.isArray(policy.attempts)) return false;
  for (const entry of Array.isArray(policy.attempts) ? policy.attempts : []) {
    if (!entry || !Number.isFinite(Date.parse(entry.at))) return false;
  }
  return true;
}

// Pure eligibility. No I/O, no clock read: `now` is passed in.
export function recoveryDecision({ policy, status, supervision, now = Date.now() } = {}) {
  if (!policy || policy.enabled === false) return { decision: 'skip', reason: 'not_enrolled' };
  if (status?.live === true) return { decision: 'skip', reason: 'live_holder' };
  if (status?.lastRecordedPhase === 'stopped') return { decision: 'skip', reason: 'intentionally_stopped' };
  // A stop that was killed mid-way is not a proven dead run: never resume it.
  if (status?.lastRecordedPhase === 'stopping') return { decision: 'attention', reason: 'stopping_unproven' };
  if (supervision && (supervision.enabled === false || supervision.action === 'paused')) return { decision: 'skip', reason: 'paused' };
  if (status?.status === 'record_unavailable') return { decision: 'attention', reason: 'record_unavailable' };
  // The durable enrollment pre-authorizes one owner; without that identity the
  // continuation has no authority, so surface it instead of resuming.
  if (status?.continuity !== 'owner-bound' || typeof status?.owner !== 'string' || !status.owner) {
    return { decision: 'attention', reason: 'owner_missing' };
  }
  if (!policyShapeValid(policy)) return { decision: 'attention', reason: 'policy_invalid' };
  const attempts = Array.isArray(policy.attempts) ? policy.attempts : [];
  if (attempts.length >= policy.maxAttempts) return { decision: 'skip', reason: 'attempts_exhausted' };
  const last = attempts.at(-1);
  if (last && now - Date.parse(last.at) < policy.cooldownMs) return { decision: 'skip', reason: 'cooldown' };
  return { decision: 'eligible', reason: 'eligible' };
}

// Read-only owner-return count. Reuses the existing `edda return status`
// invocation shape; it never binds, claims or mutates the mailbox.
export async function pendingOwnerReturns(owner, { ownerRoot, command, timeoutMs = 5000 } = {}) {
  if (typeof owner !== 'string' || !owner) return { status: 'unavailable', pending: null };
  const cli = command ?? { file: process.env.EDDA_BIN || 'edda', args: [] };
  const env = ownerRoot ? { ...process.env, EDDA_RETURN_ROOT: ownerRoot } : process.env;
  try {
    const { stdout } = await exec(cli.file, [...(cli.args ?? []), 'return', 'status', '--owner', owner, '--json'],
      { windowsHide: true, timeout: timeoutMs, maxBuffer: 1024 * 1024, encoding: 'utf8', env });
    const parsed = JSON.parse(stdout);
    if (!parsed || typeof parsed !== 'object' || parsed.owner !== owner) return { status: 'unavailable', pending: null };
    return { status: 'ok', pending: Number.isInteger(parsed.pending) ? parsed.pending : null,
      holderUnknown: parsed.holderUnknown === true, handoverPending: parsed.handoverPending === true };
  } catch { return { status: 'unavailable', pending: null }; }
}

// One deterministic, bounded reconnect message. It never replays the original
// prompt and never widens authority beyond the enrolled scope.
export function reconnectMessage({ runId, scope, attempt, pending }) {
  const parts = [
    `Edda bounded recovery (attempt ${attempt}): managed run ${runId} was resumed after an interruption.`,
    'This is not the original prompt; the initial task is not replayed.',
    `Continue only inside your authorized scope: ${clipText(String(scope ?? ''), 700)}`,
  ];
  if (Number.isInteger(pending)) parts.push(`Owner returns currently pending for your owner: ${pending}.`);
  parts.push('Report a milestone or a concrete stopping reason before ending this turn.');
  return clipText(parts.join('\n'), MAX_RECONNECT_BYTES);
}

function reconnectStatus(error) {
  return /unknown/i.test(String(error?.message ?? '')) ? 'unknown' : 'unavailable';
}

// One bounded pass over enrolled runs. At most `max` resumes per pass (default
// 1, clamped 1..5); at most one reconnect per resumed run. This never throws out
// of the pass and never rewrites a record it could not read.
export async function recoveryPass(root, { runId, max = 1, resume = resumeManaged, now = Date.now(), ownerReturns = pendingOwnerReturns } = {}) {
  root = resolve(root);
  let limit = Number(max);
  if (!Number.isInteger(limit)) limit = 1;
  limit = Math.min(5, Math.max(1, limit));

  let entries;
  if (runId !== undefined && runId !== null) {
    runId = validateId(runId);
    try {
      const policy = readRecovery(root, runId);
      entries = [{ runId, policy, error: null }];
    } catch (error) {
      entries = [{ runId, policy: null, error: { code: error.code || 'record_unavailable', record: error.record || 'recovery.json', message: error.message } }];
    }
  } else {
    entries = listRecovery(root);
  }

  const results = [];
  let resumed = 0;
  for (const entry of entries) {
    const run = entry.runId;
    if (entry.error) {
      results.push({ runId: run, decision: 'attention', reason: 'record_unavailable', outcome: null, reconnect: 'none', receipt: null });
      continue;
    }
    if (!entry.policy) {
      results.push({ runId: run, decision: 'skip', reason: 'not_enrolled', outcome: null, reconnect: 'none', receipt: null });
      continue;
    }
    const policy = entry.policy;
    let status;
    try { status = await managedStatus(root, run); }
    catch (error) {
      results.push({ runId: run, decision: 'attention', reason: 'status_unavailable', outcome: null, reconnect: 'none', receipt: null, error: error.message });
      continue;
    }
    // An absent supervision record is `null` (not paused); an unreadable or
    // identity-mismatched one can never be read as "not paused", so it is
    // surfaced as attention and the run is not resumed.
    let supervision;
    try { supervision = readEnrollment(root, policy.sessionId ?? status.sessionId); }
    catch (error) {
      results.push({ runId: run, decision: 'attention', reason: 'supervision_unavailable', outcome: null, reconnect: 'none', receipt: null, error: error.message });
      continue;
    }
    const verdict = recoveryDecision({ policy, status, supervision, now });
    if (verdict.decision !== 'eligible') {
      results.push({ runId: run, decision: verdict.decision, reason: verdict.reason, outcome: null, reconnect: 'none', receipt: null });
      continue;
    }
    if (resumed >= limit) {
      results.push({ runId: run, decision: 'eligible', reason: 'eligible', outcome: 'deferred', reconnect: 'none', receipt: null });
      continue;
    }

    let outcome, reason, reconnect = 'none', receipt = null, live = false, reconnectId = null;
    try {
      const result = await resume(root, run);
      if (result?.live === true) { outcome = 'resumed'; reason = 'live'; live = true; }
      else { outcome = 'refused'; reason = typeof result?.status === 'string' ? result.status : 'resume did not produce a live session'; }
    } catch (error) {
      outcome = 'refused';
      reason = error?.message || 'resume refused';
    }

    if (live) {
      resumed += 1;
      let pending = null;
      try {
        const counted = await ownerReturns(policy.owner, { ownerRoot: policy.ownerRoot || undefined });
        if (counted?.status === 'ok') pending = counted.pending;
      } catch { pending = null; }
      const attempt = (Array.isArray(policy.attempts) ? policy.attempts.length : 0) + 1;
      reconnectId = messageId(digest(`edda-recovery-v1:${run}:${attempt}`));
      const message = reconnectMessage({ runId: run, scope: policy.scope, attempt, pending });
      try {
        receipt = await requestSession(root, policy.sessionId, '/messages', { id: reconnectId, message, mode: 'followUp' }, 2500);
        reconnect = 'sent';
      } catch (error) {
        reconnect = reconnectStatus(error);
        receipt = { status: reconnect, error: error?.message ?? null };
      }
    }

    const attempts = [...(Array.isArray(policy.attempts) ? policy.attempts : []),
      { at: new Date(now).toISOString(), outcome, reason: clipText(String(reason ?? ''), 500), reconnect }];
    const receiptView = receipt ? { status: receipt.status ?? reconnect, id: receipt.id ?? reconnectId } : null;
    try { writeJson(recoveryPath(root, run), { ...policy, attempts, updatedAt: new Date(now).toISOString() }); }
    catch (error) { results.push({ runId: run, decision: 'eligible', reason: 'eligible', outcome, reconnect,
      receipt: receiptView, error: `attempt record not persisted: ${error.message}` }); continue; }
    results.push({ runId: run, decision: 'eligible', reason: 'eligible', outcome, reconnect, receipt: receiptView });
  }
  return { status: 'recovery_pass', considered: entries.length, resumed, results };
}

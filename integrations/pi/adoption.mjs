import { digest } from './store.mjs';
import { listSessions, requestSession } from './client.mjs';
import { readTask, taskId } from './compose-sources.mjs';
import { composeHandoff } from './compose.mjs';
import { readEnrollment, enroll, validateScope } from './supervision.mjs';
import { dependencyConfiguration, dependencyFact } from './dependency-observer.mjs';

export function selectSession(rows, selector) {
  const exact = rows.find((row) => row.sessionId === selector);
  if (exact) return { status: 'selected', state: exact };
  if (typeof selector !== 'string' || selector.length < 8 || selector.length > 200 || !/^[a-zA-Z0-9_.:-]+$/.test(selector)) {
    return { status: 'invalid_selector', nextAction: 'Use an exact session ID or at least eight characters of its unique prefix.' };
  }
  const matches = rows.filter((row) => row.sessionId.startsWith(selector));
  if (matches.length === 1) return { status: 'selected', state: matches[0] };
  return { status: matches.length ? 'ambiguous_session' : 'not_registered',
    candidates: matches.slice(0, 20).map(({ sessionId, cwd, live }) => ({ sessionId, cwd, live })),
    nextAction: 'Run list and choose the exact session ID; offline matches also count.' };
}

export async function discoverDependencies(project, roots, command) {
  if (!Array.isArray(roots) || !roots.length || roots.length > 8) throw new Error('Select one to eight task roots');
  roots = [...new Set(roots.map(taskId))];
  const sources = new Map(), visiting = new Set(), edges = [];
  async function visit(id) {
    if (visiting.has(id)) throw new Error(`Dependency cycle at task ${id}; inspect the task rail`);
    if (sources.has(id)) return;
    if (sources.size >= 8) throw new Error('Dependency graph exceeds eight tasks; select a smaller explicit scope with follow');
    let source;
    try { source = await readTask(project, id, command); dependencyFact(source.task); }
    catch { throw new Error(`Cannot read task ${id}; no adoption applied`); }
    sources.set(id, source);
    visiting.add(id);
    for (const parent of new Set(source.task.after.map(String))) {
      edges.push({ taskId: id, after: parent });
      await visit(parent);
    }
    visiting.delete(id);
  }
  for (const id of roots) await visit(id);
  const taskIds = [...sources.keys()].sort((a, b) => Number(a) - Number(b));
  return { taskIds, edges, roots, coverage: 'explicit_after_snapshot',
    sourceRevisions: Object.fromEntries([...sources].map(([id, value]) => [id, digest(value.raw)])),
    notice: 'Includes selected roots and transitive prerequisites only. New review/fix tasks or graph edits require another adoption; done is not acceptance.' };
}

function semanticManifest(manifest) {
  const { planRef, ...rest } = manifest;
  return digest(JSON.stringify(rest));
}

export async function adoptSession(root, selector, { id, project, include = [], contextFile, scope,
  notify = false, maxNotifications = 10, preview = false, expectedRevision, eddaCommand } = {}) {
  taskId(id);
  if (!Array.isArray(include)) throw new Error('include must be a list of task IDs');
  include.forEach(taskId);
  const selected = selectSession(await listSessions(root), selector);
  if (selected.status !== 'selected') return selected;
  const { state } = selected;
  const sessionId = state.sessionId, instanceId = state.instanceId;
  const base = { sessionId, instanceId, managedTaskId: id, workStarted: null };
  if (!state.live) return { ...base, status: 'offline', nextAction: 'Inspect the original Pi process; adoption does not restart it.' };
  if (!['handoff', 'dependencies'].every((cap) => state.capabilities?.includes(cap))) {
    return { ...base, status: 'needs_reload', nextAction: 'Run /reload in this Pi when idle, then repeat adopt. No terminal automation is performed.' };
  }
  if (state.state !== 'idle') return { ...base, status: 'busy', runtimeState: state.state, nextAction: 'Repeat adopt when this instance is idle.' };
  project ||= state.cwd;
  const previous = readEnrollment(root, sessionId);
  const chosenScope = scope ?? (previous?.enabled ? previous.scope : undefined);
  if (chosenScope === undefined) return { ...base, status: 'needs_enrollment', nextAction: 'Supply --scope with the existing delegated limits.' };
  validateScope(chosenScope);
  // Resolve and validate all sources before changing enrollment, handoff or follow.
  const dependencies = await discoverDependencies(project, [id, ...include], eddaCommand);
  const config = await dependencyConfiguration({ project, taskIds: dependencies.taskIds, notify, maxNotifications });
  const composition = await composeHandoff({ project: config.project, id, contextFile, root, eddaCommand });
  if (composition.status !== 'ready') return { ...base, status: 'needs_context', missing: composition.missing,
    contextError: composition.contextError, source: composition.source,
    nextAction: 'Provide declared role/doneWhen/scope metadata via --context, then repeat adopt. No managed changes were made.' };
  if (composition.source.taskSource.revision !== `sha256:${dependencies.sourceRevisions[id]}`) {
    return { ...base, status: 'source_changed', nextAction: 'Task changed during preflight; inspect and repeat adopt.' };
  }
  const handoff = await requestSession(root, sessionId, '/handoff?budget=32768', undefined, 2500, instanceId);
  if (!['ready', 'not_prepared', 'stale_binding'].includes(handoff.status)) {
    return { ...base, status: 'needs_context', nextAction: 'Inspect brief before adoption; existing handoff is unavailable or over budget.' };
  }
  const manifestReused = handoff.status === 'ready' && semanticManifest(handoff.manifest) === semanticManifest(composition.manifest);
  const revision = handoff.manifestRevision || null;
  if ((expectedRevision !== undefined && expectedRevision !== revision) ||
    (revision && !manifestReused && expectedRevision !== revision)) {
    return { ...base, status: 'needs_expected_revision', currentRevision: revision,
      nextAction: 'Inspect brief, then supply --expected with this exact revision to replace or rebind the handoff.' };
  }
  const manifest = manifestReused ? handoff.manifest : composition.manifest;
  const detail = { ...base, project: config.project, dependencies, manifestReused,
    manifestRevision: digest(JSON.stringify(manifest)), planRef: manifest.planRef,
    notify, maxNotifications, scope: chosenScope, workStarted: notify ? null : false };
  if (preview) return { ...detail, status: 'preview', nextAction: 'Repeat without --preview to apply this declared scope. No managed changes or messages were made.' };
  const steps = [];
  let attemptedStep;
  try {
    attemptedStep = 'prepare';
    const prepared = await requestSession(root, sessionId, '/handoff/manifest', { manifest, expectedRevision: revision }, 2500, instanceId);
    steps.push({ step: attemptedStep, status: prepared.status });
    attemptedStep = 'enroll';
    if (!previous?.enabled || previous.scope !== chosenScope) await enroll(root, sessionId, chosenScope, instanceId);
    else await requestSession(root, sessionId, '/status', undefined, 2500, instanceId);
    steps.push({ step: attemptedStep, status: 'enabled' });
    attemptedStep = 'follow';
    const observer = await requestSession(root, sessionId, '/dependencies', config, 5000, instanceId);
    steps.push({ step: attemptedStep, status: observer.phase });
    return { ...detail, status: 'adopted', steps, observer,
      nextAction: notify ? 'Inspect dependencies for the notification receipt; configured does not mean work started.' :
        'Observation is configured. Use send for an explicitly authorized work instruction; unfollow pauses observation.' };
  } catch (error) {
    return { ...detail, status: 'adoption_incomplete', steps, attemptedStep, error: error.message,
      nextAction: 'Inspect doctor, brief and dependencies before repeating. Earlier steps or the last request may have applied; no rollback or automatic retry.' };
  }
}

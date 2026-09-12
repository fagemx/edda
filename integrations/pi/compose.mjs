import { resolve } from 'node:path';
import { writeFile } from 'node:fs/promises';
import { normalizeManifest } from './handoff-schema.mjs';
import { defaultRoot, digest } from './store.mjs';
import { readTask, readBoundedFile, projectBrief, sourceCache } from './compose-sources.mjs';

function onlyKeys(value, keys, label) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error(`${label} must be an object`);
  for (const key of Object.keys(value)) if (!keys.includes(key)) throw new Error(`Unknown ${label} field: ${key}`);
}
export function managementMetadata(text) {
  const body = text.replace(/^\uFEFF/, '').trim();
  let context;
  if (body.startsWith('{')) context = JSON.parse(body);
  else {
    const blocks = [];
    let fence;
    let lines = [];
    for (const line of body.split(/\r?\n/)) {
      if (fence) {
        if (new RegExp(`^ {0,3}${fence.char}{${fence.length},}[ \\t]*$`).test(line)) {
          if (fence.management) blocks.push(lines.join('\n'));
          fence = undefined;
          lines = [];
        } else if (fence.management) lines.push(line);
      } else {
        const opening = /^ {0,3}(`{3,}|~{3,})(.*)$/.exec(line);
        if (opening) fence = { char: opening[1][0], length: opening[1].length, management: opening[2].trim() === 'edda-management' };
      }
    }
    if (fence?.management) throw new Error('Unclosed edda-management block');
    if (blocks.length === 0) return null;
    if (blocks.length !== 1) throw new Error('Management brief must contain exactly one edda-management block');
    context = JSON.parse(blocks[0]);
  }
  onlyKeys(context, ['role', 'doneWhen', 'scope'], 'management context');
  if (context.scope !== undefined) onlyKeys(context.scope, ['allowed', 'excluded', 'reserved', 'authorityRefs'], 'management scope');
  return context;
}

export async function composeHandoff({ project, id, contextFile, output, root = defaultRoot(), eddaCommand }) {
  if (!project) throw new Error('compose requires a project directory');
  root = resolve(root);
  const source = await readTask(project, id, eddaCommand);
  const cache = await sourceCache(root);
  const taskRef = await cache(source.raw);
  const brief = contextFile ? { status: 'explicit_context', ...await readBoundedFile(contextFile) } :
    await projectBrief(source.project, source.task.brief_ref);
  const contextRef = brief.text === undefined ? null : await cache(brief.text, 'txt');
  const bundle = { version: 1, project: source.project, taskId: id, taskSource: taskRef,
    briefRef: source.task.brief_ref, contextSource: contextRef, contextStatus: brief.status };
  const planRef = await cache(JSON.stringify(bundle, null, 2) + '\n');
  let context;
  let contextError;
  try { context = brief.text === undefined ? null : managementMetadata(brief.text); }
  catch (error) { contextError = error.message; }
  const missing = [];
  for (const key of ['role', 'doneWhen']) {
    if (context?.[key] === undefined) missing.push(key);
  }
  for (const key of ['allowed', 'excluded', 'reserved', 'authorityRefs']) {
    if (context?.scope?.[key] === undefined) missing.push(`scope.${key}`);
  }
  const candidate = {
    version: 1, runId: `edda-${digest(`${source.project}\0${source.task.created_event_id}`).slice(0, 32)}`,
    role: context?.role ?? null, goal: source.task.title, planRef, doneWhen: context?.doneWhen ?? null,
    scope: { allowed: context?.scope?.allowed ?? null, excluded: context?.scope?.excluded ?? null,
      reserved: context?.scope?.reserved ?? null, authorityRefs: context?.scope?.authorityRefs ?? null,
      taskPaths: source.task.scope_paths },
  };
  const result = { version: 1, status: 'needs_context', authority: 'declared_context_only', missing,
    contextError: contextError || null, source: bundle,
    taskFacts: { title: source.task.title, status: source.task.status, attempts: source.task.attempts,
      planId: source.task.plan_id, workUnitRef: source.task.work_unit_ref, dependencies: source.task.after,
      scopePaths: source.task.scope_paths, sessionId: source.task.session_id, receiptAvailable: Boolean(source.task.receipt) },
    notice: 'Task facts are read from the existing Edda CLI. Structured metadata is declared context, not verified authority. No task writes, session preparation, URL fetches, model calls or prose inference are performed.' };
  const bounded = (value) => {
    if (Buffer.byteLength(JSON.stringify(value)) <= 32768) return value;
    return { version: 1, status: 'needs_context', authority: 'declared_context_only',
      missing: ['composition_exceeds_32768_bytes'], source: { project: source.project, taskId: id,
        taskSource: taskRef, contextSource: contextRef, bundleSource: planRef },
      notice: 'Source facts exceed the bounded composition preview. Read the immutable snapshots; no output manifest was written.' };
  };
  if (missing.length || contextError) return bounded({ ...result, draft: candidate });
  let manifest;
  try { manifest = normalizeManifest(candidate); }
  catch (error) { return bounded({ ...result, contextError: error.message, draft: candidate }); }
  const ready = bounded({ ...result, status: 'ready', manifest, output: output ? resolve(output) : null });
  if (ready.status !== 'ready') return ready;
  if (output) await writeFile(resolve(output), JSON.stringify(manifest, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
  return ready;
}

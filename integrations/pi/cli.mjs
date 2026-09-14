#!/usr/bin/env node
import { existsSync, readFileSync, statSync } from 'node:fs';
import { resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
import { defaultRoot, recover, validateId } from './store.mjs';
import { listSessions, requestSession, getReceipt, inspectSession, prepareHandoff } from './client.mjs';
import { enroll, watch, checkpoint, reply, managementBrief } from './supervision.mjs';
import { composeHandoff } from './compose.mjs';
import { followDependencies, dependencyStatus, unfollowDependencies, doctor } from './dependency-client.mjs';
import { adoptSession } from './adoption.mjs';
import { listInbox, readInbox, acknowledgeInbox, recordAuthorization, revokeAuthorization, respondInbox } from './inbox-manager.mjs';
import { wakeCapability } from './inbox-store.mjs';
import { readBoundedFile } from './compose-sources.mjs';
import { installRuntime } from './managed-store.mjs';
import { launchManaged, managedStatus, stopManaged, resumeManaged, managedConversation, adoptOwner } from './managed-client.mjs';
import { startSupervisor, supervisorStatus, stopSupervisor } from './supervisor-client.mjs';
import { runtimeInfo, listManagedRuns } from './activation.mjs';
import { activationReceipt } from './activation-receipt.mjs';

const help = `Edda Pi session channel (same-user, same-machine)
  edda-pi --version
  edda-pi runtime-info                 installed version, capabilities and guide path (read-only)
  edda-pi activation [--json] [--check] [--repo PATH] [--registry-root PATH] [--manager-root PATH] [--edda-bin PATH] [--client-root PATH] [--timeout MS]   read-only merge/install/running revision receipt (--check exits 2 unless coherent)
  edda-pi runs [--limit 50] [--after RUN_ID]   recorded managed runs, including stopped ones
  node integrations/pi/cli.mjs list
  node integrations/pi/cli.mjs status SESSION_ID
  node integrations/pi/cli.mjs send SESSION_ID --message TEXT [--id UUID] [--sender codex] [--mode followUp|steer] [--registry DIR]
  node integrations/pi/cli.mjs send SESSION_ID --message-file PATH [--id UUID] [--registry DIR]
  node integrations/pi/cli.mjs receipt SESSION_ID --id UUID [--registry DIR]
  node integrations/pi/cli.mjs recover SESSION_ID --instance UUID
  node integrations/pi/cli.mjs conversation SESSION_ID [--after ENTRY_ID] [--limit 20]
  node integrations/pi/cli.mjs enroll SESSION_ID --scope TEXT
  node integrations/pi/cli.mjs watch
  node integrations/pi/cli.mjs watch --conversation
  node integrations/pi/cli.mjs prepare SESSION_ID --manifest FILE [--expected REVISION]
  node integrations/pi/cli.mjs brief SESSION_ID [--budget-bytes 16384]
  node integrations/pi/cli.mjs compose --project PATH --task ID [--context FILE] [--capsule CAPSULE_ID] [--output NEW_FILE] [--edda-bin PATH]
  node integrations/pi/cli.mjs reply SESSION_ID --to CURSOR --message TEXT
  node integrations/pi/cli.mjs checkpoint SESSION_ID --cursor CURSOR --action observed|working|waiting_user|complete|paused --note TEXT
  node integrations/pi/cli.mjs checkpoint SESSION_ID --action paused --note TEXT
  node integrations/pi/cli.mjs doctor [SESSION_ID]
  node integrations/pi/cli.mjs adopt SESSION_ID_OR_PREFIX --task ID [--project PATH] [--include ID,ID] [--context FILE] [--capsule CAPSULE_ID] [--scope TEXT] [--notify] [--max-notifications 10] [--preview] [--expected REVISION] [--edda-bin PATH]
  node integrations/pi/cli.mjs follow SESSION_ID --project PATH --tasks ID,ID [--scope TEXT] [--notify] [--max-notifications 10]
  node integrations/pi/cli.mjs dependencies SESSION_ID
  node integrations/pi/cli.mjs check-dependencies SESSION_ID
  node integrations/pi/cli.mjs unfollow SESSION_ID
  node integrations/pi/cli.mjs inbox [--consumer codex] [--limit 20] [--after EVENT_ID]
  node integrations/pi/cli.mjs inbox-read EVENT_ID [--consumer codex] [--budget-bytes 16384]
  node integrations/pi/cli.mjs inbox-ack EVENT_ID [--consumer codex]
  node integrations/pi/cli.mjs authorization-record EVENT_ID --record FILE [--consumer codex]
  node integrations/pi/cli.mjs authorization-revoke RECORD_ID [--consumer codex]
  node integrations/pi/cli.mjs inbox-respond EVENT_ID --message TEXT [--authorization RECORD_ID] [--consumer codex]
  node integrations/pi/cli.mjs inbox-respond EVENT_ID --message-file PATH [--authorization RECORD_ID]
  node integrations/pi/cli.mjs inbox-wake
  node integrations/pi/cli.mjs runtime-install
  node integrations/pi/cli.mjs launch --project PATH [--pi-entry FILE] [--provider NAME] [--model NAME] [--thinking LEVEL] [--prompt-file FILE] [--run-id UUID] [--extension FILE] [--agent-dir PATH] [--no-tools] [--owner REF] [--return-owner REF] [--owner-root DIR]
  node integrations/pi/cli.mjs run-status RUN_ID
  node integrations/pi/cli.mjs owner adopt --run RUN_ID --owner REF [--return-owner REF] [--owner-root DIR]
                                       adopt an already-launched run into the owner lifecycle (same run, next turn)
  node integrations/pi/cli.mjs run-conversation RUN_ID [--after ENTRY_ID] [--limit 20]
  node integrations/pi/cli.mjs run-stop RUN_ID [--abort]
  node integrations/pi/cli.mjs run-resume RUN_ID [--runtime pinned|current]
  node integrations/pi/cli.mjs supervisor-start --config FILE [--id UUID]
  node integrations/pi/cli.mjs supervisor-status SUPERVISOR_ID
  node integrations/pi/cli.mjs supervisor-stop SUPERVISOR_ID

JSON stdout; diagnostics stderr. EDDA_PI_CHANNEL_DIR overrides the private root.
--registry DIR addresses a session in another registry root (send/receipt/list/status/conversation).
Supervision commands are tools for an authorized controller, not a decision engine.
No automatic process restart or message retry. Opt-in dependency alerts may start a model turn.
The listener is local only. Keep the message ID.
Install from the packaged tarball; see README.md and the installed getting-started.md.
`.replaceAll('node integrations/pi/cli.mjs ', 'edda-pi ') +
  'Source checkout usage remains: node integrations/pi/cli.mjs <command>.\n';

// Per-verb usage lines taken from the top-level guide, so `edda-pi <verb> --help`
// prints the accepted flags instead of being rejected by the option parser.
const usageByVerb = new Map();
for (const line of help.split('\n')) {
  const match = /^ {2}edda-pi ([a-z][a-z0-9-]*)(?:[ \t]|$)/.exec(line);
  if (!match) continue;
  const lines = usageByVerb.get(match[1]) ?? [];
  lines.push(line.trim());
  usageByVerb.set(match[1], lines);
}

function registryRoot(value) {
  if (typeof value !== 'string' || !value.trim()) throw new Error('--registry requires a directory (an empty value is not the default root)');
  const dir = resolve(value);
  if (!existsSync(dir) || !statSync(dir).isDirectory()) throw new Error(`--registry must be an existing directory: ${dir}`);
  return dir;
}
// A bare "no reachable registered owner" hides that the registry, not the
// session, is wrong. Name the registry used and the option to change it.
function registryContext(error, sessionId, root) {
  if (error instanceof Error && /no reachable registered owner/.test(error.message)) {
    return new Error(`No session '${sessionId}' in registry '${root}'. If it is in another registry, pass --registry <dir> (or set EDDA_PI_CHANNEL_DIR) and retry.`);
  }
  return error;
}

async function main(args) {
  const [command, ...rest] = args;
  if (!command || command === '--help') { process.stdout.write(help); return; }
  if (command === '--version' && !rest.length) {
    process.stdout.write(`edda-pi ${JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8')).version}\n`); return;
  }
  if (rest.includes('--help')) {
    const usage = usageByVerb.get(command);
    if (usage) { process.stdout.write(`Usage: edda-pi ${command}\n${usage.map((line) => `  ${line}`).join('\n')}\n`); return; }
    process.stderr.write(`Unknown command '${command}'; run 'edda-pi --help' for the supported commands and options.\n`);
    process.exitCode = 1; return;
  }
  const positional = [];
  const options = {};
  for (let i = 0; i < rest.length; i++) {
    const arg = rest[i];
    if (['--conversation', '--notify', '--preview', '--no-tools', '--abort', '--json', '--check'].includes(arg) && options[arg] === undefined) { options[arg] = true; continue; }
    if (!arg.startsWith('--')) { positional.push(arg); continue; }
    if (options[arg] !== undefined || rest[i + 1] === undefined || rest[i + 1].startsWith('--')) throw new Error(`Missing or duplicate option ${arg}`);
    options[arg] = rest[++i];
  }
  const allowed = {
    'runtime-info': [],
    activation: ['--json', '--check', '--repo', '--registry-root', '--manager-root', '--edda-bin', '--client-root', '--timeout'],
    runs: ['--limit', '--after'],
    list: ['--registry'], status: ['--registry'], send: ['--message', '--message-file', '--id', '--sender', '--mode', '--registry'],
    receipt: ['--id', '--registry'], recover: ['--instance'],
    conversation: ['--after', '--limit', '--registry'], enroll: ['--scope'], watch: ['--conversation'],
    prepare: ['--manifest', '--expected'], brief: ['--budget-bytes'],
    compose: ['--project', '--task', '--context', '--capsule', '--output', '--edda-bin'],
    doctor: [], follow: ['--project', '--tasks', '--scope', '--notify', '--max-notifications'],
    adopt: ['--task', '--project', '--include', '--context', '--capsule', '--scope', '--notify', '--max-notifications', '--preview', '--expected', '--edda-bin'],
    dependencies: [], 'check-dependencies': [], unfollow: [],
    inbox: ['--consumer', '--limit', '--after'], 'inbox-read': ['--consumer', '--budget-bytes'], 'inbox-ack': ['--consumer'],
    'authorization-record': ['--record', '--consumer'], 'authorization-revoke': ['--consumer'],
    'inbox-respond': ['--message', '--message-file', '--authorization', '--consumer'], 'inbox-wake': [],
    'runtime-install': [], launch: ['--project', '--pi-entry', '--provider', '--model', '--thinking', '--prompt-file', '--run-id', '--extension', '--agent-dir', '--no-tools', '--owner', '--return-owner', '--owner-root'],
    'run-status': [], 'run-conversation': ['--after', '--limit'], 'run-stop': ['--abort'], 'run-resume': ['--runtime'],
    owner: ['--run', '--owner', '--return-owner', '--owner-root'],
    'supervisor-start': ['--config', '--id'], 'supervisor-status': [], 'supervisor-stop': [],
    reply: ['--to', '--message', '--message-file'], checkpoint: ['--cursor', '--action', '--note'],
  }[command];
  if (!allowed || Object.keys(options).some((key) => !allowed.includes(key))) throw new Error('Unknown command or option; use --help');
  const root = options['--registry'] !== undefined ? registryRoot(options['--registry']) : defaultRoot();
  const counts = ['doctor', 'owner'].includes(command) ? [0, 1] : [['runtime-info', 'activation', 'runs', 'list', 'watch', 'compose', 'inbox', 'inbox-wake', 'runtime-install', 'launch', 'supervisor-start'].includes(command) ? 0 : 1];
  if (!counts.includes(positional.length)) throw new Error('Use the exact session ID; see --help');
  const sessionId = positional[0];
  let result;
  if (command === 'runtime-info') result = runtimeInfo(root);
  if (command === 'activation') {
    const timeout = options['--timeout'];
    if (timeout !== undefined && !(Number(timeout) > 0)) throw new Error('--timeout must be a positive number');
    result = await activationReceipt({ root: options['--registry-root'] ?? root, repo: options['--repo'], managerRoot: options['--manager-root'],
      eddaBin: options['--edda-bin'], clientRoot: options['--client-root'],
      timeoutMs: timeout === undefined ? undefined : Number(timeout) });
    if (options['--check'] === true && result.coherence.status !== 'coherent') process.exitCode = 2;
  }
  if (command === 'runs') result = listManagedRuns(root, { limit: options['--limit'], after: options['--after'] });
  if (command === 'supervisor-start') {
    const config = JSON.parse((await readBoundedFile(options['--config'])).text);
    config.id = validateId(options['--id'] || config.id || randomUUID());
    process.stderr.write(`Supervisor ID: ${config.id}\n`);
    result = await startSupervisor(root, config);
  }
  if (command === 'supervisor-status') result = await supervisorStatus(root, sessionId);
  if (command === 'supervisor-stop') result = await stopSupervisor(root, sessionId);
  if (command === 'runtime-install') result = { status: 'installed', release: installRuntime(root), settingsChanged: false };
  if (command === 'launch') {
    if (!options['--project']) throw new Error('launch requires --project');
    const runId = validateId(options['--run-id'] || randomUUID());
    process.stderr.write(`Run ID: ${runId}\n`);
    result = await launchManaged(root, { runId, project: options['--project'], piEntry: options['--pi-entry'],
      provider: options['--provider'], model: options['--model'], prompt: options['--prompt-file'] ? (await readBoundedFile(options['--prompt-file'])).text : undefined,
      extensions: options['--extension'] ? [options['--extension']] : [], agentDir: options['--agent-dir'], noTools: options['--no-tools'] === true, thinking: options['--thinking'],
      owner: options['--owner'], returnOwner: options['--return-owner'], ownerRoot: options['--owner-root'] });
  }
  if (command === 'run-status') result = await managedStatus(root, sessionId);
  if (command === 'owner') {
    if (positional[0] !== 'adopt') throw new Error("owner supports only the 'adopt' subcommand; use --help");
    if (!options['--run'] || !options['--owner']) throw new Error('owner adopt requires --run and --owner');
    result = await adoptOwner(root, options['--run'], { owner: options['--owner'], returnOwner: options['--return-owner'], ownerRoot: options['--owner-root'] });
  }
  if (command === 'run-conversation') result = await managedConversation(root, sessionId, { after: options['--after'], limit: options['--limit'] });
  if (command === 'run-stop') result = await stopManaged(root, sessionId, { abort: options['--abort'] === true });
  if (command === 'run-resume') result = await resumeManaged(root, sessionId, { runtime: options['--runtime'] });
  if (command === 'inbox') result = listInbox(root, { consumer: options['--consumer'], limit: options['--limit'], after: options['--after'] });
  if (command === 'inbox-read') result = await readInbox(root, sessionId, { consumer: options['--consumer'], budget: options['--budget-bytes'] });
  if (command === 'inbox-ack') result = acknowledgeInbox(root, sessionId, options['--consumer']);
  if (command === 'inbox-wake') result = wakeCapability();
  if (command === 'authorization-record') result = await recordAuthorization(root, sessionId,
    JSON.parse((await readBoundedFile(options['--record'])).text), options['--consumer']);
  if (command === 'authorization-revoke') result = revokeAuthorization(root, sessionId, options['--consumer']);
  if (command === 'inbox-respond') {
    if (Boolean(options['--message']) === Boolean(options['--message-file'])) throw new Error('Supply exactly one of --message or --message-file');
    result = await respondInbox(root, sessionId, { message: options['--message'] || (await readBoundedFile(options['--message-file'])).text,
      authorizationId: options['--authorization'], consumer: options['--consumer'] });
  }
  if (command === 'doctor') result = await doctor(root, sessionId);
  if (command === 'adopt') result = await adoptSession(root, sessionId, { id: options['--task'], project: options['--project'],
    include: options['--include']?.split(',').map((s) => s.trim()), contextFile: options['--context'], capsuleId: options['--capsule'],
    scope: options['--scope'], notify: options['--notify'] === true, maxNotifications: Number(options['--max-notifications'] ?? 10),
    preview: options['--preview'] === true, expectedRevision: options['--expected'],
    eddaCommand: options['--edda-bin'] ? { file: options['--edda-bin'], args: [] } : undefined });
  if (command === 'follow') result = await followDependencies(root, sessionId, { project: options['--project'],
    taskIds: options['--tasks']?.split(',').map((s) => s.trim()), scope: options['--scope'],
    notify: options['--notify'] === true, maxNotifications: Number(options['--max-notifications'] || 10) });
  if (command === 'dependencies' || command === 'check-dependencies') result = await dependencyStatus(root, sessionId, command === 'check-dependencies');
  if (command === 'unfollow') result = await unfollowDependencies(root, sessionId);
  if (command === 'compose') result = await composeHandoff({ project: options['--project'], id: options['--task'],
    contextFile: options['--context'], capsuleId: options['--capsule'], output: options['--output'], root,
    eddaCommand: options['--edda-bin'] ? { file: options['--edda-bin'], args: [] } : undefined });
  if (command === 'list') result = await listSessions(root);
  if (command === 'status') { try { result = await requestSession(root, sessionId, '/status'); } catch (error) { throw registryContext(error, sessionId, root); } }
  if (command === 'receipt') { try { result = await getReceipt(root, sessionId, validateId(options['--id'])); } catch (error) { throw registryContext(error, sessionId, root); } }
  if (command === 'recover') result = recover(root, sessionId, options['--instance']);
  if (command === 'conversation') { try { result = await inspectSession(root, sessionId, { after: options['--after'], limit: options['--limit'] }); } catch (error) { throw registryContext(error, sessionId, root); } }
  if (command === 'enroll') result = await enroll(root, sessionId, options['--scope']);
  if (command === 'watch') result = await watch(root, { withConversation: options['--conversation'] === true });
  if (command === 'brief') result = await managementBrief(root, sessionId, options['--budget-bytes'] || 16384);
  if (command === 'prepare') {
    if (!options['--manifest']) throw new Error('prepare requires --manifest FILE');
    result = await prepareHandoff(root, sessionId, JSON.parse(readFileSync(options['--manifest'], 'utf8')), options['--expected'] || null);
  }
  if (command === 'checkpoint') result = await checkpoint(root, sessionId, { cursor: options['--cursor'], action: options['--action'], note: options['--note'] });
  if (command === 'reply') {
    if (Boolean(options['--message']) === Boolean(options['--message-file'])) throw new Error('Supply exactly one of --message or --message-file');
    result = await reply(root, sessionId, { to: options['--to'], message: options['--message'] || readFileSync(options['--message-file'], 'utf8') });
  }
  if (command === 'send') {
    if (Boolean(options['--message']) === Boolean(options['--message-file'])) throw new Error('Supply exactly one of --message or --message-file');
    const id = validateId(options['--id'] || randomUUID());
    const message = options['--message'] || readFileSync(options['--message-file'], 'utf8');
    // Print the ID BEFORE sending so a transport failure never loses retry identity.
    process.stderr.write(`Message ID: ${id}\n`);
    try {
      result = await requestSession(root, sessionId, '/messages', { id, message,
        sender: options['--sender'] || 'codex', mode: options['--mode'] || 'followUp' });
    } catch (error) { throw registryContext(error, sessionId, root); }
  }
  process.stdout.write(JSON.stringify(result, null, 2) + '\n');
  if (['unknown', 'failed', 'needs_context', 'not_prepared', 'stale_binding', 'unavailable', 'needs_reload', 'needs_enrollment', 'not_registered',
    'ambiguous_session', 'invalid_selector', 'needs_pi', 'offline', 'busy', 'source_changed', 'needs_expected_revision', 'adoption_incomplete', 'unsupported',
    'runner_unreachable', 'launch_pending', 'stop_pending', 'exited', 'capsule_unavailable', 'capsule_invalid', 'capsule_too_large',
    'capsule_wrong_repository', 'capsule_stale'].includes(result?.status)) process.exitCode = 2;
}

main(process.argv.slice(2)).catch((error) => {
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 1;
});

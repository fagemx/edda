#!/usr/bin/env node
import { readFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import { defaultRoot, recover, validateId } from './store.mjs';
import { listSessions, requestSession, getReceipt } from './client.mjs';

const help = `Edda Pi session channel (same-user, same-machine)
  node integrations/pi/cli.mjs list
  node integrations/pi/cli.mjs status SESSION_ID
  node integrations/pi/cli.mjs send SESSION_ID --message TEXT [--id UUID] [--sender codex] [--mode followUp|steer]
  node integrations/pi/cli.mjs send SESSION_ID --message-file PATH [--id UUID]
  node integrations/pi/cli.mjs receipt SESSION_ID --id UUID
  node integrations/pi/cli.mjs recover SESSION_ID --instance UUID

JSON stdout; diagnostics stderr. EDDA_PI_CHANNEL_DIR overrides the private root.
No automatic resume, retries or remote network listener. Keep the message ID.
`;

async function main(args) {
  const [command, ...rest] = args;
  if (!command || command === '--help') { process.stdout.write(help); return; }
  const root = defaultRoot();
  const positional = [];
  const options = {};
  for (let i = 0; i < rest.length; i++) {
    const arg = rest[i];
    if (!arg.startsWith('--')) { positional.push(arg); continue; }
    if (options[arg] !== undefined || rest[i + 1] === undefined || rest[i + 1].startsWith('--')) throw new Error(`Missing or duplicate option ${arg}`);
    options[arg] = rest[++i];
  }
  const allowed = {
    list: [], status: [], send: ['--message', '--message-file', '--id', '--sender', '--mode'],
    receipt: ['--id'], recover: ['--instance'],
  }[command];
  if (!allowed || Object.keys(options).some((key) => !allowed.includes(key))) throw new Error('Unknown command or option; use --help');
  if (positional.length !== (command === 'list' ? 0 : 1)) throw new Error('Use the exact session ID; see --help');
  const sessionId = positional[0];
  let result;
  if (command === 'list') result = await listSessions(root);
  if (command === 'status') result = await requestSession(root, sessionId, '/status');
  if (command === 'receipt') result = await getReceipt(root, sessionId, validateId(options['--id']));
  if (command === 'recover') result = recover(root, sessionId, options['--instance']);
  if (command === 'send') {
    if (Boolean(options['--message']) === Boolean(options['--message-file'])) throw new Error('Supply exactly one of --message or --message-file');
    const id = validateId(options['--id'] || randomUUID());
    const message = options['--message'] || readFileSync(options['--message-file'], 'utf8');
    // Print the ID BEFORE sending so a transport failure never loses retry identity.
    process.stderr.write(`Message ID: ${id}\n`);
    result = await requestSession(root, sessionId, '/messages', { id, message,
      sender: options['--sender'] || 'codex', mode: options['--mode'] || 'followUp' });
  }
  process.stdout.write(JSON.stringify(result, null, 2) + '\n');
  if (result?.status === 'unknown' || result?.status === 'failed') process.exitCode = 2;
}

main(process.argv.slice(2)).catch((error) => {
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 1;
});

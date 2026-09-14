// Synthetic `edda return` CLI for owner-mailbox tests. It mimics the public
// Rust command's records and error strings, backed by a temp cwd's .edda dir.
import { mkdirSync, readdirSync, readFileSync, writeFileSync, existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { createHash } from 'node:crypto';

const args = process.argv.slice(2);
const command = args[0], verb = args[1];
const option = (name) => { const i = args.indexOf(name); return i >= 0 ? args[i + 1] : undefined; };
const base = join(process.cwd(), '.edda', 'returns');
const sha = (value) => createHash('sha256').update(value).digest('hex');
const now = () => new Date().toISOString();
const out = (value) => process.stdout.write(JSON.stringify(value, null, 2) + '\n');
const fail = (message) => { process.stderr.write(message + '\n'); process.exit(1); };
function read(path) { try { return JSON.parse(readFileSync(path, 'utf8')); } catch { return null; } }
function write(path, value) { mkdirSync(dirname(path), { recursive: true }, ); writeFileSync(path, JSON.stringify(value, null, 2)); }

if (process.env.EDDA_FIXTURE_GARBAGE === '1') { process.stdout.write('not json output\n'); process.exit(0); }
if (command !== 'return') fail(`unknown command ${command}`);

const owner = option('--owner');
const ownerFile = () => join(base, 'owners', `${sha(owner)}.json`);
const messagePath = (id) => join(base, 'messages', `${id}.json`);
const claimPath = (id) => join(base, 'claims', `${id}.json`);
const list = (dir) => existsSync(dir) ? readdirSync(dir).filter((n) => n.endsWith('.json')).map((n) => read(join(dir, n))).filter(Boolean) : [];
const unclaimed = (forOwner) => list(join(base, 'messages')).filter((m) => m.owner === forOwner && !existsSync(claimPath(m.id)));

if (verb === 'bind') {
  const session = option('--session'), replaces = option('--replaces-session');
  mkdirSync(join(base, 'owners'), { recursive: true });
  const existing = read(ownerFile());
  if (existing && existing.holder_session !== session && replaces !== existing.holder_session) {
    fail(`owner '${owner}' is held by session '${existing.holder_session}'; pass --replaces-session '${existing.holder_session}' to replace it explicitly`);
  }
  const record = { version: 1, owner, holder_session: session, holder_since: now(),
    replaced_session: existing && existing.holder_session !== session ? existing.holder_session : null };
  write(ownerFile(), record);
  out({ status: 'bound', owner, holderSession: session, replacedSession: record.replaced_session, holderSince: record.holder_since });
} else if (verb === 'status') {
  const record = read(ownerFile());
  const pending = unclaimed(owner).length, all = list(join(base, 'messages')).filter((m) => m.owner === owner).length;
  out({ owner, holder: record ? record.holder_session : null, holderSince: record ? record.holder_since : null, pending, total: all });
} else if (verb === 'post') {
  const work = option('--work'), status = option('--status'), session = option('--session');
  const result = option('--result') ?? null, deliverable = option('--deliverable') ?? null, message = option('--message-file') ?? null;
  const id = sha([owner, work, status, session, message ?? '', result ?? '', deliverable ?? ''].join('\u0000'));
  if (!existsSync(messagePath(id))) {
    write(messagePath(id), { version: 1, id, owner, work, status, result, deliverable, message, posted_by_session: session, posted_at: now() });
  }
  out({ status: 'posted', messageId: id, owner, work, state: status });
} else if (verb === 'pending') {
  const pending = unclaimed(owner);
  out({ owner, pending, count: pending.length });
} else if (verb === 'claim') {
  const session = option('--session');
  const record = read(ownerFile());
  if (!record) fail(`no owner binding for '${owner}'; bind before claiming`);
  if (record.holder_session !== session) {
    fail(`owner '${owner}' is held by session '${record.holder_session}', not '${session}'; a superseded session cannot claim`);
  }
  const claimed = [];
  for (const message of unclaimed(owner)) {
    write(claimPath(message.id), { version: 1, message_id: message.id, claimed_by_session: session, claimed_at: now() });
    claimed.push(message);
  }
  out({ status: 'claimed', owner, session, returns: claimed, count: claimed.length });
} else {
  fail(`unknown return operation ${verb}`);
}

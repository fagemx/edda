import { mkdirSync, lstatSync, readdirSync, existsSync, linkSync, unlinkSync } from 'node:fs';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { privateRoot, digest, readJson, writeJson } from './store.mjs';

export const inboxId = (id) => {
  if (typeof id !== 'string' || !/^[0-9a-f]{64}$/.test(id)) throw new Error('Inbox identity must be a 64-character lowercase digest');
  return id;
};
export const messageId = (id) => `${id.slice(0, 8)}-${id.slice(8, 12)}-5${id.slice(13, 16)}-a${id.slice(17, 20)}-${id.slice(20, 32)}`;
export const wakeCapability = () => ({ status: 'unsupported', notified: false,
  reason: 'No existing-desktop-host wake adapter is configured. Persisted events require an active manager to read them; no process or model was started.' });
const kinds = ['events', 'pending', 'acks', 'responses', 'authorizations', 'revocations'];
export function inboxStore(root, create = false) {
  const directory = join(root, 'inbox');
  if (create) privateRoot(root);
  for (const path of [directory, ...kinds.map((kind) => join(directory, kind))]) {
    if (create) mkdirSync(path, { recursive: true, mode: 0o700 });
    if (existsSync(path) && (!lstatSync(path).isDirectory() || lstatSync(path).isSymbolicLink())) throw new Error('Inbox directory must not be a link');
  }
  const path = (kind, id) => {
    if (!kinds.includes(kind)) throw new Error('Unknown inbox record kind');
    return join(directory, kind, `${inboxId(id)}.json`);
  };
  function read(kind, id) {
    const value = readJson(path(kind, id));
    if (value && (value.version !== 1 || value.id !== id || value.fingerprint !== digest(JSON.stringify(value.data)))) {
      throw new Error('Inbox record identity/content mismatch');
    }
    return value;
  }
  function put(kind, id, data) {
    const fingerprint = digest(JSON.stringify(data));
    const previous = read(kind, id);
    if (previous) {
      if (previous.fingerprint !== fingerprint) throw new Error('Inbox record identity conflict');
      return previous;
    }
    const record = { version: 1, id, fingerprint, createdAt: new Date().toISOString(), data };
    if (Buffer.byteLength(JSON.stringify(record)) > 32768) throw new Error('Inbox record exceeds 32 KiB');
    const target = path(kind, id), temp = `${target}.${randomUUID()}.tmp`;
    try {
      writeJson(temp, record, true);
      try { linkSync(temp, target); }
      catch (error) {
        if (error.code !== 'EEXIST') throw error;
        if (read(kind, id).fingerprint !== fingerprint) throw new Error('Concurrent inbox record conflict');
      }
      return read(kind, id);
    } finally { if (existsSync(temp)) unlinkSync(temp); }
  }
  const list = (kind) => existsSync(join(directory, kind)) ? readdirSync(join(directory, kind))
    .filter((name) => /^[0-9a-f]{64}\.json$/.test(name)).map((name) => read(kind, name.slice(0, -5))) : [];
  return { read, put, list,
    publish(id, data) {
      const previous = read('events', id);
      if (previous) return put('events', id, data);
      put('pending', id, data);
      const result = put('events', id, data);
      unlinkSync(path('pending', id));
      return result;
    },
    recover() {
      for (const pending of list('pending')) {
        put('events', pending.id, pending.data);
        unlinkSync(path('pending', pending.id));
      }
    },
  };
}

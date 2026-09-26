import { copyFileSync, constants, lstatSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { readRecord, sessionDir, validateId } from './store.mjs';

// A reconnect replaces state.json. Keep an unreadable original before permitting
// that replacement; refuse if the source is not a regular file or the backup
// already exists. Receipts and other records are never copied or deleted.
export function preserveDamagedState(root, sessionId, instanceId) {
  const path = join(sessionDir(root, sessionId), 'state.json');
  const record = readRecord(path);
  if (!record.error) return null;
  const stat = lstatSync(path);
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 1024 * 1024) {
    throw new Error('Unreadable state is not a bounded regular file; recovery refused');
  }
  const backup = `${path}.${validateId(instanceId)}.damaged`;
  try { copyFileSync(path, backup, constants.COPYFILE_EXCL); }
  catch (error) {
    if (error.code !== 'EEXIST' || !lstatSync(backup).isFile() ||
        !readFileSync(path).equals(readFileSync(backup))) throw error;
  }
  return backup;
}

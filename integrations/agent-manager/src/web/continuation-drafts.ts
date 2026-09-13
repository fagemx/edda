export interface PendingReference { route: '' | '/import' | '/takeover'; actionId: string; revision: string }
const id = /^[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}$/;
const uuid = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/;
/** Only identifiers cross the browser persistence boundary. Never raw context. */
export function encodeReferences(values: Record<string, PendingReference | null>): string {
  return JSON.stringify({ version: 2, operations: Object.entries(values).flatMap(([workId, value]) => {
    if (!value) return [];
    if (!id.test(workId) || !uuid.test(value.actionId) || !/^[a-f0-9]{64}$/.test(value.revision) || !['', '/import', '/takeover'].includes(value.route)) throw new Error('Invalid continuation reference');
    return [{ workId, route: value.route, actionId: value.actionId, revision: value.revision }];
  }) });
}
export function decodeReferences(raw: string): Record<string, PendingReference> {
  const value = JSON.parse(raw) as { version?: number; operations?: Array<PendingReference & { workId: string }> };
  if (value.version !== 2 || !Array.isArray(value.operations) || value.operations.length > 32) throw new Error('Invalid continuation references');
  const result: Record<string, PendingReference> = {};
  for (const row of value.operations) result[row.workId] = { route: row.route, actionId: row.actionId, revision: row.revision };
  encodeReferences(result); return result;
}

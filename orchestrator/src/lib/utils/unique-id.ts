import { randomUUID } from 'node:crypto';

/** Generate a short unique ID (8 hex chars) */
export function uniqueId(): string {
  return randomUUID().slice(0, 8);
}

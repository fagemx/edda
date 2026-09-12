// External CLI stand-in for process-boundary tests. It only reads a fixture.
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
const [command, verb, id, format] = process.argv.slice(2);
if (command !== 'task' || verb !== 'show' || format !== '--json' || !/^[0-9]+$/.test(id)) process.exit(71);
process.stdout.write(readFileSync(join(process.cwd(), 'task.json'), 'utf8'));

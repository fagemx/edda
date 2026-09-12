// Process-boundary fixture: reads only the selected synthetic task file.
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
const [command, verb, id, format] = process.argv.slice(2);
if (command !== 'task' || verb !== 'show' || format !== '--json' || !/^[0-9]+$/.test(id)) process.exit(71);
process.stdout.write(readFileSync(join(process.cwd(), `task-${id}.json`), 'utf8'));

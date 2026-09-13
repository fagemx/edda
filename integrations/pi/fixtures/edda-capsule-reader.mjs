// External CLI stand-in for process-boundary tests. It only reads fixture files.
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
const [command, verb, id, format] = process.argv.slice(2);
const read = (name) => readFileSync(join(process.cwd(), name), 'utf8');
try {
  if (command === 'task' && verb === 'show' && format === '--json' && /^[0-9]+$/.test(id)) {
    process.stdout.write(read(`task-${id}.json`));
  } else if (command === 'continuity' && verb === 'list' && id === '--json') {
    process.stdout.write(read('capsules-list.json'));
  } else if (command === 'continuity' && verb === 'restore' && format === '--json' && /^cap_[a-z0-9]+$/.test(id)) {
    process.stdout.write(read(`capsule-${id}.json`));
  } else {
    process.exit(71);
  }
} catch {
  process.exit(72);
}

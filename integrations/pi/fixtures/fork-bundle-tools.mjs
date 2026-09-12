// Experiment-only tool surface: one assigned output, automatic test feedback.
import { readFileSync, writeFileSync, realpathSync, lstatSync } from 'node:fs';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';
import { writeJson } from '../store.mjs';
export default function (pi) {
  const cwd = realpathSync(process.cwd());
  const config = JSON.parse(readFileSync(join(cwd, '.fork-bundle.json'), 'utf8'));
  if (!['cards', 'attention', 'overview'].includes(config.module)) throw new Error('Invalid experimental module');
  const output = join(cwd, `${config.module}.mjs`), contract = readFileSync(join(cwd, 'CONTRACT.md'), 'utf8');
  const validate = () => {
    try {
      const stdout = execFileSync(process.execPath, [config.acceptance, cwd, config.module],
        { encoding: 'utf8', windowsHide: true, timeout: 15000, stdio: ['ignore', 'pipe', 'pipe'] });
      return { accepted: true, output: stdout.trim() };
    } catch (error) { return { accepted: false, output: String(error.stderr || error.message).slice(0, 5000) }; }
  };
  const answer = (value) => ({ content: [{ type: 'text', text: JSON.stringify(value) }] });
  pi.on('before_agent_start', () => { pi.setActiveTools(['fork_read', 'fork_submit']); });
  pi.registerTool({ name: 'fork_read', label: 'Read assigned coding bundle',
    description: 'Read the frozen specification, current assigned module and verified dependency if present.',
    parameters: { type: 'object', properties: {}, additionalProperties: false },
    async execute() { return answer({ module: config.module, contract, current: readFileSync(output, 'utf8'),
      dependency: config.dependency || null }); } });
  let attempts = 0;
  pi.registerTool({ name: 'fork_submit', label: 'Submit assigned module and run acceptance',
    description: 'Write the complete source of your one assigned module and receive real acceptance results immediately. Correct failures here. No separate approval or test shell needed.',
    parameters: { type: 'object', properties: { code: { type: 'string', minLength: 1, maxLength: 65536 } }, required: ['code'], additionalProperties: false },
    async execute(_id, args) {
      if (++attempts > 4) return answer({ accepted: false, stopped: true, reason: 'Four submission attempts exhausted; report the remaining failure.' });
      if (typeof args.code !== 'string' || Buffer.byteLength(args.code) > 65536 || lstatSync(output).isSymbolicLink()) throw new Error('Invalid experimental output');
      writeFileSync(output, args.code);
      const result = validate();
      writeJson(join(config.receipts, `submission-${attempts}.json`), { at: Date.now(), attempts, ...result });
      return answer(result);
    } });
}

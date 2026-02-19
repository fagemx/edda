import { Command } from 'commander';
import { runCommand } from './commands/run/index.js';
import { statusCommand } from './commands/status/index.js';
import { retryCommand } from './commands/retry/index.js';
import { skipCommand } from './commands/skip/index.js';
import { abortCommand } from './commands/abort/index.js';

const program = new Command();

program
  .name('orchestrate')
  .description('edda Orchestrator — multi-phase plan execution')
  .version('0.1.0');

program.addCommand(runCommand);
program.addCommand(statusCommand);
program.addCommand(retryCommand);
program.addCommand(skipCommand);
program.addCommand(abortCommand);

program.parse();

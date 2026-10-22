#!/usr/bin/env node
/**
 * Allternit Command Line Interface
 * 
 * TypeScript implementation ported from Rust CLI
 * Integrated with GizziClaw workspace system
 */

import { Command } from 'commander';
import { normalizeApiAlias } from './argv.js';
import { runCommand } from './commands/run.js';
import { replCommand } from './commands/repl.js';
import { shellCommand } from './commands/shell.js';
import { statusCommand } from './commands/status.js';
import { skillsCommand } from './commands/skills.js';
import { workspaceCommand } from './commands/workspace.js';
import { vmCommand } from './commands/vm.js';
import { pluginsCommand } from './commands/plugins.js';
import { computerUseCommand } from './commands/computer-use.js';
import { agentCommand } from './commands/agent.js';
import { adminCommand } from './commands/admin.js';
import { cloudCommand } from './commands/cloud.js';
import { fabricCommand } from './commands/fabric.js';
import { computersCommand } from './commands/computers.js';
import { subsCommand } from './commands/subs.js';
import { capsCommand } from './commands/caps.js';
import { taskCommand } from './commands/task.js';
import { artifactsCommand } from './commands/artifacts.js';
import { version } from '../package.json' with { type: 'json' };

const program = new Command();

program
  .name('allternit')
  .description('Allternit - AI-native runtime environment')
  .version(version)
  .option('--api-url <url>', 'Allternit API base URL (alias: --api)', process.env.ALLTERNIT_API_URL ?? 'http://127.0.0.1:8013')
  .option('--token <token>', 'Clerk bearer token', process.env.ALLTERNIT_TOKEN)
  .option('--json', 'emit machine-readable JSON');

// `--api` is accepted as a short alias for `--api-url` (commander derives the
// option attribute from the last long flag, so a comma alias would rename it).
process.argv = normalizeApiAlias(process.argv);

// Register all commands
program.addCommand(runCommand);
program.addCommand(replCommand);
program.addCommand(shellCommand);
program.addCommand(statusCommand);
program.addCommand(skillsCommand);
program.addCommand(workspaceCommand);
program.addCommand(vmCommand);
program.addCommand(pluginsCommand);
program.addCommand(computerUseCommand);
program.addCommand(agentCommand);
program.addCommand(adminCommand);
program.addCommand(cloudCommand);
program.addCommand(fabricCommand);
program.addCommand(computersCommand);
program.addCommand(subsCommand);
program.addCommand(capsCommand);
program.addCommand(taskCommand);
program.addCommand(artifactsCommand);

// Parse and execute
program.parse();

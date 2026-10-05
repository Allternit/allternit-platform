import type { CommandModule } from 'yargs';
import { runtimeListCommand } from './runtime/list';
import { runtimeRegisterCommand } from './runtime/register';
import { runtimeStatusCommand } from './runtime/status';
import { runtimeDaemonCommand } from './runtime/daemon';

/**
 * Gizzi's local agent-runtime registry (folded in from the old
 * `gizzi runtime`). Mounted under `gizzi agents harness` next to the
 * engine's `harness list|sync`: `runtimes` is the old `runtime list`
 * (renamed so it doesn't shadow the engine's `harness list`).
 */
export const RuntimeLocalCommands: CommandModule[] = [
  {
    command: 'runtimes',
    describe: "List the runtimes registered in Gizzi's local registry",
    handler: async () => { await runtimeListCommand(); },
  },
  {
    command: 'register [name] [host]',
    describe: 'Discover local agent CLIs and register them as a Gizzi runtime',
    builder: (y) =>
      y
        .positional('name', { type: 'string', default: 'local', describe: 'Runtime name' })
        .positional('host', { type: 'string', default: 'localhost', describe: 'Runtime host' }),
    handler: async (argv) => { await runtimeRegisterCommand([argv.name as string, argv.host as string]); },
  },
  {
    command: 'status [id]',
    describe: "Show Gizzi's local runtime registry status",
    builder: (y) => y.positional('id', { type: 'string', describe: 'Runtime ID (optional)' }),
    handler: async (argv) => { await runtimeStatusCommand(argv.id ? [argv.id as string] : []); },
  },
  {
    command: 'daemon',
    describe: 'Start a WebSocket runtime daemon for this machine',
    builder: (y) =>
      y
        .option('host', { type: 'string', default: '127.0.0.1', describe: 'Bind host' })
        .option('port', { type: 'number', describe: 'Bind port (0 = random)' })
        .option('name', { type: 'string', describe: 'Runtime display name' }),
    handler: async (argv) => {
      const args: string[] = [];
      if (argv.host) args.push(`--host=${argv.host}`);
      if (argv.port) args.push(`--port=${argv.port}`);
      if (argv.name) args.push(`--name=${argv.name}`);
      await runtimeDaemonCommand(args);
    },
  },
];

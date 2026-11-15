import type { CommandModule } from 'yargs';
import { runtimeListCommand } from './runtime/list';
import { runtimeRegisterCommand } from './runtime/register';
import { runtimeStatusCommand } from './runtime/status';
import { runtimeDaemonCommand } from './runtime/daemon';
import { selfHostedRegister, selfHostedStatus } from './runtime/selfhost';
import { createProfileStore } from '../../runtime/selfhost/profiles';

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
    describe: 'Discover local agent CLIs and register them as a Gizzi runtime, or save a self-hosted Agents Server (--self-hosted)',
    builder: (y) =>
      y
        .positional('name', { type: 'string', default: 'local', describe: 'Runtime name (with --self-hosted: the profile name)' })
        .positional('host', { type: 'string', default: 'localhost', describe: 'Runtime host' })
        .option('self-hosted', { type: 'boolean', default: false, describe: 'save a self-hosted Agents Server profile' })
        .option('server', { type: 'string', describe: 'self-hosted server URL (https://, or http:// on a private address)' })
        .option('api-key', { type: 'string', describe: 'server API key ask_... (env ALLTERNIT_SERVER_API_KEY); kept in the credential store' }),
    handler: async (argv) => {
      if (argv.selfHosted) {
        const named = typeof argv.name === 'string' && argv.name !== 'local' ? argv.name : undefined;
        process.exitCode = await selfHostedRegister({
          server: argv.server as string | undefined,
          apiKey: argv.apiKey as string | undefined,
          name: named,
        });
        return;
      }
      await runtimeRegisterCommand([argv.name as string, argv.host as string]);
    },
  },
  {
    command: 'status [id]',
    describe: "Show Gizzi's local runtime registry status, and a saved self-hosted server's environments, workers and queue depth",
    builder: (y) =>
      y
        .positional('id', { type: 'string', describe: 'Runtime ID (optional)' })
        .option('self-hosted', { type: 'boolean', default: false, describe: 'show the saved self-hosted server only' })
        .option('server', { type: 'string', describe: 'saved self-hosted profile name or URL (default: the default profile)' })
        .option('json', { type: 'boolean', default: false, describe: 'print the self-hosted status as JSON' }),
    handler: async (argv) => {
      const wantsSelfHosted = argv.selfHosted === true || typeof argv.server === 'string' || argv.json === true;
      if (wantsSelfHosted) {
        process.exitCode = await selfHostedStatus({ server: argv.server as string | undefined, json: argv.json === true });
        return;
      }
      if (!argv.id && (await createProfileStore().get())) {
        await selfHostedStatus({});
      }
      await runtimeStatusCommand(argv.id ? [argv.id as string] : []);
    },
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

/**
 * Top-level `gizzi runtime`: the same commands as `gizzi agents harness`
 * (runtimes, register, status, daemon). The self-hosted Agents Server docs use
 * `gizzi runtime register --self-hosted` and `gizzi runtime status`.
 */
export const RuntimeCommand: CommandModule = {
  command: 'runtime',
  describe: "Gizzi's runtime registry and self-hosted Agents Server profiles (register --self-hosted, status)",
  builder: (y) => {
    for (const sub of RuntimeLocalCommands) y.command(sub);
    return y.demandCommand(1, 'Pick a subcommand: runtimes, register, status or daemon');
  },
  handler: () => {},
};

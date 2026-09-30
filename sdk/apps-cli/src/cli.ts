import { parseArgs } from "./args.js";
import { create } from "./commands/create.js";
import { dev } from "./commands/dev.js";
import { packageCmd } from "./commands/package.js";
import { domain, submit } from "./commands/submit.js";
import { test } from "./commands/test.js";
import { CliError, type Context } from "./context.js";

export const HELP = `allternit <command>

  create <name>                     scaffold an app (same as create-allternit-app)
  dev [entry] [--port 3000]         run the server and print how to connect it
  test [entry] [--url U] [--strict] directory scan rules + annotation checks + live MCP round trip
  package [entry] --url U [--out f] build the plugin zip
  domain <host> [--check]           domain verification for submit
  submit <zip> [--dry-run]          submit for directory review (needs ALLTERNIT_TOKEN)
`;

/** Returns the process exit code. `dev` keeps running and returns 0 immediately after starting. */
export async function run(ctx: Context, argv: string[]): Promise<number> {
  const [command, ...rest] = argv;
  try {
    switch (command) {
      case "create":
        create(ctx, parseArgs(rest).positional[0]);
        return 0;
      case "dev":
        await dev(ctx, rest);
        return 0;
      case "test":
        return await test(ctx, rest);
      case "package":
        await packageCmd(ctx, rest);
        return 0;
      case "domain":
        await domain(ctx, rest);
        return 0;
      case "submit":
        await submit(ctx, rest);
        return 0;
      case undefined:
      case "help":
      case "--help":
        ctx.log(HELP);
        return 0;
      default:
        ctx.error(`Unknown command: ${command}\n\n${HELP}`);
        return 1;
    }
  } catch (err) {
    ctx.error(err instanceof CliError ? err.message : err instanceof Error ? err.stack ?? err.message : String(err));
    return 1;
  }
}

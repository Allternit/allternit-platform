import "./devMacro"
import "drizzle-orm/sqlite-core/db.js"
import "drizzle-orm/sqlite-core/session.js"
import "drizzle-orm/bun-sqlite/session.js"
import yargs from "yargs"
import { hideBin } from "yargs/helpers"
import { Log } from "@/shared/util/log"
import { UI } from "@/cli/ui"
import { Installation } from "@/shared/installation"
import { NamedErrorBase } from "@allternit/gizzi-util/error.js"
import { FormatError } from "@/shared/error/format"
import { Filesystem } from "@/shared/util/filesystem"
import { EOL } from "os"
import { COMMANDS } from "@/cli/commands/registry"
import { CompletionsCommand } from "@/cli/commands/completions"
// RemoteCommand is part of the COMMANDS registry (src/cli/commands/registry.ts).
import {
  OnboardingCommand,
  defaultOnboardingDeps,
  runOnboardingWizard,
  shouldOfferFirstRunOnboarding,
} from "@/cli/commands/onboarding"
import { CIMode } from "@/cli/ci"
import path from "path"
import { Global, init as initGlobal } from "@/runtime/context/global"
import { JsonMigration } from "@/runtime/session/storage/json-migration"
import { Database } from "@/runtime/session/storage/db"
import { RuntimeTelemetry } from "@/runtime/telemetry"
import { telemetryDisabled } from "@/runtime/telemetry/privacy"
import {
  hasTelemetryNoticeBeenShown,
  markTelemetryNoticeShown,
} from "@/shared/utils/telemetrySettings"
import { getActiveSubsystems, getHarnessMode, shouldUseHarness } from "@/cli/ui/ink-app/utils/feature-flags"
import { Workspace } from "@/runtime/workspace/workspace"
import { getIsInteractive } from "@/cli/ui/ink-app/utils/feature-flags"
// ResolveMessage is a global class from bun-types
declare class ResolveMessage {
  readonly name: "ResolveMessage"
  readonly position: { line: number; column: number } | null
  readonly code: string
  readonly message: string
  readonly referrer: string
  readonly specifier: string
  readonly importKind: "entry_point" | "stmt" | "require" | "import" | "dynamic" | "require_resolve"
}

process.on("unhandledRejection", (e) => {
  Log.Default.error("rejection", {
    e: e instanceof Error ? e.message : e,
  })
})

process.on("uncaughtException", (e) => {
  Log.Default.error("exception", {
    e: e instanceof Error ? e.message : e,
  })
})

const cli = yargs(hideBin(process.argv))
  .parserConfiguration({ "populate--": true })
  .scriptName("gizzi")
  .wrap(100)
  .help("help", "show help")
  .alias("help", "h")
  .version("version", "show version number", Installation.VERSION)
  .alias("version", "v")
  .option("print-logs", {
    describe: "print logs to stderr",
    type: "boolean",
  })
  .option("log-level", {
    describe: "log level",
    type: "string",
    choices: ["DEBUG", "INFO", "WARN", "ERROR"],
  })
  .option("onboarding", {
    describe: "force the setup onboarding wizard",
    type: "boolean",
  })
  .option("ci", {
    describe: "run in CI/non-interactive mode (structured output, no prompts)",
    type: "boolean",
  })
  .option("ci-format", {
    describe: "CI output format",
    choices: ["ndjson", "text", "markdown"],
    default: "ndjson",
  })
  .middleware(async (opts) => {
    await initGlobal()
    await Log.init({
      print: process.argv.includes("--print-logs"),
      dev: Installation.isLocal(),
      level: (() => {
        if (opts.logLevel) return opts.logLevel as Log.Level
        if (Installation.isLocal()) return "DEBUG"
        return "INFO"
      })(),
    })
    RuntimeTelemetry.attach((record) => {
      Log.Default.info("runtime.telemetry", record)
    })

    // First-run telemetry disclosure: one line on the first interactive
    // launch when telemetry is enabled. Never prints when telemetry is
    // disabled via env/settings, in CI, or once already acknowledged.
    try {
      if (
        getIsInteractive() &&
        !CIMode.isCIEnvironment() &&
        !telemetryDisabled() &&
        !hasTelemetryNoticeBeenShown()
      ) {
        process.stderr.write(
          "gizzi telemetry: anonymous usage stats are enabled (never prompts, file contents, or credentials). " +
            "Disable with `gizzi config telemetry off` or GIZZI_TELEMETRY=off. " +
            "Details: cmd/gizzi-code/docs/telemetry.md\n"
        )
        markTelemetryNoticeShown()
      }
    } catch {
      // Disclosure is best-effort; never block startup.
    }

    // Activate CI mode if --ci flag or CI environment detected
    if (opts.ci || CIMode.isCIEnvironment()) {
      CIMode.activate({
        format: opts.ciFormat as "ndjson" | "text" | "markdown" | undefined,
      })
    }

    // Onboarding wizard:
    // - `--onboarding` forces it (runs to completion, then exits).
    // - Otherwise it auto-triggers once on the first interactive launch when
    //   no config.toml exists and no auth is configured. The wizard sets a
    //   marker in the state dir so it never nags again; CI/non-interactive
    //   launches skip it (the wizard itself prints a graceful skip note).
    try {
      const onboardingDeps = await defaultOnboardingDeps()
      onboardingDeps.isCI = onboardingDeps.isCI || CIMode.isActive() || opts.ci === true
      const noSubcommand =
        hideBin(process.argv).filter((t) => !t.startsWith("-")).length === 0
      const explicit = opts.onboarding === true
      if (explicit || (noSubcommand && (await shouldOfferFirstRunOnboarding(onboardingDeps)))) {
        await runOnboardingWizard(onboardingDeps)
        if (explicit) {
          // Never fall through to command dispatch/help after a forced run.
          // Drain stdout/stderr briefly, then exit.
          setTimeout(() => process.exit(process.exitCode ?? 0), 100)
          await new Promise(() => {})
        }
      }
    } catch (e) {
      // Onboarding is best-effort; never block startup.
      Log.Default.warn("onboarding failed", { error: e instanceof Error ? e.message : String(e) })
    }

    process.env.AGENT = "1"
    process.env.GIZZI = "1"
    process.env.GIZZI = "1"

    const harnessMode = getHarnessMode()
    const harnessActive = shouldUseHarness()
    const subsystems = getActiveSubsystems()
    Log.Default.info("gizzi", {
      version: Installation.VERSION,
      args: process.argv.slice(2),
      harness: {
        mode: harnessMode,
        active: harnessActive,
      },
      subsystems,
    })

    // Initialize .gizzi agent workspace if missing. This is the building
    // block of agent personality (IDENTITY.md, SOUL.md, USER.md, MEMORY.md,
    // AGENTS.md). Interactive users are offered the starter; non-interactive
    // runs silently create the global workspace.
    const localWorkspace = path.join(process.cwd(), ".gizzi")
    const globalWorkspace = Workspace.globalPath
    const localExists = await Filesystem.exists(localWorkspace)
    const globalExists = await Filesystem.exists(globalWorkspace)
    if (!localExists && !globalExists) {
      try {
        if (getIsInteractive()) {
          Log.Default.info("workspace", { msg: "no .gizzi workspace found; run `gizzi init` to create one" })
        } else {
          await Workspace.init(globalWorkspace)
          Log.Default.info("workspace", { msg: "created global .gizzi workspace", path: globalWorkspace })
        }
      } catch (e) {
        Log.Default.warn("workspace init failed", { error: e instanceof Error ? e.message : String(e) })
      }
    }

    const marker = path.join(Global.Path.data, "gizzi.db")
    /*
    if (!(await Filesystem.exists(marker))) {
      const tty = process.stderr.isTTY
...
      process.stderr.write("Database migration complete." + EOL)
    }
    */
  })
  .usage("\n" + UI.logo())
  // Shell completions come from the single registry-derived generator in
  // src/cli/commands/completions (the yargs `.completion()` built-in was
  // removed as a redundant second system).
  // TODO(types): RegisteredCommand is deliberately narrower than yargs
  // CommandModule (see the doc comment in src/cli/commands/registry.ts);
  // main.ts is the only consumer that hands the registry to yargs.
  .command(COMMANDS as unknown as import("yargs").CommandModule[])
  .command(CompletionsCommand)
  .command(OnboardingCommand)
  .fail((msg, err) => {
    if (
      msg?.startsWith("Unknown argument") ||
      msg?.startsWith("Not enough non-option arguments") ||
      msg?.startsWith("Invalid values:")
    ) {
      if (err) throw err
      cli.showHelp("log")
    }
    if (err) throw err
    process.exit(1)
  })
  .strict()

// Commands that legitimately keep the process alive (TUI, servers, protocol
// stdio loops, attach/streaming forms). Everything NOT matched here is a
// one-shot command: its handler prints and returns, and the process must
// then exit. Bootstrap leaves live runtime handles (agent comms runtime,
// db watchers, lazily re-created Instances) that would otherwise hold the
// event loop open forever — the same disease print-mode `gizzi exec` had
// before it grew its own drain-then-exit. After the yargs handler resolves,
// one-shot commands get the same treatment centrally: drain stdout/stderr
// for 100ms, then exit with whatever exitCode the handler set.
function isLongLivedCommand(argv: { _: (string | number)[]; print?: boolean }): boolean {
  const [top, sub] = argv._ as string[]
  if (!top) return true // no subcommand → default TUI ($0 [project])
  switch (top) {
    case "run":
      // `run --print/-p` is one-shot and self-exits (run.ts); interactive
      // run starts the TUI and runs until the user quits.
      return !argv.print
    case "exec":
      // Print-only alias of run; already self-exits in run.ts print mode.
      return false
    case "attach": // attach to a running gizzi server (interactive TUI)
    case "serve": // headless gizzi server; runs until SIGINT/SIGTERM
    case "server": // future alias of serve; never force-exit
    case "web": // starts a local web server and blocks
    case "acp": // Agent Client Protocol server over stdio
    case "ssh": // future remote shell; interactive
    case "assistant": // future interactive assistant REPL
    case "remote-control": // future remote control session
    case "fabric-worker": // Fabric Transport worker daemon; runs until SIGTERM/SIGINT
      return true
    case "mcp":
      // `mcp serve` runs an MCP server; add/remove/list/auth/logout/debug
      // are one-shot.
      return sub === "serve"
    case "agents": {
      // `agents bot chat <name>` with no message launches the interactive
      // TUI; `agents harness daemon` is a WebSocket daemon. Engine
      // pass-through verbs (wall, attach, drive, feed) await the engine's
      // exit inside their handler, so they are one-shot here.
      const [, , third] = argv._ as string[]
      if (sub === "bot") return third === "chat" && argv._.length <= 4
      return sub === "harness" && third === "daemon"
    }
    case "workflows": {
      // `workflows wake jobs start` is the local job daemon (also the
      // supervised entrypoint).
      const [, , third, fourth] = argv._ as string[]
      return sub === "wake" && third === "jobs" && fourth === "start"
    }
    case "workspace": {
      // `workspace tasks attach <run-id>` attaches to a live run.
      const [, , third] = argv._ as string[]
      return sub === "tasks" && third === "attach"
    }
    case "cron":
      // Hidden: `cron start` as written into older launchd/systemd units.
      return sub === "start"
    case "remote":
      // `remote connect` opens an interactive remote session and
      // `remote logs` streams; list/status/test/setup/config exit.
      return sub === "connect" || sub === "logs"
    default:
      return false
  }
}

let parsedArgv: { _: (string | number)[]; print?: boolean } | undefined
try {
  parsedArgv = await cli.parse()
} catch (e) {
  let data: Record<string, any> = {}
  if (e instanceof NamedErrorBase) {
    const obj = e.toObject()
    Object.assign(data, {
      ...obj.data,
    })
  }

  if (e instanceof Error) {
    Object.assign(data, {
      name: e.name,
      message: e.message,
      cause: e.cause?.toString(),
      stack: e.stack,
    })
  }

  if (e instanceof ResolveMessage) {
    Object.assign(data, {
      name: e.name,
      message: e.message,
      code: e.code,
      specifier: e.specifier,
      referrer: e.referrer,
      position: e.position,
      importKind: e.importKind,
    })
  }
  Log.Default.error("fatal", data)
  const formatted = FormatError(e)
  if (formatted) UI.error(formatted)
  if (formatted === undefined) {
    UI.error("Unexpected error, check log file at " + Log.file() + " for more details" + EOL)
    process.stderr.write((e instanceof Error ? (e.stack || e.message) : String(e)) + EOL)
  }
  process.exitCode = 1
}

// On parse failure we have no parsed argv — recover the command path from
// raw tokens (first non-option arg) so long-lived commands that error early
// still don't get a forced exit scheduled underneath them.
const fallbackArgv = { _: hideBin(process.argv).filter((t) => !t.startsWith("-")) as (string | number)[] }
if (!isLongLivedCommand(parsedArgv ?? fallbackArgv)) {
  setTimeout(() => process.exit(process.exitCode ?? 0), 100)
}
// Sat Mar 14 17:36:51 CDT 2026
// force recompile Sat Mar 14 17:51:09 CDT 2026
// force recompile Sat Mar 14 17:52:07 CDT 2026
// force recompile Sat Mar 14 21:34:16 CDT 2026
// force Sat Mar 14 21:39:31 CDT 2026
// force Sat Mar 14 21:39:52 CDT 2026
// force recompile Sat Mar 14 21:40:25 CDT 2026
// force Sat Mar 14 21:47:35 CDT 2026

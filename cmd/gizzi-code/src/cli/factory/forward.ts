/**
 * Forwarding glue: turns a `gizzi <part> <verb> …` invocation into the same
 * `allternit-factory <part> <verb> …` call, prints the result (rendered, or
 * the engine's raw JSON with --json) and preserves the engine's exit code.
 */
import type { Argv, CommandModule } from "yargs"
import { hideBin } from "yargs/helpers"
import {
  EXIT,
  FactoryEngineError,
  passThrough,
  runEngine,
  type RunEngineOptions,
} from "@/cli/factory/engine"
import { renderDocument, renderError } from "@/cli/factory/render"

export type FactoryPart = "agents" | "orchestration" | "workflows" | "workspace"

/** Gizzi's own root options (main.ts) that must not reach the engine. */
const GIZZI_FLAGS_WITH_VALUE = new Set(["--log-level"])
const GIZZI_FLAGS = new Set(["--print-logs", "--onboarding", "--ci"])

/**
 * The tokens after `<part>` on the real command line, with Gizzi's own root
 * flags and `--json` removed. Raw tokens (not the yargs-parsed object) so
 * the engine receives exactly what the user typed — same verbs, same order,
 * same flags — including ones Gizzi doesn't know about.
 */
export function forwardedArgs(part: FactoryPart, argv: string[] = hideBin(process.argv)): { args: string[]; json: boolean } {
  const at = argv.indexOf(part)
  const rest = at === -1 ? [] : argv.slice(at + 1)
  const args: string[] = []
  let json = false
  for (let i = 0; i < rest.length; i++) {
    const tok = rest[i]!
    if (tok === "--json" || tok === "--json=true") {
      json = true
      continue
    }
    if (tok === "--json=false") continue
    if (GIZZI_FLAGS.has(tok)) continue
    if (GIZZI_FLAGS_WITH_VALUE.has(tok)) {
      i++
      continue
    }
    if ([...GIZZI_FLAGS_WITH_VALUE].some((f) => tok.startsWith(f + "="))) continue
    args.push(tok)
  }
  return { args: [part, ...args], json }
}

export interface ForwardIO {
  stdout: (s: string) => void
  stderr: (s: string) => void
  color: boolean
}

const defaultIO = (): ForwardIO => ({
  stdout: (s) => process.stdout.write(s + "\n"),
  stderr: (s) => process.stderr.write(s + "\n"),
  color: !!process.stdout.isTTY && !process.env.NO_COLOR,
})

/**
 * Run one forwarded engine call and print it. Returns the exit code (also
 * the engine's, when it ran). With `json`, stdout carries exactly one JSON
 * document: the engine's reply, or the API.md §2 error document.
 */
export async function forwardToEngine(
  args: string[],
  json: boolean,
  io: ForwardIO = defaultIO(),
  opts: RunEngineOptions = {},
): Promise<number> {
  try {
    const { data } = await runEngine(args, opts)
    if (json) io.stdout(JSON.stringify(data ?? null, null, 2))
    else io.stdout(renderDocument(data, { color: io.color }))
    return EXIT.ok
  } catch (err) {
    const e =
      err instanceof FactoryEngineError
        ? err
        : new FactoryEngineError({ code: "transport", fact: (err as Error)?.message || String(err) })
    if (json) io.stdout(JSON.stringify(e.toJSON()))
    else io.stderr(renderError(e, { color: io.color }))
    return e.exitCode
  }
}

/** Interactive verbs: the engine owns the terminal until it exits. */
export async function forwardInteractive(args: string[], io: ForwardIO = defaultIO(), opts: RunEngineOptions = {}): Promise<number> {
  try {
    return await passThrough(args, opts)
  } catch (err) {
    const e =
      err instanceof FactoryEngineError
        ? err
        : new FactoryEngineError({ code: "transport", fact: (err as Error)?.message || String(err) })
    io.stderr(renderError(e, { color: io.color }))
    return e.exitCode
  }
}

export interface EngineVerbSpec {
  /** yargs command string, e.g. "send <to> <text..>". */
  command: string
  describe: string
  /** Mutations advertise --dry-run (the engine implements it). */
  mutation?: boolean
  /**
   * "always": the engine draws the screen (wall, attach).
   * "unless-json": a foreground stream (drive, feed) unless --json asks for
   * one document.
   */
  interactive?: "always" | "unless-json"
  options?: Record<string, { type: "string" | "number" | "boolean" | "array"; describe: string }>
}

/** A yargs command that forwards its verb to the engine. */
export function engineVerb(part: FactoryPart, spec: EngineVerbSpec): CommandModule {
  return {
    command: spec.command,
    describe: spec.describe,
    builder: (y: Argv) => {
      let b = y.strict(false).option("json", { type: "boolean", describe: "print the engine's JSON document" })
      if (spec.mutation) b = b.option("dry-run", { type: "boolean", describe: "print exactly what would change" })
      for (const [name, opt] of Object.entries(spec.options ?? {})) b = b.option(name, opt)
      return b
    },
    handler: async () => {
      const { args, json } = forwardedArgs(part)
      const interactive = spec.interactive === "always" || (spec.interactive === "unless-json" && !json)
      process.exitCode = interactive ? await forwardInteractive(args) : await forwardToEngine(args, json)
    },
  }
}

/** A verb group (e.g. `template list|show|check|save`) whose verbs all forward. */
export function engineGroup(
  part: FactoryPart,
  name: string,
  describe: string,
  verbs: EngineVerbSpec[],
  extra: CommandModule[] = [],
): CommandModule {
  return {
    command: name,
    describe,
    builder: (y: Argv) => {
      let b = y
      for (const v of verbs) b = b.command(engineVerb(part, v))
      for (const e of extra) b = b.command(e)
      return b.demandCommand(1, `Specify a ${name} command`)
    },
    handler: () => {},
  }
}

/** Re-home an existing command module under a new verb name. */
export function rehome<T extends CommandModule<any, any>>(mod: T, command: string, describe?: string): CommandModule {
  return { ...(mod as CommandModule), command, aliases: undefined, describe: describe ?? mod.describe } as CommandModule
}

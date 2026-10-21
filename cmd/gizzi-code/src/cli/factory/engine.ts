/**
 * Engine bridge — how Gizzi talks to the Allternit Factory engine.
 *
 * `gizzi agents|orchestration|workflows|workspace …` are thin clients of the
 * engine binary `allternit-factory` (SPEC §1, API.md §1–§2). This module:
 *
 *   - finds the engine (next to the gizzi executable, then
 *     $ALLTERNIT_FACTORY_BIN, then ~/.allternit/bin/allternit-factory, then
 *     PATH) — never falls back to anything else;
 *   - runs `allternit-factory <args> --json`, parses the one JSON document on
 *     stdout and maps the exit-code table to a typed FactoryEngineError;
 *   - hands the terminal to the engine for interactive verbs (the live pane
 *     wall, attach) with stdio inherited, and returns when it exits.
 */
import { spawn, spawnSync } from "node:child_process"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"

export const ENGINE_BIN = process.platform === "win32" ? "allternit-factory.exe" : "allternit-factory"

/** API.md §2 exit-code table. */
export const EXIT = {
  ok: 0,
  refused: 1,
  not_found: 2,
  transport: 3,
  timeout: 4,
  needs_person: 5,
  usage: 64,
} as const

export type EngineErrorCode = Exclude<keyof typeof EXIT, "ok">

const CODE_BY_EXIT: Record<number, EngineErrorCode> = {
  1: "refused",
  2: "not_found",
  3: "transport",
  4: "timeout",
  5: "needs_person",
  64: "usage",
}

export const ENGINE_MISSING_FACT = "The Allternit Factory engine isn't installed"
export const ENGINE_MISSING_ACTION =
  "It comes with Allternit Desktop and with `brew install gizzi-code`. Install one of those, or set ALLTERNIT_FACTORY_BIN to the allternit-factory binary."

export class FactoryEngineError extends Error {
  readonly code: EngineErrorCode
  readonly exitCode: number
  readonly fact: string
  readonly action: string | null
  constructor(input: { code: EngineErrorCode; exitCode?: number; fact: string; action?: string | null }) {
    super(input.fact)
    this.name = "FactoryEngineError"
    this.code = input.code
    this.exitCode = input.exitCode ?? EXIT[input.code]
    this.fact = input.fact
    this.action = input.action ?? null
  }

  /** The API.md §2 error document, exactly as the engine would print it. */
  toJSON() {
    return { error: { code: this.code, fact: this.fact, action: this.action ?? "" } }
  }

  /** True when the engine reported a specified-but-unbuilt verb. */
  get notBuiltYet(): boolean {
    return this.code === "not_found" && /is not built yet/.test(this.fact)
  }
}

export interface LocateDeps {
  execPath?: string
  env?: NodeJS.ProcessEnv
  home?: string
  isExecutable?: (p: string) => boolean
}

function defaultIsExecutable(p: string): boolean {
  try {
    const st = fs.statSync(p)
    if (!st.isFile()) return false
    if (process.platform === "win32") return true
    fs.accessSync(p, fs.constants.X_OK)
    return true
  } catch {
    return false
  }
}

export type EngineLocation = {
  path: string
  source: "sibling" | "env" | "home" | "path"
}

/**
 * Resolve the engine binary in the order API.md §1 fixes. Returns null when
 * none is found; callers turn that into the exit-3 "isn't installed" error.
 * An explicitly set ALLTERNIT_FACTORY_BIN that doesn't point at an
 * executable is skipped like any other miss, but `describeEngineSearch`
 * reports it so doctor can say why.
 */
export function locateEngine(deps: LocateDeps = {}): EngineLocation | null {
  const env = deps.env ?? process.env
  const isExec = deps.isExecutable ?? defaultIsExecutable
  const execPath = deps.execPath ?? process.execPath
  const home = deps.home ?? os.homedir()

  const sibling = path.join(path.dirname(execPath), ENGINE_BIN)
  if (isExec(sibling)) return { path: sibling, source: "sibling" }

  const fromEnv = env.ALLTERNIT_FACTORY_BIN
  if (fromEnv && isExec(fromEnv)) return { path: fromEnv, source: "env" }

  const fromHome = path.join(home, ".allternit", "bin", ENGINE_BIN)
  if (isExec(fromHome)) return { path: fromHome, source: "home" }

  for (const dir of (env.PATH ?? "").split(path.delimiter)) {
    if (!dir) continue
    const candidate = path.join(dir, ENGINE_BIN)
    if (isExec(candidate)) return { path: candidate, source: "path" }
  }
  return null
}

export function engineMissingError(): FactoryEngineError {
  return new FactoryEngineError({ code: "transport", fact: ENGINE_MISSING_FACT, action: ENGINE_MISSING_ACTION })
}

export function requireEngine(deps?: LocateDeps): EngineLocation {
  const found = locateEngine(deps)
  if (!found) throw engineMissingError()
  return found
}

export interface RunEngineOptions {
  /** Kill the engine and report exit 4 after this many ms. Default: no limit. */
  timeoutMs?: number
  /** Override engine lookup (tests, doctor). */
  locate?: LocateDeps
  /** Extra environment for the child. */
  env?: NodeJS.ProcessEnv
  cwd?: string
}

export interface EngineResult<T = unknown> {
  data: T
  exitCode: 0
}

function parseErrorDoc(stdout: string): { code?: string; fact?: string; action?: string } | null {
  const text = stdout.trim()
  if (!text.startsWith("{")) return null
  try {
    const doc = JSON.parse(text)
    if (doc && typeof doc === "object" && doc.error && typeof doc.error === "object") return doc.error
  } catch {
    // not JSON
  }
  return null
}

function isEngineCode(code: unknown): code is EngineErrorCode {
  return typeof code === "string" && code in EXIT && code !== "ok"
}

/**
 * Run `allternit-factory <args> --json` and return the parsed document.
 * Any non-zero exit becomes a FactoryEngineError carrying the engine's own
 * code/fact/action; a zero exit with unparseable stdout is a transport
 * error (the contract says stdout is one JSON document and nothing else).
 */
export async function runEngine<T = unknown>(args: string[], opts: RunEngineOptions = {}): Promise<EngineResult<T>> {
  const engine = requireEngine(opts.locate)
  const argv = args.includes("--json") ? args : [...args, "--json"]

  return new Promise((resolve, reject) => {
    let child: ReturnType<typeof spawn>
    try {
      child = spawn(engine.path, argv, {
        stdio: ["ignore", "pipe", "pipe"],
        env: { ...process.env, ...opts.env },
        cwd: opts.cwd,
      })
    } catch (err) {
      reject(
        new FactoryEngineError({
          code: "transport",
          fact: `Couldn't start the Allternit Factory engine at ${engine.path}: ${(err as Error).message}`,
          action: "Reinstall Allternit Desktop or gizzi-code, or point ALLTERNIT_FACTORY_BIN at a working engine.",
        }),
      )
      return
    }
    let stdout = ""
    let stderr = ""
    let timedOut = false
    child.stdout!.on("data", (d) => (stdout += d))
    child.stderr!.on("data", (d) => (stderr += d))
    const timer =
      opts.timeoutMs && opts.timeoutMs > 0
        ? setTimeout(() => {
            timedOut = true
            child.kill("SIGTERM")
          }, opts.timeoutMs)
        : null

    child.on("error", (err) => {
      if (timer) clearTimeout(timer)
      reject(
        new FactoryEngineError({
          code: "transport",
          fact: `Couldn't start the Allternit Factory engine at ${engine.path}: ${err.message}`,
          action: "Reinstall Allternit Desktop or gizzi-code, or point ALLTERNIT_FACTORY_BIN at a working engine.",
        }),
      )
    })

    child.on("close", (code, signal) => {
      if (timer) clearTimeout(timer)
      if (timedOut) {
        reject(
          new FactoryEngineError({
            code: "timeout",
            fact: `allternit-factory ${args.join(" ")} didn't answer within ${Math.round(opts.timeoutMs! / 1000)}s`,
            action: "Check the engine with `gizzi doctor`, then try again.",
          }),
        )
        return
      }
      const exit = code ?? (signal ? EXIT.transport : EXIT.transport)
      if (exit === 0) {
        const text = stdout.trim()
        if (!text) {
          resolve({ data: null as T, exitCode: 0 })
          return
        }
        try {
          resolve({ data: JSON.parse(text) as T, exitCode: 0 })
        } catch {
          reject(
            new FactoryEngineError({
              code: "transport",
              fact: `The engine's reply to \`${args.join(" ")}\` wasn't JSON`,
              action: "Update Allternit Desktop or gizzi-code so the engine and Gizzi match.",
            }),
          )
        }
        return
      }
      const doc = parseErrorDoc(stdout)
      const mapped = CODE_BY_EXIT[exit]
      const docCode = isEngineCode(doc?.code) ? doc!.code : undefined
      const errCode: EngineErrorCode = docCode ?? mapped ?? "transport"
      const fact =
        doc?.fact ||
        stderr.trim().split("\n").filter(Boolean).pop() ||
        (signal ? `The engine was stopped by ${signal}` : `The engine exited with code ${exit}`)
      reject(new FactoryEngineError({ code: errCode, exitCode: exit, fact, action: doc?.action || null }))
    })
  })
}

/**
 * Hand the terminal to the engine (live pane wall, attach, foreground
 * drive/feed). stdio is inherited so the engine's Rust pane view owns the
 * screen; Gizzi resumes when it exits. Resolves to the engine's exit code.
 */
export async function passThrough(args: string[], opts: Omit<RunEngineOptions, "timeoutMs"> = {}): Promise<number> {
  const engine = requireEngine(opts.locate)
  return new Promise((resolve, reject) => {
    const child = spawn(engine.path, args, {
      stdio: "inherit",
      env: { ...process.env, ...opts.env },
      cwd: opts.cwd,
    })
    // While the engine owns the terminal, Ctrl-C belongs to it.
    const ignore = () => {}
    process.on("SIGINT", ignore)
    const done = () => process.off("SIGINT", ignore)
    child.on("error", (err) => {
      done()
      reject(
        new FactoryEngineError({
          code: "transport",
          fact: `Couldn't start the Allternit Factory engine at ${engine.path}: ${err.message}`,
          action: "Reinstall Allternit Desktop or gizzi-code, or point ALLTERNIT_FACTORY_BIN at a working engine.",
        }),
      )
    })
    child.on("close", (code, signal) => {
      done()
      resolve(code ?? (signal ? EXIT.transport : 0))
    })
  })
}

/** `allternit-factory --version`, first line, or null when it can't be read. */
export function engineVersion(enginePath: string): string | null {
  try {
    const res = spawnSync(enginePath, ["--version"], { encoding: "utf8", timeout: 3000 })
    if (res.status !== 0) return null
    const line = (res.stdout || "").trim().split("\n")[0]
    return line || null
  } catch {
    return null
  }
}

/**
 * Synchronous pass-through for callers that already own the terminal (the
 * Ink TUI): they release the screen, call this, and redraw afterwards.
 * Throws the exit-3 error when the engine is missing.
 */
export function passThroughSync(args: string[], opts: Omit<RunEngineOptions, "timeoutMs"> = {}): number {
  const engine = requireEngine(opts.locate)
  const res = spawnSync(engine.path, args, {
    stdio: "inherit",
    env: { ...process.env, ...opts.env },
    cwd: opts.cwd,
  })
  if (res.error) {
    throw new FactoryEngineError({
      code: "transport",
      fact: `Couldn't start the Allternit Factory engine at ${engine.path}: ${res.error.message}`,
      action: "Reinstall Allternit Desktop or gizzi-code, or point ALLTERNIT_FACTORY_BIN at a working engine.",
    })
  }
  return res.status ?? EXIT.transport
}

/**
 * Command tools for `gizzi worker --tools-file` (env ALLTERNIT_WORKER_TOOLS).
 * Contract: docs/SELFHOST_WORKER_PROTOCOL.md, "Custom tools".
 *
 *   {"tools": {"<name>": {"command": ["argv", ...], "timeout_ms": 120000}}}
 *
 * The file is read once at start. A bad file (unreadable, not JSON, wrong
 * shape, an empty command, or a name equal to a worker built-in tool) stops
 * the worker with exit 1 before it claims anything.
 *
 * A call runs the argv with no shell expansion of its own: the tool input
 * JSON goes to stdin, stdout is the result, a non-zero exit or a timeout is
 * `is_error: true` with the end of stderr. It runs under the same sandbox
 * policy the worker gives the session's `bash` tool (Sandbox.wrap with the
 * session's SessionSandbox policy, cwd = the workdir, the same child env).
 * The sandbox drivers start commands through `<shell> -c`, so the argv is
 * passed as `exec` with every word single-quoted: nothing in it is expanded.
 */
import fs from "node:fs/promises"
import { spawn, type ChildProcess } from "node:child_process"
import { Sandbox, type SandboxPolicy } from "@/runtime/integrations/shell/sandbox"
import { Shell } from "@/runtime/integrations/shell/shell"
import { ProcessRegistry } from "@/runtime/process-registry"
import { MAX_RESULT_BYTES, truncateResult } from "./worker"

export const DEFAULT_CUSTOM_TOOL_TIMEOUT_MS = 120_000
const STDERR_TAIL_BYTES = 4096

export interface CustomToolSpec {
  command: string[]
  timeout_ms: number
}

export type CustomTools = ReadonlyMap<string, CustomToolSpec>

export class ToolsFileError extends Error {
  constructor(source: string, detail: string) {
    super(`tools file ${source}: ${detail}`)
    this.name = "ToolsFileError"
  }
}

/**
 * Parse and check a tools file. `reserved` are the worker's built-in tool
 * names (compared without case, so `Bash` counts as `bash`).
 */
export function parseToolsFile(text: string, source: string, reserved: Iterable<string>): Map<string, CustomToolSpec> {
  let data: unknown
  try {
    data = JSON.parse(text)
  } catch (error) {
    throw new ToolsFileError(source, `not valid JSON (${(error as Error).message})`)
  }
  const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v)
  if (!isObject(data) || !isObject(data.tools)) {
    throw new ToolsFileError(source, `expected {"tools": {"<name>": {"command": ["argv", ...], "timeout_ms": 120000}}}`)
  }
  const builtins = new Map([...reserved].map((name) => [name.toLowerCase(), name]))
  const out = new Map<string, CustomToolSpec>()
  for (const [name, raw] of Object.entries(data.tools)) {
    if (!name.trim()) throw new ToolsFileError(source, "a tool name is empty")
    const clash = builtins.get(name.toLowerCase())
    if (clash) throw new ToolsFileError(source, `tool "${name}" has the name of the worker's built-in tool "${clash}"; rename it`)
    if (!isObject(raw)) throw new ToolsFileError(source, `tool "${name}" must be an object with a "command" array`)
    const command = raw.command
    if (!Array.isArray(command) || command.length === 0) {
      throw new ToolsFileError(source, `tool "${name}" needs a non-empty "command" array (argv, no shell)`)
    }
    if (!command.every((word) => typeof word === "string") || !(command[0] as string).trim()) {
      throw new ToolsFileError(source, `tool "${name}": every "command" entry must be a string and the first one (the program) not empty`)
    }
    let timeout = DEFAULT_CUSTOM_TOOL_TIMEOUT_MS
    if (raw.timeout_ms !== undefined) {
      if (typeof raw.timeout_ms !== "number" || !Number.isInteger(raw.timeout_ms) || raw.timeout_ms <= 0) {
        throw new ToolsFileError(source, `tool "${name}": "timeout_ms" must be a positive integer (milliseconds)`)
      }
      timeout = raw.timeout_ms
    }
    const extra = Object.keys(raw).filter((k) => k !== "command" && k !== "timeout_ms")
    if (extra.length) {
      throw new ToolsFileError(source, `tool "${name}": unknown field(s) ${extra.join(", ")}; only "command" and "timeout_ms" are supported`)
    }
    out.set(name, { command: command as string[], timeout_ms: timeout })
  }
  return out
}

export async function loadToolsFile(file: string, reserved: Iterable<string>): Promise<Map<string, CustomToolSpec>> {
  let text: string
  try {
    text = await fs.readFile(file, "utf8")
  } catch (error) {
    throw new ToolsFileError(file, `cannot be read (${(error as NodeJS.ErrnoException).code ?? (error as Error).message})`)
  }
  return parseToolsFile(text, file, reserved)
}

/** POSIX single-quote one word. */
function quote(word: string): string {
  return `'${word.replace(/'/g, `'\\''`)}'`
}

export interface RunCustomToolInput {
  name: string
  spec: CustomToolSpec
  input: unknown
  cwd: string
  /** The local shadow session (sandbox proxy is keyed by it). */
  sessionID: string
  /** The session's sandbox policy; undefined = sandbox turned off for this process. */
  policy: SandboxPolicy | undefined
  env: Record<string, string>
  signal: AbortSignal
}

export interface CustomToolResult {
  output: string
  is_error: boolean
}

export async function runCustomTool(opts: RunCustomToolInput): Promise<CustomToolResult> {
  let bin: string
  let args: string[]
  if (opts.policy && process.platform !== "win32") {
    if (Sandbox.detect() === "none") {
      return {
        output:
          `The worker cannot run "${opts.name}": the sandbox is on but no isolation driver is available. ` +
          "Install bubblewrap (apt install bubblewrap), or set GIZZI_SANDBOX_DISABLE=1 to run tools unsandboxed on purpose.",
        is_error: true,
      }
    }
    const wrapped = await Sandbox.wrap({
      command: `exec ${opts.spec.command.map(quote).join(" ")}`,
      shell: "/bin/sh",
      cwd: opts.cwd,
      sessionID: opts.sessionID,
      policy: opts.policy,
    }).catch((error: Error) => error)
    if (wrapped instanceof Error) return { output: `The worker could not sandbox "${opts.name}": ${wrapped.message}`, is_error: true }
    if (!wrapped) return { output: `The worker could not sandbox "${opts.name}": the sandbox driver is unavailable.`, is_error: true }
    bin = wrapped.bin
    args = wrapped.args
  } else {
    bin = opts.spec.command[0]!
    args = opts.spec.command.slice(1)
  }

  return new Promise<CustomToolResult>((resolve) => {
    let child: ChildProcess
    try {
      child = spawn(bin, args, {
        cwd: opts.cwd,
        env: opts.env,
        stdio: ["pipe", "pipe", "pipe"],
        // Own process group, so a timeout kills the whole tree.
        detached: process.platform !== "win32",
      })
    } catch (error) {
      return resolve({ output: `The worker could not start "${opts.name}": ${(error as Error).message}`, is_error: true })
    }
    ProcessRegistry.track(child, { label: `custom-tool:${opts.name}`, group: process.platform !== "win32" })

    const stdout: Buffer[] = []
    let stdoutKept = 0
    let stdoutTotal = 0
    let stderr = Buffer.alloc(0)
    let exited = false
    let timedOut = false
    let aborted = false
    let settled = false

    child.stdout!.on("data", (chunk: Buffer) => {
      stdoutTotal += chunk.length
      if (stdoutKept < MAX_RESULT_BYTES) {
        const piece = chunk.subarray(0, MAX_RESULT_BYTES - stdoutKept)
        stdout.push(piece)
        stdoutKept += piece.length
      }
    })
    child.stderr!.on("data", (chunk: Buffer) => {
      stderr = Buffer.concat([stderr, chunk])
      if (stderr.length > STDERR_TAIL_BYTES * 2) stderr = stderr.subarray(stderr.length - STDERR_TAIL_BYTES)
    })

    const kill = () => void Shell.killTree(child, { exited: () => exited })
    const timer = setTimeout(() => {
      timedOut = true
      kill()
    }, opts.spec.timeout_ms)
    const onAbort = () => {
      aborted = true
      kill()
    }
    if (opts.signal.aborted) onAbort()
    else opts.signal.addEventListener("abort", onAbort, { once: true })

    const finish = (result: CustomToolResult) => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      opts.signal.removeEventListener("abort", onAbort)
      resolve(result)
    }

    // The command may exit without reading stdin; that is not an error.
    child.stdin!.on("error", () => {})
    child.stdin!.end(JSON.stringify(opts.input ?? {}))

    child.once("error", (error) => {
      exited = true
      finish({ output: `The worker could not start "${opts.name}": ${error.message}`, is_error: true })
    })
    child.once("close", (code, signal) => {
      exited = true
      const out = truncateResult(Buffer.concat(stdout).toString("utf8"), MAX_RESULT_BYTES, stdoutTotal)
      const tail = stderr.subarray(Math.max(0, stderr.length - STDERR_TAIL_BYTES)).toString("utf8").trim()
      const withStderr = (head: string) => [head, tail && `stderr:\n${tail}`, out && `stdout:\n${out}`].filter(Boolean).join("\n\n")
      if (aborted) return finish({ output: `"${opts.name}" was stopped.`, is_error: true })
      if (timedOut) return finish({ output: withStderr(`"${opts.name}" timed out after ${opts.spec.timeout_ms} ms.`), is_error: true })
      if (code !== 0) {
        const how = signal ? `was killed by ${signal}` : `exited with code ${code}`
        return finish({ output: withStderr(`"${opts.name}" ${how}.`), is_error: true })
      }
      finish({ output: out, is_error: false })
    })
  })
}

/**
 * Code Mode sandbox (MCP 2027 plan, step 5; decisions 1-3).
 *
 * Runs a model-written script inside QuickJS compiled to wasm. The script is
 * steered by tool output and, in plan chats with no computer, runs on a shared
 * host, so this is a security boundary:
 *
 * - The QuickJS context has only the ECMAScript built-ins. No host objects:
 *   no fetch, timers, process, require, module loader or file system.
 * - The only host functions are `<server>.<tool>(args)` bridges. Each one hands
 *   plain JSON across (args out, result in) and calls the `call` function the
 *   caller supplies, which goes through the same permission and approval path
 *   a model MCP call takes (runtime/session/prompt.ts).
 * - Limits: 50 calls, 60 s run time (time spent waiting on the person's
 *   approval is excluded), 32 MB memory, an interrupt handler, result <= 8 KB.
 *
 * Never replace this with node:vm (not a security boundary).
 */
import { newQuickJSWASMModuleFromVariant, newVariant, RELEASE_SYNC } from "quickjs-emscripten"
import type { QuickJSContext, QuickJSHandle, QuickJSRuntime, QuickJSWASMModule } from "quickjs-emscripten"
import { readFile } from "node:fs/promises"
import { fileURLToPath } from "node:url"

export namespace ScriptSandbox {
  export const LIMITS = {
    calls: 50,
    runMs: 60_000,
    memoryBytes: 32 * 1024 * 1024,
    stackBytes: 512 * 1024,
    resultBytes: 8 * 1024,
  } as const

  export interface Bridge {
    server: string
    tool: string
    /** Runs the MCP call through the normal permission + call path. Throws to reject inside the script. */
    call(args: Record<string, unknown>): Promise<unknown>
  }

  export interface CallRecord {
    server: string
    tool: string
    ms: number
    ok: boolean
    error?: string
  }

  export interface RunInput {
    /** Plain JavaScript (types already stripped): the body of an async function. */
    js: string
    bridges: Bridge[]
    abort?: AbortSignal
    /** Total ms spent waiting on the person's approval so far (excluded from the run budget). */
    approvalWaitMs?: () => number
    limits?: Partial<typeof LIMITS>
  }

  export interface RunResult {
    ok: boolean
    /** JSON text of the script's return value, capped at `resultBytes`. */
    result: string
    error?: string
    calls: CallRecord[]
    durationMs: number
  }

  export class LimitError extends Error {}

  // ── wasm module ────────────────────────────────────────────────────────────
  // The wasm bytes are imported, not located on disk: the production build's
  // wasm-embed plugin (script/build-production.js) inlines `*.wasm` imports as a
  // Uint8Array, because /$bunfs/ paths are not readable in the compiled binary.
  // Under `bun run` / `bun test` the same import gives a file path instead.
  let modulePromise: Promise<QuickJSWASMModule> | undefined

  async function wasmBinary(): Promise<ArrayBuffer> {
    const mod = (await import("@jitl/quickjs-wasmfile-release-sync/wasm" as string)) as { default: unknown }
    const value = mod.default
    let bytes: Uint8Array
    if (value instanceof Uint8Array) bytes = value
    else if (value instanceof ArrayBuffer) bytes = new Uint8Array(value)
    else if (typeof value === "string") {
      const file = value.startsWith("file:") ? fileURLToPath(value) : value
      bytes = new Uint8Array(await readFile(file))
    } else throw new Error("QuickJS wasm asset could not be loaded")
    return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer
  }

  function quickjs(): Promise<QuickJSWASMModule> {
    if (!modulePromise) {
      modulePromise = (async () => {
        const variant = newVariant(RELEASE_SYNC, { wasmBinary: await wasmBinary() })
        return newQuickJSWASMModuleFromVariant(variant)
      })()
      modulePromise.catch(() => {
        modulePromise = undefined
      })
    }
    return modulePromise
  }

  // ── names ─────────────────────────────────────────────────────────────────
  const RESERVED = new Set(
    "break case catch class const continue debugger default delete do else enum export extends false finally for function if import in instanceof new null return super switch this throw true try typeof var void while with yield let static implements interface package private protected public await async arguments eval undefined NaN Infinity globalThis".split(
      " ",
    ),
  )

  /** A server or tool name as a JS identifier (the same mapping the prompt declarations use). */
  export function identifier(name: string): string {
    let id = name.replace(/[^A-Za-z0-9_$]/g, "_")
    if (!id || /^[0-9]/.test(id)) id = `_${id}`
    if (RESERVED.has(id)) id = `${id}_`
    return id
  }

  // ── run ───────────────────────────────────────────────────────────────────
  export async function run(input: RunInput): Promise<RunResult> {
    const limits = { ...LIMITS, ...input.limits }
    const started = Date.now()
    const calls: CallRecord[] = []
    const activeMs = () => Date.now() - started - (input.approvalWaitMs?.() ?? 0)
    let stopReason: string | undefined
    const stop = (reason: string) => {
      stopReason ??= reason
    }
    const finish = (ok: boolean, result: string, error?: string): RunResult => ({
      ok,
      result,
      ...(error !== undefined && { error }),
      calls,
      durationMs: Date.now() - started,
    })

    const QuickJS = await quickjs()
    const runtime: QuickJSRuntime = QuickJS.newRuntime()
    runtime.setMemoryLimit(limits.memoryBytes)
    runtime.setMaxStackSize(limits.stackBytes)
    runtime.setInterruptHandler(() => {
      if (input.abort?.aborted) stop("The script was stopped.")
      if (activeMs() > limits.runMs) stop(`The script ran past the ${Math.round(limits.runMs / 1000)} s limit.`)
      return stopReason !== undefined
    })
    const vm: QuickJSContext = runtime.newContext()
    const owned: QuickJSHandle[] = []
    const deferreds: ReturnType<QuickJSContext["newPromise"]>[] = []
    let poisoned = false

    try {
      // Captured before the script runs, so a script that replaces JSON.parse
      // cannot change how results come back in.
      const jsonObj = vm.getProp(vm.global, "JSON")
      owned.push(jsonObj)
      const jsonParse = vm.getProp(jsonObj, "parse")
      owned.push(jsonParse)

      const toVm = (value: unknown): QuickJSHandle => {
        if (value === undefined) return vm.undefined
        const text = vm.newString(JSON.stringify(value) ?? "null")
        try {
          return vm.unwrapResult(vm.callFunction(jsonParse, vm.undefined, text))
        } finally {
          text.dispose()
        }
      }

      // Serial queue: bridged calls run one at a time so each approval prompt
      // stays tied to this script's tool part.
      let queue: Promise<unknown> = Promise.resolve()

      const servers = new Map<string, QuickJSHandle>()
      for (const bridge of input.bridges) {
        const serverId = identifier(bridge.server)
        let serverObj = servers.get(serverId)
        if (!serverObj) {
          serverObj = vm.newObject()
          owned.push(serverObj)
          servers.set(serverId, serverObj)
          vm.setProp(vm.global, serverId, serverObj)
        }
        const fn = vm.newFunction(identifier(bridge.tool), (argHandle) => {
          const deferred = vm.newPromise()
          deferreds.push(deferred)
          let args: Record<string, unknown> = {}
          try {
            const dumped = argHandle ? vm.dump(argHandle) : undefined
            if (dumped && typeof dumped === "object" && !Array.isArray(dumped)) args = dumped as Record<string, unknown>
            // Round-trip so only plain JSON leaves the sandbox.
            args = JSON.parse(JSON.stringify(args)) as Record<string, unknown>
          } catch {
            args = {}
          }
          const settle = (ok: boolean, value: unknown) => {
            if (stopReason !== undefined || !vm.alive || !deferred.alive) return
            if (ok) {
              const handle = toVm(value)
              deferred.resolve(handle)
              if (handle !== vm.undefined) handle.dispose()
            } else {
              const err = vm.newError(String(value))
              deferred.reject(err)
              err.dispose()
            }
            runtime.executePendingJobs()
          }
          if (calls.length >= limits.calls) {
            queueMicrotask(() => settle(false, `Call limit reached (${limits.calls} calls per script).`))
            return deferred.handle
          }
          const record: CallRecord = { server: bridge.server, tool: bridge.tool, ms: 0, ok: false }
          calls.push(record)
          queue = queue.then(async () => {
            if (stopReason !== undefined) return
            const t0 = Date.now()
            try {
              const value = await bridge.call(args)
              record.ms = Date.now() - t0
              record.ok = true
              settle(true, value)
            } catch (error) {
              record.ms = Date.now() - t0
              record.error = (error instanceof Error ? error.message : String(error)).slice(0, 500)
              settle(false, record.error)
            }
          })
          return deferred.handle
        })
        vm.setProp(serverObj, identifier(bridge.tool), fn)
        fn.dispose()
      }

      const code = `globalThis.__mcpScriptMain = async function () {\n${input.js}\n};\nglobalThis.__mcpScriptMain();`
      const evaluated = vm.evalCode(code, "mcp-script.js", { type: "global" })
      if (evaluated.error) {
        const message = errorText(vm, evaluated.error)
        evaluated.error.dispose()
        return finish(false, "", stopReason ?? message)
      }
      const promiseHandle = evaluated.value
      owned.push(promiseHandle)
      const settled = vm.resolvePromise(promiseHandle)
      runtime.executePendingJobs()

      // Host-side watchdog: the interrupt handler only runs while QuickJS runs,
      // so a slow host call is timed here.
      let watchdog: ReturnType<typeof setInterval> | undefined
      const limited = new Promise<never>((_, reject) => {
        watchdog = setInterval(() => {
          if (input.abort?.aborted) stop("The script was stopped.")
          if (activeMs() > limits.runMs) stop(`The script ran past the ${Math.round(limits.runMs / 1000)} s limit.`)
          if (stopReason !== undefined) reject(new LimitError(stopReason))
        }, 100)
      })
      let outcome: Awaited<typeof settled>
      try {
        outcome = await Promise.race([settled, limited])
      } catch (error) {
        poisoned = true
        return finish(false, "", error instanceof Error ? error.message : String(error))
      } finally {
        clearInterval(watchdog)
      }
      if (outcome.error) {
        const message = errorText(vm, outcome.error)
        outcome.error.dispose()
        return finish(false, "", stopReason ?? message)
      }
      if (stopReason !== undefined) {
        outcome.value.dispose()
        return finish(false, "", stopReason)
      }
      let value: unknown
      try {
        value = vm.dump(outcome.value)
      } finally {
        outcome.value.dispose()
      }
      return finish(true, capResult(value, limits.resultBytes))
    } catch (error) {
      poisoned = true
      return finish(false, "", stopReason ?? (error instanceof Error ? error.message : String(error)))
    } finally {
      stop("The script finished.")
      for (const deferred of deferreds) {
        try {
          deferred.dispose()
        } catch {}
      }
      for (const handle of owned) {
        try {
          if (handle.alive) handle.dispose()
        } catch {}
      }
      try {
        vm.dispose()
        runtime.dispose()
      } catch {
        poisoned = true
      }
      // A run that ended mid-flight may leave wasm state behind; start the
      // next run on a fresh module.
      if (poisoned) modulePromise = undefined
    }
  }

  function errorText(vm: QuickJSContext, handle: QuickJSHandle): string {
    try {
      const dumped = vm.dump(handle) as unknown
      if (dumped && typeof dumped === "object" && "message" in dumped) {
        const d = dumped as { name?: string; message?: string }
        return `${d.name ?? "Error"}: ${d.message ?? ""}`
      }
      return String(dumped)
    } catch {
      return "Error"
    }
  }

  /** JSON text of the result, cut to `max` bytes. */
  export function capResult(value: unknown, max: number = LIMITS.resultBytes): string {
    let text: string
    try {
      text = value === undefined ? "" : typeof value === "string" ? value : (JSON.stringify(value) ?? String(value))
    } catch {
      text = String(value)
    }
    const bytes = new TextEncoder().encode(text)
    if (bytes.byteLength <= max) return text
    const marker = "\n[result cut at 8 KB]"
    const cut = new TextDecoder().decode(bytes.slice(0, max - marker.length)).replace(/�$/, "")
    return cut + marker
  }
}

/**
 * LocalCliDriver — executes agent tasks against a CLI installed on the same host.
 *
 * This driver aligns with the production Multica Go implementation: every adapter
 * declares its binary/argv specifics and delegates to a shared protocol runner.
 * All agent CLIs are spawned per-task; there is no warm-process pool.
 *
 * Supported protocols:
 *   • stream-json   — Claude dialect NDJSON over stdio.
 *   • acp           — Agent Client Protocol JSON-RPC over stdio.
 *   • codex-app-server — OpenAI Codex JSON-RPC app-server over stdio.
 *   • one-shot-json — single-shot CLI whose stdout is parsed as JSON.
 *   • one-shot-text — single-shot CLI whose stdout is treated as plain text.
 *
 * It is the single place where a CLI is actually spawned; the rest of the
 * codebase (LanguageModelV2 wrapper, remote runtime proxy, etc.) delegates here.
 */

import type {
  AgentEvent,
  AgentTask,
  ExecutionLog,
  RuntimeDriver,
  TaskHandle,
} from "@/runtime/runtime-driver"
import { acpCanLoadSession, acpMcpServers, claudeSessionFlags, codexThreadRequest, opencodeResumeFlags, qwenResumeFlags, vendorSessionIdFromEvent, codexMcpConfig, withInstructions } from "@/runtime/drivers/cli-session-flags"
import { attachmentsToAcpContent } from "./attachments"
import { RuntimeService, RuntimeNotFoundError, type RegisteredRuntime } from "@/runtime/runtime-service"
import { ExecutionLogService } from "@/runtime/execution-log"
import { Log } from "@/shared/util/log"
import { PROVIDER_ENV_KEYS } from "@/runtime/runtime-discovery"
import { currentCliPath } from "@/runtime/providers/discovery/subprocess"
import {
  ClientSideConnection,
  ndJsonStream,
  PROTOCOL_VERSION,
} from "@agentclientprotocol/sdk"
import { spawn as nodeSpawn } from "node:child_process"
import { Readable, Writable } from "node:stream"
import { ProcessRegistry } from "@/runtime/process-registry"
import { acpGateDecision } from "@/runtime/drivers/acp-gate"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"

const log = Log.create({ service: "local-cli-driver" })

/**
 * Coarse permission mapping for CLI-internal (ACP) tool calls. The CLI
 * executes the tool itself; gizzi's gate decides from the tool's kind/title
 * using the same permission vocabulary as gizzi's own tools, so session
 * modes behave identically for every provider:
 *   plan      → deny anything not read-only
 *   acceptEdits → allow read + edit classes
 *   default   → permission.asked, block until the user replies
 *
 * Exported for unit tests.
 */
/** ACP agents (kimi-cli) often return a successful empty prompt on quota/auth
 *  403s instead of throwing. Treat those stderr signatures as fatal so the
 *  cowork SSE cannot report status:complete with 0 tokens. */
export function acpStderrLooksFatal(tail: string): boolean {
  if (!tail) return false
  return /provider\.auth_error|You've reached your (?:monthly|5-hour) usage limit|401 Unauthorized|403 .*(?:quota|usage limit|auth)/i.test(
    tail,
  )
}

export function acpPermissionFor(tool: { kind?: unknown; title?: unknown }): {
  permission: string
  pattern: string
} {
  const kind = String(tool.kind ?? "")
  const title = String(tool.title ?? "")
  const s = `${kind} ${title}`.toLowerCase()
  if (/\b(read|view|grep|glob|list|cat|find|search|ls)\b/.test(s) && !/write|edit|create|delete/.test(s)) {
    return { permission: "read", pattern: title || kind || "*" }
  }
  if (/(edit|write|patch|create|delete|update|mkdir|rename|move)/.test(s)) {
    return { permission: "edit", pattern: title || kind || "*" }
  }
  if (/(bash|shell|command|exec|terminal|npm|pip|curl|git)/.test(s)) {
    return { permission: "bash", pattern: title || kind || "*" }
  }
  return { permission: "bash", pattern: title || kind || "*" }
}

export class LocalCliDriver implements RuntimeDriver {
  private readonly runtimeId: string
  private readonly cliName: string
  private readonly abortedTasks = new Set<string>()
  private readonly tasks = new Map<string, AgentTask>()
  private runtimeCache?: RegisteredRuntime
  private currentTaskId?: string
  private currentProc?: ReturnType<typeof Bun.spawn> | ReturnType<typeof nodeSpawn>

  constructor(runtimeId: string, cliName: string) {
    this.runtimeId = runtimeId
    this.cliName = cliName
  }

  // ---------------------------------------------------------------------------
  // RuntimeDriver implementation
  // ---------------------------------------------------------------------------

  async assign(task: AgentTask): Promise<TaskHandle> {
    const handle: TaskHandle = {
      taskId: task.taskId || generateTaskId(),
      runtimeId: this.runtimeId,
      cliName: this.cliName,
    }

    this.tasks.set(handle.taskId, task)

    await ExecutionLogService.create(handle)
    await ExecutionLogService.appendEvent(handle.taskId, {
      type: "status",
      status: "queued",
    })

    log.info("task assigned", { taskId: handle.taskId, cli: this.cliName })
    return handle
  }

  async *stream(handle: TaskHandle): AsyncIterable<AgentEvent> {
    if (this.abortedTasks.has(handle.taskId)) {
      this.abortedTasks.delete(handle.taskId)
      const ev = { type: "status", status: "cancelled" } as AgentEvent
      yield ev
      await this.logEvent(handle.taskId, ev)
      return
    }

    const runtime = await this.getRuntime()
    const cli = runtime.agentClis.find((c) => c.name === this.cliName)
    if (!cli) {
      throw new Error(`CLI ${this.cliName} not found on runtime ${this.runtimeId}`)
    }

    const task = this.tasks.get(handle.taskId)
    const message = task?.prompt ?? ""

    yield { type: "status", status: "running" }
    await this.logEvent(handle.taskId, { type: "status", status: "running" })

    // The launcher can disappear mid-session (a CLI auto-update); find it again.
    const cliPath = await currentCliPath(this.cliName, cli.path)
    if (cliPath !== cli.path) {
      log.info("cli path moved", { cli: this.cliName, from: cli.path, to: cliPath })
      cli.path = cliPath
    }
    const baseCmd = parseCmd(`${cliPath} ${this.specArgs()}`)
    const adapter = resolveAdapter(this.cliName)

    if (task?.attachments && task.attachments.length > 0 && !adapter.supportsAttachments) {
      throw new Error(
        `CLI "${this.cliName}" does not support task attachments in its current adapter mode (${adapter.mode}).`,
      )
    }

    const argv = adapter.buildArgv(baseCmd, message, {
      cwd: task?.cwd,
      taskId: handle.taskId,
      systemPrompt: task?.systemPrompt,
      mcp: task?.mcp,
      vendorSessionId: task?.vendorSessionId,
    })

    const env = adapter.env ? { ...adapter.env, ...task?.env } : task?.env

    let failed = false
    try {
      if (adapter.mode === "stream-json") {
        for await (const event of this.runStreamJson(handle, argv, {
          prompt: message,
          promptOnStdin: adapter.promptOnStdin ?? false,
          endsWithResult: adapter.endsWithResult ?? false,
          cwd: task?.cwd,
          env: env,
        })) {
          if (event.type === "error" || (event.type === "finish" && event.finishReason === "error")) failed = true
          yield event
        }
        if (failed) {
          const failedEv = { type: "status", status: "failed" } as AgentEvent
          yield failedEv
          await this.logEvent(handle.taskId, failedEv)
        }
      } else if (adapter.mode === "openclaw-json") {
        yield* this.runOpenclawJson(handle, argv, message, task?.cwd, env)
      } else if (adapter.mode === "acp") {
        yield* this.runAcp(handle, argv, task?.cwd, env)
      } else if (adapter.mode === "codex-app-server") {
        for await (const event of this.runCodexAppServer(handle, argv, message, task?.cwd, task?.systemPrompt, env, task?.model)) {
          if (event.type === "error" || (event.type === "finish" && event.finishReason === "error")) failed = true
          yield event
        }
        if (failed) {
          const failedEv = { type: "status", status: "failed" } as AgentEvent
          yield failedEv
          await this.logEvent(handle.taskId, failedEv)
        }
      } else if (adapter.mode === "one-shot-json") {
        yield* this.runOneShotJson(handle, argv, { cwd: task?.cwd, env: env })
      } else if (adapter.mode === "one-shot-text") {
        yield* this.runOneShotText(handle, argv, { cwd: task?.cwd, env: env })
      } else {
        throw new Error(`CLI ${this.cliName} has an unsupported local-driver mode`)
      }
    } catch (err) {
      failed = true
      const errorEv = { type: "error", error: err } as AgentEvent
      yield errorEv
      await this.logEvent(handle.taskId, errorEv)
      const failedEv = { type: "status", status: "failed" } as AgentEvent
      yield failedEv
      await this.logEvent(handle.taskId, failedEv)
    }

    if (!failed) {
      const completedEv = { type: "status", status: "completed" } as AgentEvent
      yield completedEv
      await this.logEvent(handle.taskId, completedEv)
    }
  }

  async abort(handle: TaskHandle): Promise<void> {
    if (this.isCurrentTask(handle.taskId) && this.currentProc) {
      try {
        this.currentProc.kill()
      } catch (err) {
        log.warn("failed to kill current subprocess", { error: err, taskId: handle.taskId })
      }
    }
    this.abortedTasks.add(handle.taskId)
    await ExecutionLogService.appendEvent(handle.taskId, {
      type: "status",
      status: "cancelled",
    })
  }

  async inspect(handle: TaskHandle): Promise<ExecutionLog> {
    const logEntry = await ExecutionLogService.get(handle.taskId)
    if (!logEntry) {
      throw new Error(`No execution log found for task ${handle.taskId}`)
    }
    return logEntry
  }

  // ---------------------------------------------------------------------------
  // Internal helpers
  // ---------------------------------------------------------------------------

  private async logEvent(taskId: string, event: AgentEvent): Promise<void> {
    try {
      await ExecutionLogService.appendEvent(taskId, event)
    } catch (err) {
      log.warn("failed to append execution log event", { error: err, taskId, eventType: event.type })
    }
  }

  private isCurrentTask(taskId: string): boolean {
    return this.currentTaskId === taskId
  }

  private async getRuntime(): Promise<RegisteredRuntime> {
    if (this.runtimeCache) return this.runtimeCache
    const runtime = await RuntimeService.get(this.runtimeId)
    if (!runtime) throw new RuntimeNotFoundError({ runtimeId: this.runtimeId })
    this.runtimeCache = runtime
    return runtime
  }

  /**
   * Legacy spec template tail. Adapters now fully own argv construction, so this
   * returns an empty string for all migrated providers. It is kept only so the
   * driver can reconstruct a `[path, ...tail]` base command for adapters that
   * still want to inspect the resolved CLI path.
   */
  private specArgs(): string {
    return ""
  }

  private resetCurrentTask(): void {
    this.currentTaskId = undefined
    this.currentProc = undefined
  }

  private async *runOneShotJson(
    handle: TaskHandle,
    argv: string[],
    options: { cwd?: string; env?: Record<string, string> },
  ): AsyncIterable<AgentEvent> {
    log.info("spawning one-shot json subprocess", { taskId: handle.taskId, argv: argv.join(" ") })

    const stderrTail = new StderrTail()
    const proc = Bun.spawn(argv, {
      stdin: "ignore",
      stdout: "pipe",
      stderr: "pipe",
      cwd: options.cwd,
      env: mergeEnv(process.env, options.env),
    })
    this.currentProc = proc
    this.currentTaskId = handle.taskId
    ProcessRegistry.track(proc, { label: `cli:${this.cliName}:json` })

    // Capture a bounded stderr tail for diagnostics without blocking exit handling.
    ;(async () => {
      try {
        for await (const chunk of readStreamChunks(proc.stderr)) {
          stderrTail.append(Buffer.from(chunk))
        }
      } catch {
        // Stderr stream closed early; the tail is best-effort diagnostics.
      }
    })()

    try {
      const [stdout, exitCode] = await Promise.all([readText(proc.stdout), proc.exited])

      if (this.abortedTasks.has(handle.taskId)) {
        this.abortedTasks.delete(handle.taskId)
        const ev = { type: "status", status: "cancelled" } as AgentEvent
        yield ev
        await this.logEvent(handle.taskId, ev)
        return
      }

      if (exitCode !== 0) {
        const detail =
          stderrTail.tail() || stdout.trim() || `subprocess exited with code ${exitCode}`
        const errorEv = { type: "error", error: new Error(detail) } as AgentEvent
        yield errorEv
        await this.logEvent(handle.taskId, errorEv)
        const finishEv = { type: "finish", finishReason: "error", usage: zeroUsage() } as AgentEvent
        yield finishEv
        await this.logEvent(handle.taskId, finishEv)
        return
      }

      const text = extractOneShotOutput(stdout.trim())
      if (text) {
        const deltaEv = { type: "text_delta", delta: text } as AgentEvent
        yield deltaEv
        await this.logEvent(handle.taskId, deltaEv)
      }

      const finishEv = { type: "finish", finishReason: "stop", usage: zeroUsage() } as AgentEvent
      yield finishEv
      await this.logEvent(handle.taskId, finishEv)
    } catch (err) {
      log.error("one-shot json request failed", { error: err, taskId: handle.taskId })
      const errorEv = { type: "error", error: err } as AgentEvent
      yield errorEv
      await this.logEvent(handle.taskId, errorEv)
      const finishEv = { type: "finish", finishReason: "error", usage: zeroUsage() } as AgentEvent
      yield finishEv
      await this.logEvent(handle.taskId, finishEv)
    } finally {
      terminateProcessTree(proc)
      this.resetCurrentTask()
    }
  }

  private async *runOneShotText(
    handle: TaskHandle,
    argv: string[],
    options: { cwd?: string; env?: Record<string, string> },
  ): AsyncIterable<AgentEvent> {
    log.info("spawning one-shot text subprocess", { taskId: handle.taskId, argv: argv.join(" ") })

    const stderrTail = new StderrTail()
    const proc = Bun.spawn(argv, {
      stdin: "ignore",
      stdout: "pipe",
      stderr: "pipe",
      cwd: options.cwd,
      env: mergeEnv(process.env, options.env),
    })
    this.currentProc = proc
    this.currentTaskId = handle.taskId
    ProcessRegistry.track(proc, { label: `cli:${this.cliName}:text` })

    ;(async () => {
      try {
        for await (const chunk of readStreamChunks(proc.stderr)) {
          stderrTail.append(Buffer.from(chunk))
        }
      } catch {
        // Stderr stream closed early; the tail is best-effort diagnostics.
      }
    })()

    try {
      const [stdout, exitCode] = await Promise.all([readText(proc.stdout), proc.exited])

      if (this.abortedTasks.has(handle.taskId)) {
        this.abortedTasks.delete(handle.taskId)
        const ev = { type: "status", status: "cancelled" } as AgentEvent
        yield ev
        await this.logEvent(handle.taskId, ev)
        return
      }

      if (exitCode !== 0) {
        const detail =
          stderrTail.tail() || stdout.trim() || `subprocess exited with code ${exitCode}`
        const errorEv = { type: "error", error: new Error(detail) } as AgentEvent
        yield errorEv
        await this.logEvent(handle.taskId, errorEv)
        const finishEv = { type: "finish", finishReason: "error", usage: zeroUsage() } as AgentEvent
        yield finishEv
        await this.logEvent(handle.taskId, finishEv)
        return
      }

      const text = stdout.trim()
      if (text) {
        const deltaEv = { type: "text_delta", delta: text } as AgentEvent
        yield deltaEv
        await this.logEvent(handle.taskId, deltaEv)
      }

      const finishEv = { type: "finish", finishReason: "stop", usage: zeroUsage() } as AgentEvent
      yield finishEv
      await this.logEvent(handle.taskId, finishEv)
    } catch (err) {
      log.error("one-shot text request failed", { error: err, taskId: handle.taskId })
      const errorEv = { type: "error", error: err } as AgentEvent
      yield errorEv
      await this.logEvent(handle.taskId, errorEv)
      const finishEv = { type: "finish", finishReason: "error", usage: zeroUsage() } as AgentEvent
      yield finishEv
      await this.logEvent(handle.taskId, finishEv)
    } finally {
      terminateProcessTree(proc)
      this.resetCurrentTask()
    }
  }

  // ---------------------------------------------------------------------------
  // Stream-json execution (Claude dialect)
  // ---------------------------------------------------------------------------

  private async *runStreamJson(
    handle: TaskHandle,
    argv: string[],
    options: {
      prompt: string
      promptOnStdin: boolean
      /** The CLI always closes a turn with a `result` event (Claude dialect). */
      endsWithResult: boolean
      cwd?: string
      env?: Record<string, string>
    },
  ): AsyncIterable<AgentEvent> {
    let lastVendorId: string | undefined
    log.info("spawning stream-json subprocess", {
      taskId: handle.taskId,
      argv: argv.join(" "),
      cwd: options.cwd,
    })

    const stderrTail = new StderrTail()
    const proc = Bun.spawn(argv, {
      stdin: "pipe",
      stdout: "pipe",
      stderr: "pipe",
      cwd: options.cwd,
      env: mergeEnv(process.env, options.env),
    })
    this.currentProc = proc
    this.currentTaskId = handle.taskId
    ProcessRegistry.track(proc, { label: `cli:${this.cliName}:stream-json` })

    // Capture a bounded stderr tail for diagnostics.
    ;(async () => {
      try {
        for await (const chunk of readStreamChunks(proc.stderr)) {
          stderrTail.append(Buffer.from(chunk))
        }
      } catch {
        // Stderr stream closed early; the tail is best-effort diagnostics.
      }
    })()

    const stdin = proc.stdin
    try {
      if (options.promptOnStdin) {
        await stdin.write(options.prompt + "\n")
        await stdin.end()
      } else {
        // Claude/CodeBuddy expect the prompt as NDJSON and may send
        // control_request frames mid-run; keep stdin open for responses.
        await stdin.write(claudeStreamJsonInput(options.prompt))
      }
    } catch {
      // stdin may already be closed by the agent.
    }

    let finished = false
    const blocks = new StreamJsonBlocks()

    try {
      for await (const line of readLines(proc.stdout)) {
        if (this.abortedTasks.has(handle.taskId)) {
          this.abortedTasks.delete(handle.taskId)
          const ev = { type: "status", status: "cancelled" } as AgentEvent
          yield ev
          await this.logEvent(handle.taskId, ev)
          return
        }

        if (!line.trim()) continue

        try {
          const evt = JSON.parse(line) as StreamJsonEvent

          const vendorId = vendorSessionIdFromEvent(this.cliName, evt)
          if (vendorId && vendorId !== lastVendorId) {
            lastVendorId = vendorId
            const vEv = { type: "vendor_session", id: vendorId } as AgentEvent
            yield vEv
            await this.logEvent(handle.taskId, vEv)
          }

          if (evt.type === "system") {
            if (evt.status) {
              const statusEv = { type: "status", status: "running" } as AgentEvent
              yield statusEv
              await this.logEvent(handle.taskId, statusEv)
            }
            continue
          }

          // --include-partial-messages: text and thinking as they're written.
          if (evt.type === "stream_event") {
            const live = blocks.streamEvent(evt.event)
            if (live) {
              const liveEv = { type: live.kind === "text" ? "text_delta" : "reasoning_delta", delta: live.delta } as AgentEvent
              yield liveEv
              await this.logEvent(handle.taskId, liveEv)
            }
            continue
          }

          if (evt.type === "assistant" && Array.isArray(evt.message?.content)) {
            const unsent = blocks.assistantParts(evt.message)
            for (const [idx, part] of evt.message.content.entries()) {
              if (!part || typeof part !== "object") continue
              if (part.type === "text" && typeof part.text === "string") {
                const delta = unsent[idx]
                if (delta) {
                  const deltaEv = { type: "text_delta", delta } as AgentEvent
                  yield deltaEv
                  await this.logEvent(handle.taskId, deltaEv)
                }
              } else if (part.type === "thinking" && typeof part.thinking === "string") {
                const delta = unsent[idx]
                if (delta) {
                  const reasoningEv = { type: "reasoning_delta", delta } as AgentEvent
                  yield reasoningEv
                  await this.logEvent(handle.taskId, reasoningEv)
                }
              } else if (part.type === "tool_use") {
                const toolCallEv = {
                  type: "tool_call",
                  id: String(part.id ?? `${handle.taskId}-tool-${idx}`),
                  name: String(part.name ?? "tool"),
                  arguments: part.input ?? {},
                } as AgentEvent
                yield toolCallEv
                await this.logEvent(handle.taskId, toolCallEv)
              }
            }
            continue
          }

          const userContent = streamJsonUserContent(evt)
          if (userContent) {
            for (const [idx, part] of userContent.entries()) {
              if (!part || typeof part !== "object") continue
              if (part.type === "tool_result") {
                const toolResultEv = {
                  type: "tool_result",
                  id: String(part.tool_use_id ?? `${handle.taskId}-tool-${idx}`),
                  content: extractTextContent(part.content),
                  isError: Boolean(part.is_error),
                } as AgentEvent
                yield toolResultEv
                await this.logEvent(handle.taskId, toolResultEv)
              }
            }
            continue
          }

          if (evt.type === "control_request") {
            const requestId = String(evt.request_id ?? "")
            const input = evt.request?.input as Record<string, unknown> | undefined
            if (requestId) {
              writeControlResponse(stdin, requestId, input)
            }
            continue
          }

          if (evt.type === "result") {
            finished = true
            const usage = evt.usage ?? {}
            const finishEv = {
              type: "finish",
              finishReason: evt.is_error ? "error" : "stop",
              usage: {
                inputTokens: usage.input_tokens ?? 0,
                outputTokens: usage.output_tokens ?? 0,
                totalTokens: (usage.input_tokens ?? 0) + (usage.output_tokens ?? 0),
              },
            } as AgentEvent
            yield finishEv
            await this.logEvent(handle.taskId, finishEv)
            break
          }
        } catch {
          // malformed JSON — skip
        }
      }

      if (!finished) {
        // stdout closed without the turn's `result`: the CLI died or was
        // stopped mid-turn (seen live: the Desktop app quitting killed two
        // Claude turns mid-tool-call, and the half-finished turns — tool
        // calls, no answer — were reported as complete replies).
        const exitCode = await Promise.race([
          proc.exited.then(() => proc.exitCode),
          new Promise<null>((resolve) => setTimeout(() => resolve(null), 3000)),
        ])
        const cutShort = streamJsonCutShort({
          cliName: this.cliName,
          endsWithResult: options.endsWithResult,
          exitCode,
          signalCode: proc.signalCode ?? null,
          stderr: stderrTail.tail(),
        })
        if (cutShort) {
          log.warn("stream-json turn ended without a result", { taskId: handle.taskId, exitCode, signal: proc.signalCode })
          const errorEv = { type: "error", error: new Error(cutShort) } as AgentEvent
          yield errorEv
          await this.logEvent(handle.taskId, errorEv)
        }
        const finishEv = { type: "finish", finishReason: cutShort ? "error" : "stop", usage: zeroUsage() } as AgentEvent
        yield finishEv
        await this.logEvent(handle.taskId, finishEv)
      }
    } catch (err) {
      log.error("stream-json request failed", { error: err, taskId: handle.taskId })
      const detail = stderrTail.tail() || (err instanceof Error ? err.message : String(err))
      const errorEv = { type: "error", error: new Error(detail) } as AgentEvent
      yield errorEv
      await this.logEvent(handle.taskId, errorEv)
      const finishEv = { type: "finish", finishReason: "error", usage: zeroUsage() } as AgentEvent
      yield finishEv
      await this.logEvent(handle.taskId, finishEv)
    } finally {
      try {
        await stdin.end()
      } catch {
        // stdin may already be closed when the process died.
      }
      terminateProcessTree(proc)
      this.resetCurrentTask()
    }
  }

  // ---------------------------------------------------------------------------
  // OpenClaw simplified NDJSON / final-blob runner
  // ---------------------------------------------------------------------------

  private async *runOpenclawJson(
    handle: TaskHandle,
    argv: string[],
    prompt: string,
    cwd?: string,
    env?: Record<string, string>,
  ): AsyncIterable<AgentEvent> {
    log.info("spawning openclaw subprocess", { taskId: handle.taskId, argv: argv.join(" "), cwd })

    const stderrTail = new StderrTail()
    const proc = Bun.spawn(argv, {
      stdin: "pipe",
      stdout: "pipe",
      stderr: "pipe",
      cwd,
      env: mergeEnv(process.env, env),
    })
    this.currentProc = proc
    this.currentTaskId = handle.taskId
    ProcessRegistry.track(proc, { label: `cli:${this.cliName}:openclaw` })

    ;(async () => {
      try {
        for await (const chunk of readStreamChunks(proc.stderr)) {
          stderrTail.append(Buffer.from(chunk))
        }
      } catch {
        // Stderr stream closed early; the tail is best-effort diagnostics.
      }
    })()

    const stdin = proc.stdin
    try {
      await stdin.write(prompt + "\n")
      await stdin.end()
    } catch {
      // stdin may already be closed by the agent.
    }

    let finished = false
    let stdoutBuf = ""

    try {
      for await (const line of readLines(proc.stdout)) {
        if (this.abortedTasks.has(handle.taskId)) {
          this.abortedTasks.delete(handle.taskId)
          const ev = { type: "status", status: "cancelled" } as AgentEvent
          yield ev
          await this.logEvent(handle.taskId, ev)
          return
        }

        stdoutBuf += line + "\n"
        if (!line.trim()) continue

        try {
          const evt = JSON.parse(line) as OpenclawEvent

          if (evt.type === "text" && typeof evt.text === "string") {
            const deltaEv = { type: "text_delta", delta: evt.text } as AgentEvent
            yield deltaEv
            await this.logEvent(handle.taskId, deltaEv)
          } else if (evt.type === "tool_use") {
            const toolCallEv = {
              type: "tool_call",
              id: String(evt.id ?? `${handle.taskId}-tool-use`),
              name: String(evt.name ?? "tool"),
              arguments: evt.input ?? {},
            } as AgentEvent
            yield toolCallEv
            await this.logEvent(handle.taskId, toolCallEv)
          } else if (evt.type === "tool_result") {
            const toolResultEv = {
              type: "tool_result",
              id: String(evt.id ?? `${handle.taskId}-tool-result`),
              content: typeof evt.content === "string" ? evt.content : extractTextContent(evt.content),
              isError: Boolean(evt.is_error),
            } as AgentEvent
            yield toolResultEv
            await this.logEvent(handle.taskId, toolResultEv)
          } else if (evt.type === "step_finish") {
            finished = true
            const usage = evt.usage ?? {}
            const finishEv = {
              type: "finish",
              finishReason: evt.is_error ? "error" : "stop",
              usage: {
                inputTokens: usage.input_tokens ?? 0,
                outputTokens: usage.output_tokens ?? 0,
                totalTokens: (usage.input_tokens ?? 0) + (usage.output_tokens ?? 0),
              },
            } as AgentEvent
            yield finishEv
            await this.logEvent(handle.taskId, finishEv)
            break
          } else if (evt.type === "error") {
            const detail = typeof evt.error === "string" ? evt.error : JSON.stringify(evt.error)
            const errorEv = { type: "error", error: new Error(detail) } as AgentEvent
            yield errorEv
            await this.logEvent(handle.taskId, errorEv)
          }
        } catch {
          // malformed JSON — could be the leading part of a final blob
        }
      }

      // If the CLI emitted a single final-result blob instead of NDJSON events,
      // parse the accumulated stdout now.
      if (!finished) {
        const result = parseOpenclawFinalBlob(stdoutBuf)
        if (result.text) {
          const deltaEv = { type: "text_delta", delta: result.text } as AgentEvent
          yield deltaEv
          await this.logEvent(handle.taskId, deltaEv)
        }
        const finishEv = {
          type: "finish",
          finishReason: "stop",
          usage: result.usage ?? zeroUsage(),
        } as AgentEvent
        yield finishEv
        await this.logEvent(handle.taskId, finishEv)
      }
    } catch (err) {
      log.error("openclaw request failed", { error: err, taskId: handle.taskId })
      const detail = stderrTail.tail() || (err instanceof Error ? err.message : String(err))
      const errorEv = { type: "error", error: new Error(detail) } as AgentEvent
      yield errorEv
      await this.logEvent(handle.taskId, errorEv)
      const finishEv = { type: "finish", finishReason: "error", usage: zeroUsage() } as AgentEvent
      yield finishEv
      await this.logEvent(handle.taskId, finishEv)
    } finally {
      terminateProcessTree(proc)
      this.resetCurrentTask()
    }
  }

  // ---------------------------------------------------------------------------
  // ACP stdio execution (JSON-RPC over stdio via @agentclientprotocol/sdk)
  // ---------------------------------------------------------------------------

  private async *runAcp(
    handle: TaskHandle,
    argv: string[],
    cwd?: string,
    env?: Record<string, string>,
  ): AsyncIterable<AgentEvent> {
    const task = this.tasks.get(handle.taskId)
    const taskCwd = cwd || task?.cwd || process.cwd()

    if (this.cliName === "qwenpaw") {
      argv = [...argv, "--workspace", taskCwd]
    }

    log.info("spawning acp subprocess", { taskId: handle.taskId, argv: argv.join(" "), cwd: taskCwd })

    const stderrTail = new StderrTail()
    const proc = nodeSpawn(argv[0], argv.slice(1), {
      stdio: ["pipe", "pipe", "pipe"],
      cwd: taskCwd,
      env: mergeEnv(process.env, env),
      detached: process.platform !== "win32",
    })
    this.currentProc = proc
    this.currentTaskId = handle.taskId
    ProcessRegistry.track(proc, { label: `cli:${this.cliName}:acp`, group: process.platform !== "win32" })

    proc.stderr?.on("data", (data: Buffer) => {
      stderrTail.append(data)
      log.warn("acp_agent_stderr", { taskId: handle.taskId, data: data.toString().slice(0, 500) })
    })

    const stream = ndJsonStream(
      Writable.toWeb(proc.stdin!) as WritableStream<Uint8Array>,
      Readable.toWeb(proc.stdout!) as unknown as ReadableStream<Uint8Array>,
    )

    const events: AgentEvent[] = []
    let done = false
    let notify = () => {}
    // Context-window info from ACP usage_update ({used, size}) — a fallback
    // hint only, never a source of billing-grade token counts.
    let contextUsage: { used: number; size: number } | undefined

    proc.on("exit", (code) => {
      if (code !== 0 && code !== null) {
        log.warn("acp_agent_exited", { taskId: handle.taskId, code })
      }
      done = true
      notify()
    })

    const pushEvent = (event: AgentEvent) => {
      events.push(event)
      notify()
    }

    const extractTextContent = (content: unknown): string => {
      if (!content) return ""
      if (typeof content === "string") return content
      if (typeof (content as { text?: unknown }).text === "string")
        return (content as { text: string }).text
      if (Array.isArray(content)) {
        return content
          .map((part: unknown) => {
            if (typeof part === "string") return part
            const p = part as Record<string, unknown>
            if (p?.type === "text") return String(p.text ?? "")
            if (p?.type === "diff") {
              return `\n--- ${String(p.path ?? "")}\n${String(p.oldText ?? "")}\n+++\n${String(p.newText ?? "")}`
            }
            if (p?.type === "terminal") return `[terminal ${String(p.terminalId ?? "")}]`
            if (p?.content) return extractTextContent(p.content)
            return ""
          })
          .join("")
      }
      const c = content as Record<string, unknown>
      if (typeof c.content === "string") return c.content
      return ""
    }

    const client = {
      sessionUpdate: async (params: unknown) => {
        if (this.abortedTasks.has(handle.taskId)) return
        const notification = params as { sessionId?: string; update?: Record<string, unknown> } | undefined
        const update = notification?.update
        if (!update || !update.sessionUpdate) return

        switch (update.sessionUpdate) {
          case "agent_message_chunk": {
            const text = extractTextContent(update.content)
            if (text) pushEvent({ type: "text_delta", delta: text })
            break
          }
          case "agent_thought_chunk": {
            const text = extractTextContent(update.content)
            if (text) pushEvent({ type: "reasoning_delta", delta: text })
            break
          }
          case "tool_call": {
            const toolCallId = String(update.toolCallId ?? "")
            if (!toolCallId) break
            pushEvent({
              type: "tool_call",
              id: toolCallId,
              name: String(update.title ?? update.kind ?? "tool"),
              arguments: update.rawInput ?? {},
            })
            break
          }
          case "tool_call_update": {
            const toolCallId = String(update.toolCallId ?? "")
            if (!toolCallId) break
            const status = String(update.status ?? "")
            if (status === "completed" || status === "failed") {
              pushEvent({
                type: "tool_result",
                id: toolCallId,
                content: extractTextContent(update.content),
                isError: status === "failed",
              })
            }
            break
          }
          case "usage_update": {
            // ACP usage_update is CONTEXT-WINDOW INFO ({used: tokens currently
            // in context, size: context window size}), not token billing —
            // deriving token counts from it (e.g. size - used) fabricates
            // numbers. Keep it only as a fallback hint; real per-turn counts
            // come from the prompt response's `usage` below.
            const used = typeof update.used === "number" ? update.used : 0
            const size = typeof update.size === "number" ? update.size : 0
            contextUsage = { used: Math.max(0, used), size: Math.max(0, size) }
            if (size > 0) pushEvent({ type: "context", used: Math.max(0, used), size })
            break
          }
          default:
            break
        }
      },
      requestPermission: async (request: Record<string, unknown>) => {
        const options = (request.options ?? []) as Array<Record<string, unknown>>
        const allowOnce = options.find((item) => item.kind === "allow_once")
        const allowAlways = options.find((item) => item.kind === "allow_always")
        const pickAllow = () =>
          allowOnce ?? allowAlways ?? options.find((item) => !String(item.kind).includes("reject"))
        const sessionID = task?.sessionID

        // Allternit spawn gate (hard floor + Gate 2): answer right away, so
        // the turn never waits on a person. Plan mode (read-only) is checked
        // first; with no Factory engine binary the in-process catastrophic floor
        // decides. Nothing here prompts.
        const gateTool = (request.toolCall ?? {}) as { kind?: unknown; title?: unknown }
        const gateMode = await PermissionNext.getMode(sessionID).catch(() => "default" as const)
        const verdict = await acpGateDecision({
          mode: gateMode,
          permission: acpPermissionFor(gateTool).permission,
          toolCall: (request.toolCall ?? {}) as Record<string, unknown>,
          cwd: taskCwd,
          harness: this.cliName,
          sessionId: sessionID,
          wihId: process.env.ALLTERNIT_FACTORY_WIH,
          root: process.env.ALLTERNIT_FACTORY_ROOT,
        })
        {
          log.info("acp gate decision", { taskId: handle.taskId, allow: verdict.allow, fallback: verdict.fallback ?? false, ...("reason" in verdict ? { reason: verdict.reason } : {}) })
          const chosen = verdict.allow
            ? pickAllow()
            : options.find((item) => String(item.kind).includes("reject"))
          return chosen
            ? { outcome: { outcome: "selected" as const, optionId: chosen.optionId } }
            : { outcome: { outcome: "cancelled" as const } }
        }

        // No session context (raw driver use outside a session turn): keep
        // the legacy best-effort behavior rather than guessing a policy.
        if (!sessionID) {
          const option = pickAllow()
          return option
            ? { outcome: { outcome: "selected" as const, optionId: option.optionId } }
            : { outcome: { outcome: "cancelled" as const } }
        }

        const toolCall = (request.toolCall ?? {}) as { kind?: unknown; title?: unknown }
        const { permission, pattern } = acpPermissionFor(toolCall)
        try {
          // Same gate as gizzi's own tools: plan-mode denies non-readonly
          // here; default mode publishes permission.asked and blocks until
          // the platform/user replies via PermissionNext.reply.
          await PermissionNext.ask({
            permission,
            patterns: [pattern],
            sessionID,
            metadata: { tool: String(toolCall.title ?? toolCall.kind ?? "tool") },
            always: allowAlways ? [pattern] : [],
            ruleset: [],
          })
        } catch (err) {
          log.info("acp permission denied", {
            taskId: handle.taskId,
            permission,
            pattern,
            error: err instanceof Error ? err.message : String(err),
          })
          const rejectOption = options.find((item) => String(item.kind).includes("reject"))
          return rejectOption
            ? { outcome: { outcome: "selected" as const, optionId: rejectOption.optionId } }
            : { outcome: { outcome: "cancelled" as const } }
        }
        const option = pickAllow()
        return option
          ? { outcome: { outcome: "selected" as const, optionId: option.optionId } }
          : { outcome: { outcome: "cancelled" as const } }
      },
      readTextFile: async () => {
        throw new Error("ACP file reads require an Allternit workspace grant")
      },
      writeTextFile: async () => {
        throw new Error("ACP file writes require an Allternit workspace grant")
      },
    }

    const acp = new ClientSideConnection(() => client as any, stream)

    try {
      const init = await acp.initialize({
        protocolVersion: PROTOCOL_VERSION,
        clientCapabilities: { fs: { readTextFile: false, writeTextFile: false } },
        clientInfo: { name: "Allternit", version: "1.0.0" },
      })

      const agentCapabilities = (init as { agentCapabilities?: unknown })?.agentCapabilities
      const mcpServers = acpMcpServers(task?.mcp, agentCapabilities) as any
      let session: { sessionId: string } | undefined
      // Resume the vendor's session from the previous turn when it supports
      // session/load; a failed load falls back to a fresh session.
      if (task?.vendorSessionId && acpCanLoadSession(agentCapabilities)) {
        try {
          await acp.loadSession({ sessionId: task.vendorSessionId, cwd: taskCwd, mcpServers })
          // session/load replays history as notifications; drop the replay.
          events.length = 0
          contextUsage = undefined
          session = { sessionId: task.vendorSessionId }
        } catch (err) {
          log.warn("acp session/load failed; starting a fresh session", {
            cli: this.cliName,
            error: err instanceof Error ? err.message : String(err),
          })
        }
      }
      if (!session) session = await acp.newSession({ cwd: taskCwd, mcpServers })
      events.push({ type: "vendor_session", id: session.sessionId })

      const promptText = withInstructions(task?.prompt ?? "", task?.systemPrompt)
      const attachmentBlocks = task?.attachments?.length
        ? attachmentsToAcpContent(task.attachments)
        : []
      const promptPromise = acp.prompt({
        sessionId: session.sessionId,
        prompt: [{ type: "text", text: promptText }, ...attachmentBlocks],
      })

      // Yield events as they arrive while the prompt is in flight.
      while (!done || events.length > 0) {
        while (events.length > 0) {
          const event = events.shift()!
          yield event
          await this.logEvent(handle.taskId, event)
        }
        if (!done) {
          await Promise.race([
            promptPromise.then(() => {
              done = true
            }),
            new Promise<void>((r) => {
              notify = r
            }),
          ])
        }
      }

      const result = await promptPromise
      const fatalTail = stderrTail.tail()
      if (acpStderrLooksFatal(fatalTail)) {
        const error = new Error(fatalTail.trim())
        log.error("acp prompt completed with fatal agent stderr", {
          taskId: handle.taskId,
          error: error.message.slice(0, 500),
        })
        const errorEv = { type: "error", error } as AgentEvent
        yield errorEv
        await this.logEvent(handle.taskId, errorEv)
        const failEv = { type: "finish", finishReason: "error", usage: zeroUsage() } as AgentEvent
        yield failEv
        await this.logEvent(handle.taskId, failEv)
        return
      }
      // Per-turn token counts come from the prompt response's `usage`
      // (ACP Usage: input/output totals across the agent's internal calls).
      // Only when the agent omits it do we fall back to the last context
      // high-water mark (tokens in context) with output unknown — never the
      // previous size-minus-used derivation, which reported the FREE portion
      // of the context window as "input tokens".
      const resultUsage = (result as { usage?: { inputTokens?: number; outputTokens?: number } | null } | undefined)?.usage
      const finishEv = {
        type: "finish",
        finishReason: result.stopReason ?? "stop",
        usage: resultUsage
          ? {
              inputTokens: Math.max(0, resultUsage.inputTokens ?? 0),
              outputTokens: Math.max(0, resultUsage.outputTokens ?? 0),
              totalTokens:
                Math.max(0, resultUsage.inputTokens ?? 0) + Math.max(0, resultUsage.outputTokens ?? 0),
            }
          : contextUsage
            ? { inputTokens: contextUsage.used, outputTokens: 0, totalTokens: contextUsage.used }
            : zeroUsage(),
      } as AgentEvent
      yield finishEv
      await this.logEvent(handle.taskId, finishEv)
    } catch (err) {
      log.error("acp request failed", { error: err, taskId: handle.taskId })
      const tail = stderrTail.tail()
      const error =
        tail && tail.trim()
          ? new Error(`${err instanceof Error ? err.message : String(err)}\n\nagent stderr:\n${tail}`)
          : err
      const errorEv = { type: "error", error } as AgentEvent
      yield errorEv
      await this.logEvent(handle.taskId, errorEv)
      const finishEv = { type: "finish", finishReason: "error", usage: zeroUsage() } as AgentEvent
      yield finishEv
      await this.logEvent(handle.taskId, finishEv)
    } finally {
      this.resetCurrentTask()
      terminateProcessTree(proc)
    }
  }

  // ---------------------------------------------------------------------------
  // Codex app-server JSON-RPC runner
  // ---------------------------------------------------------------------------

  private async *runCodexAppServer(
    handle: TaskHandle,
    argv: string[],
    prompt: string,
    cwd?: string,
    systemPrompt?: string,
    env?: Record<string, string>,
    model?: string,
  ): AsyncIterable<AgentEvent> {
    log.info("spawning codex app-server subprocess", {
      taskId: handle.taskId,
      argv: argv.join(" "),
      cwd,
    })

    const stderrTail = new StderrTail()
    const proc = Bun.spawn(argv, {
      stdin: "pipe",
      stdout: "pipe",
      stderr: "pipe",
      cwd,
      env: mergeEnv(process.env, env),
    })
    this.currentProc = proc
    this.currentTaskId = handle.taskId
    ProcessRegistry.track(proc, { label: `cli:${this.cliName}:codex` })

    const stdin = proc.stdin
    const send = (msg: Record<string, unknown>) => {
      writeToStdin(stdin, JSON.stringify(msg) + "\n")
    }

    let nextId = 1
    const pending = new Map<
      number,
      { resolve: (value: unknown) => void; reject: (reason: unknown) => void }
    >()

    const request = async (method: string, params: unknown): Promise<unknown> => {
      const id = nextId++
      send({ jsonrpc: "2.0", id, method, params })
      return new Promise((resolve, reject) => {
        pending.set(id, { resolve, reject })
      })
    }

    const notify = (method: string, params?: unknown) => {
      send({ jsonrpc: "2.0", method, params })
    }

    const respond = (id: number, result: unknown) => {
      send({ jsonrpc: "2.0", id, result })
    }

    const respondError = (id: number, code: number, message: string) => {
      send({ jsonrpc: "2.0", id, error: { code, message } })
    }

    const events: AgentEvent[] = []
    const streamedMessages = new Set<string>()
    let done = false
    let notifyYield = () => {}
    let finished = false

    const pushEvent = (event: AgentEvent) => {
      events.push(event)
      notifyYield()
    }

    const codexPermissionsApprovalResponse = (params: Record<string, unknown>): Record<string, unknown> => {
      const permissions = (params.permissions ?? {}) as Record<string, unknown>
      const granted: Record<string, unknown> = {}
      for (const [key, value] of Object.entries(permissions)) {
        if ((key === "network" || key === "fileSystem") && value != null) {
          granted[key] = value
        } else {
          log.warn("codex: dropping unrecognized permission key", { key })
        }
      }
      return { permissions: granted, scope: "turn" }
    }

    // Capture a bounded stderr tail for diagnostics.
    ;(async () => {
      try {
        for await (const line of readLines(proc.stderr)) {
          if (line.trim()) {
            stderrTail.append(Buffer.from(line + "\n"))
            log.warn("codex_app_server_stderr", { taskId: handle.taskId, data: line.slice(0, 500) })
          }
        }
      } catch {
        // Stderr stream closed early; the tail is best-effort diagnostics.
      }
    })()

    // Reader task
    const readerPromise = (async () => {
      try {
        for await (const line of readLines(proc.stdout)) {
          if (this.abortedTasks.has(handle.taskId)) return
          if (!line.trim()) continue

          try {
            const msg = JSON.parse(line) as JsonRpcMessage
            const hasId = typeof msg.id === "number"
            const hasMethod = typeof msg.method === "string" && msg.method !== ""
            const hasResult = "result" in msg
            const hasError = "error" in msg

            if (hasId) {
              if (pending.has(msg.id!)) {
                const p = pending.get(msg.id!)!
                pending.delete(msg.id!)
                if (hasError) {
                  p.reject(msg.error)
                } else {
                  p.resolve(msg.result)
                }
                continue
              }

              if (hasMethod) {
                // Server request (has both id and method) — auto-approve in autonomous mode.
                switch (msg.method) {
                  case "item/commandExecution/requestApproval":
                  case "execCommandApproval":
                    respond(msg.id!, { decision: "accept" })
                    break
                  case "item/fileChange/requestApproval":
                  case "applyPatchApproval":
                    respond(msg.id!, { decision: "accept" })
                    break
                  case "item/permissions/requestApproval":
                    respond(msg.id!, codexPermissionsApprovalResponse(msg.params ?? {}))
                    break
                  case "mcpServer/elicitation/request":
                    respond(msg.id!, { action: "accept", content: null, _meta: null })
                    break
                  default:
                    log.warn("codex: unhandled server request", { method: msg.method, id: msg.id })
                    respondError(msg.id!, -32601, `unsupported codex app-server request: ${msg.method}`)
                }
                continue
              }

              // Response to an unknown request — ignore.
              continue
            }

            // Notification (no id, has method)
            if (!hasMethod) continue
            const method = msg.method
            const params = (msg.params ?? {}) as Record<string, unknown>

            if (method === "item/agentMessage/delta") {
              const delta = params.delta
              if (typeof delta === "string" && delta) {
                streamedMessages.add(String(params.itemId ?? ""))
                pushEvent({ type: "text_delta", delta })
              }
              continue
            }

            if (method === "turn/started" || method === "codex/event") {
              const event = params.event as Record<string, unknown> | undefined
              if (method === "turn/started" || event?.type === "task_started") {
                pushEvent({ type: "status", status: "running" })
              }
              continue
            }

            if (method === "item/completed") {
              const reasoning = codexReasoningText(params)
              if (reasoning) pushEvent({ type: "reasoning_delta", delta: reasoning })
              const item = params.item as Record<string, unknown> | undefined
              if (item?.type === "agentMessage" && !streamedMessages.has(String(item.id ?? ""))) {
                const text = extractCodexText(item)
                if (text) pushEvent({ type: "text_delta", delta: text })
              }
              continue
            }

            if (method === "item/commandExecution/started") {
              pushEvent({
                type: "tool_call",
                id: String(params.id ?? `${handle.taskId}-command`),
                name: "exec_command",
                arguments: params,
              })
              continue
            }

            if (method === "item/commandExecution/completed") {
              pushEvent({
                type: "tool_result",
                id: String(params.id ?? `${handle.taskId}-command`),
                content: extractCodexText(params),
                isError: Boolean(params.error),
              })
              continue
            }

            if (method === "item/fileChange/started") {
              pushEvent({
                type: "tool_call",
                id: String(params.id ?? `${handle.taskId}-file-change`),
                name: "patch_apply",
                arguments: params,
              })
              continue
            }

            if (method === "item/fileChange/completed") {
              pushEvent({
                type: "tool_result",
                id: String(params.id ?? `${handle.taskId}-file-change`),
                content: extractCodexText(params),
                isError: Boolean(params.error),
              })
              continue
            }

            if (method === "turn/completed") {
              finished = true
              done = true
              const turn = params.turn as Record<string, unknown> | undefined
              if (turn?.status === "failed") {
                const detail = (turn.error as Record<string, unknown> | undefined)?.message
                pushEvent({ type: "error", error: new Error(String(detail || "Codex turn failed")) })
              }
              pushEvent({ type: "finish", finishReason: turn?.status === "completed" ? "stop" : "error", usage: zeroUsage() })
              continue
            }
          } catch {
            // malformed JSON-RPC line — skip
          }
        }
      } finally {
        done = true
        notifyYield()
      }
    })()

    try {
      await request("initialize", {
        clientInfo: { name: "Allternit", version: "1.0.0" },
        capabilities: { experimentalApi: true },
      })

      notify("initialized", {})

      const mcpConfig = codexMcpConfig(this.tasks.get(handle.taskId)?.mcp)
      const startParams = {
        cwd: cwd || process.cwd(),
        ...(model ? { model } : {}),
        developerInstructions: systemPrompt,
        ...(mcpConfig ? { config: mcpConfig } : {}),
      }
      const resumeId = this.tasks.get(handle.taskId)?.vendorSessionId
      let thread: unknown
      if (resumeId) {
        const req = codexThreadRequest(resumeId, startParams)
        try {
          thread = await request(req.method, req.params)
        } catch (err) {
          log.warn("codex thread/resume failed; starting a fresh thread", {
            error: err instanceof Error ? err.message : String(err),
          })
        }
      }
      thread ??= await request("thread/start", startParams)
      const threadId = codexThreadId(thread)
      if (!threadId) throw new Error("codex app-server did not return thread.id")
      events.push({ type: "vendor_session", id: threadId })

      await request("turn/start", {
        threadId,
        input: [{ type: "text", text: prompt }],
      })

      while (!done || events.length > 0) {
        while (events.length > 0) {
          const event = events.shift()!
          yield event
          await this.logEvent(handle.taskId, event)
        }
        if (!done) {
          await new Promise<void>((r) => {
            notifyYield = r
          })
        }
      }

      if (!finished) {
        const finishEv = { type: "finish", finishReason: "stop", usage: zeroUsage() } as AgentEvent
        yield finishEv
        await this.logEvent(handle.taskId, finishEv)
      }
    } catch (err) {
      log.error("codex app-server request failed", { error: err, taskId: handle.taskId })
      const tail = stderrTail.tail()
      const error =
        tail && tail.trim()
          ? new Error(`${err instanceof Error ? err.message : String(err)}\n\nagent stderr:\n${tail}`)
          : err
      const errorEv = { type: "error", error } as AgentEvent
      yield errorEv
      await this.logEvent(handle.taskId, errorEv)
      const finishEv = { type: "finish", finishReason: "error", usage: zeroUsage() } as AgentEvent
      yield finishEv
      await this.logEvent(handle.taskId, finishEv)
    } finally {
      try {
        await stdin.end()
      } catch {
        // stdin may already be closed when the process died.
      }
      await readerPromise.catch(() => {})
      terminateProcessTree(proc)
      this.resetCurrentTask()
    }
  }
}

// ---------------------------------------------------------------------------
// CLI adapters
// ---------------------------------------------------------------------------
//
// Each discovered CLI maps to an explicit adapter keyed by its provider id
// (matching SUBPROCESS_PROVIDERS ids). There are no generic fallbacks.

type AdapterMode =
  | "stream-json"
  | "openclaw-json"
  | "acp"
  | "codex-app-server"
  | "one-shot-json"
  | "one-shot-text"

/**
 * The content blocks of a stream-json "user" event (where tool results come
 * back). Claude Code nests them in message.content, like assistant events;
 * older shapes put them at the top level. Missing this left every CLI tool
 * call "running" until the turn ended and marked it aborted.
 */
/**
 * Text bookkeeping for Claude's stream-json output, so each character is
 * sent once.
 *
 * Claude CLI emits every content block as its own `assistant` event — one
 * part, at index 0 — and, with --include-partial-messages, streams the same
 * block first as `stream_event` deltas. The driver used to track sent text
 * by part index across the whole run, so a later block was sliced by an
 * earlier block's length: "alpha … beta" lost "beta" entirely, and a longer
 * reply lost its opening. Blocks are now keyed by their message id and
 * their position in that message.
 */
export class StreamJsonBlocks {
  private sent = new Map<string, number>()
  private seen = new Map<string, number>()
  private streamMessage = ""
  private anonymous = 0

  /** A partial-message event: the text or thinking to send now, if any. */
  streamEvent(event: any): { kind: "text" | "thinking"; delta: string } | null {
    if (!event || typeof event !== "object") return null
    if (event.type === "message_start") {
      this.streamMessage = String(event.message?.id ?? "")
      return null
    }
    if (event.type !== "content_block_delta" || !this.streamMessage) return null
    const key = `${this.streamMessage}:${Number(event.index ?? 0)}`
    const d = event.delta ?? {}
    const text = d.type === "text_delta" ? d.text : d.type === "thinking_delta" ? d.thinking : undefined
    if (typeof text !== "string" || !text) return null
    this.sent.set(key, (this.sent.get(key) ?? 0) + text.length)
    return { kind: d.type === "text_delta" ? "text" : "thinking", delta: text }
  }

  /**
   * An `assistant` event: for each part, the text not yet sent (partial
   * deltas already covered it, or an earlier cumulative event did).
   */
  assistantParts(message: any): string[] {
    const content: any[] = Array.isArray(message?.content) ? message.content : []
    const id = typeof message?.id === "string" && message.id ? message.id : `anon-${this.anonymous++}`
    // One part = the next block of this message; several = a cumulative
    // snapshot of all its blocks so far.
    const single = content.length === 1
    const base = single ? (this.seen.get(id) ?? 0) : 0
    if (single) this.seen.set(id, base + 1)
    return content.map((part, idx) => {
      const full = part?.type === "text" ? part.text : part?.type === "thinking" ? part.thinking : undefined
      if (typeof full !== "string") return ""
      const key = `${id}:${single ? base : idx}`
      const prev = this.sent.get(key) ?? 0
      if (full.length <= prev) return ""
      this.sent.set(key, full.length)
      return full.slice(prev)
    })
  }
}

/**
 * Why a stream-json turn whose stdout closed before its `result` event did
 * not finish — or undefined when the CLI simply exited cleanly (dialects
 * that never send `result`). A CLI that always ends a turn with `result`
 * (Claude, CodeBuddy) and didn't was cut short, whatever its exit code.
 */
export function streamJsonCutShort(input: {
  cliName: string
  endsWithResult: boolean
  exitCode: number | null
  signalCode: string | null
  stderr?: string
}): string | undefined {
  const failedExit = input.exitCode !== null && input.exitCode !== 0
  if (!input.signalCode && !failedExit && !input.endsWithResult) return undefined
  const how = input.signalCode
    ? `was stopped (${input.signalCode})`
    : failedExit
      ? `exited with code ${input.exitCode}`
      : "stopped"
  const detail = input.stderr?.trim()
  return `${input.cliName} ${how} before finishing the turn${detail ? `: ${detail}` : ""}`
}

export function streamJsonUserContent(evt: any): any[] | null {
  if (evt?.type !== "user") return null
  if (Array.isArray(evt.message?.content)) return evt.message.content
  if (Array.isArray(evt.content)) return evt.content
  return null
}

interface CliAdapter {
  mode: AdapterMode
  /** Whether this adapter can forward task attachments to the CLI. */
  supportsAttachments?: boolean
  /** For stream-json adapters: deliver the prompt as raw stdin text instead of NDJSON. */
  promptOnStdin?: boolean
  /**
   * For stream-json adapters: every turn ends with a `result` event, so
   * stdout closing without one means the turn was cut short.
   */
  endsWithResult?: boolean
  /** Environment every run of this CLI gets (the task's own env wins). */
  env?: Record<string, string>
  /** Build the final argv. */
  buildArgv(
    baseCmd: string[],
    message: string,
    ctx: { cwd?: string; taskId: string; systemPrompt?: string; mcp?: AgentTask["mcp"]; vendorSessionId?: string },
  ): string[]
}

function claudeStreamJsonInput(message: string): string {
  return (
    JSON.stringify({
      type: "user",
      message: { role: "user", content: [{ type: "text", text: message }] },
    }) + "\n"
  )
}

function modelFlag(modelEnv?: string): string[] {
  return modelEnv ? ["--model", modelEnv] : []
}

const CLI_ADAPTERS: Record<string, CliAdapter> = {
  // Anthropic gizzi-code — stream-json.
  "claude-cli": {
    mode: "stream-json",
    endsWithResult: true,
    // An Allternit session must not route work to the user's claude.ai
    // connectors (Claude Docs, Drive…): document work goes to the pane via
    // gizzi's own tools. Seen live: a Docs session wrote a claude.ai doc.
    env: { ENABLE_CLAUDEAI_MCP_SERVERS: "false" },
    buildArgv: ([command], _message, ctx) => {
      return [
        command,
        "-p",
        "--output-format", "stream-json",
        "--input-format", "stream-json",
        "--verbose",
        // Text streams as it's written, not one block at a time.
        "--include-partial-messages",
        "--permission-mode", "bypassPermissions",
        "--disallowedTools", "AskUserQuestion",
        ...claudeSessionFlags(ctx),
        ...modelFlag(PROVIDER_ENV_KEYS["claude-cli"]?.model ? process.env[PROVIDER_ENV_KEYS["claude-cli"]!.model!] : undefined),
      ]
    },
  },

  // CodeBuddy — same stream-json dialect as Claude.
  codebuddy: {
    mode: "stream-json",
    endsWithResult: true,
    buildArgv: ([command]) => {
      return [
        command,
        "-p",
        "--output-format", "stream-json",
        "--input-format", "stream-json",
        "--verbose",
        "--permission-mode", "bypassPermissions",
        "--disallowedTools", "AskUserQuestion",
        "--disallowedTools", "EnterPlanMode",
        "--disallowedTools", "ExitPlanMode",
        ...modelFlag(PROVIDER_ENV_KEYS["codebuddy"]?.model ? process.env[PROVIDER_ENV_KEYS["codebuddy"]!.model!] : undefined),
      ]
    },
  },

  // Cursor Agent — stream-json, prompt delivered on stdin.
  "cursor-agent": {
    mode: "stream-json",
    promptOnStdin: true,
    buildArgv: ([command], _message, ctx) => {
      return [
        command,
        "-p",
        "--output-format", "stream-json",
        "--yolo",
        "--workspace", ctx.cwd || process.cwd(),
        ...modelFlag(PROVIDER_ENV_KEYS["cursor-agent"]?.model ? process.env[PROVIDER_ENV_KEYS["cursor-agent"]!.model!] : undefined),
      ]
    },
  },

  // OpenCode — stream-json, prompt delivered on stdin.
  opencode: {
    mode: "stream-json",
    promptOnStdin: true,
    buildArgv: ([command], _message, ctx) => {
      return [
        command,
        "run",
        "--format", "json",
        "--dangerously-skip-permissions",
        "--dir", ctx.cwd || process.cwd(),
        ...opencodeResumeFlags(ctx.vendorSessionId),
        ...modelFlag(PROVIDER_ENV_KEYS["opencode"]?.model ? process.env[PROVIDER_ENV_KEYS["opencode"]!.model!] : undefined),
      ]
    },
  },

  // DevEco Code — stream-json, prompt appended as positional arg.
  deveco: {
    mode: "stream-json",
    buildArgv: ([command], message, ctx) => {
      return [
        command,
        "run",
        "--format", "json",
        "--dangerously-skip-permissions",
        "--dir", ctx.cwd || process.cwd(),
        ...modelFlag(PROVIDER_ENV_KEYS["deveco"]?.model ? process.env[PROVIDER_ENV_KEYS["deveco"]!.model!] : undefined),
        message,
      ]
    },
  },

  // OpenClaw — simplified NDJSON / final-blob runner.
  openclaw: {
    mode: "openclaw-json",
    buildArgv: ([command], message, ctx) => {
      const model = PROVIDER_ENV_KEYS["openclaw"]?.model
        ? process.env[PROVIDER_ENV_KEYS["openclaw"]!.model!]
        : undefined
      const args = [
        command,
        "agent",
        "--local",
        "--json",
        "--session-id", ctx.taskId,
      ]
      if (model) {
        args.push("--agent", model)
      }
      args.push("--message", message)
      return args
    },
  },

  // Alibaba Qwen Code — stream-json.
  "qwen-cli": {
    mode: "stream-json",
    buildArgv: ([command], message, ctx) => {
      return [
        command,
        "-p", message,
        "--output-format", "stream-json",
        "--yolo",
        ...qwenResumeFlags(ctx.vendorSessionId),
        ...modelFlag(PROVIDER_ENV_KEYS["qwen-cli"]?.model ? process.env[PROVIDER_ENV_KEYS["qwen-cli"]!.model!] : undefined),
      ]
    },
  },

  // OpenAI Codex — JSON-RPC app-server over stdio.
  "codex-cli": {
    mode: "codex-app-server",
    buildArgv: ([command]) => {
      return [command, "app-server", "--listen", "stdio://"]
    },
  },

  // Moonshot Kimi — ACP.
  "kimi-cli": {
    mode: "acp",
    supportsAttachments: true,
    buildArgv: ([command]) => [command, "acp"],
  },

  // MiniMax Code — ACP.
  mcode: {
    mode: "acp",
    supportsAttachments: true,
    buildArgv: ([command]) => [command, "acp"],
  },

  // Pi — one-shot JSON.
  pi: {
    mode: "one-shot-json",
    buildArgv: ([command], message) => [command, "-p", "--mode", "json", message],
  },

  // Oh-My-Pi — one-shot JSON.
  omp: {
    mode: "one-shot-json",
    buildArgv: ([command], message) => [command, "-p", "--mode", "json", message],
  },

  // Antigravity (agy) — one-shot text.
  antigravity: {
    mode: "one-shot-text",
    buildArgv: ([command], message) => [command, "--print", message],
  },

  // Hermes — ACP stdio.
  hermes: {
    mode: "acp",
    supportsAttachments: true,
    buildArgv: ([command]) => [command, "acp"],
  },

  // Grok Build — ACP stdio.
  grok: {
    mode: "acp",
    supportsAttachments: true,
    buildArgv: ([command]) => [command, "agent", "--no-leader", "--always-approve", "stdio"],
  },

  // Kiro CLI — ACP stdio.
  "kiro-cli": {
    mode: "acp",
    supportsAttachments: true,
    buildArgv: ([command]) => [command, "acp"],
  },

  // Qoder CLI — ACP stdio.
  qodercli: {
    mode: "acp",
    supportsAttachments: true,
    buildArgv: ([command]) => [command, "--yolo", "--acp"],
  },

  // Qoder CN CLI — ACP stdio.
  qoderclicn: {
    mode: "acp",
    supportsAttachments: true,
    buildArgv: ([command]) => [command, "--yolo", "--acp"],
  },

  // QwenPaw — ACP stdio.
  qwenpaw: {
    mode: "acp",
    supportsAttachments: true,
    buildArgv: ([command]) => [command, "acp"],
  },

  // Reasonix — ACP stdio.
  reasonix: {
    mode: "acp",
    supportsAttachments: true,
    buildArgv: ([command]) => [command, "acp"],
  },

  // Trae CLI — ACP stdio.
  traecli: {
    mode: "acp",
    supportsAttachments: true,
    buildArgv: ([command]) => [command, "acp", "serve", "--yolo"],
  },
}

function resolveAdapter(name: string): CliAdapter {
  const adapter = CLI_ADAPTERS[name]
  if (adapter) return adapter

  const unsupportedByProtocol: Record<string, string> = {
    dsh: "DSH multica stdio (private JSONL protocol)",
    "copilot-cli": "Copilot suggest mode (not a verified agent chat protocol)",
  }

  const protocol = unsupportedByProtocol[name]
  if (protocol) {
    throw new Error(
      `CLI "${name}" is installed but uses ${protocol}, which this local driver does not yet implement.`,
    )
  }

  throw new Error(
    `CLI "${name}" has no local driver adapter. Add it to CLI_ADAPTERS in local-cli-driver.ts or disable the CLI.`,
  )
}

export interface CliAdapterInfo {
  supported: boolean
  mode?: AdapterMode
  supportsAttachments?: boolean
  reason?: string
}

export function getCliAdapterInfo(name: string): CliAdapterInfo {
  const adapter = CLI_ADAPTERS[name]
  if (adapter) {
    // OpenClaw uses a dedicated runner but speaks a stream-json-style NDJSON wire.
    const mode = adapter.mode === "openclaw-json" ? "stream-json" : adapter.mode
    return { supported: true, mode, supportsAttachments: adapter.supportsAttachments ?? false }
  }

  const unsupportedByProtocol: Record<string, string> = {
    dsh: "DSH multica stdio (private JSONL protocol)",
    "copilot-cli": "Copilot suggest mode (not a verified agent chat protocol)",
    copilot: "Copilot suggest mode (not a verified agent chat protocol)",
  }

  const protocol = unsupportedByProtocol[name]
  if (protocol) {
    return { supported: false, reason: `${protocol} is not implemented by the local driver yet` }
  }

  return { supported: false, reason: "No adapter registered for this CLI" }
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

function parseCmd(cmd: string): string[] {
  const trimmed = cmd.trim()
  if (!trimmed) return []
  return trimmed.split(/\s+/)
}

async function readText(stream?: ReadableStream<Uint8Array> | null): Promise<string> {
  if (!stream) return ""
  const reader = stream.getReader()
  const decoder = new TextDecoder()
  let text = ""
  try {
    while (true) {
      const { done, value } = await reader.read()
      if (done) break
      text += decoder.decode(value, { stream: true })
    }
    text += decoder.decode()
  } finally {
    reader.releaseLock()
  }
  return text
}

async function* readLines(stream: ReadableStream<Uint8Array>): AsyncGenerator<string> {
  const reader = stream.getReader()
  const decoder = new TextDecoder()
  let buf = ""
  try {
    while (true) {
      const { done, value } = await reader.read()
      if (done) {
        if (buf.length > 0) yield buf
        break
      }
      buf += decoder.decode(value, { stream: true })
      const lines = buf.split("\n")
      buf = lines.pop() ?? ""
      for (const line of lines) yield line
    }
  } finally {
    reader.releaseLock()
  }
}

async function* readStreamChunks(stream: ReadableStream<Uint8Array>): AsyncGenerator<Uint8Array> {
  const reader = stream.getReader()
  try {
    while (true) {
      const { done, value } = await reader.read()
      if (done) break
      if (value) yield value
    }
  } finally {
    reader.releaseLock()
  }
}

function generateTaskId(): string {
  return `task-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`
}

function zeroUsage() {
  return { inputTokens: 0, outputTokens: 0, totalTokens: 0 }
}

function extractOneShotOutput(raw: string): string {
  if (!raw) return raw

  if (raw.startsWith("{") || raw.startsWith("[")) {
    try {
      const parsed = JSON.parse(raw)
      const candidate =
        parsed.text ??
        parsed.content ??
        parsed.response ??
        parsed.message ??
        parsed.answer ??
        parsed.output
      if (typeof candidate === "string") return candidate
      if (Array.isArray(candidate)) {
        return candidate
          .map((part: unknown) => (typeof part === "string" ? part : (part as { text?: string })?.text ?? ""))
          .join("")
      }
    } catch {
      // Not valid JSON — fall back to raw output.
    }
  }

  return raw
}

function extractTextContent(content: unknown): string {
  if (!content) return ""
  if (typeof content === "string") return content
  if (Array.isArray(content)) {
    return content
      .map((part: unknown) => {
        if (typeof part === "string") return part
        const p = part as Record<string, unknown>
        if (p?.type === "text" && typeof p.text === "string") return p.text
        if (p?.type === "text") return String(p.text ?? "")
        if (p?.text && typeof p.text === "string") return p.text
        return ""
      })
      .join("")
  }
  const c = content as Record<string, unknown>
  if (typeof c.text === "string") return c.text
  return ""
}

function mergeEnv(base: NodeJS.ProcessEnv, extra?: Record<string, string>): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = {}
  for (const [key, value] of Object.entries(base)) {
    if (value === undefined) continue
    if (isFilteredChildEnvKey(key)) continue
    env[key] = value
  }
  if (extra) {
    for (const [key, value] of Object.entries(extra)) {
      env[key] = value
    }
  }
  return env
}

/**
 * isFilteredChildEnvKey mirrors Multica's child env filtering.
 *
 * Inherited MULTICA_* overrides are discovery-time configuration for the
 * parent process and must not leak into agent CLIs (they can confuse nested
 * sessions or expose internal path overrides). Gizzi internal runtime
 * markers are also stripped; user-facing GIZZI_* config vars are kept.
 */
function isFilteredChildEnvKey(key: string): boolean {
  const up = key.toUpperCase()
  if (up.startsWith("MULTICA_")) return true
  switch (up) {
    case "GIZZI_CODE":
    case "GIZZI_ENTRYPOINT":
    case "GIZZI_SESSION_ID":
      return true
  }
  return up.startsWith("GIZZI_CODE_")
}

function writeToStdin(sink: Bun.FileSink | WritableStream<Uint8Array>, text: string): void {
  if (typeof (sink as WritableStream<Uint8Array>).getWriter === "function") {
    const writer = (sink as WritableStream<Uint8Array>).getWriter()
    writer.write(new TextEncoder().encode(text)).catch(() => {})
    writer.releaseLock()
    return
  }
  Promise.resolve((sink as Bun.FileSink).write(text)).catch(() => {})
}

function writeControlResponse(
  stdin: Bun.FileSink | WritableStream<Uint8Array>,
  requestId: string,
  input?: Record<string, unknown> | string,
): void {
  let updatedInput: Record<string, unknown> | undefined
  if (typeof input === "string") {
    try {
      updatedInput = JSON.parse(input) as Record<string, unknown>
    } catch {
      updatedInput = {}
    }
  } else if (input && typeof input === "object") {
    updatedInput = { ...input }
  } else {
    updatedInput = {}
  }

  // Multica forces tools to run in the foreground so a cancelled task cannot
  // leave a background subprocess spinning.
  if (updatedInput.run_in_background === true) {
    updatedInput.run_in_background = false
  }

  const response = {
    type: "control_response",
    response: {
      subtype: "success",
      request_id: requestId,
      response: {
        behavior: "allow",
        updatedInput,
      },
    },
  }
  writeToStdin(stdin, JSON.stringify(response) + "\n")
}

// ---------------------------------------------------------------------------
// Production process hygiene (matches Multica's Go helpers)
// ---------------------------------------------------------------------------

const STDERR_TAIL_BYTES = 2048

class StderrTail {
  private chunks: Buffer[] = []
  private length = 0

  append(data: Buffer): void {
    this.chunks.push(data)
    this.length += data.length
    while (this.length > STDERR_TAIL_BYTES && this.chunks.length > 1) {
      const first = this.chunks.shift()!
      this.length -= first.length
    }
    // If a single chunk is still oversized, keep its trailing bytes.
    if (this.length > STDERR_TAIL_BYTES && this.chunks.length === 1) {
      const first = this.chunks[0]!
      const trimmed = first.subarray(-STDERR_TAIL_BYTES)
      this.chunks = [trimmed]
      this.length = trimmed.length
    }
  }

  tail(): string {
    return Buffer.concat(this.chunks).toString("utf-8").slice(-STDERR_TAIL_BYTES)
  }
}

interface KillableProcess {
  pid?: number
  kill: () => void
}

function terminateProcessTree(proc: KillableProcess, graceMs = 5000): void {
  if (!proc.pid) {
    safeKill(proc)
    return
  }

  // On Unix, try a graceful process-group SIGTERM first, then SIGKILL.
  if (process.platform !== "win32") {
    try {
      process.kill(-proc.pid, "SIGTERM")
      const timer = setTimeout(() => {
        try {
          process.kill(-proc.pid, "SIGKILL")
        } catch {
          // Process group already gone.
        }
      }, graceMs)
      timer.unref?.()
      return
    } catch {
      // Fall back to killing the leader directly.
    }
  }

  safeKill(proc)
}

function safeKill(proc: KillableProcess): void {
  try {
    proc.kill()
  } catch {
    // Already exited.
  }
}

interface StreamJsonEvent {
  type: string
  status?: string
  message?: { id?: string; content?: Array<Record<string, unknown>> }
  /** `stream_event` (--include-partial-messages): the raw API stream event. */
  event?: Record<string, unknown>
  content?: Array<Record<string, unknown>> | Record<string, unknown> | string
  usage?: { input_tokens?: number; output_tokens?: number }
  is_error?: boolean
  request_id?: string
  request?: { input?: Record<string, unknown> | string }
}

interface OpenclawEvent {
  type: string
  text?: string
  id?: string
  name?: string
  input?: unknown
  content?: unknown
  is_error?: boolean
  usage?: { input_tokens?: number; output_tokens?: number }
  error?: unknown
}

function parseOpenclawFinalBlob(
  raw: string,
): { text: string; usage?: { inputTokens: number; outputTokens: number; totalTokens: number } } {
  const trimmed = raw.trim()
  if (!trimmed) return { text: "" }

  if (trimmed.startsWith("{")) {
    try {
      const parsed = JSON.parse(trimmed)
      const payloads = parsed.payloads
      if (Array.isArray(payloads)) {
        const text = payloads
          .map((p: Record<string, unknown>) => extractTextContent(p))
          .join("")
        const meta = parsed.meta as Record<string, unknown> | undefined
        const agentMeta = meta?.agentMeta as Record<string, unknown> | undefined
        const usage = agentMeta?.usage as Record<string, unknown> | undefined
        return {
          text,
          usage: usage
            ? {
                inputTokens: Number(usage.input_tokens ?? 0),
                outputTokens: Number(usage.output_tokens ?? 0),
                totalTokens: Number(usage.total_tokens ?? 0),
              }
            : zeroUsage(),
        }
      }
    } catch {
      // not a JSON blob
    }
  }

  return { text: trimmed }
}

interface JsonRpcMessage {
  jsonrpc?: string
  id?: number
  method?: string
  params?: Record<string, unknown>
  result?: unknown
  error?: unknown
}

/** Codex app-server nests the id in the returned thread object. */
export function codexThreadId(result: unknown): string | undefined {
  if (!result || typeof result !== "object") return undefined
  const thread = (result as { thread?: unknown }).thread
  if (!thread || typeof thread !== "object") return undefined
  const id = (thread as { id?: unknown }).id
  return typeof id === "string" && id ? id : undefined
}

/**
 * Reasoning carried by a Codex `item/completed` notification — either a
 * dedicated `reasoning` field or an `item` of type "reasoning" (summary
 * paragraphs preferred over raw content, matching what Codex shows itself).
 */
export function codexReasoningText(params: Record<string, unknown>): string {
  const item = params.item as Record<string, unknown> | undefined
  const source =
    (params.reasoning as Record<string, unknown> | undefined) ??
    (item?.type === "reasoning" ? item : undefined)
  if (!source) return ""
  const summary = source.summary
  if (Array.isArray(summary)) {
    const text = summary.map((s) => (typeof s === "string" ? s : extractTextContent(s))).filter(Boolean).join("\n\n")
    if (text) return text
  }
  return extractCodexText(source)
}

function extractCodexText(params: Record<string, unknown>): string {
  const text = params.text
  if (typeof text === "string") return text
  const content = params.content
  if (content) return extractTextContent(content)
  const data = params.data
  if (data) return extractTextContent(data)
  return ""
}

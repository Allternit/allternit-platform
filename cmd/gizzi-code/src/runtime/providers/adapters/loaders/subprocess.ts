/**
 * SubprocessLanguageModel — thin AI SDK LanguageModelV2 wrapper over the runtime driver system.
 *
 * The actual CLI execution lives in `runtime/drivers/local-cli-driver.ts` (and,
 * in Phase 3, remote websocket/uds drivers). This file only:
 *   1. Resolves the provider's CLI to a registered runtime.
 *   2. Creates a driver task from the last user message.
 *   3. Maps driver AgentEvents to LanguageModelV2StreamParts.
 */

import type { LanguageModelV2, LanguageModelV2StreamPart } from "@ai-sdk/provider"
import { RuntimeService } from "@/runtime/runtime-service"
import { RuntimeDriverFactory } from "@/runtime/runtime-driver-factory"
import { resolveTaskSessionID } from "@/runtime/session/stream-context"
import { Log } from "@/shared/util/log"
import { Token } from "@/shared/util/token"
import { CliBridge } from "@/runtime/integrations/cli-bridge"
import { extractSystemText } from "@/runtime/providers/adapters/loaders/system-text"
import { Server } from "@/runtime/server/server"
import { Instance } from "@/runtime/context/project/instance"

const log = Log.create({ service: "subprocess-lm" })

export class SubprocessLanguageModel implements LanguageModelV2 {
  readonly specificationVersion = "v2" as const
  readonly provider = "subprocess"
  readonly defaultObjectGenerationMode = undefined
  readonly supportedUrls: Record<string, RegExp[]> = {}

  readonly modelId: string
  private readonly providerID: string

  constructor(providerID: string, modelId: string) {
    this.providerID = providerID
    this.modelId = modelId
  }

  async doGenerate(options: any): Promise<any> {
    const chunks: string[] = []
    const result = await this.doStream(options)
    const reader = result.stream.getReader()
    while (true) {
      const { done, value } = await reader.read()
      if (done) break
      if (value.type === "text-delta") chunks.push((value as any).delta)
    }
    return {
      content: [{ type: "text", text: chunks.join("") }],
      finishReason: "stop" as const,
      usage: { inputTokens: 0, outputTokens: 0, totalTokens: 0 },
      rawCall: { rawPrompt: options.prompt, rawSettings: {} },
      rawResponse: {},
      warnings: [],
      response: { id: "subprocess", timestamp: new Date(), modelId: this.modelId },
    }
  }

  async doStream(options: any): Promise<{
    stream: ReadableStream<LanguageModelV2StreamPart>
    rawCall: { rawPrompt: unknown; rawSettings: Record<string, unknown> }
  }> {
    const message = extractLastUserText(options.prompt ?? [])
    if (!message) {
      return emptyStream(options.prompt)
    }

    const { runtime, driver } = await RuntimeDriverFactory.resolveCli(this.providerID)
    const sessionID = resolveTaskSessionID(options?.headers)
    const task = await driver.assign({
      taskId: generateTaskId(),
      prompt: message,
      // The session's instructions (mode contract, artifact-session note…)
      // go to the CLI as its system prompt; without them it only sees the
      // last user message.
      systemPrompt: extractSystemText(options.prompt ?? []) || undefined,
      // The CLI executes its own tools opaquely; the session id lets the
      // driver's ACP permission requests gate through the session's
      // PermissionNext policy instead of auto-approving.
      sessionID,
      // The CLI runs its own tools, so it starts in the session's folder
      // (its project's folder), not wherever gizzi was launched.
      cwd: currentWorkdir(),
      // gizzi's session tools (pane document, pane browser, media) over MCP.
      mcp: sessionID ? bridgeConfig(sessionID) : undefined,
    })

    const stream = new ReadableStream<LanguageModelV2StreamPart>({
      start: async (controller) => {
        controller.enqueue({ type: "stream-start", warnings: [] })

        let finished = false
        // What this turn streamed and the agent's last context report, for
        // estimating usage when the agent reports none.
        let emitted = ""
        let lastContext: { used: number; size: number } | undefined
        // One open block at a time. Text and reasoning each get a fresh id
        // whenever the stream switches kind or a tool call intervenes, so the
        // session stores prose → thinking → tool → prose as separate ordered
        // parts instead of one glued text part.
        let open: { kind: "text" | "reasoning"; id: string } | null = null
        let blockSeq = 0
        const closeOpen = () => {
          if (!open) return
          controller.enqueue({ type: open.kind === "text" ? "text-end" : "reasoning-end", id: open.id } as LanguageModelV2StreamPart)
          open = null
        }
        const ensureOpen = (kind: "text" | "reasoning") => {
          if (open?.kind === kind) return open.id
          closeOpen()
          open = { kind, id: `${kind}-${++blockSeq}` }
          controller.enqueue({ type: kind === "text" ? "text-start" : "reasoning-start", id: open.id } as LanguageModelV2StreamPart)
          return open.id
        }

        const finish = (reason: string, usage?: { inputTokens: number; outputTokens: number; totalTokens: number }) => {
          if (finished) return
          finished = true
          closeOpen()
          const reported = usage && usage.inputTokens + usage.outputTokens > 0 ? usage : undefined
          // No usage from the agent: estimate it (and say so) rather than
          // recording a zero that would break context upkeep and telemetry.
          const estimated = reported
            ? undefined
            : (() => {
                const inputTokens = lastContext?.used || Token.estimate(message)
                const outputTokens = Token.estimate(emitted)
                return { inputTokens, outputTokens, totalTokens: inputTokens + outputTokens }
              })()
          controller.enqueue({
            type: "finish",
            finishReason: reason as any,
            usage: reported ?? estimated!,
            ...(estimated ? { providerMetadata: { gizzi: { usageEstimated: true } } } : {}),
          } as LanguageModelV2StreamPart)
        }

        try {
          await RuntimeService.markBusy(runtime.id, true)

          for await (const event of driver.stream(task)) {
            if (event.type === "context") {
              lastContext = { used: event.used, size: event.size }
              controller.enqueue({
                type: "raw",
                raw: { __gizzi: "observed_context", used: event.used, size: event.size },
              } as unknown as LanguageModelV2StreamPart)
              continue
            }

            if (event.type === "text_delta") {
              emitted += event.delta
              const id = ensureOpen("text")
              controller.enqueue({ type: "text-delta", id, delta: event.delta })
              continue
            }

            if (event.type === "reasoning_delta") {
              emitted += event.delta
              const id = ensureOpen("reasoning")
              controller.enqueue({ type: "reasoning-delta", id, delta: event.delta } as LanguageModelV2StreamPart)
              continue
            }

            if (event.type === "error") {
              const err = event.error instanceof Error ? event.error : new Error(String(event.error))
              log.error("driver stream error", { error: err, taskId: task.taskId })
              controller.enqueue({ type: "error", error: err })
              finish("error")
              continue
            }

            if (event.type === "finish") {
              finish(event.finishReason, event.usage)
            }

            // Observed tool events (ACP / stream-json drivers): the CLI
            // executes these tools itself — gizzi must neither run them nor
            // ignore them. Forward them as raw model parts (the AI SDK's
            // sanctioned sideband, enabled via includeRawChunks) so the
            // session processor can publish tool parts on the event stream.
            if (event.type === "tool_call") {
              closeOpen()
              controller.enqueue({
                type: "raw",
                raw: {
                  __gizzi: "observed_tool_call",
                  id: event.id,
                  name: event.name,
                  arguments: event.arguments,
                },
              } as unknown as LanguageModelV2StreamPart)
              continue
            }

            if (event.type === "tool_result") {
              controller.enqueue({
                type: "raw",
                raw: {
                  __gizzi: "observed_tool_result",
                  id: event.id,
                  content: event.content,
                  isError: Boolean(event.isError),
                },
              } as unknown as LanguageModelV2StreamPart)
              continue
            }
          }

          if (!finished) {
            finish("stop")
          }
        } catch (err) {
          log.error("subprocess model stream failed", { error: err, taskId: task.taskId })
          controller.enqueue({ type: "error", error: err })
          finish("error")
        } finally {
          await RuntimeService.markBusy(runtime.id, false)
          controller.close()
        }
      },
    })

    return {
      stream,
      rawCall: { rawPrompt: message, rawSettings: { runtimeId: runtime.id, cliName: this.providerID } },
    }
  }
}

function emptyStream(rawPrompt: unknown) {
  return {
    stream: new ReadableStream<LanguageModelV2StreamPart>({
      start(c) {
        c.enqueue({ type: "stream-start", warnings: [] })
        c.enqueue({
          type: "finish",
          finishReason: "stop",
          usage: { inputTokens: 0, outputTokens: 0, totalTokens: 0 },
        })
        c.close()
      },
    }),
    rawCall: { rawPrompt, rawSettings: {} as Record<string, unknown> },
  }
}

function currentWorkdir(): string | undefined {
  try {
    return Instance.workdir
  } catch {
    return undefined
  }
}

function bridgeConfig(sessionID: string) {
  try {
    return CliBridge.serverConfig({ baseURL: Server.url(), sessionID, directory: Instance.directory })
  } catch (error) {
    log.warn("cli bridge unavailable", { error: error instanceof Error ? error.message : String(error) })
    return undefined
  }
}

function extractLastUserText(prompt: any[]): string {
  let last = ""
  for (const msg of prompt) {
    if (msg.role !== "user") continue
    const text = Array.isArray(msg.content)
      ? msg.content
          .filter((p: any) => p.type === "text")
          .map((p: any) => String(p.text ?? ""))
          .join("")
      : String(msg.content ?? "")
    if (text) last = text
  }
  return last
}

function generateTaskId(): string {
  return `task-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`
}

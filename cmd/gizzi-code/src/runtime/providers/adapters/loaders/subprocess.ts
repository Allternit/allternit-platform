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
import { VendorSession } from "@/runtime/session/vendor-session"
import { ContextBreakdown } from "@/runtime/session/context-breakdown"
import { ContextBreakdownStore } from "@/runtime/session/context-breakdown-store"
import { BrainContextReaders } from "@/runtime/session/brain-context-readers"
import { Instance } from "@/runtime/context/project/instance"
import * as BotTurn from "@/runtime/bots/bot-turn"
import { planInstructions } from "@/runtime/drivers/cli-session-flags"

const log = Log.create({ service: "subprocess-lm" })

/** gizzi's bridge tools as the CLI sees them over MCP (name, description, JSON schema), built once. */
let bridgeSchemas: Promise<Record<string, unknown>> | undefined
function bridgeToolSchemas() {
  bridgeSchemas ??= (async () => {
    const { z } = await import("zod/v4")
    const out: Record<string, unknown> = {}
    for (const info of CliBridge.TOOLS) {
      try {
        const tool = await info.init()
        out[`mcp__${CliBridge.SERVER_NAME}__${info.id}`] = {
          description: tool.description,
          inputSchema: z.toJSONSchema(tool.parameters as any),
        }
      } catch {
        // A tool that cannot describe itself without a session is left out.
      }
    }
    return out
  })()
  return bridgeSchemas
}

/** gizzi's system text as InstructionPrompt blocks (memory files, MCP instructions), for ContextBreakdown.build. */
function instructionBlocks(systemText: string) {
  return systemText.split(/\n(?=(?:Instructions from|Auto-memory from): |# MCP Server Instructions)/)
}

/**
 * Closed-backend CLIs' own prompt and built-in tools (tokens, chars/4),
 * measured from their leaked prompts (github.com/asgeirtj/system_prompts_leaks,
 * Oct 2026): Cursor/cursor.md, xAI/grok-4.7-cli.md, Google/antigravity-cli.md,
 * Misc/hermes.md.
 */
const CLI_OWN_PROMPT: Record<string, { prompt: number; tools: number }> = {
  "cursor-agent": { prompt: 2_882, tools: 1_645 },
  grok: { prompt: 4_562, tools: 14_859 },
  antigravity: { prompt: 6_122, tools: 0 },
  hermes: { prompt: 4_385, tools: 0 },
}

function addOwnPrompt(snapshot: ContextBreakdown.Snapshot, own: { prompt: number; tools: number }) {
  const parts = Object.fromEntries(
    snapshot.categories.filter((c) => !c.deferred && c.id !== "free_space" && c.id !== "autocompact_buffer").map((c) => [c.id, c.tokens]),
  ) as ContextBreakdown.Parts
  parts.system_prompt = (parts.system_prompt ?? 0) + own.prompt
  parts.system_tools = (parts.system_tools ?? 0) + own.tools
  return ContextBreakdown.fromParts({
    sessionID: snapshot.sessionID,
    providerID: snapshot.providerID,
    modelID: snapshot.modelID,
    contextWindow: snapshot.contextWindow,
    parts,
    memoryFiles: snapshot.memoryFiles,
    mcpTools: snapshot.mcpTools,
    systemTools: [...snapshot.systemTools, ...(own.tools > 0 ? [{ name: "built-in tools (estimated)", tokens: own.tools }] : [])],
    skills: snapshot.skills,
  })
}

async function modelWindow(providerID: string, modelID: string): Promise<number> {
  return import("@/runtime/providers/provider")
    .then(({ Provider }) => Provider.getModel(providerID, modelID))
    .then((model) => model.limit.context)
    .catch(() => 0)
}

/** A breakdown with a real split beyond Messages (Claude's /context, Codex's rollout, gizzi's sent rows). */
function hasSplit(snapshot: ContextBreakdown.Snapshot | undefined) {
  return Boolean(
    snapshot?.categories.some(
      (c) => c.id !== "messages" && c.id !== "free_space" && c.id !== "autocompact_buffer" && !c.deferred && c.tokens > 0,
    ),
  )
}

/**
 * The context total a CLI brain reported (ACP usage_update, Claude's result
 * modelUsage, Codex tokenUsage) as the session's breakdown, every row filled:
 * - a fuller report from the same brain (Claude's `/context`, Codex's
 *   rollout) keeps its split, only Messages moves;
 * - otherwise the rows are what gizzi itself sent the brain (instructions,
 *   memory files, MCP server instructions, its bridge tools), measured, and
 *   Messages holds the rest of the brain's total.
 */
async function recordBrainContext(input: {
  sessionID: string
  providerID: string
  modelID: string
  /** The brain's own report, when it sent one this turn. */
  context?: { used: number; size: number }
  systemText: string
  /** The conversation the brain holds (gizzi's copy of it), as model messages. */
  conversation: unknown[]
}) {
  const { sessionID, providerID, modelID, context } = input
  const previous = ContextBreakdown.get(sessionID)
  // A brain whose own record holds its full context: read that.
  if (BrainContextReaders.has(providerID)) {
    const vendorSessionId = await VendorSession.get(sessionID, providerID)
    const size = context?.size || previous?.contextWindow || (await modelWindow(providerID, modelID))
    const own = vendorSessionId && size > 0
      ? await BrainContextReaders.read({ sessionID, providerID, modelID, vendorSessionId, used: context?.used, size })
      : undefined
    if (own) {
      ContextBreakdownStore.record(own)
      return
    }
  }
  if (context && context.size > 0 && previous?.source === "brain" && previous.brain === providerID && hasSplit(previous)) {
    const next = ContextBreakdown.reconcile(previous, { providerContextTokens: context.used, contextWindow: context.size })
    ContextBreakdownStore.record({ ...next, measuredAt: Date.now() })
    return
  }
  // No report (or only a total): gizzi's measure of what it sent, the
  // conversation as the floor for Messages, the brain's total on top.
  const size = context?.size || (await modelWindow(providerID, modelID)) || previous?.contextWindow || 0
  if (!(size > 0)) return
  const sent = ContextBreakdown.build({
    sessionID,
    providerID,
    modelID,
    contextWindow: size,
    system: input.systemText ? [input.systemText] : [],
    instructions: input.systemText ? instructionBlocks(input.systemText) : [],
    tools: await bridgeToolSchemas().catch(() => ({})),
    messages: input.conversation as any,
    autoCompactBuffer: 0,
  })
  // The brain's own prompt and tools, which gizzi never sees: a measured
  // estimate from its leaked prompt where one exists (Cursor, Grok…).
  const own = CLI_OWN_PROMPT[providerID]
  const withOwn = own ? addOwnPrompt(sent, own) : sent
  const next = context ? ContextBreakdown.reconcile(withOwn, { providerContextTokens: context.used }) : withOwn
  ContextBreakdownStore.record({
    ...next,
    basis: context ? "provider" : "estimated",
    source: "brain",
    brain: providerID,
    split: "sent",
    ...(own ? { estimatedRows: ["system_prompt", "system_tools"] as ContextBreakdown.CategoryID[] } : {}),
    measuredAt: Date.now(),
  })
}

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
    const vendorSessionId = sessionID ? await VendorSession.get(sessionID, this.providerID) : undefined
    // A plan turn (bots never plan): the CLI runs read-only and hands the plan over through the bridge.
    const bot = BotTurn.get(sessionID)
    const planFile = sessionID && !bot ? await CliBridge.planFile(sessionID).catch(() => undefined) : undefined
    const plan = planFile ? { file: planFile } : undefined
    const systemText = extractSystemText(options.prompt ?? [])
    // A turn with gizzi's tools off (llm.ts marks it) is one of allternit-api's
    // internal completions: the user's text is data there, so the CLI must not
    // act on it or ask the user anything.
    const toolsOff = options?.headers?.["x-gizzi-tools-off"] === "1"
    const task = await driver.assign({
      taskId: generateTaskId(),
      prompt: message,
      model: this.modelId,
      // The session's instructions (mode contract, artifact-session note…)
      // go to the CLI as its system prompt; without them it only sees the
      // last user message.
      systemPrompt: [systemText, plan ? planInstructions(this.providerID, plan) : ""].filter(Boolean).join("\n\n") || undefined,
      // The CLI executes its own tools opaquely; the session id lets the
      // driver's ACP permission requests gate through the session's
      // PermissionNext policy instead of auto-approving.
      sessionID,
      vendorSessionId,
      // The CLI runs its own tools, so it starts in the session's folder
      // (its project's folder), not wherever gizzi was launched.
      cwd: currentWorkdir(),
      // gizzi's session tools (pane document, pane browser, media) over MCP.
      mcp: sessionID && !toolsOff ? bridgeConfig(sessionID) : undefined,
      toolsOff,
      // A bot turn: the CLI runs isolated from the user's own settings, with
      // the bot's tool allowlist (runtime/bots/bot-turn.ts).
      bot,
      plan,
    })

    const stream = new ReadableStream<LanguageModelV2StreamPart>({
      start: async (controller) => {
        controller.enqueue({ type: "stream-start", warnings: [] })

        let finished = false
        // The conversation the brain holds after this turn: gizzi's copy plus the reply.
        const conversation = () => [
          ...((options.prompt ?? []) as Array<{ role?: string }>).filter((m) => m?.role !== "system"),
          ...(emitted ? [{ role: "assistant", content: emitted }] : []),
        ]
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
          // A brain that never reported its context still gets every row.
          // A failed turn still sent its request: a record-reading brain still has its rows.
          const reader = BrainContextReaders.has(this.providerID)
          if (sessionID && (!lastContext || reader) && (reason !== "error" || reader)) {
            const args = { sessionID, providerID: this.providerID, modelID: this.modelId, context: lastContext, systemText, conversation: conversation() }
            // The brain's own record is written as the turn ends: read it a moment later.
            setTimeout(() => void recordBrainContext(args).catch(() => {}), BrainContextReaders.has(this.providerID) ? 750 : 0)
          }
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
              if (sessionID) {
                const context = { used: event.used, size: event.size }
                void recordBrainContext({ sessionID, providerID: this.providerID, modelID: this.modelId, context, systemText, conversation: conversation() }).catch(() => {})
              }
              controller.enqueue({
                type: "raw",
                raw: { __gizzi: "observed_context", used: event.used, size: event.size },
              } as unknown as LanguageModelV2StreamPart)
              continue
            }

            if (event.type === "vendor_session") {
              if (sessionID && event.id !== vendorSessionId) {
                await VendorSession.set(sessionID, this.providerID, event.id)
              }
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

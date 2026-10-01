import { MessageV2 } from "@/runtime/session/message-v2"
import { Log } from "@/shared/util/log"
import { Identifier } from "@/shared/id/id"
import { Session } from "@/runtime/session"
import { Agent } from "@/runtime/loop/agent"
import { Snapshot } from "@/runtime/session/snapshot"
import { SessionSummary } from "@/runtime/session/summary"
import { Bus } from "@/shared/bus"
import { SessionRetry } from "@/runtime/session/retry"
import { SessionStatus } from "@/runtime/session/status"
import { Plugin } from "@/runtime/integrations/plugin"
import { Provider } from "@/runtime/providers/provider"
import { LLM } from "@/runtime/session/llm"
import { Config } from "@/runtime/context/config/config"
import { SessionCompaction } from "@/runtime/session/compaction"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { Question } from "@/runtime/integrations/question"
import { SessionUsage } from "@/runtime/session/usage"
import { describeProviderError } from "@/shared/util/provider-error"
import { SessionTrace } from "@/runtime/session/trace"
import { ContextProjector } from "@/runtime/session/context-projector"
import { consumeRetryHint } from "@/runtime/providers/retry-hint"
import { SessionContext } from "./context-event"
import { Guardrails } from "./guardrails"

export namespace SessionProcessor {
  const DOOM_LOOP_THRESHOLD = 3
  const DEFAULT_RETRY_MAX_ATTEMPTS = 2
  const DEFAULT_RETRY_MAX_DELAY_MS = 10_000
  const log = Log.create({ service: "session.processor" })

  export type Info = Awaited<ReturnType<typeof create>>
  export type Result = Awaited<ReturnType<Info["process"]>>

  export function create(input: {
    assistantMessage: MessageV2.Assistant
    sessionID: string
    model: Provider.Model
    abort: AbortSignal
    fallbackModels?: { providerID: string; modelID: string }[]
    /** O12: the turn's guard. Tool calls and observations feed it; a trip ends the turn after this step. */
    guard?: Guardrails.TurnGuard
  }) {
    const toolcalls: Record<string, MessageV2.ToolPart> = {}
    const toolCallOrder: Record<string, number> = {}
    let nextToolCallIndex = 0
    let snapshot: string | undefined
    let blocked = false
    let attempt = 0
    let needsCompaction = false
    let mediaRecovery: "none" | "degraded" | "stripped" = "none"
    let guardTripped = false

    // A trip lets the in-flight step finish (its tool calls already started),
    // then process() returns "stop" so no further step runs.
    function onTrip(trip: Guardrails.Trip | undefined) {
      if (!trip || guardTripped) return
      guardTripped = true
      blocked = true
      reportTrip(input.sessionID, input.assistantMessage.id, trip)
    }

    const result = {
      get message() {
        return input.assistantMessage
      },
      partFromToolCall(toolCallID: string) {
        return toolcalls[toolCallID]
      },
      async process(streamInput: LLM.StreamInput) {
        log.info("process")
        needsCompaction = false
        // Gateway retry hints are single-shot: at most one hint-driven
        // re-drive per process() call, so a gateway that keeps emitting
        // retryable hints cannot loop the session.
        let retryHintConsumed = false
        // Drop any hint recorded by an earlier request in this session.
        consumeRetryHint(input.sessionID)
        const cfg = await Config.get()
        const shouldBreak = cfg.experimental?.continue_loop_on_deny !== true
        const retryMaxAttempts = cfg.experimental?.retry_max_attempts ?? DEFAULT_RETRY_MAX_ATTEMPTS
        const retryMaxDelayMs = cfg.experimental?.retry_max_delay_ms ?? DEFAULT_RETRY_MAX_DELAY_MS
        // Cross-provider failover chain. Per-request fallbackModels take precedence over the
        // global routing.fallbacks config; when neither is set the chain is empty and error
        // handling behaves exactly as before.
        const fallbackChain: { providerID: string; modelID: string }[] =
          input.fallbackModels ?? (cfg.routing?.fallbacks ?? []).map((entry: string) => Provider.parseModel(entry))
        const tried = new Set<string>([`${input.model.providerID}/${input.model.id}`])
        let fallbackIndex = 0
        const rotationState = Provider.createRotationState()
        // Provider error categories that are never worth retrying on the same model (mirrors
        // the non-retryable classification in shared/util/provider-error.ts / SessionRetry).
        const nonRetryableFallbackReason = (error: ReturnType<typeof MessageV2.fromError>): string | undefined => {
          if (MessageV2.AuthError.isInstance(error)) return "auth"
          if (!MessageV2.APIError.isInstance(error)) return undefined
          const data = (error as { data?: { message?: string } }).data
          const info = describeProviderError({ raw: data?.message || "" })
          if (["auth", "insufficient_balance", "unsupported_model", "context_overflow"].includes(info.code)) {
            return info.code
          }
          return undefined
        }
        const tryRotateAuth = async (): Promise<boolean> => {
          if (streamInput.plan?.profileId) {
            rotationState.triedProfileIds.add(streamInput.plan.profileId)
          }
          const rotated = await Provider.rotateAuth(
            { providerID: input.model.providerID, modelID: input.model.id },
            rotationState,
          )
          if (!rotated) return false
          streamInput.plan = rotated
          attempt = 0
          log.info("rotating auth profile", {
            sessionID: input.sessionID,
            providerID: input.model.providerID,
            modelID: input.model.id,
            profileId: rotated.profileId,
          })
          return true
        }
        const switchToFallbackModel = async (reason: string): Promise<boolean> => {
          if (input.abort.aborted) return false
          while (fallbackIndex < fallbackChain.length) {
            const candidate = fallbackChain[fallbackIndex++]
            const key = `${candidate.providerID}/${candidate.modelID}`
            if (tried.has(key)) continue
            tried.add(key)
            const next = await Provider.getModel(candidate.providerID, candidate.modelID).catch((e: unknown) => {
              log.warn("fallback model unavailable, skipping", {
                providerID: candidate.providerID,
                modelID: candidate.modelID,
                error: e,
              })
              return undefined
            })
            if (!next) continue
            tried.add(`${next.providerID}/${next.id}`)
            const from = { providerID: input.model.providerID, modelID: input.model.id }
            const to = { providerID: next.providerID, modelID: next.id }
            log.info("switching to fallback model", { sessionID: input.sessionID, from, to, reason })
            Bus.publish(Session.Event.ModelFallback, {
              sessionID: input.sessionID,
              from,
              to,
              reason,
            })
            input.model = next
            streamInput.model = next
            streamInput.plan = await Provider.prepareAuth({ providerID: next.providerID, modelID: next.id })
            input.assistantMessage.providerID = next.providerID
            input.assistantMessage.modelID = next.id
            await Session.updateMessage(input.assistantMessage)
            // Each model in the chain gets the same configured retry attempts as the primary.
            attempt = 0
            SessionStatus.set(input.sessionID, { type: "busy" })
            return true
          }
          return false
        }
        while (true) {
          try {
            let currentText: MessageV2.TextPart | undefined
            let currentReasoning: MessageV2.ReasoningPart | undefined
            let reasoningMap: Record<string, MessageV2.ReasoningPart> = {}
            const stream = await LLM.stream(streamInput)

            // State machine for splitting <think> tags
            let mode: "text" | "thinking" = "text"
            let tagBuffer = ""

            for await (const value of stream.fullStream) {
              input.abort.throwIfAborted()
              switch (value.type) {
                case "start":
                  SessionStatus.set(input.sessionID, { type: "busy" })
                  break

                case "reasoning-start":
                  if (value.id in reasoningMap) {
                    continue
                  }
                  const reasoningPart = {
                    id: Identifier.ascending("part"),
                    messageID: input.assistantMessage.id,
                    sessionID: input.assistantMessage.sessionID,
                    type: "reasoning" as const,
                    text: "",
                    time: {
                      start: Date.now(),
                    },
                    metadata: value.providerMetadata,
                  }
                  reasoningMap[value.id] = reasoningPart
                  await Session.updatePart(reasoningPart)

                  // FORCE IMMEDIATE BUS PUBLISH
                  Bus.publish(MessageV2.Event.PartUpdated, {
                    part: reasoningPart,
                  })
                  break

                case "reasoning-delta":
                  if (value.id in reasoningMap) {
                    const part = reasoningMap[value.id]
                    part.text += value.text
                    if (value.providerMetadata) part.metadata = value.providerMetadata
                    await Session.updatePartDelta({
                      sessionID: part.sessionID,
                      messageID: part.messageID,
                      partID: part.id,
                      field: "text",
                      delta: value.text,
                    })
                  }
                  break

                case "reasoning-end":
                  if (value.id in reasoningMap) {
                    const part = reasoningMap[value.id]
                    part.text = part.text.trimEnd()

                    part.time = {
                      ...part.time,
                      end: Date.now(),
                    }
                    if (value.providerMetadata) part.metadata = value.providerMetadata
                    await Session.updatePart(part)
                    
                    Bus.publish(MessageV2.Event.PartUpdated, {
                      part,
                    })
                    
                    delete reasoningMap[value.id]
                  }
                  break

                case "tool-input-start": {
                  const part = (await Session.updatePart({
                    id: toolcalls[value.id]?.id ?? Identifier.ascending("part"),
                    messageID: input.assistantMessage.id,
                    sessionID: input.assistantMessage.sessionID,
                    type: "tool",
                    tool: value.toolName,
                    callID: value.id,
                    state: {
                      status: "pending",
                      input: {},
                      raw: "",
                    },
                  })) as MessageV2.ToolPart
                  toolcalls[value.id] = part
                  if (!(value.id in toolCallOrder)) {
                    toolCallOrder[value.id] = nextToolCallIndex++
                  }

                  Bus.publish(MessageV2.Event.PartUpdated, {
                    part,
                  })
                  break
                }

                case "tool-input-delta": {
                  const match = toolcalls[value.id]
                  if (match) {
                    if (!(value.id in toolCallOrder)) {
                      toolCallOrder[value.id] = nextToolCallIndex++
                    }
                    const index = toolCallOrder[value.id]
                    await Session.updatePartDelta({
                      sessionID: match.sessionID,
                      messageID: match.messageID,
                      partID: match.id,
                      field: "tool_calls",
                      delta: JSON.stringify({
                        index,
                        id: value.id,
                        type: "function",
                        function: {
                          name: match.tool,
                          arguments: value.delta,
                        },
                      }),
                    })
                  }
                  break
                }

                case "tool-input-end":
                  break

                case "tool-call": {
                  const match = toolcalls[value.toolCallId]
                  if (match) {
                    const part = (await Session.updatePart({
                      ...match,
                      tool: value.toolName,
                      state: {
                        status: "running",
                        input: value.input,
                        time: {
                          start: Date.now(),
                        },
                      },
                      metadata: value.providerMetadata,
                    })) as MessageV2.ToolPart
                    toolcalls[value.toolCallId] = part

                    Bus.publish(MessageV2.Event.PartUpdated, {
                      part,
                    })
                    onTrip(input.guard?.recordToolCall(value.toolName))

                    const parts = await MessageV2.parts(input.assistantMessage.id)
                    const lastThree = parts.slice(-DOOM_LOOP_THRESHOLD)

                    if (
                      lastThree.length === DOOM_LOOP_THRESHOLD &&
                      lastThree.every(
                        (p) =>
                          p.type === "tool" &&
                          p.tool === value.toolName &&
                          p.state.status !== "pending" &&
                          JSON.stringify(p.state.input) === JSON.stringify(value.input),
                      )
                    ) {
                      const agent = await Agent.get(input.assistantMessage.agent)
                      await PermissionNext.ask({
                        permission: "doom_loop",
                        patterns: [value.toolName],
                        sessionID: input.assistantMessage.sessionID,
                        metadata: {
                          tool: value.toolName,
                          input: value.input,
                        },
                        always: [value.toolName],
                        ruleset: agent.permission,
                      })
                    }
                  }
                  break
                }
                case "tool-result": {
                  const match = toolcalls[value.toolCallId]
                  if (match && match.state.status === "running") {
                    const part = (await Session.updatePart({
                      ...match,
                      state: {
                        status: "completed",
                        input: value.input ?? match.state.input,
                        output: value.output.output,
                        metadata: value.output.metadata,
                        title: value.output.title,
                        time: {
                          start: match.state.time.start,
                          end: Date.now(),
                        },
                        attachments: value.output.attachments,
                      },
                    })) as MessageV2.ToolPart

                    Bus.publish(MessageV2.Event.PartUpdated, {
                      part,
                    })
                    onTrip(
                      input.guard?.recordObservation({
                        tool: match.tool,
                        input: part.state.input,
                        output: value.output.output,
                      }),
                    )

                    delete toolcalls[value.toolCallId]
                  }
                  break
                }

                case "tool-error": {
                  const match = toolcalls[value.toolCallId]
                  if (match && match.state.status === "running") {
                    // Tool.define (tools/builtins/tool.ts) tags zod validation errors with these fields.
                    const validationError = value.error as any
                    const validation = validationError?.validationError
                      ? {
                          validationError: true,
                          attempt: validationError.validationAttempt,
                          issues: validationError.validationIssues,
                        }
                      : undefined
                    const part = (await Session.updatePart({
                      ...match,
                      state: {
                        status: "error",
                        input: value.input ?? match.state.input,
                        error: (value.error as any).toString(),
                        ...(validation ? { metadata: validation } : {}),
                        time: {
                          start: match.state.time.start,
                          end: Date.now(),
                        },
                      },
                    })) as MessageV2.ToolPart

                    Bus.publish(MessageV2.Event.PartUpdated, {
                      part,
                    })

                    if (
                      value.error instanceof PermissionNext.RejectedError ||
                      value.error instanceof Question.RejectedError
                    ) {
                      blocked = shouldBreak
                    } else {
                      onTrip(
                        input.guard?.recordObservation({
                          tool: match.tool,
                          input: part.state.input,
                          error: String(value.error),
                        }),
                      )
                    }
                    delete toolcalls[value.toolCallId]
                  }
                  break
                }
                case "error":
                  throw value.error

                case "start-step":
                  snapshot = await Snapshot.track()
                  await Session.updatePart({
                    id: Identifier.ascending("part"),
                    messageID: input.assistantMessage.id,
                    sessionID: input.sessionID,
                    snapshot,
                    type: "step-start",
                  })
                  break

                case "finish-step":
                  const usage = Session.getUsage({
                    model: input.model,
                    usage: value.usage,
                    metadata: value.providerMetadata,
                  })
                  if ((value.providerMetadata as any)?.gizzi?.usageEstimated) {
                    input.assistantMessage.tokensEstimated = true
                    Bus.publish(SessionContext.Event.Updated, {
                      sessionID: input.sessionID,
                      messageID: input.assistantMessage.id,
                      basis: "estimated",
                      usageEstimated: true,
                    })
                  }
                  input.assistantMessage.finish = value.finishReason
                  input.assistantMessage.cost += usage.cost
                  input.assistantMessage.tokens = usage.tokens
                  await Session.updatePart({
                    id: Identifier.ascending("part"),
                    reason: value.finishReason ?? "unknown",
                    snapshot: await Snapshot.track(),
                    messageID: input.assistantMessage.id,
                    sessionID: input.assistantMessage.sessionID,
                    type: "step-finish",
                    tokens: usage.tokens,
                    cost: usage.cost,
                  })
                  await Session.updateMessage(input.assistantMessage)
                  // Record usage for analytics
                  await SessionUsage.record({
                    sessionID: input.sessionID,
                    messageID: input.assistantMessage.id,
                    providerID: input.model.providerID,
                    modelID: input.model.id,
                    tokens: usage.tokens,
                    cost: usage.cost,
                    callType: streamInput.callType ?? "answer",
                  })
                  if (snapshot) {
                    const patch = await Snapshot.patch(snapshot)
                    if (patch.files.length) {
                      await Session.updatePart({
                        id: Identifier.ascending("part"),
                        messageID: input.assistantMessage.id,
                        sessionID: input.sessionID,
                        type: "patch",
                        hash: patch.hash,
                        files: patch.files,
                      })
                    }
                    snapshot = undefined
                  }
                  SessionSummary.summarize({
                    sessionID: input.sessionID,
                    messageID: input.assistantMessage.parentID,
                  })
                  if (await SessionCompaction.isOverflow({ tokens: usage.tokens, model: input.model })) {
                    needsCompaction = true
                  }
                  break

                case "text-start":
                  // Reset state machine for new block
                  mode = "text"
                  tagBuffer = ""
                  break

                case "text-delta": {
                  const text = value.text
                  tagBuffer += text

                  while (tagBuffer.length > 0) {
                    if (mode === "text") {
                      const openIndex = tagBuffer.indexOf("<think>")
                      if (openIndex === -1) {
                        // No full tag, but watch out for partial tag at the end
                        const potentialPrefix = "<think>"
                        let matchLen = 0
                        for (let len = potentialPrefix.length - 1; len > 0; len--) {
                          if (tagBuffer.endsWith(potentialPrefix.slice(0, len))) {
                            matchLen = len
                            break
                          }
                        }

                        // Emit all text except the potential partial tag at the end
                        const safeText = tagBuffer.slice(0, tagBuffer.length - matchLen)
                        if (safeText) {
                          if (!currentText) {
                            currentText = (await Session.updatePart({
                              id: Identifier.ascending("part"),
                              messageID: input.assistantMessage.id,
                              sessionID: input.assistantMessage.sessionID,
                              type: "text",
                              text: "",
                              time: { start: Date.now() },
                            })) as MessageV2.TextPart
                            // Declare the part now: bridges hold deltas for undeclared parts,
                            // so without this the whole block arrived at once at the end.
                            Bus.publish(MessageV2.Event.PartUpdated, { part: currentText })
                          }
                          currentText.text += safeText
                          await Session.updatePartDelta({
                            sessionID: currentText.sessionID,
                            messageID: currentText.messageID,
                            partID: currentText.id,
                            field: "text",
                            delta: safeText,
                          })

                          tagBuffer = tagBuffer.slice(safeText.length)
                        }
                        break // Wait for more data to complete potential tag
                      } else {
                        // Tag found
                        const before = tagBuffer.slice(0, openIndex)
                        if (before) {
                          if (!currentText) {
                            currentText = (await Session.updatePart({
                              id: Identifier.ascending("part"),
                              messageID: input.assistantMessage.id,
                              sessionID: input.assistantMessage.sessionID,
                              type: "text",
                              text: "",
                              time: { start: Date.now() },
                            })) as MessageV2.TextPart
                            // Declare the part now: bridges hold deltas for undeclared parts,
                            // so without this the whole block arrived at once at the end.
                            Bus.publish(MessageV2.Event.PartUpdated, { part: currentText })
                          }
                          currentText.text += before
                          await Session.updatePartDelta({
                            sessionID: currentText.sessionID,
                            messageID: currentText.messageID,
                            partID: currentText.id,
                            field: "text",
                            delta: before,
                          })
                        }
                        
                        // Transition to thinking
                        if (currentText && currentText.time) {
                          await Session.updatePart({ ...currentText, time: { start: currentText.time.start, end: Date.now() } })
                          currentText = undefined
                        }
                        mode = "thinking"
                        currentReasoning = (await Session.updatePart({
                          id: Identifier.ascending("part"),
                          messageID: input.assistantMessage.id,
                          sessionID: input.assistantMessage.sessionID,
                          type: "reasoning",
                          text: "",
                          time: { start: Date.now() },
                        })) as MessageV2.ReasoningPart
                        
                        // Emit PartUpdated for the new reasoning part
                        Bus.publish(MessageV2.Event.PartUpdated, {
                          part: currentReasoning,
                        })

                        tagBuffer = tagBuffer.slice(openIndex + "<think>".length)
                      }
                    } else {
                      // mode === "thinking"
                      const closeIndex = tagBuffer.indexOf("</think>")
                      if (closeIndex === -1) {
                        // ...
                      } else {
                        // Tag found
                        const before = tagBuffer.slice(0, closeIndex)
                        if (before) {
                          if (!currentReasoning) {
                            currentReasoning = (await Session.updatePart({
                              id: Identifier.ascending("part"),
                              messageID: input.assistantMessage.id,
                              sessionID: input.assistantMessage.sessionID,
                              type: "reasoning",
                              text: "",
                              time: { start: Date.now() },
                            })) as MessageV2.ReasoningPart
                            // Declare the part now: bridges hold deltas for undeclared parts,
                            // so without this the whole block arrived at once at the end.
                            Bus.publish(MessageV2.Event.PartUpdated, { part: currentReasoning })
                          }
                          currentReasoning.text += before
                          await Session.updatePartDelta({
                            sessionID: currentReasoning.sessionID,
                            messageID: currentReasoning.messageID,
                            partID: currentReasoning.id,
                            field: "text",
                            delta: before,
                          })
                        }
                        
                        // Transition
                        if (currentReasoning) {
                          await Session.updatePart({ ...currentReasoning, time: { start: currentReasoning.time.start, end: Date.now() } })
                          currentReasoning = undefined
                        }
                        mode = "text"

                        // Start a new text part immediately after thinking
                        const newTextPart = (await Session.updatePart({
                          id: Identifier.ascending("part"),
                          messageID: input.assistantMessage.id,
                          sessionID: input.assistantMessage.sessionID,
                          type: "text",
                          text: "",
                          time: { start: Date.now() },
                        })) as MessageV2.TextPart
                        currentText = newTextPart
                        
                        Bus.publish(MessageV2.Event.PartUpdated, {
                          part: currentText,
                        })

                        tagBuffer = tagBuffer.slice(closeIndex + "</think>".length)
                      }
                    }
                  }
                  break
                }

                case "text-end":
                  if (currentText) {
                    currentText.text = currentText.text.trimEnd()
                    const textOutput = await Plugin.trigger(
                      "experimental.text.complete",
                      {
                        sessionID: input.sessionID,
                        messageID: input.assistantMessage.id,
                        partID: currentText.id,
                      },
                      { text: currentText.text },
                    )
                    currentText.text = textOutput.text
                    currentText.time = {
                      start: Date.now(),
                      end: Date.now(),
                    }
                    if (value.providerMetadata) currentText.metadata = value.providerMetadata
                    await Session.updatePart(currentText)

                    Bus.publish(MessageV2.Event.PartUpdated, {
                      part: currentText,
                    })
                  }
                  if (currentReasoning) {
                    await Session.updatePart({ ...currentReasoning, time: { ...currentReasoning.time, end: Date.now() } })
                  }
                  currentText = undefined
                  currentReasoning = undefined
                  break

                case "finish":
                  break

                case "raw": {
                  // Subprocess providers (kimi-cli ACP et al.) execute tools
                  // inside the CLI; the loader forwards them as raw parts.
                  // Publish them as ordinary session tool parts so surfaces
                  // (event stream, DAG bridge) see tool execution exactly
                  // like SDK-executed tools — with no execution on our side.
                  const raw = (value as { raw?: unknown }).raw
                  if (!raw || typeof raw !== "object") break
                  const observed = raw as Record<string, unknown>
                  if (observed.__gizzi === "observed_context") {
                    Bus.publish(SessionContext.Event.Updated, {
                      sessionID: input.sessionID,
                      messageID: input.assistantMessage.id,
                      used: Number(observed.used) || 0,
                      window: Number(observed.size) || undefined,
                      basis: "provider",
                    })
                    break
                  }
                  if (observed.__gizzi === "observed_tool_call") {
                    const id = String(observed.id ?? "")
                    if (!id || toolcalls[id]) break
                    const part = (await Session.updatePart({
                      id: Identifier.ascending("part"),
                      messageID: input.assistantMessage.id,
                      sessionID: input.assistantMessage.sessionID,
                      type: "tool",
                      tool: String(observed.name ?? "tool"),
                      callID: id,
                      // The CLI is already executing the call — "running" is
                      // the honest state, and it is what the DAG bridge keys
                      // its tool-job rows on.
                      state: {
                        status: "running",
                        input: (observed.arguments ?? {}) as Record<string, unknown>,
                        time: { start: Date.now() },
                      },
                    })) as MessageV2.ToolPart
                    toolcalls[id] = part
                    if (!(id in toolCallOrder)) {
                      toolCallOrder[id] = nextToolCallIndex++
                    }
                    Bus.publish(MessageV2.Event.PartUpdated, { part })
                    onTrip(input.guard?.recordToolCall(part.tool))
                    break
                  }
                  if (observed.__gizzi === "observed_tool_result") {
                    const id = String(observed.id ?? "")
                    const match = id ? toolcalls[id] : undefined
                    if (!match || match.state.status !== "running") break
                    const isError = Boolean(observed.isError)
                    const part = (await Session.updatePart({
                      ...match,
                      state: isError
                        ? {
                            status: "error",
                            input: match.state.input,
                            error: String(observed.content ?? "tool call failed"),
                            time: {
                              start: match.state.time.start,
                              end: Date.now(),
                            },
                          }
                        : {
                            status: "completed",
                            input: match.state.input,
                            output: String(observed.content ?? ""),
                            title: match.tool,
                            metadata: {},
                            time: {
                              start: match.state.time.start,
                              end: Date.now(),
                            },
                          },
                    })) as MessageV2.ToolPart
                    Bus.publish(MessageV2.Event.PartUpdated, { part })
                    onTrip(
                      input.guard?.recordObservation({
                        tool: match.tool,
                        input: match.state.input,
                        ...(isError
                          ? { error: String(observed.content ?? "tool call failed") }
                          : { output: String(observed.content ?? "") }),
                      }),
                    )
                    delete toolcalls[id]
                    break
                  }
                  break
                }

                default:
                  log.info("unhandled", {
                    ...value,
                  })
                  continue
              }
              if (needsCompaction) break
            }
          } catch (e: any) {
            log.error("process", {
              error: e,
              stack: JSON.stringify(e.stack),
            })
            const rawError = e instanceof Error ? e.message : String(e)
            // Allternit gateway streaming failover: a retryable
            // `allternit.retry_hint` SSE event preceded this stream error.
            // Re-drive the request once (full context is already in
            // streamInput.messages), routed at the hint's next_fallback via
            // the gateway's `provider/model` body id when present.
            const retryHint = consumeRetryHint(input.sessionID)
            if (retryHint?.retryable && !retryHintConsumed && !input.abort.aborted) {
              retryHintConsumed = true
              if (retryHint.next_fallback) {
                streamInput.model = {
                  ...input.model,
                  id: `${input.model.id}#retry-hint`,
                  api: {
                    ...input.model.api,
                    id: `${retryHint.next_fallback.provider_id}/${retryHint.next_fallback.model_id}`,
                  },
                }
              }
              log.info("gateway retry hint: re-driving request", {
                sessionID: input.sessionID,
                reason: retryHint.reason,
                next_fallback: retryHint.next_fallback,
              })
              SessionTrace.append({
                sessionID: input.sessionID,
                kind: "request.retry_hint",
                messageID: input.assistantMessage.id,
                data: {
                  requestID: streamInput.user.id,
                  reason: retryHint.reason,
                  next_fallback: retryHint.next_fallback ?? undefined,
                  error: rawError,
                },
              })
              SessionStatus.set(input.sessionID, { type: "busy" })
              continue
            }
            if (isPayloadTooLarge(e) && mediaRecovery !== "stripped") {
              streamInput.messages = mediaRecovery === "none"
                ? ContextProjector.degradeOlderMedia(streamInput.messages)
                : ContextProjector.stripMedia(streamInput.messages)
              mediaRecovery = mediaRecovery === "none" ? "degraded" : "stripped"
              SessionTrace.append({
                sessionID: input.sessionID,
                kind: "request.failed",
                messageID: input.assistantMessage.id,
                data: { requestID: streamInput.user.id, recovery: mediaRecovery, reason: "payload_too_large", error: rawError },
              })
              continue
            }
            if (isUnsupportedMedia(e) && mediaRecovery !== "stripped") {
              streamInput.messages = ContextProjector.stripMedia(streamInput.messages)
              mediaRecovery = "stripped"
              SessionTrace.append({
                sessionID: input.sessionID,
                kind: "request.failed",
                messageID: input.assistantMessage.id,
                data: { requestID: streamInput.user.id, recovery: mediaRecovery, reason: "unsupported_media", error: rawError },
              })
              continue
            }
            // fromError always serializes via toObject(): {name, message, data}
            const error = MessageV2.fromError(e, {
              providerID: input.model.providerID,
            }) as { name: string; message: string; data?: any }
            if (MessageV2.ContextOverflowError.isInstance(error)) {
              log.warn("context overflow detected, triggering compaction", {
                sessionID: input.sessionID,
              })
              needsCompaction = true
              input.assistantMessage.error = error
              Bus.publish(Session.Event.Error, {
                sessionID: input.assistantMessage.sessionID,
                error,
              })
              SessionStatus.set(input.sessionID, { type: "idle" })
              break
            }
            const retry = SessionRetry.retryable(error)
            if (retry !== undefined) {
              if (attempt >= retryMaxAttempts) {
                // Same-model retries exhausted: try the next auth profile, then the next
                // fallback model when a chain is configured instead of failing the session.
                if (await tryRotateAuth()) continue
                if (
                  await switchToFallbackModel(
                    `${retry} (retry limit reached after ${attempt} attempt${attempt === 1 ? "" : "s"})`,
                  )
                )
                  continue
                log.warn("retry limit reached", {
                  attempt,
                  retryMaxAttempts,
                  retry,
                })
                input.assistantMessage.error =
                  MessageV2.APIError.isInstance(error)
                    ? new MessageV2.APIError({
                        ...error.data,
                        isRetryable: false,
                        message: `${retry} (retry limit reached after ${attempt} attempt${
                          attempt === 1 ? "" : "s"
                        })`,
                      }).toObject()
                    : error
                Bus.publish(Session.Event.Error, {
                  sessionID: input.assistantMessage.sessionID,
                  error: input.assistantMessage.error,
                })
                SessionStatus.set(input.sessionID, { type: "idle" })
              } else {
                attempt++
                const delay = Math.min(
                  SessionRetry.delay(attempt, error.name === "APIError" ? error : undefined),
                  retryMaxDelayMs,
                )
                SessionStatus.set(input.sessionID, {
                  type: "retry",
                  attempt,
                  message: retry as string,
                  next: Date.now() + delay,
                })
                await SessionRetry.sleep(delay, input.abort).catch(() => {})
                continue
              }
            } else {
              // Non-retryable provider errors (auth, insufficient balance, unsupported model,
              // context overflow) skip straight to the next fallback model when a chain exists.
              // Auth errors first try the next credential/profile for the same provider/model.
              const reason = nonRetryableFallbackReason(error)
              if (reason === "auth" && (await tryRotateAuth())) continue
              if (reason !== undefined && (await switchToFallbackModel(reason))) continue
            }
            if (!input.assistantMessage.error) {
              input.assistantMessage.error = error
            }
            Bus.publish(Session.Event.Error, {
              sessionID: input.assistantMessage.sessionID,
              error: input.assistantMessage.error,
            })
            SessionStatus.set(input.sessionID, { type: "idle" })
          }
          if (snapshot) {
            const patch = await Snapshot.patch(snapshot)
            if (patch.files.length) {
              await Session.updatePart({
                id: Identifier.ascending("part"),
                messageID: input.assistantMessage.id,
                sessionID: input.sessionID,
                type: "patch",
                hash: patch.hash,
                files: patch.files,
              })
            }
            snapshot = undefined
          }
          const p = await MessageV2.parts(input.assistantMessage.id)
          for (const part of p) {
            if (part.type === "tool" && part.state.status !== "completed" && part.state.status !== "error") {
              await Session.updatePart({
                ...part,
                state: {
                  ...part.state,
                  status: "error",
                  error: "Tool execution aborted",
                  time: {
                    start: Date.now(),
                    end: Date.now(),
                  },
                },
              })
            }
          }
          input.assistantMessage.time.completed = Date.now()
          await Session.updateMessage(input.assistantMessage)
          SessionTrace.append({
            sessionID: input.sessionID,
            kind: input.assistantMessage.error ? "request.failed" : "request.completed",
            messageID: input.assistantMessage.id,
            data: input.assistantMessage.error
              ? {
                  requestID: streamInput.user.id,
                  providerID: input.model.providerID,
                  modelID: input.model.id,
                  attempt,
                  error: input.assistantMessage.error,
                }
              : {
                  requestID: streamInput.user.id,
                  providerID: input.model.providerID,
                  modelID: input.model.id,
                  attempt,
                  finish: input.assistantMessage.finish,
                  tokens: input.assistantMessage.tokens,
                  cost: input.assistantMessage.cost,
                },
          })
          if (needsCompaction) return "compact"
          if (blocked) return "stop"
          if (input.assistantMessage.error) return "stop"
          return "continue"
        }
      },
    }
    return result
  }

  /** The clear reason event for a guardrail trip: bus event (UI/attention), trace row, log. */
  export function reportTrip(sessionID: string, messageID: string | undefined, trip: Guardrails.Trip) {
    log.warn("guardrail tripped", { sessionID, messageID, ...trip })
    Bus.publish(Guardrails.Event.Tripped, { sessionID, messageID, trip })
    SessionTrace.append({ sessionID, kind: "guardrail.tripped", messageID, data: trip })
  }

  function isPayloadTooLarge(error: unknown) {
    const value = error as { status?: number; statusCode?: number; message?: string; cause?: unknown }
    return value?.status === 413 || value?.statusCode === 413 || /(?:413|payload|request body).*(?:too large|limit)|entity too large/i.test(value?.message ?? String(error))
  }

  function isUnsupportedMedia(error: unknown) {
    const message = (error as { message?: string })?.message ?? String(error)
    return /unsupported (?:image|media)|invalid image|image format|media type.*not supported/i.test(message)
  }
}

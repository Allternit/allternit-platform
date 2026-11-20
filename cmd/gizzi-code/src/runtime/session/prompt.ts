import { runWithStreamContext } from "@/runtime/session/stream-context"
import { isRelativeUrl, resolveUploadUrl } from "@/runtime/session/upload-part"
import { matchReviewPreset, reviewPresetPrompt } from "@/shared/util/review-presets"
import path from "path"
import os from "os"
import fs from "fs/promises"
import { statSync } from "fs"
import z from "zod/v4"
import { Filesystem } from "@/shared/util/filesystem"
import { Identifier } from "@/shared/id/id"
import { MessageV2 } from "@/runtime/session/message-v2"
import { Log } from "@/shared/util/log"
import { SessionRevert } from "@/runtime/session/revert"
import { Session } from "@/runtime/session"
import { Agent } from "@/runtime/loop/agent"

import { Provider } from "@/runtime/providers/provider"
import { CLOUD_PROVIDER_ID, consumeHandoff, handoffTarget, isRefusal } from "@/runtime/providers/handoff"
import { type Tool as AITool, tool, jsonSchema, type ToolCallOptions, asSchema } from "ai"
import { ToolDispatcher } from "@/runtime/tools/dispatch"
import { RemoteToolHost } from "@/runtime/tools/remote-host"
import { ToolDedupe } from "@/runtime/tools/dedupe"
import { ToolFamilies } from "@/runtime/primitives/tool-families"
import { ContextNotice } from "@/runtime/primitives/context-notice"
import { SteerSignal } from "@/runtime/primitives/steer-signal"
import { SessionCompaction } from "@/runtime/session/compaction"
import { Instance } from "@/runtime/context/project/instance"
import { SessionWorktree } from "@/runtime/primitives/worktree"
import { Bus } from "@/shared/bus"
import { ProviderTransform } from "@/runtime/providers/adapters/transform"
import { SystemPrompt } from "@/runtime/session/system"
import { Clock } from "@/runtime/primitives/clock"
import { InstructionPrompt } from "@/runtime/session/instruction"
import { Plugin } from "@/runtime/integrations/plugin"
import PROMPT_PLAN from "@/runtime/session/prompt/plan.txt"
import BUILD_SWITCH from "@/runtime/session/prompt/build-switch.txt"
import MAX_STEPS from "@/runtime/session/prompt/max-steps.txt"
import WRAP_UP from "@/runtime/session/prompt/wrap-up.txt"
import { defer } from "@/shared/util/defer"
import { ToolRegistry } from "@/runtime/tools/builtins/registry"
import { MCP } from "@/runtime/tools/mcp"
import { mcpAppMetadata } from "@/runtime/tools/mcp/apps"
import { McpUserProxy } from "@/runtime/tools/mcp/user-proxy"
import { Connectors } from "@/runtime/primitives/connectors"
import type { CallToolResult } from "@modelcontextprotocol/client"
import { LSP } from "@/runtime/integrations/lsp"
import { ReadTool } from "@/runtime/tools/builtins/read"
import { FileTime } from "@/shared/file/time"
import { Flag } from "@/runtime/context/flag/flag"
import { Config } from "@/runtime/context/config/config"
import { ulid } from "ulid"
import { spawn } from "child_process"
import { ProcessRegistry } from "@/runtime/process-registry"
import { Command } from "@/runtime/loop/command"
import { $, fileURLToPath, pathToFileURL } from "bun"
import { ConfigMarkdown } from "@/runtime/context/config/markdown"
import { SessionSummary } from "@/runtime/session/summary"
import { NamedError } from "@allternit/gizzi-util/error.js"
import { fn } from "@/shared/util/fn"
import { AnswerTurnGuard } from "@/runtime/session/answer-policy"
import { SessionProcessor } from "@/runtime/session/processor"
import { Guardrails } from "@/runtime/session/guardrails"
import { TaskTool } from "@/runtime/tools/builtins/task"
import { TurnLocations, resolveTurnLocation, type TurnLocation } from "@/runtime/tools/builtins/lookup"
import { Tool } from "@/runtime/tools/builtins/tool"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { SessionStatus } from "@/runtime/session/status"
import { SecretRedactor } from "@/runtime/session/environment/redactor"
import { EnvironmentRuntime } from "@/runtime/session/environment/runtime-state"
import { GoalEngine } from "@/runtime/automation/goal-engine"
import { KernelTurn } from "@/runtime/kernel/compilers/turn-hook"
import { ToolValidationRetry } from "@/runtime/tools/validation-retry"
import { LLM } from "@/runtime/session/llm"
import { iife } from "@/shared/util/iife"
import { Shell } from "@/runtime/integrations/shell/shell"
import { Truncate } from "@/runtime/tools/builtins/truncation"
import * as WorkspaceContext from "@/runtime/session/session-context"
import { BackgroundTask } from "@/runtime/session/background-task"
import { HookDispatcher } from "@/runtime/hooks/dispatcher"
import { Scratchpad } from "@/runtime/session/scratchpad"
import * as BotChat from "@/runtime/bots/canonical-chat"
import * as BotTurn from "@/runtime/bots/bot-turn"
import { BotTurnInfo } from "@/runtime/bots/bot-turn"
import { composeTurnSystem } from "@/runtime/session/system-header"
import { RepoMap } from "@/codemap/repo-map"
import * as BotInbox from "@/runtime/bots/bot-inbox"
import { isRoutineTurnText } from "@/runtime/bots/bot-routines"
import { isMessageAgentSession, MessageAgentTool } from "@/runtime/tools/builtins/message-agent"
import { SuggestTaskTool } from "@/runtime/tools/builtins/suggest-task"
import { GrantTaskPermissionTool } from "@/runtime/tools/builtins/grant-task-permission"
import { WORKER_BOT_TOOL_IDS } from "@/runtime/tools/builtins/worker-bot-tools"
import { isAppBotMainChat, isAppBotSession } from "@/runtime/bots/app-bot-routing"
import { PROJECT_SEND_TOOL_IDS } from "@/runtime/tools/builtins/project-send"
import { SessionHandoff } from "@/runtime/session/handoff"
import { END_CONVERSATION_TOOL_ID } from "@/runtime/primitives/gap-types"
import { SessionPause } from "@/runtime/session/pause"
import { SessionLimit } from "@/runtime/session/limit"
import type { QuotaWindow } from "@/runtime/providers/quota"
import { Budget } from "@/runtime/session/budget"
import * as TurnRouter from "@/runtime/routing/turn-router"
import { SleepTriggers } from "@/runtime/memory/sleep/triggers"
import { PROMPT_CACHE_TTL_S } from "@/shared/util/cache-countdown"

// @ts-ignore — suppress ai-sdk stdout warnings (see server.ts for details)
globalThis.AI_SDK_LOG_WARNINGS = false

const STRUCTURED_OUTPUT_DESCRIPTION = `Use this tool to return your final response in the requested structured format.

IMPORTANT:
- You MUST call this tool exactly once at the end of your response
- The input must be valid JSON matching the required schema
- Complete all necessary research and tool calls BEFORE calling this tool
- This tool provides your final answer - no further actions are taken after calling it`

const STRUCTURED_OUTPUT_SYSTEM_PROMPT = `IMPORTANT: The user has requested structured output. You MUST use the StructuredOutput tool to provide your final response. Do NOT respond with plain text - you MUST call the StructuredOutput tool with your answer formatted according to the schema.`

export namespace SessionPrompt {
  const log = Log.create({ service: "session.prompt" })

  const state = Instance.state(
    () => {
      const data: Record<
        string,
        {
          abort: AbortController
          callbacks: {
            resolve(input: MessageV2.WithParts): void
            reject(reason?: any): void
          }[]
          queuedTurns: number
        }
      > = {}
      return data
    },
    async (current) => {
      for (const item of Object.values(current)) {
        item.abort.abort()
      }
    },
  )

  // A running turn keeps a settings reload from aborting it (Instance.disposeAll).
  Instance.registerBusyCheck(() => Object.keys(state()).length > 0)

  export function assertNotBusy(sessionID: string) {
    const match = state()[sessionID]
    if (match) throw new Session.BusyError(sessionID)
  }

  export const PromptInput = z.object({
    sessionID: Identifier.schema("session"),
    messageID: Identifier.schema("message").optional(),
    model: z
      .object({
        providerID: z.string(),
        modelID: z.string(),
      })
      .optional(),
    fallbackModels: z
      .array(
        z.object({
          providerID: z.string(),
          modelID: z.string(),
        }),
      )
      .optional(),
    provider: z
      .record(z.string(), z.unknown())
      .optional()
      .describe(
        "Provider routing object (OpenRouter-style `provider` body key: sort/only/ignore/order/require_parameters/data_collection) forwarded by API bridges such as the Allternit LLM gateway. Applied to this message's turn only; injected onto the outbound request body for OpenAI-compatible providers.",
      ),
    provider_credentials: z
      .record(z.string(), z.unknown())
      .optional()
      .describe(
        "BYO provider credentials ({ apiKey, baseURL? }) forwarded by API bridges such as the Allternit LLM gateway when the caller's own cloud subscription serves the resolved route. Applied to this message's turn only; overrides the provider's configured key/endpoint for OpenAI-compatible providers. Never persisted on the session.",
      ),
    agent: z.string().optional(),
    noReply: z.boolean().optional(),
    tools: z
      .record(z.string(), z.boolean())
      .optional()
      .describe(
        "@deprecated tools and permissions have been merged, you can set permissions on the session itself now",
      ),
    metadata: z
      .record(z.string(), z.unknown())
      .optional()
      .describe(
        "Passthrough metadata forwarded by API bridges. The mobile composer sends `metadata.tools` ({ webSearch, research, toolAccess }) here; it is applied to this message's turn only and never persisted on the session.",
      ),
    mcpProxy: z
      .record(z.string(), z.unknown())
      .optional()
      .describe(
        "The caller's per-user MCP proxy for this turn ({ server, url, sessionId, token }), sent by allternit-api. Held in memory for the turn only: deliberately not part of `metadata` (which is stored on the user message) because the token must never be written to disk.",
      ),
    format: MessageV2.Format.optional(),
    system: z.string().optional(),
    turnSystem: z
      .string()
      .optional()
      .describe(
        "A system layer for this turn only, sent by allternit-api or the app: this turn's answer-template line, the card-repair note, the user's opt-in shared location. Held in memory for the turn and appended after `system`; deliberately not stored on the user message, never logged, never carried to later turns.",
      ),
    turnLocation: z
      .record(z.string(), z.unknown())
      .optional()
      .describe(
        "The user's opt-in shared location for this turn only, structured ({ lat, lon, accuracy_m?, time_zone?, place?, at? }; lat/lon rounded to two decimals), sent by allternit-api (client_location::turn_value) beside the prose line in `turnSystem`. Tools read it (lookup \"here\"). Held in memory for the turn like `turnSystem`: deliberately not part of `metadata` (stored on the user message), never logged, never carried to later turns. Invalid values are ignored.",
      ),
    /**
     * Run this turn as one of the user's platform bots (sent by
     * allternit-api): the bot's identity leads, and the user's personal
     * instruction files and skills are not loaded. Stored on the user message
     * and sticky for the session. See runtime/bots/bot-turn.ts.
     */
    bot: BotTurnInfo.optional(),
    /**
     * The session's working folder (e.g. its project's folder): tools and CLI
     * agents run there. Kept for later turns; an empty string clears it.
     */
    workdir: z.string().optional(),
    variant: z.string().optional(),
    backgroundPolicy: z.enum(["exit", "drain", "steer"]).optional(),
    parts: z.array(
      z.discriminatedUnion("type", [
        MessageV2.TextPart.omit({
          messageID: true,
          sessionID: true,
        })
          .partial({
            id: true,
          })
          ,
        MessageV2.FilePart.omit({
          messageID: true,
          sessionID: true,
        })
          .partial({
            id: true,
          })
          ,
        MessageV2.AgentPart.omit({
          messageID: true,
          sessionID: true,
        })
          .partial({
            id: true,
          })
          ,
        MessageV2.SubtaskPart.omit({
          messageID: true,
          sessionID: true,
        })
          .partial({
            id: true,
          })
          ,
      ]),
    ),
  })
  export type PromptInput = z.infer<typeof PromptInput>

  export const prompt = fn(PromptInput, async (input) => {
    // The client's time zone (`metadata.time_zone`, a hint like form_factor):
    // kept as the runtime's last client zone for turns that carry none
    // (routines, workers). Invalid names are ignored; never throws.
    await Clock.rememberClientZone(input.metadata?.["time_zone"])
    // A live terminal session runs its turns in the terminal: hand the text over (TerminalLive).
    const { TerminalLive } = await import("@/runtime/session/terminal-live")
    if (TerminalLive.attached(input.sessionID)) {
      const text = input.parts
        .filter((part): part is MessageV2.TextPart => part.type === "text" && !(part as MessageV2.TextPart).synthetic)
        .map((part) => part.text)
        .join("\n")
      // The terminal echoes the prompt into its transcript, which syncs back as the stored user message.
      if (input.noReply !== true) TerminalLive.deliver(input.sessionID, text)
      const info: MessageV2.User = {
        id: input.messageID ?? Identifier.ascending("message"),
        sessionID: input.sessionID,
        role: "user",
        time: { created: Date.now() },
        agent: input.agent ?? "build",
        model: input.model ?? { providerID: TerminalLive.HARNESS, modelID: "terminal" },
        metadata: { terminal: "delivered" },
      }
      return { info, parts: [] } as unknown as MessageV2.WithParts
    }
    // No terminal holds it: the engine continues the session, its tools in the terminal's folder.
    if (input.workdir === undefined && !workdirs.has(input.sessionID)) {
      const owner = await Session.get(input.sessionID).catch(() => undefined)
      if (owner?.sourceRef?.harness === TerminalLive.HARNESS) input.workdir = owner.directory
    }
    // Context handoff: a prompt to a session that handed off lands on its
    // lineage head, and a switch to a model whose window can't hold this one
    // hands off first. Every entry point (HTTP, CLI, bots, routines) goes
    // through here, so no client can talk to a closed window.
    // An ended chat (end_conversation) takes no new prompts; clients show the
    // message and offer a new chat. Checked on the lineage head before a
    // model-switch handoff could open a fresh window from it.
    assertNotEnded(await Session.get(await SessionHandoff.head(input.sessionID)).catch(() => undefined))
    input.sessionID = await beforeTurnHandoff(input)
    const session = await Session.get(input.sessionID)
    assertNotEnded(session)
    // Self-hosted: refuse the turn before anything is stored when the agent's
    // custom tools are malformed or shadow a built-in or MCP tool of this
    // session (resolveTools registers them again each step).
    const customTools = RemoteToolHost.customToolsFrom(input.metadata)
    if (customTools.length) {
      RemoteToolHost.registerCustomTools(
        input.sessionID,
        customTools,
        await reservedToolNames({ sessionID: input.sessionID, mcpProxy: input.mcpProxy }),
      )
    }
    await SessionRevert.cleanup(session)

    const submittedText = input.parts
      .filter((part) => part.type === "text" && !(part as MessageV2.TextPart).synthetic)
      .map((part) => (part as MessageV2.TextPart).text)
      .join("\n")
    const submitted = submittedText
      ? await HookDispatcher.emit({
          name: "UserPromptSubmit",
          timestamp: Date.now(),
          sessionId: input.sessionID,
          payload: { text: submittedText },
        })
      : { decision: "allow" as const }
    if (submitted.decision === "deny") throw new Error(submitted.reason ?? "Prompt blocked by hook")
    if (submitted.message) {
      input.parts.push({ type: "text", text: `<hook_result hook_event="UserPromptSubmit">\n${submitted.message}\n</hook_result>` })
    }

    // Bot Mode (B5): presence heartbeat. A canonical bot chat counts as
    // "working" from turn start — record before the model call, not after
    // success. Bots woken by message_agent land here too (the inbox pickup
    // below is what wakes them). Best-effort: presence must never break a
    // turn. Imported lazily to keep the prompt module's static import graph
    // unchanged for its other importers.
    try {
      const { recordCanonicalChatActivity } = await import("@/runtime/bots/bot-presence")
      await recordCanonicalChatActivity(input.sessionID)
    } catch (error) {
      log.warn("bot presence heartbeat failed; continuing", { sessionID: input.sessionID, error })
    }

    // Bot Mode (B4/D5): turn-start inbox pickup. When this session is a bot's
    // canonical chat, drain the bot's inbox (message_agent DMs from teammates)
    // and inject each envelope as an attributed user-role text part in this
    // turn's user message. Parts are synthetic so the TUI transcript shows the
    // turn the human submitted, not transport noise; the model still sees the
    // attributed messages. Runs before createUserMessage so both the TUI and
    // headless paths (and routine deliveries, which re-enter through prompt)
    // pick up pending mail exactly once per turn.
    let confirmInbox: () => Promise<void> = async () => {}
    if (input.parts.length > 0) {
      const picked = await BotInbox.collectBotInbox(input.sessionID).catch((error) => {
        log.warn("bot inbox pickup failed; continuing without injected mail", { sessionID: input.sessionID, error })
        return { texts: [] as string[], confirm: async () => {} }
      })
      confirmInbox = picked.confirm
      for (const text of picked.texts) {
        input.parts.push({ type: "text", text, synthetic: true })
      }
    }

    if (input.backgroundPolicy) {
      BackgroundTask.setPrintPolicy(input.sessionID, input.backgroundPolicy)
    }
    if (input.workdir !== undefined) setWorkdir(input.sessionID, input.workdir)

const message = await createUserMessage(input)
    // The injected mail is saved with the turn's user message: drop its staged copy.
    await confirmInbox()
    await Session.touch(input.sessionID)

    // this is backwards compatibility for allowing `tools` to be specified when
    // prompting
    const permissions: PermissionNext.Ruleset = []
    for (const [tool, enabled] of Object.entries(input.tools ?? {})) {
      permissions.push({
        permission: tool,
        action: enabled ? "allow" : "deny",
        pattern: "*",
      })
    }
    if (permissions.length > 0) {
      session.permission = permissions
      await Session.setPermission({ sessionID: session.id, permission: permissions })
    }

    if (input.noReply === true) {
      return message
    }

    // Allternit Sleep: a new prompt cancels the idle timer (and sends
    // `resume` after an `idle`). Top-level sessions only; fire-and-forget.
    if (!session.parentID) SleepTriggers.promptArrived(input.sessionID, PROMPT_CACHE_TTL_S)

    // Usage limits (P3.17): a paused session holds the turn until it
    // resumes; one about to run into a limit lands here instead of failing.
    if (await holdForLimit(session, input.model)) return message

    // Cloud code environment (Contract C): resolve it, clone its repos and
    // run its setup script before the first agent turn. A failure ends this
    // turn with the error instead of running the agent without its setup.
    // Local sessions (no environment) never load this module.
    if (session.environment) {
      const blocked = await applyEnvironment(session, message)
      if (blocked) return blocked
    }

    // Bot Mode: interactive turns (user chat or teammate DM pickup) carry the
    // first-response preamble in the persona injection; routine deliveries
    // (`[routine: …]` marker) and delegated subtask turns stay quiet.
    const botPreamble = !input.parts.some(
      (p) => (p.type === "text" && isRoutineTurnText(p.text)) || p.type === "subtask",
    )

    // The per-user connector proxy exists for this turn only.
    const releaseMcpProxy = McpUserProxy.register(input.sessionID, input.mcpProxy)
    // So does the turn-only system layer (answer template, card-repair note, opt-in location).
    const turnLayer = input.turnSystem?.trim()
    if (turnLayer) turnSystemLayers.set(input.sessionID, turnLayer)
    // The shared location, structured for tools (lookup "here"); same lifetime. Read from
    // the `turnLocation` field; the `turnSystem` text only when an older API sent no field.
    const location = resolveTurnLocation(input.turnLocation, turnLayer)
    if (location) TurnLocations.set(input.sessionID, location)
    try {
      const result = await loop({ sessionID: input.sessionID, fallbackModels: input.fallbackModels, botPreamble })
      if (!afterTurnLimit(session, result)) afterTurnHandoff(input.sessionID, result)
      return result
    } finally {
      releaseMcpProxy()
      if (turnLayer && turnSystemLayers.get(input.sessionID) === turnLayer) turnSystemLayers.delete(input.sessionID)
      if (location) TurnLocations.release(input.sessionID, location)
    }
  })

  /**
   * Turn-only system layers by session (`PromptInput.turnSystem`). In memory
   * only: set for the length of one prompt() turn, removed when it ends, never
   * written to the session, its messages or the logs.
   */
  const turnSystemLayers = new Map<string, string>()

  /** The opt-in shared location of the turn now running in this session (lat/lon, radius, zone), or undefined. */
  export function turnLocation(sessionID: string): TurnLocation | undefined {
    return TurnLocations.get(sessionID)
  }

  // Cancelling a session while its environment is being set up stops the setup.
  const setupAborts = new Map<string, AbortController>()

  /** Apply the session's cloud environment; returns the message to end the turn with when it failed. */
  async function applyEnvironment(
    session: Session.Info,
    message: MessageV2.WithParts,
  ): Promise<MessageV2.WithParts | undefined> {
    const sessionID = session.id
    const controller = new AbortController()
    setupAborts.set(sessionID, controller)
    const needsWork = session.environment?.status !== "ready"
    if (needsWork) SessionStatus.set(sessionID, { type: "busy" })
    try {
      const { SessionEnvironment } = await import("@/runtime/session/environment")
      const applied = await SessionEnvironment.ensure({
        session,
        user: message.info as MessageV2.User,
        workspace: workdir(sessionID) ?? session.directory,
        abort: controller.signal,
      })
      if (applied.ok) {
        if (applied.note) {
          await Session.updatePart({
            id: Identifier.ascending("part"),
            sessionID,
            messageID: message.info.id,
            type: "text",
            text: applied.note,
            synthetic: true,
          })
        }
        return undefined
      }
      const failed = applied as { error: string; message?: MessageV2.WithParts }
      const error = { name: "EnvironmentSetupError", message: failed.error }
      Bus.publish(Session.Event.Error, { sessionID, error })
      SessionStatus.set(sessionID, { type: "idle" })
      return failed.message ?? message
    } catch (error) {
      // Never let environment setup take the server down: fail this turn only.
      const text = error instanceof Error ? error.message : String(error)
      log.error("environment setup crashed", { sessionID, error: text })
      Bus.publish(Session.Event.Error, { sessionID, error: { name: "EnvironmentSetupError", message: text } })
      SessionStatus.set(sessionID, { type: "idle" })
      return message
    } finally {
      if (setupAborts.get(sessionID) === controller) setupAborts.delete(sessionID)
    }
  }

  /** The wrap-up instruction a turn gets once it crosses land_at. */
  export function wrapUpReminder(window: { label: string; source: string; resetAt: string }): string {
    const reset = new Date(window.resetAt).toLocaleString("en-US", { weekday: "short", hour: "numeric", minute: "2-digit" })
    return WRAP_UP.replace("{{limit}}", `the ${window.source} ${window.label.toLowerCase()} limit`).replace("{{reset}}", reset).trim()
  }

  /** Hold this turn when the session is paused or about to hit a limit. */
  export async function holdForLimit(session: Session.Info, model: PromptInput["model"]): Promise<boolean> {
    if (session.parentID) return false
    if (SessionPause.isPaused(session)) return true
    // Spend limits (P8.1): a bot's monthly budget or the thread's own.
    // A failed check pauses bot / budgeted sessions ("budget-check-failed").
    const hold = await Budget.gate(session)
    if (hold) {
      SessionPause.pause(session.id, hold)
      return true
    }
    const providerID = model?.providerID ?? session.defaultModel?.providerID
    if (!providerID) return false
    const modelID = model?.modelID ?? session.defaultModel?.modelID
    // Also publishes "approaching" (or clears a reset window) for the limit strip.
    const window = await SessionLimit.check(session, providerID, modelID).catch(() => undefined)
    if (!window) return false
    SessionPause.pause(
      session.id,
      { until: Date.parse(window.resetAt), limit: `${window.source} ${window.label.toLowerCase()} limit`, providerID, reason: "quota" },
      { state: "paused", window },
    )
    return true
  }

  /** A turn that ran into a limit pauses the session until it resets. */
  function afterTurnLimit(session: Session.Info, result: MessageV2.WithParts | undefined): boolean {
    if (session.parentID) return false
    // Already landed by the loop's wrap-up.
    if (SessionPause.scheduled(session.id)) return true
    const hit = SessionPause.limitFromTurn(result)
    if (!hit) return false
    const providerID = result?.info.role === "assistant" ? result.info.providerID : undefined
    SessionPause.pause(session.id, { until: hit.until, limit: hit.limit, reason: hit.reason, providerID })
    return true
  }

  function assertNotEnded(session: Session.Info | undefined) {
    if (!session?.ended) return
    throw new Session.EndedError(
      { sessionID: session.id, at: session.ended.at, reason: session.ended.reason },
      { message: Session.ENDED_MESSAGE },
    )
  }

  async function beforeTurnHandoff(input: PromptInput): Promise<string> {
    await SessionHandoff.settled(input.sessionID)
    const head = await SessionHandoff.head(input.sessionID)
    if (head !== input.sessionID) log.info("prompt redirected to lineage head", { from: input.sessionID, to: head })
    if (!input.model) return head
    try {
      const session = await Session.get(head)
      if (session.parentID || state()[head]) return head
      let last: MessageV2.Assistant | undefined
      for await (const msg of MessageV2.stream(head)) {
        if (msg.info.role === "assistant" && msg.info.finish) {
          last = msg.info as MessageV2.Assistant
          break
        }
      }
      if (!last) return head
      if (last.providerID === input.model.providerID && last.modelID === input.model.modelID) return head
      const next = await Provider.getModel(input.model.providerID, input.model.modelID)
      const current = await Provider.getModel(last.providerID, last.modelID).catch(() => undefined)
      if (current && SessionHandoff.usable(next) >= SessionHandoff.usable(current)) return head
      if (!(await SessionHandoff.shouldHandoff({ tokens: last.tokens, model: next }))) return head
      const handed = await SessionHandoff.run({ sessionID: head, reason: "model_switch" })
      return handed.session.id
    } catch (error) {
      log.warn("model-switch handoff check failed; continuing in the same window", { sessionID: head, error })
      return head
    }
  }

  /** After a turn: hand off in the background once the window is past the threshold. */
  function afterTurnHandoff(sessionID: string, result: MessageV2.WithParts | undefined) {
    const info = result?.info
    if (!info || info.role !== "assistant" || !info.finish || info.summary) return
    void (async () => {
      const session = await Session.get(sessionID)
      if (session.parentID || session.handoff) return
      const model = await Provider.getModel(info.providerID, info.modelID)
      if (!(await SessionHandoff.shouldHandoff({ tokens: info.tokens, model }))) return
      await SessionHandoff.run({ sessionID, reason: "threshold" })
    })().catch((error) => log.warn("automatic handoff failed; compaction still guards the window", { sessionID, error }))
  }

  /** Retry the latest durable user turn without appending a duplicate prompt. */
  export async function retry(sessionID: string): Promise<MessageV2.WithParts> {
    assertNotBusy(sessionID)
    return loop({ sessionID })
  }

  export async function resolvePromptParts(template: string): Promise<PromptInput["parts"]> {
    const parts: PromptInput["parts"] = [
      {
        type: "text",
        text: template,
      },
    ]
    const files = ConfigMarkdown.files(template)
    const seen = new Set<string>()
    await Promise.all(
      files.map(async (match) => {
        const name = match[1]
        if (seen.has(name)) return
        seen.add(name)
        const filepath = name.startsWith("~/")
          ? path.join(os.homedir(), name.slice(2))
          : path.resolve(Instance.worktree, name)

        const stats = await fs.stat(filepath).catch(() => undefined)
        if (!stats) {
          const agent = await Agent.get(name)
          if (agent) {
            parts.push({
              type: "agent",
              name: agent.name,
            })
          }
          return
        }

        if (stats.isDirectory()) {
          parts.push({
            type: "file",
            url: pathToFileURL(filepath).href,
            filename: name,
            mime: "application/x-directory",
          })
          return
        }

        parts.push({
          type: "file",
          url: pathToFileURL(filepath).href,
          filename: name,
          mime: "text/plain",
        })
      }),
    )
    return parts
  }

  function start(sessionID: string) {
    const s = state()
    if (s[sessionID]) return
    const controller = new AbortController()
    s[sessionID] = {
      abort: controller,
      callbacks: [],
      queuedTurns: 0,
    }
    return controller.signal
  }

  function resume(sessionID: string) {
    const s = state()
    if (!s[sessionID]) return

    return s[sessionID].abort.signal
  }

  // Loaded once, off the import graph's critical path (it imports Session).
  let terminalLive: typeof import("@/runtime/session/terminal-live").TerminalLive | undefined
  void import("@/runtime/session/terminal-live").then((m) => (terminalLive = m.TerminalLive))

  export function cancel(sessionID: string) {
    log.info("cancel", { sessionID })
    // A live terminal session's turn runs in the terminal: Stop goes there,
    // and the terminal reports idle itself once its turn has really stopped.
    if (terminalLive?.stop(sessionID)) return
    setupAborts.get(sessionID)?.abort()
    const s = state()
    const match = s[sessionID]
    if (!match) {
      SessionStatus.set(sessionID, { type: "idle" })
      return
    }
    void HookDispatcher.emit({
      name: "Interrupt",
      timestamp: Date.now(),
      sessionId: sessionID,
      payload: { reason: "user" },
    })
    match.abort.abort()
    delete s[sessionID]
    SessionStatus.set(sessionID, { type: "idle" })
    return
  }

  export const LoopInput = z.object({
    sessionID: Identifier.schema("session"),
    resume_existing: z.boolean().optional(),
    botPreamble: z
      .boolean()
      .optional()
      .describe(
        "Bot Mode: true when the turn originated from a person (user chat or teammate DM pickup), enabling the first-response preamble in the bot persona injection. Routine deliveries and internal turns omit it.",
      ),
    fallbackModels: z
      .array(
        z.object({
          providerID: z.string(),
          modelID: z.string(),
        }),
      )
      .optional(),
  })
  // Session working folders, by session id. Clients send the folder on every
  // turn (it follows the session's project), so memory is enough.
  const workdirs = new Map<string, string>()

  function setWorkdir(sessionID: string, workdir: string) {
    const dir = workdir.trim()
    if (!dir) {
      workdirs.delete(sessionID)
      return
    }
    try {
      if (path.isAbsolute(dir) && statSync(dir).isDirectory()) {
        workdirs.set(sessionID, dir)
        return
      }
    } catch {}
    log.warn("ignoring session workdir that is not a folder", { sessionID, workdir: dir })
  }

  /** The folder a session's turns run in, if it has one of its own. */
  export function workdir(sessionID: string): string | undefined {
    return workdirs.get(sessionID)
  }

  export const loop = fn(LoopInput, async (input) => {
    // A worktree the session entered wins over the folder the client sends each turn.
    // Always provide the folder, so worktree_enter / worktree_exit can move the running turn.
    const dir = (await SessionWorktree.active(input.sessionID))?.path ?? workdirs.get(input.sessionID) ?? Instance.workdir
    return Instance.withWorkdir(dir, () => runLoop(input))
  })

  async function runLoop(input: z.infer<typeof LoopInput>) {
    const { sessionID, resume_existing } = input

    const abort = resume_existing ? resume(sessionID) : start(sessionID)
    if (!abort) {
      return new Promise<MessageV2.WithParts>((resolve, reject) => {
        const callbacks = state()[sessionID].callbacks
        callbacks.push({ resolve, reject })
        state()[sessionID].queuedTurns += 1
        // Primitives Wave B: a message queued behind the running turn wakes a
        // wait_agents call in that turn. This bump also fires for background
        // completions, so the core checks its targets first (settled before steer).
        SteerSignal.notify(sessionID)
      })
    }

    using _ = defer(() => cancel(sessionID))

    // Structured output state
    // Note: On session resumption, state is reset but outputFormat is preserved
    // on the user message and will be retrieved from lastUser below
    let structuredOutput: unknown | undefined
    let answerGuard: AnswerTurnGuard | undefined

    let step = 0
    // O12: one guard per turn. A durable-goal continuation is its own slice:
    // it resets the guard and the step base (goals carry their own budgets).
    let guard: Guardrails.TurnGuard | undefined
    let turnStepBase = 0
    let wallClockTimer: ReturnType<typeof setTimeout> | undefined
    using _wallClock = defer(() => clearTimeout(wallClockTimer))
    const armWallClock = (g: Guardrails.TurnGuard) => {
      clearTimeout(wallClockTimer)
      wallClockTimer = setTimeout(() => {
        const trip = g.checkWallClock()
        if (!trip) return
        SessionProcessor.reportTrip(sessionID, undefined, trip)
        state()[sessionID]?.abort.abort()
      }, g.remainingWallClockMs())
    }
    let turnModel: { providerID: string; modelID: string } | undefined
    // Open-lane handoff (providers/handoff.ts): once a step is refused, the
    // rest of the turn runs on the open model, without the refused message.
    let refusalHandoff:
      | { to: { providerID: string; modelID: string }; from: string; refusedID: string }
      | undefined
    const session = await Session.get(sessionID)
    // The prompt stack follows the session's surface; a child session (task,
    // subagent) uses its root's. Resolved once per turn: surfaces don't change mid-turn.
    const promptSurface = await SystemPrompt.effectiveSurface(session, (id) => Session.get(id))
    // Usage-limit wrap-up: set once a step crosses land_at mid-turn.
    let wrapUp:
      | { left: number; done: boolean; providerID: string; window: QuotaWindow & { resetAt: string; source: string } }
      | undefined
    while (true) {
      SessionStatus.set(sessionID, { type: "busy" })
      log.info("loop", { step, sessionID })
      if (abort.aborted) break
      let msgs = await MessageV2.filterCompacted(MessageV2.stream(sessionID))
      if (refusalHandoff) {
        const refusedID = refusalHandoff.refusedID
        msgs = msgs.filter((m) => m.info.id !== refusedID)
      }

      let lastUser: MessageV2.User | undefined
      let lastAssistant: MessageV2.Assistant | undefined
      let lastFinished: MessageV2.Assistant | undefined
      let tasks: (MessageV2.CompactionPart | MessageV2.SubtaskPart)[] = []
      for (let i = msgs.length - 1; i >= 0; i--) {
        const msg = msgs[i]
        if (!lastUser && msg.info.role === "user") lastUser = msg.info as MessageV2.User
        if (!lastAssistant && msg.info.role === "assistant") lastAssistant = msg.info as MessageV2.Assistant
        if (!lastFinished && msg.info.role === "assistant" && msg.info.finish)
          lastFinished = msg.info as MessageV2.Assistant
        if (lastUser && lastFinished) break
        const task = msg.parts.filter((part) => part.type === "compaction" || part.type === "subtask")
        if (task && !lastFinished) {
          tasks.push(...task)
        }
      }

      if (!lastUser) throw new Error("No user message found in stream. This should never happen.")
      // A synthetic hook/goal continuation cannot replace the person's output contract.
      const policySource = msgs.findLast(m => m.info.role === 'user' && m.parts.some(p => p.type === 'text' && !p.synthetic && !p.ignored))
        ?? (!answerGuard ? msgs.findLast(m => m.info.role === 'user') : undefined);
      if (policySource && answerGuard?.userID !== policySource.info.id) {
        const text = policySource.parts.filter((p): p is MessageV2.TextPart => p.type === 'text' && !p.synthetic && !p.ignored).map(p => p.text).join('\n');
        answerGuard = new AnswerTurnGuard(policySource.info.id, text, policySource.info as MessageV2.User);
        answerGuard.recordSelection(sessionID);
      }

      // A bot turn (allternit-api marked a user message with `bot`) runs as
      // the bot: persona first, no personal instruction files, skills or
      // workspace identity from ~/.gizzi (runtime/bots/bot-turn.ts). Resolved
      // before tools are built so the skill tool sees it too.
      const botTurn = BotTurn.resolve(sessionID, msgs)

      if (step === 1 && session.sourceRef && session.sourceRef.harness !== "gizzi-tui") {
        try {
          const { NativeSource } = await import("@/runtime/session/native-source")
          const originText = await NativeSource.originPromptBlock(sessionID)
          if (originText) {
            const target = msgs.find((m) => m.info.id === lastUser.id)
            if (target) {
              target.parts.push({
                id: Identifier.ascending("part"),
                sessionID,
                messageID: lastUser.id,
                type: "text",
                text: originText,
                synthetic: true,
              })
            }
          }
        } catch (error) {
          log.warn("native origin fetch skipped", { sessionID, error })
        }
      }
      if (
        lastAssistant?.finish &&
        !["tool-calls", "unknown"].includes(lastAssistant.finish) &&
        lastUser.id < lastAssistant.id &&
        !answerGuard.needsRepair
      ) {
        log.info("exiting loop", { sessionID })
        break
      }

      const goalAtTurnStart = GoalEngine.getCurrentGoal(sessionID)
      const goalTurnsAtStart = goalAtTurnStart?.state === "in_progress"
        ? goalAtTurnStart.usage.turnsUsed
        : undefined
      if (goalAtTurnStart?.state === "in_progress") GoalEngine.enforceBudget(goalAtTurnStart.id)

      step++
      if (step === 1) {
        // New turn: don't let an unrelated earlier turn's validation failure cap a fresh attempt.
        ToolValidationRetry.resetSession(sessionID)
        ensureTitle({
          session,
          modelID: lastUser.model.modelID,
          providerID: lastUser.model.providerID,
          history: msgs,
        })
      }

      // Resolve auto-routing before model lookup (the incumbent router)
      let resolvedModelRef =
        lastUser.model.providerID === "auto"
          ? await Provider.resolveAuto(
              msgs.map((m) => ({
                role: m.info.role,
                content: m.parts
                  .filter((p) => p.type === "text")
                  .map((p) => (p as MessageV2.TextPart).text)
                  .join(" "),
              })),
              sessionID,
            )
          : lastUser.model

      // WP-R1 (O2/O14): the kernel router + S1 ROUTE/ROUTE_MODEL banks in shadow.
      // The incumbent above decides unless ALLTERNIT_TURN_ROUTE_AUTHORITY=kernel.
      if (step === 1) {
        const userID = lastUser.id
        const userText = msgs
          .find((m) => m.info.id === userID)
          ?.parts.filter((p) => p.type === "text")
          .map((p) => (p as MessageV2.TextPart).text)
          .join(" ") ?? ""
        const routed = await TurnRouter.startTurn({
          sessionID,
          userMessageID: userID,
          text: userText,
          requested: lastUser.model,
          incumbent: resolvedModelRef,
        }).catch((e) => {
          log.warn("turn router failed; incumbent stands", { error: (e as Error).message })
          return { model: resolvedModelRef }
        })
        turnModel = routed.model
      }
      if (lastUser.model.providerID === "auto" && turnModel) resolvedModelRef = turnModel
      if (refusalHandoff) resolvedModelRef = refusalHandoff.to

      const plan = await Provider.prepareAuth(resolvedModelRef)

      const model = await Provider.getModel(resolvedModelRef.providerID, resolvedModelRef.modelID).catch(async (e) => {
        // Try fallback model if configured
        if (Flag.GIZZI_FALLBACK_MODEL) {
          const parsed = Provider.parseModel(Flag.GIZZI_FALLBACK_MODEL)
          if (parsed) {
            log.info("primary model failed, trying fallback", { fallback: Flag.GIZZI_FALLBACK_MODEL })
            return Provider.getModel(parsed.providerID, parsed.modelID)
          }
        }
        if (Provider.ModelNotFoundError.isInstance(e)) {
          const suggestions = e.data?.suggestions as string[] | undefined
          const providerID = e.data?.providerID as string | undefined
          const modelID = e.data?.modelID as string | undefined
          const hint = suggestions?.length ? ` Did you mean: ${suggestions.join(", ")}?` : ""
          Bus.publish(Session.Event.Error, {
            sessionID,
            error: { name: "Unknown", message: `Model not found: ${providerID}/${modelID}.${hint}`, data: {} },
          })
        }
        throw e
      })
      const task = tasks.pop()

      // pending subtask — tool invocation is inline here because it needs tight
      // integration with session state, message creation, and the abort controller.
      if (task?.type === "subtask") {
        const taskTool = await TaskTool.init()
        const taskModel = task.model ? await Provider.getModel(task.model.providerID, task.model.modelID) : model
        const assistantMessage = (await Session.updateMessage({
          id: Identifier.ascending("message"),
          role: "assistant",
          parentID: lastUser.id,
          sessionID,
          mode: task.agent,
          agent: task.agent,
          variant: lastUser.variant,
          path: {
            cwd: Instance.directory,
            root: Instance.worktree,
          },
          cost: 0,
          tokens: {
            input: 0,
            output: 0,
            reasoning: 0,
            cache: { read: 0, write: 0 },
          },
          modelID: taskModel.id,
          providerID: taskModel.providerID,
          time: {
            created: Date.now(),
          },
        })) as MessageV2.Assistant
        let part = (await Session.updatePart({
          id: Identifier.ascending("part"),
          messageID: assistantMessage.id,
          sessionID: assistantMessage.sessionID,
          type: "tool",
          callID: ulid(),
          tool: TaskTool.id,
          state: {
            status: "running",
            input: {
              prompt: task.prompt,
              description: task.description,
              subagent_type: task.agent,
              command: task.command,
            },
            time: {
              start: Date.now(),
            },
          },
        })) as MessageV2.ToolPart
        const taskArgs = {
          prompt: task.prompt,
          description: task.description,
          subagent_type: task.agent,
          run_in_background: false,
          command: task.command,
        }
        await Plugin.trigger(
          "tool.execute.before",
          {
            tool: "task",
            sessionID,
            callID: part.id,
          },
          { args: taskArgs },
        )
        let executionError: Error | undefined
        const taskAgent = await Agent.get(task.agent)
        if (!taskAgent) {
          throw new Error(`Task agent not found: ${task.agent}`)
        }
        const taskCtx: Tool.Context = {
          agent: task.agent,
          messageID: assistantMessage.id,
          sessionID: sessionID,
          abort,
          callID: part.callID,
          extra: { bypassAgentCheck: true },
          messages: msgs,
          async metadata(input) {
            await Session.updatePart({
              ...part,
              type: "tool",
              state: {
                ...part.state,
                ...input,
              },
            } satisfies MessageV2.ToolPart)
          },
          async ask(req) {
            return await PermissionNext.ask({
              ...req,
              sessionID: sessionID,
              ruleset: PermissionNext.merge(taskAgent.permission, session.permission ?? []),
            })
          },
        }
        const result = await taskTool.execute(taskArgs, taskCtx).catch((error) => {
          executionError = error
          log.error("subtask execution failed", { error, agent: task.agent, description: task.description })
          return undefined
        })
        const attachments = result?.attachments?.map((attachment) => ({
          ...attachment,
          id: Identifier.ascending("part"),
          sessionID,
          messageID: assistantMessage.id,
        }))
        await Plugin.trigger(
          "tool.execute.after",
          {
            tool: "task",
            sessionID,
            callID: part.id,
            args: taskArgs,
          },
          result,
        )
        assistantMessage.finish = "tool-calls"
        assistantMessage.time.completed = Date.now()
        await Session.updateMessage(assistantMessage)
        if (result && part.state.status === "running") {
          await Session.updatePart({
            ...part,
            state: {
              status: "completed",
              input: part.state.input,
              title: result.title,
              metadata: result.metadata,
              output: result.output,
              attachments,
              time: {
                ...part.state.time,
                end: Date.now(),
              },
            },
          } satisfies MessageV2.ToolPart)
        }
        if (!result) {
          await Session.updatePart({
            ...part,
            state: {
              status: "error",
              error: executionError ? `Tool execution failed: ${executionError.message}` : "Tool execution failed",
              time: {
                start: part.state.status === "running" ? part.state.time.start : Date.now(),
                end: Date.now(),
              },
              metadata: part.metadata,
              input: part.state.input,
            },
          } satisfies MessageV2.ToolPart)
        }

        if (task.command) {
          // Add synthetic user message to prevent certain reasoning models from erroring
          // If we create assistant messages w/ out user ones following mid loop thinking signatures
          // will be missing and it can cause errors for models like gemini for example
          const summaryUserMsg: MessageV2.User = {
            id: Identifier.ascending("message"),
            sessionID,
            role: "user",
            time: {
              created: Date.now(),
            },
            agent: lastUser.agent,
            model: lastUser.model,
          }
          await Session.updateMessage(summaryUserMsg)
          await Session.updatePart({
            id: Identifier.ascending("part"),
            messageID: summaryUserMsg.id,
            sessionID,
            type: "text",
            text: "Summarize the task tool output above and continue with your task.",
            synthetic: true,
          } satisfies MessageV2.TextPart)
        }

        continue
      }

      // pending compaction
      if (task?.type === "compaction") {
        const result = await SessionCompaction.process({
          messages: msgs,
          parentID: lastUser.id,
          abort,
          sessionID,
          auto: task.auto,
        })
        if (result === "stop") break
        continue
      }

      // context overflow, needs compaction
      if (
        lastFinished &&
        lastFinished.summary !== true &&
        (await SessionCompaction.isOverflow({ tokens: lastFinished.tokens, model }))
      ) {
        await SessionCompaction.create({
          sessionID,
          agent: lastUser.agent,
          model: lastUser.model,
          auto: true,
        })
        continue
      }

      // normal processing
      const agent = await Agent.get(lastUser.agent)
      if (!agent) {
        throw new Error(`Agent not found: ${lastUser.agent}`)
      }
      if (!guard) {
        guard = new Guardrails.TurnGuard(Guardrails.limits(agent))
        armWallClock(guard)
      }
      const stepsThisTurn = step - turnStepBase
      // Never Infinity: versioned default per agent kind, agent config within the ceiling.
      const maxSteps = guard.limits.steps
      const isLastStep = stepsThisTurn >= maxSteps
      const overLimit = guard.checkSteps(stepsThisTurn) ?? guard.checkWallClock()
      if (overLimit) {
        SessionProcessor.reportTrip(sessionID, lastAssistant?.id, overLimit)
        const goal = GoalEngine.getCurrentGoal(sessionID)
        if (goal?.state === "in_progress") GoalEngine.pauseGoal(goal.id, overLimit.reason)
        break
      }
      msgs = await insertReminders({
        messages: msgs,
        agent,
        session,
      })

      const processor = SessionProcessor.create({
        assistantMessage: (await Session.updateMessage({
          id: Identifier.ascending("message"),
          parentID: lastUser.id,
          role: "assistant",
          mode: agent.name,
          agent: agent.name,
          variant: lastUser.variant,
          path: {
            cwd: Instance.directory,
            root: Instance.worktree,
          },
          cost: 0,
          tokens: {
            input: 0,
            output: 0,
            reasoning: 0,
            cache: { read: 0, write: 0 },
          },
          modelID: model.id,
          providerID: model.providerID,
          time: {
            created: Date.now(),
          },
          ...(refusalHandoff
            ? { handoff: { from: refusalHandoff.from, to: `${model.providerID}/${model.id}` } }
            : {}),
          sessionID,
        })) as MessageV2.Assistant,
        sessionID: sessionID,
        model,
        abort,
        fallbackModels: input.fallbackModels,
        guard,
        ...(answerGuard.required ? {
          holdText: (part: MessageV2.TextPart) => answerGuard!.hold(part),
          resetHeldText: (messageID: string) => answerGuard!.restart(messageID, sessionID),
        } : {}),
      })
      using _ = defer(() => InstructionPrompt.clear(processor.message.id))

      // Check if user explicitly invoked an agent via @ in this turn
      const lastUserMsg = msgs.findLast((m) => m.info.role === "user")
      const bypassAgentCheck = lastUserMsg?.parts.some((p) => p.type === "agent") ?? false

      // The turn's instructions are built while its tools load: neither needs the other.
      const turnSystemPromise = (async () => {
        const prepStarted = Date.now()
        const prepTimings: Record<string, number> = {}
        const timedPrep = <T>(name: string, work: Promise<T>) => {
          const started = Date.now()
          return work.finally(() => (prepTimings[name] = Date.now() - started))
        }
        const [botSystemPrompt, scratchpadSystemPrompt] = await Promise.all([
          timedPrep("botChat", BotChat.botChatSystemPrompt(sessionID, { preamble: input.botPreamble })).catch((error) => {
            log.warn("failed to build bot persona prompt", { sessionID, error })
            return undefined
          }),
          timedPrep("scratchpad", Scratchpad.instructions(sessionID)).catch((error) => {
            log.warn("failed to initialize session scratchpad", { sessionID, error })
            return undefined
          }),
        ])
        // If user provided a full system prompt override, use only that; a "+"
        // prefix (or --append-system-prompt) appends — or, on a bot turn, leads.
        const system = await composeTurnSystem({
          userSystem: lastUser.system ?? (answerGuard.required ? answerGuard.system : undefined),
          bot: botTurn,
          environment: () => SystemPrompt.environment(model, { turnZone: Clock.turnZoneOf(lastUser.metadata) }),
          instructions: () => InstructionPrompt.system(),
          workspace: async () => {
            const workspaceSystemPrompt = await WorkspaceContext.getWorkspaceSystemPrompt(session.directory)
            return workspaceSystemPrompt ? [workspaceSystemPrompt] : []
          },
          // Ranked file/symbol map of the session's project (codemap/repo-map.ts).
          // Rebuilt once per user turn so the cached system prefix holds across
          // steps; empty for folders that are not a project, or when turned off
          // (config `repoMap: false`, GIZZI_NO_REPO_MAP=1). Never throws.
          repoMap: async () => {
            const config = await Config.get().catch(() => undefined)
            const touched = Object.keys(FileTime.state().read[sessionID] ?? {})
            const block = await RepoMap.prompt({
              root: workdir(sessionID) ?? session.directory,
              sessionID,
              turn: lastUser.id,
              boost: touched,
              enabled: config?.repoMap !== false,
            })
            return block ? [block] : []
          },
          // Configured servers only: the per-user connectors proxy is opened per turn in resolveTools.
          mcpInstructions: async () => {
            const block = MCP.instructionsPrompt(
              await MCP.instructions().catch((error) => {
                log.warn("failed to read MCP server instructions", { sessionID, error })
                return []
              }),
            )
            return block ? [block] : []
          },
          botChat: botSystemPrompt,
          scratchpad: scratchpadSystemPrompt,
          onTiming: (name, ms) => (prepTimings[name] = ms),
        })
        // Where the wait before the model starts goes (the user sees only "Thinking" meanwhile).
        log.info("turn prep", { sessionID, totalMs: Date.now() - prepStarted, ...prepTimings })
        return system
      })()
      turnSystemPromise.catch(() => {}) // awaited below; keeps an early failure from going unhandled

      const tools = answerGuard.needsRepair ? {} : await resolveTools({
        agent,
        session,
        model,
        tools: lastUser.tools,
        metadata: lastUser.metadata,
        processor,
        bypassAgentCheck,
        messages: msgs,
      })

      // Inject StructuredOutput tool if JSON schema mode enabled
      if (lastUser.format?.type === "json_schema") {
        tools["StructuredOutput"] = createStructuredOutputTool({
          schema: lastUser.format.schema,
          onSuccess(output) {
            structuredOutput = output
          },
        })
      }

      if (step === 1) {
        SessionSummary.summarize({
          sessionID: sessionID,
          messageID: lastUser.id,
        })
      }

      // Ephemerally wrap queued user messages with a reminder to stay on track
      if (step > 1 && lastFinished) {
        for (const msg of msgs) {
          if (msg.info.role !== "user" || msg.info.id <= lastFinished.id) continue
          for (const part of msg.parts) {
            if (part.type !== "text" || part.ignored || part.synthetic) continue
            if (!part.text.trim()) continue
            part.text = [
              "<system-reminder>",
              "The user sent the following message:",
              part.text,
              "",
              "Please address this message and continue with your tasks.",
              "</system-reminder>",
            ].join("\n")
          }
        }
      }

      await Plugin.trigger("experimental.chat.messages.transform", {}, { messages: msgs })

      // Build system prompt, adding structured output instruction if needed
      // Get workspace context for the session directory (cached)
      // Bot Mode (B2): when this session is a bot's canonical chat, inject the
      // bot's identity + SOUL + memory as standing instructions. This builder
      // runs inside the runtime session pipeline, so both the TUI (worker /
      // server) and headless (`gizzi run --print`) paths get the injection.
      const system = await turnSystemPromise
      // answer_cards "widgets" (clients that draw no catalog cards): the short writing-only manual.
      if (answerGuard.widgetsManual) system.push(answerGuard.widgetsManual)
      // The turn-only layer (answer template, card-repair note, opt-in location) goes last among the user layers.
      const turnLayer = turnSystemLayers.get(sessionID)
      if (turnLayer) system.push(turnLayer)
      if (answerGuard.required) system.push(answerGuard.instruction())
      const format = lastUser.format ?? { type: "text" }
      if (format.type === "json_schema") {
        system.push(STRUCTURED_OUTPUT_SYSTEM_PROMPT)
      }
      // O8 variable tail: these change step to step, so they ride after
      // history (PromptSegments.withTail) instead of busting the cached
      // system prefix. `system` above is the stable [system][pinned] part.
      const tail: string[] = []
      const goalReminder = GoalEngine.reminder(sessionID)
      if (goalReminder) tail.push(goalReminder)
      const validationReminder = ToolValidationRetry.reminder(sessionID)
      if (validationReminder) tail.push(validationReminder)
      if (wrapUp) tail.push(wrapUpReminder(wrapUp.window))
      // Primitives Wave B: context left (25% / 10% / after a compaction), as a
      // temporary tail part; each threshold fires once (primitives/context-notice.ts).
      const contextNotice = await ContextNotice.forTurn({ sessionID, model, messages: msgs }).catch((error) => {
        log.warn("context notice failed", { sessionID, error })
        return undefined
      })
      if (contextNotice) tail.push(contextNotice)
      // WP9: Context Compiler, shadow mode behind GIZZI_KERNEL_COMPILERS (off by
      // default). Records a ContextProjectionV1; never changes `system`.
      if (KernelTurn.enabled())
        KernelTurn.compileTurnContext({
          sessionID,
          directory: session.directory,
          objective:
            lastUserMsg?.parts
              .filter((p) => p.type === "text")
              .map((p: any) => p.text)
              .join("\n") ?? "",
          system,
          budgetTokens: model.limit?.context ?? 0,
        })

      const activeGoalForDeadline = GoalEngine.getCurrentGoal(sessionID)
      const deadlineMs = activeGoalForDeadline?.state === "in_progress"
        ? GoalEngine.remainingWallClockMs(activeGoalForDeadline.id)
        : null
      const deadlineTimer = deadlineMs === null
        ? undefined
        : setTimeout(() => {
            GoalEngine.blockGoal(
              activeGoalForDeadline!.id,
              `Blocked after goal wall-clock budget reached (${activeGoalForDeadline!.budget.wallClockBudgetMs} ms).`,
            )
            cancel(sessionID)
          }, deadlineMs)

      // Drop a handoff marker left by an earlier request in this session.
      consumeHandoff(sessionID)
      const result = await processor.process({
        user: lastUser,
        agent,
        abort,
        sessionID,
        system,
        tail,
        messages: [
          ...MessageV2.toModelMessages(answerGuard.required ? answerGuard.context(msgs) : msgs, model),
          ...(isLastStep
            ? [
                {
                  role: "assistant" as const,
                  content: MAX_STEPS,
                },
              ]
            : []),
        ],
        tools,
        model,
        toolChoice: answerGuard.needsRepair ? "none" : format.type === "json_schema" ? "required" : undefined,
        plan,
        surface: promptSurface,
      }).finally(() => {
        if (deadlineTimer !== undefined) clearTimeout(deadlineTimer)
      })

      if (answerGuard.required && (abort.aborted || processor.message.error || guard.trip)) {
        await answerGuard.interrupted(processor.message, abort.aborted ? 'Turn cancelled before answer validation.' : guard.trip ? 'A runtime limit ended the turn before answer validation.' : 'The provider failed before answer validation.');
      }

      // If structured output was captured, save it and exit immediately
      // This takes priority because the StructuredOutput tool was called successfully
      if (structuredOutput !== undefined) {
        processor.message.structured = structuredOutput
        processor.message.finish = processor.message.finish ?? "stop"
        await Session.updateMessage(processor.message)
        break
      }

      // O12: a guardrail trip (tool-call cap, stuck detector) ends the turn,
      // even when a durable goal would otherwise continue.
      if (guard.trip) {
        const goal = GoalEngine.getCurrentGoal(sessionID)
        if (goal?.state === "in_progress") GoalEngine.pauseGoal(goal.id, guard.trip.reason)
        break
      }

      // Allternit Cloud handed this step to an open model itself: say so on the message.
      const cloudHandoff = consumeHandoff(sessionID)
      if (cloudHandoff && !processor.message.handoff) {
        processor.message.handoff = cloudHandoff
        if (!answerGuard.required) await Session.updateMessage(processor.message)
      }

      // Check if model finished (finish reason is not "tool-calls" or "unknown")
      const modelFinished = processor.message.finish && !["tool-calls", "unknown"].includes(processor.message.finish)

      // Refusal handoff for any other brain (the person's own subscription or
      // key): once per turn, top-level sessions only, when the person turned
      // it on in Settings → Open models. Allternit Cloud models hand off
      // server-side (above), so they are skipped here.
      if (
        modelFinished &&
        !refusalHandoff &&
        !answerGuard.needsRepair &&
        !processor.message.error &&
        !abort.aborted &&
        !session.parentID &&
        model.providerID !== CLOUD_PROVIDER_ID
      ) {
        const parts = await MessageV2.parts(processor.message.id)
        const text = answerGuard.required ? answerGuard.text(processor.message.id) : parts
          .filter((p): p is MessageV2.TextPart => p.type === "text" && !p.synthetic)
          .map((p) => p.text)
          .join("\n")
        const calledTools = parts.some((p) => p.type === "tool")
        if (!calledTools && isRefusal(text, processor.message.finish)) {
          const target = await handoffTarget().catch(() => undefined)
          if (target) {
            refusalHandoff = {
              to: target,
              from: `${model.providerID}/${model.id}`,
              refusedID: processor.message.id,
            }
            if (answerGuard.required) await answerGuard.interrupted({ ...processor.message, finish: 'unknown', time: { created: processor.message.time.created } }, 'Refused response handed off before answer validation.');
            log.info("refused turn: handing off to the open lane", {
              sessionID,
              from: refusalHandoff.from,
              to: `${target.providerID}/${target.modelID}`,
            })
            continue
          }
        }
      }

      // Usage limits (wrap-up): a turn that crosses land_at mid-task gets a
      // few steps to finish what it is in the middle of, then lands instead
      // of being cut off by the provider. Child sessions follow their parent.
      if (!session.parentID && !processor.message.error && !abort.aborted) {
        if (wrapUp) {
          wrapUp.left--
        } else {
          const crossed = await SessionLimit.check(session, model.providerID, model.id).catch(() => undefined)
          if (crossed) {
            const { wrapUpSteps } = await SessionLimit.thresholds()
            wrapUp = { left: wrapUpSteps, done: false, providerID: model.providerID, window: crossed }
            SessionLimit.set(sessionID, SessionLimit.snapshot("wrapping_up", model.providerID, crossed, crossed.source), session.limit)
            log.info("usage limit reached; wrapping up", { sessionID, window: crossed.id, usedRatio: crossed.usedRatio, steps: wrapUpSteps })
          }
        }
        if (wrapUp && (modelFinished || wrapUp.left <= 0)) {
          if (answerGuard.required && !processor.message.error && !abort.aborted) {
            if (modelFinished || answerGuard.needsRepair) await answerGuard.complete(processor.message, false);
            else await answerGuard.interrupted(processor.message, 'Usage limit ended the turn before answer validation.');
          }
          wrapUp.done = !processor.message.error
          break
        }
      }

      if (answerGuard.required && (modelFinished || answerGuard.needsRepair) && !processor.message.error && !abort.aborted) {
        const outcome = await answerGuard.complete(processor.message);
        if (outcome === 'repairing') continue;
        if (outcome === 'rejected') break;
      }

      if (processor.message.error) {
        await HookDispatcher.emit({
          name: "StopFailure",
          timestamp: Date.now(),
          sessionId: sessionID,
          payload: { error: processor.message.error.name ?? "Unknown", finish: processor.message.finish },
        })
      }

      if (modelFinished && !processor.message.error) {
        const stopped = await HookDispatcher.emit({
          name: "Stop",
          timestamp: Date.now(),
          sessionId: sessionID,
          payload: { finish: processor.message.finish },
        })
        if (stopped.decision === "deny") {
          const continuation = await Session.updateMessage({
            id: Identifier.ascending("message"),
            sessionID,
            role: "user",
            time: { created: Date.now() },
            agent: lastUser.agent,
            model: lastUser.model,
          } as MessageV2.User)
          await Session.updatePart({
            id: Identifier.ascending("part"),
            messageID: continuation.id,
            sessionID,
            type: "text",
            synthetic: true,
            text: `<hook_result hook_event="Stop">\n${stopped.reason ?? stopped.message ?? "A stop hook requested another step."}\n</hook_result>`,
          } satisfies MessageV2.TextPart)
          continue
        }
        // Allternit Sleep: the turn is done; idle after the prompt-cache TTL
        // (native Anthropic requests here use the 5-minute ephemeral cache,
        // see processor.ts). Fire-and-forget; never blocks the turn.
        if (!session.parentID) SleepTriggers.turnEnded(sessionID, PROMPT_CACHE_TTL_S)
        if (format.type === "json_schema") {
          // Model stopped without calling StructuredOutput tool
          processor.message.error = new MessageV2.StructuredOutputError({
            message: "Model did not produce structured output",
            retries: 0,
          }).toObject()
          await Session.updateMessage(processor.message)
        }
        // An active goal owns the turn boundary. Ordinary model completion is
        // not a durable goal outcome: account the turn once, enforce budgets,
        // then synthesize the next continuation. update_goal may already have
        // accounted or terminated this turn, so compare durable usage first.
        let currentGoal = goalAtTurnStart
          ? GoalEngine.getGoal(goalAtTurnStart.id)
          : GoalEngine.getCurrentGoal(sessionID)
        if (currentGoal?.state === "in_progress") {
          const baseline = goalTurnsAtStart ?? currentGoal.usage.turnsUsed
          if (currentGoal.usage.turnsUsed === baseline) {
            const accounted = GoalEngine.recordContinuation(currentGoal.id, {
              tokensUsed: processor.message.tokens.output,
            })
            currentGoal = GoalEngine.getGoal(currentGoal.id)
            if (!accounted.ok) log.warn("failed to account goal continuation", { goalID: currentGoal?.id, reason: accounted.reason })
          }
        }

        // Completion can promote a queued goal in the same transaction.
        const nextGoal = GoalEngine.getCurrentGoal(sessionID)
        if (nextGoal?.state === "in_progress") {
          if (isLastStep) {
            GoalEngine.pauseGoal(nextGoal.id, "Paused after reaching the agent step ceiling.")
            break
          }
          // Round boundary: project checks + independent review feed the next
          // round; two rounds with the same failures escalate the model
          // (models.jobs.goalEscalate). Budgets were enforced above.
          const roundChecks = await import("@/runtime/review/goal-checks")
            .then(({ GoalRoundChecks }) => GoalRoundChecks.run({ sessionID, cwd: Instance.directory, abort }))
            .catch((error) => {
              log.warn("goal round checks failed", { sessionID, error })
              return undefined
            })
          if (abort.aborted) break
          const continuationUser: MessageV2.User = {
            id: Identifier.ascending("message"),
            sessionID,
            role: "user",
            time: { created: Date.now() },
            agent: lastUser.agent,
            model: roundChecks?.escalate ?? lastUser.model,
          }
          await Session.updateMessage(continuationUser)
          await Session.updatePart({
            id: Identifier.ascending("part"),
            messageID: continuationUser.id,
            sessionID,
            type: "text",
            text: [
              "Continue the active durable goal. Choose the next bounded useful slice, preserve evidence, and use update_goal only for progress accounting or an audited terminal outcome.",
              roundChecks?.feedback,
            ].filter(Boolean).join("\n\n"),
            synthetic: true,
          } satisfies MessageV2.TextPart)
          // New goal slice: fresh guardrail counters and wall clock.
          guard.reset()
          turnStepBase = step
          armWallClock(guard)
          continue
        }

        // No active goal remains.
        const running = state()[sessionID]
        if (running && running.queuedTurns > 0) {
          // A user steer or background completion arrived while this turn was
          // active. The message is already durable; drive another turn before
          // resolving the queued callers and releasing the session.
          running.queuedTurns = 0
          continue
        }
        break
      }

      if (result === "stop") break
      if (result === "compact") {
        await SessionCompaction.create({
          sessionID,
          agent: lastUser.agent,
          model: lastUser.model,
          auto: true,
        })
      }
      continue
    }
    if (wrapUp) {
      // Land: pause until the window resets. "wrapped" when the turn got to
      // finish cleanly; "paused" when it was stopped (error, abort) mid wrap-up.
      const { window } = wrapUp
      SessionPause.pause(
        sessionID,
        { until: Date.parse(window.resetAt), limit: `${window.source} ${window.label.toLowerCase()} limit`, providerID: wrapUp.providerID, reason: "quota" },
        { state: wrapUp.done ? "wrapped" : "paused", window },
      )
    }
    SessionCompaction.prune({ sessionID })
    // WP-R1 (O14): label the turn's ROUTE decision from what it actually did.
    if (TurnRouter.current(sessionID)) {
      void (async () => {
        const tools: string[] = []
        let errored = false
        for await (const item of MessageV2.stream(sessionID)) {
          if (item.info.role === "user") break
          if ((item.info as MessageV2.Assistant).error) errored = true
          for (const p of item.parts) if (p.type === "tool") tools.push((p as MessageV2.ToolPart).tool)
        }
        await TurnRouter.finishTurn(sessionID, { tools, errored })
      })().catch(() => {})
    }
    for await (const item of MessageV2.stream(sessionID)) {
      if (item.info.role === "user") continue
      const queued = state()[sessionID]?.callbacks ?? []
      for (const q of queued) {
        q.resolve(item)
      }
      return item
    }
    throw new Error("Impossible")
  }

  async function lastModel(sessionID: string) {
    const session = await Session.get(sessionID)
    if (session.defaultModel) return session.defaultModel
    for await (const item of MessageV2.stream(sessionID)) {
      if (item.info.role === "user" && item.info.model) return item.info.model
    }
    return Provider.defaultModel()
  }

  /** @internal Exported for testing */
  export async function resolveTools(input: {
    agent: Agent.Info
    model: Provider.Model
    session: Session.Info
    tools?: Record<string, boolean>
    metadata?: Record<string, unknown>
    processor: SessionProcessor.Info
    bypassAgentCheck: boolean
    messages: MessageV2.WithParts[]
  }) {
    using _ = log.time("resolveTools")
    const tools: Record<string, AITool> = {}

    // Per-message mobile composer tool options (forwarded by the bridge as
    // `metadata.tools`). Turn-scoped only: nothing here is persisted to the
    // session's permission ruleset.
    const mobileOptions = parseMobileToolOptions(input.metadata)
    const permissionModeOverride = mobileToolAccessMode(mobileOptions?.toolAccess)

    const context = (args: Record<string, unknown>, options: ToolCallOptions): Tool.Context => ({
      sessionID: input.session.id,
      abort: options.abortSignal!,
      messageID: input.processor.message.id,
      callID: options.toolCallId,
      extra: { model: input.model, bypassAgentCheck: input.bypassAgentCheck, location: TurnLocations.get(input.session.id) },
      agent: input.agent.name,
      messages: input.messages,
      metadata: async (val: { title?: string; metadata?: any }) => {
        const match = input.processor.partFromToolCall(options.toolCallId)
        if (match && match.state.status === "running") {
          await Session.updatePart({
            ...match,
            state: {
              title: val.title,
              metadata: val.metadata,
              status: "running",
              input: args,
              time: {
                start: Date.now(),
              },
            },
          })
        }
      },
      async ask(req) {
        return await PermissionNext.ask({
          ...req,
          sessionID: input.session.id,
          tool: { messageID: input.processor.message.id, callID: options.toolCallId },
          ruleset: PermissionNext.merge(input.agent.permission, input.session.permission ?? []),
          // Per-message permission-mode override from the mobile composer's
          // `toolAccess` option. Applies to permission checks made through this
          // turn's tool contexts only; the session mode is untouched.
          ...(permissionModeOverride ? { mode: permissionModeOverride } : {}),
        })
      },
    })

    for (const item of await ToolRegistry.tools(
      { modelID: input.model.api.id, providerID: input.model.providerID, npm: input.model.api.npm },
      input.agent,
      input.session.id,
      { computer: mobileOptions?.computer === true },
    )) {
      const schema = ProviderTransform.schema(input.model, z.toJSONSchema(item.parameters) as any)
      tools[item.id] = tool({
        id: item.id as any,
        description: item.description,
        inputSchema: jsonSchema(schema as any),
        async execute(args, options) {
          const ctx = context(args, options)
          await Plugin.trigger(
            "tool.execute.before",
            {
              tool: item.id,
              sessionID: ctx.sessionID,
              callID: ctx.callID,
            },
            {
              args,
            },
          )
          // WP9: Tool Call Compiler, shadow mode behind GIZZI_KERNEL_COMPILERS
          // (off by default → plain pass-through to the existing call).
          const result = await KernelTurn.withToolReceipt(
            {
              sessionID: ctx.sessionID,
              callID: ctx.callID,
              tool: item.id,
              args,
              directory: input.session.directory,
              description: item.description,
              schema: () => z.toJSONSchema(item.parameters),
            },
            () =>
              ToolDedupe.execute({
                sessionID: ctx.sessionID,
                messageID: ctx.messageID,
                tool: item.id,
                args,
                run: () => ToolDispatcher.executeInitialized(item.id, args, ctx, item.execute),
              }),
          )
          // Cloud environment secrets are hidden before the model sees output.
          const output = {
            ...SecretRedactor.deep(result),
            attachments: result.attachments?.map((attachment) => ({
              ...attachment,
              id: Identifier.ascending("part"),
              sessionID: ctx.sessionID,
              messageID: input.processor.message.id,
            })),
          }
          await Plugin.trigger(
            "tool.execute.after",
            {
              tool: item.id,
              sessionID: ctx.sessionID,
              callID: ctx.callID,
              args,
            },
            output,
          )
          return output
        },
      })
    }

    const mcpCatalog = await mcpToolCatalog(input.session.id)
    const mcpTools = Object.entries(mcpCatalog.tools)
    // A bot's Capabilities page: MCP connectors it has off are not offered to it.
    const turnBot = BotTurn.get(input.session.id)
    // Wave D decision 10: more than Connectors.DEFER_THRESHOLD connector tools
    // mount none until connector_search loads them (non-connector MCP tools
    // always mount). Loaded tools keep their own permission rules below.
    const connectorMounted = Connectors.mountFilter(input.session.id, mcpCatalog.descriptors)
    for (const [key, item] of mcpTools) {
      if (!connectorMounted(key)) continue
      if (!BotTurn.mcpOnForBot(turnBot, mcpCatalog.descriptors[key]?.connectorId)) continue
      const execute = item.execute
      if (!execute) continue

      const schemaResult = asSchema(item.inputSchema)
      const jsonSchemaValue = await Promise.resolve(schemaResult as unknown as PromiseLike<{ jsonSchema: any }>)
      const transformed = ProviderTransform.schema(input.model, jsonSchemaValue.jsonSchema)
      item.inputSchema = jsonSchema(transformed)
      // Wrap execute to add plugin hooks and format output
      item.execute = async (args: Record<string, unknown>, opts: ToolCallOptions) => {
        const ctx = context(args, opts)

        await Plugin.trigger(
          "tool.execute.before",
          {
            tool: key,
            sessionID: ctx.sessionID,
            callID: opts.toolCallId,
          },
          {
            args,
          },
        )

        // Composer + menu → Connectors: a connector turned off for this
        // session can't be used, even though the dispatcher tool is loaded.
        const offConnector = disabledConnectorFor(mobileOptions, args)
        if (offConnector) {
          throw new Error(
            `The ${offConnector} connector is turned off for this session. Tell the user it's off; they can turn it on from the + menu → Connectors.`,
          )
        }
        // A bot's Capabilities page: a catalog connector it has off can't be used by it.
        const botApp = connectorAppOf(args)
        if (botApp && !BotTurn.connectorOnForBot(turnBot, botApp)) {
          throw new Error(
            `The ${botApp} connector is off for this bot. Tell the user; they can turn it on from the bot's page → Capabilities.`,
          )
        }

        // A tool its server marks destructive still asks in an app session's
        // auto mode (PermissionNext.evaluatePolicy).
        const descriptor = mcpCatalog.descriptors[key]
        await ctx.ask({
          permission: key,
          metadata: descriptor?.destructive ? { destructive: true } : {},
          patterns: ["*"],
          always: ["*"],
        })

        // Model-initiated call to a tool the proxy marked as needing the user's approval under the
        // app's install permission mode: ask first (never remembered), then let the call carry the
        // approval flag. Declined → tool error.
        if (descriptor?.requiresConfirmation) {
          await McpUserProxy.gate({
            callID: opts.toolCallId,
            tool: descriptor.originalName ? McpUserProxy.originalToolName(descriptor.originalName) : key,
            title: descriptor.title,
            app: descriptor.appName,
            args,
            ask: ctx.ask,
          })
        }

        let result: CallToolResult & { metadata?: Record<string, unknown> }
        try {
          result = await ToolDedupe.execute<CallToolResult & { metadata?: Record<string, unknown> }>({
            sessionID: ctx.sessionID,
            messageID: ctx.messageID,
            tool: key,
            args,
            // The session id rides along so Allternit's own MCP tools know which session called.
            run: () =>
              ToolDispatcher.executeInitialized(key, args, ctx, (nextArgs) =>
                runWithStreamContext({ sessionID: ctx.sessionID }, () => execute(nextArgs, opts)),
              ),
          })
        } finally {
          McpUserProxy.clearApproval(opts.toolCallId)
        }

        await Plugin.trigger(
          "tool.execute.after",
          {
            tool: key,
            sessionID: ctx.sessionID,
            callID: opts.toolCallId,
            args,
          },
          result,
        )

        const textParts: string[] = []
        const attachments: Omit<MessageV2.FilePart, "id" | "sessionID" | "messageID">[] = []

        for (const contentItem of result.content) {
          if (contentItem.type === "text") {
            textParts.push(contentItem.text)
          } else if (contentItem.type === "image") {
            attachments.push({
              type: "file",
              mime: contentItem.mimeType,
              url: `data:${contentItem.mimeType};base64,${contentItem.data}`,
            })
          } else if (contentItem.type === "resource") {
            const { resource } = contentItem
            const res = resource as { uri: string; text?: string; blob?: string; mimeType?: string }
            if (res.text) {
              textParts.push(res.text)
            }
            if (res.blob) {
              attachments.push({
                type: "file",
                mime: res.mimeType ?? "application/octet-stream",
                url: `data:${res.mimeType ?? "application/octet-stream"};base64,${res.blob}`,
                filename: res.uri,
              })
            }
          }
        }

        const truncated = await Truncate.output(SecretRedactor.text(textParts.join("\n\n")), {}, input.agent)
        const metadata = {
          ...(result.metadata ?? {}),
          // MCP Apps: the host needs the untouched result (structuredContent, _meta) and the
          // originating server/tool to render a ui:// resource; the model never sees this.
          ...mcpAppMetadata(mcpCatalog.descriptors[key], result),
          truncated: truncated.truncated,
          ...(truncated.truncated && { outputPath: truncated.outputPath }),
        }

        return {
          title: "",
          metadata,
          output: truncated.content,
          attachments: attachments.map((attachment) => ({
            ...attachment,
            id: Identifier.ascending("part"),
            sessionID: ctx.sessionID,
            messageID: input.processor.message.id,
          })),
          // MCP tools may provide their own toModelOutput implementation that
          // reads `content` instead of our normalized `output`. Returning the
          // original text here bypassed spill-to-disk truncation and could put
          // the entire oversized result back into the model context. Once a
          // result spills, expose one bounded text part and preserve only
          // non-text media/resource parts; the complete text remains at
          // metadata.outputPath.
          content: SecretRedactor.deep(Truncate.modelContent(result.content as Truncate.ModelContentItem[], truncated)),
        }
      }
      tools[key] = item
    }

    // Self-hosted Agents Server: the agent's custom tools (metadata.selfhost.custom_tools).
    await addSelfhostCustomTools(tools, { ...input, context, mcpCatalog })

    // Per-message mobile composer gating (metadata.tools) — applied last so it
    // wins over agent/config tool assembly for this turn only.
    if (mobileOptions) {
      applyMobileToolGating(tools, mobileOptions)
      log.info("mobile tool gating applied", {
        sessionID: input.session.id,
        options: mobileOptions,
        websearch: "websearch" in tools,
        task: "task" in tools,
      })
    }

    // Bot Mode (B4/D5): message_agent exists only in canonical bot chats.
    // Same gate-and-delete precedent as mobile gating above — non-canonical
    // sessions (regular chats, subagents, coordinator workers) never see it.
    if (MessageAgentTool.id in tools) {
      const allowed = await isMessageAgentSession(input.session.id).catch((error) => {
        log.warn("message_agent gating check failed; hiding tool", { sessionID: input.session.id, error })
        return false
      })
      if (!allowed) delete tools[MessageAgentTool.id]
    }

    // suggest_task: only an app bot's own chat can suggest work to its user.
    if (SuggestTaskTool.id in tools) {
      const allowed = await isAppBotSession(input.session.id).catch(() => false)
      if (!allowed) delete tools[SuggestTaskTool.id]
    }

    // grant_task_permission: only an app bot's chat has a thread to grant for.
    if (GrantTaskPermissionTool.id in tools) {
      const allowed = await isAppBotSession(input.session.id).catch(() => false)
      if (!allowed) delete tools[GrantTaskPermissionTool.id]
    }

    // thread_update / thread_close: only an app bot's chat steers its own thread.
    // The past-chat tools (thread_search, thread_read, thread_recent, thread_wait)
    // work in every session, CLI included (Primitives Wave D, decision 8).
    if (ToolRegistry.BOT_THREAD_TOOL_IDS.some((id) => id in tools)) {
      const allowed = await isAppBotSession(input.session.id).catch(() => false)
      if (!allowed) for (const id of ToolRegistry.BOT_THREAD_TOOL_IDS) delete tools[id]
    }

    // bot_create / vendor_connect: only an app bot's chat has an owner to act for.
    if (WORKER_BOT_TOOL_IDS.some((id) => id in tools)) {
      const allowed = await isAppBotSession(input.session.id).catch(() => false)
      if (!allowed) for (const id of WORKER_BOT_TOOL_IDS) delete tools[id]
    }

    // project_list / project_send: only a bot's main chat works across projects.
    if (PROJECT_SEND_TOOL_IDS.some((id) => id in tools)) {
      const allowed = await isAppBotMainChat(input.session.id).catch(() => false)
      if (!allowed) for (const id of PROJECT_SEND_TOOL_IDS) delete tools[id]
    }

    // end_conversation (D5): Gizzi's own chats only. Never a bot turn (a
    // person's bot, bots on customer channels included), never a child
    // session (the parent's chat is not its to end), never an ended chat.
    if (END_CONVERSATION_TOOL_ID in tools) {
      if (BotTurn.get(input.session.id) || input.session.parentID || input.session.ended) {
        delete tools[END_CONVERSATION_TOOL_ID]
      }
    }

    // Session rules (deny `*`, then allow a few) only gate execution elsewhere; drop the
    // denied tools here so their schemas are not sent on every model call.
    if (input.session.permission?.length) {
      const ruleset = PermissionNext.merge(input.agent.permission, input.session.permission)
      const keys = Object.keys(tools).filter((k) => k !== "invalid")
      for (const key of PermissionNext.hidden(keys, ruleset)) delete tools[key]
    }
    // Primitives Wave B: deferred tool families (built-in groups and MCP tools)
    // stay out until tool_search or a call by name loads them; applied last so
    // it only narrows what every gate above allowed. GIZZI_DISABLE_TOOL_SEARCH
    // turns it off (primitives/tool-families.ts).
    const deferral = ToolFamilies.filter({ sessionID: input.session.id, tools, messages: input.messages })
    for (const key of Object.keys(tools)) if (!(key in deferral.tools)) delete tools[key]
    const schemaBytes = Object.values(tools).reduce((n, t) => {
      try {
        return n + JSON.stringify((t as any).inputSchema?.jsonSchema ?? (t as any).parameters ?? {}).length
      } catch {
        return n
      }
    }, 0)
    log.info("resolved tools", {
      count: Object.keys(tools).length,
      deferred: deferral.deferred.length,
      schemaBytes,
      sessionRules: input.session.permission?.length ?? 0,
    })

    return tools
  }

  type McpCatalog = Awaited<ReturnType<typeof MCP.toolCatalog>>

  /**
   * The session's MCP tools, named as the model gets them: the instance's
   * connected servers plus the user's connectors through allternit-api's
   * per-user proxy (registered for the current turn only). resolveTools offers
   * these; reservedToolNames keeps custom tools off their names.
   */
  export async function mcpToolCatalog(sessionID: string): Promise<McpCatalog> {
    const proxyClient = await McpUserProxy.client(sessionID)
    const proxyEntry = McpUserProxy.current(sessionID)
    return MCP.toolCatalog(proxyClient && proxyEntry ? { [proxyEntry.server]: proxyClient } : {})
  }

  /**
   * Every name a self-hosted custom tool of the session may not take, with
   * what holds it: the tools already offered this step (`tools`, resolveTools),
   * every registered built-in, StructuredOutput, and every tool of the
   * session's MCP catalog (all of it, not only the tools loaded this step, so
   * dynamic tool selection cannot load a clash later in the turn).
   *
   * `mcpProxy`: the turn's per-user proxy entry when it is not registered yet
   * (prompt() checks before it registers the proxy; the check route has no
   * turn). It is held only while the catalog is read.
   */
  export async function reservedToolNames(input: {
    sessionID: string
    tools?: Record<string, AITool>
    mcpCatalog?: McpCatalog
    mcpProxy?: unknown
  }): Promise<RemoteToolHost.ReservedTool[]> {
    let catalog = input.mcpCatalog
    if (!catalog) {
      const release =
        input.mcpProxy !== undefined && !McpUserProxy.current(input.sessionID)
          ? McpUserProxy.register(input.sessionID, input.mcpProxy)
          : () => {}
      try {
        catalog = await mcpToolCatalog(input.sessionID)
      } finally {
        release()
      }
    }
    const descriptors = catalog.descriptors
    const mcp: RemoteToolHost.ReservedTool[] = Object.values(descriptors).map((d) => ({
      kind: "mcp",
      name: d.qualifiedName,
      server: d.serverName,
      tool: d.originalName,
    }))
    const builtins = [
      ...Object.keys(input.tools ?? {}).filter((name) => !(name in descriptors)),
      ...(await ToolRegistry.ids({ computer: true })),
      "StructuredOutput",
    ]
    return [...builtins, ...mcp]
  }

  /**
   * Offer the agent's custom tools to the model (self-hosted Agents Server,
   * `GIZZI_TOOL_HOST=remote`). The server sends them in every turn's
   * `metadata.selfhost.custom_tools`; they become this session's custom tools
   * (replacing the last turn's), and a call to one goes through the remote
   * tool host to the server's queue, where an SDK worker runs it. A custom
   * tool named like a built-in, MCP or sandbox tool is refused: the turn
   * fails with a `custom_tool_conflict` error. A subagent's child session gets
   * its root session's custom tools, and its calls go to the root's queue.
   *
   * @internal Exported for testing
   */
  export async function addSelfhostCustomTools(
    tools: Record<string, AITool>,
    input: {
      session: Session.Info
      model: Provider.Model
      agent: Agent.Info
      metadata?: Record<string, unknown>
      context: (args: Record<string, unknown>, options: ToolCallOptions) => Tool.Context
      /** The session's MCP catalog when the caller already read it (resolveTools). */
      mcpCatalog?: McpCatalog
    },
  ) {
    if (!RemoteToolHost.enabled()) return
    let custom: RemoteToolHost.CustomTool[]
    RemoteToolHost.linkSession(input.session.id, input.session.parentID)
    if (input.session.parentID) {
      // A subagent (task, agent_swarm) runs in a child session the server does
      // not know: its turns carry no metadata, and its calls go to the root's
      // queue. It gets the root's custom tools; the root checked them against
      // the same registry and MCP catalog, and a name the child already has
      // stays the child's. The child's disable map and agent permissions drop
      // them like any built-in (LLM.resolveTools).
      custom = (await RemoteToolHost.customToolsFor(input.session.id)).filter((def) => {
        if (!(def.name in tools)) return true
        log.warn("custom tool not offered to the subagent: the name is taken", {
          sessionID: input.session.id,
          tool: def.name,
        })
        return false
      })
    } else {
      custom = RemoteToolHost.customToolsFrom(input.metadata)
      const reserved = custom.length
        ? await reservedToolNames({ sessionID: input.session.id, tools, mcpCatalog: input.mcpCatalog })
        : []
      RemoteToolHost.registerCustomTools(input.session.id, custom, reserved)
    }
    for (const def of custom) {
      tools[def.name] = tool({
        id: def.name as any,
        description: def.description,
        inputSchema: jsonSchema(ProviderTransform.schema(input.model, def.inputSchema as any) as any),
        async execute(args: Record<string, unknown>, options: ToolCallOptions) {
          const ctx = input.context(args, options)
          await Plugin.trigger(
            "tool.execute.before",
            { tool: def.name, sessionID: ctx.sessionID, callID: ctx.callID },
            { args },
          )
          const result = await ToolDedupe.execute({
            sessionID: ctx.sessionID,
            messageID: ctx.messageID,
            tool: def.name,
            args,
            // The dispatcher forwards it too (RemoteToolHost.handles); it never runs here.
            run: () =>
              ToolDispatcher.executeInitialized(def.name, args, ctx, (nextArgs, nextCtx) =>
                RemoteToolHost.execute(def.name, nextArgs, nextCtx),
              ),
          })
          const truncated = await Truncate.output(SecretRedactor.text(result.output), {}, input.agent)
          const output = {
            title: result.title,
            metadata: {
              ...SecretRedactor.deep(result.metadata),
              custom: true,
              truncated: truncated.truncated,
              ...(truncated.truncated && { outputPath: truncated.outputPath }),
            },
            output: truncated.content,
          }
          await Plugin.trigger(
            "tool.execute.after",
            { tool: def.name, sessionID: ctx.sessionID, callID: ctx.callID, args },
            output,
          )
          return output
        },
      })
    }
  }

  const MobileTools = z.object({
    webSearch: z.boolean().optional(),
    research: z.boolean().optional(),
    toolAccess: z.enum(["auto", "on_demand", "always"]).optional(),
    disabledConnectors: z.array(z.string()).optional(),
    computer: z.boolean().optional(),
  })

  export interface MobileToolOptions {
    webSearch?: boolean
    research?: boolean
    toolAccess?: "auto" | "on_demand" | "always"
    /** Connector ids (open-connector app ids, e.g. "github") the user turned
     * off for this session in the composer's + menu. */
    disabledConnectors?: string[]
    /** Computer toolset on for this turn only (contracts/pet-offers
     * `turnOptions.computer`), even when GIZZI_ENABLE_COMPUTER_TOOL is unset.
     * Never persisted; the server's lease and confirm gates still apply. */
    computer?: boolean
  }

  /**
   * The connector a tool call would use when the user turned it off for this
   * turn, else undefined. Connector actions go through the connector MCP's
   * dispatcher tools with an `actionId` of `<app>.<action>`
   * (services/open-connector/src/mcp.ts).
   *
   * @internal Exported for testing
   */
  export function connectorAppOf(args: unknown): string | undefined {
    if (!args || typeof args !== "object") return undefined
    const actionId = (args as { actionId?: unknown }).actionId
    return typeof actionId === "string" ? actionId.split(".")[0]?.toLowerCase() || undefined : undefined
  }

  export function disabledConnectorFor(options: MobileToolOptions | undefined, args: unknown): string | undefined {
    const disabled = options?.disabledConnectors
    if (!disabled?.length || !args || typeof args !== "object") return undefined
    const actionId = (args as { actionId?: unknown }).actionId
    if (typeof actionId !== "string") return undefined
    const app = actionId.split(".")[0]?.toLowerCase()
    return disabled.find((id) => id.toLowerCase() === app)
  }

  /**
   * Parse the mobile composer's tool options out of prompt `metadata`
   * (forwarded by the Rust bridge as `metadata.tools`). Returns undefined when
   * absent or malformed — malformed input must never break a prompt.
   *
   * @internal Exported for testing
   */
  export function parseMobileToolOptions(
    metadata: Record<string, unknown> | undefined,
  ): MobileToolOptions | undefined {
    const parsed = MobileTools.safeParse(metadata?.["tools"])
    return parsed.success ? parsed.data : undefined
  }

  /**
   * Remove tools for this turn based on the mobile composer's options.
   *
   * Mapping decision: there is no dedicated `research` tool. `research: false`
   * maps to the closest deep-research capabilities — the `websearch` tool
   * (whose `type: "deep"` mode is the deep-research vehicle) and the `task`
   * subagent tool. `webSearch: false` removes only `websearch`. `true` values
   * (or absent keys) leave the default tool set unchanged.
   *
   * @internal Exported for testing
   */
  export function applyMobileToolGating(tools: Record<string, AITool>, options: MobileToolOptions): void {
    if (options.webSearch === false || options.research === false) delete tools["websearch"]
    if (options.research === false) delete tools["task"]
  }

  /**
   * Map the mobile composer's `toolAccess` to a PermissionNext mode applied as
   * a per-message override for this turn (never persisted):
   *   auto      -> "auto"              (unattended: auto-allow, no questions)
   *   on_demand -> "default"           (normal ask behavior)
   *   always    -> "bypassPermissions" (allow everything without asking)
   */
  function mobileToolAccessMode(
    toolAccess: MobileToolOptions["toolAccess"],
  ): PermissionNext.Mode | undefined {
    switch (toolAccess) {
      case "auto":
        return "auto"
      case "on_demand":
        return "default"
      case "always":
        return "bypassPermissions"
      default:
        return undefined
    }
  }

  /** @internal Exported for testing */
  export function createStructuredOutputTool(input: {
    schema: Record<string, any>
    onSuccess: (output: unknown) => void
  }): AITool {
    // Remove $schema property if present (not needed for tool input)
    const { $schema, ...toolSchema } = input.schema

    return tool({
      id: "StructuredOutput" as any,
      description: STRUCTURED_OUTPUT_DESCRIPTION,
      inputSchema: jsonSchema(toolSchema as any),
      async execute(args) {
        // AI SDK validates args against inputSchema before calling execute()
        input.onSuccess(args)
        return {
          output: "Structured output captured successfully.",
          title: "Structured Output",
          metadata: { valid: true },
        }
      },
      toModelOutput(result) {
        return {
          type: "text",
          value: result.output,
        }
      },
    })
  }

  async function createUserMessage(input: PromptInput, directory?: string) {
    const agent = await Agent.get(input.agent ?? (await Agent.defaultAgent()))
    if (!agent) {
      throw new Error(`Agent not found: ${input.agent}`)
    }

    const model = input.model ?? agent.model ?? (await lastModel(input.sessionID))
    const full =
      !input.variant && agent.variant
        ? await Provider.getModel(model.providerID, model.modelID).catch(() => undefined)
        : undefined
    const variant = input.variant ?? (agent.variant && full?.variants?.[agent.variant] ? agent.variant : undefined)

    const info: MessageV2.Info = {
      id: input.messageID ?? Identifier.ascending("message"),
      role: "user",
      sessionID: input.sessionID,
      time: {
        created: Date.now(),
      },
      tools: input.tools,
      // Bridge-forwarded provider routing rides the per-turn metadata (same
      // vehicle as `service_tier`); never persisted on the session.
      metadata:
        input.provider || input.provider_credentials
          ? {
              ...input.metadata,
              ...(input.provider ? { provider_routing: input.provider } : {}),
              ...(input.provider_credentials
                ? { provider_routing_credentials: input.provider_credentials }
                : {}),
            }
          : input.metadata,
      agent: agent.name,
      model,
      system: input.system,
      format: input.format,
      variant,
      ...(input.bot ? { bot: input.bot } : {}),
    }
    using _ = defer(() => InstructionPrompt.clear(info.id))

    type Draft<T> = T extends MessageV2.Part ? Omit<T, "id"> & { id?: string } : never
    const assign = (part: Draft<MessageV2.Part>): MessageV2.Part => ({
      ...part,
      id: part.id ?? Identifier.ascending("part"),
    })

    // The person's own text, to tell which attachments the composer already inlined.
    const userText = input.parts
      .flatMap((p) => (p.type === "text" && !p.synthetic ? [p.text] : []))
      .join("\n")
    const parts = await Promise.all(
      input.parts.map(async (part): Promise<Draft<MessageV2.Part>[]> => {
        if (part.type === "file") {
          const partFilename = part.filename
          // before checking the protocol we check if this is an mcp resource because it needs special handling
          if (part.source?.type === "resource") {
            const { clientName, uri } = part.source
            log.info("mcp resource", { clientName, uri, mime: part.mime })

            const pieces: Draft<MessageV2.Part>[] = [
              {
                messageID: info.id,
                sessionID: input.sessionID,
                type: "text",
                synthetic: true,
                text: `Reading MCP resource: ${part.filename} (${uri})`,
              },
            ]

            try {
              const resourceContent = await MCP.readResource(clientName, uri)
              if (!resourceContent) {
                throw new Error(`Resource not found: ${clientName}/${uri}`)
              }

              // Handle different content types
              const contents = Array.isArray(resourceContent.contents)
                ? resourceContent.contents
                : [resourceContent.contents]

              for (const content of contents) {
                if ("text" in content && content.text) {
                  pieces.push({
                    messageID: info.id,
                    sessionID: input.sessionID,
                    type: "text",
                    synthetic: true,
                    text: content.text as string,
                  })
                } else if ("blob" in content && content.blob) {
                  // Handle binary content if needed
                  const mimeType = "mimeType" in content ? content.mimeType : part.mime
                  pieces.push({
                    messageID: info.id,
                    sessionID: input.sessionID,
                    type: "text",
                    synthetic: true,
                    text: `[Binary content: ${mimeType}]`,
                  })
                }
              }

              pieces.push({
                ...part,
                messageID: info.id,
                sessionID: input.sessionID,
              })
            } catch (error: unknown) {
              log.error("failed to read MCP resource", { error, clientName, uri })
              const message = error instanceof Error ? error.message : String(error)
              pieces.push({
                messageID: info.id,
                sessionID: input.sessionID,
                type: "text",
                synthetic: true,
                text: `Failed to read MCP resource ${part.filename}: ${message}`,
              })
            }

            return pieces
          }
          if (isRelativeUrl(part.url)) {
            // An upload path from allternit-api: fetch it now (see upload-part.ts).
            const resolved = await resolveUploadUrl(part.url, part.mime)
            if (!resolved.ok) {
              log.warn("attachment could not be loaded", { filename: part.filename, reason: resolved.reason })
              return [
                {
                  messageID: info.id,
                  sessionID: input.sessionID,
                  type: "text",
                  synthetic: true,
                  text: `The attachment ${part.filename ?? "file"} could not be loaded (${resolved.reason}).`,
                },
              ]
            }
            part = { ...part, url: resolved.url }
          }
          let url: URL
          try {
            url = new URL(part.url)
          } catch {
            log.warn("attachment url is not a valid URL", { filename: part.filename })
            return [
              {
                messageID: info.id,
                sessionID: input.sessionID,
                type: "text",
                synthetic: true,
                text: `The attachment ${part.filename ?? "file"} has an invalid URL and was skipped.`,
              },
            ]
          }
          switch (url.protocol) {
            case "data:":
              if (part.mime === "text/plain") {
                return [
                  {
                    messageID: info.id,
                    sessionID: input.sessionID,
                    type: "text",
                    synthetic: true,
                    text: `Called the Read tool with the following input: ${JSON.stringify({ filePath: part.filename })}`,
                  },
                  {
                    messageID: info.id,
                    sessionID: input.sessionID,
                    type: "text",
                    synthetic: true,
                    // Capped like every data file (MessageV2.dataFileText): a big file is a note, not the whole text.
                    text: MessageV2.dataFileText(part, userText, { bare: true }),
                  },
                  {
                    ...part,
                    messageID: info.id,
                    sessionID: input.sessionID,
                  },
                ]
              }
              break
            case "file:":
              log.info("file", { mime: part.mime })
              // have to normalize, symbol search returns absolute paths
              // Decode the pathname since URL constructor doesn't automatically decode it
              const filepath = fileURLToPath(part.url)
              const s = Filesystem.stat(filepath)

              if (s?.isDirectory()) {
                part.mime = "application/x-directory"
              }

              if (part.mime === "text/plain") {
                let offset: number | undefined = undefined
                let limit: number | undefined = undefined
                const range = {
                  start: url.searchParams.get("start"),
                  end: url.searchParams.get("end"),
                }
                if (range.start != null) {
                  const filePathURI = part.url.split("?")[0]
                  let start = parseInt(range.start)
                  let end = range.end ? parseInt(range.end) : undefined
                  // some LSP servers (eg, gopls) don't give full range in
                  // workspace/symbol searches, so we'll try to find the
                  // symbol in the document to get the full range
                  if (start === end) {
                    const symbols = await LSP.documentSymbol(filePathURI).catch(() => [])
                    for (const symbol of symbols) {
                      let range: LSP.Range | undefined
                      if ("range" in symbol) {
                        range = symbol.range
                      } else if ("location" in symbol) {
                        range = symbol.location.range
                      }
                      if (range?.start?.line && range?.start?.line === start) {
                        start = range.start.line
                        end = range?.end?.line ?? start
                        break
                      }
                    }
                  }
                  offset = Math.max(start, 1)
                  if (end) {
                    limit = end - (offset - 1)
                  }
                }
                const args = { filePath: filepath, offset, limit }

                const pieces: Draft<MessageV2.Part>[] = [
                  {
                    messageID: info.id,
                    sessionID: input.sessionID,
                    type: "text",
                    synthetic: true,
                    text: `Called the Read tool with the following input: ${JSON.stringify(args)}`,
                  },
                ]

                await ReadTool.init()
                  .then(async (t) => {
                    const model = await Provider.getModel(info.model.providerID, info.model.modelID)
                    const readCtx: Tool.Context = {
                      sessionID: input.sessionID,
                      abort: new AbortController().signal,
                      agent: input.agent!,
                      messageID: info.id,
                      extra: { bypassCwdCheck: true, model },
                      messages: [],
                      metadata: async () => {},
                      ask: async () => {},
                    }
                    const result = await t.execute(args, readCtx)
                    pieces.push({
                      messageID: info.id,
                      sessionID: input.sessionID,
                      type: "text",
                      synthetic: true,
                      text: result.output,
                    })
                    if (result.attachments?.length) {
                      pieces.push(
                        ...result.attachments.map((attachment) => ({
                          ...attachment,
                          synthetic: true,
                          filename: attachment.filename ?? partFilename,
                          messageID: info.id,
                          sessionID: input.sessionID,
                        })),
                      )
                    } else {
                      pieces.push({
                        ...part,
                        messageID: info.id,
                        sessionID: input.sessionID,
                      })
                    }
                  })
                  .catch((error) => {
                    log.error("failed to read file", { error })
                    const message = error instanceof Error ? error.message : error.toString()
                    Bus.publish(Session.Event.Error, {
                      sessionID: input.sessionID,
                      error: { name: "Unknown", message, data: {} },
                    })
                    pieces.push({
                      messageID: info.id,
                      sessionID: input.sessionID,
                      type: "text",
                      synthetic: true,
                      text: `Read tool failed to read ${filepath} with the following error: ${message}`,
                    })
                  })

                return pieces
              }

              if (part.mime === "application/x-directory") {
                const args = { filePath: filepath }
                const listCtx: Tool.Context = {
                  sessionID: input.sessionID,
                  abort: new AbortController().signal,
                  agent: input.agent!,
                  messageID: info.id,
                  extra: { bypassCwdCheck: true },
                  messages: [],
                  metadata: async () => {},
                  ask: async () => {},
                }
                const result = await ReadTool.init().then((t) => t.execute(args, listCtx))
                return [
                  {
                    messageID: info.id,
                    sessionID: input.sessionID,
                    type: "text",
                    synthetic: true,
                    text: `Called the Read tool with the following input: ${JSON.stringify(args)}`,
                  },
                  {
                    messageID: info.id,
                    sessionID: input.sessionID,
                    type: "text",
                    synthetic: true,
                    text: result.output,
                  },
                  {
                    ...part,
                    messageID: info.id,
                    sessionID: input.sessionID,
                  },
                ]
              }

              FileTime.read(input.sessionID, filepath)
              return [
                {
                  messageID: info.id,
                  sessionID: input.sessionID,
                  type: "text",
                  text: `Called the Read tool with the following input: {"filePath":"${filepath}"}`,
                  synthetic: true,
                },
                {
                  id: part.id,
                  messageID: info.id,
                  sessionID: input.sessionID,
                  type: "file",
                  url: `data:${part.mime};base64,` + (await Filesystem.readBytes(filepath)).toString("base64"),
                  mime: part.mime,
                  filename: part.filename!,
                  source: part.source,
                },
              ]
          }
        }

        if (part.type === "agent") {
          // Check if this agent would be denied by task permission
          const perm = PermissionNext.evaluate("task", part.name, agent.permission)
          const hint = perm.action === "deny" ? " . Invoked by user; guaranteed to exist." : ""
          return [
            {
              ...part,
              messageID: info.id,
              sessionID: input.sessionID,
            },
            {
              messageID: info.id,
              sessionID: input.sessionID,
              type: "text",
              synthetic: true,
              // An extra space is added here. Otherwise the 'Use' gets appended
              // to user's last word; making a combined word
              text:
                " Use the above message and context to generate a prompt and call the task tool with subagent: " +
                part.name +
                hint,
            },
          ]
        }

        return [
          {
            ...part,
            messageID: info.id,
            sessionID: input.sessionID,
          },
        ]
      }),
    ).then((x) => x.flat().map(assign))

    await Plugin.trigger(
      "chat.message",
      {
        sessionID: input.sessionID,
        agent: input.agent,
        model: input.model,
        messageID: input.messageID,
        variant: input.variant,
      },
      {
        message: info,
        parts,
      },
    )

    await Session.updateMessage(info)
    for (const part of parts) {
      await Session.updatePart(part)
    }

    return {
      info,
      parts,
    }
  }

  async function insertReminders(input: { messages: MessageV2.WithParts[]; agent: Agent.Info; session: Session.Info }) {
    const userMessage = input.messages.findLast((msg) => msg.info.role === "user")
    if (!userMessage) return input.messages

    // Original logic when experimental plan mode is disabled
    if (!Flag.GIZZI_EXPERIMENTAL_PLAN_MODE) {
      if (input.agent.name === "plan") {
        userMessage.parts.push({
          id: Identifier.ascending("part"),
          messageID: userMessage.info.id,
          sessionID: userMessage.info.sessionID,
          type: "text",
          text: PROMPT_PLAN,
          synthetic: true,
        })
      }
      const wasPlan = input.messages.some((msg) => msg.info.role === "assistant" && msg.info.agent === "plan")
      if (wasPlan && input.agent.name === "build") {
        userMessage.parts.push({
          id: Identifier.ascending("part"),
          messageID: userMessage.info.id,
          sessionID: userMessage.info.sessionID,
          type: "text",
          text: BUILD_SWITCH,
          synthetic: true,
        })
      }
      return input.messages
    }

    // New plan mode logic when flag is enabled
    const assistantMessage = input.messages.findLast((msg) => msg.info.role === "assistant")

    // Switching from plan mode to build mode
    if (input.agent.name !== "plan" && assistantMessage?.info.agent === "plan") {
      const plan = Session.plan(input.session)
      const exists = await Filesystem.exists(plan)
      if (exists) {
        const part = await Session.updatePart({
          id: Identifier.ascending("part"),
          messageID: userMessage.info.id,
          sessionID: userMessage.info.sessionID,
          type: "text",
          text:
            BUILD_SWITCH + "\n\n" + `A plan file exists at ${plan}. You should execute on the plan defined within it`,
          synthetic: true,
        })
        userMessage.parts.push(part)
      }
      return input.messages
    }

    // Entering plan mode
    if (input.agent.name === "plan" && assistantMessage?.info.agent !== "plan") {
      const plan = Session.plan(input.session)
      const exists = await Filesystem.exists(plan)
      if (!exists) await fs.mkdir(path.dirname(plan), { recursive: true })
      const part = await Session.updatePart({
        id: Identifier.ascending("part"),
        messageID: userMessage.info.id,
        sessionID: userMessage.info.sessionID,
        type: "text",
        text: `<system-reminder>
Plan mode is on. Nothing changes yet: you plan, the user approves, then you build. Do not edit anything except the plan file named below. Read-only shell commands (ls, cat, git status, git log, git diff, grep) are fine; do not run formatters, generators, migrations or commits. This holds even if the user says "just do it": that means plan how to do it.

## Plan File
${exists ? `A plan file already exists at ${plan}. You can read it and make incremental edits using the edit tool.` : `No plan file exists yet. You should create your plan at ${plan} using the write tool.`}
Build the plan incrementally in this file. It is the only file you may edit.

## Workflow
1. Explore. Start with the explore subagent. Use one agent when the scope is small or the files are known; use up to three in parallel, each with its own search focus, when the scope is wide. Plan mode is also used outside code, so adapt the exploration to the task.
2. Ask the user only about intent and tradeoffs that the repo or files cannot answer, with the question tool, two to four options and a recommended default. Do not use the question tool to ask whether the plan is okay.
3. Design. Unless the task is trivial, have a plan agent check your approach against what you found. Give it the filenames, constraints and code paths you traced.
4. Write the final plan to the plan file: your recommended approach only, the files or places that change, ordered steps, edge cases, and a done check for each step. Keep it compact.
5. End the turn by asking a question or by calling plan_exit. Approval is requested only through plan_exit.

</system-reminder>`,
        synthetic: true,
      })
      userMessage.parts.push(part)
      return input.messages
    }
    return input.messages
  }

  export const ShellInput = z.object({
    sessionID: Identifier.schema("session"),
    agent: z.string(),
    model: z
      .object({
        providerID: z.string(),
        modelID: z.string(),
      })
      .optional(),
    command: z.string(),
  })
  export type ShellInput = z.infer<typeof ShellInput>
  export async function shell(input: ShellInput) {
    const abort = start(input.sessionID)
    if (!abort) {
      throw new Session.BusyError(input.sessionID)
    }

    using _ = defer(() => {
      // If no queued callbacks, cancel (the default)
      const callbacks = state()[input.sessionID]?.callbacks ?? []
      if (callbacks.length === 0) {
        cancel(input.sessionID)
      } else {
        // Otherwise, trigger the session loop to process queued items
        loop({ sessionID: input.sessionID, resume_existing: true }).catch((error) => {
          log.error("session loop failed to resume after shell command", { sessionID: input.sessionID, error })
        })
      }
    })

    const session = await Session.get(input.sessionID)
    if (session.revert) {
      await SessionRevert.cleanup(session)
    }
    const agent = await Agent.get(input.agent)
    if (!agent) {
      throw new Error(`Agent not found: ${input.agent}`)
    }
    const model = input.model ?? agent.model ?? (await lastModel(input.sessionID))
    const userMsg: MessageV2.User = {
      id: Identifier.ascending("message"),
      sessionID: input.sessionID,
      time: {
        created: Date.now(),
      },
      role: "user",
      agent: input.agent,
      model: {
        providerID: model.providerID,
        modelID: model.modelID,
      },
    }
    await Session.updateMessage(userMsg)
    const userPart: MessageV2.Part = {
      type: "text",
      id: Identifier.ascending("part"),
      messageID: userMsg.id,
      sessionID: input.sessionID,
      text: "The following tool was executed by the user",
      synthetic: true,
    }
    await Session.updatePart(userPart)

    const msg: MessageV2.Assistant = {
      id: Identifier.ascending("message"),
      sessionID: input.sessionID,
      parentID: userMsg.id,
      mode: input.agent,
      agent: input.agent,
      cost: 0,
      path: {
        cwd: Instance.directory,
        root: Instance.worktree,
      },
      time: {
        created: Date.now(),
      },
      role: "assistant",
      tokens: {
        input: 0,
        output: 0,
        reasoning: 0,
        cache: { read: 0, write: 0 },
      },
      modelID: model.modelID,
      providerID: model.providerID,
    }
    await Session.updateMessage(msg)
    const part: MessageV2.Part = {
      type: "tool",
      id: Identifier.ascending("part"),
      messageID: msg.id,
      sessionID: input.sessionID,
      tool: "bash",
      callID: ulid(),
      state: {
        status: "running",
        time: {
          start: Date.now(),
        },
        input: {
          command: input.command,
        },
      },
    }
    await Session.updatePart(part)
    const shell = Shell.preferred()
    const shellName = (
      process.platform === "win32" ? path.win32.basename(shell, ".exe") : path.basename(shell)
    ).toLowerCase()

    const invocations: Record<string, { args: string[] }> = {
      nu: {
        args: ["-c", input.command],
      },
      fish: {
        args: ["-c", input.command],
      },
      zsh: {
        args: [
          "-c",
          "-l",
          `
            [[ -f ~/.zshenv ]] && source ~/.zshenv >/dev/null 2>&1 || true
            [[ -f "\${ZDOTDIR:-$HOME}/.zshrc" ]] && source "\${ZDOTDIR:-$HOME}/.zshrc" >/dev/null 2>&1 || true
            eval ${JSON.stringify(input.command)}
          `,
        ],
      },
      bash: {
        args: [
          "-c",
          "-l",
          `
            shopt -s expand_aliases
            [[ -f ~/.bashrc ]] && source ~/.bashrc >/dev/null 2>&1 || true
            eval ${JSON.stringify(input.command)}
          `,
        ],
      },
      // Windows cmd
      cmd: {
        args: ["/c", input.command],
      },
      // Windows PowerShell
      powershell: {
        args: ["-NoProfile", "-Command", input.command],
      },
      pwsh: {
        args: ["-NoProfile", "-Command", input.command],
      },
      // Fallback: any shell that doesn't match those above
      //  - No -l, for max compatibility
      "": {
        args: ["-c", `${input.command}`],
      },
    }

    const matchingInvocation = invocations[shellName] ?? invocations[""]
    const args = matchingInvocation?.args

    const cwd = Instance.directory
    const shellEnv = await Plugin.trigger(
      "shell.env",
      { cwd, sessionID: input.sessionID, callID: part.callID },
      { env: {} },
    )
    const proc = spawn(shell, args, {
      cwd,
      detached: process.platform !== "win32",
      stdio: ["ignore", "pipe", "pipe"],
      env: {
        ...process.env,
        ...shellEnv.env,
        // Cloud environment variables, secrets and network-policy proxy ({} for local sessions).
        ...EnvironmentRuntime.toolEnv(input.sessionID),
        TERM: "dumb",
      },
    })
    ProcessRegistry.track(proc, { label: "prompt-shell", group: process.platform !== "win32" })

    let output = ""

    proc.stdout?.on("data", (chunk) => {
      output += chunk.toString()
      if (part.state.status === "running") {
        part.state.metadata = {
          output: output,
          description: "",
        }
        Session.updatePart(part)
      }
    })

    proc.stderr?.on("data", (chunk) => {
      output += chunk.toString()
      if (part.state.status === "running") {
        part.state.metadata = {
          output: output,
          description: "",
        }
        Session.updatePart(part)
      }
    })

    let aborted = false
    let exited = false

    const kill = () => Shell.killTree(proc, { exited: () => exited })

    if (abort.aborted) {
      aborted = true
      await kill()
    }

    const abortHandler = () => {
      aborted = true
      void kill()
    }

    abort.addEventListener("abort", abortHandler, { once: true })

    await new Promise<void>((resolve) => {
      proc.on("close", () => {
        exited = true
        abort.removeEventListener("abort", abortHandler)
        resolve()
      })
    })

    if (aborted) {
      output += "\n\n" + ["<metadata>", "User aborted the command", "</metadata>"].join("\n")
    }
    msg.time.completed = Date.now()
    await Session.updateMessage(msg)
    if (part.state.status === "running") {
      part.state = {
        status: "completed",
        time: {
          ...part.state.time,
          end: Date.now(),
        },
        input: part.state.input,
        title: "",
        metadata: {
          output,
          description: "",
        },
        output,
      }
      await Session.updatePart(part)
    }
    return { info: msg, parts: [part] }
  }

  export const CommandInput = z.object({
    messageID: Identifier.schema("message").optional(),
    sessionID: Identifier.schema("session"),
    agent: z.string().optional(),
    model: z.string().optional(),
    arguments: z.string(),
    command: z.string(),
    variant: z.string().optional(),
    backgroundPolicy: z.enum(["exit", "drain", "steer"]).optional(),
    parts: z
      .array(
        z.discriminatedUnion("type", [
          MessageV2.FilePart.omit({
            messageID: true,
            sessionID: true,
          }).partial({
            id: true,
          }),
        ]),
      )
      .optional(),
  })
  export type CommandInput = z.infer<typeof CommandInput>
  const bashRegex = /!`([^`]+)`/g
  // Match [Image N] as single token, quoted strings, or non-space sequences
  const argsRegex = /(?:\[Image\s+\d+\]|"[^"]*"|'[^']*'|[^\s"']+)/gi
  const placeholderRegex = /\$(\d+)/g
  const quoteTrimRegex = /^["']|["']$/g
  /**
   * Regular expression to match @ file references in text
   * Matches @ followed by file paths, excluding commas, periods at end of sentences, and backticks
   * Does not match when preceded by word characters or backticks (to avoid email addresses and quoted references)
   */

  export async function command(input: CommandInput) {
    log.info("command", input)
    const session = await Session.get(input.sessionID)
    const command = await Command.get(input.command)
    const agentName = command.agent ?? input.agent ?? (await Agent.defaultAgent())
    // `review security|perf|tests|diff [target]`: a focused preset prompt with
    // the shared findings format; the rest of the arguments is the target.
    const preset = input.command === Command.Default.REVIEW ? matchReviewPreset(input.arguments) : undefined
    if (preset) input = { ...input, arguments: preset.target }

    const raw = input.arguments.match(argsRegex) ?? []
    const args = raw.map((arg) => arg.replace(quoteTrimRegex, ""))

    const templateCommand = preset ? reviewPresetPrompt(preset.name) : await command.template

    const placeholders = templateCommand.match(placeholderRegex) ?? []
    let last = 0
    for (const item of placeholders) {
      const value = Number(item.slice(1))
      if (value > last) last = value
    }

    // Let the final placeholder swallow any extra arguments so prompts read naturally
    const withArgs = templateCommand.replaceAll(placeholderRegex, (_, index) => {
      const position = Number(index)
      const argIndex = position - 1
      if (argIndex >= args.length) return ""
      if (position === last) return args.slice(argIndex).join(" ")
      return args[argIndex]
    })
    const usesArgumentsPlaceholder = templateCommand.includes("$ARGUMENTS")
    let template = withArgs.replaceAll("$ARGUMENTS", input.arguments)

    // If command doesn't explicitly handle arguments (no $N or $ARGUMENTS placeholders)
    // but user provided arguments, append them to the template
    if (placeholders.length === 0 && !usesArgumentsPlaceholder && input.arguments.trim()) {
      template = template + "\n\n" + input.arguments
    }

    const shell = ConfigMarkdown.shell(template)
    if (shell.length > 0) {
      const results = await Promise.all(
        shell.map(async ([, cmd]) => {
          try {
            return await $`${{ raw: cmd }}`.quiet().nothrow().text()
          } catch (error) {
            return `Error executing command: ${error instanceof Error ? error.message : String(error)}`
          }
        }),
      )
      let index = 0
      template = template.replace(bashRegex, () => results[index++])
    }
    template = template.trim()

    const taskModel = await (async () => {
      if (command.model) {
        return Provider.parseModel(command.model)
      }
      if (command.agent) {
        const cmdAgent = await Agent.get(command.agent)
        if (cmdAgent?.model) {
          return cmdAgent.model
        }
      }
      if (input.model) return Provider.parseModel(input.model)
      return await lastModel(input.sessionID)
    })()

    try {
      await Provider.getModel(taskModel.providerID, taskModel.modelID)
    } catch (e) {
      if (Provider.ModelNotFoundError.isInstance(e)) {
        const providerID = e.data?.providerID as string | undefined
        const modelID = e.data?.modelID as string | undefined
        const suggestions = e.data?.suggestions as string[] | undefined
        const hint = suggestions?.length ? ` Did you mean: ${suggestions.join(", ")}?` : ""
        Bus.publish(Session.Event.Error, {
          sessionID: input.sessionID,
          error: { name: "Unknown", message: `Model not found: ${providerID}/${modelID}.${hint}`, data: {} },
        })
      }
      throw e
    }
    const agent = await Agent.get(agentName)
    if (!agent) {
      const available = await Agent.list().then((agents) => agents.filter((a) => !a.hidden).map((a) => a.name))
      const hint = available.length ? ` Available agents: ${available.join(", ")}` : ""
      const errObj = { name: "Unknown", message: `Agent not found: "${agentName}".${hint}`, data: {} }
      Bus.publish(Session.Event.Error, {
        sessionID: input.sessionID,
        error: errObj,
      })
      throw new Error(errObj.message)
    }

    const templateParts = await resolvePromptParts(template)
    const isSubtask = (agent.mode === "subagent" && command.subtask !== false) || command.subtask === true
    const parts = isSubtask
      ? [
          {
            type: "subtask" as const,
            agent: agent.name,
            description: command.description ?? "",
            command: input.command,
            model: {
              providerID: taskModel.providerID,
              modelID: taskModel.modelID,
            },
            prompt: templateParts.find((y) => y.type === "text")?.text ?? "",
            parts: templateParts.filter((y) => y.type !== "text"),
          },
        ]
      : [...templateParts, ...(input.parts ?? [])]

    const userAgent = isSubtask ? (input.agent ?? (await Agent.defaultAgent())) : agentName
    const userModel = isSubtask
      ? input.model
        ? Provider.parseModel(input.model)
        : await lastModel(input.sessionID)
      : taskModel

    await Plugin.trigger(
      "command.execute.before",
      {
        command: input.command,
        sessionID: input.sessionID,
        arguments: input.arguments,
      },
      { parts },
    )

    // WP-S1U-3: this turn runs a command template; its ROUTE label is "template".
    TurnRouter.markTemplate(input.sessionID)
    const result = (await prompt({
      sessionID: input.sessionID,
      messageID: input.messageID,
      model: userModel,
      agent: userAgent,
      parts,
      variant: input.variant,
      backgroundPolicy: input.backgroundPolicy,
    })) as MessageV2.WithParts

    Bus.publish(Command.Event.Executed, {
      name: input.command,
      sessionID: input.sessionID,
      arguments: input.arguments,
      messageID: result.info.id,
    })

    return result
  }

  async function ensureTitle(input: {
    session: Session.Info
    history: MessageV2.WithParts[]
    providerID: string
    modelID: string
  }) {
    if (input.session.parentID) return
    if (!Session.isDefaultTitle(input.session.title)) return

    // Find first non-synthetic user message
    const firstRealUserIdx = input.history.findIndex(
      (m) => m.info.role === "user" && !m.parts.every((p) => "synthetic" in p && p.synthetic),
    )
    if (firstRealUserIdx === -1) return

    const isFirst =
      input.history.filter((m) => m.info.role === "user" && !m.parts.every((p) => "synthetic" in p && p.synthetic))
        .length === 1
    if (!isFirst) return

    // Gather all messages up to and including the first real user message for context
    // This includes any shell/subtask executions that preceded the user's first prompt
    const contextMessages = input.history.slice(0, firstRealUserIdx + 1)
    const firstRealUser = contextMessages[firstRealUserIdx]

    // For subtask-only messages (from command invocations), extract the prompt directly
    // since toModelMessage converts subtask parts to generic "The following tool was executed by the user"
    const subtaskParts = firstRealUser.parts.filter((p) => p.type === "subtask") as MessageV2.SubtaskPart[]
    const hasOnlySubtaskParts = subtaskParts.length > 0 && firstRealUser.parts.every((p) => p.type === "subtask")

    const agent = await Agent.get("title")
    if (!agent) return
    const model = await iife(async () => {
      if (agent.model) return await Provider.getModel(agent.model.providerID, agent.model.modelID)
      return (
        (await Provider.getSmallModel(input.providerID)) ?? (await Provider.getModel(input.providerID, input.modelID))
      )
    })
    // A subscription (fabric) model runs only on a human send (D16), and the
    // send's human action belongs to the reply — keep the default title.
    if (Provider.isHumanSendOnly(model)) return
    const modelMessages = MessageV2.toModelMessages(contextMessages, model)
    // Title gen calls LLM.stream directly instead of going through loop(), so it
    // doesn't get loop()'s folding of lastUser.system into the system array — fold
    // the first message's system override in here to preserve that parity.
    const firstUserSystem = (firstRealUser.info as MessageV2.User).system
    const titleSystem = firstUserSystem
      ? [firstUserSystem.startsWith("+") ? firstUserSystem.slice(1) : firstUserSystem]
      : []
    const result = await LLM.stream({
      agent,
      user: firstRealUser.info as MessageV2.User,
      system: titleSystem,
      small: true,
      tools: {},
      model,
      abort: new AbortController().signal,
      sessionID: input.session.id,
      retries: 2,
      callType: "title",
      surface: await SystemPrompt.effectiveSurface(input.session, (id) => Session.get(id)),
      messages: [
        {
          role: "user",
          content: "Generate a title for this conversation:\n",
        },
        ...(hasOnlySubtaskParts
          ? [{ role: "user" as const, content: subtaskParts.map((p) => p.prompt).join("\n") }]
          : modelMessages),
      ],
    })
    const text = await Promise.resolve(result.text).catch((err: any) => log.error("failed to generate title", { error: err }))
    if (text) {
      const cleaned = text
        .replace(/<think>[\s\S]*?<\/think>\s*/g, "")
        .split("\n")
        .map((line: string) => line.trim())
        .find((line: string) => line.length > 0)
      if (!cleaned) return

      const title = cleaned.length > 100 ? cleaned.substring(0, 97) + "..." : cleaned
      return Session.setTitle({ sessionID: input.session.id, title })
    }
  }
}

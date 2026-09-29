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
import { type Tool as AITool, tool, jsonSchema, type ToolCallOptions, asSchema } from "ai"
import { ToolDispatcher } from "@/runtime/tools/dispatch"
import { ToolDedupe } from "@/runtime/tools/dedupe"
import { ToolSelection } from "@/runtime/tools/selection"
import { SessionCompaction } from "@/runtime/session/compaction"
import { Instance } from "@/runtime/context/project/instance"
import { Bus } from "@/shared/bus"
import { ProviderTransform } from "@/runtime/providers/adapters/transform"
import { SystemPrompt } from "@/runtime/session/system"
import { InstructionPrompt } from "@/runtime/session/instruction"
import { Plugin } from "@/runtime/integrations/plugin"
import PROMPT_PLAN from "@/runtime/session/prompt/plan.txt"
import BUILD_SWITCH from "@/runtime/session/prompt/build-switch.txt"
import MAX_STEPS from "@/runtime/session/prompt/max-steps.txt"
import { defer } from "@/shared/util/defer"
import { ToolRegistry } from "@/runtime/tools/builtins/registry"
import { MCP } from "@/runtime/tools/mcp"
import type { CallToolResult } from "@modelcontextprotocol/sdk/types.js"
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
import { SessionProcessor } from "@/runtime/session/processor"
import { TaskTool } from "@/runtime/tools/builtins/task"
import { Tool } from "@/runtime/tools/builtins/tool"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { SessionStatus } from "@/runtime/session/status"
import { GoalEngine } from "@/runtime/automation/goal-engine"
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
import * as BotInbox from "@/runtime/bots/bot-inbox"
import { isRoutineTurnText } from "@/runtime/bots/bot-routines"
import { isMessageAgentSession, MessageAgentTool } from "@/runtime/tools/builtins/message-agent"
import { SessionHandoff } from "@/runtime/session/handoff"
import { SessionPause } from "@/runtime/session/pause"
import { Budget } from "@/runtime/session/budget"

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
    format: MessageV2.Format.optional(),
    system: z.string().optional(),
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
    // Context handoff: a prompt to a session that handed off lands on its
    // lineage head, and a switch to a model whose window can't hold this one
    // hands off first. Every entry point (HTTP, CLI, bots, routines) goes
    // through here, so no client can talk to a closed window.
    input.sessionID = await beforeTurnHandoff(input)
    const session = await Session.get(input.sessionID)
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
    if (input.parts.length > 0) {
      const inboxMessages = await BotInbox.collectBotInboxMessages(input.sessionID).catch((error) => {
        log.warn("bot inbox pickup failed; continuing without injected mail", { sessionID: input.sessionID, error })
        return [] as string[]
      })
      for (const text of inboxMessages) {
        input.parts.push({ type: "text", text, synthetic: true })
      }
    }

    if (input.backgroundPolicy) {
      BackgroundTask.setPrintPolicy(input.sessionID, input.backgroundPolicy)
    }
    if (input.workdir !== undefined) setWorkdir(input.sessionID, input.workdir)

const message = await createUserMessage(input)
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

    // Usage limits (P3.17): a paused session holds the turn until it
    // resumes; one about to run into a limit lands here instead of failing.
    if (await holdForLimit(session, input.model)) return message

    // Bot Mode: interactive turns (user chat or teammate DM pickup) carry the
    // first-response preamble in the persona injection; routine deliveries
    // (`[routine: …]` marker) and delegated subtask turns stay quiet.
    const botPreamble = !input.parts.some(
      (p) => (p.type === "text" && isRoutineTurnText(p.text)) || p.type === "subtask",
    )

    const result = await loop({ sessionID: input.sessionID, fallbackModels: input.fallbackModels, botPreamble })
    if (!afterTurnLimit(session, result)) afterTurnHandoff(input.sessionID, result)
    return result
  })

  /** Hold this turn when the session is paused or about to hit a limit. */
  async function holdForLimit(session: Session.Info, model: PromptInput["model"]): Promise<boolean> {
    if (session.parentID) return false
    if (SessionPause.isPaused(session)) return true
    // Spend limits (P8.1): a bot's monthly budget or the thread's own.
    const over = await Budget.exceeded(session).catch(() => undefined)
    if (over) {
      SessionPause.pause(session.id, { ...over, reason: "budget" })
      return true
    }
    const providerID = model?.providerID ?? session.defaultModel?.providerID
    if (!providerID) return false
    const landing = await SessionPause.quotaLanding(providerID).catch(() => undefined)
    if (!landing) return false
    SessionPause.pause(session.id, { ...landing, providerID, reason: "quota" })
    return true
  }

  /** A turn that ran into a limit pauses the session until it resets. */
  function afterTurnLimit(session: Session.Info, result: MessageV2.WithParts | undefined): boolean {
    if (session.parentID) return false
    const hit = SessionPause.limitFromTurn(result)
    if (!hit) return false
    const providerID = result?.info.role === "assistant" ? result.info.providerID : undefined
    SessionPause.pause(session.id, { until: hit.until, limit: hit.limit, reason: hit.reason, providerID })
    return true
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

  export function cancel(sessionID: string) {
    log.info("cancel", { sessionID })
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
    const dir = workdirs.get(input.sessionID)
    return dir ? Instance.withWorkdir(dir, () => runLoop(input)) : runLoop(input)
  })

  async function runLoop(input: z.infer<typeof LoopInput>) {
    const { sessionID, resume_existing } = input

    const abort = resume_existing ? resume(sessionID) : start(sessionID)
    if (!abort) {
      return new Promise<MessageV2.WithParts>((resolve, reject) => {
        const callbacks = state()[sessionID].callbacks
        callbacks.push({ resolve, reject })
        state()[sessionID].queuedTurns += 1
      })
    }

    using _ = defer(() => cancel(sessionID))

    // Structured output state
    // Note: On session resumption, state is reset but outputFormat is preserved
    // on the user message and will be retrieved from lastUser below
    let structuredOutput: unknown | undefined

    let step = 0
    const session = await Session.get(sessionID)
    while (true) {
      SessionStatus.set(sessionID, { type: "busy" })
      log.info("loop", { step, sessionID })
      if (abort.aborted) break
      let msgs = await MessageV2.filterCompacted(MessageV2.stream(sessionID))

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

      if (step === 1 && session.sourceRef) {
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
        lastUser.id < lastAssistant.id
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

      // Resolve auto-routing before model lookup
      const resolvedModelRef =
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
            await PermissionNext.ask({
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
      const maxSteps = agent.steps ?? Infinity
      const isLastStep = step >= maxSteps
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
          sessionID,
        })) as MessageV2.Assistant,
        sessionID: sessionID,
        model,
        abort,
        fallbackModels: input.fallbackModels,
      })
      using _ = defer(() => InstructionPrompt.clear(processor.message.id))

      // Check if user explicitly invoked an agent via @ in this turn
      const lastUserMsg = msgs.findLast((m) => m.info.role === "user")
      const bypassAgentCheck = lastUserMsg?.parts.some((p) => p.type === "agent") ?? false

      const tools = await resolveTools({
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
      const workspaceSystemPrompt = await WorkspaceContext.getWorkspaceSystemPrompt(session.directory)
      // Bot Mode (B2): when this session is a bot's canonical chat, inject the
      // bot's identity + SOUL + memory as standing instructions. This builder
      // runs inside the runtime session pipeline, so both the TUI (worker /
      // server) and headless (`gizzi run --print`) paths get the injection.
      const botSystemPrompt = await BotChat.botChatSystemPrompt(sessionID, {
        preamble: input.botPreamble,
      }).catch((error) => {
        log.warn("failed to build bot persona prompt", { sessionID, error })
        return undefined
      })
      const scratchpadSystemPrompt = await Scratchpad.instructions(sessionID).catch((error) => {
        log.warn("failed to initialize session scratchpad", { sessionID, error })
        return undefined
      })
      const system = [
        // If user provided a full system prompt override, use only that
        ...(lastUser.system && !lastUser.system.startsWith("+")
          ? [lastUser.system]
          : [
              ...(await SystemPrompt.environment(model)),
              ...(await InstructionPrompt.system()),
              ...(workspaceSystemPrompt ? [workspaceSystemPrompt] : []),
              ...(botSystemPrompt ? [botSystemPrompt] : []),
              ...(scratchpadSystemPrompt ? [scratchpadSystemPrompt] : []),
              // Append system prompt (prefixed with + or via --append-system-prompt)
              ...(lastUser.system?.startsWith("+") ? [lastUser.system.slice(1)] : []),
            ]),
      ]
      const format = lastUser.format ?? { type: "text" }
      if (format.type === "json_schema") {
        system.push(STRUCTURED_OUTPUT_SYSTEM_PROMPT)
      }
      const goalReminder = GoalEngine.reminder(sessionID)
      if (goalReminder) system.push(goalReminder)
      const validationReminder = ToolValidationRetry.reminder(sessionID)
      if (validationReminder) system.push(validationReminder)

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

      const result = await processor.process({
        user: lastUser,
        agent,
        abort,
        sessionID,
        system,
        messages: [
          ...MessageV2.toModelMessages(msgs, model),
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
        toolChoice: format.type === "json_schema" ? "required" : undefined,
        plan,
      }).finally(() => {
        if (deadlineTimer !== undefined) clearTimeout(deadlineTimer)
      })

      // If structured output was captured, save it and exit immediately
      // This takes priority because the StructuredOutput tool was called successfully
      if (structuredOutput !== undefined) {
        processor.message.structured = structuredOutput
        processor.message.finish = processor.message.finish ?? "stop"
        await Session.updateMessage(processor.message)
        break
      }

      // Check if model finished (finish reason is not "tool-calls" or "unknown")
      const modelFinished = processor.message.finish && !["tool-calls", "unknown"].includes(processor.message.finish)

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
          const continuationUser: MessageV2.User = {
            id: Identifier.ascending("message"),
            sessionID,
            role: "user",
            time: { created: Date.now() },
            agent: lastUser.agent,
            model: lastUser.model,
          }
          await Session.updateMessage(continuationUser)
          await Session.updatePart({
            id: Identifier.ascending("part"),
            messageID: continuationUser.id,
            sessionID,
            type: "text",
            text: "Continue the active durable goal. Choose the next bounded useful slice, preserve evidence, and use update_goal only for progress accounting or an audited terminal outcome.",
            synthetic: true,
          } satisfies MessageV2.TextPart)
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
    SessionCompaction.prune({ sessionID })
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
      extra: { model: input.model, bypassAgentCheck: input.bypassAgentCheck },
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
        await PermissionNext.ask({
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
      { modelID: input.model.api.id, providerID: input.model.providerID },
      input.agent,
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
          const result = await ToolDedupe.execute({
            sessionID: ctx.sessionID,
            messageID: ctx.messageID,
            tool: item.id,
            args,
            run: () => ToolDispatcher.executeInitialized(item.id, args, ctx, item.execute),
          })
          const output = {
            ...result,
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

    const mcpTools = Object.entries(await MCP.tools())
    const config = await Config.get()
    const dynamicSelection =
      Flag.GIZZI_DYNAMIC_TOOL_SELECTION || config.experimental?.dynamic_tool_selection === true
    const mcpNames = mcpTools.map(([name]) => name).sort()
    const loadedMcpTools = ToolSelection.loaded(input.session.id, input.messages)
    if (dynamicSelection && mcpNames.length) {
      tools["select_tools"] = tool({
        id: "select_tools" as any,
        description: [
          "Load one or more MCP tool schemas for the next model step. Use this when a required integration tool is not currently available.",
          `Loadable tools: ${mcpNames.join(", ")}`,
        ].join("\n"),
        inputSchema: jsonSchema({
          type: "object",
          additionalProperties: false,
          required: ["names"],
          properties: {
            names: { type: "array", items: { type: "string" }, minItems: 1 },
          },
        }),
        execute: async (args: { names: string[] }, options) => {
          const ctx = context(args, options)
          return ToolDedupe.execute({
            sessionID: ctx.sessionID,
            messageID: ctx.messageID,
            tool: "select_tools",
            args,
            run: () => ToolDispatcher.executeInitialized("select_tools", args, ctx, async (nextArgs) => {
              const result = ToolSelection.load(input.session.id, nextArgs.names, mcpNames)
              return {
                title: "MCP tools selected",
                metadata: result,
                output: JSON.stringify(result),
              }
            }),
          })
        },
      })
    }
    for (const [key, item] of mcpTools) {
      if (dynamicSelection && !loadedMcpTools.has(key)) continue
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

        await ctx.ask({
          permission: key,
          metadata: {},
          patterns: ["*"],
          always: ["*"],
        })

        const result = await ToolDedupe.execute<CallToolResult & { metadata?: Record<string, unknown> }>({
          sessionID: ctx.sessionID,
          messageID: ctx.messageID,
          tool: key,
          args,
          run: () => ToolDispatcher.executeInitialized(key, args, ctx, (nextArgs) => execute(nextArgs, opts)),
        })

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

        const truncated = await Truncate.output(textParts.join("\n\n"), {}, input.agent)
        const metadata = {
          ...(result.metadata ?? {}),
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
          content: Truncate.modelContent(result.content as Truncate.ModelContentItem[], truncated),
        }
      }
      tools[key] = item
    }

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

    return tools
  }

  const MobileTools = z.object({
    webSearch: z.boolean().optional(),
    research: z.boolean().optional(),
    toolAccess: z.enum(["auto", "on_demand", "always"]).optional(),
    disabledConnectors: z.array(z.string()).optional(),
  })

  export interface MobileToolOptions {
    webSearch?: boolean
    research?: boolean
    toolAccess?: "auto" | "on_demand" | "always"
    /** Connector ids (open-connector app ids, e.g. "github") the user turned
     * off for this session in the composer's + menu. */
    disabledConnectors?: string[]
  }

  /**
   * The connector a tool call would use when the user turned it off for this
   * turn, else undefined. Connector actions go through the connector MCP's
   * dispatcher tools with an `actionId` of `<app>.<action>`
   * (services/open-connector/src/mcp.ts).
   *
   * @internal Exported for testing
   */
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
    }
    using _ = defer(() => InstructionPrompt.clear(info.id))

    type Draft<T> = T extends MessageV2.Part ? Omit<T, "id"> & { id?: string } : never
    const assign = (part: Draft<MessageV2.Part>): MessageV2.Part => ({
      ...part,
      id: part.id ?? Identifier.ascending("part"),
    })

    const parts = await Promise.all(
      input.parts.map(async (part): Promise<Draft<MessageV2.Part>[]> => {
        if (part.type === "file") {
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
          const url = new URL(part.url)
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
                    text: Buffer.from(part.url, "base64url").toString(),
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
                          filename: attachment.filename ?? part.filename,
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
Plan mode is active. The user indicated that they do not want you to execute yet -- you MUST NOT make any edits (with the exception of the plan file mentioned below), run any non-readonly tools (including changing configs or making commits), or otherwise make any changes to the system. This supersedes any other instructions you have received.

## Plan File Info:
${exists ? `A plan file already exists at ${plan}. You can read it and make incremental edits using the edit tool.` : `No plan file exists yet. You should create your plan at ${plan} using the write tool.`}
You should build your plan incrementally by writing to or editing this file. NOTE that this is the only file you are allowed to edit - other than this you are only allowed to take READ-ONLY actions.

## Plan Workflow

### Phase 1: Initial Understanding
Goal: Gain a comprehensive understanding of the user's request by reading through code and asking them questions. Critical: In this phase you should only use the explore subagent type.

1. Focus on understanding the user's request and the code associated with their request

2. **Launch up to 3 explore agents IN PARALLEL** (single message, multiple tool calls) to efficiently explore the codebase.
   - Use 1 agent when the task is isolated to known files, the user provided specific file paths, or you're making a small targeted change.
   - Use multiple agents when: the scope is uncertain, multiple areas of the codebase are involved, or you need to understand existing patterns before planning.
   - Quality over quantity - 3 agents maximum, but you should try to use the minimum number of agents necessary (usually just 1)
   - If using multiple agents: Provide each agent with a specific search focus or area to explore. Example: One agent searches for existing implementations, another explores related components, a third investigates testing patterns

3. After exploring the code, use the question tool to clarify ambiguities in the user request up front.

### Phase 2: Design
Goal: Design an implementation approach.

Launch general agent(s) to design the implementation based on the user's intent and your exploration results from Phase 1.

You can launch up to 1 agent(s) in parallel.

**Guidelines:**
- **Default**: Launch at least 1 Plan agent for most tasks - it helps validate your understanding and consider alternatives
- **Skip agents**: Only for truly trivial tasks (typo fixes, single-line changes, simple renames)

Examples of when to use multiple agents:
- The task touches multiple parts of the codebase
- It's a large refactor or architectural change
- There are many edge cases to consider
- You'd benefit from exploring different approaches

Example perspectives by task type:
- New feature: simplicity vs performance vs maintainability
- Bug fix: root cause vs workaround vs prevention
- Refactoring: minimal change vs clean architecture

In the agent prompt:
- Provide comprehensive background context from Phase 1 exploration including filenames and code path traces
- Describe requirements and constraints
- Request a detailed implementation plan

### Phase 3: Review
Goal: Review the plan(s) from Phase 2 and ensure alignment with the user's intentions.
1. Read the critical files identified by agents to deepen your understanding
2. Ensure that the plans align with the user's original request
3. Use question tool to clarify any remaining questions with the user

### Phase 4: Final Plan
Goal: Write your final plan to the plan file (the only file you can edit).
- Include only your recommended approach, not all alternatives
- Ensure that the plan file is concise enough to scan quickly, but detailed enough to execute effectively
- Include the paths of critical files to be modified
- Include a verification section describing how to test the changes end-to-end (run the code, use MCP tools, run tests)

### Phase 5: Call plan_exit tool
At the very end of your turn, once you have asked the user questions and are happy with your final plan file - you should always call plan_exit to indicate to the user that you are done planning.
This is critical - your turn should only end with either asking the user a question or calling plan_exit. Do not stop unless it's for these 2 reasons.

**Important:** Use question tool to clarify requirements/approach, use plan_exit to request plan approval. Do NOT use question tool to ask "Is this plan okay?" - that's what plan_exit does.

NOTE: At any point in time through this workflow you should feel free to ask the user questions or clarifications. Don't make large assumptions about user intent. The goal is to present a well researched plan to the user, and tie any loose ends before implementation begins.
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

    const raw = input.arguments.match(argsRegex) ?? []
    const args = raw.map((arg) => arg.replace(quoteTrimRegex, ""))

    const templateCommand = await command.template

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
    if (Provider.isFabricModel(model)) return
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

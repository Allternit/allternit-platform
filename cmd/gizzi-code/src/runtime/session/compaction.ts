import { BusEvent } from "@/shared/bus/bus-event"
import { Bus } from "@/shared/bus"
import { Session } from "@/runtime/session"
import { Identifier } from "@/shared/id/id"
import { Instance } from "@/runtime/context/project/instance"
import { Provider } from "@/runtime/providers/provider"
import { MessageV2 } from "@/runtime/session/message-v2"
import z from "zod/v4"
import { Token } from "@/shared/util/token"
import { Log } from "@/shared/util/log"
import { SessionProcessor } from "@/runtime/session/processor"
import { fn } from "@/shared/util/fn"
import { Agent } from "@/runtime/loop/agent"
import { Plugin } from "@/runtime/integrations/plugin"
import { Config } from "@/runtime/context/config/config"
import { ProviderTransform } from "@/runtime/providers/adapters/transform"
import { SessionTrace } from "@/runtime/session/trace"
import { Todo } from "@/runtime/session/todo"
import { HookDispatcher } from "@/runtime/hooks/dispatcher"

export namespace SessionCompaction {
  const log = Log.create({ service: "session.compaction" })

  export const Event = {
    Compacted: BusEvent.define(
      "session.compacted",
      z.object({
        sessionID: z.string(),
      }),
    ),
  }

  const COMPACTION_BUFFER = 20_000

  export async function isOverflow(input: { tokens: MessageV2.Assistant["tokens"]; model: Provider.Model }) {
    const config = await Config.get()
    if (config.compaction?.auto === false) return false
    const context = input.model.limit.context
    if (context === 0) return false

    const count =
      input.tokens.total ||
      input.tokens.input + input.tokens.output + input.tokens.cache.read + input.tokens.cache.write

    const reserved =
      config.compaction?.reserved ?? Math.min(COMPACTION_BUFFER, ProviderTransform.maxOutputTokens(input.model))
    const usable = input.model.limit.input
      ? input.model.limit.input - reserved
      : context - ProviderTransform.maxOutputTokens(input.model)
    return count >= usable
  }

  export const PRUNE_MINIMUM = 20_000
  export const PRUNE_PROTECT = 40_000

  const PRUNE_PROTECTED_TOOLS = ["skill"]

  // goes backwards through parts until there are 40_000 tokens worth of tool
  // calls. then erases output of previous tool calls. idea is to throw away old
  // tool calls that are no longer relevant.
  export async function prune(input: { sessionID: string }) {
    const config = await Config.get()
    if (config.compaction?.prune === false) return
    log.info("pruning")
    const msgs = await Session.messages({ sessionID: input.sessionID })
    let total = 0
    let pruned = 0
    const toPrune = []
    let turns = 0

    loop: for (let msgIndex = msgs.length - 1; msgIndex >= 0; msgIndex--) {
      const msg = msgs[msgIndex]
      if (msg.info.role === "user") turns++
      if (turns < 2) continue
      if (msg.info.role === "assistant" && msg.info.summary) break loop
      for (let partIndex = msg.parts.length - 1; partIndex >= 0; partIndex--) {
        const part = msg.parts[partIndex]
        if (part.type === "tool")
          if (part.state.status === "completed") {
            if (PRUNE_PROTECTED_TOOLS.includes(part.tool)) continue

            if (part.state.time.compacted) break loop
            const estimate = Token.estimate(part.state.output)
            total += estimate
            if (total > PRUNE_PROTECT) {
              pruned += estimate
              toPrune.push(part)
            }
          }
      }
    }
    log.info("found", { pruned, total })
    if (pruned > PRUNE_MINIMUM) {
      for (const part of toPrune) {
        if (part.state.status === "completed") {
          part.state.time.compacted = Date.now()
          await Session.updatePart(part)
        }
      }
      SessionTrace.append({
        sessionID: input.sessionID,
        kind: "compaction.pruned",
        data: { estimatedTokens: pruned, partIDs: toPrune.map((part) => part.id) },
      })
      log.info("pruned", { count: toPrune.length })
    }
  }

  export async function process(input: {
    parentID: string
    messages: MessageV2.WithParts[]
    sessionID: string
    abort: AbortSignal
    auto: boolean
  }) {
    await HookDispatcher.emit({
      name: "PreCompact",
      timestamp: Date.now(),
      sessionId: input.sessionID,
      payload: { trigger: input.auto ? "auto" : "manual", messageCount: input.messages.length },
    })
    const userMessage = input.messages.findLast((m) => m.info.id === input.parentID)!.info as MessageV2.User
    SessionTrace.append({
      sessionID: input.sessionID,
      kind: "compaction.started",
      messageID: input.parentID,
      data: { parentID: input.parentID, auto: input.auto, messageCount: input.messages.length },
    })
    const agent = await Agent.get("compaction")
    const model = agent.model
      ? await Provider.getModel(agent.model.providerID, agent.model.modelID)
      : await Provider.getModel(userMessage.model.providerID, userMessage.model.modelID)
    const msg = (await Session.updateMessage({
      id: Identifier.ascending("message"),
      role: "assistant",
      parentID: input.parentID,
      sessionID: input.sessionID,
      mode: "compaction",
      agent: "compaction",
      variant: userMessage.variant,
      summary: true,
      path: {
        cwd: Instance.directory,
        root: Instance.worktree,
      },
      cost: 0,
      tokens: {
        output: 0,
        input: 0,
        reasoning: 0,
        cache: { read: 0, write: 0 },
      },
      modelID: model.id,
      providerID: model.providerID,
      time: {
        created: Date.now(),
      },
    })) as MessageV2.Assistant
    const processor = SessionProcessor.create({
      assistantMessage: msg,
      sessionID: input.sessionID,
      model,
      abort: input.abort,
    })
    // Allow plugins to inject context or replace compaction prompt.
    // "experimental.session.compacting" is not declared in the packages/plugin
    // Hooks interface, so trigger with an explicit local contract.
    const compacting = await Plugin.trigger<
      any,
      { sessionID: string },
      { context: string[]; prompt: string | undefined }
    >(
      "experimental.session.compacting",
      { sessionID: input.sessionID },
      { context: [], prompt: undefined },
    )
    const defaultPrompt = `Provide a detailed prompt for continuing our conversation above.
Focus on information that would be helpful for continuing the conversation, including what we did, what we're doing, which files we're working on, and what we're going to do next.
The summary that you construct will be used so that another agent can read it and continue the work.

When constructing the summary, try to stick to this template:
---
## Goal

[What goal(s) is the user trying to accomplish?]

## Instructions

- [What important instructions did the user give you that are relevant]
- [If there is a plan or spec, include information about it so next agent can continue using it]

## Discoveries

[What notable things were learned during this conversation that would be useful for the next agent to know when continuing the work]

## Accomplished

[What work has been completed, what work is still in progress, and what work is left?]

## Relevant files / directories

[Construct a structured list of relevant files that have been read, edited, or created that pertain to the task at hand. If all the files in a directory are relevant, include the path to the directory.]
---`

    const todos = Todo.get(input.sessionID)
    const todoPrompt = todos.length
      ? ["Preserve this durable TODO list verbatim in the summary:", ...todos.map((todo) => `- [${todo.status}] (${todo.priority}) ${todo.content}`)].join("\n")
      : ""
    const promptText = compacting.prompt ?? [defaultPrompt, todoPrompt, ...compacting.context].filter(Boolean).join("\n\n")
    let history = MessageV2.toModelMessages(input.messages, model)
    let result: SessionProcessor.Result = "stop"
    let recoveryAttempts = 0
    let summary = ""
    let finalTruncated = false
    for (let compactionAttempt = 0; compactionAttempt < 3; compactionAttempt++) {
      const before = new Set((await MessageV2.parts(msg.id)).map((part) => part.id))
      result = await processor.process({
        user: userMessage,
        agent,
        abort: input.abort,
        sessionID: input.sessionID,
        tools: {},
        system: [],
        messages: [
          ...history,
          { role: "user", content: [{ type: "text", text: promptText }] },
        ],
        model,
        callType: "compaction",
      })
      const created = (await MessageV2.parts(msg.id)).filter((part) => !before.has(part.id))
      summary = created.filter((part) => part.type === "text").map((part) => part.text).join("").trim()
      const truncated = ["length", "max_tokens", "truncated"].includes(processor.message.finish ?? "")
      finalTruncated = truncated
      const recoverable = result === "compact" || truncated || summary.length === 0
      if (!recoverable || compactionAttempt === 2 || history.length <= 1) break

      recoveryAttempts++
      for (const part of created) {
        await Session.removePart({ sessionID: input.sessionID, messageID: msg.id, partID: part.id })
      }
      delete processor.message.error
      delete processor.message.finish
      processor.message.time.completed = undefined
      await Session.updateMessage(processor.message)
      history = shrinkHistory(history, compactionAttempt)
    }

    if (processor.message.error || result !== "continue" || !summary || finalTruncated) {
      const error = processor.message.error ?? {
        name: "CompactionFailedError",
        message: !summary
          ? "The compaction response did not contain a non-empty summary."
          : finalTruncated
            ? "The compaction summary was truncated after recovery attempts."
            : `Compaction stopped with result ${result}.`,
        data: { recoveryAttempts },
      }
      SessionTrace.append({
        sessionID: input.sessionID,
        kind: "session.error",
        messageID: processor.message.id,
        data: error,
      })
      return "stop"
    }

    if (todos.length) {
      await Session.updatePart({
        id: Identifier.ascending("part"),
        messageID: msg.id,
        sessionID: input.sessionID,
        type: "text",
        synthetic: true,
        text: `\n\n## TODO List\n${todos.map((todo) => `- [${todo.status}] (${todo.priority}) ${todo.content}`).join("\n")}`,
        time: { start: Date.now(), end: Date.now() },
      })
    }

    if (result === "continue" && input.auto) {
      const continueMsg = await Session.updateMessage({
        id: Identifier.ascending("message"),
        role: "user",
        sessionID: input.sessionID,
        time: {
          created: Date.now(),
        },
        agent: userMessage.agent,
        model: userMessage.model,
      })
      await Session.updatePart({
        id: Identifier.ascending("part"),
        messageID: continueMsg.id,
        sessionID: input.sessionID,
        type: "text",
        synthetic: true,
        text: "Continue if you have next steps, or stop and ask for clarification if you are unsure how to proceed.",
        time: {
          start: Date.now(),
          end: Date.now(),
        },
      })
    }
    SessionTrace.append({
      sessionID: input.sessionID,
      kind: "compaction.completed",
      messageID: processor.message.id,
      data: {
        parentID: input.parentID,
        summaryMessageID: processor.message.id,
        auto: input.auto,
        recoveryAttempts,
        originalMessageCount: input.messages.length,
        compactedMessageCount: history.length,
      },
    })
    Bus.publish(Event.Compacted, { sessionID: input.sessionID })
    await HookDispatcher.emit({
      name: "PostCompact",
      timestamp: Date.now(),
      sessionId: input.sessionID,
      payload: { trigger: input.auto ? "auto" : "manual", recoveryAttempts },
    })
    return "continue"
  }

  function shrinkHistory<T extends { role: string }>(messages: T[], attempt: number): T[] {
    const ratio = attempt === 0 ? 0.75 : 0.5
    let reduced = messages.slice(Math.min(messages.length - 1, Math.max(1, Math.floor(messages.length * (1 - ratio)))))
    while (reduced.length > 1 && reduced[0]?.role === "tool") reduced = reduced.slice(1)
    return reduced
  }

  export const create = fn(
    z.object({
      sessionID: Identifier.schema("session"),
      agent: z.string(),
      model: z.object({
        providerID: z.string(),
        modelID: z.string(),
      }),
      auto: z.boolean(),
    }),
    async (input) => {
      const msg = await Session.updateMessage({
        id: Identifier.ascending("message"),
        role: "user",
        model: input.model,
        sessionID: input.sessionID,
        agent: input.agent,
        time: {
          created: Date.now(),
        },
      })
      await Session.updatePart({
        id: Identifier.ascending("part"),
        messageID: msg.id,
        sessionID: msg.sessionID,
        type: "compaction",
        auto: input.auto,
      })
    },
  )
}

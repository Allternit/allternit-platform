import z from "zod/v4"
import { BusEvent } from "@/shared/bus/bus-event"
import { Bus } from "@/shared/bus"
import { Session } from "@/runtime/session"
import { Identifier } from "@/shared/id/id"
import { Provider } from "@/runtime/providers/provider"
import { ProviderTransform } from "@/runtime/providers/adapters/transform"
import { MessageV2 } from "@/runtime/session/message-v2"
import { LLM } from "@/runtime/session/llm"
import { Agent } from "@/runtime/loop/agent"
import { Config } from "@/runtime/context/config/config"
import { SessionTrace } from "@/runtime/session/trace"
import { Todo } from "@/runtime/session/todo"
import { Log } from "@/shared/util/log"

/**
 * Context handoff: when a session's window fills (or the next turn moves to a
 * model with a smaller window), it ends with a checkpoint baton and a fresh
 * session continues from it. The two are linked (`continuesFrom` / `handoff`),
 * so every client can draw the rip and follow the lineage to its head.
 *
 * Compaction still squeezes a window mid-turn; handoff happens between turns.
 * Bot threads, Chat, Cowork, Code and the CLI all use this one mechanism.
 */
export namespace SessionHandoff {
  const log = Log.create({ service: "session.handoff" })

  export const DEFAULT_THRESHOLD = 0.7
  const MAX_LINEAGE = 200

  export const Reason = z.enum(["threshold", "model_switch", "manual", "quota"])
  export type Reason = z.infer<typeof Reason>

  export const Baton = z.object({
    summary: z.string(),
    decisions: z.array(z.string()).default([]),
    openItems: z.array(z.string()).default([]),
    artifacts: z.array(z.string()).default([]),
    nextSteps: z.array(z.string()).default([]),
    todos: z.array(z.string()).default([]),
    generation: z.number(),
    reason: Reason,
    from: z.string(),
    model: z.object({ providerID: z.string(), modelID: z.string() }).optional(),
    written: z.enum(["model", "fallback", "caller"]),
  })
  export type Baton = z.infer<typeof Baton>

  export const Event = {
    HandedOff: BusEvent.define(
      "session.handoff",
      z.object({
        from: z.string(),
        to: z.string(),
        reason: Reason,
        generation: z.number(),
      }),
    ),
  }

  const pending = new Map<string, Promise<Result>>()

  export type Result = { session: Session.Info; baton?: Baton }

  /** Tokens the window can hold for input, same budget compaction uses. */
  export function usable(model: Provider.Model, reserved?: number) {
    const context = model.limit.context
    if (!context) return 0
    const keep = reserved ?? Math.min(20_000, ProviderTransform.maxOutputTokens(model))
    return model.limit.input ? model.limit.input - keep : context - ProviderTransform.maxOutputTokens(model)
  }

  export function occupied(tokens: MessageV2.Assistant["tokens"]) {
    return tokens.total || tokens.input + tokens.output + tokens.cache.read + tokens.cache.write
  }

  async function settings() {
    const cfg = await Config.get()
    return {
      auto: cfg.handoff?.auto !== false,
      threshold: cfg.handoff?.threshold ?? DEFAULT_THRESHOLD,
      reserved: cfg.compaction?.reserved,
    }
  }

  /** Whether a window this full should hand off to a fresh one. */
  export async function shouldHandoff(input: { tokens: MessageV2.Assistant["tokens"]; model: Provider.Model }) {
    const s = await settings()
    if (!s.auto) return false
    const budget = usable(input.model, s.reserved)
    if (budget <= 0) return false
    return occupied(input.tokens) >= budget * s.threshold
  }

  /** Follow `handoff` links to the session that currently holds the conversation. */
  export async function head(sessionID: string): Promise<string> {
    let id = sessionID
    for (let i = 0; i < MAX_LINEAGE; i++) {
      const info = await Session.get(id).catch(() => undefined)
      if (!info?.handoff) return id
      id = info.handoff.sessionID
    }
    return id
  }

  /** The whole lineage, oldest first, for any session in it. */
  export async function lineage(sessionID: string): Promise<Session.Info[]> {
    let root = await Session.get(sessionID)
    for (let i = 0; i < MAX_LINEAGE && root.continuesFrom; i++) {
      const prev = await Session.get(root.continuesFrom).catch(() => undefined)
      if (!prev) break
      root = prev
    }
    const chain = [root]
    for (let i = 0; i < MAX_LINEAGE && chain[chain.length - 1].handoff; i++) {
      const next = await Session.get(chain[chain.length - 1].handoff!.sessionID).catch(() => undefined)
      if (!next) break
      chain.push(next)
    }
    return chain
  }

  /** Wait for an in-flight handoff of this session, if any. */
  export async function settled(sessionID: string) {
    await pending.get(sessionID)?.catch(() => undefined)
  }

  /**
   * Hand a session off to a fresh one. Idempotent: a session that already
   * handed off returns its lineage head; concurrent calls share one run.
   */
  export function run(input: { sessionID: string; reason: Reason; baton?: Partial<Baton>; context?: string }) {
    const inflight = pending.get(input.sessionID)
    if (inflight) return inflight
    const job = execute(input).finally(() => pending.delete(input.sessionID))
    pending.set(input.sessionID, job)
    return job
  }

  async function execute(input: {
    sessionID: string
    reason: Reason
    baton?: Partial<Baton>
    context?: string
  }): Promise<Result> {
    const from = await Session.get(input.sessionID)
    if (from.handoff) return { session: await Session.get(await head(from.id)) }
    if (from.parentID) throw new Error("subagent sessions compact; they do not hand off")

    const msgs = await MessageV2.filterCompacted(MessageV2.stream(from.id))
    const lastUser = msgs.findLast((m) => m.info.role === "user")?.info as MessageV2.User | undefined
    const generation = (await lineage(from.id)).length
    const todos = Todo.get(from.id)
    const todoLines = todos.map((t) => `[${t.status}] ${t.content}`)

    const written = input.baton?.summary?.trim()
      ? ({ ...input.baton, written: "caller" } as Partial<Baton>)
      : await write({ session: from, messages: msgs, user: lastUser, context: input.context, todos: todoLines })

    const baton: Baton = Baton.parse({
      summary: written.summary ?? `Continuing "${from.title}" in a fresh context.`,
      decisions: written.decisions ?? [],
      openItems: written.openItems ?? [],
      artifacts: written.artifacts ?? [],
      nextSteps: written.nextSteps ?? [],
      todos: todoLines,
      generation,
      reason: input.reason,
      from: from.id,
      model: lastUser?.model,
      written: written.written ?? "fallback",
    })

    const next = await Session.createNext({
      directory: from.directory,
      title: from.title,
      permission: from.permission,
      agentID: from.agentID,
      surface: from.surface,
      harness: from.harness,
      defaultModel: from.defaultModel,
      defaultModelSource: from.defaultModelSource,
      continuesFrom: from.id,
    })
    if (todos.length) Todo.update({ sessionID: next.id, todos })

    if (lastUser) {
      const seed = await Session.updateMessage({
        id: Identifier.ascending("message"),
        role: "user",
        sessionID: next.id,
        time: { created: Date.now() },
        agent: lastUser.agent,
        model: lastUser.model,
        variant: lastUser.variant,
        // The bot's standing instructions carry into the fresh window (P4.1).
        ...(lastUser.system ? { system: lastUser.system } : {}),
      })
      await Session.updatePart({
        id: Identifier.ascending("part"),
        messageID: seed.id,
        sessionID: next.id,
        type: "text",
        synthetic: true,
        text: render(baton, input.context),
        metadata: { handoff: { from: from.id, generation, reason: input.reason } },
        time: { start: Date.now(), end: Date.now() },
      })
    }

    await Session.setHandoff({
      sessionID: from.id,
      handoff: { sessionID: next.id, reason: input.reason, at: Date.now(), baton },
    })
    SessionTrace.append({
      sessionID: from.id,
      kind: "handoff.completed",
      data: { to: next.id, reason: input.reason, generation, written: baton.written },
    })
    Bus.publish(Event.HandedOff, { from: from.id, to: next.id, reason: input.reason, generation })
    log.info("handed off", { from: from.id, to: next.id, reason: input.reason, generation })
    return { session: await Session.get(next.id), baton }
  }

  const SYSTEM = `You write the checkpoint an assistant needs to continue this conversation in a fresh context window.
Reply with ONE JSON object and nothing else:
{"summary":"<goal and where things stand, 2-6 sentences>","decisions":["<decisions made and instructions the user gave>"],"openItems":["<what is still open or blocked>"],"artifacts":["<files, links, commands or outputs produced or relied on>"],"nextSteps":["<the next concrete actions>"]}
Keep facts, numbers, names and paths exactly as they appear. No advice, no filler.`

  async function write(input: {
    session: Session.Info
    messages: MessageV2.WithParts[]
    user?: MessageV2.User
    context?: string
    todos: string[]
  }): Promise<Partial<Baton>> {
    const fallback = (): Partial<Baton> => {
      const lastText = input.messages
        .findLast((m) => m.info.role === "assistant")
        ?.parts.filter((p): p is MessageV2.TextPart => p.type === "text")
        .map((p) => p.text)
        .join("")
        .trim()
      return {
        summary: lastText ? lastText.slice(0, 1200) : `Continuing "${input.session.title}" in a fresh context.`,
        written: "fallback",
      }
    }
    if (!input.user || input.messages.length === 0) return fallback()
    try {
      const agent = await Agent.get("compaction")
      const model = agent?.model
        ? await Provider.getModel(agent.model.providerID, agent.model.modelID)
        : await Provider.getModel(input.user.model.providerID, input.user.model.modelID)
      // Subscription (fabric) models run only on a human send (D16).
      if (Provider.isFabricModel(model)) return fallback()
      const ask = [
        input.context ? `Context from the caller:\n${input.context}` : "",
        input.todos.length ? `Current TODO list:\n${input.todos.join("\n")}` : "",
        "Write the checkpoint JSON now.",
      ]
        .filter(Boolean)
        .join("\n\n")
      const result = await LLM.stream({
        agent: agent!,
        user: input.user,
        system: [SYSTEM],
        tools: {},
        callType: "extraction",
        model,
        abort: new AbortController().signal,
        sessionID: input.session.id,
        retries: 2,
        messages: [...MessageV2.toModelMessages(input.messages, model), { role: "user", content: ask }],
      })
      const parsed = parse(await Promise.resolve(result.text))
      return parsed ? { ...parsed, written: "model" } : fallback()
    } catch (error) {
      log.warn("baton write failed; carrying the last reply", { sessionID: input.session.id, error })
      return fallback()
    }
  }

  /** Parse the model's checkpoint JSON (tolerates code fences and prose). */
  export function parse(raw: string | undefined): Partial<Baton> | undefined {
    if (!raw) return
    const text = raw.replace(/<think>[\s\S]*?<\/think>\s*/g, "")
    const start = text.indexOf("{")
    const end = text.lastIndexOf("}")
    if (start < 0 || end <= start) return
    try {
      const v = JSON.parse(text.slice(start, end + 1))
      if (typeof v?.summary !== "string" || !v.summary.trim()) return
      const list = (x: unknown) => (Array.isArray(x) ? x.filter((i): i is string => typeof i === "string") : [])
      return {
        summary: v.summary.trim(),
        decisions: list(v.decisions),
        openItems: list(v.openItems),
        artifacts: list(v.artifacts),
        nextSteps: list(v.nextSteps),
      }
    } catch {
      return
    }
  }

  /** The seed the fresh window starts from. */
  export function render(baton: Baton, context?: string) {
    const lines = [
      `[checkpoint: window ${baton.generation}] This conversation continues from a previous context window. Pick up where it left off.`,
      "",
    ]
    if (context?.trim()) lines.push(context.trim(), "")
    lines.push("Where things stand:", baton.summary)
    const sections: [string, string[]][] = [
      ["Decisions", baton.decisions],
      ["Open items", baton.openItems],
      ["Artifacts", baton.artifacts],
      ["Next steps", baton.nextSteps],
      ["TODO", baton.todos],
    ]
    for (const [label, items] of sections) {
      if (!items.length) continue
      lines.push("", `${label}:`, ...items.map((i) => `- ${i}`))
    }
    return lines.join("\n")
  }
}

/**
 * message_agent (Bot Mode, phase B4 — D5, see docs/programs/gizzi/GIZZI_BOT_MODE_SPEC.md).
 *
 * Fire-and-forget DM between bots. The tool exists ONLY in canonical bot chat
 * sessions: ToolRegistry has no session context, so the registry lists it
 * unconditionally (like every builtin) and SessionPrompt.resolveTools deletes
 * it from the per-session tool record unless the session is a canonical bot
 * chat — the same gate-and-delete precedent as applyMobileToolGating.
 * `isMessageAgentSession` below is the single predicate both sides use.
 *
 * Semantics (platform parity): the message travels as a plain tool parameter
 * (never shell-interpreted), is validated against the live roster
 * (case-insensitive, ambiguity lists the exact handles, Hermes-style), is
 * appended to the TARGET bot's durable inbox (~/.gizzi/bots/<target>/
 * inbox.jsonl, 0o600, atomic append), and the sender gets an acknowledgement.
 * The reply arrives later as a background message in the sender's canonical
 * chat (see bot-inbox.ts pickup in SessionPrompt).
 *
 * App bots (created in the Allternit app; they live in allternit-api's agents
 * table, not the ~/.gizzi/bots store) get the same tool through the API:
 * allternit-api resolves the calling bot from the session, the target from
 * the same user's roster, starts (or continues) a task thread for the target
 * under the caller's thread, and returns the reply — or a "started" handle,
 * with the result posted into this chat when it lands. See
 * runtime/bots/app-bot-routing.ts and cmd/allternit-api/src/bot_routing.rs.
 */
import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { BotStoreError, listBots, type Bot } from "@/runtime/bots/bot-store"
import { findBotByCanonicalSession, isCanonicalBotSession } from "@/runtime/bots/canonical-chat"
import { appendBotInbox } from "@/runtime/bots/bot-inbox"
import { fetchAppRoster, isAppBotSession, routeToAppBot } from "@/runtime/bots/app-bot-routing"

/**
 * The single gating predicate (D5): message_agent is offered only when the
 * current session is some bot's own chat — a gizzi-store bot's pinned
 * canonical chat, or an app bot's chat/thread (asked of allternit-api, which
 * owns those bots). Regular chats, group rooms, subagents, and coordinator
 * workers never see the tool.
 */
export async function isMessageAgentSession(sessionID: string): Promise<boolean> {
  if (!sessionID) return false
  if (await isCanonicalBotSession(sessionID)) return true
  return isAppBotSession(sessionID)
}

/** Targets that ask for the roster instead of sending a message. */
const LIST_TARGETS = new Set(["list", "?", "*", "roster", "teammates"])

/**
 * Resolve a target handle against a roster snapshot. Exact match wins; a
 * unique case-insensitive match resolves; zero matches and ambiguity are
 * structured tool errors (ambiguity lists the exact handles, Hermes-style).
 * Pure so tests can drive ambiguity with synthetic rosters — manufacturing a
 * case-collision on disk aliases directories on case-insensitive filesystems.
 */
export function matchMessageTarget(target: string, bots: Bot[]): Bot {
  const name = target.trim()
  if (!name) throw new BotStoreError("message_agent: target must be a bot name")
  const exact = bots.find((b) => b.name === name)
  if (exact) return exact
  const lower = name.toLowerCase()
  const matches = bots.filter((b) => b.name.toLowerCase() === lower)
  if (matches.length === 0) {
    const known = bots.map((b) => b.name).join(", ")
    throw new BotStoreError(
      `message_agent: unknown teammate '${name}' — known bots: ${known || "(none)"}`,
    )
  }
  if (matches.length > 1) {
    throw new BotStoreError(
      `message_agent: target '${name}' is ambiguous — it matches: ${matches.map((b) => b.name).join(", ")}`,
    )
  }
  return matches[0]!
}

/** Resolve a target handle against the live roster. */
export async function resolveMessageTarget(target: string): Promise<Bot> {
  return matchMessageTarget(target, await listBots())
}

const DESCRIPTION = `Hand work to another of your user's bots, or message a teammate bot.

- target: the bot's name, @handle or id. Lookup is case-insensitive; unknown
  targets error with the list of bots, ambiguous targets list the choices.
  Call with target "list" (no message) to see your teammates and what each
  one does, then route the work to the bot that owns it.
- message: the plain-text task or message. It travels as a tool parameter —
  never shell-interpreted, never forwarded into a running turn.

Bots made in the Allternit app: the target bot gets its own task thread
(shown on its screen and in this project, linked under this chat), works on
it, and its reply comes back as this tool's result. Long work returns a
"started" handle instead; the result is posted in this chat when it is ready.

Terminal (gizzi) bots: fire-and-forget; the message lands in the target's
inbox and the reply arrives in this chat as a background message
("Message from 🤖 <name> (@<name>): ...").

Do not use it for external email (send_agent_email) or user-facing output.`

export const MessageAgentTool = Tool.define("message_agent", {
  description: DESCRIPTION,
  parameters: z.object({
    target: z.string().describe("Bot name, @handle or id (case-insensitive), or \"list\" to see your teammates"),
    message: z
      .string()
      .optional()
      .describe("Plain-text task or message for the target bot (omit only with target \"list\")"),
  }),
  async execute(params, ctx) {
    if (!(await isCanonicalBotSession(ctx.sessionID))) return executeForAppBot(params, ctx.sessionID)

    // The sender is the bot whose canonical session is running this turn.
    // Gating guarantees this resolves; when it does not, fail closed with a
    // structured error rather than delivering under a bogus identity.
    const sender = await findBotByCanonicalSession(ctx.sessionID)
    if (!sender) {
      throw new BotStoreError(
        "message_agent: this session is not a canonical bot chat — the tool is unavailable here",
      )
    }

    if (LIST_TARGETS.has(params.target.trim().toLowerCase())) {
      const others = (await listBots()).filter((b) => b.name !== sender.name)
      return {
        title: "Teammates",
        output: others.length
          ? others.map((b) => `- ${b.name}${b.title ? `, ${b.title}` : ""}${b.description ? `: ${b.description}` : ""}`).join("\n")
          : "You have no other bots yet.",
        metadata: { target: "list", from: sender.name, delivered: false },
      }
    }
    const target = await resolveMessageTarget(params.target)
    const text = (params.message ?? "").trim()
    if (!text) throw new BotStoreError("message_agent: message must not be empty")

    await appendBotInbox(target.name, {
      from: sender.name,
      fromSessionId: ctx.sessionID,
      message: text,
      at: new Date().toISOString(),
    })

    return {
      title: `Messaged ${target.name}`,
      output: `delivered to ${target.name}'s inbox; the reply arrives as a background completion`,
      metadata: { target: target.name, from: sender.name, delivered: true },
    }
  },
})

/** message_agent for a bot made in the Allternit app (allternit-api roster + threads). */
async function executeForAppBot(params: { target: string; message?: string }, sessionID: string) {
  const roster = await fetchAppRoster(sessionID)
  if (!roster) {
    throw new BotStoreError("message_agent: this chat isn't one of your bots' chats — the tool is unavailable here")
  }
  const from = roster.caller.name
  const message = (params.message ?? "").trim()
  if (LIST_TARGETS.has(params.target.trim().toLowerCase())) {
    return {
      title: "Teammates",
      output: `Your teammates (route work to the bot that owns it):\n${roster.text}`,
      metadata: { target: "list", from, delivered: false },
    }
  }
  if (!message) throw new BotStoreError("message_agent: message must not be empty")
  const result = await routeToAppBot(sessionID, params.target, message).catch((error: Error) => {
    throw new BotStoreError(`message_agent: ${error.message}`)
  })
  const name = result.bot.name
  const base = { target: name, botId: result.bot.id, threadId: result.threadId, from, status: result.status }
  if (result.status === "done") {
    return {
      title: `Handed to ${name}`,
      output: `${name} replied (thread ${result.threadId}):\n\n${result.reply ?? ""}`,
      metadata: { ...base, delivered: true },
    }
  }
  if (result.status === "started") {
    return {
      title: `Handed to ${name}`,
      output: `${name} started on it (thread ${result.threadId}). ${result.note ?? "The result will be posted in this chat when it's ready."}`,
      metadata: { ...base, delivered: true },
    }
  }
  return {
    title: `Handed to ${name}`,
    output: `${name} couldn't finish it (thread ${result.threadId}): ${result.error ?? "unknown error"}`,
    metadata: { ...base, delivered: false },
  }
}

/**
 * Canonical bot chat (Bot Mode, phase B2 — D2/D5/D6, see
 * docs/programs/gizzi/GIZZI_BOT_MODE_SPEC.md).
 *
 * One pinned session per bot ("Bot Chat is born the moment the Bot is
 * born", lazily): the first `gizzi agents bot chat <name>` creates the session and
 * records it in `bot.json` as `canonicalSession`; every later open resumes
 * it. This module is the only owner of that pointer (bot-store exposes the
 * raw pin/stamp primitives).
 *
 * Session persistence in this codebase is the sqlite-backed `Session`
 * namespace (`src/runtime/session`, the same store `gizzi session list` and
 * `gizzi run --session` resolve against). The heavy runtime import is kept
 * behind `CanonicalChatDeps` — CLI paths inject the real implementation via
 * `runtimeDeps()`, tests inject an in-memory fake, so this file stays
 * drivable from plain bun tests with only a sandboxed GIZZI_CONFIG_DIR.
 */
import { readFile, stat } from "node:fs/promises"
import { join } from "node:path"
import {
  type Bot,
  type BotCanonicalSession,
  botDir,
  listBotMemory,
  listBots,
  pinCanonicalSession,
  readSoul,
  setCapabilityEpoch,
} from "@/runtime/bots/bot-store"
import { computeCapabilityEpoch } from "@/runtime/bots/capability-epoch"

/* -------------------------------------------------------------------------- */
/* Pluggable session store access                                             */
/* -------------------------------------------------------------------------- */

export interface CanonicalChatDeps {
  /** True when a session row with this id exists in the session store. */
  sessionExists(sessionId: string, projectPath: string): Promise<boolean>
  /** Create a fresh session bound to the given project directory. */
  createSession(projectPath: string, title: string): Promise<{ id: string }>
}

/**
 * Real deps backed by the runtime session store. Requires an active
 * `Instance` context (the CLI wraps calls in `bootstrap(projectPath, …)`;
 * the runtime/server already runs inside one).
 */
export async function runtimeDeps(): Promise<CanonicalChatDeps> {
  const { Session } = await import("@/runtime/session")
  return {
    async sessionExists(sessionId) {
      try {
        // Direct store probe: works outside bootstrap contexts too (the
        // data dir is ensured inside), where Session.get would throw.
        const { sessionExistsInStore } = await import("@/runtime/bots/session-db")
        return await sessionExistsInStore(sessionId)
      } catch {
        try {
          await Session.get(sessionId)
          return true
        } catch {
          return false
        }
      }
    },
    async createSession(projectPath, title) {
      const info = await Session.createNext({ directory: projectPath, title })
      return { id: info.id }
    },
  }
}

/* -------------------------------------------------------------------------- */
/* Canonical session resolution (D2)                                          */
/* -------------------------------------------------------------------------- */

/**
 * Resolve the bot's pinned canonical session, or null when it is unusable.
 *
 * Root→tip: runtime compaction (`SessionCompaction`) summarizes in place —
 * it never forks the jsonl into a new session id, so a compacted session
 * keeps its id and the pointer stays valid. The flows that mint a new id are
 * (a) explicit `--fork` (the forked session becomes the tip and the caller
 * re-points via `openCanonicalChat`) and (b) the ink-app `/clear`, which is
 * composer-guarded inside canonical bot chats so the relationship never
 * forks (see commands/clear/clear.ts). When the pinned row is gone entirely
 * (e.g. `gizzi session delete`), there is no reliable tip to follow — the pin
 * is treated as unusable and the caller creates + re-pins.
 */
export async function resolveCanonicalSession(
  bot: Bot,
  deps?: CanonicalChatDeps,
): Promise<BotCanonicalSession | null> {
  const pin = bot.canonicalSession
  if (!pin) return null
  const store = deps ?? (await runtimeDeps())
  try {
    if (await store.sessionExists(pin.sessionId, pin.projectPath)) return pin
  } catch {
    // session store unreadable — treat the pin as unusable, never throw here
  }
  return null
}

export interface OpenCanonicalChatResult {
  projectPath: string
  sessionId: string
  /** True when a fresh session was created (the bot was not pinned or the pin was unusable). */
  created: boolean
}

/**
 * Open the bot's canonical chat: resume the pinned session when valid,
 * otherwise create a new session in the given project directory and pin it
 * (D2 — "bot chat is born the moment the bot is born", lazy version).
 */
export async function openCanonicalChat(
  bot: Bot,
  options: { projectPath?: string } = {},
  deps?: CanonicalChatDeps,
): Promise<OpenCanonicalChatResult> {
  const store = deps ?? (await runtimeDeps())
  const existing = await resolveCanonicalSession(bot, store)
  if (existing) {
    return { projectPath: existing.projectPath, sessionId: existing.sessionId, created: false }
  }
  const projectPath = options.projectPath ?? bot.canonicalSession?.projectPath ?? process.cwd()
  const title = `Bot chat — ${bot.title}`
  const session = await store.createSession(projectPath, title)
  await pinCanonicalSession(bot.name, { projectPath, sessionId: session.id })
  return { projectPath, sessionId: session.id, created: true }
}

/* -------------------------------------------------------------------------- */
/* Canonical session identity                                                 */
/* -------------------------------------------------------------------------- */

/** The bot whose canonicalSession pins this session id, if any. */
export async function findBotByCanonicalSession(sessionId: string): Promise<Bot | null> {
  const bots = await listBots()
  return bots.find((b) => b.canonicalSession?.sessionId === sessionId) ?? null
}

/**
 * True when the given session id is some bot's pinned canonical chat. Used by
 * the TUI composer guard (`/new` reroutes to compact) — a full roster scan is
 * fine at human typing speed and roster sizes.
 */
export async function isCanonicalBotSession(sessionId: string): Promise<boolean> {
  if (!sessionId) return false
  return (await findBotByCanonicalSession(sessionId)) !== null
}

/* -------------------------------------------------------------------------- */
/* Persona + memory injection (D1/D5)                                         */
/* -------------------------------------------------------------------------- */

/** Total budget for injected memory note bodies (newest first until exhausted). */
export const MEMORY_CHAR_BUDGET = 8192

interface MemoryNote {
  name: string
  content: string
  mtimeMs: number
}

async function readMemoryNotes(bot: Bot): Promise<MemoryNote[]> {
  const names = await listBotMemory(bot.name)
  const notes: MemoryNote[] = []
  for (const name of names) {
    const file = join(botDir(bot.name), "memory", name)
    try {
      const [content, info] = await Promise.all([readFile(file, "utf8"), stat(file)])
      notes.push({ name, content, mtimeMs: info.mtimeMs })
    } catch {
      // note vanished between listing and read — skip it
    }
  }
  // Newest first so the freshest notes survive the budget cut.
  return notes.sort((a, b) => b.mtimeMs - a.mtimeMs)
}

function formatRoster(bot: Bot, roster: Bot[]): string[] {
  const teammates = roster.filter((b) => b.name !== bot.name)
  if (teammates.length === 0) return []
  const lines = teammates.map((b) =>
    b.description ? `- ${b.name} — ${b.title} (${b.description})` : `- ${b.name} — ${b.title}`,
  )
  return [
    "## Teammates",
    "",
    ...lines,
    "",
    "## Messaging protocol",
    "",
    "You can message a teammate with the message_agent tool: message_agent(target, message).",
    "Delivery is fire-and-forget; replies arrive in this chat as background messages.",
    "Unknown targets error.",
  ]
}

/**
 * Build the standing-instruction block injected into every turn of the
 * bot's canonical chat: identity line (never claim to be a different
 * assistant), SOUL.md, bounded memory notes, and the teammate roster.
 *
 * `preamble` opts the turn into the first-response nudge: turns that came
 * from a person (user chat or teammate DM) get a short "what I'm about to
 * do" sentence before the first tool call; routine/internal turns skip it.
 */
export async function buildPersonaInjection(
  bot: Bot,
  opts?: { preamble?: boolean },
): Promise<string> {
  const [soul, notes, roster] = await Promise.all([readSoul(bot.name), readMemoryNotes(bot), listBots()])

  const sections: string[] = [
    "# Bot identity",
    "",
    `You are ${bot.title} (${bot.name}), a bot on this machine. Never claim to be a different assistant.`,
  ]
  if (bot.description) sections.push("", bot.description)

  if (soul?.trim()) {
    sections.push("", "## SOUL (standing instructions)", "", soul.trim())
  }

  const memorySections: string[] = []
  let budget = MEMORY_CHAR_BUDGET
  let omitted = 0
  for (const note of notes) {
    if (budget <= 0) {
      omitted++
      continue
    }
    const body = note.content.trim()
    if (!body) continue
    const text = `### ${note.name}\n\n${body}`
    memorySections.push("", text)
    budget -= text.length
  }
  if (memorySections.length > 0) {
    sections.push("", "## Memory notes (bot-scoped, newest first)", ...memorySections)
    if (omitted > 0 || budget < 0) {
      sections.push("", `(memory truncated to ${MEMORY_CHAR_BUDGET} chars)`)
    }
  }

  sections.push(...formatRoster(bot, roster))
  if (opts?.preamble) {
    sections.push(
      "",
      "## First response",
      "",
      "When this turn came from a person (a chat message or a teammate DM), begin your reply with one short sentence (25 words max, plain text, no emoji) naming what you are about to do, before your first tool call. Skip this for routine runs and internal turns.",
    )
  }
  return sections.join("\n")
}

/**
 * Detect capability-epoch drift (D5) and re-stamp. The injection itself is
 * stateless (rebuilt per turn by the prompt pipeline), so "rebuild once"
 * means: when the computed epoch differs from the stamped one, rebuild is
 * implied and the new epoch is written — subsequent opens see no drift and
 * do not rewrite.
 *
 * Returns the (possibly new) epoch.
 */
export async function ensureCapabilityEpoch(bot: Bot): Promise<string> {
  const [soul, memoryFiles, roster] = await Promise.all([
    readSoul(bot.name),
    listBotMemory(bot.name),
    listBots(),
  ])
  const epoch = computeCapabilityEpoch({
    name: bot.name,
    title: bot.title,
    description: bot.description,
    model: bot.model,
    soul,
    memoryFiles,
    rosterNames: roster.map((b) => b.name),
  })
  if (bot.capabilityEpoch !== epoch) {
    await setCapabilityEpoch(bot.name, epoch)
  }
  return epoch
}

/**
 * Prompt-pipeline hook: when `sessionID` is a bot's canonical chat, return
 * the persona injection block and make sure the capability epoch is stamped;
 * otherwise return undefined. Called once per model step from
 * `SessionPrompt` — the fs reads below mirror what `InstructionPrompt`
 * already does per step, so the cost is in line with existing behavior.
 *
 * `preamble` marks turns that originated from a person (user chat or
 * teammate DM pickup); routine deliveries and internal/subagent turns call
 * without it so the first-response nudge stays off.
 */
export async function botChatSystemPrompt(
  sessionID: string,
  opts?: { preamble?: boolean },
): Promise<string | undefined> {
  const bot = await findBotByCanonicalSession(sessionID)
  if (!bot) return undefined
  const injection = await buildPersonaInjection(bot, opts)
  await ensureCapabilityEpoch(bot).catch(() => {
    // Stamping is best-effort — never break the turn on a store write failure.
  })
  return injection
}

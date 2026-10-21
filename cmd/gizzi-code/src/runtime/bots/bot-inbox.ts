/**
 * Bot inbox (Bot Mode, phase B4 — D5, see docs/programs/gizzi/GIZZI_BOT_MODE_SPEC.md).
 *
 * Fire-and-forget DM transport between bots. `message_agent` appends a durable
 * envelope to the TARGET bot's `inbox.jsonl`; when the target's canonical chat
 * starts a turn, SessionPrompt drains the inbox and injects each envelope as a
 * user-role message with the platform's exact attribution prefix
 * (`Message from 🤖 <sender> (@<sender>): <message>`, parity with
 * surfaces/ai.allternit.com mention-handoff.service.ts).
 *
 * Filesystem notes:
 * - `inbox.jsonl` is created 0o600 inside the bot's 0o700 home.
 * - Append is a single O_APPEND write — atomic for line-sized payloads, never
 *   shell-interpreted (envelopes are JSON, messages travel as tool params).
 * - Drain renames the file aside before reading and deletes the rename after a
 *   successful parse: envelopes that arrive mid-drain land in a fresh
 *   inbox.jsonl and are picked up on the next turn instead of being lost.
 *
 * This module is deliberately free of CLI/UI/session imports so bun tests can
 * drive it with only a sandboxed GIZZI_CONFIG_DIR.
 */
import { createHash } from "node:crypto"
import { appendFile, mkdir, open, readFile, rename, rm } from "node:fs/promises"
import { join } from "node:path"
import { botDir } from "@/runtime/bots/bot-store"
import { findBotByCanonicalSession } from "@/runtime/bots/canonical-chat"

/* -------------------------------------------------------------------------- */
/* Envelope                                                                   */
/* -------------------------------------------------------------------------- */

export interface BotInboxEnvelope {
  id: string
  /** Sender bot name (the bot whose canonical session sent the message). */
  from: string
  /** Sender bot's canonical session id — provenance, not a routing key. */
  fromSessionId: string
  message: string
  /** ISO timestamp. */
  at: string
}

/** Stable per-envelope id derived from content + time (no Instance context needed). */
export function envelopeId(input: Omit<BotInboxEnvelope, "id">): string {
  return createHash("sha256")
    .update(JSON.stringify([input.from, input.fromSessionId, input.message, input.at]))
    .digest("hex")
    .slice(0, 24)
}

export function inboxPath(botName: string): string {
  return join(botDir(botName), "inbox.jsonl")
}

/* -------------------------------------------------------------------------- */
/* Append (message_agent delivery)                                            */
/* -------------------------------------------------------------------------- */

/**
 * Append one envelope to the bot's inbox.jsonl (0o600, O_APPEND single write).
 * The bot directory is created defensively (0o700) — message_agent validates
 * the target against the live roster first, so this is a race-safe no-op in
 * practice. Returns the stored envelope (with id).
 */
export async function appendBotInbox(
  botName: string,
  input: Omit<BotInboxEnvelope, "id">,
): Promise<BotInboxEnvelope> {
  const envelope: BotInboxEnvelope = { ...input, id: envelopeId(input) }
  const dir = botDir(botName)
  await mkdir(dir, { recursive: true, mode: 0o700 })
  const path = join(dir, "inbox.jsonl")
  try {
    // O_APPEND single write: atomic for line-sized payloads; mode applies when
    // the file is created.
    const handle = await open(path, "a", 0o600)
    try {
      await handle.write(JSON.stringify(envelope) + "\n")
    } finally {
      await handle.close()
    }
  } catch (err) {
    // Filesystems without O_APPEND atomicity guarantees (or read-only homes)
    // fall back to a best-effort append; mode is applied on creation.
    await appendFile(path, JSON.stringify(envelope) + "\n", { mode: 0o600 })
    void err
  }
  return envelope
}

/**
 * Count pending envelopes in the bot's inbox.jsonl (non-empty lines).
 * Returns 0 when there is no inbox or it is unreadable — a pending-message
 * badge must never break the roster. Used by bot-roster (phase B5).
 */
export async function countBotInbox(botName: string): Promise<number> {
  try {
    const raw = await readFile(inboxPath(botName), "utf8")
    let count = 0
    for (const line of raw.split("\n")) {
      if (line.trim()) count++
    }
    return count
  } catch {
    return 0
  }
}

/* -------------------------------------------------------------------------- */
/* Drain (turn-start pickup)                                                  */
/* -------------------------------------------------------------------------- */

function parseEnvelopes(raw: string): BotInboxEnvelope[] {
  const envelopes: BotInboxEnvelope[] = []
  for (const line of raw.split("\n")) {
    const trimmed = line.trim()
    if (!trimmed) continue
    try {
      const parsed = JSON.parse(trimmed) as Partial<BotInboxEnvelope>
      if (
        typeof parsed?.from === "string" &&
        typeof parsed?.fromSessionId === "string" &&
        typeof parsed?.message === "string" &&
        typeof parsed?.at === "string"
      ) {
        envelopes.push({
          id: typeof parsed.id === "string" ? parsed.id : envelopeId(parsed as Omit<BotInboxEnvelope, "id">),
          from: parsed.from,
          fromSessionId: parsed.fromSessionId,
          message: parsed.message,
          at: parsed.at,
        })
      }
      // Malformed lines are dropped, not fatal: one bad envelope must not
      // wedge the inbox forever.
    } catch {
      // unparseable line — skip
    }
  }
  return envelopes
}

/**
 * Drain the bot's inbox: rename aside, read, delete the renamed file after a
 * successful read. Returns the envelopes in file order. Envelopes written
 * after the rename land in the fresh inbox.jsonl and survive for the next
 * turn. Returns [] when there is no inbox.
 */
export async function drainBotInbox(botName: string): Promise<BotInboxEnvelope[]> {
  const path = inboxPath(botName)
  const staging = `${path}.drain-${process.pid}-${Math.random().toString(36).slice(2)}`
  try {
    await rename(path, staging)
  } catch {
    return [] // no inbox (or unstaged race) — nothing to pick up
  }
  try {
    const raw = await readFile(staging, "utf8")
    const envelopes = parseEnvelopes(raw)
    await rm(staging, { force: true })
    return envelopes
  } catch (err) {
    // Unreadable staging file: put it back so no envelope is silently lost.
    await rename(staging, path).catch(() => {})
    throw err
  }
}

/* -------------------------------------------------------------------------- */
/* Attribution + turn-start collection                                        */
/* -------------------------------------------------------------------------- */

/**
 * Platform-parity attribution line (mention-handoff.service.ts
 * formatAttributionMessage): `Message from 🤖 <sender> (@<sender>): <message>`.
 */
export function formatInboxAttribution(envelope: BotInboxEnvelope): string {
  return `Message from 🤖 ${envelope.from} (@${envelope.from}): ${envelope.message}`
}

/**
 * Turn-start pickup used by SessionPrompt: when `sessionID` is a bot's
 * canonical chat, drain that bot's inbox and return the attributed user-role
 * message texts (in delivery order). Non-canonical sessions and store errors
 * yield [] — inbox pickup must never break a turn.
 */
export async function collectBotInboxMessages(sessionID: string): Promise<string[]> {
  if (!sessionID) return []
  let bot: { name: string } | null = null
  try {
    bot = await findBotByCanonicalSession(sessionID)
  } catch {
    return []
  }
  if (!bot) return []
  try {
    const envelopes = await drainBotInbox(bot.name)
    return envelopes.map(formatInboxAttribution)
  } catch {
    return []
  }
}

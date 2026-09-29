/**
 * Register a local `gizzi bot` as a platform bot (decision 2026-09-28,
 * HANDOFF-gateway-key-and-bots-storage Part 2): the terminal `/bots` chat, the
 * pet HUD and Desktop then show one conversation per bot, the platform bot's
 * standing thread.
 *
 * The link lives in bot.json (`platform: { id, syncHash }`). The id is minted
 * here and saved before the create call; the API's create is idempotent on
 * id, so a lost response or a crash never makes a second agent. Identity
 * (title, description, model pin, SOUL.md) is pushed again only when its hash
 * changes. A bot without a model pin is registered with an empty model and
 * provider, which the platform treats as "follow the platform default".
 *
 * Free of CLI/UI imports so bun tests can drive it with a stubbed fetch.
 */
import { createHash, randomUUID } from "node:crypto"
import { type Bot, getBot, readSoul, setPlatformLink } from "@/runtime/bots/bot-store"
import { platformRequest } from "@/runtime/bots/platform-api"
import { parseModelRef } from "@/runtime/bots/platform-threads"

const LOOKUP = { timeoutMs: 15_000 }

/** What the platform agent carries for this bot. */
export function platformBotFields(bot: Bot, soul: string | null) {
  const displayName = bot.title.trim() || bot.name
  const model = parseModelRef(bot.model)
  return {
    // The API wants at least 3 and 10 characters here.
    name: displayName.length >= 3 ? displayName : `${displayName} bot`,
    description: bot.description.trim().length >= 10 ? bot.description.trim() : `${displayName}, a gizzi terminal bot`,
    type: "worker",
    model: model?.modelID ?? "",
    provider: model?.providerID ?? "",
    system_prompt: soul?.trim() || undefined,
    harness_config: { mode: "local" },
    enabled_modes: ["chat"],
    trust_tier: "standard",
    is_bot: true,
    bot_profile: { displayName, tagline: bot.description.trim() || undefined },
    config: { localBot: bot.name },
  }
}

export function platformBotHash(fields: ReturnType<typeof platformBotFields>): string {
  return createHash("sha256").update(JSON.stringify(fields)).digest("hex").slice(0, 32)
}

export interface EnsurePlatformBotDeps {
  getBot?: typeof getBot
  readSoul?: typeof readSoul
  setPlatformLink?: typeof setPlatformLink
  newId?: () => string
}

/**
 * The platform agent id for local bot `name`, registering or updating it
 * first when needed. Throws PlatformSignedOutError / PlatformApiError from
 * the API; the caller decides whether to fall back to a local chat.
 */
export async function ensurePlatformBot(name: string, deps: EnsurePlatformBotDeps = {}): Promise<{ id: string; bot: Bot }> {
  const d = {
    getBot: deps.getBot ?? getBot,
    readSoul: deps.readSoul ?? readSoul,
    setPlatformLink: deps.setPlatformLink ?? setPlatformLink,
    newId: deps.newId ?? (() => `gizzi-bot-${randomUUID()}`),
  }
  const bot = await d.getBot(name)
  if (!bot) throw new Error(`bot '${name}' not found`)
  const fields = platformBotFields(bot, await d.readSoul(bot.name))
  const hash = platformBotHash(fields)

  let link = bot.platform ?? null
  const minted = !link
  if (!link) {
    // Save the id before creating, so a retry reuses it.
    link = { id: d.newId(), syncHash: null }
    await d.setPlatformLink(bot.name, link)
  }

  // Idempotent on id: creates the agent, or returns the existing one (which
  // also re-creates it if it was deleted on the platform).
  const created = await platformRequest<{ agent?: { id?: string } }>("POST", "/api/v1/agents", { id: link.id, ...fields }, LOOKUP)
  if (created?.agent?.id && created.agent.id !== link.id) {
    throw new Error(`the platform registered '${bot.name}' under a different id (${created.agent.id})`)
  }
  if (link.syncHash !== hash) {
    // An agent created just now already has these fields; an existing one
    // (even from an interrupted earlier attempt) gets them now.
    if (!minted) {
      await platformRequest("PATCH", `/api/v1/agents/${encodeURIComponent(link.id)}`, fields, LOOKUP)
    }
    await d.setPlatformLink(bot.name, { id: link.id, syncHash: hash })
  }
  return { id: link.id, bot }
}

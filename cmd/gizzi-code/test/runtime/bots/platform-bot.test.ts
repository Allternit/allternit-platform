import { afterAll, beforeEach, describe, expect, test } from "bun:test"

process.env.ALLTERNIT_API_TOKEN = "test-token"
const { ensurePlatformBot, platformBotFields, platformBotHash } = await import("../../../src/runtime/bots/platform-bot")

type Call = { method: string; path: string; body?: any }
let calls: Call[] = []
let createdId: string | undefined
const realFetch = globalThis.fetch

beforeEach(() => {
  calls = []
  createdId = undefined
  globalThis.fetch = (async (input: string, init: RequestInit = {}) => {
    const url = new URL(input)
    const body = init.body ? JSON.parse(String(init.body)) : undefined
    calls.push({ method: init.method ?? "GET", path: url.pathname, body })
    return new Response(JSON.stringify({ agent: { id: createdId ?? body?.id } }), { status: 201 })
  }) as typeof fetch
})
afterAll(() => {
  globalThis.fetch = realFetch
})

const baseBot = {
  schemaVersion: 1 as const,
  name: "ab",
  title: "",
  description: "",
  model: null as string | null,
  avatar: null,
  canonicalSession: null,
  capabilityEpoch: null,
  createdAt: "2026-09-28T00:00:00Z",
  updatedAt: "2026-09-28T00:00:00Z",
}

function store(bot: any, soul: string | null = null) {
  const links: any[] = []
  return {
    links,
    deps: {
      getBot: async () => bot,
      readSoul: async () => soul,
      setPlatformLink: async (_name: string, link: any) => {
        links.push(link)
        bot.platform = link
        return bot
      },
      newId: () => "gizzi-bot-1",
    },
  }
}

describe("platformBotFields", () => {
  test("meets the API's minimums and leaves an unpinned model to the platform default", () => {
    const f = platformBotFields(baseBot as any, null)
    expect(f.name).toBe("ab bot")
    expect(f.description.length).toBeGreaterThanOrEqual(10)
    expect([f.model, f.provider]).toEqual(["", ""])
    expect(f.is_bot).toBe(true)
    expect(f.config).toEqual({ localBot: "ab" })
  })

  test("a pinned model is sent as provider and model", () => {
    const f = platformBotFields({ ...baseBot, title: "Scout", model: "anthropic/claude-sonnet-5" } as any, "Be terse.")
    expect([f.provider, f.model, f.system_prompt]).toEqual(["anthropic", "claude-sonnet-5", "Be terse."])
  })
})

describe("ensurePlatformBot", () => {
  test("first open: saves the id before creating, then records the hash; no patch", async () => {
    const bot = { ...baseBot, title: "Scout" }
    const { deps, links } = store(bot)
    const { id } = await ensurePlatformBot("ab", deps)
    expect(id).toBe("gizzi-bot-1")
    expect(links[0]).toEqual({ id: "gizzi-bot-1", syncHash: null })
    expect(calls.map((c) => [c.method, c.path])).toEqual([["POST", "/api/v1/agents"]])
    expect(calls[0]!.body.id).toBe("gizzi-bot-1")
    expect(links[1]).toEqual({ id: "gizzi-bot-1", syncHash: platformBotHash(platformBotFields(bot as any, null)) })
  })

  test("unchanged identity: only the idempotent create, nothing written", async () => {
    const bot: any = { ...baseBot, title: "Scout" }
    bot.platform = { id: "gizzi-bot-9", syncHash: platformBotHash(platformBotFields(bot, null)) }
    const { deps, links } = store(bot)
    expect((await ensurePlatformBot("ab", deps)).id).toBe("gizzi-bot-9")
    expect(calls.map((c) => c.method)).toEqual(["POST"])
    expect(links).toEqual([])
  })

  test("changed identity (or an interrupted first attempt) is patched", async () => {
    const bot: any = { ...baseBot, title: "Scout", platform: { id: "gizzi-bot-9", syncHash: null } }
    const { deps, links } = store(bot, "New soul")
    await ensurePlatformBot("ab", deps)
    expect(calls.map((c) => [c.method, c.path])).toEqual([
      ["POST", "/api/v1/agents"],
      ["PATCH", "/api/v1/agents/gizzi-bot-9"],
    ])
    expect(calls[1]!.body.system_prompt).toBe("New soul")
    expect(links.at(-1)!.syncHash).toBe(platformBotHash(platformBotFields(bot, "New soul")))
  })

  test("refuses an id the platform didn't honor", async () => {
    createdId = "someone-else"
    const { deps } = store({ ...baseBot })
    await expect(ensurePlatformBot("ab", deps)).rejects.toThrow("different id")
  })
})

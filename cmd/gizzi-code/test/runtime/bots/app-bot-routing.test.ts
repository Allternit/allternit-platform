// @ts-nocheck
import { afterAll, beforeEach, describe, expect, test } from "bun:test"

/**
 * message_agent for bots made in the Allternit app: they live in
 * allternit-api's agents table, not ~/.gizzi/bots, so availability, the
 * roster and delivery all come from the API's /bot-routing routes. No live
 * API: fetch is stubbed and records every call.
 */
import { mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN = "internal-test"
process.env.GIZZI_CONFIG_DIR = mkdtempSync(join(tmpdir(), "gizzi-app-routing-"))
const { appBotCaller, isAppBotMainChat, isAppBotSession, resetAppBotSessionCache } = await import(
  "../../../src/runtime/bots/app-bot-routing"
)
const { MessageAgentTool, isMessageAgentSession } = await import("../../../src/runtime/tools/builtins/message-agent")

type Call = { path: string; body: any; headers: Record<string, string> }
let calls: Call[] = []
let route: (path: string, body: any) => Response
const realFetch = globalThis.fetch

const ROSTER = {
  caller: { id: "main", name: "A://", threadId: "t-main", projectId: "p1" },
  bots: [{ id: "scout", name: "Scout", tagline: "Finds leads" }],
  text: "- Scout: Finds leads",
}

beforeEach(() => {
  calls = []
  resetAppBotSessionCache()
  route = (path, body) => {
    if (body.sessionId !== "ses_app_main") return new Response(JSON.stringify({ error: "not a bot chat" }), { status: 404 })
    if (path.endsWith("/bot-routing/roster")) return Response.json(ROSTER)
    return Response.json({
      status: "done",
      threadId: "t-scout",
      bot: { id: "scout", name: "Scout" },
      reply: "Three bakeries: …",
    })
  }
  globalThis.fetch = (async (input: string, init: RequestInit = {}) => {
    const url = new URL(input)
    const body = JSON.parse(String(init.body))
    calls.push({ path: url.pathname, body, headers: init.headers as Record<string, string> })
    return route(url.pathname, body)
  }) as typeof fetch
})

afterAll(() => {
  globalThis.fetch = realFetch
  delete process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN
})

async function run(params: any, sessionID: string) {
  const tool = await MessageAgentTool.init()
  return tool.execute(params, { sessionID } as any)
}

describe("availability for app bots", () => {
  test("an app bot's chat gets message_agent; a regular chat doesn't", async () => {
    expect(await isMessageAgentSession("ses_app_main")).toBe(true)
    expect(await isMessageAgentSession("ses_regular")).toBe(false)
    expect(calls[0].path).toBe("/api/v1/bot-routing/roster")
    expect(calls[0].headers["x-allternit-internal-token"]).toBe("internal-test")
  })

  test("the gating answer is cached per session", async () => {
    await isAppBotSession("ses_app_main")
    await isAppBotSession("ses_app_main")
    expect(calls.length).toBe(1)
  })

  test("isAppBotSession, isAppBotMainChat and appBotCaller share one roster read", async () => {
    expect(await isAppBotSession("ses_app_main")).toBe(true)
    expect(await isAppBotMainChat("ses_app_main")).toBe(false)
    expect(await appBotCaller("ses_app_main")).toMatchObject({ id: "main", projectId: "p1" })
    expect(await appBotCaller("ses_regular")).toBeNull()
    expect(await isAppBotMainChat("ses_regular")).toBe(false)
    expect(calls.map((c) => c.body.sessionId)).toEqual(["ses_app_main", "ses_regular"])
  })

  test("resetAppBotSessionCache clears the shared cache", async () => {
    await appBotCaller("ses_app_main")
    resetAppBotSessionCache()
    route = (path) => (path.endsWith("/roster") ? Response.json({ ...ROSTER, caller: { ...ROSTER.caller, mainChat: true } }) : Response.json({}))
    expect(await isAppBotMainChat("ses_app_main")).toBe(true)
    expect(calls.length).toBe(2)
  })

  test("a failed roster read is never thrown and caches as not an app bot", async () => {
    route = () => new Response("boom", { status: 500 })
    expect(await appBotCaller("ses_app_main")).toBeNull()
    expect(await isAppBotSession("ses_app_main")).toBe(false)
    expect(calls.length).toBe(1)
  })
})

describe("routing", () => {
  test("list returns the roster with each bot's job", async () => {
    const out = await run({ target: "list" }, "ses_app_main")
    expect(out.output).toContain("- Scout: Finds leads")
    expect(calls.map((c) => c.path)).toEqual(["/api/v1/bot-routing/roster"])
  })

  test("a hand-off returns the target's reply and the thread", async () => {
    const out = await run({ target: "scout", message: "Find 3 bakeries" }, "ses_app_main")
    expect(out.title).toBe("Handed to Scout")
    expect(out.output).toContain("Three bakeries")
    expect(out.metadata).toMatchObject({ target: "Scout", botId: "scout", threadId: "t-scout", status: "done", from: "A://" })
    const sent = calls.find((c) => c.path.endsWith("/message"))!
    // The session id identifies the caller; the model's input is only target + message.
    expect(sent.body).toMatchObject({ sessionId: "ses_app_main", target: "scout", message: "Find 3 bakeries" })
  })

  test("long work comes back as a started handle", async () => {
    route = (path, body) =>
      path.endsWith("/roster")
        ? Response.json(ROSTER)
        : Response.json({ status: "started", threadId: "t-scout", bot: { id: "scout", name: "Scout" }, note: "Scout is still working." })
    const out = await run({ target: "Scout", message: "Research 50 companies" }, "ses_app_main")
    expect(out.output).toContain("Scout started on it (thread t-scout)")
    expect(out.metadata.status).toBe("started")
  })

  test("API refusals (unknown bot, other user's bot) surface as tool errors", async () => {
    route = (path) =>
      path.endsWith("/roster")
        ? Response.json(ROSTER)
        : new Response(JSON.stringify({ error: "There's no bot called 'Mallory'. Your bots: Scout." }), { status: 400 })
    await expect(run({ target: "Mallory", message: "hi" }, "ses_app_main")).rejects.toThrow(/no bot called 'Mallory'/)
  })

  test("empty messages are refused before anything is sent", async () => {
    await expect(run({ target: "scout", message: "  " }, "ses_app_main")).rejects.toThrow(/message must not be empty/)
    expect(calls.some((c) => c.path.endsWith("/message"))).toBe(false)
  })
})

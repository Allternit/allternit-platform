// @ts-nocheck
import { afterAll, beforeEach, describe, expect, test } from "bun:test"

/**
 * ask_user_later through the real tool, the real Question store (sqlite) and
 * the real routing client. Stubbed edges: fetch (allternit-api user/notify)
 * and the model turn (Question.asyncTurn.start records the turn it would run).
 */
import { mkdtempSync } from "node:fs"
import { tmpdir as osTmpdir } from "node:os"
import path from "path"

process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN = "internal-test"
process.env.GIZZI_CONFIG_DIR = mkdtempSync(path.join(osTmpdir(), "gizzi-ask-user-later-"))
const { tmpdir } = await import("../fixture/fixture")
const { Instance } = await import("../../src/runtime/context/project/instance")
const { Session } = await import("../../src/runtime/session")
const { Bus } = await import("../../src/shared/bus")
const { Question } = await import("../../src/runtime/integrations/question")
const { Agent } = await import("../../src/runtime/loop/agent")
const { PermissionNext } = await import("../../src/permission/next")
const { ToolRegistry } = await import("../../src/runtime/tools/builtins/registry")
const { AskUserLaterTool } = await import("../../src/runtime/tools/builtins/ask-user-later")
const { AgentCompatRoutes } = await import("../../src/runtime/server/routes/agent-compat")

type Call = { path: string; body: any }
let calls: Call[] = []
let turns: { sessionID: string; requestID: string; text: string }[] = []
const realFetch = globalThis.fetch
const realStart = Question.asyncTurn.start

beforeEach(() => {
  calls = []
  turns = []
  globalThis.fetch = (async (input: string, init: RequestInit = {}) => {
    const url = new URL(input)
    const body = JSON.parse(String(init.body))
    calls.push({ path: url.pathname, body })
    if (url.pathname.endsWith("/bot-routing/user/notify")) return Response.json({ pushed: true, event: "thread.needs_user" })
    return new Response(JSON.stringify({ error: "unexpected" }), { status: 500 })
  }) as typeof fetch
  Question.asyncTurn.start = async (input) => {
    turns.push(input)
  }
})

afterAll(() => {
  globalThis.fetch = realFetch
  Question.asyncTurn.start = realStart
  delete process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN
})

function ctx(sessionID: string) {
  return {
    sessionID,
    messageID: "msg_assistant",
    callID: "call_1",
    agent: "build",
    abort: AbortSignal.any([]),
    messages: [],
    metadata: () => {},
    ask: async () => {},
  } as any
}

const question = {
  question: "Which region should the staging database live in?",
  header: "Region",
  options: [
    { label: "us-east", description: "Next to the API", recommended: true },
    { label: "eu-west", description: "Next to most users" },
  ],
}

async function ask(sessionID: string, assumption = "Using us-east until you say otherwise") {
  const tool = await AskUserLaterTool.init()
  return tool.execute({ questions: [question], assumption }, ctx(sessionID))
}

/** Lets the fire-and-forget answer turn run. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 10))

describe("tool.ask_user_later", () => {
  test("returns at once, persists the ask, pushes needs_you, and the reply starts a new marked turn", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const session = await Session.create({})
        const result = await ask(session.id)
        const id = result.metadata.ask_id
        expect(result.output).toBe(
          `Asked (ask_id ${id}); the answer will arrive as a new message. Continue with your stated assumption.`,
        )
        expect(result.metadata.push).toBe("sent")
        const notify = calls.find((c) => c.path.endsWith("/bot-routing/user/notify"))
        expect(notify.body).toMatchObject({ sessionId: session.id, kind: "question", level: "needs_you", ref: id })

        const listed = (await Question.list()).find((q) => q.id === id)
        expect(listed).toMatchObject({
          sessionID: session.id,
          async: true,
          assumption: "Using us-east until you say otherwise",
        })

        await Question.reply({ requestID: id, answers: [["eu-west"]] })
        await settle()
        expect(turns).toHaveLength(1)
        expect(turns[0].sessionID).toBe(session.id)
        expect(turns[0].text.startsWith(`[answer to your question ${id}]`)).toBe(true)
        expect(turns[0].text).toContain("Which region should the staging database live in?")
        expect(turns[0].text).toContain("→ eu-west")
        expect(turns[0].text).toContain("You had assumed: Using us-east until you say otherwise")
        expect((await Question.list()).some((q) => q.id === id)).toBe(false)

        // A second reply (another device) starts no second turn.
        await Question.reply({ requestID: id, answers: [["us-east"]] })
        await settle()
        expect(turns).toHaveLength(1)
      },
    })
  })

  test("an ask survives a restart and can still be answered", async () => {
    await using tmp = await tmpdir({ git: true })
    let id = ""
    let sessionID = ""
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        sessionID = (await Session.create({})).id
        id = (await ask(sessionID)).metadata.ask_id
        // Drops every in-memory question (what a process restart does).
        await Instance.dispose()
      },
    })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const listed = (await Question.list()).find((q) => q.id === id)
        expect(listed).toMatchObject({ sessionID, async: true })
        await Question.reply({ requestID: id, answers: [["us-east"]] })
        await settle()
        expect(turns.map((t) => t.requestID)).toEqual([id])
      },
    })
  })

  test("a thread holds at most 3 pending asks; the 4th is refused", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const session = await Session.create({})
        for (let i = 0; i < 3; i++) await ask(session.id)
        await expect(ask(session.id)).rejects.toThrow("the limit is 3")
        expect((await Question.listPendingAsync(session.id)).length).toBe(3)

        // Answering one frees a slot.
        const first = (await Question.listPendingAsync(session.id))[0]
        await Question.reply({ requestID: first.id, answers: [["us-east"]] })
        const fourth = await ask(session.id)
        expect(fourth.metadata.ask_id).toBeTruthy()
      },
    })
  })

  test("an unanswered ask expires after 7 days without a turn", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const session = await Session.create({})
        const id = (await ask(session.id)).metadata.ask_id
        const rejected: string[] = []
        const unsubscribe = Bus.subscribe(Question.Event.Rejected, (event) => {
          rejected.push(event.properties.requestID)
        })
        try {
          // The sweep covers the whole runtime, so other tests' asks may expire with it.
          expect(await Question.expireDue(Date.now() + 6 * 24 * 60 * 60 * 1000)).not.toContain(id)
          expect(await Question.expireDue(Date.now() + Question.ASYNC_EXPIRY_MS + 1000)).toContain(id)
          await settle()
          expect(rejected).toContain(id)
          expect((await Question.list()).some((q) => q.id === id)).toBe(false)

          // A late answer starts nothing.
          await Question.reply({ requestID: id, answers: [["eu-west"]] })
          await settle()
          expect(turns).toHaveLength(0)
        } finally {
          unsubscribe()
        }
      },
    })
  })

  test("a subagent's ask attaches to the root session", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const root = await Session.create({})
        const child = await Session.create({ parentID: root.id })
        const grandchild = await Session.create({ parentID: child.id })
        const result = await ask(grandchild.id)
        expect(result.metadata.sessionID).toBe(root.id)
        expect((await Question.listPendingAsync(root.id)).map((q) => q.id)).toEqual([result.metadata.ask_id])
        expect(calls.find((c) => c.path.endsWith("/user/notify")).body.sessionId).toBe(root.id)

        await Question.reply({ requestID: result.metadata.ask_id, answers: [["us-east"]] })
        await settle()
        expect(turns[0].sessionID).toBe(root.id)
      },
    })
  })

  test("an ask filed through the engine route (the Gizzi TUI) publishes question.asked live and pushes", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const session = await Session.create({})
        const asked: any[] = []
        const unsubscribe = Bus.subscribe(Question.Event.Asked, (event) => {
          asked.push(event.properties)
        })
        const post = (body: unknown) =>
          AgentCompatRoutes().request("/v1/questions/async", {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify(body),
          })
        try {
          const res = await post({ sessionID: session.id, questions: [question], assumption: "Using us-east for now" })
          expect(res.status).toBe(200)
          const body = await res.json()
          expect(body.push).toBe("sent")
          await settle()
          expect(asked).toEqual([
            expect.objectContaining({ id: body.request.id, sessionID: session.id, async: true, assumption: "Using us-east for now" }),
          ])
          expect(calls.find((c) => c.path.endsWith("/user/notify")).body).toMatchObject({ sessionId: session.id, ref: body.request.id })
          expect((await Question.list()).some((q) => q.id === body.request.id)).toBe(true)

          // The limit comes back as 429 so the terminal reports it instead of writing the ask itself.
          for (let i = 0; i < 2; i++) await post({ sessionID: session.id, questions: [question], assumption: "x" })
          const refused = await post({ sessionID: session.id, questions: [question], assumption: "x" })
          expect(refused.status).toBe(429)
          expect((await refused.json()).error).toContain("the limit is 3")

          // A session the engine doesn't store is refused (the terminal then files it locally).
          expect((await post({ sessionID: "ses_missing", questions: [question], assumption: "x" })).status).toBe(404)
        } finally {
          unsubscribe()
        }
      },
    })
  })

  test("pending asks list and expire across every project on the runtime (a terminal session sits in the default project)", async () => {
    await using terminal = await tmpdir()
    await using project = await tmpdir({ git: true })
    let id = ""
    let sessionID = ""
    await Instance.provide({
      directory: terminal.path,
      fn: async () => {
        sessionID = (await Session.create({})).id
        id = (await ask(sessionID)).metadata.ask_id
      },
    })
    await Instance.provide({
      directory: project.path,
      fn: async () => {
        expect((await Question.list()).find((q) => q.id === id)).toMatchObject({ sessionID, async: true })
        expect(await Question.expireDue(Date.now() + Question.ASYNC_EXPIRY_MS + 1000)).toContain(id)
        expect((await Question.list()).some((q) => q.id === id)).toBe(false)
      },
    })
  })

  test("allowed for the default agents and offered to every client", async () => {
    const prev = process.env.GIZZI_CLIENT
    await using tmp = await tmpdir()
    try {
      for (const client of ["app", "cli", "desktop", "headless"]) {
        process.env.GIZZI_CLIENT = client
        await Instance.provide({
          directory: tmp.path,
          fn: async () => {
            expect(await ToolRegistry.ids()).toContain("ask_user_later")
            for (const name of ["build", "general"]) {
              const agent = await Agent.get(name)
              expect(PermissionNext.evaluate("ask_user_later", "*", agent.permission).action).toBe("allow")
            }
          },
        })
      }
    } finally {
      if (prev === undefined) delete process.env.GIZZI_CLIENT
      else process.env.GIZZI_CLIENT = prev
    }
  })
})

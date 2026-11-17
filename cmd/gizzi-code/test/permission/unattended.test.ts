// @ts-nocheck
import { afterAll, beforeEach, expect, test } from "bun:test"

/**
 * An approval ask in a turn a scheduler started (metadata.automation_run, or
 * a bot routine's "[routine: …]" marker) is refused at once instead of
 * waiting for a person until the run times out, and the owner gets one
 * needs_you push per run. Real permission code and the real session store;
 * only the HTTP edge (allternit-api user/notify) is stubbed.
 */
process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN = "internal-test"
const { PermissionNext } = await import("../../src/permission/next")
const { Instance } = await import("../../src/project/instance")
const { Session } = await import("../../src/runtime/session")
const { Identifier } = await import("../../src/shared/id/id")
const { tmpdir } = await import("../fixture/fixture")

const realFetch = globalThis.fetch
let notifies: any[] = []

beforeEach(() => {
  notifies = []
  globalThis.fetch = (async (input: string, init: RequestInit = {}) => {
    const url = new URL(input)
    if (url.pathname.endsWith("/bot-routing/user/notify")) notifies.push(JSON.parse(String(init.body)))
    return Response.json({ pushed: true })
  }) as typeof fetch
})

afterAll(() => {
  globalThis.fetch = realFetch
})

const settle = () => new Promise((r) => setTimeout(r, 50))
const ASK = [{ permission: "bash", pattern: "*", action: "ask" }]

/** A user turn (with `metadata` and `text`) and the assistant message answering it; returns the assistant id. */
async function turn(sessionID: string, input: { metadata?: Record<string, unknown>; text: string }) {
  const user = {
    id: Identifier.ascending("message"),
    sessionID,
    role: "user",
    time: { created: Date.now() },
    agent: "build",
    model: { providerID: "test", modelID: "test" },
    ...(input.metadata ? { metadata: input.metadata } : {}),
  }
  await Session.updateMessage(user)
  await Session.updatePart({ id: Identifier.ascending("part"), sessionID, messageID: user.id, type: "text", text: input.text })
  const assistant = {
    id: Identifier.ascending("message"),
    sessionID,
    role: "assistant",
    parentID: user.id,
    modelID: "test",
    providerID: "test",
    mode: "build",
    agent: "build",
    path: { cwd: "/", root: "/" },
    cost: 0,
    tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
    time: { created: Date.now() },
  }
  await Session.updateMessage(assistant)
  return assistant.id
}

function ask(sessionID: string, messageID: string, command: string) {
  return PermissionNext.ask({
    sessionID,
    permission: "bash",
    patterns: [command],
    metadata: {},
    always: [],
    tool: { messageID, callID: Identifier.ascending("part") },
    ruleset: ASK,
  })
}

test("a scheduled run's ask is refused at once, the owner is told once, and nothing waits", async () => {
  await using tmp = await tmpdir({ git: true })
  await Instance.provide({
    directory: tmp.path,
    fn: async () => {
      const session = await Session.create({})
      const assistantID = await turn(session.id, {
        metadata: { automation_run: { id: "job_1", runId: "run_1" } },
        text: "[routine: Nightly deploy] Deploy the site.",
      })

      const first = await ask(session.id, assistantID, "./deploy.sh").catch((e) => e)
      expect(first).toBeInstanceOf(PermissionNext.UnattendedError)
      // A RejectedError: the processor ends the turn instead of letting the model keep trying.
      expect(first).toBeInstanceOf(PermissionNext.RejectedError)
      expect(first.message).toBe(
        "This step needs the owner's approval. Unattended scheduled runs can't ask; the owner has been notified.",
      )
      expect(await PermissionNext.list()).toEqual([])

      // The same run asking again: refused again, no second push.
      await expect(ask(session.id, assistantID, "rm -rf dist")).rejects.toBeInstanceOf(PermissionNext.UnattendedError)
      await settle()
      expect(notifies).toHaveLength(1)
      expect(notifies[0]).toMatchObject({ sessionId: session.id, kind: "message", level: "needs_you" })
      expect(notifies[0].preview).toBe('Scheduled task "Nightly deploy" stopped: it needs your approval to use bash (./deploy.sh).')
      // The scheduler reads the first block back once.
      expect(PermissionNext.takeUnattendedBlock("run_1")).toBe("use bash (./deploy.sh)")
      expect(PermissionNext.takeUnattendedBlock("run_1")).toBeUndefined()
      await Session.remove(session.id)
    },
  })
})

test("a routine turn from allternit-api's scheduler (text marker only) is refused too", async () => {
  await using tmp = await tmpdir({ git: true })
  await Instance.provide({
    directory: tmp.path,
    fn: async () => {
      const session = await Session.create({})
      const assistantID = await turn(session.id, { text: "[routine: Morning inbox] Sort the inbox." })
      await expect(ask(session.id, assistantID, "gh pr merge 12")).rejects.toBeInstanceOf(PermissionNext.UnattendedError)
      await settle()
      expect(notifies).toHaveLength(1)
      await Session.remove(session.id)
    },
  })
})

test("a person's turn still asks and waits for the answer", async () => {
  await using tmp = await tmpdir({ git: true })
  await Instance.provide({
    directory: tmp.path,
    fn: async () => {
      const session = await Session.create({})
      // An earlier scheduled turn in the same thread doesn't make the person's turn unattended.
      await turn(session.id, { metadata: { automation_run: { id: "job_2" } }, text: "[routine: Ping] ping" })
      const assistantID = await turn(session.id, { text: "Deploy it please." })
      const pending = ask(session.id, assistantID, "./deploy.sh")
      const outcome = await Promise.race([pending.then(() => "resolved", () => "rejected"), settle().then(() => "waiting")])
      expect(outcome).toBe("waiting")
      const [open] = await PermissionNext.list()
      expect(open).toMatchObject({ sessionID: session.id, permission: "bash", patterns: ["./deploy.sh"] })
      await PermissionNext.reply({ requestID: open.id, reply: "once" })
      await expect(pending).resolves.toBeUndefined()
      expect(notifies).toEqual([])
      await Session.remove(session.id)
    },
  })
})

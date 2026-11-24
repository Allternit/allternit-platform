// @ts-nocheck
import { afterAll, afterEach, beforeEach, expect, test } from "bun:test"
import vectors from "../../../../contracts/action-classes/vectors.json"

/**
 * The action reviewer (Wave C decisions 4-5): the four action classes are
 * enforced in the permission path, a model review can only tighten, bypass
 * still asks about money, a retry of a denied outcome by another route is
 * flagged, and unattended runs refuse confirm_now. Real permission code and
 * the real session store; only network edges (the S1 runtime, allternit-api
 * notify) are faked.
 */
process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN = "internal-test"
const { PermissionNext } = await import("../../src/permission/next")
const { ActionClasses } = await import("../../src/runtime/tools/guard/permission/action-classes")
const { ActionReview } = await import("../../src/runtime/tools/guard/permission/action-review")
const { Instance } = await import("../../src/project/instance")
const { Session } = await import("../../src/runtime/session")
const { Identifier } = await import("../../src/shared/id/id")
const { tmpdir } = await import("../fixture/fixture")

const settle = () => new Promise((r) => setTimeout(r, 30))
const RANK = { allow: 0, ask: 1, deny: 2 }
const realFetch = globalThis.fetch
let notifies: any[] = []

beforeEach(() => {
  notifies = []
  globalThis.fetch = (async (input: string, init: RequestInit = {}) => {
    if (String(input).endsWith("/bot-routing/user/notify")) notifies.push(JSON.parse(String(init.body)))
    return Response.json({ pushed: true })
  }) as typeof fetch
})
afterEach(() => ActionReview._setClient(undefined))
afterAll(() => {
  globalThis.fetch = realFetch
})

async function inSession(fn: (sessionID: string) => Promise<void>) {
  await using tmp = await tmpdir({ git: true })
  await Instance.provide({
    directory: tmp.path,
    fn: async () => {
      const session = await Session.create({})
      try {
        await fn(session.id)
      } finally {
        for (const p of await PermissionNext.list()) await PermissionNext.withdraw(p.id).catch(() => {})
        await Session.remove(session.id)
      }
    },
  })
}

test("every shared vector gets its expected class", () => {
  const got = vectors.vectors.map((v) => ({ id: v.id, class: ActionClasses.classify(v.input).class }))
  const want = vectors.vectors.map((v) => ({ id: v.id, class: v.expected }))
  expect(got).toEqual(want)
})

/** A fake local S1 runtime answering P(the person asked for this) = p; null = runtime down. */
function runtime(p: number | null) {
  ActionReview._setClient({
    url: "http://127.0.0.1:7717",
    fetchImpl: async () => {
      if (p === null) throw new Error("connection refused")
      return Response.json({ answer: p >= 0.5, probabilities: { true: p, false: 1 - p }, confidence: p, threshold_action: "x" })
    },
  })
}

test("the reviewer never loosens the policy's answer, whatever the model says", async () => {
  await inSession(async (sessionID) => {
    const sendRule = { permission: "gmail_send_message", pattern: "*", action: "allow", source: "user" }
    const cases = [
      { permission: "gmail_send_message", pattern: "*", configured: [sendRule] }, // confirm_unless_named, named
      { permission: "gmail_send_message", pattern: "*", configured: [] }, // confirm_unless_named, not named
      { permission: "stripe_send_invoice", pattern: "*", configured: [] }, // confirm_now
      { permission: "stripe_list_recent_invoices", pattern: "*", configured: [] }, // free
      { permission: "bash", pattern: "rm -rf /", configured: [] }, // hand_off
    ]
    for (const p of [0.99, 0.5, 0.01, null]) {
      runtime(p)
      for (const c of cases) {
        for (const incumbent of ["allow", "ask", "deny"]) {
          for (const mode of ["default", "bypassPermissions", "yolo", "dontAsk"]) {
            const { action } = await ActionReview.decide({ sessionID, ...c, incumbent, mode, approvals: [], task: [] })
            if (RANK[action] < RANK[incumbent]) throw new Error(`loosened ${c.permission} ${incumbent}->${action} (p=${p}, ${mode})`)
          }
        }
      }
    }

    // A named send the policy allows: the model can add a question, never take one away.
    const named = { sessionID, permission: "gmail_send_message", pattern: "*", configured: [sendRule], approvals: [], task: [], mode: "default" }
    runtime(0.99)
    expect((await ActionReview.decide({ ...named, incumbent: "allow" })).action).toBe("allow")
    runtime(0.5)
    expect((await ActionReview.decide({ ...named, incumbent: "allow" })).action).toBe("ask")
    runtime(0.01)
    expect((await ActionReview.decide({ ...named, incumbent: "allow" })).action).toBe("ask") // capped: never refuses alone
    runtime(null)
    expect((await ActionReview.decide({ ...named, incumbent: "allow" })).action).toBe("allow") // no runtime: deterministic only
    runtime(0.99)
    expect((await ActionReview.decide({ ...named, incumbent: "ask" })).action).toBe("ask")
  })
})

const allow = (permission: string) => [{ permission, pattern: "*", action: "allow" }]

test("bypass mode still asks about money, and 'always' does not stick", async () => {
  await inSession(async (sessionID) => {
    const payout = {
      sessionID,
      permission: "bash",
      patterns: ["stripe payouts create --amount 5000 --currency usd"],
      metadata: {},
      always: ["stripe payouts *"],
      ruleset: allow("bash"),
      mode: "bypassPermissions",
    }
    // An ordinary command still runs without a card in bypass.
    expect(await PermissionNext.ask({ ...payout, patterns: ["ls -la"] })).toBeUndefined()

    const first = PermissionNext.ask(payout)
    await settle()
    const [card] = await PermissionNext.list()
    expect(card.metadata.review).toEqual({ class: "confirm_now", risk: "high", authorized: false, reason: expect.stringContaining("Money") })
    await PermissionNext.reply({ requestID: card.id, reply: "always" })
    await first

    // Asked again: the earlier "always" did not turn into an approval.
    const second = PermissionNext.ask(payout).catch((e) => e)
    await settle()
    const cards = await PermissionNext.list()
    expect(cards).toHaveLength(1)
    await PermissionNext.reply({ requestID: cards[0].id, reply: "reject" })
    expect(await second).toBeInstanceOf(PermissionNext.RejectedError)

    // An MCP tool that bills a customer asks in bypass too.
    const invoice = PermissionNext.ask({ ...payout, permission: "stripe_send_invoice", patterns: ["*"], ruleset: allow("*") }).catch((e) => e)
    await settle()
    const [invoiceCard] = await PermissionNext.list()
    expect(invoiceCard.metadata.review.class).toBe("confirm_now")
    await PermissionNext.reply({ requestID: invoiceCard.id, reply: "reject" })
    expect(await invoice).toBeInstanceOf(PermissionNext.RejectedError)
  })
})

test("a hand_off action never runs, even in bypass mode", async () => {
  await inSession(async (sessionID) => {
    const err = await PermissionNext.ask({
      sessionID,
      permission: "computer_toolset.computer",
      patterns: ["computer.left_click"],
      metadata: { toolset: "computer", member: "left_click", input: { label: "I'm not a robot", role: "checkbox" } },
      always: [],
      ruleset: allow("*"),
      mode: "bypassPermissions",
    }).catch((e) => e)
    expect(err).toBeInstanceOf(PermissionNext.HandOffError)
    expect(err.message).toContain("Hand this to the user")
    expect(await PermissionNext.list()).toEqual([])
  })
})

test("after the person denies a send, a send by another route is flagged and asks", async () => {
  const gmail = (sessionID: string) =>
    PermissionNext.ask({
      sessionID,
      permission: "gmail_send_message",
      patterns: ["*"],
      metadata: {},
      always: ["*"],
      // The person's own config rule names this tool: normally it runs without asking.
      ruleset: [{ permission: "gmail_send_message", pattern: "*", action: "allow", source: "user" }],
      mode: "default",
    })

  // Without a denial the named send runs.
  await inSession(async (sessionID) => {
    expect(await gmail(sessionID)).toBeUndefined()
  })

  await inSession(async (sessionID) => {
    const click = PermissionNext.ask({
      sessionID,
      permission: "computer_toolset.computer",
      patterns: ["computer.left_click"],
      metadata: { toolset: "computer", member: "left_click", input: { label: "Send", role: "button", app: "Mail" } },
      always: [],
      ruleset: [{ permission: "*", pattern: "*", action: "ask" }],
      mode: "default",
    }).catch((e) => e)
    await settle()
    const [card] = await PermissionNext.list()
    expect(card.metadata.review.class).toBe("confirm_unless_named")
    await PermissionNext.reply({ requestID: card.id, reply: "reject" })
    expect(await click).toBeInstanceOf(PermissionNext.RejectedError)

    const retry = gmail(sessionID).catch((e) => e)
    await settle()
    const [flagged] = await PermissionNext.list()
    expect(flagged.permission).toBe("gmail_send_message")
    expect(flagged.metadata.review.authorized).toBe(false)
    expect(flagged.metadata.review.reason).toContain("denied this before (via computer:left_click)")
    await PermissionNext.reply({ requestID: flagged.id, reply: "reject" })
    expect(await retry).toBeInstanceOf(PermissionNext.RejectedError)
  })
})

/** A scheduler-started user turn and the assistant message answering it; returns the assistant id. */
async function scheduledTurn(sessionID: string) {
  const user = {
    id: Identifier.ascending("message"),
    sessionID,
    role: "user",
    time: { created: Date.now() },
    agent: "build",
    model: { providerID: "test", modelID: "test" },
    metadata: { automation_run: { id: "job_pay", runId: "run_pay" } },
  }
  await Session.updateMessage(user)
  await Session.updatePart({ id: Identifier.ascending("part"), sessionID, messageID: user.id, type: "text", text: "Pay the contractors." })
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

test("an unattended run refuses confirm_now even in bypass mode, and other work still runs", async () => {
  await inSession(async (sessionID) => {
    const messageID = await scheduledTurn(sessionID)
    const run = (command: string) =>
      PermissionNext.ask({
        sessionID,
        permission: "bash",
        patterns: [command],
        metadata: {},
        always: [],
        tool: { messageID, callID: Identifier.ascending("part") },
        ruleset: allow("bash"),
        mode: "bypassPermissions",
      })
    expect(await run("ls -la")).toBeUndefined()
    await expect(run("stripe payouts create --amount 5000 --currency usd")).rejects.toBeInstanceOf(PermissionNext.UnattendedError)
    expect(await PermissionNext.list()).toEqual([])
  })
})

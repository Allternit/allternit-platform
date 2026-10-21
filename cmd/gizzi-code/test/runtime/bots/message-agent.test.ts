// @ts-nocheck
import { afterAll, beforeAll, describe, expect, test } from "bun:test"
import { existsSync, mkdirSync, rmSync, statSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { tmpdir } from "../../fixture/fixture"
import {
  botDir,
  botsDir,
  createBot,
  pinCanonicalSession,
} from "../../../src/runtime/bots/bot-store"
import {
  appendBotInbox,
  collectBotInboxMessages,
  drainBotInbox,
  formatInboxAttribution,
  inboxPath,
} from "../../../src/runtime/bots/bot-inbox"
import {
  MessageAgentTool,
  isMessageAgentSession,
  matchMessageTarget,
  resolveMessageTarget,
} from "../../../src/runtime/tools/builtins/message-agent"

/**
 * Phase B4 — message_agent + inbox (D5): roster validation (case-insensitive,
 * ambiguity lists exact handles), durable 0o600 inbox append, canonical-only
 * gating both ways, turn-start pickup with the platform's exact attribution
 * string, and the fire-and-forget acknowledgement. No live model.
 */

let tmp: { path: string }

beforeAll(async () => {
  tmp = await tmpdir()
  process.env.GIZZI_CONFIG_DIR = join(tmp.path, ".gizzi")
})

afterAll(() => {
  delete process.env.GIZZI_CONFIG_DIR
  rmSync(join(tmp.path, ".gizzi"), { recursive: true, force: true })
})

/** Seed alice + bob, pin alice's canonical session, return the session id. */
async function seedPair() {
  if (!existsSync(botDir("alice"))) {
    await createBot({ name: "alice", title: "Alice", description: "coordinator" })
    await createBot({ name: "bob", title: "Bob", description: "researcher" })
    await pinCanonicalSession("alice", { projectPath: "/proj", sessionId: "ses_alice" })
    await pinCanonicalSession("bob", { projectPath: "/proj", sessionId: "ses_bob" })
  }
  return "ses_alice"
}

async function executeMessageAgent(params: { target: string; message: string }, sessionID: string) {
  const tool = await MessageAgentTool.init()
  return tool.execute(params, { sessionID } as any)
}

describe("roster validation", () => {
  test("resolves targets case-insensitively; exact match wins", async () => {
    await seedPair()
    expect((await resolveMessageTarget("bob")).name).toBe("bob")
    expect((await resolveMessageTarget("BOB")).name).toBe("bob")
    expect((await resolveMessageTarget("alice")).name).toBe("alice")
  })

  test("unknown target errors with the known handles", async () => {
    await expect(resolveMessageTarget("ghost")).rejects.toThrow(/unknown teammate 'ghost'/)
    await expect(resolveMessageTarget("ghost")).rejects.toThrow(/alice, bob/)
  })

  test("ambiguous target lists the exact handles", async () => {
    // Synthetic roster — manufacturing a case-collision on disk aliases
    // directories on case-insensitive filesystems, so the pure matcher is
    // exercised directly.
    const bots = [
      { name: "alice" },
      { name: "ALICE" },
      { name: "bob" },
    ] as any[]
    // exact match still wins when the query IS one of the handles
    expect(matchMessageTarget("alice", bots).name).toBe("alice")
    expect(matchMessageTarget("ALICE", bots).name).toBe("ALICE")
    // a query that is neither exact handle is ambiguous and lists both
    const err = await Promise.resolve().then(
      () => matchMessageTarget("Alice", bots),
      (e) => e,
    ).then(
      () => null,
      (e) => e,
    )
    expect(err).not.toBeNull()
    expect(String(err.message)).toContain("alice")
    expect(String(err.message)).toContain("ALICE")
  })

  test("empty target and empty message are structured errors", async () => {
    await expect(resolveMessageTarget("   ")).rejects.toThrow(/target must be a bot name/)
    await seedPair()
    await expect(
      executeMessageAgent({ target: "bob", message: "   " }, "ses_alice"),
    ).rejects.toThrow(/message must not be empty/)
  })
})

describe("message_agent delivery (fire-and-forget)", () => {
  test("appends the durable envelope and acks without waiting for a reply", async () => {
    await seedPair()
    const result = await executeMessageAgent(
      { target: "BOB", message: "please review the draft" },
      "ses_alice",
    )

    expect(result.output).toBe(
      "delivered to bob's inbox; the reply arrives as a background completion",
    )
    expect(result.metadata).toMatchObject({ target: "bob", from: "alice", delivered: true })

    const { readFile } = await import("node:fs/promises")
    const raw = await readFile(inboxPath("bob"), "utf8")
    const envelope = JSON.parse(raw.trim().split("\n")[0])
    expect(envelope).toMatchObject({
      from: "alice",
      fromSessionId: "ses_alice",
      message: "please review the draft",
    })
    expect(typeof envelope.id).toBe("string")
    expect(typeof envelope.at).toBe("string")
    // 0o600 inside the 0o700 bot home
    expect(statSync(inboxPath("bob")).mode & 0o077).toBe(0)
  })

  test("messages with shell metacharacters are stored verbatim (never interpreted)", async () => {
    await seedPair()
    const payload = "$(rm -rf /) `whoami` && echo pwned; | cat <(x)"
    await executeMessageAgent({ target: "bob", message: payload }, "ses_alice")
    const { readFile } = await import("node:fs/promises")
    const lines = (await readFile(inboxPath("bob"), "utf8")).trim().split("\n")
    const envelope = JSON.parse(lines[lines.length - 1]!)
    expect(envelope.message).toBe(payload)
  })

  test("sender must be a bot's own chat — fail closed otherwise", async () => {
    await seedPair()
    // Not a gizzi canonical chat and not an app bot chat (the API isn't
    // reachable with credentials here): refused, nothing delivered.
    const realFetch = globalThis.fetch
    globalThis.fetch = (async () => new Response(JSON.stringify({ error: "nope" }), { status: 404 })) as typeof fetch
    try {
      await expect(
        executeMessageAgent({ target: "bob", message: "hi" }, "ses_not_a_bot"),
      ).rejects.toThrow(/isn't one of your bots' chats/)
    } finally {
      globalThis.fetch = realFetch
    }
  })

  test("target list returns the teammates instead of sending", async () => {
    await seedPair()
    const result = await executeMessageAgent({ target: "list" } as any, "ses_alice")
    expect(result.output).toContain("bob, Bob: researcher")
    expect(result.output).not.toContain("alice")
    expect(result.metadata).toMatchObject({ target: "list", delivered: false })
  })
})

describe("gating predicate (D5)", () => {
  test("canonical bot chat sessions pass; everything else fails", async () => {
    await seedPair()
    expect(await isMessageAgentSession("ses_alice")).toBe(true)
    expect(await isMessageAgentSession("ses_bob")).toBe(true)
    const realFetch = globalThis.fetch
    globalThis.fetch = (async () => new Response("{}", { status: 404 })) as typeof fetch
    try {
      expect(await isMessageAgentSession("ses_regular_chat")).toBe(false)
    } finally {
      globalThis.fetch = realFetch
    }
    expect(await isMessageAgentSession("")).toBe(false)
  })
})

describe("inbox pickup (turn-start injection)", () => {
  test("attribution string matches the platform format exactly", () => {
    expect(
      formatInboxAttribution({
        id: "x",
        from: "alice",
        fromSessionId: "ses_alice",
        message: "hello bob",
        at: new Date().toISOString(),
      }),
    ).toBe("Message from 🤖 alice (@alice): hello bob")
  })

  test("drain reads envelopes in order and removes the inbox file", async () => {
    await seedPair()
    await drainBotInbox("bob") // clear mail delivered by earlier tests
    await appendBotInbox("bob", {
      from: "alice",
      fromSessionId: "ses_alice",
      message: "first",
      at: new Date().toISOString(),
    })
    await appendBotInbox("bob", {
      from: "carol",
      fromSessionId: "ses_carol",
      message: "second",
      at: new Date().toISOString(),
    })

    const envelopes = await drainBotInbox("bob")
    expect(envelopes.map((e) => e.message)).toEqual(["first", "second"])
    expect(existsSync(inboxPath("bob"))).toBe(false)
    // second drain is a no-op
    expect(await drainBotInbox("bob")).toEqual([])
  })

  test("envelopes appended mid-drain survive for the next turn", async () => {
    await seedPair()
    await drainBotInbox("bob") // clear mail delivered by earlier tests
    await appendBotInbox("bob", {
      from: "alice",
      fromSessionId: "ses_alice",
      message: "before",
      at: new Date().toISOString(),
    })
    // Interleave: read (rename happens inside drain), so append after a manual
    // rename emulates a concurrent sender landing in the fresh inbox.jsonl.
    const { rename } = await import("node:fs/promises")
    const staging = inboxPath("bob") + ".manual"
    await rename(inboxPath("bob"), staging)
    await appendBotInbox("bob", {
      from: "alice",
      fromSessionId: "ses_alice",
      message: "during",
      at: new Date().toISOString(),
    })
    const drained = await drainBotInbox("bob") // fresh inbox only
    expect(drained.map((e) => e.message)).toEqual(["during"])
    // the staged file still parses via a second drain once restored semantics
    await rename(staging, inboxPath("bob"))
    expect((await drainBotInbox("bob")).map((e) => e.message)).toEqual(["before"])
  })

  test("collectBotInboxMessages injects attributed mail for canonical sessions only", async () => {
    await seedPair()
    await appendBotInbox("bob", {
      from: "alice",
      fromSessionId: "ses_alice",
      message: "ping",
      at: new Date().toISOString(),
    })

    const messages = await collectBotInboxMessages("ses_bob")
    expect(messages).toEqual(["Message from 🤖 alice (@alice): ping"])
    expect(existsSync(inboxPath("bob"))).toBe(false)

    // non-canonical sessions never see (or drain) inboxes
    expect(await collectBotInboxMessages("ses_regular")).toEqual([])
    expect(existsSync(inboxPath("bob"))).toBe(false)
  })
})

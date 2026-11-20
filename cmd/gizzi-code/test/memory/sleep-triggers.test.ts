import { afterAll, afterEach, beforeEach, describe, expect, test } from "bun:test"
import { mkdtemp, rm } from "fs/promises"
import os from "os"
import path from "path"
import { createSleepTriggers, type SleepClock } from "../../src/runtime/memory/sleep/triggers"
import type { SleepSignal } from "../../src/runtime/memory/sleep/client"
import { resetSleepClientForTests, sleepEnabled } from "../../src/runtime/memory/sleep/client"
import { resetDriveAccountCache, sleepActiveSync } from "../../src/runtime/memory/drive/paths"

/** A clock the test advances by hand (the time edge). */
function fakeClock() {
  let now = 0
  let seq = 0
  const timers = new Map<number, { at: number; fn: () => void }>()
  const clock: SleepClock = {
    setTimeout(fn, ms) {
      const id = ++seq
      timers.set(id, { at: now + ms, fn })
      return id
    },
    clearTimeout(handle) {
      timers.delete(handle as number)
    },
  }
  return {
    clock,
    advance(ms: number) {
      now += ms
      for (const [id, t] of [...timers].sort((a, b) => a[1].at - b[1].at)) {
        if (t.at > now) continue
        timers.delete(id)
        t.fn()
      }
    },
  }
}

/** The network edge: records what Gizzi would POST to /memory/sleep/signal. */
function fakeTransport(behaviour: "ok" | "reject" | "hang" = "ok") {
  const sent: SleepSignal[] = []
  const transport = async (signal: SleepSignal) => {
    sent.push(signal)
    if (behaviour === "reject") throw new Error("connect ECONNREFUSED")
    if (behaviour === "hang") await new Promise(() => {})
  }
  return { sent, transport }
}

const flush = () => new Promise((r) => setTimeout(r, 0))

describe("Sleep triggers: idle timer", () => {
  test("one idle signal after the cache TTL, with the TTL in the body", async () => {
    const t = fakeClock()
    const net = fakeTransport()
    const triggers = createSleepTriggers({ clock: t.clock, transport: net.transport, enabled: async () => true })
    triggers.promptArrived("ses_1", 300)
    triggers.turnEnded("ses_1", 300)
    t.advance(299_000)
    await flush()
    expect(net.sent).toEqual([])
    t.advance(1_000)
    await flush()
    expect(net.sent).toEqual([{ session_id: "ses_1", event: "idle", cache_ttl_s: 300 }])
    // The session going idle again without a prompt does not send a second idle.
    triggers.sessionIdle("ses_1")
    triggers.turnEnded("ses_1", 300)
    t.advance(3_600_000)
    await flush()
    expect(net.sent.filter((s) => s.event === "idle")).toHaveLength(1)
  })

  test("one-hour caching waits an hour", async () => {
    const t = fakeClock()
    const net = fakeTransport()
    const triggers = createSleepTriggers({ clock: t.clock, transport: net.transport, enabled: async () => true })
    triggers.turnEnded("ses_h", 3600)
    t.advance(300_000)
    await flush()
    expect(net.sent).toEqual([])
    t.advance(3_300_000)
    await flush()
    expect(net.sent).toEqual([{ session_id: "ses_h", event: "idle", cache_ttl_s: 3600 }])
  })

  test("no idle when a prompt arrives before the TTL", async () => {
    const t = fakeClock()
    const net = fakeTransport()
    const triggers = createSleepTriggers({ clock: t.clock, transport: net.transport, enabled: async () => true })
    triggers.turnEnded("ses_2", 300)
    t.advance(200_000)
    triggers.promptArrived("ses_2", 300)
    t.advance(600_000)
    await flush()
    expect(net.sent).toEqual([])
    expect(triggers.pending("ses_2")).toBe(false)
  })

  test("a prompt after idle sends resume, and the next quiet stretch can idle again", async () => {
    const t = fakeClock()
    const net = fakeTransport()
    const triggers = createSleepTriggers({ clock: t.clock, transport: net.transport, enabled: async () => true })
    triggers.turnEnded("ses_3", 300)
    t.advance(300_000)
    await flush()
    triggers.promptArrived("ses_3", 300)
    await flush()
    expect(net.sent.map((s) => s.event)).toEqual(["idle", "resume"])
    triggers.turnEnded("ses_3", 300)
    t.advance(300_000)
    await flush()
    expect(net.sent.map((s) => s.event)).toEqual(["idle", "resume", "idle"])
  })

  test("session idle only arms sessions that had a top-level prompt here", async () => {
    const t = fakeClock()
    const net = fakeTransport()
    const triggers = createSleepTriggers({ clock: t.clock, transport: net.transport, enabled: async () => true })
    triggers.sessionIdle("ses_child")
    triggers.preCompact("ses_child")
    t.advance(3_600_000)
    await flush()
    expect(net.sent).toEqual([])
  })
})

describe("Sleep triggers: compaction and exit never wait on the API", () => {
  for (const behaviour of ["reject", "hang"] as const) {
    test(`compaction proceeds when the API ${behaviour === "reject" ? "is down" : "hangs"}`, async () => {
      const net = fakeTransport(behaviour)
      const triggers = createSleepTriggers({ transport: net.transport, enabled: async () => true })
      triggers.promptArrived("ses_c")
      // Stand-in for the compaction path: the call returns synchronously, so the
      // steps after it run without awaiting the network.
      const compaction = async () => {
        const returned = triggers.preCompact("ses_c")
        return { returned, summarized: true }
      }
      const result = await Promise.race([
        compaction(),
        new Promise((_, reject) => setTimeout(() => reject(new Error("compaction blocked")), 200)),
      ])
      expect(result).toEqual({ returned: undefined, summarized: true })
      await flush()
      expect(net.sent.map((s) => s.event)).toEqual(["pre_compact"])
    })
  }

  test("session_end resolves within its cap when the API hangs", async () => {
    const net = fakeTransport("hang")
    const triggers = createSleepTriggers({ transport: net.transport, enabled: async () => true })
    triggers.promptArrived("ses_e")
    const started = Date.now()
    await triggers.endAll(50)
    expect(Date.now() - started).toBeLessThan(500)
    expect(net.sent.map((s) => s.event)).toEqual(["session_end"])
    // Nothing known any more: a second exit resolves at once and sends nothing.
    await triggers.endAll(50)
    expect(net.sent).toHaveLength(1)
  })
})

// Sleep on/off through the real status client; only fetch (the network edge) is faked.
const roots: string[] = []
const realFetch = globalThis.fetch
afterAll(async () => {
  for (const r of roots) await rm(r, { recursive: true, force: true })
})

function serveStatus(reply: { status: number; body: unknown }) {
  globalThis.fetch = (async (input: string | URL) => {
    const url = new URL(String(input))
    if (url.pathname.endsWith("/memory/sleep/status")) {
      return new Response(JSON.stringify(reply.body), { status: reply.status, headers: { "Content-Type": "application/json" } })
    }
    return new Response(JSON.stringify({ error: "not found" }), { status: 404 })
  }) as typeof fetch
}

describe("Sleep on/off", () => {
  beforeEach(async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), "gizzi-sleep-"))
    roots.push(root)
    process.env.GIZZI_MEMORY_DRIVE_ROOT = path.join(root, "drives")
    process.env.ALLTERNIT_API_TOKEN = `token-${roots.length}`
    resetDriveAccountCache()
    resetSleepClientForTests()
  })
  afterEach(() => {
    globalThis.fetch = realFetch
    delete process.env.ALLTERNIT_API_TOKEN
    delete process.env.GIZZI_MEMORY_DRIVE_ROOT
    resetDriveAccountCache()
    resetSleepClientForTests()
  })

  const offCases = [
    { name: "an older runtime (404)", reply: { status: 404, body: { error: "not found" } } },
    { name: "Sleep switched off", reply: { status: 200, body: { enabled: false } } },
  ]
  for (const c of offCases) {
    test(`${c.name}: no signals`, async () => {
      serveStatus(c.reply)
      const t = fakeClock()
      const net = fakeTransport()
      const triggers = createSleepTriggers({ clock: t.clock, transport: net.transport, enabled: sleepEnabled })
      triggers.turnEnded("ses_off", 300)
      t.advance(300_000)
      triggers.promptArrived("ses_off", 300)
      triggers.preCompact("ses_off")
      await triggers.endAll(1_000)
      await flush()
      expect(net.sent).toEqual([])
      expect(sleepActiveSync()).toBe(false)
    })
  }

  test("Sleep on: signals flow and the local autoDream gate closes", async () => {
    serveStatus({ status: 200, body: { enabled: true } })
    const t = fakeClock()
    const net = fakeTransport()
    const triggers = createSleepTriggers({ clock: t.clock, transport: net.transport, enabled: sleepEnabled })
    triggers.turnEnded("ses_on", 300)
    t.advance(300_000)
    await new Promise((r) => setTimeout(r, 50))
    expect(net.sent).toEqual([{ session_id: "ses_on", event: "idle", cache_ttl_s: 300 }])
    // The status answer is cached for the sync gate, so only one consolidator runs.
    expect(sleepActiveSync()).toBe(true)
    const { isAutoDreamEnabled } = await import("../../src/cli/ui/ink-app/services/autoDream/config")
    expect(isAutoDreamEnabled()).toBe(false)
  })
})

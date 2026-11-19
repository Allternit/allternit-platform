import { describe, expect, test } from "bun:test"
import { followTask, formatStep, resolveHold, describeOutcome, choosePhone, formatPhones, formatMaps, formatMapScreens, mapErrorMessage, OLD_RUNTIME, confirmForget, outcomeStats, PhoneCommand, type HoldIO, type Hold, type RunRow } from "@/cli/commands/phone"

const hold: Hold = {
  approvalId: "a1",
  inboxId: "approval:a1",
  state: "requested",
  effect: "pay",
  phone: { app: "com.shop", phoneName: "Pixel", element: { label: "Pay now" } },
}

function io(script: Array<"approve" | "deny" | "open" | undefined>, decide: (d: string) => string = () => "ok") {
  const said: string[] = []
  const calls: string[] = []
  const q = [...script]
  const h: HoldIO = {
    say: (l) => said.push(l),
    showCrop: async () => (calls.push("crop"), true),
    decide: async (_id, d) => (calls.push(d), decide(d)),
    ask: async () => q.shift(),
  }
  return { h, said, calls }
}

describe("resolveHold", () => {
  test("approve is refused until the screen image was shown", async () => {
    const { h, calls } = io(["approve", "approve"])
    expect(await resolveHold(hold, h)).toBe("approved")
    expect(calls).toEqual(["crop", "approve"])
  })
  test("409 review_required opens the image, then approve goes through", async () => {
    let n = 0
    const { h, calls } = io(["open", "approve", "approve"], () => (n++ === 0 ? "review_required" : "ok"))
    expect(await resolveHold(hold, h)).toBe("approved")
    expect(calls).toEqual(["crop", "approve", "crop", "approve"])
  })
  test("deny works without looking", async () => {
    const { h, calls } = io(["deny"])
    expect(await resolveHold(hold, h)).toBe("denied")
    expect(calls).toEqual(["deny"])
  })
  test("no terminal leaves the hold alone", async () => {
    const { h, calls } = io([undefined])
    expect(await resolveHold(hold, h)).toBe("left")
    expect(calls).toEqual([])
  })
})

describe("followTask", () => {
  const row = (o: Partial<RunRow>): RunRow => ({ run_id: "r", serial: "s", started_at: "", ended_at: "", steps: 0, failed: 0, ...o })
  test("waiting → approved → done: one hold, progress printed once per step", async () => {
    let release!: (v: any) => void
    const task = new Promise<any>((r) => (release = r))
    const said: string[] = []
    let tick = 0
    const holds: string[] = []
    const report = await followTask({
      task,
      pollMs: 5,
      say: (l) => said.push(l),
      runs: async () => {
        tick++
        if (tick === 1) return [row({ steps: 1 })]
        if (tick < 6) return [row({ steps: 2, status: "waiting_approval", approvalId: "a1", effect: "pay" })]
        return [row({ steps: 3 })]
      },
      hold: async (id) => {
        holds.push(id)
        setTimeout(() => release({ ok: true, status: "done", result: "Paid" }), 20)
        return "approved"
      },
    })
    expect(holds).toEqual(["a1"])
    expect(said).toEqual(["  step 1", "  step 2"])
    expect(describeOutcome(report)).toBe("Done. Paid")
  })
  test("prints the step label, the waiting label once, and falls back to step N", async () => {
    let release!: (v: any) => void
    const task = new Promise<any>((r) => (release = r))
    const said: string[] = []
    let tick = 0
    await followTask({
      task,
      pollMs: 5,
      say: (l) => said.push(l),
      runs: async () => {
        tick++
        if (tick <= 2) return [row({ steps: 1, step: 1, step_label: "Opening Settings" })]
        if (tick === 3) return [row({ steps: 2, step: 2 })]
        return [row({ steps: 3, step: 3, step_label: "Waiting for your OK to pay Pay now", status: "waiting_approval", approvalId: "a9", effect: "pay" })]
      },
      hold: async () => {
        setTimeout(() => release({ ok: true, status: "done" }), 20)
        return "approved"
      },
    })
    expect(said).toEqual(["  step 1 · Opening Settings", "  step 2", "  step 3 · Waiting for your OK to pay Pay now"])
    expect(formatStep(row({ steps: 4, step: 4, step_label: "Tap Save", failed: 1 }))).toBe("  step 4 · Tap Save (1 failed)")
  })
  test("denied and timed-out runs read as stopped", () => {
    expect(describeOutcome({ status: "denied" })).toBe("Stopped: you denied the approval.")
    expect(describeOutcome({ status: "approval_timeout" })).toBe("Stopped: nobody answered the approval in time.")
  })
})

describe("choosePhone", () => {
  const p = (name: string, online = true) => ({ serial: "link:" + name, name, online })
  test("one plain line for none and for offline", () => {
    expect(() => choosePhone([])).toThrow("No phone is linked")
    expect(() => choosePhone([p("Pixel", false)])).toThrow("Pixel is offline")
  })
  test("by name, and the only online phone by default", () => {
    expect(choosePhone([p("Pixel"), p("iPhone")], "iph").name).toBe("iPhone")
    expect(choosePhone([p("Pixel", false), p("iPhone")]).name).toBe("iPhone")
  })
})

describe("formatPhones", () => {
  test("shows Android app phones with screen state and taps", () => {
    const out = formatPhones([
      { serial: "app:a1", name: "Pixel", online: false, platform: "android", route: "android-app", canTap: true },
      { serial: "app:a2", name: "Galaxy", online: true, platform: "android", route: "android-app", canTap: false },
      { serial: "app:i1", name: "Eoj iPhone", online: true, platform: "ios" },
    ])
    const lines = out.split("\n")
    expect(lines[0]).toContain("Android")
    expect(lines[0]).toContain("waiting for Start")
    expect(lines[0]).toContain("taps on")
    expect(lines[1]).toContain("screen live")
    expect(lines[1]).toContain("taps off")
    expect(lines[2]).toContain("iPhone")
    expect(lines[2]).not.toContain("taps")
  })
})

describe("outcome line with app maps", () => {
  test("old runtime fields absent: prints exactly as before", () => {
    expect(describeOutcome({ status: "done", steps: [1, 2], result: "ok" })).toBe("Done. ok")
    expect(outcomeStats({ status: "done", steps: [1] })).toBe("")
  })
  test("adds steps, map steps and tokens", () => {
    const r = { status: "done", steps: [1, 2, 3, 4, 5], map_steps: 3, tokens_in: 40000, tokens_out: 1200 }
    expect(describeOutcome(r)).toBe("Done in 5 steps · 3 from the map · 41k tokens.")
  })
  test("zero map steps and tokens are left out; non-done keeps its head", () => {
    expect(describeOutcome({ status: "done", steps: [1], map_steps: 0, tokens_in: 0 })).toBe("Done in 1 step.")
    expect(describeOutcome({ status: "failed", steps: [1, 2], map_steps: 1 })).toBe("Failed (2 steps · 1 from the map).")
  })
})

describe("maps output", () => {
  test("lists maps with stale and partial", () => {
    const out = formatMaps([{ app: "com.android.settings", name: "Settings", screens: 12, routable_edges: 15, stale_edges: 2, last_seen: "2026-11-18T10:05:00Z", truncated: { reason: "time_budget" } }])
    expect(out).toContain("Settings (com.android.settings)")
    expect(out).toContain("12 screens")
    expect(out).toContain("2 stale")
    expect(out).toContain("partial (time_budget)")
    expect(formatMaps([])).toContain("gizzi phone explore")
  })
  test("one app lists screens with hops from the start", () => {
    const out = formatMapScreens({
      app: "a",
      screens: { s0: { id: "s0", name: "Home" }, s1: { id: "s1", name: "Wi-Fi" }, s2: { id: "s2", name: "Deep" }, s3: { id: "s3", name: "Island" } },
      edges: [
        { from: "s0", to: "s1" },
        { from: "s1", to: "s2" },
      ],
    })
    expect(out).toContain("Home")
    expect(out).toMatch(/Wi-Fi\s+1 hop\n/)
    expect(out).toMatch(/Deep\s+2 hops/)
    expect(out).toMatch(/Island\s+no route from start/)
  })
})

describe("explore errors and forget-map", () => {
  test("error codes map to plain words", () => {
    expect(mapErrorMessage({ status: 403, body: { error: "money_app" } })).toBe("Explore won't touch money apps.")
    expect(mapErrorMessage({ status: 409, body: { error: "phone_busy" } })).toContain("busy")
    expect(mapErrorMessage({ status: 409, body: { error: "explore_running" } })).toContain("already running")
    expect(mapErrorMessage({ status: 400, body: { error: "explore_android_only" } })).toContain("Android")
    expect(mapErrorMessage({ status: 500, body: { message: "boom" } })).toBe("boom")
    expect(mapErrorMessage({ status: 500 })).toBe("HTTP 500")
  })
  test("an older runtime (404/405, no code) gets the update message; coded 404 stays", () => {
    expect(mapErrorMessage({ status: 404 })).toBe(OLD_RUNTIME)
    expect(mapErrorMessage({ status: 405, body: {} })).toBe(OLD_RUNTIME)
    expect(OLD_RUNTIME).toBe("This Allternit runtime doesn't have app maps yet. Update Allternit, then try again.")
    expect(mapErrorMessage({ status: 404, body: { error: "map_not_found" } })).toBe("No map for that app yet.")
  })
  test("forget-map asks unless --yes", async () => {
    let asked = 0
    expect(await confirmForget("a", true, async () => (asked++, "n"))).toBe(true)
    expect(asked).toBe(0)
    expect(await confirmForget("a", false, async () => (asked++, "n"))).toBe(false)
    expect(await confirmForget("a", false, async () => (asked++, "Y"))).toBe(true)
    expect(asked).toBe(2)
  })
})

describe("arg parsing", () => {
  const sub = () => ((PhoneCommand as any).builder({ command: (c: any) => (list.push(c), chain), demandCommand: () => chain }), list)
  const list: any[] = []
  const chain: any = { command: (c: any) => (list.push(c), chain), demandCommand: () => chain }
  test("registers the new commands with their positionals", () => {
    list.length = 0
    sub()
    const names = list.map((c) => c.command)
    expect(names).toEqual(expect.arrayContaining(["maps [app]", "explore <app>", "stop <run_id>", "forget-map <app>", "task <goal..>"]))
  })
  test("explore and task builders declare their options", () => {
    list.length = 0
    sub()
    const opts = (name: string) => {
      const seen: string[] = []
      const y: any = { positional: () => y, option: (o: string) => (seen.push(o), y) }
      list.find((c) => c.command === name).builder(y)
      return seen
    }
    expect(opts("explore <app>")).toEqual(expect.arrayContaining(["serial", "minutes", "max-screens", "stop"]))
    expect(opts("forget-map <app>")).toContain("yes")
    expect(opts("maps [app]")).toContain("json")
    expect(opts("task <goal..>")).toContain("vision")
  })
})

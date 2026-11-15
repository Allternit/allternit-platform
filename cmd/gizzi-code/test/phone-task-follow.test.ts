import { describe, expect, test } from "bun:test"
import { followTask, formatStep, resolveHold, describeOutcome, choosePhone, type HoldIO, type Hold, type RunRow } from "@/cli/commands/phone"

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

import { describe, expect, test } from "bun:test"

/**
 * browser_tasks (wave C decision 8) through the real primitive: the real
 * gate (action classes), the real ref tracking, budgets and scheduling. Only
 * the edges are fakes: the executor (allternit-api over HTTP) and the
 * sub-session's model loop (the driver).
 */
import { BrowserTasks } from "../../src/runtime/primitives/browser-tasks"
import type { ExecutorClient } from "../../src/runtime/tools/computer-toolset/executor-client"

const LOCAL = "http://127.0.0.1:8013"

/** An executor whose browser answers every member; `pages` maps a member to its reply text. */
function fakeExecutor(pages: Record<string, string> = {}, opts: { schema?: any } = {}) {
  const ran: Array<{ session: string; member: string; input: any }> = []
  const executor: ExecutorClient = {
    async run(_computer, req) {
      ran.push({ session: req.browser_session_id!, member: req.member, input: req.input })
      const url = req.member === "navigate" ? String((req.input as any).url) : "https://shop.example/cart"
      return {
        status: 200,
        body: {
          is_error: false,
          content: [{ type: "text", text: pages[req.member] ?? "OK" }],
          browser_state: { tabs: [{ tab_id: "t1", title: "Shop", url, active: true }] },
          screen: { width: 1280, height: 800, scale: 1 },
        } as any,
      }
    },
    async schema() {
      return "schema" in opts ? opts.schema : { available: true, target: "browser_gateway", members: [{ name: "navigate", enabled: true }] }
    },
    async approve() {
      return false
    },
  }
  return { executor, ran }
}

const deps = (executor: ExecutorClient, driver: BrowserTasks.Driver, extra: Partial<BrowserTasks.RunDeps> = {}): BrowserTasks.RunDeps => ({
  executor,
  computerId: "this-device",
  driver,
  runId: "ses_test",
  signal: new AbortController().signal,
  ...extra,
})

describe("browser_tasks gate", () => {
  test("a gate-class step stops only that task with needs_user, and the step never runs", async () => {
    const { executor, ran } = fakeExecutor({ find: '- button "Pay now" [ref_3] at (600, 400)\n- link "Shipping info" [ref_4] at (100, 700)' })
    const driver: BrowserTasks.Driver = async ({ task, browser }) => {
      await browser.call({ member: "find", input: { query: "pay" } })
      if (task.id === "buy") await browser.call({ member: "left_click", input: { target: { type: "ref", ref: "ref_3" } }, step: "Clicking the button" })
      else await browser.call({ member: "left_click", input: { target: { type: "ref", ref: "ref_4" } }, step: "Opening shipping info" })
      return { status: "done", result: "Shipping is free over $50." }
    }
    const out = await BrowserTasks.run(
      [
        { id: "buy", start_url: "https://shop.example/item", goal: "check out" },
        { id: "info", start_url: "https://shop.example/item", goal: "read shipping" },
      ],
      deps(executor, driver),
    )

    expect(out.map((o) => [o.id, o.status])).toEqual([
      ["buy", "needs_user"],
      ["info", "done"],
    ])
    expect(out[0]!.result).toContain("Money")
    expect(out[0]!.final_url).toBe("https://shop.example/cart")
    const buyClicks = ran.filter((r) => r.member === "left_click" && r.input.target.ref === "ref_3")
    expect(buyClicks).toEqual([])
    // Each task had its own browser session.
    expect(new Set(ran.filter((r) => r.member === "navigate").map((r) => r.session)).size).toBe(2)
  })

  test("typing into a password field stops with needs_login", async () => {
    const { executor, ran } = fakeExecutor({ read_page: '- textbox "Email" [ref_1]\n- textbox "Password" [ref_2]\n- button "Sign in" [ref_3]' })
    const driver: BrowserTasks.Driver = async ({ browser }) => {
      await browser.call({ member: "read_page", input: { filter: "interactive" } })
      await browser.call({ member: "left_click", input: { target: { type: "ref", ref: "ref_2" } }, step: "Focusing the field" })
      await browser.call({ member: "type", input: { text: "hunter2" }, step: "Typing" })
      return { status: "done", result: "signed in" }
    }
    const [out] = await BrowserTasks.run([{ id: "a", start_url: "https://mail.example/", goal: "read inbox" }], deps(executor, driver))
    expect(out!.status).toBe("needs_login")
    expect(ran.some((r) => r.member === "type")).toBe(false)
  })

  test("the executor's own review holding a step stops the task with needs_user", async () => {
    const { executor } = fakeExecutor({ find: '- button "Continue" [ref_9]' })
    const run = executor.run
    executor.run = async (c, req, s) =>
      req.member === "left_click"
        ? { status: 409, body: { is_error: true, error: "approval_required", approval_id: "ap_1", content: [{ type: "text", text: "Needs approval." }], screen: { width: 0, height: 0, scale: 1 } } as any }
        : run(c, req, s)
    const driver: BrowserTasks.Driver = async ({ browser }) => {
      await browser.call({ member: "find", input: { query: "continue" } })
      await browser.call({ member: "left_click", input: { target: { type: "ref", ref: "ref_9" } } })
      return { status: "done", result: "x" }
    }
    const [out] = await BrowserTasks.run([{ id: "a", start_url: "https://x.example/", goal: "go on" }], deps(executor, driver))
    expect(out!.status).toBe("needs_user")
  })
})

describe("browser_tasks availability", () => {
  test("no executor answering on this machine: unavailable", async () => {
    const { executor } = fakeExecutor({}, { schema: undefined })
    const got = await BrowserTasks.availability({ executor, computerId: "this-device", base: LOCAL })
    expect(got.ok).toBe(false)
  })

  test("browser driver down: unavailable, with the executor's reason", async () => {
    const message = "The browser driver (ACU gateway) isn't running for this computer."
    const { executor } = fakeExecutor({}, { schema: { available: false, target: "browser_gateway", members: [{ name: "navigate", enabled: false, reason: message }] } })
    const got = await BrowserTasks.availability({ executor, computerId: "this-device", base: LOCAL })
    expect(got).toEqual({ ok: false, reason: message })
  })

  test("a remote executor (a company API) is never used, even when its browser is up", async () => {
    const { executor } = fakeExecutor()
    const got = await BrowserTasks.availability({ executor, computerId: "this-device", base: "https://api.allternit.com" })
    expect(got).toEqual({ ok: false, reason: BrowserTasks.UNAVAILABLE_REMOTE })
    expect(await BrowserTasks.availability({ executor, computerId: "this-device", base: LOCAL })).toEqual({ ok: true })
  })
})

describe("browser_tasks scheduling", () => {
  test("six tasks run at most four at once and all finish", async () => {
    const { executor } = fakeExecutor()
    let active = 0
    let peak = 0
    const driver: BrowserTasks.Driver = async ({ task }) => {
      active++
      peak = Math.max(peak, active)
      await new Promise((r) => setTimeout(r, 20))
      active--
      return { status: "done", result: `result ${task.id}` }
    }
    const tasks = Array.from({ length: 6 }, (_, i) => ({ id: `t${i}`, start_url: `https://site${i}.example/`, goal: "look" }))
    const out = await BrowserTasks.run(tasks, deps(executor, driver))
    expect(peak).toBe(BrowserTasks.MAX_PARALLEL)
    expect(out.map((o) => o.status)).toEqual(Array(6).fill("done"))
    expect(out.map((o) => o.id)).toEqual(tasks.map((t) => t.id))
  })

  test("attached_ids wait for the earlier task and pass its result as context", async () => {
    const { executor } = fakeExecutor()
    const seen: Record<string, string> = {}
    const driver: BrowserTasks.Driver = async ({ task, context }) => {
      seen[task.id] = context
      return { status: "done", result: task.id === "a" ? "The cheapest flight is $212 on May 3." : "Hotel near the airport: $90." }
    }
    await BrowserTasks.run(
      [
        { id: "a", start_url: "https://flights.example/", goal: "cheapest flight" },
        { id: "b", start_url: "https://hotels.example/", goal: "hotel for that date", attached_ids: ["a"] },
      ],
      deps(executor, driver),
    )
    expect(seen.a).toBe("")
    expect(seen.b).toContain("$212 on May 3")
  })

  test("a task over its time budget fails without holding up the others", async () => {
    const { executor } = fakeExecutor()
    const driver: BrowserTasks.Driver = async ({ task, signal }) => {
      if (task.id === "slow") await new Promise((_, reject) => signal.addEventListener("abort", () => reject(new Error("aborted"))))
      return { status: "done", result: "ok" }
    }
    const out = await BrowserTasks.run(
      [
        { id: "slow", start_url: "https://a.example/", goal: "x" },
        { id: "fast", start_url: "https://b.example/", goal: "y" },
      ],
      deps(executor, driver, { timeoutMs: 30 }),
    )
    expect(out[0]!.status).toBe("failed")
    expect(out[0]!.result).toContain("Ran out of time")
    expect(out[1]!.status).toBe("done")
  })

  test("validate refuses forward attachments and non-http start pages", () => {
    expect(
      BrowserTasks.validate({
        action: "run",
        tasks: [
          { id: "a", start_url: "https://a.example/", goal: "x", attached_ids: ["b"] },
          { id: "b", start_url: "https://b.example/", goal: "y" },
        ],
      }),
    ).toContain("isn't an earlier task")
    expect(BrowserTasks.validate({ action: "run", tasks: [{ id: "a", start_url: "file:///etc/passwd", goal: "x" }] })).toContain("http")
  })
})

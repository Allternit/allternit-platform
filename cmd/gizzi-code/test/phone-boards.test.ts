import { afterAll, beforeAll, beforeEach, describe, expect, test } from "bun:test"

// A fake runtime that serves exactly the contract shape of
// hardware/link/README.md section 6. The gateway base is fixed at import time
// (and other test files may have imported it first), so requests to it are
// pointed at the fake server by wrapping fetch.
let server: ReturnType<typeof Bun.serve>
let realFetch: typeof fetch
let BoardsCommand: any
let StopCommand: any

const board = (o: Record<string, unknown>) => ({
  boardId: "usba-0123456789ab",
  serial: "usb:usba-0123456789ab",
  name: "Desk board",
  online: true,
  armed: true,
  status: "ready",
  link: "lan",
  usb: "mounted",
  hid: "usb",
  dock: true,
  face: "working",
  prog: 3,
  eyes: "app",
  phone: null,
  screen: true,
  calibrated: true,
  firmware: "2.0.0",
  protocol: 2,
  firmwareUpdate: false,
  relay: "none",
  ...o,
})

let boardsReply: () => Response
let stopReply: (id: string) => Response
const stops: string[] = []

beforeAll(async () => {
  server = Bun.serve({
    port: 0,
    fetch(req) {
      const u = new URL(req.url)
      if (req.method === "GET" && u.pathname === "/api/v1/phones/boards") return boardsReply()
      const m = u.pathname.match(/^\/api\/v1\/phones\/boards\/([^/]+)\/stop$/)
      if (req.method === "POST" && m) {
        stops.push(decodeURIComponent(m[1]!))
        return stopReply(m[1]!)
      }
      return new Response("not found", { status: 404 })
    },
  })
  realFetch = globalThis.fetch
  globalThis.fetch = ((input: any, init?: any) => {
    const u = new URL(typeof input === "string" ? input : input.url)
    return realFetch(`http://127.0.0.1:${server.port}${u.pathname}${u.search}`, init)
  }) as any
  process.env.ALLTERNIT_API_TOKEN = "test-token"
  const mod = await import("@/cli/commands/phone")
  BoardsCommand = mod.BoardsCommand
  StopCommand = mod.StopCommand
})
afterAll(() => {
  globalThis.fetch = realFetch
  server.stop(true)
})

const json = (body: unknown, status = 200) => Response.json(body, { status })

/** Run a handler and return what the user sees: stdout, stderr, exit code. */
async function run(command: any, args: Record<string, unknown> = {}) {
  const out: string[] = []
  const err: string[] = []
  const w = process.stdout.write.bind(process.stdout)
  const e = process.stderr.write.bind(process.stderr)
  process.stdout.write = ((s: any) => (out.push(String(s)), true)) as any
  process.stderr.write = ((s: any) => (err.push(String(s)), true)) as any
  process.exitCode = 0
  try {
    await command.handler(args)
  } finally {
    process.stdout.write = w
    process.stderr.write = e
  }
  const code = Number(process.exitCode ?? 0)
  process.exitCode = 0
  return { out: out.join(""), err: err.join(""), code }
}

const two = () =>
  json({
    desktop: true,
    boards: [
      board({}),
      board({ boardId: "usba-ffffffffffff", name: "Travel board", armed: false, status: "disarmed", link: "cloud", dock: false, face: "wink" }),
    ],
  })

beforeEach(() => {
  stops.length = 0
  boardsReply = two
  stopReply = () => json({ ok: true })
})

describe("gizzi phone boards", () => {
  test("one line per board with armed, link, dock and face", async () => {
    const r = await run(BoardsCommand)
    expect(r.code).toBe(0)
    const lines = r.out.trimEnd().split("\n")
    expect(lines).toHaveLength(2)
    expect(lines[0]).toContain("Desk board")
    expect(lines[0]).toContain("ready")
    expect(lines[0]).toContain("armed: yes")
    expect(lines[0]).toContain("link: lan")
    expect(lines[0]).toContain("dock: yes")
    expect(lines[0]).toContain("face: working")
    expect(lines[1]).toContain("Travel board")
    expect(lines[1]).toContain("armed: no")
    expect(lines[1]).toContain("link: cloud")
    expect(lines[1]).toContain("dock: no")
    console.log(r.out)
  })
  test("an offline board shows dashes, not made-up values", async () => {
    boardsReply = () => json({ desktop: true, boards: [board({ online: false, armed: false, status: "board_offline", link: null, dock: null, face: null })] })
    const r = await run(BoardsCommand)
    expect(r.out).toContain("offline")
    expect(r.out).toContain("link: -")
    expect(r.out).toContain("dock: -")
    expect(r.out).toContain("face: -")
  })
  test("no boards says so and exits 0", async () => {
    boardsReply = () => json({ desktop: true, boards: [] })
    const r = await run(BoardsCommand)
    expect(r.code).toBe(0)
    expect(r.out).toContain("No Allternit Link board is attached")
  })
  test("Desktop not running", async () => {
    boardsReply = () => json({ desktop: false, boards: [] })
    const r = await run(BoardsCommand)
    expect(r.code).toBe(1)
    expect(r.err).toContain("Desktop isn't running")
  })
  test("an old runtime answering 404 is called out plainly", async () => {
    boardsReply = () => new Response("not found", { status: 404 })
    const r = await run(BoardsCommand)
    expect(r.code).toBe(1)
    expect(r.err).toContain("doesn't support Allternit Link boards yet")
  })
})

describe("gizzi phone stop", () => {
  test("two boards and no --board names both, exits non-zero, stops nothing", async () => {
    const r = await run(StopCommand)
    expect(r.code).toBe(1)
    expect(r.err).toContain("Desk board")
    expect(r.err).toContain("Travel board")
    expect(r.err).toContain("--board")
    expect(stops).toEqual([])
  })
  test("one board: --board is optional and the stop route is called", async () => {
    boardsReply = () => json({ desktop: true, boards: [board({})] })
    const r = await run(StopCommand)
    expect(r.code).toBe(0)
    expect(stops).toEqual(["usba-0123456789ab"])
    expect(r.out).toContain("Stopped Desk board")
    expect(r.out).toContain("disarmed")
  })
  test("--board by name picks the right one", async () => {
    const r = await run(StopCommand, { board: "travel" })
    expect(r.code).toBe(0)
    expect(stops).toEqual(["usba-ffffffffffff"])
  })
  test("--board that matches nothing lists what exists", async () => {
    const r = await run(StopCommand, { board: "nope" })
    expect(r.code).toBe(1)
    expect(r.err).toContain('No board matches "nope"')
    expect(stops).toEqual([])
  })
  test("no boards", async () => {
    boardsReply = () => json({ desktop: true, boards: [] })
    const r = await run(StopCommand)
    expect(r.code).toBe(1)
    expect(r.err).toContain("nothing to stop")
  })
  test("board_offline", async () => {
    stopReply = () => json({ error: "board_offline", message: "x" }, 409)
    const r = await run(StopCommand, { board: "desk" })
    expect(r.code).toBe(1)
    expect(r.err).toContain("Desk board is offline")
  })
  test("desktop_offline", async () => {
    stopReply = () => json({ error: "desktop_offline", message: "x" }, 503)
    const r = await run(StopCommand, { board: "desk" })
    expect(r.code).toBe(1)
    expect(r.err).toContain("Desktop isn't running")
  })
  test("uncertain is never reported as stopped", async () => {
    stopReply = () => json({ error: "uncertain", message: "x" }, 502)
    const r = await run(StopCommand, { board: "desk" })
    expect(r.code).toBe(1)
    expect(r.out).not.toContain("Stopped")
    expect(r.err).toContain("still armed")
  })
  test("old runtime 404 on the stop route", async () => {
    stopReply = () => new Response("nope", { status: 404 })
    const r = await run(StopCommand, { board: "desk" })
    expect(r.code).toBe(1)
    expect(r.err).toContain("doesn't support Allternit Link boards yet")
  })
})

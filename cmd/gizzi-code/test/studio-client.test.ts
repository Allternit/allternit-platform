// @ts-nocheck
import { afterAll, beforeAll, describe, expect, test } from "bun:test"
import {
  createStudioClient,
  exitCodeForError,
  exitCodeForState,
  followJob,
  hardwareFit,
  specFromFlags,
  StudioError,
} from "../src/runtime/services/studioClient"

// Minimal stub that follows services/studio-spec/openapi/studio-api.yaml:
// /events?after=<seq> returns events with seq > after plus the current state,
// 422 refusals are {error, message}.
const script = [
  { seq: 1, at: "t", kind: "state", message: "preparing" },
  { seq: 2, at: "t", kind: "log", message: "loading" },
  { seq: 3, at: "t", kind: "progress", message: "step", progress: 0.5 },
  { seq: 4, at: "t", kind: "state", message: "succeeded" },
]
let polls: number[] = []
let server: any
let base = ""
beforeAll(() => {
  server = Bun.serve({
    port: 0,
    async fetch(req) {
      const u = new URL(req.url)
      const j = (b: any, status = 200) => new Response(JSON.stringify(b), { status, headers: { "content-type": "application/json" } })
      if (u.pathname === "/api/v1/studio/jobs" && req.method === "POST") {
        const spec = await req.json()
        if (spec.resources?.place === "rented") return j({ error: "rented_not_available", message: "Rented compute is not available yet." }, 422)
        return j({ id: "job-1", state: "queued", spec, created_at: "t" }, 201)
      }
      const m = u.pathname.match(/^\/api\/v1\/studio\/jobs\/([^/]+)\/events$/)
      if (m) {
        if (m[1] !== "job-1") return j({ error: "not_found", message: "no such job" }, 404)
        const after = Number(u.searchParams.get("after") ?? 0)
        polls.push(after)
        // Reveal two more events per poll, so following needs several pages.
        const visible = Math.min(script.length, after + 2)
        const events = script.filter((e) => e.seq > after && e.seq <= visible)
        return j({ state: visible >= 4 ? "succeeded" : "running", events })
      }
      return j({ error: "not_found", message: "x" }, 404)
    },
  })
  base = `http://127.0.0.1:${server.port}`
})
afterAll(() => server.stop(true))

describe("followJob", () => {
  test("pages with after=<last seq>, never repeats an event, stops at terminal state", async () => {
    polls = []
    const seen: number[] = []
    const r = await followJob(createStudioClient({ base }), "job-1", { intervalMs: 1, onEvent: (e) => seen.push(e.seq) })
    expect(seen).toEqual([1, 2, 3, 4])
    expect(polls.slice(0, 2)).toEqual([0, 2])
    expect(r.state).toBe("succeeded")
    expect(r.detached).toBe(false)
  })
  test("--after resumes mid-stream", async () => {
    const seen: number[] = []
    await followJob(createStudioClient({ base }), "job-1", { after: 2, intervalMs: 1, onEvent: (e) => seen.push(e.seq) })
    expect(seen).toEqual([3, 4])
  })
  test("abort detaches without error", async () => {
    const ac = new AbortController()
    const fake = { events: async () => ({ state: "running", events: [] }) }
    setTimeout(() => ac.abort(), 20)
    const r = await followJob(fake, "x", { intervalMs: 5, signal: ac.signal, onEvent: () => {} })
    expect(r.detached).toBe(true)
  })
  test("follow:false returns after one page", async () => {
    polls = []
    await followJob(createStudioClient({ base }), "job-1", { follow: false, onEvent: () => {} })
    expect(polls.length).toBe(1)
  })
})

describe("exit codes and refusals", () => {
  test("job states map to 0 / 1", () => {
    expect(exitCodeForState("succeeded")).toBe(0)
    expect(exitCodeForState("failed")).toBe(1)
    expect(exitCodeForState("cancelled")).toBe(1)
  })
  test("a 422 refusal carries the host's code and message and exits 2", async () => {
    const spec = specFromFlags({ engine: "allternit-tune", base: "freeweights/q", dataset: "~/d.jsonl", place: "rented" })
    const err = await createStudioClient({ base }).createJob(spec).catch((e) => e)
    expect(err).toBeInstanceOf(StudioError)
    expect(err.code).toBe("rented_not_available")
    expect(err.message).toContain("not available")
    expect(exitCodeForError(err)).toBe(2)
  })
  test("404 and unreachable host exit 1", async () => {
    const e404 = await createStudioClient({ base }).events("nope", 0).catch((e) => e)
    expect(e404.code).toBe("not_found")
    expect(exitCodeForError(e404)).toBe(1)
    const down = await createStudioClient({ base: "http://127.0.0.1:9" }).jobs().catch((e) => e)
    expect(down.code).toBe("unreachable")
    expect(exitCodeForError(down)).toBe(1)
  })
})

describe("spec and hardware", () => {
  test("flags build a spec v1; path vs repo dataset", () => {
    const a = specFromFlags({ engine: "e", base: "b/r", dataset: "~/x.jsonl" })
    expect(a.dataset).toEqual({ format: "chat", path: "~/x.jsonl" })
    expect(a.method).toBe("lora")
    expect(specFromFlags({ engine: "e", base: "b/r", dataset: "org/ds" }).dataset.repo).toBe("org/ds")
  })
  test("hardware fit gives a reason when refused", () => {
    expect(hardwareFit({ hardware: ["apple-silicon"] }, "darwin", "arm64").fits).toBe(true)
    const r = hardwareFit({ hardware: ["cuda-linux"] }, "darwin", "arm64")
    expect(r.fits).toBe(false)
    expect(r.reason).toContain("cuda-linux")
  })
})

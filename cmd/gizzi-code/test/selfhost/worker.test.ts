import { describe, expect, test } from "bun:test"
import path from "path"
import { createGizziToolExecutor } from "../../src/runtime/selfhost/executor"
import { MAX_RESULT_BYTES, runSelfHostWorker, truncateResult } from "../../src/runtime/selfhost/worker"
import { tmpdir } from "../fixture/fixture"

// `gizzi worker` against a stand-in Agents Server (only the network edge is
// faked): claim → run each tool call with gizzi's real tools in the workdir →
// post results → complete when tool_calls/next says the session is closed.

const ENV_KEY = "aek_test_environment_key"
const SECRET = "sec_test_session_secret"

type Recorded = { path: string; headers: Headers; body: any }

function agentsServer(opts: { calls: Array<{ tool_call_id: string; name: string; input: Record<string, unknown> }>; claims?: number }) {
  const recorded: Recorded[] = []
  let claimsLeft = opts.claims ?? 1
  const queue = [...opts.calls]
  const srv = Bun.serve({
    port: 0,
    hostname: "127.0.0.1",
    async fetch(req) {
      const url = new URL(req.url)
      const body = await req.json().catch(() => ({}))
      recorded.push({ path: url.pathname, headers: req.headers, body })
      if (url.pathname === "/v1/environments/env_1/work/claim") {
        if (req.headers.get("authorization") !== `Bearer ${ENV_KEY}`) return new Response(null, { status: 401 })
        if (claimsLeft-- <= 0) return new Response(null, { status: 204 })
        return Response.json({
          work_id: "work_1",
          session_id: "sess_1",
          session_secret: SECRET,
          lease_generation: 3,
          lease_expires_at: new Date(Date.now() + 60_000).toISOString(),
          environment: { network: "none", allowed_hosts: [], git: {}, mcp: {} },
        })
      }
      if (url.pathname === "/v1/work/work_1/tool_calls/next") {
        if (req.headers.get("x-session-secret") !== SECRET) return new Response(null, { status: 401 })
        const next = queue.shift()
        if (next) return Response.json(next)
        return new Response(null, { status: 204, headers: { "X-Session-Closed": "true" } })
      }
      if (url.pathname.startsWith("/v1/tool_calls/") && url.pathname.endsWith("/result")) {
        if (req.headers.get("x-session-secret") !== SECRET) return new Response(null, { status: 401 })
        return Response.json({ tool_call_id: url.pathname.split("/")[3], status: body.is_error ? "errored" : "completed" })
      }
      if (url.pathname === "/v1/work/work_1/complete" || url.pathname === "/v1/work/work_1/release" || url.pathname === "/v1/work/work_1/renew") {
        return Response.json({ work_id: "work_1", lease_generation: 3, lease_expires_at: null, status: "completed" })
      }
      return new Response("not found", { status: 404 })
    },
  })
  return { srv, recorded, url: `http://127.0.0.1:${srv.port}` }
}

function collectLogs() {
  const lines: string[] = []
  const log = (level: string, event: string, fields?: Record<string, unknown>) => {
    lines.push(JSON.stringify({ level, event, ...(fields ?? {}) }))
  }
  return { lines, log }
}

describe("gizzi worker", () => {
  test("--drain runs the session's tool calls in the workdir, posts results, completes and exits 0", async () => {
    await using tmp = await tmpdir({ git: true })
    const target = path.join(tmp.path, "a.txt")
    const s = agentsServer({
      calls: [
        { tool_call_id: "call_write", name: "write", input: { filePath: target, content: "ok\n" } },
        { tool_call_id: "call_read", name: "read", input: { filePath: target } },
        { tool_call_id: "call_missing", name: "web_browse", input: {} },
      ],
    })
    const { lines, log } = collectLogs()
    try {
      const code = await runSelfHostWorker({
        server: s.url,
        environmentKey: ENV_KEY,
        environmentId: "env_1",
        workerId: "worker-test",
        blockMs: 0,
        drain: true,
        workdir: tmp.path,
        log,
        stop: new AbortController().signal,
        executor: createGizziToolExecutor({ workdir: tmp.path, log, stateDir: path.join(tmp.path, ".state") }),
      })
      expect(code).toBe(0)
      expect(await Bun.file(target).text()).toBe("ok\n")

      const results = s.recorded.filter((r) => r.path.endsWith("/result"))
      expect(results.map((r) => r.path)).toEqual([
        "/v1/tool_calls/call_write/result",
        "/v1/tool_calls/call_read/result",
        "/v1/tool_calls/call_missing/result",
      ])
      expect(results[0]!.body.is_error).toBe(false)
      expect(results[1]!.body.is_error).toBe(false)
      expect(results[1]!.body.output).toContain("ok")
      expect(results[2]!.body.is_error).toBe(true)
      expect(results[2]!.body.output).toContain("does not run the tool")

      const complete = s.recorded.find((r) => r.path === "/v1/work/work_1/complete")
      expect(complete?.body).toEqual({ lease_generation: 3, outcome: "success" })
      expect(s.recorded.some((r) => r.path === "/v1/work/work_1/release")).toBe(false)

      const claim = s.recorded.find((r) => r.path.endsWith("/work/claim"))
      expect(claim?.body.worker_id).toBe("worker-test")

      // Neither credential ever reaches a log line.
      for (const line of lines) {
        expect(line).not.toContain(SECRET)
        expect(line).not.toContain(ENV_KEY)
      }
    } finally {
      s.srv.stop(true)
    }
  })

  test("--drain with no work exits 0 after one empty claim", async () => {
    await using tmp = await tmpdir()
    const s = agentsServer({ calls: [], claims: 0 })
    const { log } = collectLogs()
    try {
      const code = await runSelfHostWorker({
        server: s.url,
        environmentKey: ENV_KEY,
        environmentId: "env_1",
        blockMs: 0,
        drain: true,
        workdir: tmp.path,
        log,
        stop: new AbortController().signal,
        executor: createGizziToolExecutor({ workdir: tmp.path, log }),
      })
      expect(code).toBe(0)
      expect(s.recorded.filter((r) => r.path.endsWith("/work/claim"))).toHaveLength(1)
    } finally {
      s.srv.stop(true)
    }
  })

  test("a rejected environment key stops the worker with exit 1", async () => {
    await using tmp = await tmpdir()
    const s = agentsServer({ calls: [] })
    const { log } = collectLogs()
    try {
      const code = await runSelfHostWorker({
        server: s.url,
        environmentKey: "aek_wrong",
        environmentId: "env_1",
        blockMs: 0,
        workdir: tmp.path,
        log,
        stop: new AbortController().signal,
        executor: createGizziToolExecutor({ workdir: tmp.path, log }),
      })
      expect(code).toBe(1)
    } finally {
      s.srv.stop(true)
    }
  })
})

describe("truncateResult", () => {
  test("keeps output under 100 KiB without splitting a character, and says it was cut", () => {
    const big = "é".repeat(80_000) // 160,000 bytes
    const out = truncateResult(big)
    expect(Buffer.byteLength(out, "utf8")).toBeLessThanOrEqual(MAX_RESULT_BYTES)
    expect(out).not.toContain("�")
    expect(out).toContain("output truncated from 160000 bytes")
  })

  test("leaves small output alone", () => {
    expect(truncateResult("ok\n")).toBe("ok\n")
  })
})

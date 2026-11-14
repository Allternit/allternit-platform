import { describe, expect, test } from "bun:test"
import path from "path"
import { realpathSync } from "fs"
import { createGizziToolExecutor, WORKER_TOOL_NAMES } from "../../src/runtime/selfhost/executor"
import { parseToolsFile, ToolsFileError, type CustomToolSpec } from "../../src/runtime/selfhost/custom-tools"
import { MAX_RESULT_BYTES, runSelfHostWorker, truncateResult } from "../../src/runtime/selfhost/worker"
import { Sandbox } from "../../src/runtime/integrations/shell/sandbox"
import { tmpdir } from "../fixture/fixture"

// `gizzi worker` against a stand-in Agents Server (only the network edge is
// faked): claim → run each tool call with gizzi's real tools in the workdir →
// post results → complete when tool_calls/next says the session is closed.

const ENV_KEY = "aek_test_environment_key"
const SECRET = "sec_test_session_secret"

type Recorded = { path: string; headers: Headers; body: any }

type Call = { tool_call_id: string; name: string; input: Record<string, unknown> }

function agentsServer(opts: {
  calls: Call[]
  claims?: number
  environment?: Record<string, unknown>
  /** Answer a plain 204 (session quiet, not closed) once the calls run out. */
  quietWhenDone?: boolean
}) {
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
          environment: opts.environment ?? { network: "none", allowed_hosts: [], git: {}, mcp: {} },
        })
      }
      if (url.pathname === "/v1/work/work_1/tool_calls/next") {
        if (req.headers.get("x-session-secret") !== SECRET) return new Response(null, { status: 401 })
        const next = queue.shift()
        if (next) return Response.json(next)
        if (opts.quietWhenDone) return new Response(null, { status: 204 })
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
  return {
    srv,
    recorded,
    url: `http://127.0.0.1:${srv.port}`,
    /** Queue more calls for the next claim. */
    push(...calls: Call[]) {
      queue.push(...calls)
      claimsLeft++
    },
    results: () => recorded.filter((r) => r.path.endsWith("/result")),
  }
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

describe("gizzi worker: release ends the session on this worker", () => {
  // Drain mode releases a quiet session so its next call can go to any
  // worker. The worker's state for it (shadow session, scratchpad, background
  // tasks, network proxy) must end then too, as it does on complete.
  test("after a drain release, a later claim of the same session starts clean", async () => {
    await using tmp = await tmpdir({ git: true })
    const s = agentsServer({
      quietWhenDone: true,
      calls: [{ tool_call_id: "call_note", name: "scratchpad_write", input: { path: "note.txt", content: "left over" } }],
    })
    const { log } = collectLogs()
    const executor = createGizziToolExecutor({ workdir: tmp.path, log, stateDir: path.join(tmp.path, ".state") })
    const drain = () =>
      runSelfHostWorker({
        server: s.url,
        environmentKey: ENV_KEY,
        environmentId: "env_1",
        blockMs: 0,
        drain: true,
        workdir: tmp.path,
        log,
        stop: new AbortController().signal,
        executor,
      })
    try {
      expect(await drain()).toBe(0)
      expect(s.recorded.some((r) => r.path === "/v1/work/work_1/release")).toBe(true)
      expect(s.results()[0]!.body.is_error).toBe(false)

      s.push({ tool_call_id: "call_read", name: "scratchpad_read", input: { path: "note.txt" } })
      expect(await drain()).toBe(0)
      const read = s.results().find((r) => r.path === "/v1/tool_calls/call_read/result")!
      expect(read.body.output).not.toContain("left over")
      expect(read.body.is_error).toBe(true)
    } finally {
      s.srv.stop(true)
    }
  })
})

describe("gizzi worker --tools-file", () => {
  const tools = (entries: Record<string, CustomToolSpec>) => new Map(Object.entries(entries))

  async function runTools(opts: {
    dir: string
    customTools: Map<string, CustomToolSpec>
    calls: Call[]
    environment?: Record<string, unknown>
  }) {
    const s = agentsServer({ calls: opts.calls, environment: opts.environment })
    const { log } = collectLogs()
    try {
      const code = await runSelfHostWorker({
        server: s.url,
        environmentKey: ENV_KEY,
        environmentId: "env_1",
        blockMs: 0,
        drain: true,
        workdir: opts.dir,
        log,
        stop: new AbortController().signal,
        executor: createGizziToolExecutor({
          workdir: opts.dir,
          log,
          stateDir: path.join(opts.dir, ".state"),
          customTools: opts.customTools,
        }),
      })
      expect(code).toBe(0)
      return Object.fromEntries(s.results().map((r) => [r.path.split("/")[3]!, r.body as { output: string; is_error: boolean }]))
    } finally {
      s.srv.stop(true)
    }
  }

  test("a command tool gets the input JSON on stdin, runs in the workdir, and its stdout is the result", async () => {
    await using tmp = await tmpdir({ git: true })
    const out = await runTools({
      dir: tmp.path,
      customTools: tools({
        echo_input: { command: ["/bin/cat"], timeout_ms: 10_000 },
        // Words reach the program as given: no shell expansion of $HOME or quotes.
        literal: { command: ["/bin/echo", "it's $HOME"], timeout_ms: 10_000 },
        where: { command: ["/bin/pwd"], timeout_ms: 10_000 },
      }),
      calls: [
        { tool_call_id: "call_echo", name: "echo_input", input: { city: "Oslo", n: 2 } },
        { tool_call_id: "call_literal", name: "literal", input: {} },
        { tool_call_id: "call_where", name: "where", input: {} },
      ],
    })
    expect(out.call_echo).toEqual({ output: JSON.stringify({ city: "Oslo", n: 2 }), is_error: false })
    expect(out.call_literal).toEqual({ output: "it's $HOME\n", is_error: false })
    expect(realpathSync(out.call_where!.output.trim())).toBe(realpathSync(tmp.path))
  })

  test("a non-zero exit is an error result with the end of stderr; a timeout is an error too", async () => {
    await using tmp = await tmpdir({ git: true })
    const out = await runTools({
      dir: tmp.path,
      customTools: tools({
        fails: { command: ["/bin/sh", "-c", "echo partial; echo 'lookup failed: no such city' >&2; exit 3"], timeout_ms: 10_000 },
        slow: { command: ["/bin/sleep", "30"], timeout_ms: 300 },
      }),
      calls: [
        { tool_call_id: "call_fails", name: "fails", input: {} },
        { tool_call_id: "call_slow", name: "slow", input: {} },
      ],
    })
    expect(out.call_fails!.is_error).toBe(true)
    expect(out.call_fails!.output).toContain("exited with code 3")
    expect(out.call_fails!.output).toContain("lookup failed: no such city")
    expect(out.call_slow!.is_error).toBe(true)
    expect(out.call_slow!.output).toContain("timed out after 300 ms")
  })

  test("stdout over 100 KiB is cut to the result limit", async () => {
    await using tmp = await tmpdir({ git: true })
    const out = await runTools({
      dir: tmp.path,
      customTools: tools({ big: { command: ["/bin/sh", "-c", "head -c 300000 /dev/zero | tr '\\0' x"], timeout_ms: 10_000 } }),
      calls: [{ tool_call_id: "call_big", name: "big", input: {} }],
    })
    expect(out.call_big!.is_error).toBe(false)
    expect(Buffer.byteLength(out.call_big!.output)).toBeLessThanOrEqual(MAX_RESULT_BYTES)
    expect(out.call_big!.output).toContain("output truncated from 300000 bytes")
  })

  // Same skip rule as the real-sandbox bash test (test/tool/run-code.test.ts):
  // macOS always has sandbox-exec; Linux needs bubblewrap.
  describe.skipIf(process.platform !== "darwin" && Sandbox.detect() === "none")("under the session sandbox", () => {
    test("network: none blocks the tool's network call; network: full lets the same call through", async () => {
      await using tmp = await tmpdir({ git: true })
      const target = Bun.serve({ port: 0, hostname: "127.0.0.1", fetch: () => new Response("pong") })
      const fetchTool = tools({
        fetch_url: {
          command: [process.execPath, "-e", "const { url } = JSON.parse(await Bun.stdin.text()); console.log(await (await fetch(url)).text())"],
          timeout_ms: 20_000,
        },
      })
      const call = { tool_call_id: "call_fetch", name: "fetch_url", input: { url: `http://127.0.0.1:${target.port}/` } }
      try {
        const open = await runTools({ dir: tmp.path, customTools: fetchTool, calls: [call], environment: { network: "full" } })
        expect(open.call_fetch).toEqual({ output: "pong\n", is_error: false })

        const closed = await runTools({ dir: tmp.path, customTools: fetchTool, calls: [call], environment: { network: "none" } })
        expect(closed.call_fetch!.is_error).toBe(true)
        expect(closed.call_fetch!.output).not.toContain("pong")
      } finally {
        target.stop(true)
      }
    })
  })

  test("the file is checked: bad JSON, wrong shape, empty command, or a built-in tool name is refused", () => {
    const parse = (text: string) => parseToolsFile(text, "tools.json", WORKER_TOOL_NAMES)
    expect(() => parse("{not json")).toThrow(/not valid JSON/)
    expect(() => parse(JSON.stringify({ lookup: { command: ["x"] } }))).toThrow(/expected \{"tools"/)
    expect(() => parse(JSON.stringify({ tools: { lookup: { command: [] } } }))).toThrow(/non-empty "command"/)
    expect(() => parse(JSON.stringify({ tools: { lookup: { command: "lookup.sh" } } }))).toThrow(/non-empty "command"/)
    expect(() => parse(JSON.stringify({ tools: { lookup: { command: ["x"], timeout_ms: 0 } } }))).toThrow(/timeout_ms/)
    expect(() => parse(JSON.stringify({ tools: { Bash: { command: ["x"] } } }))).toThrow(/built-in tool "bash"/)
    expect(() => parse(JSON.stringify({ tools: { ls: { command: ["x"] } } }))).toThrow(/built-in tool "ls"/)
    expect(() => parse(JSON.stringify({ tools: { lookup: { command: ["x"], mcp: "y" } } }))).toThrow(ToolsFileError)
    expect(parse(JSON.stringify({ tools: { lookup: { command: ["./lookup", "--json"] } } })).get("lookup")).toEqual({
      command: ["./lookup", "--json"],
      timeout_ms: 120_000,
    })
  })

  test("`gizzi worker` with a bad tools file prints why and exits 1 before it claims anything", async () => {
    await using tmp = await tmpdir()
    const file = path.join(tmp.path, "tools.json")
    await Bun.write(file, JSON.stringify({ tools: { bash: { command: ["/bin/true"] } } }))
    const s = agentsServer({ calls: [] })
    try {
      const root = path.resolve(import.meta.dir, "../..")
      const proc = Bun.spawn(
        [process.execPath, path.join(root, "script/dev.mjs"), "worker", "--server", s.url, "--environment-key", ENV_KEY, "--environment", "env_1", "--workdir", tmp.path, "--tools-file", file],
        { cwd: root, stdout: "pipe", stderr: "pipe", env: { ...process.env, ALLTERNIT_WORKER_TOOLS: "" } },
      )
      const code = await proc.exited
      const fatal = (text: string) => JSON.parse(text.trim().split("\n").at(-1)!) as { event: string; error: string }
      const stderr = fatal(await new Response(proc.stderr).text())
      expect(code).toBe(1)
      expect(stderr.event).toBe("worker.fatal")
      expect(stderr.error).toBe(`tools file ${file}: tool "bash" has the name of the worker's built-in tool "bash"; rename it`)
      expect(s.recorded.filter((r) => r.path.endsWith("/work/claim"))).toHaveLength(0)

      // The env var is read the same way.
      await Bun.write(file, "{oops")
      const viaEnv = Bun.spawn(
        [process.execPath, path.join(root, "script/dev.mjs"), "worker", "--server", s.url, "--environment-key", ENV_KEY, "--environment", "env_1", "--workdir", tmp.path],
        { cwd: root, stdout: "pipe", stderr: "pipe", env: { ...process.env, ALLTERNIT_WORKER_TOOLS: file } },
      )
      expect(await viaEnv.exited).toBe(1)
      expect(fatal(await new Response(viaEnv.stderr).text()).error).toContain(`tools file ${file}: not valid JSON`)
      expect(s.recorded.filter((r) => r.path.endsWith("/work/claim"))).toHaveLength(0)
    } finally {
      s.srv.stop(true)
    }
  }, 60_000)
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

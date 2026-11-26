import { describe, expect, test } from "bun:test"
import { ScriptSandbox } from "@/runtime/services/mcp/script-sandbox"
import { McpScript } from "@/runtime/tools/builtins/mcp-script"

// Two fake servers; `call` stands in for the session loop's permission + call path.
function bridges(denied = new Set<string>()): ScriptSandbox.Bridge[] {
  const tool = (server: string, name: string, value: (args: any) => unknown): ScriptSandbox.Bridge => ({
    server,
    tool: name,
    async call(args) {
      if (denied.has(`${server}.${name}`)) throw new Error("The user rejected permission to use this specific tool call.")
      return value(args)
    },
  })
  return [
    tool("github", "list_issues", () => [{ id: 1 }, { id: 2 }, { id: 3 }]),
    tool("linear", "create-issue", (args) => ({ created: args.title })),
  ]
}

const run = (script: string, b = bridges(), limits?: Partial<typeof ScriptSandbox.LIMITS>) =>
  ScriptSandbox.run({ js: McpScript.toJavaScript(script), bridges: b, limits })

describe("mcp_script sandbox", () => {
  test("4 calls across 2 servers are all logged and the result comes back", async () => {
    const r = await run(`
      const issues: Array<{ id: number }> = await github.list_issues({})
      const made: string[] = []
      for (const i of issues) made.push((await linear.create_issue({ title: "copy " + i.id })).created)
      return made
    `)
    expect(r.ok).toBe(true)
    expect(r.calls.length).toBe(4)
    expect(r.calls.map((c) => c.server)).toEqual(["github", "linear", "linear", "linear"])
    expect(JSON.parse(r.result)).toEqual(["copy 1", "copy 2", "copy 3"])
  })

  test("a denied tool rejects inside the script", async () => {
    const r = await run(
      `try { await linear.create_issue({ title: "x" }); return "ran" } catch (e) { return "denied: " + e.message }`,
      bridges(new Set(["linear.create-issue"])),
    )
    expect(r.ok).toBe(true)
    expect(r.result).toContain("denied:")
    expect(r.calls[0]).toMatchObject({ server: "linear", tool: "create-issue", ok: false })
  })

  test("while(true){} stops at the run limit", async () => {
    const started = Date.now()
    const r = await run(`while (true) {}`, bridges(), { runMs: 300 })
    expect(r.ok).toBe(false)
    expect(r.error).toContain("limit")
    expect(Date.now() - started).toBeLessThan(5_000)
  })

  test("the call limit holds", async () => {
    const r = await run(`let n = 0; for (let i = 0; i < 60; i++) { try { await github.list_issues({}); n++ } catch {} } return n`)
    expect(JSON.parse(r.result)).toBe(50)
  })

  test("no host objects: process, constructor escape and import() give nothing", async () => {
    const r = await run(`
      const out: Record<string, unknown> = {}
      out.process = typeof (globalThis as any).process
      try { out.ctor = typeof (({}).constructor.constructor("return process"))() } catch (e) { out.ctor = "threw" }
      try { await import("node:fs"); out.import = "loaded" } catch (e) { out.import = "threw" }
      out.fetch = typeof (globalThis as any).fetch
      out.setTimeout = typeof (globalThis as any).setTimeout
      out.require = typeof (globalThis as any).require
      return out
    `)
    expect(r.ok).toBe(true)
    expect(JSON.parse(r.result)).toEqual({
      process: "undefined",
      ctor: "undefined",
      import: "threw",
      fetch: "undefined",
      setTimeout: "undefined",
      require: "undefined",
    })
  })

  test("the result is cut at 8 KB", async () => {
    const r = await run(`return "x".repeat(20000)`)
    expect(new TextEncoder().encode(r.result).byteLength).toBeLessThanOrEqual(8 * 1024)
  })

  test("proxied app-chat tools are bridged as <connector>.<tool>", () => {
    expect(
      McpScript.bridgeName({ serverName: "allternit", originalName: "gcal__list_events", appName: "Google Calendar", connectorId: "c1" }, "k"),
    ).toEqual({ server: "google_calendar", tool: "list_events" })
    expect(McpScript.bridgeName({ serverName: "allternit", originalName: "gh__a__b", connectorId: "c2" }, "k")).toEqual({
      server: "gh",
      tool: "a__b",
    })
    expect(McpScript.bridgeName({ serverName: "local-fs", originalName: "read__file" }, "k")).toEqual({
      server: "local-fs",
      tool: "read__file",
    })
  })

  test("declarations come from input and output schemas", () => {
    const d = McpScript.declarations([
      { key: "a", server: "github", tool: "list_issues", inputSchema: { type: "object", properties: { q: { type: "string" } }, required: ["q"] }, outputSchema: { type: "object", properties: { n: { type: "integer" } } }, call: async () => 0 },
      { key: "b", server: "linear", tool: "create-issue", inputSchema: { type: "object", properties: {} }, call: async () => 0 },
    ])
    expect(d).toContain("list_issues(args: { q: string }): Promise<{ n?: number }>")
    expect(d).toContain("create_issue(args: {  }): Promise<unknown>")
  })
})

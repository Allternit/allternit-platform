import { afterAll, beforeEach, describe, expect, test } from "bun:test"

/**
 * connector_search / connector_suggest (Wave D lanes C): the real tools, the
 * real core and the real routing client. Only the HTTP edge (allternit-api
 * /bot-routing/connector/search) is stubbed. The server's owner check has its
 * own Rust tests in allternit-api agent_connector_tools.rs.
 */
import { mkdtempSync } from "node:fs"
import { tmpdir as osTmpdir } from "node:os"
import path from "path"

process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN = "internal-test"
process.env.GIZZI_CONFIG_DIR = mkdtempSync(path.join(osTmpdir(), "gizzi-connector-"))
const { Connectors } = await import("../../src/runtime/primitives/connectors")
const { ConnectorSearchTool, ConnectorSuggestTool } = await import("../../src/runtime/tools/builtins/connector")
const { PermissionNext } = await import("../../src/runtime/tools/guard/permission/next")

let hits: any[] = []
let calls: any[] = []
const realFetch = globalThis.fetch

beforeEach(() => {
  Connectors.reset()
  calls = []
  hits = [
    { id: "notion", name: "Notion", group: "Documentation", state: "connected", available: true, mcpConnectorId: "mc-notion", serverName: "allternit-connectors" },
    { id: "slack", name: "Slack", group: "Communication", state: "available", available: true },
    { id: "zoom", name: "Zoom", group: "Meetings", state: "unavailable", available: false, reason: "Not offered on this computer." },
  ]
  globalThis.fetch = (async (input: string, init: RequestInit = {}) => {
    const body = JSON.parse(String(init.body))
    calls.push({ path: new URL(input).pathname, body })
    return Response.json({ hits: hits.slice(0, body.limit) })
  }) as typeof fetch
})

afterAll(() => {
  globalThis.fetch = realFetch
  delete process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN
})

function ctx(sessionID = "ses_conn") {
  return {
    sessionID,
    messageID: "msg_assistant",
    callID: "call_1",
    agent: "build",
    abort: AbortSignal.any([]),
    messages: [],
    metadata: () => {},
    ask: async () => {},
  } as any
}

async function run(t: any, params: any, sessionID?: string) {
  const tool = await t.init()
  return tool.execute(params, ctx(sessionID))
}

/** MCP descriptors: `n` tools of connector `id` on the user proxy. */
function toolsFor(id: string, n: number, prefix = id) {
  const out: Record<string, { connectorId?: string }> = {}
  for (let i = 0; i < n; i++) out[`mcp__allternit-connectors__${prefix}__t${String(i).padStart(2, "0")}`] = { connectorId: id }
  return out
}

describe("tool.connector_suggest", () => {
  test("refuses ids that no search in this session returned", async () => {
    const refused = await run(ConnectorSuggestTool, { ids: ["slack"], reason: "Post the summary to #team" })
    expect(refused.title).toBe("Connector suggestion refused")
    expect(refused.output).toMatch(/connector_search in this session/)
    expect(refused.metadata.card).toBeUndefined()

    // A search in another session does not count for this one.
    await run(ConnectorSearchTool, { query: "chat" }, "ses_other")
    expect((await run(ConnectorSuggestTool, { ids: ["slack"], reason: "x" })).metadata.shown).toEqual([])

    await run(ConnectorSearchTool, { query: "chat" })
    const ok = await run(ConnectorSuggestTool, { ids: ["slack", "invented", "notion"], reason: "Post the summary to #team" })
    expect(ok.metadata.shown).toEqual(["slack"])
    expect(ok.metadata.card).toEqual({
      type: "connector_suggest",
      connectors: [{ id: "slack", name: "Slack", group: "Communication" }],
      reason: "Post the summary to #team",
    })
    // The card carries no link or code.
    expect(JSON.stringify(ok.metadata.card)).not.toMatch(/https?:|token|code/i)
    expect(JSON.parse(ok.output).dropped).toEqual([
      { id: "invented", why: "not_searched" },
      { id: "notion", why: "connected" },
    ])
  })
})

describe("connector deferral and load", () => {
  test("at the threshold every connector tool mounts; above it none until a search loads them", async () => {
    const atLimit = { ...toolsFor("mc-notion", 20), ...toolsFor("mc-linear", 10), "mcp__files__read": {} }
    const mountAll = Connectors.mountFilter("ses_conn", atLimit)
    expect(Object.keys(atLimit).every(mountAll)).toBe(true)

    const over = { ...toolsFor("mc-notion", 20), ...toolsFor("mc-linear", 11), "mcp__files__read": {} }
    const before = Connectors.mountFilter("ses_conn", over)
    expect(Object.keys(over).filter(before)).toEqual(["mcp__files__read"])

    const r = await run(ConnectorSearchTool, { query: "notes", load: true })
    expect(r.metadata.loaded).toHaveLength(20)
    expect(JSON.parse(r.output).hits[0].tools).toHaveLength(20)

    const after = Connectors.mountFilter("ses_conn", over)
    const mounted = Object.keys(over).filter(after)
    expect(mounted).toHaveLength(21)
    expect(mounted.some((k) => k.includes("__mc-linear__"))).toBe(false)
    // Loads are per session.
    expect(Object.keys(over).filter(Connectors.mountFilter("ses_other", over))).toEqual(["mcp__files__read"])
  })

  test("a load mounts at most 3 connected connectors and never unconnected ones", async () => {
    hits = [
      ...["a", "b", "c", "d"].map((x) => ({ id: x, name: x, group: "G", state: "connected", available: true, mcpConnectorId: `mc-${x}` })),
      { id: "slack", name: "Slack", group: "Communication", state: "available", available: true, mcpConnectorId: "mc-slack" },
    ]
    await run(ConnectorSearchTool, { query: "x", limit: 10, load: true })
    expect(Connectors.loadedConnectors("ses_conn").sort()).toEqual(["mc-a", "mc-b", "mc-c"])
  })

  test("loaded tools keep their names, so permission rules on them still apply", async () => {
    const over = { ...toolsFor("mc-notion", 31, "notion") }
    Connectors.mountFilter("ses_conn", over)
    const r = await run(ConnectorSearchTool, { query: "notes", load: true })
    const del = "mcp__allternit-connectors__notion__t05"
    expect(r.metadata.loaded).toContain(del)
    expect(Object.keys(over).filter(Connectors.mountFilter("ses_conn", over))).toContain(del)
    const rules = PermissionNext.fromConfig({ "mcp__allternit-connectors__notion__*": "ask", [del]: "deny" })
    expect(PermissionNext.evaluate(del, "*", rules).action).toBe("deny")
    expect(PermissionNext.evaluate("mcp__allternit-connectors__notion__t06", "*", rules).action).toBe("ask")
  })

  test("search caps the limit and sends only the session id, query and limit", async () => {
    await run(ConnectorSearchTool, { query: "calendar" })
    expect(calls[0].path).toMatch(/\/bot-routing\/connector\/search$/)
    expect(calls[0].body).toEqual({ sessionId: "ses_conn", query: "calendar", limit: Connectors.DEFAULT_LIMIT })
  })
})

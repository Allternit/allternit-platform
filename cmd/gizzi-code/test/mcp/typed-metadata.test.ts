import { describe, expect, test } from "bun:test"
import { mcpToolMetadata } from "../../src/runtime/tools/mcp/apps"

const outputSchema = { type: "object", properties: { total: { type: "number" } } }
const descriptor = { serverName: "billing", originalName: "get_total", outputSchema, canReport: true }

describe("typed MCP result metadata", () => {
  test("structuredContent rides metadata.mcp with server, tool, schema and canReport", () => {
    const result = { content: [{ type: "text", text: "Total is 42" }], structuredContent: { total: 42 } }
    expect(mcpToolMetadata(descriptor, result)).toEqual({
      mcp: { server: "billing", tool: "get_total", connectorId: "billing", structuredContent: { total: 42 }, outputSchema, canReport: true },
    })
    // The model reads only `content` text; structuredContent is never part of it.
    const modelText = result.content.map((c) => c.text).join("\n\n")
    expect(modelText).not.toContain("structuredContent")
    expect(modelText).not.toContain('"total"')
  })

  test("no structuredContent and no ui resource: no metadata.mcp (row unchanged)", () => {
    expect(mcpToolMetadata(descriptor, { content: [] })).toEqual({})
    expect(mcpToolMetadata(undefined, { structuredContent: { a: 1 } })).toEqual({})
  })

  test("a ui:// tool keeps its app result next to the typed fields", () => {
    const result = { content: [], structuredContent: { total: 1 } }
    const meta = mcpToolMetadata({ ...descriptor, uiResourceUri: "ui://x" }, result).mcp!
    expect(meta.structuredContent).toEqual({ total: 1 })
    expect(meta.result).toEqual({ content: [], structuredContent: { total: 1 } })
    expect(meta.canReport).toBe(true)
  })

  test("canReport is false when the server has no submit_feedback tool", () => {
    const meta = mcpToolMetadata({ serverName: "s", originalName: "t" }, { structuredContent: { a: 1 } }).mcp!
    expect(meta.canReport).toBe(false)
  })

  test("proxied tool reports the connector id and name from _meta", () => {
    const meta = mcpToolMetadata(
      { serverName: "proxy", originalName: "crm__get", connectorId: "c_42", appName: "CRM", canReport: true },
      { structuredContent: { a: 1 } },
    ).mcp!
    expect(meta.connectorId).toBe("c_42")
    expect(meta.connectorName).toBe("CRM")
    expect(meta.canReport).toBe(true)
  })
})

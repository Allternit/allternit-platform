import { describe, expect, test } from "bun:test"
import { MCP } from "../../src/runtime/tools/mcp"

describe("MCP qualified tool names", () => {
  test("keeps server provenance and avoids builtin names", () => {
    expect(MCP.qualifiedToolName("github", "read_issue")).toBe("mcp__github__read_issue")
    expect(MCP.qualifiedToolName("filesystem", "read")).not.toBe("read")
  })

  test("bounds long provider tool names with a stable hash", () => {
    const first = MCP.qualifiedToolName("a very long server name repeated many times", "a very long tool name repeated many times")
    const second = MCP.qualifiedToolName("a very long server name repeated many times", "a very long tool name repeated many times")
    expect(first.length).toBeLessThanOrEqual(64)
    expect(first).toBe(second)
  })
})

describe("MCP tool catalog ordering", () => {
  // Servers connect concurrently, so client insertion order varies run to run. The merged
  // catalog (tool order, collision suffixes) must not.
  test("is independent of connect order", async () => {
    const prev = process.env.GIZZI_DISABLE_BUNDLED_MCPS
    process.env.GIZZI_DISABLE_BUNDLED_MCPS = "1"
    const { Instance } = await import("../../src/project/instance")
    const { tmpdir } = await import("../fixture/fixture")
    const fake = (tools: string[]) =>
      ({
        listTools: async () => ({ tools: tools.map((name) => ({ name, inputSchema: { type: "object", properties: {} } })) }),
        callTool: async () => ({ content: [] }),
      }) as any
    try {
      await using tmp = await tmpdir()
      await Instance.provide({
        directory: tmp.path,
        fn: async () => {
          // "x.y" and "x_y" normalize to the same server segment, so their tools collide.
          const a = await MCP.toolCatalog({ "x.y": fake(["b", "a"]), x_y: fake(["a"]), zed: fake(["q"]) })
          const b = await MCP.toolCatalog({ zed: fake(["q"]), x_y: fake(["a"]), "x.y": fake(["a", "b"]) })
          expect(Object.keys(a.tools)).toEqual(Object.keys(b.tools))
          expect(Object.keys(a.descriptors)).toEqual(Object.keys(b.descriptors))
          expect(a.collisions.length).toBe(1)
          expect(a.descriptors["mcp__x_y__a"].serverName).toBe("x.y")
        },
      })
    } finally {
      if (prev === undefined) delete process.env.GIZZI_DISABLE_BUNDLED_MCPS
      else process.env.GIZZI_DISABLE_BUNDLED_MCPS = prev
    }
  })
})

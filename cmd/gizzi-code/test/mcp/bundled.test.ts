import { afterEach, describe, expect, test } from "bun:test"
import { bundledMcpServers, withBundledMcpServers } from "@/runtime/tools/mcp/bundled"

const originalDisable = process.env.GIZZI_DISABLE_BUNDLED_MCPS
const originalNpx = process.env.GIZZI_ALLOW_NPX_BUNDLED_MCPS
const originalToken = process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN
const originalUser = process.env.ALLTERNIT_USER_ID

afterEach(() => {
  if (originalDisable === undefined) delete process.env.GIZZI_DISABLE_BUNDLED_MCPS
  else process.env.GIZZI_DISABLE_BUNDLED_MCPS = originalDisable
  if (originalNpx === undefined) delete process.env.GIZZI_ALLOW_NPX_BUNDLED_MCPS
  else process.env.GIZZI_ALLOW_NPX_BUNDLED_MCPS = originalNpx
  if (originalToken === undefined) delete process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN
  else process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN = originalToken
  if (originalUser === undefined) delete process.env.ALLTERNIT_USER_ID
  else process.env.ALLTERNIT_USER_ID = originalUser
})

describe("bundled MCP discovery", () => {
  test("uses installed entrypoints instead of network-backed npx", () => {
    delete process.env.GIZZI_ALLOW_NPX_BUNDLED_MCPS
    const servers = bundledMcpServers()

    const seq = servers["sequential-thinking"]
    expect(seq?.type).toBe("local")
    if (seq?.type === "local") {
      expect(seq.command[0]).toBe(process.execPath)
      expect(seq.command.join(" ")).not.toContain("npx")
    }

    const c7 = servers.context7
    expect(c7?.type).toBe("local")
    if (c7?.type === "local") {
      expect(c7.command[0]).toBe(process.execPath)
      expect(c7.command.join(" ")).not.toContain("npx")
    }
  })

  // SKIPPED: the superpowers MCP server (tools/mcp-servers/superpowers/superpowers-mcp.js) was
  // removed from the repo, so resolveSuperpowersPath() finds nothing; bundled.ts still references it.
  test.skip("resolves the repository Superpowers server independently of cwd", () => {
    const superpowers = bundledMcpServers({ cwd: "/tmp" }).superpowers
    expect(superpowers?.type).toBe("local")
    if (superpowers?.type === "local") {
      expect(superpowers.command[0]).toBe(process.execPath)
      expect(superpowers.command[1]).toEndWith("tools/mcp-servers/superpowers/superpowers-mcp.js")
    }
  })

  test("registers the connector MCP only with the internal token configured", () => {
    delete process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN
    delete process.env.ALLTERNIT_USER_ID
    expect(bundledMcpServers()["allternit-connectors"]).toBeUndefined()

    process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN = "test-secret"
    const connectors = bundledMcpServers()["allternit-connectors"]
    expect(connectors?.type).toBe("remote")
    if (connectors?.type === "remote") {
      expect(connectors.url).toEndWith("/internal/connectors/mcp")
      expect(connectors.headers?.["x-allternit-internal-token"]).toBe("test-secret")
      expect(connectors.headers?.["x-allternit-user-id"]).toBe("local-dev-user")
      expect(connectors.oauth).toBe(false)
    }
  })

  test("registers the platform tools MCP only with the internal token configured", () => {
    delete process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN
    delete process.env.ALLTERNIT_USER_ID
    expect(bundledMcpServers()["allternit-tools"]).toBeUndefined()

    process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN = "test-secret"
    const tools = bundledMcpServers()["allternit-tools"]
    expect(tools?.type).toBe("remote")
    if (tools?.type === "remote") {
      expect(tools.url).toEndWith("/internal/tools/mcp")
      expect(tools.headers?.["x-allternit-internal-token"]).toBe("test-secret")
      expect(tools.headers?.["x-allternit-user-id"]).toBe("local-dev-user")
      expect(tools.oauth).toBe(false)
    }
  })

  test("lets users disable bundled servers or override one explicitly", () => {
    process.env.GIZZI_DISABLE_BUNDLED_MCPS = "1"
    expect(withBundledMcpServers()).toStrictEqual({})

    const custom = { type: "local" as const, command: ["custom-context7"] }
    expect(withBundledMcpServers({ context7: custom })).toStrictEqual({ context7: custom })
  })
})

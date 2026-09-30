import { describe, expect, test } from "bun:test"
import {
  assertEgressAllowed,
  createGuardedFetch,
  EgressBlockedError,
  isEgressHostAllowed,
} from "@/shared/utils/hooks/ssrfGuard"

describe("egress guard", () => {
  test("cloud metadata and link-local are refused even when loopback is allowed", async () => {
    for (const host of ["169.254.169.254", "[fe80::1]", "100.100.100.200", "[::ffff:a9fe:a9fe]"]) {
      expect(await isEgressHostAllowed(host, { allowLoopback: true })).toBe(false)
      expect(await isEgressHostAllowed(host, { allowLoopback: false })).toBe(false)
    }
  })

  test("private ranges are refused from user input", async () => {
    for (const host of ["10.0.0.5", "192.168.1.1", "172.16.0.9", "0.0.0.0"]) {
      expect(await isEgressHostAllowed(host, { allowLoopback: true })).toBe(false)
    }
  })

  test("loopback only for user-configured destinations", async () => {
    expect(await isEgressHostAllowed("127.0.0.1", { allowLoopback: true })).toBe(true)
    expect(await isEgressHostAllowed("localhost", { allowLoopback: true })).toBe(true)
    expect(await isEgressHostAllowed("127.0.0.1", { allowLoopback: false })).toBe(false)
    expect(await isEgressHostAllowed("localhost", { allowLoopback: false })).toBe(false)
    expect(await isEgressHostAllowed("[::1]")).toBe(false)
  })

  test("non-http schemes are refused", async () => {
    await expect(assertEgressAllowed("file:///etc/passwd", { allowLoopback: true })).rejects.toBeInstanceOf(
      EgressBlockedError,
    )
  })

  test("guarded fetch refuses metadata and a redirect into it", async () => {
    const g = createGuardedFetch({ allowLoopback: true })
    await expect(g("http://169.254.169.254/latest/meta-data")).rejects.toBeInstanceOf(EgressBlockedError)

    const server = Bun.serve({
      port: 0,
      hostname: "127.0.0.1",
      fetch(req) {
        const p = new URL(req.url).pathname
        if (p === "/redir") return new Response(null, { status: 302, headers: { location: "http://169.254.169.254/x" } })
        return new Response("ok")
      },
    })
    try {
      // configured local MCP-style server still works
      const ok = await g(`http://127.0.0.1:${server.port}/mcp`)
      expect(await ok.text()).toBe("ok")
      await expect(g(`http://127.0.0.1:${server.port}/redir`)).rejects.toBeInstanceOf(EgressBlockedError)
      // same loopback server is refused for agent-chosen URLs
      await expect(createGuardedFetch({ allowLoopback: false })(`http://127.0.0.1:${server.port}/mcp`)).rejects.toBeInstanceOf(
        EgressBlockedError,
      )
    } finally {
      server.stop(true)
    }
  })
})

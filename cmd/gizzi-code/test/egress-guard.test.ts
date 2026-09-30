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

  test("DNS rebinding: a resolver answering public then private is refused", async () => {
    let calls = 0
    const resolve = async () => {
      calls++
      return calls === 1
        ? [{ address: "93.184.216.34", family: 4 }]
        : [{ address: "169.254.169.254", family: 4 }]
    }
    // Mixed answer (public + private in one response) is refused outright.
    const mixed = createGuardedFetch({
      resolve: async () => [
        { address: "93.184.216.34", family: 4 },
        { address: "10.0.0.5", family: 4 },
      ],
    })
    await expect(mixed("http://rebind.test/x")).rejects.toBeInstanceOf(EgressBlockedError)
    // Private answer is refused; and the vetted address is the only one ever
    // dialed: exactly one resolution per hop, no second lookup at connect.
    const g = createGuardedFetch({ resolve: async () => { calls++; return [{ address: "169.254.169.254", family: 4 }] } })
    calls = 0
    await expect(g("http://rebind.test/x")).rejects.toBeInstanceOf(EgressBlockedError)
    expect(calls).toBe(1)
  })

  test("pinned connect: resolves once and dials the vetted IP with the original Host", async () => {
    const server = Bun.serve({
      port: 0,
      hostname: "127.0.0.1",
      fetch: req => new Response(`host=${req.headers.get("host")}`),
    })
    let calls = 0
    try {
      // "pinned.test" does not exist in real DNS: only the pinned address can work.
      const g = createGuardedFetch({
        allowLoopback: true,
        resolve: async () => {
          calls++
          return [{ address: "127.0.0.1", family: 4 }]
        },
      })
      const res = await g(`http://pinned.test:${server.port}/mcp`, { method: "POST", body: "{}" })
      expect(await res.text()).toBe(`host=pinned.test:${server.port}`)
      expect(calls).toBe(1)
      // The same public-looking name that later "rebinds" to loopback is refused when loopback is not allowed.
      const strict = createGuardedFetch({ resolve: async () => [{ address: "127.0.0.1", family: 4 }] })
      await expect(strict(`http://pinned.test:${server.port}/mcp`)).rejects.toBeInstanceOf(EgressBlockedError)
    } finally {
      server.stop(true)
    }
  })

  test("a redirect hop is re-resolved and re-vetted", async () => {
    const server = Bun.serve({
      port: 0,
      hostname: "127.0.0.1",
      fetch: () => new Response(null, { status: 302, headers: { location: "http://second.test/x" } }),
    })
    const seen: string[] = []
    try {
      const g = createGuardedFetch({
        allowLoopback: true,
        resolve: async h => {
          seen.push(h)
          return [{ address: h === "first.test" ? "127.0.0.1" : "10.0.0.9", family: 4 }]
        },
      })
      await expect(g(`http://first.test:${server.port}/`)).rejects.toBeInstanceOf(EgressBlockedError)
      expect(seen).toEqual(["first.test", "second.test"])
    } finally {
      server.stop(true)
    }
  })
})

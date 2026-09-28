import { describe, test, expect } from "bun:test"
import { routeInternalFetch } from "../../src/cli/ui/ink-app/utils/routeInternalFetch"

describe("routeInternalFetch", () => {
  const calls: string[] = []
  const worker = (async (input: RequestInfo | URL) => {
    calls.push(`worker ${String(input instanceof Request ? input.url : input)}`)
    return new Response("w")
  }) as typeof fetch
  const network = (async (input: RequestInfo | URL) => {
    calls.push(`network ${String(input instanceof Request ? input.url : input)}`)
    return new Response("n")
  }) as typeof fetch

  test("only the in-process server goes through the worker", async () => {
    calls.length = 0
    const f = routeInternalFetch(worker, "http://gizzi.internal", network)
    await f("http://gizzi.internal/session")
    await f(new URL("http://gizzi.internal/event"))
    await f("https://openrouter.ai/api/v1/chat/completions", { method: "POST" })
    await f(new Request("http://localhost:8080/v1/models"))
    expect(calls).toEqual([
      "worker http://gizzi.internal/session",
      "worker http://gizzi.internal/event",
      "network https://openrouter.ai/api/v1/chat/completions",
      "network http://localhost:8080/v1/models",
    ])
  })

  test("without an internal URL the native fetch is kept", () => {
    expect(routeInternalFetch(worker, undefined, network)).toBe(network)
  })
})

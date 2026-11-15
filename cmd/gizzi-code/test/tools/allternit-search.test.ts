// WebSearch through Allternit's own route (cloud-api POST /api/v1/search/web).
// The network is the edge: a fake fetch stands in for the route, so no
// metered search runs in tests.
import { describe, expect, test } from "bun:test"
import { AllternitSearch } from "../../src/runtime/tools/builtins/allternit-search"
import { sourcesFromExaText } from "../../src/runtime/session/response-parts"

function fakeRoute(answers: Array<{ status: number; body?: unknown }>) {
  const calls: Array<{ url: string; auth: string; body: any }> = []
  const fetch = (async (url: string, init: RequestInit) => {
    calls.push({ url, auth: (init.headers as Record<string, string>).Authorization, body: JSON.parse(String(init.body)) })
    const a = answers.shift()!
    return new Response(a.body === undefined ? "" : JSON.stringify(a.body), { status: a.status })
  }) as unknown as typeof globalThis.fetch
  return { fetch, calls }
}

describe("AllternitSearch", () => {
  test("searches through the route with the account token and keeps sources parseable", async () => {
    const route = fakeRoute([
      { status: 200, body: { results: [{ title: "Bun 1.3", url: "https://bun.sh/blog/bun-v1.3", text: "release notes" }] } },
    ])
    const results = await AllternitSearch.search("bun release", { fetch: route.fetch, tokens: ["alt_key"] })
    expect(route.calls[0]!.url).toEndWith("/api/v1/search/web")
    expect(route.calls[0]!.auth).toBe("Bearer alt_key")
    expect(route.calls[0]!.body).toEqual({ query: "bun release", num_results: 8 })
    const text = AllternitSearch.toText(results!)
    expect(sourcesFromExaText(text).map(s => s.url)).toEqual(["https://bun.sh/blog/bun-v1.3"])
  })

  test("a rejected token tries the next one", async () => {
    const route = fakeRoute([{ status: 401 }, { status: 200, body: { results: [{ title: "A", url: "https://a.dev" }] } }])
    const results = await AllternitSearch.search("q", { fetch: route.fetch, tokens: ["stale", "device"] })
    expect(route.calls.map(c => c.auth)).toEqual(["Bearer stale", "Bearer device"])
    expect(results).toHaveLength(1)
  })

  test("not configured (503) or signed out gives undefined so callers fall back", async () => {
    const route = fakeRoute([{ status: 503, body: { error: "search_not_configured" } }])
    expect(await AllternitSearch.search("q", { fetch: route.fetch, tokens: ["t"] })).toBeUndefined()
    const none = fakeRoute([])
    expect(await AllternitSearch.search("q", { fetch: none.fetch, tokens: [] })).toBeUndefined()
    expect(none.calls).toHaveLength(0)
  })
})

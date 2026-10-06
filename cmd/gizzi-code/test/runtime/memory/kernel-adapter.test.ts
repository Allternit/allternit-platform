import { afterEach, describe, expect, test } from "bun:test"
import { MemoryKernelAdapter } from "../../../src/runtime/memory/kernel-adapter"

// The Memory Drive is canonical; the kernel is an index rebuilt from it after
// push. gizzi only reads canonical recall — no row mirror, no import marker.
const realFetch = globalThis.fetch
const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } })

afterEach(() => {
  globalThis.fetch = realFetch
  delete process.env.ALLTERNIT_API_TOKEN
  delete process.env.GIZZI_MEMORY_KERNEL
})

describe("memory kernel adapter (read side)", () => {
  test("search hits the canonical recall route", async () => {
    process.env.ALLTERNIT_API_TOKEN = "t"
    const calls: string[] = []
    globalThis.fetch = (async (url: string) => {
      calls.push(String(url))
      return json({ items: [{ id: "f1", type: "fact", text: "Prefers tabs", score: 1, source: "drive", external_id: "entry-1" }] })
    }) as typeof fetch
    const hits = await MemoryKernelAdapter.search("tabs")
    expect(hits[0]!.text).toBe("Prefers tabs")
    expect(calls[0]).toEndWith("/api/v1/memory/adapters/search")
    expect("upsert" in MemoryKernelAdapter).toBe(false)
    expect("importOnce" in MemoryKernelAdapter).toBe(false)
  })

  test("failures, empty queries and the off switch return no hits", async () => {
    process.env.ALLTERNIT_API_TOKEN = "t"
    globalThis.fetch = (async () => json({ error: "boom" }, 500)) as unknown as typeof fetch
    expect(await MemoryKernelAdapter.search("tabs")).toEqual([])
    expect(await MemoryKernelAdapter.search("  ")).toEqual([])
    process.env.GIZZI_MEMORY_KERNEL = "0"
    expect(await MemoryKernelAdapter.search("tabs")).toEqual([])
  })
})

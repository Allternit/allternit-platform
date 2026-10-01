import { afterEach, describe, expect, test } from "bun:test"
import { mkdtemp, readFile } from "fs/promises"
import { existsSync } from "fs"
import { tmpdir } from "os"
import path from "path"
import { IMPORT_MARKER, KERNEL_SOURCE, MemoryKernelAdapter } from "../../../src/runtime/memory/kernel-adapter"

const realFetch = globalThis.fetch
const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } })

afterEach(() => {
  globalThis.fetch = realFetch
  delete process.env.ALLTERNIT_API_TOKEN
  delete process.env.GIZZI_MEMORY_KERNEL
  MemoryKernelAdapter.resetForTests()
})

const entry = (dir: string, filename: string, type = "feedback") => ({
  name: filename.replace(".md", ""),
  description: "how Eoj wants answers",
  type,
  filename,
  filepath: path.join(dir, filename),
  body: "Short, direct, no hype.",
})

function capture(reply: (url: string, body: any) => Response = () => json({})) {
  const calls: { url: string; body: any }[] = []
  globalThis.fetch = (async (url: string, init: RequestInit = {}) => {
    const body = init.body ? JSON.parse(String(init.body)) : undefined
    calls.push({ url, body })
    return reply(url, body)
  }) as typeof fetch
  return calls
}

describe("memory kernel adapter", () => {
  test("maps memdir entries to canonical items", () => {
    const item = MemoryKernelAdapter.toItem(entry("/m/a", "user_tone.md"))
    expect(item.memory_type).toBe("preference")
    expect(item.text).toBe("user_tone: how Eoj wants answers\n\nShort, direct, no hype.")
    // Same filename in another memory dir is a different external id.
    expect(item.external_id).not.toBe(MemoryKernelAdapter.toItem(entry("/m/b", "user_tone.md")).external_id)
    expect(MemoryKernelAdapter.memoryType("project")).toBe("task_state")
    expect(MemoryKernelAdapter.memoryType("user")).toBe("fact")
  })

  test("save and delete go to the adapter routes", async () => {
    process.env.ALLTERNIT_API_TOKEN = "t"
    const calls = capture()
    const e = entry("/m/a", "x.md")
    expect(await MemoryKernelAdapter.upsert([e])).toBe(true)
    expect(await MemoryKernelAdapter.remove([e.filepath])).toBe(true)
    expect(calls[0]!.url).toEndWith("/api/v1/memory/adapters/upsert")
    expect(calls[0]!.body.source).toBe(KERNEL_SOURCE)
    expect(calls[0]!.body.items[0].external_id).toBe(MemoryKernelAdapter.externalId(e.filepath))
    expect(calls[1]!.url).toEndWith("/api/v1/memory/adapters/delete")
    expect(calls[1]!.body.external_ids).toEqual([MemoryKernelAdapter.externalId(e.filepath)])
  })

  test("failures never throw; disabled sends nothing", async () => {
    process.env.ALLTERNIT_API_TOKEN = "t"
    capture(() => json({ error: "boom" }, 500))
    expect(await MemoryKernelAdapter.upsert([entry("/m", "a.md")])).toBe(false)
    expect(await MemoryKernelAdapter.search("tone")).toEqual([])
    process.env.GIZZI_MEMORY_KERNEL = "0"
    const calls = capture()
    expect(await MemoryKernelAdapter.upsert([entry("/m", "a.md")])).toBe(true)
    expect(calls.length).toBe(0)
  })

  test("one-time import writes a marker and is skipped afterwards", async () => {
    process.env.ALLTERNIT_API_TOKEN = "t"
    const dir = await mkdtemp(path.join(tmpdir(), "memdir-"))
    const entries = Array.from({ length: 250 }, (_, i) => entry(dir, `m${i}.md`))
    const calls = capture()
    expect(await MemoryKernelAdapter.importOnce(dir, async () => entries)).toBe(true)
    expect(calls.length).toBe(2) // chunks of 200
    expect(existsSync(path.join(dir, IMPORT_MARKER))).toBe(true)
    expect((await readFile(path.join(dir, IMPORT_MARKER), "utf8")).length).toBeGreaterThan(0)
    MemoryKernelAdapter.resetForTests()
    expect(await MemoryKernelAdapter.importOnce(dir, async () => entries)).toBe(true)
    expect(calls.length).toBe(2)
  })

  test("a failed import retries later", async () => {
    process.env.ALLTERNIT_API_TOKEN = "t"
    const dir = await mkdtemp(path.join(tmpdir(), "memdir-"))
    capture(() => json({}, 503))
    expect(await MemoryKernelAdapter.importOnce(dir, async () => [entry(dir, "a.md")])).toBe(false)
    const calls = capture()
    expect(await MemoryKernelAdapter.importOnce(dir, async () => [entry(dir, "a.md")])).toBe(true)
    expect(calls.length).toBe(1)
  })

  test("search returns kernel hits", async () => {
    process.env.ALLTERNIT_API_TOKEN = "t"
    capture(() => json({ items: [{ id: "f1", type: "fact", text: "x", score: 1, source: KERNEL_SOURCE, external_id: "abc/x.md" }] }))
    const hits = await MemoryKernelAdapter.search("tone")
    expect(hits[0]!.external_id).toBe("abc/x.md")
  })
})

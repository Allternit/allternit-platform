import { describe, test, expect, beforeAll, afterAll } from "bun:test"
import { mkdtempSync, writeFileSync, rmSync } from "fs"
import { tmpdir } from "os"
import { join } from "path"
import { queryLocalModelWithStreaming } from "../../src/cli/ui/ink-app/services/api/localModel"

// An OpenRouter-style stream: reasoning deltas beside `content: ""`, then the
// reply. Reasoning must stream as a thinking block and never reach the text.
const SSE = [
  { choices: [{ index: 0, delta: { role: "assistant", content: "", reasoning: "Count the " } }] },
  { choices: [{ index: 0, delta: { content: "", reasoning: "primes." } }] },
  { choices: [{ index: 0, delta: { content: null, reasoning_content: " Done." } }] },
  { choices: [{ index: 0, delta: { content: "62" } }] },
  { choices: [], usage: { prompt_tokens: 10, completion_tokens: 5 } },
]
  .map((c) => `data: ${JSON.stringify(c)}\n\n`)
  .join("") + "data: [DONE]\n\n"

describe("local provider reasoning", () => {
  let dir: string
  const prevDir = process.env.GIZZI_CONFIG_DIR
  const prevFetch = globalThis.fetch

  beforeAll(() => {
    dir = mkdtempSync(join(tmpdir(), "gizzi-reasoning-"))
    writeFileSync(join(dir, "gizzi.json"), JSON.stringify({ provider: { testp: { options: { baseURL: "http://reasoning.test/v1" } } } }))
    process.env.GIZZI_CONFIG_DIR = dir
    globalThis.fetch = (async (input: RequestInfo | URL) => {
      const url = String(input instanceof Request ? input.url : input)
      if (url.endsWith("/models")) return Response.json({ data: [{ id: "m" }] })
      return new Response(SSE, { headers: { "content-type": "text/event-stream" } })
    }) as typeof fetch
  })

  afterAll(() => {
    globalThis.fetch = prevFetch
    if (prevDir === undefined) delete process.env.GIZZI_CONFIG_DIR
    else process.env.GIZZI_CONFIG_DIR = prevDir
    rmSync(dir, { recursive: true, force: true })
  })

  test("reasoning streams as thinking, the reply as text", async () => {
    const out: any[] = []
    for await (const ev of queryLocalModelWithStreaming({
      messages: [],
      systemPrompt: ["sys"] as any,
      tools: [],
      signal: new AbortController().signal,
      options: { model: "testp/m" } as any,
    })) out.push(ev)

    const events = out.filter((e) => e.type === "stream_event").map((e) => e.event)
    const starts = events.filter((e) => e.type === "content_block_start").map((e) => e.content_block.type)
    expect(starts).toEqual(["thinking", "text"])

    const thinking = events
      .filter((e) => e.type === "content_block_delta" && e.delta.type === "thinking_delta")
      .map((e) => e.delta.thinking)
      .join("")
    expect(thinking).toBe("Count the primes. Done.")

    // The thinking block closes before the text block opens.
    const thinkingIndex = events.find((e) => e.type === "content_block_start" && e.content_block.type === "thinking").index
    const stopAt = events.findIndex((e) => e.type === "content_block_stop" && e.index === thinkingIndex)
    const textAt = events.findIndex((e) => e.type === "content_block_start" && e.content_block.type === "text")
    expect(stopAt).toBeGreaterThan(-1)
    expect(stopAt).toBeLessThan(textAt)

    const final = out.find((e) => e.type === "assistant")
    expect(final.message.content).toEqual([{ type: "text", text: "62" }])
  })
})

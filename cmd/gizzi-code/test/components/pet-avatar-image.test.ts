import { afterEach, describe, expect, test } from "bun:test"
import sharp from "sharp"

const { loadAvatarPng, clearAvatarCacheForTest } = await import("../../src/cli/ui/ink-app/pet/avatarImage")

const realFetch = globalThis.fetch
afterEach(() => {
  globalThis.fetch = realFetch
  clearAvatarCacheForTest()
})

async function png(width: number, height: number): Promise<Buffer> {
  return sharp({ create: { width, height, channels: 4, background: { r: 217, g: 119, b: 87, alpha: 1 } } }).png().toBuffer()
}

describe("pet image avatars", () => {
  test("a data: URL becomes a 144x120 PNG, cropped rather than stretched", async () => {
    const url = `data:image/png;base64,${(await png(400, 400)).toString("base64")}`
    const out = await loadAvatarPng(url)
    expect(out).not.toBeNull()
    const meta = await sharp(Buffer.from(out!, "base64")).metadata()
    expect([meta.format, meta.width, meta.height]).toEqual(["png", 144, 120])
  })

  test("fetches http images once per URL, without the Allternit token on other hosts", async () => {
    const body = await png(64, 64)
    const calls: Array<{ url: string; auth?: string }> = []
    globalThis.fetch = (async (input: string | URL, init?: RequestInit) => {
      calls.push({ url: String(input), auth: (init?.headers as Record<string, string>)?.Authorization })
      return new Response(new Uint8Array(body), { headers: { "content-type": "image/png" } })
    }) as unknown as typeof fetch
    const url = "https://images.example.com/bot.png"
    const [a, b] = await Promise.all([loadAvatarPng(url), loadAvatarPng(url)])
    expect(a).not.toBeNull()
    expect(b).toBe(a)
    expect(calls).toEqual([{ url, auth: undefined }])
  })

  test("null when the image can't be loaded", async () => {
    globalThis.fetch = (async () => new Response("nope", { status: 404 })) as unknown as typeof fetch
    expect(await loadAvatarPng("https://images.example.com/missing.png")).toBeNull()
    globalThis.fetch = (async () => new Response("not an image")) as unknown as typeof fetch
    expect(await loadAvatarPng("https://images.example.com/garbage.png")).toBeNull()
    expect(await loadAvatarPng("data:image/png;base64,")).toBeNull()
  })
})

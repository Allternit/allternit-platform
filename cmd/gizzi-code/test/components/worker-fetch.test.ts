import { describe, expect, test } from "bun:test"

const { createWorkerFetch, WORKER_ORIGIN } = await import("../../src/cli/ui/ink-app/thread")

describe("TUI worker fetch", () => {
  test("only the in-process server goes over RPC; other hosts keep binary bodies intact", async () => {
    const rpcCalls: string[] = []
    const client = {
      call: (async (_method: string, args: { url: string }) => {
        rpcCalls.push(args.url)
        return { status: 200, headers: { "content-type": "application/json" }, body: '{"ok":true}' }
      }) as never,
    }
    const png = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0x00])
    const directCalls: string[] = []
    const direct = (async (input: Request) => {
      directCalls.push(input.url)
      return new Response(png, { headers: { "content-type": "image/png" } })
    }) as unknown as typeof fetch
    const f = createWorkerFetch(client, direct)

    expect(await (await f(`${WORKER_ORIGIN}/session`)).json()).toEqual({ ok: true })
    const bytes = new Uint8Array(await (await f("https://images.example.com/bot.png")).arrayBuffer())
    expect([...bytes]).toEqual([...png])
    expect(rpcCalls).toEqual([`${WORKER_ORIGIN}/session`])
    expect(directCalls).toEqual(["https://images.example.com/bot.png"])
  })
})

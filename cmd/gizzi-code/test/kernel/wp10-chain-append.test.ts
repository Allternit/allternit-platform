import { afterEach, describe, expect, test } from "bun:test"
import { KernelTurn } from "../../src/runtime/kernel/compilers/turn-hook"
import type { ToolReceiptV1 } from "../../../../spec/Contracts/kernel/v1/generated/ts/kernel-abi"

const oldFlag = process.env.GIZZI_KERNEL_COMPILERS
const oldURL = process.env.ALLTERNIT_FACTORY_URL
let server: ReturnType<typeof Bun.serve> | undefined

afterEach(() => {
  server?.stop(true)
  server = undefined
  KernelTurn.reset()
  if (oldFlag === undefined) delete process.env.GIZZI_KERNEL_COMPILERS
  else process.env.GIZZI_KERNEL_COMPILERS = oldFlag
  if (oldURL === undefined) delete process.env.ALLTERNIT_FACTORY_URL
  else process.env.ALLTERNIT_FACTORY_URL = oldURL
})

function listen(handler: (req: Request) => Promise<Response>) {
  server = Bun.serve({ port: 0, hostname: "127.0.0.1", fetch: handler })
  process.env.ALLTERNIT_FACTORY_URL = server.url.toString().replace(/\/$/, "")
}

describe("WP10 external chain append", () => {
  test("flag off executes unchanged and makes no request", async () => {
    let calls = 0
    listen(async () => { calls++; return new Response() })
    delete process.env.GIZZI_KERNEL_COMPILERS
    const result = { output: "same" }
    expect(await KernelTurn.withToolReceipt({ sessionID: "s", tool: "read", args: {} }, async () => result)).toBe(result)
    expect(calls).toBe(0)
    expect(KernelTurn.record("s")).toBeUndefined()
  })

  test("flag on sends canonical receipts and records chain acknowledgement", async () => {
    process.env.GIZZI_KERNEL_COMPILERS = "1"
    const bodies: ToolReceiptV1[] = []
    listen(async (req) => {
      expect(req.method).toBe("POST")
      const receipt = await req.json() as ToolReceiptV1
      expect(new URL(req.url).pathname).toBe(`/v1/receipts/chain/${receipt.envelope.run_id}`)
      expect(receipt.envelope.schema_id).toBe("allternit.kernel.ToolReceiptV1")
      expect(receipt.extensions?.["x-chain_append"]).toBe("pending")
      bodies.push(receipt)
      return Response.json({ chain: { receipt_id: `rcpt_${bodies.length}`, run_id: receipt.envelope.run_id } })
    })
    KernelTurn.noteGateDecision("call1", { permission: "edit", pattern: "src/a.ts", action: "allow", source: "policy" })
    const result = { output: "done" }
    expect(await KernelTurn.withToolReceipt({ sessionID: "ses_1", callID: "call1", tool: "edit", args: { filePath: "src/a.ts" } }, async () => result)).toBe(result)
    const originalError = new Error("tool failed")
    await expect(KernelTurn.withToolReceipt({ sessionID: "ses_1", tool: "read", args: {} }, async () => { throw originalError })).rejects.toBe(originalError)
    expect(bodies.map((r) => r.exit_class)).toEqual(["SUCCESS", "FAILURE"])
    const record = KernelTurn.record("ses_1")!
    expect(record.errors).toEqual([])
    expect(record.receipts.map((r) => r.extensions?.["x-chain_append"])).toEqual(["appended", "appended"])
    expect(record.receipts[0].extensions?.["x-chain_receipt_id"]).toBe("rcpt_1")
  })

  test("append rejection logs failure and preserves original success and error", async () => {
    process.env.GIZZI_KERNEL_COMPILERS = "1"
    let calls = 0
    listen(async () => ++calls === 1 ? new Response("invalid", { status: 400 }) : Response.json({ chain: { receipt_id: "r", run_id: "wrong" } }))
    expect(await KernelTurn.withToolReceipt({ sessionID: "s", tool: "read", args: {} }, async () => 42)).toBe(42)
    const originalError = new Error("original")
    await expect(KernelTurn.withToolReceipt({ sessionID: "s", tool: "read", args: {} }, async () => { throw originalError })).rejects.toBe(originalError)
    expect(KernelTurn.record("s")!.errors).toEqual(["chain_append:HTTP 400", "chain_append:invalid chain acknowledgement"])
    expect(KernelTurn.record("s")!.receipts.every((r) => r.extensions?.["x-chain_append"] === "failed")).toBe(true)
  })

  test("unavailable service never breaks the turn", async () => {
    process.env.GIZZI_KERNEL_COMPILERS = "1"
    listen(async () => new Response())
    server!.stop(true)
    expect(await KernelTurn.withToolReceipt({ sessionID: "s", tool: "read", args: {} }, async () => "ok")).toBe("ok")
    expect(KernelTurn.record("s")!.errors[0]).toStartWith("chain_append:")
    expect(KernelTurn.record("s")!.receipts[0].extensions?.["x-chain_append"]).toBe("failed")
  })

  test("engine URL: ALLTERNIT_FACTORY_URL wins, old shell var still honoured, default is local engine", () => {
    expect(KernelTurn.engineUrl({ ALLTERNIT_FACTORY_URL: "http://f:1/" })).toBe("http://f:1")
    expect(KernelTurn.engineUrl({ GIZZI_RAILS_URL: "http://old:2" })).toBe("http://old:2") // old-names: keep (tests the deprecated fallback)
    expect(KernelTurn.engineUrl({})).toBe("http://127.0.0.1:3011")
  })
})

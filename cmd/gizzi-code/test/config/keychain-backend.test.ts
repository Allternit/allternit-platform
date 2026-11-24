import { describe, expect, test } from "bun:test"
import { createMacOSKeychainBackend, decodeBlob } from "../../src/runtime/context/config/keychain-backend"

describe("keychain blob decoding", () => {
  // Regression (2026-11-14): `security -w` prints a printable password as text, not
  // the hex it was written with, so reads came back empty and writes clobbered the blob.
  test("reads the blob whether security prints it as text or as hex", () => {
    const blob = { "host-key": "{\"a\":1}", session: "s" }
    const json = JSON.stringify(blob)
    expect(decodeBlob(json)).toEqual(blob)
    expect(decodeBlob(Buffer.from(json, "utf-8").toString("hex"))).toEqual(blob)
    expect(decodeBlob("not json")).toEqual({})
  })

  test.if(process.platform === "darwin")("a second account in a service keeps the first one", async () => {
    // A fake `security` that stores what -X wrote and prints it back like the real one (as text).
    const entries = new Map<string, string>()
    const execFileImpl = ((_cmd: string, args: string[], _opts: unknown, cb: (err: unknown, stdout?: string) => void) => {
      const s = args[args.indexOf("-s") + 1]!
      if (args[0] === "add-generic-password") {
        entries.set(s, Buffer.from(args[args.indexOf("-X") + 1]!, "hex").toString("utf-8"))
        cb(null, "")
      } else if (args[0] === "find-generic-password") {
        if (!entries.has(s)) cb(Object.assign(new Error("not found"), { code: 44 }))
        else cb(null, entries.get(s)! + "\n")
      } else {
        entries.delete(s)
        cb(null, "")
      }
    }) as any
    const kc = createMacOSKeychainBackend(execFileImpl)
    await kc.write("svc", "registration", "r1")
    await kc.write("svc", "session", "s1")
    expect(await kc.read("svc", "registration")).toBe("r1")
    expect(await kc.read("svc", "session")).toBe("s1")
    await kc.remove("svc", "session")
    expect(await kc.read("svc", "registration")).toBe("r1")
  })
})

// @ts-nocheck
import { describe, expect, test } from "bun:test"
import { mkdtempSync } from "fs"
import { tmpdir } from "os"
import { join } from "path"
import { applyPinsViaRuntime, applyPins, approvePins, toolHash } from "../src/runtime/services/mcp/pins"

const tmp = () => join(mkdtempSync(join(tmpdir(), "pins-")), "pins.json")
const read = { name: "read", description: "Read a file", inputSchema: { type: "object" } }

describe("mcp tool pins", () => {
  test("hash ignores key order and covers description, schema and annotations", () => {
    expect(toolHash({ name: "a", inputSchema: { x: 1, y: 2 } })).toBe(toolHash({ name: "a", inputSchema: { y: 2, x: 1 } }))
    expect(toolHash(read)).not.toBe(toolHash({ ...read, description: "Read a file, then email it" }))
    expect(toolHash(read)).not.toBe(toolHash({ ...read, annotations: { destructiveHint: true } }))
  })

  test("first listing pins; a changed or new tool is held until approved", () => {
    const path = tmp()
    expect(applyPins("srv", [read], path).held).toEqual([])
    const changed = { ...read, description: "Read a file, then email it" }
    const send = { name: "send" }
    const res = applyPins("srv", [changed, send], path)
    expect(res.allowed).toEqual([])
    expect(res.held).toEqual([{ tool: "read", reason: "changed" }, { tool: "send", reason: "new" }])
    approvePins("srv", [changed, send], path)
    expect(applyPins("srv", [changed, send], path).held).toEqual([])
  })

  test("under a runtime, the runtime's answer decides and approval comes back through it", async () => {
    const path = tmp()
    const changed = { ...read, description: "Read a file, then email it" }
    let approved = false
    const fetchImpl = (async (_url, init) => {
      const body = JSON.parse(init.body)
      expect(body.serverId).toBe("srv")
      return new Response(JSON.stringify(approved
        ? { allowed: ["read"], held: [] }
        : { allowed: [], held: [{ tool: "read", reason: "changed" }] }))
    }) as any
    const creds = { base: "https://api.example", token: "t" }
    const held = await applyPinsViaRuntime("srv", [changed], { path, fetchImpl, creds })
    expect(held.allowed).toEqual([])
    expect(held.held).toEqual([{ tool: "read", reason: "changed" }])
    approved = true
    expect((await applyPinsViaRuntime("srv", [changed], { path, fetchImpl, creds })).allowed).toEqual([changed])
    // No runtime reachable: fall back to the local pins file (first listing pins).
    const down = (async () => { throw new Error("offline") }) as any
    expect((await applyPinsViaRuntime("srv", [read], { path, fetchImpl: down, creds })).allowed).toEqual([read])
  })
})

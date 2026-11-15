import { describe, test, expect } from "bun:test"
import fs from "fs"
import { parseServiceStatus, exitCodeFor, renderServiceTable, fetchServiceStatus, summarizeServices } from "../../src/cli/utils/serviceStatus"

const comp = (id: string, status: string) => ({ id, name: id.toUpperCase(), status, uptime_90d: 1, last_change: null })
const doc = (overall: string, components: any[], incidents: any[] = []) => ({
  schema: "allternit.status/v1",
  last_checked_at: "2026-11-12T08:00:00Z",
  overall,
  components,
  incidents,
})

describe("service status", () => {
  test("exit codes: 0 operational, 1 degraded/maintenance, 2 down/unknown", () => {
    expect(exitCodeFor(parseServiceStatus(doc("operational", [comp("a", "operational")])))).toBe(0)
    expect(exitCodeFor(parseServiceStatus(doc("degraded", [comp("a", "degraded")])))).toBe(1)
    expect(exitCodeFor(parseServiceStatus(doc("operational", [comp("a", "maintenance")])))).toBe(1)
    expect(exitCodeFor(parseServiceStatus(doc("down", [comp("a", "down"), comp("b", "degraded")])))).toBe(2)
    expect(exitCodeFor(parseServiceStatus(doc("operational", [comp("a", "weird")])))).toBe(2)
  })

  test("table lists open incident titles next to their component and skips resolved ones", () => {
    const s = parseServiceStatus(
      doc("degraded", [comp("api", "degraded"), comp("docs", "operational")], [
        { id: "1", title: "Elevated errors", state: "open", impact: "degraded", component_ids: ["api"] },
        { id: "2", title: "Old", state: "resolved", impact: "down", component_ids: ["docs"] },
      ]),
    )
    const lines = renderServiceTable(s).split("\n")
    expect(lines[1]).toContain("▲ degraded")
    expect(lines[1]).toContain("Elevated errors")
    expect(lines[2]).not.toContain("Old")
    expect(lines.at(-1)).toContain("2026-11-12T08:00:00Z")
    expect(summarizeServices(s)).toBe("API degraded")
  })

  test("rejects documents that are not the status contract", () => {
    expect(() => parseServiceStatus({ hello: 1 })).toThrow()
    expect(() => parseServiceStatus(null)).toThrow()
  })

  test("parses the contract fixtures", () => {
    const p = `${process.env.HOME}/Desktop/allternit-workspace/allternit-feat-status-worker/services/status/contract/fixtures.json`
    if (!fs.existsSync(p)) return
    const fx = JSON.parse(fs.readFileSync(p, "utf8"))
    const codes = Object.keys(fx).map((k) => exitCodeFor(parseServiceStatus(fx[k].status)))
    expect(codes).toEqual([0, 1, 2])
  })

  test("fetch failure and HTTP errors give one-line errors", async () => {
    const bad = (async () => new Response("x", { status: 503 })) as unknown as typeof fetch
    await expect(fetchServiceStatus({ url: "http://x", fetchImpl: bad })).rejects.toThrow("HTTP 503")
    const boom = (async () => {
      throw new Error("ECONNREFUSED")
    }) as unknown as typeof fetch
    await expect(fetchServiceStatus({ url: "http://x", fetchImpl: boom })).rejects.toThrow("ECONNREFUSED")
  })
})

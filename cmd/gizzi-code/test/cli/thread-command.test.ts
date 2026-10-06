import { describe, expect, test } from "bun:test"
import { formatThreadRows } from "../../src/cli/commands/thread"

const t = (id: string, title: string, status: string, mins: number, incognito = false) => ({
  id, botId: "b", projectId: null, kind: "task" as const, incognito, title, status: status as never,
  currentSessionId: "s", generation: 1, contextUsed: null,
  lastActivityAt: new Date(Date.now() - mins * 60_000).toISOString(), createdAt: "",
})

describe("gizzi agents bot threads list", () => {
  test("groups like the Threads panel and hides incognito asks", () => {
    const rows = formatThreadRows([
      t("aaaaaaaa11", "Monthly close", "done", 60 * 30),
      t("bbbbbbbb22", "GPU unit economics", "needs_you", 1),
      t("cccccccc33", "Research cloud pricing", "working", 5),
      t("dddddddd44", "Secret ask", "working", 1, true),
    ])
    expect(rows.map((r) => r.split(/\s{2,}/)[1])).toEqual(["waiting", "working", "resolved"])
    expect(rows[0]).toContain("GPU unit economics")
    expect(rows.join("\n")).not.toContain("Secret ask")
    expect(rows[1].endsWith("5m")).toBe(true)
  })
})

import { describe, expect, test } from "bun:test"
import type { ConsoleRow, HostSnapshot } from "@/cli/work/console"
import { attentionReasons, ATTENTION_GROUP, boundLogs, clampLines, flatRows, groupRows, MAX_LINE_CHARS, MAX_LOG_LINES, meter, meterLine } from "@/cli/work/console-view"

const GB = 1024 ** 3
const sample = (cpu: number, memUsed: number, diskUsed: number): HostSnapshot => ({
  sampledAt: "2026-11-20T00:00:00Z",
  uptimeSec: 100,
  os: { name: "Ubuntu", version: "24.04", family: "linux", arch: "x64" },
  cpu: { percent: cpu, cores: 4 },
  memory: { usedBytes: memUsed * GB, totalBytes: 10 * GB },
  disk: [{ mount: "/", usedBytes: diskUsed * GB, totalBytes: 100 * GB }],
})
const row = (id: string, o: Partial<ConsoleRow> = {}): ConsoleRow => ({ id, kind: "cloud", name: id, status: "running", ...o })

describe("console view", () => {
  test("needs attention: full disk, error status; asleep is never flagged from its sample", () => {
    expect(attentionReasons(row("a", { lastSample: sample(10, 2, 95) }))).toEqual(["disk / 95% full"])
    expect(attentionReasons(row("b", { status: "error" }))).toEqual(["status error"])
    expect(attentionReasons(row("c", { status: "asleep", lastSample: sample(99, 9.9, 99) }))).toEqual([])
    expect(attentionReasons(row("d", { lastSample: sample(10, 2, 10) }))).toEqual([])
    expect(attentionReasons(row("e", { status: "offline" }))).toEqual(["status offline"])
    expect(attentionReasons(row("f", { lastSample: sample(90, 2, 10) }))).toEqual(["cpu 90%"])
    expect(attentionReasons(row("g", { lastSample: sample(89, 9, 10) }))).toEqual(["memory 90% used"])
    expect(attentionReasons(row("h", { status: "degraded", lastSample: sample(89, 8, 89) }))).toEqual([])
  })

  test("groups: attention first, then named groups, kind as fallback", () => {
    const g = groupRows([row("a", { group: "Work" }), row("b", { lastSample: sample(10, 2, 95) }), row("c", { kind: "paired" }), row("d", { group: "Work" })])
    expect(g.map((x) => x.name)).toEqual([ATTENTION_GROUP, "Paired", "Work"])
    expect(flatRows(g).map((r) => r.id)).toEqual(["b", "c", "a", "d"])
  })

  test("meters clamp", () => {
    expect(meter(150, 4)).toBe("████")
    expect(meter(-5, 4)).toBe("░░░░")
    expect(meterLine(undefined)).toBe("no sample yet")
    expect(meterLine(sample(50, 5, 20))).toContain("cpu ███░░░ 50%")
  })

  test("log bounds", () => {
    expect(clampLines(undefined)).toBe(50)
    expect(clampLines(0)).toBe(1)
    expect(clampLines(5000)).toBe(MAX_LOG_LINES)
    const lines = Array.from({ length: 300 }, (_, i) => ({ ts: String(i), unit: "u", level: "info", message: "x".repeat(i === 299 ? 1000 : 1) }))
    const out = boundLogs(lines, 3)
    expect(out.map((l) => l.ts)).toEqual(["297", "298", "299"])
    expect(out[2].message.length).toBe(MAX_LINE_CHARS + 1)
  })
})

import { describe, expect, test } from "bun:test"
import { bytes, consoleEnabled, formatPorts, formatServices, formatStatus, formatUpdates, uptime } from "../../src/cli/work/console"

const snap = {
  sampledAt: "2026-11-20T10:00:00Z",
  uptimeSec: 90061,
  os: { name: "Ubuntu", version: "24.04", family: "linux", arch: "x86_64" },
  cpu: { percent: 12.4, cores: 4 },
  memory: { usedBytes: 2 * 1024 ** 3, totalBytes: 8 * 1024 ** 3 },
  disk: [{ mount: "/", usedBytes: 10 * 1024 ** 3, totalBytes: 40 * 1024 ** 3 }],
}

describe("computers console cli", () => {
  test("flag reads GIZZI_COMPUTERS_CONSOLE", () => {
    expect(consoleEnabled({})).toBe(false)
    expect(consoleEnabled({ GIZZI_COMPUTERS_CONSOLE: "1" })).toBe(true)
    expect(consoleEnabled({ GIZZI_COMPUTERS_CONSOLE: "off" })).toBe(false)
  })
  test("bytes and uptime", () => {
    expect(bytes(1536)).toBe("1.5 KB")
    expect(uptime(90061)).toBe("1d 1h")
  })
  test("status shows asleep with last sample", () => {
    const out = formatStatus({ id: "c1", kind: "cloud", name: "box" }, snap, true)
    expect(out).toContain("asleep")
    expect(out).toContain("12%")
  })
  test("status with no sample says so", () => {
    expect(formatStatus({ id: "c1", kind: "paired", name: "mac" }, undefined)).toContain("No health sample yet")
  })
  test("empty tables print plain lines", () => {
    expect(formatServices([])).toBe("No services found.")
    expect(formatPorts([])).toBe("No listening ports found.")
    expect(formatUpdates([])).toBe("Up to date.")
  })
})

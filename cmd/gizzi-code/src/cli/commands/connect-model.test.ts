import { describe, expect, test } from "bun:test"
import { brainSub, findEntry, mergeRows, renderHub, resolve, statusText } from "./connect-model"

describe("mergeRows", () => {
  test("a runtime row wins only when it is on", () => {
    const cloud = [{ id: "chatgpt", state: "off" as const, source: "cloud" }]
    expect(mergeRows(cloud, [{ id: "chatgpt", state: "on", source: "runtime:a" }])[0].source).toBe("runtime:a")
    expect(mergeRows(cloud, [{ id: "chatgpt", state: "needs", source: "runtime:a" }])[0].source).toBe("cloud")
    expect(mergeRows([], [{ id: "codex", state: "needs" }])).toHaveLength(1)
  })
})

describe("status words", () => {
  const entry = findEntry("slack")!
  test("uses only the four words", () => {
    expect(statusText(entry, { id: "slack", state: "off" })).toBe("Not connected")
    expect(statusText(entry, { id: "slack", state: "connecting" })).toBe("Connecting…")
    expect(statusText(entry, { id: "slack", state: "on", found: "Acme" })).toBe("Connected · Acme")
    expect(statusText(entry, { id: "slack", state: "needs", reason_code: "login_expired" })).toBe("Needs you · Sign-in expired")
  })
})

describe("hub", () => {
  test("splits brains in three and ignores unknown ids", () => {
    const rows = [
      { id: "anthropic-key", state: "on" as const, found: "Key works", runs_on: "server" as const },
      { id: "chatgpt", state: "off" as const, available: false, reason_code: "no_computer", runs_on: "computer" as const, unavailable_reason: "Needs your own computer for now. Or use an OpenAI key.", fallbacks: ["this-mac", "openai-key", "x"] },
      { id: "claude", available: false, terms: "pending" as const, reason_code: "not_available_yet", unavailable_reason: "Not available yet. Use a key for now.", fallbacks: ["anthropic-key"] },
      { id: "mystery", state: "on" as const },
    ]
    const x = resolve(rows)
    expect(brainSub(x.find((i) => i.entry.id === "claude")!.entry, rows[2])).toBe("Coming soon")
    expect(brainSub(x.find((i) => i.entry.id === "chatgpt")!.entry, rows[1])).toBe("Needs your computer")
    const text = renderHub(rows).join("\n")
    expect(text).not.toContain("mystery")
    expect(text).toContain(" Works on any device")
    expect(text).toContain("Connected · Key works")
    expect(text).toContain("Try: gizzi connect this-mac  or  gizzi connect openai-key")
    expect(text).not.toContain("gizzi connect x")
    expect(text.indexOf("Needs your computer")).toBeLessThan(text.indexOf("Coming soon"))
  })
})

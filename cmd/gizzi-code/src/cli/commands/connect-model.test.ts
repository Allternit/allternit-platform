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
    expect(text).not.toMatch(/gizzi connect x(\s|$)/)
    expect(text.indexOf("Needs your computer")).toBeLessThan(text.indexOf("Coming soon"))
  })
})

describe("rows the server did not send", () => {
  test("claude, grok, kimi and copilot use the contract defaults and group as Coming soon", () => {
    const text = renderHub([]).join("\n")
    const soon = text.slice(text.indexOf(" Coming soon"), text.indexOf("Computers"))
    for (const n of ["Claude", "Grok", "Kimi", "GitHub Copilot"]) expect(soon).toContain(n)
    expect(soon).toContain("Not available yet. Use a key for now.  Try: gizzi connect xai-key")
    expect(soon).toContain("Coming soon.  Try: gizzi connect github")
    expect(text.slice(text.indexOf(" Works on any device"), text.indexOf(" Coming soon"))).not.toContain("Grok")
  })
})

describe("runner rows (caps=runner, §9.1)", () => {
  test("an offered runner row is connectable with terms still pending, and says it needs no computer", () => {
    const rows = [{ id: "claude", state: "off" as const, runs_on: "runner" as const, available: true, terms: "pending" as const, person_only: true, disclosure: { version: "2026-11-14.1", accepted_at: null } }]
    const x = resolve(rows).find((i) => i.entry.id === "claude")!
    expect(brainSub(x.entry, x.row)).toBe("Works on any device")
    const text = renderHub(rows).join("\n")
    expect(text).toContain("Claude                        Not connected · Pro or Max plan · Runs on Allternit · no computer needed")
  })

  test("ChatGPT signed in on this device wins over the cloud runner row and the runtime row", () => {
    const cloud = [{ id: "chatgpt", state: "off" as const, runs_on: "runner" as const, source: "cloud" }]
    const runtime = [{ id: "chatgpt", state: "on" as const, source: "runtime:a" }]
    const device = [{ id: "chatgpt", state: "on" as const, source: "device" }]
    expect(mergeRows(cloud, runtime, device)[0].source).toBe("device")
    expect(mergeRows(cloud, runtime, [{ id: "chatgpt", state: "off" as const, source: "device" }])[0].source).toBe("runtime:a")
  })
})

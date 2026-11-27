import { describe, expect, test } from "bun:test"
import { formatProviderLines, jsonRows } from "./models-rows"

const row = (id: string, extra: object = {}) => ({
  ref: `p/${id}`, providerID: "p", modelID: id, name: id, default: false, current: false, ...extra,
})

describe("formatProviderLines", () => {
  test("locked row shows its reason and the access badge", () => {
    const lines = formatProviderLines("cloud", {
      id: "p", name: "Allternit Cloud",
      models: [row("a", { locked: { reason: "No credits left.", action: "add_credits" } })],
    })
    expect(lines).toHaveLength(1)
    expect(lines[0].access).toBe("credits")
    expect(lines[0].text).toContain("credits")
    expect(lines[0].text).toContain("No credits left.")
  })
  test("hidden rows are omitted", () => {
    const lines = formatProviderLines("api", { id: "p", name: "P", models: [row("a"), row("b", { hidden: true })] })
    expect(lines.map((l) => l.text)).toEqual(["p/a  key"])
  })
  test("offline provider is one line with the fix", () => {
    const lines = formatProviderLines("local", {
      id: "ollama", name: "Ollama — offline", models: [], offline: { reason: "Nothing answered", fix: "Start it or remove it from config" },
    })
    expect(lines).toEqual([{ type: "offline", text: "Ollama — offline · Start it or remove it from config" }])
  })
  test("json rows carry id, section, access, locked, offline", () => {
    const out = jsonRows([{ kind: "subscription", title: "x", providers: [{ id: "p", name: "P", models: [row("a")] }] }] as any)
    expect(out).toEqual([{ id: "p/a", section: "subscription", access: "plan", locked: null, offline: null }])
  })
})

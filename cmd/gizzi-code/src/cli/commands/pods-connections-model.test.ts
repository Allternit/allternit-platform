import { describe, expect, test } from "bun:test"
import { NO_PODS, PODS_SIGNED_OUT, podDetail, podError, podRow, podSummary } from "@/cli/work/pods-model"
import { CAP, canMore, deckLines, footer, gridLines, listLines, mergeItems, nextMode } from "@/cli/work/connections-model"

const state = {
  mainBotId: "b1",
  pods: [{ id: "chief", name: "Chief", kind: "chief", botIds: ["b1"] }, { id: "p1", name: "Launch", kind: "project", botIds: ["b2", "b3"] }],
  bots: [{ id: "b1", status: "idle" }, { id: "b2", status: "working" }, { id: "b3", status: "waiting", needs: { question: 2 } }],
}
describe("pods", () => {
  test("empty and signed out", () => {
    expect(NO_PODS).toContain("No pods")
    expect(podError(new Error("Allternit API 401"))).toBe(PODS_SIGNED_OUT)
  })
  test("filled", () => {
    expect(podSummary(state, state.pods[1]!)).toBe("2 bots · 1 working · 2 need you")
    expect(podRow(state, state.pods[0]!)).toContain("chief")
    expect(podDetail(state, state.pods[0]!)[0]).toContain("chief of staff")
  })
})

const entries = [
  { id: "slack", group: "apps" as const, name: "Slack", blurb: "Chat" },
  { id: "claude", group: "brains" as const, name: "Claude", available: false, unavailable_reason: "Runs on a computer." },
]
const items = mergeItems(entries, [{ id: "slack", state: "on" }])
describe("connections modes", () => {
  test("empty", () => {
    expect(listLines([], 0)).toEqual([])
    expect(deckLines([], 0)).toEqual([])
    expect(gridLines([], 0)).toEqual([])
  })
  test("list", () => {
    const l = listLines(items, 0)
    expect(l[0]).toContain("❯ ● Slack")
    expect(l[0]).toContain("Connected")
    expect(l[1]).toContain("Runs on a computer.")
  })
  test("grid", () => {
    const g = gridLines(items, 1, 80).join("\n")
    expect(g).toContain("Slack")
    expect(g).toContain("❯○ Claude")
  })
  test("deck", () => {
    const d = deckLines(items, 0).join("\n")
    expect(d).toContain("1 of 2")
    expect(d).toContain("Chat")
  })
  test("mode, more cap, footer", () => {
    expect(nextMode("deck")).toBe("list")
    expect(nextMode("list", -1)).toBe("deck")
    expect(canMore(24, "c")).toBe(true)
    expect(canMore(CAP, "c")).toBe(false)
    expect(canMore(10, null)).toBe(false)
    expect(footer("deck", true)).toContain("←/→")
    expect(footer("list", true)).toContain("m more")
  })
})

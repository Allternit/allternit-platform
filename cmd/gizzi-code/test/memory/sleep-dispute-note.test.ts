import { beforeEach, describe, expect, test } from "bun:test"
import { askDisputes, relatedLines, resetAskedDisputesForTests, type DisputeNote } from "../../src/runtime/memory/sleep/dispute-note"

const base = {
  side_a: { id: "f1", text: "Deploy on Fridays", current: true },
  side_b: { id: "f2", text: "Never deploy on Fridays", current: true },
}
const resolved: DisputeNote = { ...base, id: "d1", state: "resolved", pick: "b", reason: "newer and said twice" }
const needs: DisputeNote = { ...base, id: "d2", state: "needs_user", pick: null }

beforeEach(() => resetAskedDisputesForTests())

describe("memory-recall disputes", () => {
  test("recall output for a disputed item shows both sides and the pick", () => {
    const { lines } = relatedLines([{ text: "Deploy on Fridays", dispute: resolved }])
    const out = lines.join("\n")
    expect(out).toContain('A: "Deploy on Fridays"')
    expect(out).toContain('B: "Never deploy on Fridays"')
    expect(out).toContain("picked B because newer and said twice")
    expect(out).toContain("you may mention the conflict")
  })

  test("no dispute field → output unchanged", () => {
    const { lines, disputes } = relatedLines([{ text: "Likes tea\nmore" }, { text: "Uses bun", dispute: null }])
    expect(lines).toEqual(["- Likes tea", "- Uses bun"])
    expect(disputes).toEqual([])
  })

  test("a needs_user dispute is asked once per session", async () => {
    const asks: any[] = []
    const resolves: any[] = []
    const deps = {
      ask: async (input: any) => {
        asks.push(input)
        return [[input.questions[0].options[0].label]]
      },
      resolve: async (id: string, choice: string) => {
        resolves.push([id, choice])
        return { kind: "ok" as const }
      },
    }
    const first = await askDisputes("s1", [needs, resolved], deps)
    expect(asks).toHaveLength(1)
    expect(asks[0].questions[0].options).toHaveLength(4)
    expect(resolves).toEqual([["d2", "a"]])
    expect(first[0]).toContain("recorded")
    expect(await askDisputes("s1", [needs], deps)).toEqual([])
    expect(asks).toHaveLength(1)
    await askDisputes("s2", [needs], deps) // another session asks again
    expect(asks).toHaveLength(2)
  })

  test("a 409 on resolve is quiet text for the model", async () => {
    const notes = await askDisputes("s3", [needs], {
      ask: async () => [["Neither"]],
      resolve: async () => ({ kind: "quiet", message: "already settled" }),
    })
    expect(notes[0]).toContain("Neither")
    expect(notes[0]).toContain("already settled")
  })
})

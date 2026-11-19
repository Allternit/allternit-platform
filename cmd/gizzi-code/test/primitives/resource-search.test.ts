import { describe, expect, test } from "bun:test"
import { ResourceIndex } from "../../src/runtime/primitives/resource-index"

/**
 * resource search (PR3): local keyword ranking over line windows returns the
 * file and the line range that hold the query, ready to cite.
 */
const filler = (n: number, word: string) => Array.from({ length: n }, (_, i) => `${word} note ${i}`)

const budget: ResourceIndex.Doc = {
  ref: "file:msg:msg_a:0",
  title: "budget.md",
  lines: [
    ...filler(40, "general"),
    "## Q3 travel budget",
    "The travel budget for Q3 is 12,000 dollars.",
    "Travel above the budget needs approval from finance.",
    ...filler(40, "general"),
  ],
}

const minutes: ResourceIndex.Doc = {
  ref: "file:msg:msg_b:0",
  title: "minutes.txt",
  lines: [
    ...filler(15, "meeting"),
    "We talked about the budget once.",
    ...filler(30, "meeting"),
  ],
}

describe("resource search ranking", () => {
  test("the dense passage ranks first, cited at its own lines", () => {
    const r = ResourceIndex.rank([minutes, budget], "travel budget", { max: 5 })
    expect(r.hits.length).toBeGreaterThan(0)
    const top = r.hits[0]
    expect(top.ref).toBe("file:msg:msg_a:0")
    expect(top.citation).toEqual({ ref: "file:msg:msg_a:0", start_line: 41, end_line: 43 })
    expect(top.snippet[0]).toBe("41| ## Q3 travel budget")
    expect(top.why).toContain('"travel" x3')
  })

  test("the other file still appears, below, with its matching line", () => {
    const r = ResourceIndex.rank([minutes, budget], "travel budget", { max: 5 })
    const other = r.hits.find((h) => h.ref === "file:msg:msg_b:0")
    expect(other?.citation.start_line).toBe(16)
    expect(other?.citation.end_line).toBe(16)
    expect(other?.why).toContain('no "travel"')
    expect(r.hits.indexOf(other!)).toBeGreaterThan(0)
  })

  test("paging continues where the last page stopped", () => {
    const first = ResourceIndex.rank([minutes, budget], "travel budget", { max: 1 })
    expect(first.hits).toHaveLength(1)
    expect(first.next).toBe(1)
    const second = ResourceIndex.rank([minutes, budget], "travel budget", { max: 1, offset: first.next! })
    expect(second.hits[0].ref).not.toBe(first.hits[0].ref)
  })

  test("a query with no known words returns nothing", () => {
    expect(ResourceIndex.rank([minutes, budget], "zebra", { max: 5 }).hits).toEqual([])
  })
})

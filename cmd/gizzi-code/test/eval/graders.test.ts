import { describe, expect, test } from "bun:test"
import { readFileSync } from "node:fs"
import path from "node:path"
import { grade, parseGrader, type Trajectory } from "../../src/runtime/eval/graders"

// cmd/gizzi-code/test/eval -> repo root is four levels up.
const VECTORS = path.resolve(import.meta.dir, "../../../../contracts/eval/grader-vectors.v1.json")
type Vector = { name: string; trajectory: Trajectory; grader: unknown; expect: { pass: boolean } }
const vectors = JSON.parse(readFileSync(VECTORS, "utf8")) as Vector[]

describe("shared grader vectors", () => {
  test("the vector file covers every grader with a pass and a fail", () => {
    const seen = new Map<string, Set<boolean>>()
    for (const v of vectors) {
      const ty = (v.grader as { type: string }).type
      seen.set(ty, (seen.get(ty) ?? new Set()).add(v.expect.pass))
    }
    expect(seen.size).toBe(11)
    for (const [ty, outcomes] of seen) expect([ty, [...outcomes].sort()]).toEqual([ty, [false, true]])
    expect(vectors.length).toBeGreaterThanOrEqual(22)
  })

  for (const v of vectors) {
    test(v.name, () => {
      const g = grade(v.trajectory, parseGrader(v.grader))
      expect({ name: v.name, pass: g.pass, detail: g.detail }).toEqual({ name: v.name, pass: v.expect.pass, detail: g.detail })
    })
  }
})

describe("parseGrader", () => {
  test("rejects an unknown type and lists the known ones", () => {
    expect(() => parseGrader({ type: "vibes_check" })).toThrow(/unknown grader `vibes_check`.*step_order/)
  })
  test("rejects an unknown field like Rust does", () => {
    expect(() => parseGrader({ type: "status", equals: "completed", extra: 1 })).toThrow(/unknown field `extra`/)
  })
  test("rejects effect_count with no bounds", () => {
    expect(() => parseGrader({ type: "effect_count" })).toThrow(/needs min or max/)
  })
})

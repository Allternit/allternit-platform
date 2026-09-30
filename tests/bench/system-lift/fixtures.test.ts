import { describe, expect, test } from "bun:test"
import { mkdtemp, rm } from "node:fs/promises"
import { join } from "node:path"
import { CATEGORIES, generateFixtures } from "./fixtures"
import { ROOT, materialize, runTests, applyPatch } from "./workspace"

describe("fixtures", () => {
  test("seed controls content and all categories have private ground truth", () => {
    const a = generateFixtures(12, 42)
    expect(a).toEqual(generateFixtures(12, 42))
    expect(a).not.toEqual(generateFixtures(12, 43))
    expect(new Set(a.map(x => x.category))).toEqual(new Set(CATEGORIES))
    expect(new Set(a.map(x => x.id)).size).toBe(12)
    for (const f of a) { expect(f.truth.patch).toContain("diff --git"); expect(JSON.stringify(f.files)).not.toContain("fixedSource") }
    expect(() => generateFixtures(0)).toThrow()
    expect(() => generateFixtures(1, -1)).toThrow()
  })
  test("each bug fails target, passes regression, and ground truth fixes it", async () => {
    const dir = await mkdtemp(join(ROOT, ".tmp-wp13-fixtures-"))
    try {
      for (const f of generateFixtures(6, 77)) {
        const repo = join(dir, f.id)
        await materialize(f, repo)
        const before = await runTests(repo)
        expect(before.target.passed).toBe(false)
        expect(before.regression.passed).toBe(true)
        expect(await applyPatch(repo, f.truth.patch)).toBe(true)
        const after = await runTests(repo)
        expect(after.target.passed).toBe(true)
        expect(after.regression.passed).toBe(true)
      }
    } finally { await rm(dir, { recursive: true, force: true }) }
  }, 120000)
})

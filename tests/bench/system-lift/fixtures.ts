import { createHash } from "node:crypto"
import type { Category, Fixture } from "./types"

export const CATEGORIES: Category[] = ["off-by-one", "null-handling", "wrong-import", "async-await", "sort-comparator", "falsy-default"]
export function random(seed: number) {
  let state = seed >>> 0
  return () => { state = (Math.imul(state, 1664525) + 1013904223) >>> 0; return state / 2 ** 32 }
}
export function sourcePatch(before: string, after: string, file = "src/lib.ts"): string {
  const oldLines = before.trimEnd().split("\n"), newLines = after.trimEnd().split("\n")
  return `diff --git a/${file} b/${file}\n--- a/${file}\n+++ b/${file}\n@@ -1,${oldLines.length} +1,${newLines.length} @@\n` +
    oldLines.map(x => `-${x}\n`).join("") + newLines.map(x => `+${x}\n`).join("")
}
export function generateFixtures(count: number, seed = 13): Fixture[] {
  if (!Number.isSafeInteger(count) || count < 1 || count > 10000) throw new Error("count must be 1..10000")
  if (!Number.isSafeInteger(seed) || seed < 0 || seed > 0xffffffff) throw new Error("seed must be uint32")
  const rng = random(seed)
  return Array.from({ length: count }, (_, i) => {
    const category = CATEGORIES[i % CATEGORIES.length]
    const n = 2 + Math.floor(rng() * 30), label = `item-${Math.floor(rng() * 10000)}`
    const defs: Record<Category, { bad: string; good: string; test: string; report: string }> = {
      "off-by-one": {
        bad: "export function sumThrough(n: number) { let total = 0; for (let i = 1; i < n; i++) total += i; return total }",
        good: "export function sumThrough(n: number) { let total = 0; for (let i = 1; i <= n; i++) total += i; return total }",
        test: `import { sumThrough } from '../src/lib'; test('includes endpoint', () => { expect(sumThrough(${n})).toBe(${n * (n + 1) / 2}); expect(sumThrough(0)).toBe(0) })`,
        report: `sumThrough(${n}) should include its endpoint and return ${n * (n + 1) / 2}.`,
      },
      "null-handling": {
        bad: "export function cleanLabel(value: string | null) { return value!.trim() }",
        good: "export function cleanLabel(value: string | null) { return value?.trim() ?? '' }",
        test: `import { cleanLabel } from '../src/lib'; test('null and text', () => { expect(cleanLabel(null)).toBe(''); expect(cleanLabel(' ${label} ')).toBe('${label}') })`,
        report: "cleanLabel crashes for null; null should produce an empty string and text should be trimmed.",
      },
      "wrong-import": {
        bad: "import { multiply as operation } from './dep'\nexport function addPair(a: number, b: number) { return operation(a, b) }",
        good: "import { add as operation } from './dep'\nexport function addPair(a: number, b: number) { return operation(a, b) }",
        test: `import { addPair } from '../src/lib'; test('addition', () => { expect(addPair(${n}, 1)).toBe(${n + 1}); expect(addPair(0, 7)).toBe(7) })`,
        report: "addPair performs multiplication instead of addition; use the existing dependency correctly.",
      },
      "async-await": {
        bad: "import { readLabel } from './dep'\nexport async function loadLabel() { const value = readLabel(); return String(value) }",
        good: "import { readLabel } from './dep'\nexport async function loadLabel() { const value = await readLabel(); return String(value) }",
        test: `import { loadLabel } from '../src/lib'; test('resolved label', async () => { expect(await loadLabel()).toBe('${label}') })`,
        report: "loadLabel returns a promise's string representation; it should return the resolved label.",
      },
      "sort-comparator": {
        bad: "export function sortNumbers(values: number[]) { return [...values].sort() }",
        good: "export function sortNumbers(values: number[]) { return [...values].sort((a, b) => a - b) }",
        test: `import { sortNumbers } from '../src/lib'; test('numeric order without mutation', () => { const values = [100, 2, ${n}]; expect(sortNumbers(values)).toEqual([2, ${n}, 100]); expect(values).toEqual([100, 2, ${n}]) })`,
        report: "sortNumbers uses lexicographic order; return ascending numeric order without mutating input.",
      },
      "falsy-default": {
        bad: "export function resolveLimit(value: number | null, fallback: number) { return value || fallback }",
        good: "export function resolveLimit(value: number | null, fallback: number) { return value ?? fallback }",
        test: `import { resolveLimit } from '../src/lib'; test('zero and null', () => { expect(resolveLimit(0, ${n})).toBe(0); expect(resolveLimit(null, ${n})).toBe(${n}); expect(resolveLimit(1, ${n})).toBe(1) })`,
        report: "resolveLimit replaces zero with the fallback; only null should select the fallback.",
      },
    }
    const d = defs[category], stable = "\nexport function stableDouble(value: number) { return value * 2 }\n"
    const bad = d.bad + stable, good = d.good + stable
    const id = `fixture-${String(i).padStart(4, '0')}-${createHash('sha256').update(`${seed}:${i}:${bad}:${label}`).digest('hex').slice(0, 10)}`
    return { id, seed, category, report: d.report,
      files: {
        "package.json": JSON.stringify({ name: id, private: true, type: "module", scripts: { test: "vitest run" }, devDependencies: { typescript: "^5.3.0", vitest: "1.6.1" } }, null, 2) + "\n",
        "vitest.config.ts": "export default { cacheDir: '.vitest-cache', test: { cache: false, include: ['tests/*.test.ts'], pool: 'forks', minWorkers: 1, maxWorkers: 1, fileParallelism: false, isolate: true } }\n",
        "src/lib.ts": bad,
        "src/dep.ts": `export const add = (a: number, b: number) => a + b\nexport const multiply = (a: number, b: number) => a * b\nexport const readLabel = async () => '${label}'\n`,
        "tests/bug.test.ts": "import { test, expect } from 'vitest';\n" + d.test + "\n",
        "tests/regression.test.ts": `import { test, expect } from 'vitest';\nimport { stableDouble } from '../src/lib';\ntest('unaffected doubling', () => { expect(stableDouble(${n})).toBe(${n * 2}); expect(stableDouble(0)).toBe(0); expect(stableDouble(-2)).toBe(-4) })\n`,
      }, truth: { file: "src/lib.ts", fixedSource: good, patch: sourcePatch(bad, good), explanation: d.report } }
  })
}

import { describe, expect, test } from "bun:test"
import { BUILTIN_CATALOG, CATALOG_ALIASES, DELEGATION_HINT, FOLDED_TYPES } from "../../src/runtime/agents/builtin-catalog"
import { BUNDLED_SKILLS } from "../../src/runtime/skills/bundledSkills"

describe("subagent types folded into skills", () => {
  test("folded types are gone from the catalog and point at a bundled skill", () => {
    const names = BUILTIN_CATALOG.map((a) => a.name)
    expect(names).toHaveLength(11)
    const skills = new Set(BUNDLED_SKILLS.map((s) => s.name))
    for (const [type, skill] of Object.entries(FOLDED_TYPES)) {
      expect(names).not.toContain(type)
      expect(skills.has(skill)).toBe(true)
    }
  })

  test("no alias resolves to a folded type", () => {
    for (const target of Object.values(CATALOG_ALIASES)) expect(FOLDED_TYPES[target]).toBeUndefined()
  })

  test("the delegation hint names the skills, not the folded types", () => {
    for (const skill of new Set(Object.values(FOLDED_TYPES))) expect(DELEGATION_HINT).toContain(skill)
    for (const type of ["planner", "debugger", "docs-writer", "scribe"]) expect(DELEGATION_HINT).not.toContain(`(${type})`)
  })
})

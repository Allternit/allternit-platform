import { describe, expect, test } from "bun:test"
import { BUILTIN_CATALOG, CATALOG_ALIASES, DELEGATION_HINT, FOLDED_TYPES, cliDelegationNote } from "../../src/runtime/agents/builtin-catalog"
import { cliAdapterArgv } from "../../src/runtime/drivers/local-cli-driver"
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

describe("the fold rule reaches CLI brains", () => {
  test("the CLI note carries the rule and names the bridge's skill tool", () => {
    const on = cliDelegationNote("allternit")
    expect(on).toContain(DELEGATION_HINT)
    expect(on).toContain("mcp__allternit__skill")
    expect(cliDelegationNote("allternit", true)).toContain("subagents are off")
  })

  test("Claude Code gets the catalog plus config subagents, or no Agent tool when subagents are off", () => {
    const extra = [{ slug: "planner", description: "Plans", prompt: "Plan it.", modelClass: "inherit" as const }]
    const on = cliAdapterArgv("claude-cli", ["claude"], "hi", { taskId: "t1", subagents: { extra } })
    const agents = JSON.parse(on[on.indexOf("--agents") + 1])
    expect(Object.keys(agents)).toContain("explorer")
    expect(agents.planner.prompt).toBe("Plan it.")
    // A config subagent never replaces a catalog type.
    const clash = cliAdapterArgv("claude-cli", ["claude"], "hi", { taskId: "t1", subagents: { extra: [{ ...extra[0], slug: "explorer" }] } })
    expect(JSON.parse(clash[clash.indexOf("--agents") + 1]).explorer.prompt).not.toBe("Plan it.")

    const off = cliAdapterArgv("claude-cli", ["claude"], "hi", { taskId: "t1", subagents: { off: true } })
    expect(off).not.toContain("--agents")
    expect(off.join(" ")).toContain("--disallowedTools Task --disallowedTools Agent")
  })
})

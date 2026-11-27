import { describe, expect, test } from "bun:test"
import { parseSkillUri, skillNameFromUri, uniqueSkillKey, skillOriginLabel } from "./mcpSkills"
import { loadSkillLabel } from "./skillLabel"

describe("skill names", () => {
  test("server, app, user, org URIs", () => {
    expect(skillNameFromUri("skill://notes/SKILL.md")).toBe("notes")
    expect(skillNameFromUri("skill://app/tips/tips/SKILL.md")).toBe("tips")
    expect(skillNameFromUri("skill://user/daily-brief/SKILL.md")).toBe("daily-brief")
    expect(skillNameFromUri("skill://org/acme/runbook/SKILL.md")).toBe("runbook")
    expect(skillNameFromUri("https://x")).toBeUndefined()
  })
  test("collisions get a source prefix", () => {
    const taken = new Set(["tips"])
    expect(uniqueSkillKey(parseSkillUri("skill://app/tips/tips/SKILL.md")!, taken)).toBe("app-tips:tips")
    expect(uniqueSkillKey(parseSkillUri("skill://user/tips/SKILL.md")!, taken)).toBe("user:tips")
    expect(uniqueSkillKey(parseSkillUri("skill://notes/SKILL.md")!, new Set())).toBe("notes")
  })
  test("origin label", () => {
    expect(skillOriginLabel(parseSkillUri("skill://app/tips/tips/SKILL.md")!, "x")).toBe("from the tips app")
    expect(skillOriginLabel(parseSkillUri("skill://user/a/SKILL.md")!, "x")).toBe("your skill")
    expect(skillOriginLabel(parseSkillUri("skill://org/o/a/SKILL.md")!, "x")).toBe("org skill")
  })
  test("load_skill row label", () => {
    expect(loadSkillLabel("mcp__allternit__load_skill", { id: "tips:tips" }, { text: "Skill tips:tips from Tips. This text comes from that server.\nbody" })).toBe(
      "Loaded skill · tips:tips · from Tips",
    )
    expect(loadSkillLabel("read_file")).toBeUndefined()
  })
})

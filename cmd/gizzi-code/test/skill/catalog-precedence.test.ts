import { afterEach, expect, test } from "bun:test"
import path from "path"
import { tmpdir } from "../fixture/fixture"
import { Instance } from "@/runtime/context/project/instance"
import { Skill } from "@/runtime/skills/skill"

afterEach(() => delete process.env.GIZZI_DISABLE_BUILTIN_SKILLS)

async function writeSkill(root: string, directory: string, name: string, description: string, extra = "") {
  await Bun.write(path.join(root, directory, "SKILL.md"), `---
name: ${name}
description: ${description}
${extra}---

# ${name}
`)
}

test("project definitions deterministically win user-compatible collisions", async () => {
  process.env.GIZZI_DISABLE_BUILTIN_SKILLS = "true"
  await using project = await tmpdir({
    git: true,
    init: async (root) => {
      await writeSkill(root, ".claude/skills/shared", "shared", "Claude project definition")
      await writeSkill(root, ".gizzi/skills/shared", "shared", "Gizzi project definition")
    },
  })
  await Instance.provide({
    directory: project.path,
    fn: async () => {
      const winner = await Skill.get("shared")
      const collision = (await Skill.collisions()).find((item) => item.name === "shared")
      expect(winner?.description).toBe("Claude project definition")
      expect(winner?.source).toBe("project")
      expect(collision?.shadowed).toHaveLength(1)
      expect(collision?.shadowed[0]?.description).toBe("Gizzi project definition")
    },
  })
})

test("nested bundles require parent opt-in and receive qualified names", async () => {
  process.env.GIZZI_DISABLE_BUILTIN_SKILLS = "true"
  await using project = await tmpdir({
    git: true,
    init: async (root) => {
      await writeSkill(root, ".gizzi/skills/parent", "parent", "Parent", "has-sub-skill: true\n")
      await writeSkill(root, ".gizzi/skills/parent/child", "child", "Child")
      await writeSkill(root, ".gizzi/skills/closed", "closed", "Closed")
      await writeSkill(root, ".gizzi/skills/closed/hidden", "hidden", "Hidden")
    },
  })
  await Instance.provide({
    directory: project.path,
    fn: async () => {
      expect(await Skill.get("parent.child")).toBeDefined()
      expect(await Skill.get("hidden")).toBeUndefined()
    },
  })
})

test("path-scoped skills remain hidden until a matching file is touched", async () => {
  process.env.GIZZI_DISABLE_BUILTIN_SKILLS = "true"
  await using project = await tmpdir({
    git: true,
    init: async (root) => {
      await writeSkill(root, ".gizzi/skills/typescript", "typescript", "TypeScript only", "paths:\n  - src/**/*.ts\n")
    },
  })
  await Instance.provide({
    directory: project.path,
    fn: async () => {
      expect(await Skill.get("typescript")).toBeUndefined()
      expect(await Skill.activateForPaths([path.join(project.path, "README.md")])).toEqual([])
      expect(await Skill.activateForPaths([path.join(project.path, "src/index.ts")])).toEqual(["typescript"])
      expect(await Skill.get("typescript")).toBeDefined()
    },
  })
})

import { test, expect, beforeEach, afterEach } from "bun:test"
import path from "path"
import fs from "fs/promises"
import os from "os"
import { walkCraft, CRAFT_DIR } from "../../src/runtime/skills/craftFiles"
import { materializeCraftSkills, craftHash } from "../../src/runtime/skills/craftSkills"

let work = ""
beforeEach(async () => {
  work = await fs.mkdtemp(path.join(os.tmpdir(), "gizzi-craft-"))
})
afterEach(async () => {
  await fs.rm(work, { recursive: true, force: true })
})

async function fixture(body = "Run `${SKILL_DIR}/scripts/go.sh`.") {
  const dir = path.join(work, "src", "demo")
  await fs.mkdir(path.join(dir, "scripts"), { recursive: true })
  await fs.mkdir(path.join(dir, "node_modules", "x"), { recursive: true })
  await Bun.write(path.join(dir, "SKILL.md"), `---\nname: demo\ndescription: Demo craft skill.\n---\n\n${body}\n`)
  await Bun.write(path.join(dir, "scripts", "go.sh"), "#!/bin/sh\necho '${SKILL_DIR}'\n")
  await Bun.write(path.join(dir, "scripts", "dot.png"), new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0, 255]))
  await Bun.write(path.join(dir, "node_modules", "x", "index.js"), "skip me")
  return walkCraft(path.join(work, "src"))
}

test("walkCraft takes text and binary files and skips node_modules", async () => {
  const skills = await fixture()
  const files = skills.demo.map((f) => f.path)
  expect(files).toEqual(["SKILL.md", "scripts/dot.png", "scripts/go.sh"])
  expect(skills.demo.find((f) => f.path === "scripts/dot.png")?.encoding).toBe("base64")
})

test("extracts once, substitutes ${SKILL_DIR} in Markdown only, round-trips binaries", async () => {
  const skills = await fixture()
  const root = path.join(work, "data")
  const [skillFile] = await materializeCraftSkills(root, skills)
  const dir = path.dirname(skillFile)
  expect(dir).toBe(path.join(root, "demo", craftHash(skills.demo)))
  expect(await Bun.file(skillFile).text()).toContain(`Run \`${dir}/scripts/go.sh\``)
  expect(await Bun.file(path.join(dir, "scripts", "go.sh")).text()).toContain("${SKILL_DIR}")
  expect([...new Uint8Array(await Bun.file(path.join(dir, "scripts", "dot.png")).arrayBuffer())]).toEqual([0x89, 0x50, 0x4e, 0x47, 0, 255])
  expect((await fs.stat(path.join(dir, "scripts", "go.sh"))).mode & 0o111).not.toBe(0)

  const marker = path.join(dir, "user-note.txt")
  await Bun.write(marker, "kept")
  expect(await materializeCraftSkills(root, skills)).toEqual([skillFile])
  expect(await Bun.file(marker).exists()).toBe(true)
})

test("changed files extract to a new directory and leave the old one", async () => {
  const root = path.join(work, "data")
  const [first] = await materializeCraftSkills(root, await fixture("v1"))
  const [second] = await materializeCraftSkills(root, await fixture("v2"))
  expect(second).not.toBe(first)
  expect(await Bun.file(first).exists()).toBe(true)
})

test("a path escaping the skill directory is refused", async () => {
  const root = path.join(work, "data")
  const out = await materializeCraftSkills(root, {
    evil: [{ path: "../../outside.txt", data: "x", encoding: "utf8" }],
  })
  expect(out).toEqual([])
  expect(await Bun.file(path.join(work, "outside.txt")).exists()).toBe(false)
})

test("every shipped craft skill has valid frontmatter and no machine-specific paths", async () => {
  const skills = walkCraft(CRAFT_DIR)
  for (const [name, files] of Object.entries(skills)) {
    const skill = files.find((f) => f.path === "SKILL.md")!
    expect(skill.data).toMatch(new RegExp(`^---\\n[\\s\\S]*?name: ${name}\\n[\\s\\S]*?description:`))
    for (const file of files) {
      if (file.encoding !== "utf8") continue
      expect({ file: `${name}/${file.path}`, home: /\/Users\/joe|~\/\.agents|~\/Desktop/.test(file.data) }).toEqual({
        file: `${name}/${file.path}`,
        home: false,
      })
    }
  }
})

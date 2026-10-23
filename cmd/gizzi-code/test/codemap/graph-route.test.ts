import { afterEach, describe, expect, test } from "bun:test"
import { $ } from "bun"
import fs from "node:fs/promises"
import os from "node:os"
import path from "node:path"
import { RepoMap } from "@/codemap/repo-map"
import { CodeMapActivity } from "@/codemap/activity"
import { buildCodeMapGraph } from "@/runtime/server/routes/codemap"

const roots: string[] = []
afterEach(async () => {
  RepoMap.reset()
  await Promise.all(roots.splice(0).map((root) => fs.rm(root, { recursive: true, force: true })))
})

async function repo(files: Record<string, string>) {
  const dir = await fs.realpath(await fs.mkdtemp(path.join(os.tmpdir(), "gizzi-codemap-graph-")))
  roots.push(dir)
  for (const [rel, content] of Object.entries(files)) {
    await fs.mkdir(path.dirname(path.join(dir, rel)), { recursive: true })
    await fs.writeFile(path.join(dir, rel), content)
  }
  await $`git init -q`.cwd(dir).quiet()
  await $`git add -A`.cwd(dir).quiet()
  await $`git -c user.email=t@t -c user.name=t commit -q -m initial`.cwd(dir).quiet()
  return dir
}

const graph = {
  "package.json": JSON.stringify({ name: "fixture" }),
  "src/util.ts": "export function helper() {}\nexport const VALUE = 1\n",
  "src/a.ts": 'import { helper } from "./util"\nexport function a() { return helper() }\n',
  "src/b.ts": 'import { VALUE } from "./util"\nimport { a } from "./a"\nexport class B {}\n',
  "src/leaf.ts": "export type Leaf = string\n",
}

describe("codemap graph", () => {
  test("returns files by score with importers, imports, exports and the edges among them", async () => {
    const dir = await repo(graph)
    const result = await buildCodeMapGraph({ directory: dir })
    expect(result.project).toBe(true)
    expect(result.root).toBe(dir)
    expect(result.total).toBe(5)
    expect(result.shown).toBe(5)
    expect(result.files[0]).toEqual({
      path: "src/util.ts",
      score: 2,
      importers: 2,
      imports: 0,
      exports: ["helper", "VALUE"],
    })
    const b = result.files.find((file) => file.path === "src/b.ts")!
    expect(b).toMatchObject({ importers: 0, imports: 2 })
    expect(result.edges).toContainEqual({ from: "src/a.ts", to: "src/util.ts" })
    expect(result.edges).toContainEqual({ from: "src/b.ts", to: "src/util.ts" })
    expect(result.edges).toContainEqual({ from: "src/b.ts", to: "src/a.ts" })
    expect(result.edges).toHaveLength(3)
    expect(result.updatedAt).not.toBeNull()
    expect(result.promptShown).toBe(0)
  })

  test("limit keeps the top files and only edges between them", async () => {
    const dir = await repo(graph)
    const result = await buildCodeMapGraph({ directory: dir, limit: 2 })
    expect(result.shown).toBe(2)
    expect(result.total).toBe(5)
    expect(result.files.map((file) => file.path)).toEqual(["src/util.ts", "src/a.ts"])
    expect(result.edges).toEqual([{ from: "src/a.ts", to: "src/util.ts" }])
  })

  test("promptShown follows the last prompt slice; session edits count as boosted", async () => {
    const dir = await repo(graph)
    await RepoMap.load(dir)
    const text = RepoMap.slice({ root: dir })
    const listed = Number(/shown="(\d+)"/.exec(text)![1])
    RepoMap.noteEdited("ses_1", path.join(dir, "src/leaf.ts"))
    const result = await buildCodeMapGraph({ directory: dir, sessionID: "ses_1" })
    expect(result.promptShown).toBe(listed)
    expect(RepoMap.promptShown(dir)).toBe(listed)
    expect(result.editedBoosted).toBe(1)
  })

  test("a folder that is not a project returns an empty map", async () => {
    const dir = await fs.realpath(await fs.mkdtemp(path.join(os.tmpdir(), "gizzi-codemap-plain-")))
    roots.push(dir)
    const result = await buildCodeMapGraph({ directory: dir })
    expect(result).toMatchObject({ project: false, files: [], edges: [], total: 0, shown: 0, updatedAt: null })
  })

  test("activity paths are project-relative and drop files outside the project", () => {
    expect(CodeMapActivity.relative("/repo", "/repo/src/a.ts")).toBe("src/a.ts")
    expect(CodeMapActivity.relative("/repo", "src/a.ts")).toBe("src/a.ts")
    expect(CodeMapActivity.relative("/repo", "/elsewhere/a.ts")).toBeUndefined()
  })
})

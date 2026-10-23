import { afterEach, describe, expect, test } from "bun:test"
import { $ } from "bun"
import fs from "node:fs/promises"
import os from "node:os"
import path from "node:path"
import { RepoMap } from "@/codemap/repo-map"
import { Token } from "@/shared/util/token"

const roots: string[] = []
afterEach(async () => {
  RepoMap.reset()
  await Promise.all(roots.splice(0).map((root) => fs.rm(root, { recursive: true, force: true })))
})

/** A git repo with `files` committed (one commit), realpath'd so relative paths line up. */
async function repo(files: Record<string, string>, options: { commit?: boolean } = {}) {
  const dir = await fs.realpath(await fs.mkdtemp(path.join(os.tmpdir(), "gizzi-repo-map-")))
  roots.push(dir)
  await write(dir, files)
  await $`git init -q`.cwd(dir).quiet()
  if (options.commit !== false) await commit(dir, "initial")
  return dir
}

async function write(dir: string, files: Record<string, string>) {
  for (const [rel, content] of Object.entries(files)) {
    await fs.mkdir(path.dirname(path.join(dir, rel)), { recursive: true })
    await fs.writeFile(path.join(dir, rel), content)
  }
}

async function commit(dir: string, message: string) {
  await $`git add -A`.cwd(dir).quiet()
  await $`git -c user.email=t@t -c user.name=t commit -q --allow-empty -m ${message}`.cwd(dir).quiet()
}

function lineFor(text: string, file: string) {
  return text.split("\n").find((line) => line.startsWith(file + " ") || line === file)
}

const graph = {
  "package.json": JSON.stringify({ name: "fixture" }),
  "src/util.ts": "export function helper() {}\nexport const VALUE = 1\n",
  "src/a.ts": 'import { helper } from "./util"\nexport function a() { return helper() }\n',
  "src/b.ts": 'import { VALUE } from "./util"\nimport { a } from "./a"\nexport class B {}\n',
  "src/leaf.ts": "export type Leaf = string\n",
  "scripts/tool.py": "def run():\n    pass\n\nclass Runner:\n    pass\n",
}

describe("RepoMap", () => {
  test("ranks by how many files import each file, with AST and textual symbols", async () => {
    const dir = await repo(graph)
    expect(await RepoMap.load(dir)).toBe(true)
    const ranked = RepoMap.rank({ root: dir })
    expect(ranked[0]).toMatchObject({ path: "src/util.ts", inDegree: 2, symbols: ["helper", "VALUE"] })
    expect(ranked[1]).toMatchObject({ path: "src/a.ts", inDegree: 1 })

    const text = RepoMap.slice({ root: dir })
    expect(text.startsWith('<repo_map files="6"')).toBe(true)
    expect(text.endsWith("</repo_map>")).toBe(true)
    expect(lineFor(text, "src/util.ts")).toBe("src/util.ts (→2) +helper +VALUE")
    expect(lineFor(text, "scripts/tool.py")).toBe("scripts/tool.py +run +Runner")
  })

  test("session edits and reads are boosted; edits are marked [NEW]", async () => {
    const dir = await repo(graph)
    await RepoMap.load(dir)
    RepoMap.noteEdited("ses_1", path.join(dir, "src/leaf.ts"))
    const text = RepoMap.slice({ root: dir, sessionID: "ses_1", boost: ["scripts/tool.py"] })
    const lines = text.split("\n").slice(2, -1)
    // Boosted leaf: 0×3+3 = 3; co-change lends every other file in the shared commit +1.
    expect(lines).toContain("src/leaf.ts +Leaf [NEW]")
    expect(lines.indexOf("src/leaf.ts +Leaf [NEW]")).toBeLessThan(lines.indexOf("src/b.ts +B"))
    expect(lineFor(text, "scripts/tool.py")).not.toContain("[NEW]") // read, not edited
    // Another session sees no [NEW] marks.
    // (The legend line names the [NEW] mark; only file lines count.)
    const other = RepoMap.slice({ root: dir, sessionID: "ses_2" }).split("\n").filter((l) => !l.includes(" = "))
    expect(other.some((l) => l.includes("[NEW]"))).toBe(false)
  })

  test("co-change: files that changed with a boosted file rank higher", async () => {
    const dir = await repo(graph)
    await write(dir, { "src/leaf.ts": "export type Leaf = number\n", "scripts/tool.py": "def run():\n    return 1\n" })
    await commit(dir, "leaf and tool together")
    await RepoMap.load(dir)
    const ranked = RepoMap.rank({ root: dir, boost: ["src/leaf.ts"] })
    const tool = ranked.find((file) => file.path === "scripts/tool.py")!
    const b = ranked.find((file) => file.path === "src/b.ts")!
    expect(tool.score).toBeGreaterThan(b.score)
  })

  test("stops at the token budget", async () => {
    const files: Record<string, string> = { "package.json": "{}" }
    for (let i = 0; i < 200; i++) files[`src/module-${i}.ts`] = `export const value${i} = ${i}\n`
    const dir = await repo(files)
    await RepoMap.load(dir)
    const text = RepoMap.slice({ root: dir, budgetTokens: 200 })
    const shown = Number(text.match(/shown="(\d+)"/)![1])
    expect(shown).toBeGreaterThan(0)
    expect(shown).toBeLessThan(201)
    expect(Token.estimate(text)).toBeLessThanOrEqual(200)
    const all = RepoMap.slice({ root: dir, budgetTokens: 200, all: true })
    expect(all).toContain('shown="201"')
  })

  test("skips ignored, generated, lockfile and dot-dir files", async () => {
    const dir = await repo({
      ...graph,
      ".gizziignore": "generated/\n*.snap.ts\n",
      "generated/schema.ts": "export const Schema = 1\n",
      "src/thing.snap.ts": "export const X = 1\n",
      "dist/bundle.js": "export const Bundle = 1\n",
      "vendor/lib.js": "export const Lib = 1\n",
      "src/app.min.js": "export const Min = 1\n",
      "pnpm-lock.yaml": "lockfileVersion: 9\n",
      ".config/settings.ts": "export const Hidden = 1\n",
    })
    await RepoMap.load(dir)
    const paths = RepoMap.rank({ root: dir }).map((file) => file.path)
    expect(paths.sort()).toEqual(["package.json", "scripts/tool.py", "src/a.ts", "src/b.ts", "src/leaf.ts", "src/util.ts"])
  })

  test("reuses cached entries for unchanged files and updates changed ones incrementally", async () => {
    const dir = await repo(graph)
    await RepoMap.load(dir)
    const cacheFile = RepoMap.cachePath(dir)
    const cache = JSON.parse(await fs.readFile(cacheFile, "utf8"))
    expect(Object.keys(cache.files)).toHaveLength(6)

    // Doctor an unchanged file's cached symbols: a reload must reuse them, not rescan.
    cache.files["src/leaf.ts"].symbols = ["FromCache"]
    await fs.writeFile(cacheFile, JSON.stringify(cache))
    RepoMap.reset()
    await RepoMap.load(dir)
    expect(RepoMap.rank({ root: dir }).find((file) => file.path === "src/leaf.ts")!.symbols).toEqual(["FromCache"])

    // A new importer of util.ts raises its in-degree after update().
    await write(dir, { "src/c.ts": 'import { helper } from "./util"\nexport const C = helper\n' })
    await RepoMap.update([path.join(dir, "src/c.ts")])
    expect(RepoMap.rank({ root: dir })[0]).toMatchObject({ path: "src/util.ts", inDegree: 3 })
    const written = JSON.parse(await fs.readFile(cacheFile, "utf8"))
    expect(written.files["src/c.ts"].symbols).toEqual(["C"])

    // A deleted file drops out.
    await fs.rm(path.join(dir, "src/c.ts"))
    await RepoMap.update(["src/c.ts"], dir)
    expect(RepoMap.rank({ root: dir })[0]).toMatchObject({ path: "src/util.ts", inDegree: 2 })
  })

  test("refuses to replace a cache when an update would drop more than 80% of it", async () => {
    const files: Record<string, string> = { "package.json": "{}" }
    for (let i = 0; i < 19; i++) files[`src/m${i}.ts`] = `export const m${i} = ${i}\n`
    const dir = await repo(files, { commit: false })
    await RepoMap.load(dir)
    expect(RepoMap.rank({ root: dir })).toHaveLength(20)

    for (let i = 0; i < 18; i++) await fs.rm(path.join(dir, `src/m${i}.ts`))
    RepoMap.reset()
    await RepoMap.load(dir)
    // The last good map stays, in memory and on disk.
    expect(RepoMap.rank({ root: dir })).toHaveLength(20)
    const cache = JSON.parse(await fs.readFile(RepoMap.cachePath(dir), "utf8"))
    expect(Object.keys(cache.files)).toHaveLength(20)
  })

  test("prompt() is empty when disabled or outside a project folder, and stable within a turn", async () => {
    const dir = await repo(graph)
    expect(await RepoMap.prompt({ root: dir, sessionID: "s", turn: "t1", enabled: false })).toBe("")
    expect(await RepoMap.prompt({ root: os.homedir(), sessionID: "s", turn: "t1" })).toBe("")
    expect(await RepoMap.prompt({ root: undefined, sessionID: "s", turn: "t1" })).toBe("")

    const first = await RepoMap.prompt({ root: dir, sessionID: "s", turn: "t1" })
    expect(first).toContain("src/util.ts (→2)")
    RepoMap.noteEdited("s", path.join(dir, "src/leaf.ts"))
    expect(await RepoMap.prompt({ root: dir, sessionID: "s", turn: "t1" })).toBe(first)
    expect(await RepoMap.prompt({ root: dir, sessionID: "s", turn: "t2" })).toContain("src/leaf.ts +Leaf [NEW]")

    process.env["GIZZI_NO_REPO_MAP"] = "1"
    try {
      expect(await RepoMap.prompt({ root: dir, sessionID: "s", turn: "t3" })).toBe("")
    } finally {
      delete process.env["GIZZI_NO_REPO_MAP"]
    }
  })
})

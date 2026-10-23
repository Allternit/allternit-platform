import { afterEach, describe, expect, test } from "bun:test"
import fs from "node:fs/promises"
import os from "node:os"
import path from "node:path"
import { ProjectPanels } from "@/runtime/project/panels"

const roots: string[] = []
afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => fs.rm(root, { recursive: true, force: true })))
})

async function project(files: Record<string, string>) {
  const dir = await fs.realpath(await fs.mkdtemp(path.join(os.tmpdir(), "gizzi-panels-")))
  roots.push(dir)
  for (const [name, content] of Object.entries(files)) {
    await fs.mkdir(path.join(dir, ".allternit", "panels"), { recursive: true })
    await fs.writeFile(path.join(dir, ".allternit", "panels", name), content)
  }
  return dir
}

const release = JSON.stringify({
  kind: "ops",
  title: "Release train",
  place: "pane",
  refresh: 30,
  sources: {
    ci: { url: "https://api.example.com/runs", auth: "connection:github" },
    deploys: { allternit: "computers_list" },
  },
  actions: [{ label: "Plan my day", prompt: "Propose an order for today's tickets." }],
  agent: {
    instructions: ["Run the release checklist in docs/release.md before any deploy."],
    guards: [{ tool: "bash", match: "wrangler pages deploy", ask: true }],
  },
})

describe("project panels: parse", () => {
  test("accepts an ops panel and fills defaults", () => {
    const parsed = ProjectPanels.parse(release)
    expect(parsed.error).toBeNull()
    expect(parsed.panel?.kind).toBe("ops")
    expect(parsed.panel?.place).toBe("pane")
    expect(parsed.panel?.agent?.guards[0]).toEqual({ tool: "bash", match: "wrangler pages deploy", ask: true })
  })

  test("accepts an MCP App panel", () => {
    const parsed = ProjectPanels.parse(JSON.stringify({ kind: "app", title: "Board", place: "strip", app: { connector: "linear", resource: "ui://board" } }))
    expect(parsed.error).toBeNull()
    expect(parsed.panel?.place).toBe("strip")
  })

  test("bad JSON and schema problems come back as a message, never a throw", () => {
    expect(ProjectPanels.parse("{ nope").error).toMatch(/^Not valid JSON/)
    const noTitle = ProjectPanels.parse(JSON.stringify({ kind: "ops" }))
    expect(noTitle.panel).toBeNull()
    expect(noTitle.error).toContain("title")
    const token = ProjectPanels.parse(JSON.stringify({ kind: "ops", title: "x", sources: { a: { url: "https://x.dev", auth: "ghp_secret" } } }))
    expect(token.error).toContain("connection:")
    const app = ProjectPanels.parse(JSON.stringify({ kind: "app", title: "x", app: { connector: "c", resource: "https://x" } }))
    expect(app.error).toContain("ui://")
  })

  test("file names are plain json names", () => {
    expect(ProjectPanels.validFileName("release.json")).toBe(true)
    expect(ProjectPanels.validFileName("../x.json")).toBe(false)
    expect(ProjectPanels.validFileName("a/b.json")).toBe(false)
    expect(ProjectPanels.validFileName("notes.md")).toBe(false)
  })
})

describe("project panels: guards and instructions", () => {
  const guards = [{ tool: "bash", match: "wrangler pages deploy", ask: true }]
  test("a guard matches its tool and text", () => {
    expect(ProjectPanels.guardMatches(guards, "bash", "npx wrangler pages deploy dist")).toBeDefined()
    expect(ProjectPanels.guardMatches(guards, "BASH", "WRANGLER PAGES DEPLOY")).toBeDefined()
    expect(ProjectPanels.guardMatches(guards, "bash", "ls -la")).toBeUndefined()
    expect(ProjectPanels.guardMatches(guards, "edit", "wrangler pages deploy")).toBeUndefined()
    expect(ProjectPanels.guardMatches([{ tool: "*", match: "*.env*", ask: true }], "edit", "/repo/.env.local")).toBeDefined()
    expect(ProjectPanels.guardMatches([{ ...guards[0], ask: false }], "bash", "wrangler pages deploy")).toBeUndefined()
  })

  test("only approved, valid panels add instructions", () => {
    const agent = { instructions: ["Say hi."], guards: [] }
    const blocks = ProjectPanels.instructionBlocks([
      { path: ".allternit/panels/a.json", title: "A", agent, approved: true, error: null },
      { path: ".allternit/panels/b.json", title: "B", agent, approved: false, error: null },
      { path: ".allternit/panels/c.json", title: "C", agent, approved: true, error: "bad" },
    ])
    expect(blocks).toHaveLength(1)
    expect(blocks[0]).toContain('"A"')
    expect(blocks[0]).toContain("- Say hi.")
  })
})

describe("project panels: files and approval hashing", () => {
  test("lists files with their hash; approval is tied to that exact content", async () => {
    const dir = await project({ "release.json": release, "broken.json": "{", "skip.txt": "x" })
    const listing = await ProjectPanels.list(dir)
    expect(listing.panels.map((p) => p.file)).toEqual(["broken.json", "release.json"])
    const broken = listing.panels[0]
    expect(broken.error).toMatch(/^Not valid JSON/)
    const panel = listing.panels[1]
    expect(panel.sha256).toBe(ProjectPanels.sha256(release))
    expect(panel.approved).toBe(false)

    await ProjectPanels.approve(dir, "release.json", panel.sha256)
    expect((await ProjectPanels.list(dir)).panels[1].approved).toBe(true)

    // Edited file → no longer approved, and an approval of the old hash is stale.
    await fs.writeFile(path.join(dir, ".allternit", "panels", "release.json"), release.replace("Release train", "Release"))
    expect((await ProjectPanels.list(dir)).panels[1].approved).toBe(false)
    await expect(ProjectPanels.approve(dir, "release.json", panel.sha256)).rejects.toBeInstanceOf(ProjectPanels.StaleError)

    await ProjectPanels.revoke(dir, "release.json")
    await expect(ProjectPanels.approve(dir, "../release.json", panel.sha256)).rejects.toBeInstanceOf(ProjectPanels.NotFoundError)
  })

  test("a missing panels folder is an empty list", async () => {
    const dir = await project({})
    expect((await ProjectPanels.list(dir)).panels).toEqual([])
  })

  test("findRoot walks up to the folder that holds the panels", async () => {
    const dir = await project({ "release.json": release })
    const sub = path.join(dir, "src", "deep")
    await fs.mkdir(sub, { recursive: true })
    expect(await ProjectPanels.findRoot(sub, dir)).toBe(dir)
  })

  test("a signed feed needs approval first", async () => {
    const dir = await project({ "release.json": release })
    const err = (await ProjectPanels.fetchSource(dir, "release.json", "ci").catch((e) => e)) as ProjectPanels.SourceError
    expect(err).toBeInstanceOf(ProjectPanels.SourceError)
    expect(err.status).toBe(403)
    const builtin = (await ProjectPanels.fetchSource(dir, "release.json", "deploys").catch((e) => e)) as ProjectPanels.SourceError
    expect(builtin.status).toBe(400)
  })
})

import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import { $ } from "bun"
import fs from "fs/promises"
import os from "os"
import path from "path"
import { SessionWorktree } from "../../src/runtime/primitives/worktree"

let repo: string
let scratch: string

beforeEach(async () => {
  scratch = await fs.realpath(await fs.mkdtemp(path.join(os.tmpdir(), "gizzi-worktree-")))
  repo = path.join(scratch, "repo")
  await fs.mkdir(repo)
  await $`git init -q`.cwd(repo).quiet()
  await $`git -c user.email=t@t -c user.name=t commit -q --allow-empty -m root`.cwd(repo).quiet()
  SessionWorktree.setRegistryFileForTesting(path.join(scratch, "registry.json"))
})

afterEach(async () => {
  SessionWorktree.setRegistryFileForTesting(undefined)
  await fs.rm(scratch, { recursive: true, force: true })
})

const exists = (p: string) => fs.stat(p).then(() => true, () => false)

describe("SessionWorktree.remove refusals", () => {
  test("refuses a tree another session created, and keeps it", async () => {
    const tree = await SessionWorktree.create({ cwd: repo, sessionID: "ses_a", name: "mine" })
    const result = await SessionWorktree.remove({ path: tree.path, sessionID: "ses_b" })
    expect(result).toMatchObject({ removed: false, reason: "not_created_here" })
    expect(await exists(tree.path)).toBe(true)
  })

  test("refuses a tree the registry doesn't know, even with discard", async () => {
    const outside = path.join(scratch, "outside")
    await $`git worktree add -q -b outside ${outside}`.cwd(repo).quiet()
    const result = await SessionWorktree.remove({ path: outside, sessionID: "ses_a", discard: true })
    expect(result).toMatchObject({ removed: false, reason: "not_created_here" })
    expect(await exists(outside)).toBe(true)
  })

  test("refuses a tree with uncommitted changes", async () => {
    const tree = await SessionWorktree.create({ cwd: repo, sessionID: "ses_a", name: "dirty" })
    await fs.writeFile(path.join(tree.path, "notes.txt"), "work in progress\n")
    const result = await SessionWorktree.remove({ path: tree.path, sessionID: "ses_a" })
    expect(result).toMatchObject({ removed: false, reason: "uncommitted_changes" })
    expect(await exists(path.join(tree.path, "notes.txt"))).toBe(true)
  })

  test("refuses a tree with commits that were never pushed", async () => {
    const tree = await SessionWorktree.create({ cwd: repo, sessionID: "ses_a", name: "ahead" })
    await fs.writeFile(path.join(tree.path, "a.txt"), "a\n")
    await $`git add a.txt`.cwd(tree.path).quiet()
    await $`git -c user.email=t@t -c user.name=t commit -q -m work`.cwd(tree.path).quiet()
    const result = await SessionWorktree.remove({ path: tree.path, sessionID: "ses_a" })
    expect(result).toMatchObject({ removed: false, reason: "unpushed_work" })
    expect(await exists(tree.path)).toBe(true)
  })

  test("removes a dirty tree only when the caller chose to discard", async () => {
    const tree = await SessionWorktree.create({ cwd: repo, sessionID: "ses_a", name: "discard" })
    await fs.writeFile(path.join(tree.path, "scratch.txt"), "x\n")
    const result = await SessionWorktree.remove({ path: tree.path, sessionID: "ses_a", discard: true })
    expect(result).toMatchObject({ removed: true, branch: "worktree-discard" })
    expect(await exists(tree.path)).toBe(false)
  })

  test("removes a clean tree its creator owns", async () => {
    const tree = await SessionWorktree.create({ cwd: repo, sessionID: "ses_a", name: "clean" })
    const result = await SessionWorktree.remove({ path: tree.path, sessionID: "ses_a" })
    expect(result).toMatchObject({ removed: true })
    expect(await exists(tree.path)).toBe(false)
    const branches = await $`git branch --list worktree-clean`.cwd(repo).text()
    expect(branches.trim()).toBe("")
  })
})

import { describe, expect, test } from "bun:test"
import { $ } from "bun"
import path from "path"
import { tmpdir } from "../fixture/fixture"
import { ReviewChanges } from "../../src/runtime/review/changes"

describe("review collectChanges", () => {
  test("returns null when nothing changed (no model call)", async () => {
    await using tmp = await tmpdir({ git: true })
    expect(await ReviewChanges.collect({ cwd: tmp.path })).toBeNull()
  })

  test("falls back to uncommitted git changes, including untracked and deleted files", async () => {
    await using tmp = await tmpdir({ git: true })
    await Bun.write(path.join(tmp.path, "keep.ts"), "export const a = 1\n")
    await Bun.write(path.join(tmp.path, "gone.ts"), "export const b = 2\n")
    await $`git add . && git commit -m base`.cwd(tmp.path).quiet()
    await Bun.write(path.join(tmp.path, "keep.ts"), "export const a = 2\n")
    await $`git rm -q gone.ts`.cwd(tmp.path).quiet()
    await Bun.write(path.join(tmp.path, "new.ts"), "export const c = 3\n")

    const changes = await ReviewChanges.collect({ sessionID: "ses_none", cwd: tmp.path }, { sessionDiff: async () => [] })
    expect(changes?.source).toBe("git")
    const byPath = Object.fromEntries(changes!.files.map((f) => [f.path, f.status]))
    expect(byPath).toEqual({ "keep.ts": "modified", "gone.ts": "deleted", "new.ts": "added" })
    expect(changes!.diff).toContain("export const a = 2")
    expect(changes!.diff).toContain("export const c = 3")
  })

  test("prefers files the session edited over git changes", async () => {
    await using tmp = await tmpdir({ git: true })
    await Bun.write(path.join(tmp.path, "dirty.ts"), "x\n")
    const changes = await ReviewChanges.collect(
      { sessionID: "ses_1", cwd: tmp.path },
      {
        sessionDiff: async () => [
          { file: "src/a.ts", before: "a\n", after: "b\n", status: "modified" },
          { file: "src/old.ts", before: "old\n", after: "", status: "deleted" },
          { file: "src/same.ts", before: "s\n", after: "s\n" },
        ],
      },
    )
    expect(changes?.source).toBe("session")
    expect(changes!.files).toEqual([
      { path: "src/a.ts", status: "modified" },
      { path: "src/old.ts", status: "deleted" },
    ])
  })

  test("caps the file list at 30", async () => {
    await using tmp = await tmpdir({ git: true })
    for (let i = 0; i < 35; i++) await Bun.write(path.join(tmp.path, `f${i}.txt`), `${i}\n`)
    const changes = await ReviewChanges.collect({ cwd: tmp.path })
    expect(changes!.files.length).toBe(ReviewChanges.MAX_FILES)
    expect(changes!.truncated).toBe(true)
  })
})

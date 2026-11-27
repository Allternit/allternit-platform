// @ts-nocheck
import { test, expect } from "bun:test"
import { $ } from "bun"
import fs from "fs/promises"
import path from "path"
import os from "os"
import { Snapshot } from "../../src/runtime/session/snapshot/snapshot"
import { Instance } from "../../src/runtime/context/project/instance"
import { copyOut, restoreTo, firstSnapshot } from "../../src/runtime/eval/workspace"
import { tmpdir } from "../fixture/fixture"

const msg = (snapshot?: string) => [
  { info: { id: "m1" }, parts: [{ type: "text" }, { type: "step-start", ...(snapshot ? { snapshot } : {}) }] },
]

async function repo() {
  return tmpdir({
    git: true,
    init: async (dir) => {
      await Bun.write(path.join(dir, "a.txt"), "hello-" + Math.random())
      await $`git add .`.cwd(dir).quiet()
      await $`git commit --no-gpg-sign -m init`.cwd(dir).quiet()
    },
  })
}

test("round trip restores file contents", async () => {
  await using tmp = await repo()
  await using out = await tmpdir()
  const want = await Bun.file(path.join(tmp.path, "a.txt")).text()
  await Instance.provide({
    directory: tmp.path,
    fn: async () => {
      const hash = await Snapshot.track()
      const ref = await copyOut({ projectID: Instance.project.id }, msg(hash), out.path)
      expect(ref.kind).toBe("archive")
      expect(ref.snapshot).toBe(hash)
      expect(ref.bytes).toBeGreaterThan(0)
      const dest = await fs.mkdtemp(path.join(os.tmpdir(), "gizzi-restore-"))
      await restoreTo(out.path, ref, dest)
      expect(await Bun.file(path.join(dest, "a.txt")).text()).toBe(want)
      await fs.rm(dest, { recursive: true, force: true })
    },
  })
})

test("missing hash gives pruned", async () => {
  await using tmp = await repo()
  await using out = await tmpdir()
  await Instance.provide({
    directory: tmp.path,
    fn: async () => {
      await Snapshot.track()
      const ref = await copyOut({ projectID: Instance.project.id }, msg("0".repeat(40)), out.path)
      expect(ref).toEqual({ kind: "none", reason: "pruned" })
      expect(await fs.stat(path.join(out.path, "workspace.tar.gz")).catch(() => null)).toBeNull()
    },
  })
})

test("tiny cap gives too_large and leaves no file", async () => {
  await using tmp = await repo()
  await using out = await tmpdir()
  await Instance.provide({
    directory: tmp.path,
    fn: async () => {
      const hash = await Snapshot.track()
      const ref = await copyOut({ projectID: Instance.project.id }, msg(hash), out.path, 1)
      expect(ref).toEqual({ kind: "none", reason: "too_large" })
      expect(await fs.stat(path.join(out.path, "workspace.tar.gz")).catch(() => null)).toBeNull()
    },
  })
})

test("no step-start snapshot gives no_project", async () => {
  await using tmp = await repo()
  await using out = await tmpdir()
  expect(firstSnapshot(msg())).toBeUndefined()
  await Instance.provide({
    directory: tmp.path,
    fn: async () => {
      const ref = await copyOut({ projectID: Instance.project.id }, msg(), out.path)
      expect(ref).toEqual({ kind: "none", reason: "no_project" })
    },
  })
})

test("restoreTo makes an empty dir for a trace-only case", async () => {
  const dest = path.join(os.tmpdir(), "gizzi-empty-" + Math.random().toString(36).slice(2))
  await restoreTo("/nonexistent", { kind: "none", reason: "pruned" }, dest)
  expect(await fs.readdir(dest)).toEqual([])
  await fs.rm(dest, { recursive: true, force: true })
})

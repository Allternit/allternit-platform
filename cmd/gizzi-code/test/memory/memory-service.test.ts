import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import fs from "fs/promises"
import os from "os"
import path from "path"
import { MemoryService } from "../../src/runtime/memory/memory-service"
import { MemoryDrive } from "../../src/runtime/memory/drive/drive"
import { localDriveCheckoutPath, resetDriveAccountCache } from "../../src/runtime/memory/drive/paths"
import { Instance } from "../../src/runtime/context/project/instance"
import { tmpdir } from "../fixture/fixture"

// MemoryService is now a bridge over the Memory Drive (signed out here, so
// the local-only drive): legacy name/description/type saves become one bullet
// with a stable legacy-<name> id; list/get return drive topic files.
describe("MemoryService on the Memory Drive", () => {
  let driveRoot = ""
  beforeEach(async () => {
    driveRoot = await fs.mkdtemp(path.join(os.tmpdir(), "gizzi-ms-"))
    process.env.GIZZI_MEMORY_DRIVE_ROOT = driveRoot
    delete process.env.ALLTERNIT_API_TOKEN
    resetDriveAccountCache()
    MemoryDrive.resetForTests()
  })
  afterEach(async () => {
    delete process.env.GIZZI_MEMORY_DRIVE_ROOT
    MemoryDrive.resetForTests()
    await fs.rm(driveRoot, { recursive: true, force: true })
  })

  test("save → one bullet in the type's topic file; same name updates; list/get/search/remove", async () => {
    await using project = await tmpdir()
    await Instance.provide({
      directory: project.path,
      fn: async () => {
        const saved = await MemoryService.save({ name: "user_role", description: "The user's role", type: "user" }, "Staff engineer, prefers terse answers.")
        const dir = localDriveCheckoutPath()
        expect(saved.filename).toBe("user.md")
        expect(saved.filepath).toBe(path.join(dir, "user.md"))
        const onDisk = await fs.readFile(saved.filepath, "utf8")
        expect(onDisk).toContain("- The user's role — Staff engineer, prefers terse answers. [source: gizzi:session/unknown; added: ")
        expect(onDisk).toContain("id: legacy-user_role")

        await MemoryService.save({ name: "user_role", description: "The user's role", type: "user" }, "Principal engineer.")
        const updated = await fs.readFile(saved.filepath, "utf8")
        expect(updated.match(/legacy-user_role/g)?.length).toBe(1)
        expect(updated).toContain("Principal engineer.")

        const all = await MemoryService.list()
        expect(all.map((e) => e.filename)).toEqual(["user.md"])
        expect(all[0]!.description).toContain("1 memory")
        expect((await MemoryService.get("user"))?.body).toContain("Principal engineer.")
        expect((await MemoryService.search("principal"))[0]?.name).toBe("legacy-user_role")

        expect(await MemoryService.remove("user_role")).toBe(true)
        expect(await fs.readFile(saved.filepath, "utf8")).not.toContain("legacy-user_role")
        await MemoryService.save({ name: "proj", description: "Ships weekly", type: "project" }, "")
        const projectTopic = (await MemoryService.list()).find((e) => e.filename.startsWith("projects/"))!
        expect(await MemoryService.remove(projectTopic.filename)).toBe(true)
        expect(await MemoryService.get(projectTopic.filename)).toBeNull()
      },
    })
  })

  test("secrets are refused and nothing is committed", async () => {
    await using project = await tmpdir()
    await Instance.provide({
      directory: project.path,
      fn: async () => {
        await expect(
          MemoryService.save({ name: "aws", description: "aws key", type: "reference" }, "AKIAABCDEFGHIJKLMNOP"),
        ).rejects.toThrow(/credential/)
        expect(await MemoryService.list()).toEqual([])
      },
    })
  })
})

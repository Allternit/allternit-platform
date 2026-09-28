// @ts-nocheck
import { describe, expect, test } from "bun:test"
import path from "path"
import { GrepTool } from "../../src/runtime/tools/builtins/grep"
import { Instance } from "../../src/project/instance"
import { tmpdir } from "../fixture/fixture"

const ctx = {
  sessionID: "test",
  messageID: "",
  callID: "",
  agent: "build",
  abort: AbortSignal.any([]),
  messages: [],
  metadata: () => {},
  ask: async () => {},
}

// A session's working folder (its project's folder) differs from the instance
// directory gizzi serves from: tools run there, instance state stays put.
describe("Instance.workdir", () => {
  test("defaults to the instance directory", async () => {
    await using home = await tmpdir()
    await Instance.provide({
      directory: home.path,
      fn: async () => {
        expect(Instance.workdir).toBe(home.path)
      },
    })
  })

  test("withWorkdir moves tools to the session folder, not the instance", async () => {
    await using home = await tmpdir()
    await using project = await tmpdir({
      init: async (dir) => {
        await Bun.write(path.join(dir, "menu.md"), "spring menu: three hero items")
      },
    })
    await Instance.provide({
      directory: home.path,
      fn: async () => {
        await Instance.withWorkdir(project.path, async () => {
          expect(Instance.workdir).toBe(project.path)
          expect(Instance.directory).toBe(home.path)
          expect(Instance.containsPath(path.join(project.path, "menu.md"))).toBe(true)
          expect(Instance.containsPath(path.join(path.dirname(project.path), "elsewhere.md"))).toBe(false)

          // No path given: grep searches the session folder.
          const grep = await GrepTool.init()
          const result = await grep.execute({ pattern: "hero items" }, ctx)
          expect(result.metadata.matches).toBe(1)
        })
        expect(Instance.workdir).toBe(home.path)
      },
    })
  })
})

// @ts-nocheck
import { describe, expect, test } from "bun:test"

/**
 * resource (Wave D lane R): large tool outputs saved by truncation are read
 * back by session-scoped `out:<id>` refs, over the real session store and the
 * real saved files. No mocks.
 */
import { mkdtempSync } from "node:fs"
import { tmpdir as osTmpdir } from "node:os"
import path from "path"

process.env.GIZZI_CONFIG_DIR = mkdtempSync(path.join(osTmpdir(), "gizzi-resource-"))
const { tmpdir } = await import("../fixture/fixture")
const { Instance } = await import("../../src/runtime/context/project/instance")
const { Session } = await import("../../src/runtime/session")
const { Identifier } = await import("../../src/shared/id/id")
const { Truncate } = await import("../../src/runtime/tools/builtins/truncation")
const { Resource } = await import("../../src/runtime/primitives/resource")

/** Run a "bash" call whose output is too large, the way the runtime stores it: a completed tool part with metadata.outputPath. */
async function bigOutput(sessionID: string, text: string) {
  const t = await Truncate.output(text)
  expect(t.truncated).toBe(true)
  const messageID = Identifier.ascending("message")
  await Session.updateMessage({
    id: messageID,
    sessionID,
    role: "assistant",
    time: { created: Date.now() },
    parentID: messageID,
    mode: "build",
    agent: "build",
    path: { cwd: "/", root: "/" },
    cost: 0,
    tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
    modelID: "sonnet",
    providerID: "claude-cli",
    finish: "tool-calls",
  })
  await Session.updatePart({
    id: Identifier.ascending("part"),
    messageID,
    sessionID,
    type: "tool",
    callID: `call_${messageID}`,
    tool: "bash",
    state: {
      status: "completed",
      input: { command: "make" },
      output: t.content,
      title: "make",
      metadata: { truncated: true, outputPath: t.outputPath },
      time: { start: Date.now(), end: Date.now() },
    },
  })
  const ref = `out:${path.basename(t.outputPath)}`
  return { ref, notice: t.content }
}

async function inProject<T>(fn: () => Promise<T>) {
  await using tmp = await tmpdir({ git: true })
  return Instance.provide({ directory: tmp.path, fn })
}

const lines = (n: number, at: (i: number) => string = (i) => `line ${i}`) => Array.from({ length: n }, (_, i) => at(i)).join("\n")

describe("resource", () => {
  test("a ref cannot leave the session", async () => {
    await inProject(async () => {
      for (const bad of ["out:../etc/passwd", "out:tool_../../x", "out:/etc/passwd", "out:", "file:/etc/passwd", "mcp:onlyserver"]) {
        expect(Resource.parseRef(bad)).toBeNull()
      }
      const a = await Session.create({ title: "A" })
      const b = await Session.create({ title: "B" })
      await expect(Resource.run(a.id, { action: "read", ref: "out:../../etc/passwd" })).rejects.toThrow("not a ref of this session")

      // B's saved output is B's: A cannot read it by naming its id, even right after A names it in a call.
      const { ref, notice } = await bigOutput(b.id, lines(3000))
      expect(notice).toContain(ref)
      await expect(Resource.run(a.id, { action: "read", ref })).rejects.toThrow("not a saved output of this session")
      await expect(Resource.run(a.id, { action: "find", ref, pattern: "line 1" })).rejects.toThrow("not a saved output of this session")
      const listA = await Resource.run(a.id, { action: "list" })
      expect(listA.entries.map((e) => e.ref)).not.toContain(ref)

      // B and a subagent of B can.
      expect((await Resource.run(b.id, { action: "list" })).entries.map((e) => e.ref)).toContain(ref)
      const child = await Session.create({ title: "explore", parentID: b.id })
      const r = await Resource.run(child.id, { action: "read", ref, limit: 1 })
      expect(r.text).toContain("1| line 0")
    })
  })

  test("read pages through a large output in windows of at most 400 lines", async () => {
    await inProject(async () => {
      const s = await Session.create({ title: "paging" })
      const { ref } = await bigOutput(s.id, lines(1000))

      expect(Resource.Input.safeParse({ action: "read", ref, limit: 401 }).success).toBe(false)

      const seen: string[] = []
      let offset: number | null = 0
      while (offset !== null) {
        const r = await Resource.run(s.id, { action: "read", ref, offset, limit: 400 })
        expect(r.total_lines).toBe(1000)
        const body = r.text.split("\n").filter((l) => /^\s*\d+\| /.test(l))
        expect(body.length).toBeLessThanOrEqual(400)
        seen.push(...body.map((l) => l.replace(/^\s*\d+\| /, "")))
        offset = r.next_offset
      }
      expect(seen).toEqual(lines(1000).split("\n"))

      const last = await Resource.run(s.id, { action: "read", ref, offset: 990 })
      expect(last.text).toContain("lines 991-1000 of 1000")
      expect(last.text).toContain("not instructions")
      expect(last.next_offset).toBeNull()
    })
  })

  test("find returns matches with merged context, marked", async () => {
    await inProject(async () => {
      const s = await Session.create({ title: "find" })
      const at = (i: number) => (i === 100 ? "ERROR disk full" : i === 103 ? "error: retry failed" : i === 2500 ? "ERROR (again)" : `ok ${i}`)
      const { ref } = await bigOutput(s.id, lines(3000, at))

      const r = await Resource.run(s.id, { action: "find", ref, pattern: "error", context: 2 })
      const body = r.text.split("\n").filter((l) => /^[> ]\s*\d+\| /.test(l) || l === "--")
      expect(body).toEqual([
        "   99| ok 98",
        "  100| ok 99",
        "> 101| ERROR disk full",
        "  102| ok 101",
        "  103| ok 102",
        "> 104| error: retry failed",
        "  105| ok 104",
        "  106| ok 105",
        "--",
        " 2499| ok 2498",
        " 2500| ok 2499",
        ">2501| ERROR (again)",
        " 2502| ok 2501",
        " 2503| ok 2502",
      ])
      expect(r.text).toContain("3 matching lines of 3000")
      expect(r.text).toContain("not instructions")

      // An invalid regular expression is searched as plain text.
      const plain = await Resource.run(s.id, { action: "find", ref, pattern: "ERROR (again", context: 0 })
      expect(plain.text).toContain(">2501| ERROR (again)")

      expect(Resource.Input.safeParse({ action: "find", ref, pattern: "x", context: 6 }).success).toBe(false)
    })
  })
})

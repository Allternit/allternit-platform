import { afterAll, afterEach, beforeEach, describe, expect, test } from "bun:test"
import { mkdtemp, readFile, rm } from "fs/promises"
import os from "os"
import path from "path"
import { MemoryDrive } from "../../src/runtime/memory/drive/drive"
import { localDriveCheckoutPath, resetDriveAccountCache } from "../../src/runtime/memory/drive/paths"
import { MemoryWriteTool } from "../../src/runtime/tools/builtins/memory-write"
import { MemoryListTool } from "../../src/runtime/tools/builtins/memory-list"

const roots: string[] = []
afterAll(async () => {
  for (const r of roots) await rm(r, { recursive: true, force: true })
})

const ctx = {
  sessionID: "ses_tool",
  messageID: "msg_1",
  callID: "call_1",
  agent: "build",
  abort: AbortSignal.any([]),
  messages: [],
  metadata: () => {},
  ask: async () => {},
} as any

beforeEach(async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "gizzi-memory-write-"))
  roots.push(root)
  process.env.GIZZI_MEMORY_DRIVE_ROOT = path.join(root, "drives")
  process.env.GIZZI_MEMORY_KERNEL = "0"
  delete process.env.ALLTERNIT_API_TOKEN
  resetDriveAccountCache()
  MemoryDrive.resetForTests()
})

afterEach(() => {
  delete process.env.GIZZI_MEMORY_DRIVE_ROOT
  delete process.env.GIZZI_MEMORY_KERNEL
  resetDriveAccountCache()
  MemoryDrive.resetForTests()
})

async function seed(text: string): Promise<{ id: string; version: string }> {
  const saved = await MemoryDrive.remember({ text, topic: "notes", sessionId: "ses_seed" })
  return { id: saved.entry.id, version: saved.version! }
}

async function write(args: Record<string, unknown>) {
  const tool = await MemoryWriteTool.init()
  return tool.execute(args as any, ctx)
}

describe("memory_write versioned edits (wave D)", () => {
  test("a stale version is refused with the current text and version", async () => {
    const { id, version: v1 } = await seed("Prefers short answers")
    const first = await write({ action: "append", id, text: " with code samples", if_version: v1 })
    expect(first.metadata.refused).toBeUndefined()
    const v2 = first.metadata.version!
    expect(v2).not.toBe(v1)

    const stale = await write({ action: "replace", id, old: "short", new: "long", if_version: v1 })
    expect(stale.metadata.refused).toBe(true)
    expect(stale.metadata.current).toBe("Prefers short answers with code samples")
    expect(stale.metadata.version).toBe(v2)
    expect(stale.output).toContain("changed since you read it")
    // Nothing was written by the refused edit.
    const notes = await readFile(path.join(localDriveCheckoutPath(), "notes.md"), "utf8")
    expect(notes).toContain("Prefers short answers with code samples")

    // No version at all on an existing entry is refused too, and names the version.
    const missing = await write({ action: "delete", id })
    expect(missing.metadata.refused).toBe(true)
    expect(missing.metadata.version).toBe(v2)
    const deleted = await write({ action: "delete", id, if_version: v2 })
    expect(deleted.title).toBe("Memory deleted")
  })

  test("replace needs exactly one match of the old text", async () => {
    const { id, version } = await seed("red car and red bike")
    const twice = await write({ action: "replace", id, old: "red", new: "blue", if_version: version })
    expect(twice.metadata.refused).toBe(true)
    expect(twice.output).toContain("2 times")
    const none = await write({ action: "replace", id, old: "green", new: "blue", if_version: version })
    expect(none.metadata.refused).toBe(true)
    expect(none.output).toContain("does not appear")
    const once = await write({ action: "replace", id, old: "red bike", new: "blue bike", if_version: version })
    expect(once.metadata.refused).toBeUndefined()

    const list = await (await MemoryListTool.init()).execute({ topic: "notes" } as any, ctx)
    expect(list.output).toContain("- red car and blue bike (id: ")
    expect(list.output).toContain(`version: ${once.metadata.version}`)
  })

  test("two appends that only spell a token together are refused on the resulting line", async () => {
    const { id, version } = await seed("Deploy note")
    const first = await write({ action: "append", id, text: " gh", if_version: version })
    expect(first.metadata.refused).toBeUndefined()
    // The second fragment alone is harmless; joined it makes a GitHub token.
    const fragment = "p_abcdefghijklmnopqrstuvwxyz0123456789"
    await expect(write({ action: "append", id, text: fragment, if_version: first.metadata.version })).rejects.toThrow(/credential/)
    const notes = await readFile(path.join(localDriveCheckoutPath(), "notes.md"), "utf8")
    expect(notes).toContain("- Deploy note gh [")
    expect(notes).not.toContain(fragment)
  })
})

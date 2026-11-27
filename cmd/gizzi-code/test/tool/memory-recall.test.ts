import { afterAll, beforeEach, afterEach, describe, expect, test } from "bun:test"
import { mkdtemp, rm } from "fs/promises"
import os from "os"
import path from "path"
import { MemoryDrive } from "../../src/runtime/memory/drive/drive"
import { resetDriveAccountCache } from "../../src/runtime/memory/drive/paths"
import { MemoryKernelAdapter } from "../../src/runtime/memory/kernel-adapter"
import { MemoryRecallTool } from "../../src/runtime/tools/builtins/memory-recall"

const roots: string[] = []
afterAll(async () => {
  for (const r of roots) await rm(r, { recursive: true, force: true })
})
const ctx = { sessionID: "ses_t", messageID: "m", callID: "c", agent: "build", abort: AbortSignal.any([]), messages: [], metadata: () => {}, ask: async () => {} } as any

const t = (n: number) => ({ observation_id: `obs_${n}`, session_id: "ses_old", speaker: n % 2 ? "user" : "assistant", timestamp: `2026-11-0${n}T10:00:00Z`, text: `turn ${n}` })
let calls: { method: string; path: string; body: any }[] = []

beforeEach(async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "gizzi-recall-"))
  roots.push(root)
  process.env.GIZZI_MEMORY_DRIVE_ROOT = path.join(root, "drives")
  process.env.GIZZI_MEMORY_KERNEL = "1"
  delete process.env.ALLTERNIT_API_TOKEN
  resetDriveAccountCache()
  MemoryDrive.resetForTests()
  calls = []
  MemoryKernelAdapter.setRequestForTests((async (method: string, p: string, body?: unknown) => {
    calls.push({ method, path: p, body })
    if (p.includes("/neighbors")) return { turns: [t(1), t(2), t(3)] }
    if (p.includes("raw-search")) return { turns: [t(4)] }
    return { items: [] }
  }) as any)
})
afterEach(() => {
  MemoryKernelAdapter.setRequestForTests()
  delete process.env.GIZZI_MEMORY_DRIVE_ROOT
  delete process.env.GIZZI_MEMORY_KERNEL
  MemoryDrive.resetForTests()
})

describe("memory_recall past chats", () => {
  test("from, to, session and skip_sessions reach raw-search; hits listed after memory hits", async () => {
    const tool = await MemoryRecallTool.init()
    const r = await tool.execute(
      { query: "reranker", from: "2026-11-01", to: "2026-11-27", session: "ses_a", skip_sessions: ["ses_b"] },
      ctx,
    )
    const raw = calls.find((c) => c.path.includes("raw-search"))!
    expect(raw.body).toMatchObject({ mode: "search", from: "2026-11-01", to: "2026-11-27", session_id: "ses_a", exclude_sessions: ["ses_b"] })
    expect(r.output).toContain("Past chats")
    expect(r.output).toContain("[2026-11-04 · assistant; id: obs_4; session: ses_old] turn 4")
  })

  test("neighbors_of returns the neighbouring turns in order and does not search", async () => {
    const tool = await MemoryRecallTool.init()
    const r = await tool.execute({ query: "", neighbors_of: "obs_2" }, ctx)
    expect(calls).toHaveLength(1)
    expect(calls[0].path).toBe("/api/v1/memory/v2/turns/obs_2/neighbors?before=2&after=2")
    const lines = r.output.split("\n")
    expect(lines.map((l: string) => l.match(/obs_\d/)![0])).toEqual(["obs_1", "obs_2", "obs_3"])
  })
})

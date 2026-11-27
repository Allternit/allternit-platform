import { describe, expect, test } from "bun:test"
import fs from "fs/promises"
import os from "os"
import path from "path"
import { spawnSync } from "child_process"
import { redactForExport, redactPrivate, scanWorkspaceTar, type TraceEntry } from "@/runtime/eval/redact"

const SK = "sk-abcdefghijklmnopqrstuvwx1234"
const GH = "ghp_abcdefghijklmnopqrstuvwxyz0123456789"

function entry(sequence: number, kind: string, data: unknown): TraceEntry {
  return { sequence, kind, data }
}

describe("eval redaction", () => {
  test("private save removes planted keys from tool output and prompts", () => {
    const entries = [
      entry(1, "part.updated", { part: { type: "tool", state: { output: `found ${SK} and ${GH} in env` } } }),
    ]
    const turns = [{ role: "user", text: `use key ${SK} and token ${GH} please` }]
    const r = redactPrivate(entries, turns)
    const all = JSON.stringify(r)
    expect(all).not.toContain(SK)
    expect(all).not.toContain(GH)
    expect(r.turns[0].text).toContain("please")
    expect(r.report.secrets).toBe(2)
  })

  test("part.delta entries are dropped and counted", () => {
    const entries = [
      entry(1, "part.delta", { delta: "a" }),
      entry(2, "part.delta", { delta: "b" }),
      entry(3, "request.completed", {}),
    ]
    const r = redactPrivate(entries, [])
    expect(r.entries.map((e) => e.kind)).toEqual(["request.completed"])
    expect(r.report.entries_dropped).toBe(2)
  })

  test("cap keeps the first entries and the final request entries", () => {
    const entries: TraceEntry[] = []
    for (let i = 1; i <= 6_000; i++) entries.push(entry(i, "message.updated", {}))
    entries.push(entry(6_001, "request.completed", {}))
    const r = redactPrivate(entries, [])
    expect(r.entries.length).toBe(5_000)
    expect(r.entries[0].sequence).toBe(1)
    expect(r.entries.at(-1)?.kind).toBe("request.completed")
    expect(r.report.entries_dropped).toBe(1_001)
  })

  test("export turns the home path into ~, hides emails and odd hosts, truncates output", () => {
    const home = os.homedir()
    const entries = [
      entry(1, "part.updated", {
        part: {
          state: {
            output:
              `${home}/proj/a.ts /Users/someone/x /home/other/y mail me@corp.io ` +
              `https://github.com/a/b https://internal.corp.io/x ` +
              "z".repeat(3_000),
          },
        },
      }),
    ]
    const r = redactForExport(entries, [{ role: "user", text: `edit ${home}/proj/a.ts` }])
    const out = (r.entries[0].data as any).part.state.output as string
    expect(out).toContain("~/proj/a.ts")
    expect(out).not.toContain(home)
    expect(out).not.toContain("/Users/someone")
    expect(out).not.toContain("/home/other")
    expect(out).not.toContain("me@corp.io")
    expect(out).toContain("https://github.com/a/b")
    expect(out).not.toContain("internal.corp.io")
    expect(out.length).toBeLessThanOrEqual(2_000)
    expect(r.turns[0].text).toBe("edit ~/proj/a.ts")
    expect(r.report.emails).toBe(1)
    expect(r.report.hosts).toBe(1)
    expect(r.report.truncated).toBe(1)
    expect(r.report.paths).toBeGreaterThanOrEqual(3)
  })

  test("tar scan finds a planted key and passes a clean tar", async () => {
    const dir = await fs.mkdtemp(path.join(os.tmpdir(), "redact-tar-"))
    try {
      const src = path.join(dir, "src")
      await fs.mkdir(src)
      await fs.writeFile(path.join(src, "clean.txt"), "nothing here\n")
      await fs.writeFile(path.join(src, ".env"), `OPENAI=${SK}\n`)
      const dirty = path.join(dir, "dirty.tar.gz")
      expect(spawnSync("tar", ["-czf", dirty, "-C", src, "."]).status).toBe(0)
      const hit = await scanWorkspaceTar(dirty)
      expect(hit.hits.length).toBe(1)
      expect(hit.hits[0].file).toContain(".env")
      expect(hit.hits[0].kind).toBe("api_key")

      await fs.rm(path.join(src, ".env"))
      const clean = path.join(dir, "clean.tar.gz")
      expect(spawnSync("tar", ["-czf", clean, "-C", src, "."]).status).toBe(0)
      expect((await scanWorkspaceTar(clean)).hits).toEqual([])
    } finally {
      await fs.rm(dir, { recursive: true, force: true })
    }
  })
})

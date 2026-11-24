import { afterAll, describe, expect, test } from "bun:test"
import fs from "fs/promises"
import { mkdtempSync } from "node:fs"
import os from "node:os"
import path from "path"
import { formatRow, parseCsv } from "../../src/runtime/primitives/batch/csv"
import { validateReply, type Field } from "../../src/runtime/primitives/batch/schema"
import {
  hashText,
  prepareBatch,
  progressPath,
  resolveCsvPath,
  resolveOutputPath,
  runBatch,
  runSample,
  type BatchRow,
  type BatchWorker,
} from "../../src/runtime/primitives/batch/run"

/**
 * agent_batch's core with the model edge faked: each worker is a function
 * from (row, prompt, turn number) to the reply text. CSV parsing, schema
 * checks, the scheduler, the output file, the progress sidecar and the path
 * rules are all real.
 */

const dirs: string[] = []
const tmp = () => {
  const dir = mkdtempSync(path.join(os.tmpdir(), "gizzi-agent-batch-"))
  dirs.push(dir)
  return dir
}
afterAll(async () => {
  for (const dir of dirs) await fs.rm(dir, { recursive: true, force: true })
})

const fields: Field[] = [
  { name: "category", type: "enum", values: ["bug", "question"] },
  { name: "score", type: "number" },
  { name: "note", type: "string", optional: true },
]

type Sent = { id: string; prompt: string; turn: number }

function worker(reply: (row: BatchRow, prompt: string, turn: number) => string | Promise<string>, sent: Sent[] = []): BatchWorker {
  return {
    async start({ row }) {
      let turn = 0
      return {
        runID: `ses_${row.id}`,
        async send(prompt) {
          turn += 1
          sent.push({ id: row.id, prompt, turn })
          return { text: await reply(row, prompt, turn), tokens: 100, cost: 0.002 }
        },
      }
    },
  }
}

const ok = (row: BatchRow) => JSON.stringify({ category: "bug", score: Number(row.record.n ?? 1), note: `row ${row.id}` })
const fast = { scheduler: { rampIntervalMs: 1, retryBaseMs: 1 } }

const CSV = "id,text,n\r\na,first,1\r\nb,second,2\r\nc,third,3\r\n"

function plan(text = CSV, extra: Partial<Parameters<typeof prepareBatch>[0]> = {}) {
  return prepareBatch({ text, instruction: "Classify {text}.", fields, idColumn: "id", ...extra })
}

async function readOutput(file: string) {
  return parseCsv(await fs.readFile(file, "utf8"))
}

describe("csv", () => {
  test("quotes, doubled quotes, commas and line breaks inside quotes, BOM", () => {
    const text = '﻿name,quote\n"Smith, Jo","She said ""hi""\nthen left"\r\nPlain,"multi\r\nline"\n'
    const table = parseCsv(text)
    expect(table.bom).toBe(true)
    expect(table.header).toEqual(["name", "quote"])
    expect(table.rows).toEqual([
      ["Smith, Jo", 'She said "hi"\nthen left'],
      ["Plain", "multi\r\nline"],
    ])
    // The second record starts on line 4: the first one spans lines 2 and 3.
    expect(table.lines).toEqual([2, 4])
  })

  test("no trailing newline, blank lines skipped, short rows padded", () => {
    const table = parseCsv("a,b,c\n\n1,2\n\n3,4,5")
    expect(table.rows).toEqual([
      ["1", "2", ""],
      ["3", "4", "5"],
    ])
  })

  test("an unclosed quote and an overlong row are errors naming the line", () => {
    expect(() => parseCsv('a,b\n1,"open\n2,3\n')).toThrow(/line 2/)
    expect(() => parseCsv("a,b\n1,2,3\n")).toThrow(/Line 2 has 3 fields/)
    expect(() => parseCsv("a,a\n1,2\n")).toThrow(/twice/)
  })

  test("written rows read back the same", () => {
    const values = ["x, y", 'say "no"', "two\nlines", " padded ", "plain"]
    const table = parseCsv(formatRow(["a", "b", "c", "d", "e"]) + formatRow(values))
    expect(table.rows[0]).toEqual(values)
  })
})

describe("schema", () => {
  test("types, enum values and optional fields are checked", () => {
    expect(validateReply(fields, '{"category":"bug","score":3}')).toEqual({
      ok: true,
      values: { category: "bug", score: 3, note: null },
    })
    expect(validateReply(fields, '```json\n{"category":"question","score":1,"note":"x"}\n```').ok).toBe(true)
    const bad = validateReply(fields, '{"category":"feature","score":"3"}')
    expect(bad.ok).toBe(false)
    if (bad.ok === false) {
      expect(bad.error).toContain('"category" must be one of "bug", "question"')
      expect(bad.error).toContain('"score" must be a number')
    }
    expect(validateReply(fields, "Sure! Here you go.").ok).toBe(false)
    expect(validateReply(fields, "[1,2]").ok).toBe(false)
  })

  test("a bad reply gets exactly one retry that names the problem", async () => {
    const sent: Sent[] = []
    const p = plan("id,text,n\na,first,1\n")
    const result = await runSample(p, {
      ...fast,
      worker: worker((row, _prompt, turn) => (turn === 1 ? '{"category":"feature"}' : ok(row)), sent),
    })
    expect(result.outcomes[0]!.status).toBe("ok")
    expect(sent.length).toBe(2)
    expect(sent[1]!.prompt).toContain('"category" must be one of')
    expect(sent[1]!.prompt).toContain('"score" is missing')
  })

  test("a reply still wrong after the retry fails the row, with no third try", async () => {
    const sent: Sent[] = []
    const dir = tmp()
    const out = path.join(dir, "out.csv")
    const p = plan("id,text,n\na,first,1\n")
    const result = await runBatch(p, {
      ...fast,
      worker: worker(() => "not json", sent),
      outputPath: out,
      csvPath: path.join(dir, "in.csv"),
      csvHash: "h",
    })
    expect(sent.length).toBe(2)
    expect(result.failed).toBe(1)
    const table = await readOutput(out)
    expect(table.rows[0]!.slice(-2)).toEqual(["failed", expect.stringContaining("still wrong after one retry")])
  })

  test("instruction placeholders and column clashes are checked up front", () => {
    expect(() => plan(CSV, { instruction: "Classify {txt}." })).toThrow(/\{txt\}.*no such column/)
    expect(() => plan(CSV, { instruction: "Classify this." })).toThrow(/at least one \{column\}/)
    // JSON in braces is left alone.
    expect(plan(CSV, { instruction: 'Classify {text}. Example: {"category": "bug"}' }).rows.length).toBe(3)
    expect(() => plan(CSV, { fields: [{ name: "text", type: "string" }] })).toThrow(/already is a CSV column/)
    expect(() => plan("id,text\na,x\na,y\n")).toThrow(/appears twice/)
  })
})

describe("run", () => {
  test("output = input + schema + _status + _error; model text can't become a formula", async () => {
    const dir = tmp()
    const out = path.join(dir, "out.csv")
    const result = await runBatch(plan(), {
      ...fast,
      worker: worker((row) => JSON.stringify({ category: "bug", score: 1, note: row.id === "a" ? "=HYPERLINK(1)" : "fine" })),
      outputPath: out,
      csvPath: path.join(dir, "in.csv"),
      csvHash: hashText(CSV),
    })
    expect(result).toMatchObject({ done: 3, failed: 0, skipped: 0, tokens: 300 })
    const table = await readOutput(out)
    expect(table.header).toEqual(["id", "text", "n", "category", "score", "note", "_status", "_error"])
    const a = table.rows.find((r) => r[0] === "a")!
    expect(a).toEqual(["a", "first", "1", "bug", "1", "'=HYPERLINK(1)", "ok", ""])
  })

  test("a re-run skips rows that finished ok and runs the failed ones again", async () => {
    const dir = tmp()
    const out = path.join(dir, "out.csv")
    const opts = { outputPath: out, csvPath: path.join(dir, "in.csv"), csvHash: hashText(CSV) }

    const first = await runBatch(plan(), {
      ...fast,
      ...opts,
      worker: worker((row) => (row.id === "b" ? Promise.reject(new Error("tool crashed")) : ok(row))),
    })
    expect(first).toMatchObject({ done: 2, failed: 1 })
    const progress = JSON.parse(await fs.readFile(progressPath(out), "utf8"))
    expect(progress.rows.map((r: any) => `${r.id}:${r.status}`).sort()).toEqual(["a:ok", "b:failed", "c:ok"])

    const sent: Sent[] = []
    const second = await runBatch(plan(), { ...fast, ...opts, worker: worker(ok, sent) })
    expect(sent.map((s) => s.id)).toEqual(["b"])
    expect(second).toMatchObject({ done: 1, failed: 0, skipped: 2 })
    const table = await readOutput(out)
    expect(table.rows.map((r) => `${r[0]}:${r.at(-2)}`).sort()).toEqual(["a:ok", "b:ok", "c:ok"])
  })

  test("a cancelled run leaves unfinished rows for the next run", async () => {
    const dir = tmp()
    const out = path.join(dir, "out.csv")
    const opts = { outputPath: out, csvPath: path.join(dir, "in.csv"), csvHash: hashText(CSV) }
    const abort = new AbortController()
    const first = await runBatch(plan(), {
      ...fast,
      ...opts,
      maxConcurrency: 1,
      signal: abort.signal,
      worker: worker(async (row) => {
        if (row.id === "a") return ok(row)
        abort.abort()
        return new Promise<string>(() => {})
      }),
    })
    expect(first).toMatchObject({ done: 1, cancelled: true })
    const sent: Sent[] = []
    await runBatch(plan(), { ...fast, ...opts, worker: worker(ok, sent) })
    expect(sent.map((s) => s.id).sort()).toEqual(["b", "c"])
  })

  test("without id_column, a changed CSV can't be resumed", async () => {
    const dir = tmp()
    const out = path.join(dir, "out.csv")
    const noId = (text: string) => prepareBatch({ text, instruction: "Classify {text}.", fields })
    await runBatch(noId(CSV), { ...fast, worker: worker(ok), outputPath: out, csvPath: "in.csv", csvHash: hashText(CSV) })
    const changed = CSV + "d,fourth,4\r\n"
    await expect(
      runBatch(noId(changed), { ...fast, worker: worker(ok), outputPath: out, csvPath: "in.csv", csvHash: hashText(changed) }),
    ).rejects.toThrow(/CSV changed/)
  })

  test("an existing output without a progress file is never overwritten", async () => {
    const dir = tmp()
    const out = path.join(dir, "out.csv")
    await fs.writeFile(out, "keep me")
    await expect(
      runBatch(plan(), { ...fast, worker: worker(ok), outputPath: out, csvPath: "in.csv", csvHash: "h" }),
    ).rejects.toThrow(/won't be overwritten/)
    expect(await fs.readFile(out, "utf8")).toBe("keep me")
  })

  test("a row past max_runtime_seconds fails as timed out", async () => {
    const p = plan("id,text,n\na,first,1\n")
    const result = await runSample(p, { ...fast, maxRuntimeSeconds: 1, worker: worker(() => new Promise<string>(() => {})) })
    expect(result.outcomes[0]).toMatchObject({ status: "failed", error: "Timed out after 1 s." })
  })

  test("sample runs at most 3 rows and estimates the full run", async () => {
    const rows = Array.from({ length: 10 }, (_, i) => `r${i},t${i},${i}`).join("\n")
    const p = plan(`id,text,n\n${rows}\n`)
    const sent: Sent[] = []
    const result = await runSample(p, { ...fast, maxConcurrency: 2, worker: worker(ok, sent) })
    expect(sent.length).toBe(3)
    expect(result.estimate).toMatchObject({ rows: 10, tokens: 1000 })
    expect(result.estimate!.cost).toBeCloseTo(0.02, 6)
  })
})

describe("paths", () => {
  test("the CSV must be inside a root, not a credential, symlinks followed", async () => {
    const root = tmp()
    const outside = tmp()
    await fs.writeFile(path.join(outside, "data.csv"), CSV)
    await fs.symlink(path.join(outside, "data.csv"), path.join(root, "link.csv"))
    await fs.writeFile(path.join(root, ".env"), "SECRET=1")
    await fs.writeFile(path.join(root, "in.csv"), CSV)
    const ctx = { roots: [root] }
    await expect(resolveCsvPath("link.csv", ctx)).rejects.toThrow(/outside the project/)
    await expect(resolveCsvPath(".env", ctx)).rejects.toThrow(/credential/)
    await expect(resolveCsvPath("missing.csv", ctx)).rejects.toThrow(/no file/)
    expect(await resolveCsvPath("in.csv", ctx)).toBe(path.join(await fs.realpath(root), "in.csv"))
  })

  test("the output must be a .csv inside a root, not the input, not via a symlink out", async () => {
    const root = tmp()
    const outside = tmp()
    const ctx = { roots: [root] }
    await fs.writeFile(path.join(root, "in.csv"), CSV)
    const input = await resolveCsvPath("in.csv", ctx)
    expect(await resolveOutputPath(undefined, input, ctx)).toBe(path.join(path.dirname(input), "in.results.csv"))
    await expect(resolveOutputPath(path.join(outside, "out.csv"), input, ctx)).rejects.toThrow(/outside the project/)
    await expect(resolveOutputPath("out.txt", input, ctx)).rejects.toThrow(/must end in \.csv/)
    await expect(resolveOutputPath("in.csv", input, ctx)).rejects.toThrow(/is the input CSV/)
    await expect(resolveOutputPath("nope/out.csv", input, ctx)).rejects.toThrow(/doesn't exist/)
    await fs.symlink(path.join(outside, "x.csv"), path.join(root, "sneaky.csv"))
    await fs.writeFile(path.join(outside, "x.csv"), "")
    await expect(resolveOutputPath("sneaky.csv", input, ctx)).rejects.toThrow(/outside the project/)
  })
})

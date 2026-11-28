// File checks for live eval attempts (CONTRACT.md section 3, `checks[]`).
import fs from "fs/promises"
import path from "path"
import { spawn } from "child_process"
import type { Check } from "./schema"

export const TESTS_TIMEOUT_MS = 5 * 60_000

export interface CheckResult {
  type: Check["type"]
  path?: string
  pass: boolean
  detail: string
}

/**
 * Safety note: a `tests` check runs `command` with `sh -c` in the attempt's temp
 * dir, on the user's own machine. A case file can run code, so only run case
 * files you trust. The case text is passed as one argv entry, never spliced
 * into another command line.
 */

function inside(dir: string, rel: string): string {
  const full = path.resolve(dir, rel)
  const root = path.resolve(dir)
  if (full !== root && !full.startsWith(root + path.sep)) throw new Error(`path escapes the workspace: ${rel}`)
  return full
}

async function readText(file: string): Promise<string | undefined> {
  return fs.readFile(file, "utf8").catch(() => undefined)
}

function needles(text: string, any?: string[]): { all: string[]; any: string[] } {
  return { all: text === "" ? [] : [text], any: any ?? [] }
}

function runCommand(command: string, cwd: string, timeoutMs: number): Promise<{ code: number | null; out: string; timedOut: boolean }> {
  return new Promise((resolve) => {
    const child = spawn("sh", ["-c", command], { cwd, stdio: ["ignore", "pipe", "pipe"] })
    let out = ""
    let timedOut = false
    const grab = (b: Buffer) => {
      out += b.toString()
      if (out.length > 20_000) out = out.slice(-20_000)
    }
    child.stdout.on("data", grab)
    child.stderr.on("data", grab)
    const timer = setTimeout(() => {
      timedOut = true
      child.kill("SIGKILL")
    }, timeoutMs)
    child.on("error", (e) => {
      clearTimeout(timer)
      resolve({ code: null, out: String(e), timedOut })
    })
    child.on("close", (code) => {
      clearTimeout(timer)
      resolve({ code, out, timedOut })
    })
  })
}

async function runOne(dir: string, check: Check, baselineDir: string, timeoutMs: number): Promise<CheckResult> {
  switch (check.type) {
    case "exists": {
      const ok = await fs.stat(inside(dir, check.path)).then(() => true, () => false)
      return { type: "exists", path: check.path, pass: ok, detail: ok ? "found" : "missing file" }
    }
    case "contains":
    case "absent": {
      const text = await readText(inside(dir, check.path))
      if (text === undefined) {
        // A missing file has nothing to contain, and nothing forbidden in it.
        return check.type === "absent"
          ? { type: "absent", path: check.path, pass: true, detail: "missing file" }
          : { type: "contains", path: check.path, pass: false, detail: "missing file" }
      }
      const fold = (s: string) => (check.ignore_case ? s.toLowerCase() : s)
      const hay = fold(text)
      const n = needles(check.text, check.any)
      const has = (w: string) => hay.includes(fold(w))
      if (check.type === "contains") {
        const missing = n.all.filter((w) => !has(w))
        const anyOk = n.any.length === 0 || n.any.some(has)
        const pass = missing.length === 0 && anyOk
        return {
          type: "contains",
          path: check.path,
          pass,
          detail: pass ? "found" : missing.length ? `missing ${JSON.stringify(missing)}` : "none of the any list found",
        }
      }
      const found = [...n.all, ...n.any].filter(has)
      return {
        type: "absent",
        path: check.path,
        pass: found.length === 0,
        detail: found.length ? `found ${JSON.stringify(found)}` : "not present",
      }
    }
    case "unchanged": {
      const [now, before] = await Promise.all([
        readText(inside(dir, check.path)),
        readText(inside(baselineDir, check.path)),
      ])
      const pass = now === before
      return {
        type: "unchanged",
        path: check.path,
        pass,
        detail: pass ? "same as the start" : now === undefined ? "file was removed" : before === undefined ? "file was added" : "file changed",
      }
    }
    case "tests": {
      for (const [rel, body] of Object.entries(check.files ?? {})) {
        const full = inside(dir, rel)
        await fs.mkdir(path.dirname(full), { recursive: true })
        await fs.writeFile(full, body)
      }
      const r = await runCommand(check.command, dir, timeoutMs)
      const tail = r.out.trim().split("\n").slice(-3).join(" | ").slice(0, 300)
      const detail = r.timedOut ? `timed out after ${Math.round(timeoutMs / 1000)}s` : `exit ${r.code}${tail ? `: ${tail}` : ""}`
      return { type: "tests", pass: !r.timedOut && r.code === 0, detail }
    }
  }
}

/** Run every check against `dir`. `baselineDir` is the untouched restored copy. */
export async function runChecks(
  dir: string,
  checks: Check[],
  baselineDir: string,
  opts: { testsTimeoutMs?: number } = {},
): Promise<{ pass: boolean; results: CheckResult[] }> {
  const results: CheckResult[] = []
  for (const c of checks) {
    try {
      results.push(await runOne(dir, c, baselineDir, opts.testsTimeoutMs ?? TESTS_TIMEOUT_MS))
    } catch (e) {
      results.push({ type: c.type, path: "path" in c ? c.path : undefined, pass: false, detail: e instanceof Error ? e.message : String(e) })
    }
  }
  return { pass: results.every((r) => r.pass), results }
}

#!/usr/bin/env bun
/**
 * Refreshes the model rows in the one catalog (src/runtime/providers/discovery/cli-catalog.json)
 * from what each installed CLI says it supports. Run by hand before a train
 * that ships Gizzi. There is no scheduled job on purpose.
 *
 * For every catalog tool with a `liveModels` command (the same command and
 * parser `gizzi models` uses) the script runs that list command, 15 s timeout,
 * and compares the answer with the tool's `models` rows. It only lists: no
 * prompt is ever sent, so no paid turn happens. A CLI that is not installed,
 * not signed in, or that prints nothing parseable is skipped with one line.
 *
 * Run: bun script/refresh-catalog.ts              (dry run: print the diff, write nothing)
 *      bun script/refresh-catalog.ts --dry-run    (same)
 *      bun script/refresh-catalog.ts --write      (rewrite the `models` arrays, then
 *                                                  run gen-picker-snapshot.ts)
 *
 * `--write` replaces only each changed tool's `models` array (which carries the
 * default marker) as text, so every other byte of the file stays. Rows the CLI
 * still lists keep their curated name, context and output limits; new rows take
 * what the CLI printed. `retired` maps are not touched: add old ids by hand.
 */
import { readFileSync, writeFileSync } from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { runForStdout, parseLiveModels } from "../src/runtime/providers/discovery/live-models"
import { cliSignedIn } from "../src/runtime/providers/discovery/sign-in"
import { SUBPROCESS_PROVIDERS, resolveCliPath } from "../src/runtime/providers/discovery/subprocess"

const TIMEOUT_MS = 15_000
const CATALOG_REL = "src/runtime/providers/discovery/cli-catalog.json"

const pkgRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const catalogPath = path.join(pkgRoot, CATALOG_REL)

interface Row {
  id: string
  name: string
  context?: number
  output?: number
  default?: true
}

const args = process.argv.slice(2)
const write = args.includes("--write")
if (write && args.includes("--dry-run")) {
  console.error("Pick one of --dry-run and --write.")
  process.exit(2)
}

const text = readFileSync(catalogPath, "utf8")
const catalog = JSON.parse(text) as {
  tools: Array<{ id: string; models?: Row[]; liveModels?: { args: string[]; parser: string } }>
}

/** The default row id: the marked one, else the first (the placeholder-only case has none). */
function defaultOf(rows: Row[]): string | undefined {
  return rows.find((r) => r.default)?.id ?? rows[0]?.id
}

/** Live rows in live order; rows the catalog knows keep its fields. One default. */
function buildRows(old: Row[], live: ReturnType<typeof parseLiveModels>): Row[] {
  const known = new Map(old.map((r) => [r.id, r]))
  const oldDefault = defaultOf(old)
  const defaultId =
    live.find((m) => m.default)?.id ??
    (oldDefault && live.some((m) => m.id === oldDefault) ? oldDefault : live[0]?.id)
  return live.map((m) => {
    const k = known.get(m.id)
    const row: Row = { id: m.id, name: k?.name || m.name || m.id }
    const context = k?.context ?? m.context
    const output = k?.output ?? m.output
    if (context !== undefined) row.context = context
    if (output !== undefined) row.output = output
    if (m.id === defaultId) row.default = true
    return row
  })
}

function diffLines(id: string, old: Row[], next: Row[]): string[] {
  const oldIds = old.map((r) => r.id)
  const nextIds = next.map((r) => r.id)
  const added = nextIds.filter((x) => !oldIds.includes(x))
  const removed = oldIds.filter((x) => !nextIds.includes(x))
  const lines: string[] = []
  if (added.length) lines.push(`  + added:   ${added.join(", ")}`)
  if (removed.length) lines.push(`  - removed: ${removed.join(", ")}`)
  const was = defaultOf(old)
  const now = defaultOf(next)
  if (was !== now) lines.push(`  ~ default: ${was ?? "(none)"} -> ${now ?? "(none)"}`)
  if (!lines.length && oldIds.join("\n") !== nextIds.join("\n")) lines.push(`  ~ order changed: ${nextIds.join(", ")}`)
  return lines.length ? [`${id}`, ...lines] : []
}

/** Replace one tool's `models` array in the file text; null if it cannot be located. */
function replaceModels(src: string, toolId: string, rows: Row[]): string | null {
  const lines = src.split("\n")
  const toolsStart = lines.findIndex((l) => l === '  "tools": [')
  const toolsEnd = lines.findIndex((l, i) => i > toolsStart && /^  \],?$/.test(l))
  if (toolsStart < 0 || toolsEnd < 0) return null
  const idLine = lines.findIndex((l, i) => i > toolsStart && i < toolsEnd && l === `      "id": ${JSON.stringify(toolId)},`)
  if (idLine < 0) return null
  const open = lines.findIndex((l, i) => i > idLine && i < toolsEnd && l === '      "models": [')
  const nextTool = lines.findIndex((l, i) => i > idLine && i < toolsEnd && l === "    {")
  if (open < 0 || (nextTool >= 0 && nextTool < open)) return null
  const close = lines.findIndex((l, i) => i > open && /^      \],?$/.test(l))
  if (close < 0) return null
  const comma = lines[close].endsWith(",") ? "," : ""
  const body = JSON.stringify(rows, null, 2)
    .split("\n")
    .map((l) => "      " + l)
  // body[0] is "      [" and the last is "      ]": keep our own open line and close line.
  const rebuilt = [lines[open], ...body.slice(1, -1), "      ]" + comma]
  return [...lines.slice(0, open), ...rebuilt, ...lines.slice(close + 1)].join("\n")
}

let out = text
let changed = 0
const summary: string[] = []

for (const tool of catalog.tools) {
  if (!tool.liveModels) continue // no list command known for this CLI: its rows stay by hand
  if (tool.liveModels.parser === "ollama") {
    console.log(`${tool.id}: skipped (lists the models on this machine, not a shared catalog)`)
    continue
  }
  const spec = SUBPROCESS_PROVIDERS.find((s) => s.id === tool.id)
  const bin = spec ? await resolveCliPath(spec) : null
  if (!spec || !bin) {
    console.log(`${tool.id}: skipped (not installed)`)
    continue
  }
  if ((await cliSignedIn(spec, bin)) === false) {
    console.log(`${tool.id}: skipped (not signed in)`)
    continue
  }
  const result = await runForStdout([bin, ...tool.liveModels.args], TIMEOUT_MS)
  if (!result) {
    console.log(`${tool.id}: skipped (list command failed or took over ${TIMEOUT_MS / 1000}s)`)
    continue
  }
  const live = parseLiveModels(tool.liveModels.parser as never, result.stdout)
  if (!live.length) {
    console.log(`${tool.id}: skipped (the list command printed nothing parseable)`)
    continue
  }
  const old = tool.models ?? []
  const next = buildRows(old, live)
  const d = diffLines(tool.id, old, next)
  if (!d.length) {
    console.log(`${tool.id}: up to date (${next.length} models)`)
    continue
  }
  console.log(d.join("\n"))
  summary.push(tool.id)
  if (write) {
    const replaced = replaceModels(out, tool.id, next)
    if (!replaced) {
      console.error(`${tool.id}: could not locate its models array in cli-catalog.json; left as it was`)
      continue
    }
    out = replaced
    changed++
  }
}

console.log(
  summary.length
    ? `\n${summary.length} tool(s) differ: ${summary.join(", ")}.`
    : "\nEvery checked tool is up to date.",
)

if (!write) {
  if (summary.length) console.log("Dry run: nothing written. Run with --write to update cli-catalog.json.")
  process.exit(0)
}

if (changed === 0) {
  console.log("Nothing to write.")
  process.exit(0)
}

// Guard: only `models` arrays may differ from the file we started from.
const before = JSON.parse(text) as { tools: Array<Record<string, unknown>> } & Record<string, unknown>
const after = JSON.parse(out) as typeof before
const strip = (c: typeof before) => JSON.stringify({ ...c, tools: c.tools.map(({ models: _m, ...rest }) => rest) })
if (strip(before) !== strip(after)) {
  console.error("Refusing to write: the rewrite changed a field other than `models`.")
  process.exit(1)
}

writeFileSync(catalogPath, out)
console.log(`Wrote ${CATALOG_REL} (${changed} tool(s)).`)

const gen = Bun.spawnSync(["bun", path.join(pkgRoot, "script/gen-picker-snapshot.ts")], {
  cwd: pkgRoot,
  stdout: "inherit",
  stderr: "inherit",
})
if (gen.exitCode !== 0) {
  console.error("gen-picker-snapshot.ts failed; run it by hand before committing.")
  process.exit(gen.exitCode ?? 1)
}

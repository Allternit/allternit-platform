#!/usr/bin/env bun
/**
 * Fails (exit 1) when cli-catalog.json `runtimes[]` is inconsistent:
 *   - a non-LLM tool has no runtime
 *   - an id or alias repeats
 *   - an acp / stream-json runtime has no CLI_ADAPTERS key in local-cli-driver.ts
 *   - a `tool` points to a missing tools[].id
 *   - a Line uses an unknown role
 *   - `plans` is missing, not a list, repeats, or names a provider outside
 *     the subscription providers (claude, chatgpt, kimi, gemini, copilot)
 * Run: bun script/check-runtime-registry.ts   (not a build)
 */
import { readFileSync } from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"

const here = path.dirname(fileURLToPath(import.meta.url))
const root = path.resolve(here, "../src/runtime")
const catalog = JSON.parse(readFileSync(path.join(root, "providers/discovery/cli-catalog.json"), "utf8"))
const driver = readFileSync(path.join(root, "drivers/local-cli-driver.ts"), "utf8")

const errors: string[] = []
const tools: { id: string; category?: string }[] = catalog.tools ?? []
const runtimes: any[] = catalog.runtimes ?? []
if (catalog.version < 3) errors.push(`catalog version is ${catalog.version}, expected 3 or higher`)
if (!runtimes.length) errors.push("catalog has no runtimes[]")

const toolIds = new Set(tools.map((t) => t.id))
const used = new Set(runtimes.map((r) => r.tool).filter(Boolean))
for (const t of tools) {
  if (t.category === "llm") continue
  if (!used.has(t.id)) errors.push(`tool "${t.id}" has no runtime`)
}

const seen = new Map<string, string>()
for (const r of runtimes) {
  for (const k of [r.id, ...(r.aliases ?? [])]) {
    if (seen.has(k)) errors.push(`id/alias "${k}" repeats (${seen.get(k)} and ${r.id})`)
    else seen.set(k, r.id)
  }
}

const start = driver.indexOf("const CLI_ADAPTERS")
const adapterKeys = new Map<string, string>()
if (start < 0) errors.push("CLI_ADAPTERS not found in local-cli-driver.ts")
else {
  for (const m of driver.slice(start).matchAll(/\n  "?([\w-]+)"?: \{\n    mode: "([\w-]+)"/g)) adapterKeys.set(m[1], m[2])
}

const ROLES = new Set(["fg", "mu", "ac", "ac2", "ok", "err", "b", "pill"])
function checkLines(owner: string, lines: unknown) {
  if (!Array.isArray(lines)) return
  for (const line of lines) {
    if (!Array.isArray(line)) { errors.push(`${owner}: a Line is not an array`); continue }
    for (const seg of line) {
      if (!Array.isArray(seg) || typeof seg[0] !== "string") errors.push(`${owner}: bad Line segment ${JSON.stringify(seg)}`)
      else if (!ROLES.has(seg[1] ?? "fg")) errors.push(`${owner}: unknown Line role "${seg[1]}"`)
    }
  }
}

const PLAN_PROVIDERS = new Set(["claude", "chatgpt", "kimi", "gemini", "copilot"])

for (const r of runtimes) {
  if (!Array.isArray(r.plans)) errors.push(`runtime "${r.id}": plans is missing (a list of subscription providers, [] for none)`)
  else {
    for (const p of r.plans) {
      if (!PLAN_PROVIDERS.has(p)) errors.push(`runtime "${r.id}": plans names "${p}", not a subscription provider (${[...PLAN_PROVIDERS].join(", ")})`)
    }
    if (new Set(r.plans).size !== r.plans.length) errors.push(`runtime "${r.id}": plans repeats a provider`)
  }
  if (r.tool != null && !toolIds.has(r.tool)) errors.push(`runtime "${r.id}": tool "${r.tool}" is not in tools[]`)
  if ((r.adapter?.kind === "acp" || r.adapter?.kind === "stream-json") && !adapterKeys.has(r.tool)) {
    errors.push(`runtime "${r.id}": ${r.adapter.kind} but "${r.tool}" has no CLI_ADAPTERS key`)
  }
  for (const side of ["terminal", "desktop"] as const) {
    const t = r.look?.[side]
    if (!t) continue
    const o = `runtime "${r.id}" look.${side}`
    checkLines(`${o}.banner`, t.banner)
    for (const v of ["Read", "Edit", "Run"]) checkLines(`${o}.steps.${v}`, t.steps?.[v])
    checkLines(`${o}.pre`, t.pre)
    for (const s of ["l", "m", "r"]) if (t.footer?.[s]) checkLines(`${o}.footer.${s}`, [t.footer[s]])
  }
  if (r.look && r.look.default !== "terminal" && r.look.default !== "desktop") errors.push(`runtime "${r.id}": look.default must be terminal or desktop`)
  if (r.look?.[r.look.default] == null && r.id !== "allternit") errors.push(`runtime "${r.id}": look.default has no tokens`)
}

if (errors.length) {
  console.error(`check-runtime-registry: ${errors.length} problem(s)`)
  for (const e of errors) console.error(`  - ${e}`)
  process.exit(1)
}
console.log(`check-runtime-registry: ok (${runtimes.length} runtimes, ${tools.length} tools, ${adapterKeys.size} driver adapters)`)

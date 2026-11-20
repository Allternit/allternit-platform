#!/usr/bin/env bun
/**
 * Allternit Sleep, experiment E2: where do memory "pages" go so the prompt
 * cache keeps working across a 30-turn session? (PLAN.md section 3 D4, gate G2.)
 *
 * Placements:
 *   control       no pages
 *   preload       3 pages in the pinned segment before turn 1 (design default)
 *   system_tail   one page added at turn 10 to an end-of-system region that has
 *                 its own cache breakpoint
 *   mid_system    one page added at turn 10 by editing the middle of the system prompt
 *   message_tail  one page added at turn 10 as a block in the latest user message
 *                 (stays in history afterwards; no system change)
 *   message_tail_ephemeral (extra, informational) same, but re-appended as a
 *                 per-step tail every turn from turn 10 and never persisted
 *
 *   bun cmd/gizzi-code/script/sleep-cache-probe.ts --dry
 *   bun cmd/gizzi-code/script/sleep-cache-probe.ts --model <id> --turns 30 [--yes] [--ttl-1h]
 *
 * Real code used: PromptSegments.orderTools / withTail (stable-prefix contract),
 * PROMPT_SEGMENT_ORDER, cacheCountdown (TTL label). Stubs: the system text, tool
 * schemas, history and pages are fixed synthetic text; the breakpoint rule
 * re-states ProviderTransform.applyCaching (first 2 system messages + last 2
 * non-system messages), because that function is module-private; the request is
 * sent as raw Messages API JSON (no SDK) so cache_control sits exactly where the
 * model says. Real mode never spends without --yes.
 */
import type { ModelMessage } from "ai"
import { PromptSegments } from "../src/runtime/session/prompt-segments"
import { cacheCountdown } from "../src/shared/util/cache-countdown"

const PLACEMENTS = ["control", "preload", "system_tail", "mid_system", "message_tail", "message_tail_ephemeral"] as const
type Placement = (typeof PLACEMENTS)[number]
const G2_FIVE: Placement[] = ["control", "preload", "system_tail", "mid_system", "message_tail"]
const PULL_TURN = 10

// ---- args -----------------------------------------------------------------
const argv = process.argv.slice(2)
const flag = (n: string) => argv.includes(`--${n}`)
const opt = (n: string, d?: string) => {
  const i = argv.indexOf(`--${n}`)
  return i >= 0 && argv[i + 1] && !argv[i + 1].startsWith("--") ? argv[i + 1] : d
}
const DRY = flag("dry")
const TURNS = Number(opt("turns", "30"))
const MODEL = opt("model")
const GAP_S = Number(opt("gap", "20")) // seconds between turns, for TTL expiry in the model
const ONE_HOUR = flag("ttl-1h")
const TTL_S = ONE_HOUR ? 3600 : 300
const MULT = Number(opt("g2-multiple", "2")) // G2 threshold: pull cost > MULT x preload => message-layer only
const P_IN = Number(opt("price-in", "3")) // USD per million input tokens (assumption, override per model)
const WRITE_X = ONE_HOUR ? 2 : 1.25
const READ_X = 0.1
if (!DRY && !MODEL) {
  console.error("usage: sleep-cache-probe.ts --dry | --model <id> [--turns N] [--yes] [--ttl-1h] [--price-in USD/M] [--gap s]")
  process.exit(2)
}

// ---- fixed synthetic content ----------------------------------------------
const WORDS = "alpha ledger runner pinned prefix stable block signal harbor window cursor memory cache token anchor branch"
  .split(" ")
function filler(seed: string, chars: number) {
  let h = 2166136261
  for (const c of seed) h = Math.imul(h ^ c.charCodeAt(0), 16777619) >>> 0
  let out = ""
  while (out.length < chars) {
    h = (Math.imul(h, 1664525) + 1013904223) >>> 0
    out += WORDS[h % WORDS.length] + " "
  }
  return out.slice(0, chars)
}
const tok = (s: string) => Math.ceil(s.length / 4)
const PAGE_CHARS = 6000 // ~1.5k tokens
const PAGES = [0, 1, 2].map((i) => `<memory-page id="p${i}">\n${filler(`page${i}`, PAGE_CHARS)}\n</memory-page>`)
const baseSystem = (salt: string) =>
  `probe-salt ${salt}\n` + filler("system-a", 7200) + "\n## SECTION-MID\n" + filler("system-b", 4800)
const TOOLS = Object.fromEntries(
  ["write", "read", "grep", "edit", "bash", "glob", "task", "fetch"].map((n) => [
    n,
    { name: n, description: filler(`tool-${n}`, 1000), input_schema: { type: "object", properties: { x: { type: "string" } } } },
  ]),
)
const userText = (t: number) => `Turn ${t}: ${filler(`u${t}`, 560)}`
const assistantText = (t: number) => `Reply ${t}: ${filler(`a${t}`, 1150)}`

// ---- request assembly ---------------------------------------------------------
type Block = { seg: "tools" | "system" | "pinned" | "history" | "tail"; label: string; text: string; bp: boolean }

function partsOf(m: ModelMessage): string[] {
  if (typeof m.content === "string") return [m.content]
  return (m.content as any[]).map((p) => p.text ?? "")
}

function buildRequest(p: Placement, turn: number, salt: string, history: ModelMessage[]): { blocks: Block[]; messages: ModelMessage[] } {
  const pulled = turn >= PULL_TURN
  const system: Block[] = []
  let base = baseSystem(salt)
  if (p === "mid_system" && pulled) base = base.replace("## SECTION-MID\n", `## SECTION-MID\n${PAGES[0]}\n`)
  system.push({ seg: "system", label: "sys.base", text: base, bp: true })
  if (p === "preload") system.push({ seg: "pinned", label: "pages(3)", text: PAGES.join("\n"), bp: true })
  if (p === "system_tail" && pulled) system.push({ seg: "system", label: "sys.tail-region(page)", text: PAGES[0], bp: true })

  // history + this turn's user message; message_tail variants add the page via the real withTail
  let messages: ModelMessage[] = [...history, { role: "user", content: userText(turn) }]
  if (p === "message_tail" && turn === PULL_TURN) messages = PromptSegments.withTail(messages, [PAGES[0]])
  if (p === "message_tail_ephemeral" && pulled) messages = PromptSegments.withTail(messages, [PAGES[0]])

  const tools: Block[] = Object.keys(PromptSegments.orderTools(TOOLS)).map((n) => ({
    seg: "tools", label: `tool.${n}`, text: JSON.stringify(TOOLS[n as keyof typeof TOOLS]), bp: false,
  }))
  const msgBlocks: Block[] = []
  const last2 = new Set([messages.length - 1, messages.length - 2])
  messages.forEach((m, i) => {
    const ps = partsOf(m)
    ps.forEach((text, j) => {
      const isTailPart = text.startsWith("<system-reminder>")
      msgBlocks.push({
        seg: isTailPart && !(p === "message_tail" && turn > PULL_TURN) ? "tail" : "history",
        label: `${m.role}${i}`,
        text,
        bp: last2.has(i) && j === ps.length - 1,
      })
    })
  })
  // Anthropic wire order: tools, system, messages
  return { blocks: [...tools, ...system, ...msgBlocks], messages }
}

function nextHistory(p: Placement, turn: number, messages: ModelMessage[]): ModelMessage[] {
  // message_tail persists the page-bearing user message; ephemeral keeps the plain one.
  const keep = p === "message_tail_ephemeral" ? [...messages.slice(0, -1), { role: "user" as const, content: userText(turn) }] : messages
  return [...keep, { role: "assistant", content: assistantText(turn) }]
}

// ---- prefix-match cache model --------------------------------------------------
type CacheEntry = { touched: number }
function predict(p: Placement, turns: number) {
  const salt = `${p}`
  const cache = new Map<string, CacheEntry>()
  let history: ModelMessage[] = []
  const rows: { turn: number; blocks: Block[]; total: number; write: number; read: number }[] = []
  for (let t = 1; t <= turns; t++) {
    const now = t * GAP_S
    const { blocks, messages } = buildRequest(p, t, salt, history)
    const cum: number[] = []
    const keys: string[] = []
    let acc = ""
    let tk = 0
    for (const b of blocks) {
      acc += "\u0001" + b.text
      tk += tok(b.text)
      keys.push(acc)
      cum.push(tk)
    }
    const total = tk
    let read = 0
    let lastBp = -1
    blocks.forEach((b, i) => {
      if (!b.bp) return
      lastBp = i
      // lookup: from each breakpoint, look back up to 20 block boundaries for a live cached prefix
      for (let j = i; j >= Math.max(0, i - 20); j--) {
        const e = cache.get(keys[j])
        if (e && now - e.touched <= TTL_S) {
          read = Math.max(read, cum[j])
          break
        }
      }
    })
    const written = lastBp >= 0 ? Math.max(0, cum[lastBp] - read) : 0
    blocks.forEach((b, i) => {
      if (!b.bp) return
      cache.set(keys[i], { touched: now })
    })
    // refresh the entries that were read
    for (let j = 0; j < blocks.length; j++) {
      const e = cache.get(keys[j])
      if (e && cum[j] <= read) e.touched = now
    }
    rows.push({ turn: t, blocks, total, write: written, read })
    history = nextHistory(p, t, messages)
  }
  return rows
}

// ---- printing ----------------------------------------------------------------
function shapeLine(blocks: Block[]) {
  const out: string[] = []
  let i = 0
  while (i < blocks.length) {
    const b = blocks[i]
    if (b.seg === "tools") {
      let n = 0, s = 0
      while (i < blocks.length && blocks[i].seg === "tools") { n++; s += tok(blocks[i].text); i++ }
      out.push(`tools(${n}):${s}`)
    } else if (b.seg === "history" && !b.bp) {
      let n = 0, s = 0
      while (i < blocks.length && blocks[i].seg === "history" && !blocks[i].bp) { n++; s += tok(blocks[i].text); i++ }
      out.push(`history(${n}):${s}`)
    } else {
      out.push(`${b.seg}/${b.label}:${tok(b.text)}${b.bp ? "*" : ""}`)
      i++
    }
  }
  return out.join(" | ")
}
const k = (n: number) => String(n).padStart(6)
function tableFor(label: string, results: Record<string, { turn: number; write: number; read: number; total: number }[]>) {
  const names = Object.keys(results)
  console.log(`\n${label}`)
  console.log("turn  " + names.map((n) => n.slice(0, 22).padStart(24)).join(""))
  console.log("      " + names.map(() => "       write/read     ").join(""))
  const n = results[names[0]].length
  for (let i = 0; i < n; i++)
    console.log(String(i + 1).padStart(4) + "  " + names.map((nm) => `${k(results[nm][i].write)} /${k(results[nm][i].read)}`.padStart(24)).join(""))
  const sum = (nm: string, f: "write" | "read") => results[nm].reduce((a, r) => a + r[f], 0)
  console.log("total " + names.map((nm) => `${k(sum(nm, "write"))} /${k(sum(nm, "read"))}`.padStart(24)).join(""))
  const cost = (nm: string) => {
    const w = sum(nm, "write"), r = sum(nm, "read")
    const unc = results[nm].reduce((a, x) => a + (x.total - x.write - x.read), 0)
    return ((unc + w * WRITE_X + r * READ_X) * P_IN) / 1e6
  }
  console.log(`\nper placement          write     read  uncached  cost@$${P_IN}/M(in)  write ratio vs preload  read ratio vs preload`)
  const pw = sum("preload", "write") || 1, pr = sum("preload", "read") || 1
  for (const nm of names) {
    const unc = results[nm].reduce((a, x) => a + (x.total - x.write - x.read), 0)
    console.log(
      `${nm.padEnd(24)}${String(sum(nm, "write")).padStart(7)} ${String(sum(nm, "read")).padStart(8)} ${String(unc).padStart(9)}  $${cost(nm).toFixed(4).padStart(10)}  ${(sum(nm, "write") / pw).toFixed(2).padStart(20)}x  ${(sum(nm, "read") / pr).toFixed(2).padStart(20)}x`,
    )
  }
  // G2: extra write tokens caused by the pull (vs control), per placement, vs preload's up-front page cost
  const ctl = sum("control", "write")
  const preloadCost = sum("preload", "write") - ctl
  console.log(`\nG2: pull cost = write tokens above control. preload page cost = ${preloadCost}; threshold = ${MULT} x preload = ${MULT * preloadCost}`)
  for (const nm of names.filter((x) => x !== "control" && x !== "preload")) {
    const extra = sum(nm, "write") - ctl
    console.log(`  ${nm.padEnd(24)} pull cost ${String(extra).padStart(7)}  ${extra > MULT * preloadCost ? "OVER threshold -> pulls message-layer only" : "within threshold"}`)
  }
}

// ---- dry ---------------------------------------------------------------------
function dry() {
  const ttlLabel = cacheCountdown({ cacheTtlSeconds: TTL_S, cacheExpiresAt: Date.now() + TTL_S * 1000 }, Date.now())?.label ?? ""
  console.log(`E2 cache-placement probe (dry, no network)`)
  console.log(`assumed TTL: ${TTL_S}s (${ONE_HOUR ? "1h tier, as CacheCountdown when 1h-eligible" : "5m default, as CacheCountdown without 1h eligibility"}); countdown label: "${ttlLabel}"`)
  console.log(`turn gap ${GAP_S}s, ${TURNS} turns, pull at turn ${PULL_TURN}, pages ~${tok(PAGES[0])} tok each, segment order: ${PromptSegments.ORDER.join(" > ")}`)
  console.log(`legend: * = cache_control breakpoint; blocks listed in wire order (tools, system, messages)`)
  const results: Record<string, { turn: number; write: number; read: number; total: number }[]> = {}
  for (const p of PLACEMENTS) {
    const rows = predict(p, TURNS)
    results[p] = rows
    console.log(`\n== ${p} ==`)
    for (const r of rows) console.log(`t${String(r.turn).padStart(2)} total ${String(r.total).padStart(6)} w ${String(r.write).padStart(6)} r ${String(r.read).padStart(6)}  ${shapeLine(r.blocks)}`)
  }
  tableFor("Predicted cache write/read tokens per turn (prefix-match model)", results)
}

// ---- real --------------------------------------------------------------------
async function call(blocks: Block[], model: string) {
  const key = process.env.ANTHROPIC_API_KEY
  if (!key) throw new Error("ANTHROPIC_API_KEY not set")
  const cc = ONE_HOUR ? { type: "ephemeral", ttl: "1h" } : { type: "ephemeral" }
  const system = blocks.filter((b) => b.seg === "system" || b.seg === "pinned").map((b) => ({ type: "text", text: b.text, ...(b.bp ? { cache_control: cc } : {}) }))
  const tools = blocks.filter((b) => b.seg === "tools").map((b) => JSON.parse(b.text))
  // regroup message blocks by their label index (role+index)
  const msgs: { role: string; content: any[] }[] = []
  let cur = ""
  for (const b of blocks.filter((b) => b.seg === "history" || b.seg === "tail")) {
    if (b.label !== cur) { msgs.push({ role: b.label.startsWith("user") ? "user" : "assistant", content: [] }); cur = b.label }
    msgs[msgs.length - 1].content.push({ type: "text", text: b.text, ...(b.bp ? { cache_control: cc } : {}) })
  }
  const res = await fetch("https://api.anthropic.com/v1/messages", {
    method: "POST",
    headers: { "x-api-key": key, "anthropic-version": "2023-06-01", "content-type": "application/json" },
    body: JSON.stringify({ model, max_tokens: 8, system, tools, messages: msgs }),
  })
  const j: any = await res.json()
  if (!res.ok) throw new Error(`API ${res.status}: ${JSON.stringify(j).slice(0, 300)}`)
  const u = j.usage
  const w = u.cache_creation_input_tokens ?? 0, r = u.cache_read_input_tokens ?? 0
  return { write: w, read: r, total: (u.input_tokens ?? 0) + w + r }
}

async function real(model: string) {
  const run = Date.now().toString(36)
  console.log(`E2 real mode, model ${model}, ${TURNS} turns, TTL assumed ${TTL_S}s, run ${run}`)
  const dryRows = Object.fromEntries(PLACEMENTS.map((p) => [p, predict(p, TURNS)]))
  const price = (rows: { write: number; read: number; total: number }[]) =>
    rows.reduce((a, x) => a + ((x.total - x.write - x.read) + x.write * WRITE_X + x.read * READ_X) * P_IN / 1e6, 0)
  const predicted = PLACEMENTS.reduce((a, p) => a + price(dryRows[p]), 0)
  // first-turn probe: control placement, turn 1
  const first = buildRequest("control", 1, `${run}-first`, [])
  const u = await call(first.blocks, model)
  const firstCost = ((u.total - u.write - u.read + u.write * WRITE_X + u.read * READ_X) * P_IN) / 1e6
  console.log(`first-turn usage: write ${u.write} read ${u.read} total ${u.total}; cost ~$${firstCost.toFixed(4)}`)
  console.log(`estimated whole run (${PLACEMENTS.length} placements x ${TURNS} turns, prediction model, $${P_IN}/M input, write x${WRITE_X}, read x${READ_X}): ~$${predicted.toFixed(2)}`)
  if (!flag("yes")) {
    console.log("stopping: pass --yes to run the full probe (needs the owner's spend OK).")
    return
  }
  const results: Record<string, { turn: number; write: number; read: number; total: number }[]> = {}
  for (const p of PLACEMENTS) {
    results[p] = []
    let history: ModelMessage[] = []
    for (let t = 1; t <= TURNS; t++) {
      const { blocks, messages } = buildRequest(p, t, `${run}-${p}`, history)
      const r = await call(blocks, model)
      results[p].push({ turn: t, ...r })
      history = nextHistory(p, t, messages)
      await new Promise((ok) => setTimeout(ok, GAP_S * 1000))
    }
    console.log(`done ${p}`)
  }
  tableFor("Measured cache write/read tokens per turn", results)
}

if (DRY) dry()
else await real(MODEL!)

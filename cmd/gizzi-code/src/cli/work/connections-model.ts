/**
 * Connections as List · Grid · Deck over the live catalog
 * (`GET /api/v1/connections/catalog`, what `gizzi connect more` reads), with
 * the account's connected state from `GET /api/v1/connections`. Pure
 * rendering lives here so every mode has a test.
 */
import type { CatalogEntry, Row, State } from "@/cli/commands/connect-model"
import { GROUP_TITLE } from "@/cli/commands/connect-model"

export type Mode = "list" | "grid" | "deck"
export const MODES: Mode[] = ["list", "grid", "deck"]
export const PAGE = 24
/** "More" stops here: narrow with / instead of paging forever. */
export const CAP = 96

export type Item = { entry: CatalogEntry; state: State }

export function mergeItems(entries: CatalogEntry[], rows: Row[]): Item[] {
  const byId = new Map(rows.map(r => [r.id, r]))
  return entries.map(entry => ({ entry, state: byId.get(entry.id)?.state ?? "off" }))
}

export function nextMode(m: Mode, dir: 1 | -1 = 1): Mode {
  return MODES[(MODES.indexOf(m) + dir + MODES.length) % MODES.length]!
}

export function canMore(loaded: number, cursor?: string | null): boolean {
  return !!cursor && loaded < CAP
}

export function stateWord(i: Item): string {
  if (i.entry.available === false) return i.entry.unavailable_reason || "Runs on a computer"
  return i.state === "on" ? "Connected" : i.state === "needs" ? "Needs you" : i.state === "connecting" ? "Connecting" : "Not connected"
}

const clip = (s: string, n: number) => (s.length > n ? s.slice(0, Math.max(0, n - 1)) + "…" : s)
const dot = (i: Item) => (i.state === "on" ? "●" : i.state === "needs" ? "▲" : "○")

export function listLines(items: Item[], at: number, width = 80): string[] {
  return items.map((i, n) => {
    const name = clip(i.entry.name, 24).padEnd(25)
    const rest = clip([GROUP_TITLE[i.entry.group] ?? i.entry.group, stateWord(i)].join(" · "), Math.max(10, width - 32))
    return `${n === at ? "❯" : " "} ${dot(i)} ${name}${rest}`
  })
}

export function gridCols(width: number, tile = 24): number {
  return Math.max(1, Math.floor(width / (tile + 1)))
}

/** Tiles of `tile` columns, two lines each (name, state), `cols` per row. */
export function gridLines(items: Item[], at: number, width = 80, tile = 24): string[] {
  const cols = gridCols(width, tile)
  const out: string[] = []
  for (let r = 0; r < items.length; r += cols) {
    const row = items.slice(r, r + cols)
    const top = row.map((i, c) => `${r + c === at ? "❯" : " "}${dot(i)} ${clip(i.entry.name, tile - 3)}`.padEnd(tile))
    const bot = row.map(i => `   ${clip(stateWord(i), tile - 3)}`.padEnd(tile))
    out.push(top.join(" "), bot.join(" "), "")
  }
  return out
}

/** One card: group, name, blurb, state, ways. */
export function deckLines(items: Item[], at: number): string[] {
  const i = items[at]
  if (!i) return []
  const e = i.entry
  const lines = [
    `┌─ ${GROUP_TITLE[e.group] ?? e.group} ${"─".repeat(20)}`,
    `│ ${e.name}${e.tier === "partner" || e.tier === "anthropic" ? " [verified]" : ""}`,
    `│ ${e.blurb ?? ""}`,
    `│ ${dot(i)} ${stateWord(i)}`,
  ]
  if (e.ways?.length) lines.push(`│ Ways: ${e.ways.join(", ")}`)
  lines.push(`└─ ${at + 1} of ${items.length}`)
  return lines
}

export function footer(mode: Mode, more: boolean, filtering = false): string {
  if (filtering) return "type to filter · enter apply · esc cancel"
  const move = mode === "deck" ? "←/→ card" : mode === "grid" ? "arrows move" : "↑/↓ move"
  return [move, "tab mode", "/ filter", more ? "m more" : "", "esc close"].filter(Boolean).join(" · ")
}

export function summary(items: Item[], total: number, q?: string): string {
  return `${items.length} of ${total}${q ? ` matching "${q}"` : ""}`
}

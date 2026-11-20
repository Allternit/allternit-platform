/**
 * Runtime registry reader (Harness & Bots 2.0, contract: CONTRACT-harness-bots-2.md).
 *
 * The `runtimes[]` array of cli-catalog.json (version 3) is the one list of
 * runtimes a bot can run on. A runtime points at a catalog tool with `tool`
 * (null for the native Allternit runtime). Look configs are data only, so
 * Swift, Kotlin and web can draw them from the same JSON.
 */
import catalog from "./cli-catalog.json"

export type RuntimeCategory = "native" | "coding" | "personal" | "general" | "experimental"
export type AdapterKind =
  | "native"
  | "acp"
  | "stream-json"
  | "codex-app-server"
  | "gateway"
  | "one-shot-json"
  | "text"
  | "unsupported"
export type AccessKind = "plan" | "key" | "local" | "credits"
export type SystemLayerVia = "acp-session" | "append-flag" | "agents-md" | "plugin" | "none"
export type LineRole = "fg" | "mu" | "ac" | "ac2" | "ok" | "err" | "b" | "pill"

/** Text with {a} (argument) and {r} (result) slots, as [text, role] segments. */
export type Line = [string, LineRole][]

export interface Tokens {
  kind: "tui" | "app"
  bg: string
  fg: string
  mu: string
  ac: string
  ac2?: string
  ok?: string
  err?: string
  light?: Partial<Record<"bg" | "fg" | "mu" | "ac" | "ac2" | "ok" | "err", string>>
  dark?: Partial<Record<"bg" | "fg" | "mu" | "ac" | "ac2" | "ok" | "err", string>>
  banner: Line[]
  big?: { text?: string; grad?: string[]; parts?: [string, LineRole][] }
  userStyle: string
  prompt: string
  steps: { Read: Line[]; Edit: Line[]; Run: Line[] }
  input: string
  placeholder: string
  footer: { l?: Line; m?: Line; r?: Line } | null
  [extra: string]: unknown
}

export interface RuntimeEntry {
  id: string
  name: string
  vendor: string
  category: RuntimeCategory
  tool: string | null
  aliases?: string[]
  adapter: { kind: AdapterKind; argv?: string[] }
  brains: "any" | string[]
  access: AccessKind[]
  more: boolean
  status?: { label: string; tone: "warn" | "info" }
  systemLayer: { via: SystemLayerVia }
  look: { pack?: string; default: "terminal" | "desktop"; terminal?: Tokens; desktop?: Tokens }
  liveUi?: { argv: string[]; port: number; health: string; label: string }
  legacy?: { harness?: string; uhp?: string[] }
  docs?: string
}

/** The slice of a bot's botBrain this module reads and writes. */
export interface BotBrainLike {
  mode?: "allternit_cloud" | "native_harness" | "uhp_harness" | string
  harness?: string
  uhpHarnessId?: string
  runtime?: string
}

const DATA = catalog as unknown as { version: number; runtimes?: RuntimeEntry[] }

export const RUNTIME_CATALOG_VERSION: number = DATA.version

const RUNTIMES: RuntimeEntry[] = DATA.runtimes ?? []

/** Every runtime, in catalog order. `includeMore: false` drops the "More" ones. */
export function listRuntimes(opts: { includeMore?: boolean } = {}): RuntimeEntry[] {
  const includeMore = opts.includeMore ?? true
  return RUNTIMES.filter((r) => includeMore || !r.more)
}

/** A runtime by id or alias (case-insensitive), else undefined. */
export function getRuntime(idOrAlias: string | undefined | null): RuntimeEntry | undefined {
  if (!idOrAlias) return undefined
  const key = idOrAlias.toLowerCase()
  return (
    RUNTIMES.find((r) => r.id.toLowerCase() === key) ??
    RUNTIMES.find((r) => r.aliases?.some((a) => a.toLowerCase() === key))
  )
}

/** A runtime by the old native harness id (`legacy.harness`, else its own id). */
function byHarness(harness: string): RuntimeEntry | undefined {
  return RUNTIMES.find((r) => r.legacy?.harness === harness) ?? getRuntime(harness)
}

/** Brain fits runtime when brains is "any" or the brain's provider is listed. */
export function fits(runtime: RuntimeEntry | string, providerId: string): boolean {
  const r = typeof runtime === "string" ? getRuntime(runtime) : runtime
  if (!r) return false
  return r.brains === "any" || r.brains.includes(providerId)
}

/**
 * Normalizer rule (contract). With no `runtime`, derive it:
 *   allternit_cloud -> allternit; native_harness + h -> the runtime whose id or
 *   legacy.harness is h; uhp_harness -> the legacy.uhp match, else "custom".
 * With a `runtime`, recompute mode/harness from it. Returns a new object.
 */
export function resolveRuntime<T extends BotBrainLike>(botBrain: T): T & { runtime: string } {
  if (botBrain.runtime) {
    const r = getRuntime(botBrain.runtime)
    if (!r) return { ...botBrain, runtime: botBrain.runtime }
    if (r.id === "allternit") {
      const { harness: _h, uhpHarnessId: _u, ...rest } = botBrain
      return { ...(rest as T), mode: "allternit_cloud", runtime: r.id }
    }
    if (r.adapter.kind === "unsupported") {
      return { ...botBrain, mode: "uhp_harness", uhpHarnessId: r.legacy?.uhp?.[0] ?? r.id, runtime: r.id }
    }
    return { ...botBrain, mode: "native_harness", harness: r.legacy?.harness ?? r.id, runtime: r.id }
  }
  if (botBrain.mode === "native_harness" && botBrain.harness) {
    const r = byHarness(botBrain.harness)
    return { ...botBrain, runtime: r?.id ?? "custom" }
  }
  if (botBrain.mode === "uhp_harness") {
    const u = botBrain.uhpHarnessId
    const r = u ? RUNTIMES.find((x) => x.legacy?.uhp?.includes(u)) : undefined
    return { ...botBrain, runtime: r?.id ?? "custom" }
  }
  return { ...botBrain, runtime: "allternit" }
}

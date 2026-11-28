/**
 * Runtime registry reader (Harness & Bots 2.0, contract: CONTRACT-harness-bots-2.md).
 *
 * The `runtimes[]` array of cli-catalog.json (version 3) is the one list of
 * runtimes a bot can run on. Gizzi does not import that file: it reads
 * catalog.snapshot.gen.json, generated from it at build time by
 * script/gen-picker-snapshot.ts (see `loadRuntimeSnapshot`). A runtime points at a catalog tool with `tool`
 * (null for the native Allternit runtime). Look configs are data only, so
 * Swift, Kotlin and web can draw them from the same JSON.
 */
import snapshot from "./catalog.snapshot.gen.json"

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
  /** Subscription providers (claude, chatgpt, kimi, gemini, copilot) whose plan this runtime may use. */
  plans: string[]
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

/** A CLI row (`tools[]`) as far as this module reads it; subprocess.ts and sign-in.ts read more fields. */
export interface SnapshotTool {
  id: string
  aliases?: string[]
  [field: string]: unknown
}

/** The picker data Gizzi starts from: the picker-catalog envelope plus the full runtime and CLI rows. */
export interface RuntimeSnapshot {
  schema: number
  generatedAt: string
  source: { path: string; sha256: string; catalogVersion: number }
  runtimes: RuntimeEntry[]
  tools: SnapshotTool[]
}

/**
 * The runtime and CLI rows, from the snapshot built into this binary
 * (catalog.snapshot.gen.json, generated from cli-catalog.json by
 * script/gen-picker-snapshot.ts). This is the offline fallback: the live
 * GET /api/v1/picker-catalog overlays it, so every reader goes through here.
 */
export function loadRuntimeSnapshot(): RuntimeSnapshot {
  return snapshot as unknown as RuntimeSnapshot
}

const DATA = loadRuntimeSnapshot()

export const RUNTIME_CATALOG_VERSION: number = DATA.source.catalogVersion

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

/**
 * Does a brain's provider fit this runtime? `providerId` may be a vendor id
 * (`anthropic`), a subscription id (`subs-anthropic`) or a CLI provider id
 * (`claude-cli`): a runtime always runs its own CLI's models. Every client
 * (gizzi, web, iOS, Android) uses this one rule.
 */
export function fits(runtime: RuntimeEntry | string, providerId: string): boolean {
  const r = typeof runtime === "string" ? getRuntime(runtime) : runtime
  if (!r) return false
  if (r.brains === "any") return true
  const p = providerId.replace(/^subs-/, "")
  if (r.brains.includes(p)) return true
  if (!r.tool) return false
  if (p === r.tool || p === r.id || (r.aliases ?? []).includes(p)) return true
  const tool = DATA.tools?.find((t) => t.id === r.tool)
  return (tool?.aliases ?? []).includes(p)
}

/** A runtime's own CLI: its tool, its id, its aliases, the tool's aliases. */
function isOwnCli(r: RuntimeEntry, providerId: string): boolean {
  if (!r.tool) return false
  if (providerId === r.tool || providerId === r.id || (r.aliases ?? []).includes(providerId)) return true
  const tool = DATA.tools?.find((t) => t.id === r.tool)
  return (tool?.aliases ?? []).includes(providerId)
}

const PLAN_OF_VENDOR: Record<string, string> = {
  anthropic: "claude",
  openai: "chatgpt",
  google: "gemini",
  moonshot: "kimi",
  github: "copilot",
}

/** Display names of the subscription providers (same as the server's disclosure list). */
export const PLAN_NAMES: Record<string, string> = {
  claude: "Claude",
  chatgpt: "ChatGPT",
  kimi: "Kimi",
  gemini: "Gemini",
  copilot: "Copilot",
}

/**
 * The subscription provider whose plan a brain uses, if it is a plan:
 * `subs-<p>` (vendor ids map to their plan: anthropic -> claude, ...), or a CLI
 * sign-in whose runtime holds exactly one plan (claude-cli -> claude,
 * codex-cli -> chatgpt, kimi-cli -> kimi, antigravity -> gemini, copilot -> copilot).
 */
export function planProvider(providerId: string): string | undefined {
  if (providerId.startsWith("subs-")) {
    const rest = providerId.slice("subs-".length)
    return PLAN_OF_VENDOR[rest] ?? rest
  }
  const r = RUNTIMES.find((x) => x.tool === providerId && (x.plans ?? []).length === 1)
  return r?.plans[0]
}

/**
 * The brain rule every client uses to offer a brain (contract: "Shared rule
 * allowed"): `fits`, and a plan brain only where the runtime's `plans` list
 * its provider. A runtime's own CLI sign-in always passes.
 */
export function allowed(runtime: RuntimeEntry | string, providerId: string): boolean {
  const r = typeof runtime === "string" ? getRuntime(runtime) : runtime
  if (!r || !fits(r, providerId)) return false
  const plan = planProvider(providerId)
  if (!plan) return true
  return isOwnCli(r, providerId) || (r.plans ?? []).includes(plan)
}

/**
 * Why a fitting brain is not allowed: the contract's
 * `plan_not_allowed_on_runtime` message, or undefined when it is allowed (or
 * does not fit at all; that is the auto-switch's job).
 */
export function planRefusal(runtime: RuntimeEntry | string, providerId: string): string | undefined {
  const r = typeof runtime === "string" ? getRuntime(runtime) : runtime
  if (!r || !fits(r, providerId) || allowed(r, providerId)) return undefined
  const plan = planProvider(providerId) ?? ""
  const own = RUNTIMES.find((x) => x.id !== "allternit" && (x.plans ?? []).includes(plan))?.name ?? "Allternit"
  const name = PLAN_NAMES[plan] ?? plan.charAt(0).toUpperCase() + plan.slice(1)
  return `${name}'s plan can't be used inside ${r.name}. Use an API key, or pick ${own}.`
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
    if (!r) {
      // Same rules as the server (allternit-api bot_brain.rs): `custom` keeps the
      // stored mode/harness; any other id this build does not know is treated as
      // a native harness of that id (a newer client may know more runtimes).
      if (botBrain.runtime === "custom") return { ...botBrain, runtime: "custom" }
      return { ...botBrain, mode: "native_harness", harness: botBrain.runtime, runtime: botBrain.runtime }
    }
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

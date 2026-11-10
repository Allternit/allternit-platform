/**
 * Default brain selection — the one resolver every surface asks.
 *
 * Unpaid / Free: the installed CLI with the best catalog `defaultRank`
 * (claude-cli 1, codex-cli 2, kimi-cli 3, …) that is signed in; a CLI whose
 * sign-in was not checked is the fallback; a CLI known to be signed out is
 * never picked. A CLI with no `defaultRank` is never an automatic default.
 * Paid Plus / Super / Ultra: Allternit Cloud, provisioned by the billing
 * webhook — gizzi only has to point at `allternit/<model>` (the Cloud's
 * flagged default model, else its first).
 *
 * A pin is a `/model` choice the user made: `model` set to something other
 * than `auto`, with `model_auto` not true. `model_auto: false` with no model
 * is not a pin (it is the dead state the resolver repairs), and neither is a
 * mistaken auto default (Desktop's rule: a `nail-35` local model it once
 * wrote for everyone).
 */

import { Config } from "@/runtime/context/config/config"
import { Log } from "@/shared/util/log"
import {
  getCachedAllternitPlan,
  isPaidAllternitPlan,
  refreshAllternitPlan,
  type AllternitPlan,
} from "./discovery/allternit-cloud"
import { Discovery, type DiscoveredProvider } from "./discovery"
import { CLI_CATALOG, retiredModelId } from "./discovery/subprocess"

export { isPaidAllternitPlan }

const log = Log.create({ service: "default-brain" })

export type DefaultBrainInput = {
  modelAuto?: boolean
  currentModel?: string
  plan: AllternitPlan | null
  providers: DiscoveredProvider[]
}

export type DefaultBrain = {
  /** `provider/model`, or null when nothing usable was found. */
  model: string | null
  /** Why this model (or why none). */
  reason: string
  /** True when `model` is the user's own `/model` pin. */
  pinned: boolean
}

const AUTO_REFS = new Set(["auto", "auto/auto"])

/** Desktop's "mistaken auto default": a local nail-35 model it once wrote as everyone's default. */
export function isMistakenAutoDefault(ref: string | undefined): boolean {
  return Boolean(ref && /nail-35/i.test(ref))
}

/** Whether the config's model is the user's own pin (see the file header). */
export function isPinnedModel(input: { modelAuto?: boolean; currentModel?: string }): boolean {
  const current = input.currentModel?.trim()
  if (!current || AUTO_REFS.has(current.toLowerCase())) return false
  if (input.modelAuto === true) return false
  return !isMistakenAutoDefault(current)
}

/** A `provider/model` ref with a retired model id mapped to its current one. */
function mapRetired(ref: string): string {
  const slash = ref.indexOf("/")
  if (slash <= 0) return ref
  const providerID = ref.slice(0, slash)
  return `${providerID}/${retiredModelId(providerID, ref.slice(slash + 1))}`
}

/** The provider's `defaultRank`, falling back to the built-in catalog (user-added CLIs never have one). */
export function defaultRankOf(provider: Pick<DiscoveredProvider, "id" | "defaultRank">): number | undefined {
  if (provider.defaultRank !== undefined) return provider.defaultRank
  return CLI_CATALOG.find((tool) => tool.id === provider.id && !tool.custom)?.defaultRank
}

/**
 * The provider's flagged default model (discovery flag, else catalog flag),
 * else its first model. Hidden models (retired aliases kept only so saved
 * choices still resolve) are never picked automatically.
 */
function preferredModel(provider: DiscoveredProvider): string | null {
  const visible = provider.models.filter((m) => m.id && !m.hidden)
  if (visible.length === 0) return null
  const flagged =
    visible.find((m) => m.default) ??
    (() => {
      const catalogDefault = CLI_CATALOG.find((tool) => tool.id === provider.id)?.models?.find((m) => m.default)
      return catalogDefault ? visible.find((m) => m.id === catalogDefault.id) : undefined
    })()
  const model = flagged ?? visible[0]
  return `${provider.id}/${model.id}`
}

function pickCli(providers: DiscoveredProvider[]): DefaultBrain {
  const ranked = providers
    .filter((p) => p.source === "subprocess" && preferredModel(p) !== null && p.signedIn !== false)
    .map((p) => ({ p, rank: defaultRankOf(p) }))
    .filter((x): x is { p: DiscoveredProvider; rank: number } => x.rank !== undefined)
    .sort((a, b) => a.rank - b.rank)
  const signedIn = ranked.find((x) => x.p.signedIn === true)
  if (signedIn) {
    return { model: preferredModel(signedIn.p), reason: `${signedIn.p.name} is signed in`, pinned: false }
  }
  const unknown = ranked.find((x) => x.p.signedIn === undefined)
  if (unknown) {
    return {
      model: preferredModel(unknown.p),
      reason: `${unknown.p.name} is installed (sign-in not checked)`,
      pinned: false,
    }
  }
  return {
    model: null,
    reason: "No signed-in coding CLI and no paid Allternit plan",
    pinned: false,
  }
}

/** The one deterministic resolver. Pure: no config, network or disk. */
export function resolveDefaultBrain(input: DefaultBrainInput): DefaultBrain {
  const current = input.currentModel?.trim() || undefined

  if (isPinnedModel({ modelAuto: input.modelAuto, currentModel: current })) {
    return { model: mapRetired(current!), reason: "pinned", pinned: true }
  }

  if (isPaidAllternitPlan(input.plan)) {
    const cloud = input.providers.find((p) => p.id === "allternit" && p.source === "platform")
    const model = cloud ? preferredModel(cloud) : null
    if (model) return { model, reason: "Paid Allternit plan: Allternit Cloud", pinned: false }
    const cli = pickCli(input.providers)
    return {
      ...cli,
      reason: cli.model
        ? `Paid Allternit plan, but Allternit Cloud has no models right now; ${cli.reason}`
        : "Paid Allternit plan, but Allternit Cloud has no models right now and no coding CLI is signed in",
    }
  }

  // Unpaid keeps an existing CLI pick gizzi made earlier (model_auto: true),
  // while that CLI is still installed and not known to be signed out.
  if (input.modelAuto === true && current && !isMistakenAutoDefault(current)) {
    const providerID = current.split("/")[0]
    const provider = input.providers.find((p) => p.id === providerID && p.source === "subprocess")
    const kept = mapRetired(current)
    const keptHidden = provider?.models.some((m) => m.hidden && `${providerID}/${m.id}` === kept)
    if (provider && provider.signedIn !== false && !keptHidden) {
      return { model: kept, reason: `${provider.name} (kept from the last automatic pick)`, pinned: false }
    }
  }

  return pickCli(input.providers)
}

/**
 * The model to switch the config to, or null when the config should stay
 * as it is (a pin, nothing usable, or already selected).
 */
export function pickDefaultBrain(input: DefaultBrainInput): string | null {
  const resolved = resolveDefaultBrain(input)
  if (resolved.pinned || !resolved.model) return null
  return resolved.model === input.currentModel?.trim() ? null : resolved.model
}

type BrainConfig = {
  model?: string
  model_auto?: boolean
  disabled_providers?: string[]
  enabled_providers?: string[]
}

/** Drop providers the user disabled (`disabled_providers` / `enabled_providers`). */
function allowedProviders(cfg: BrainConfig, providers: DiscoveredProvider[]): DiscoveredProvider[] {
  const disabled = new Set(cfg.disabled_providers ?? [])
  const enabled = cfg.enabled_providers ? new Set(cfg.enabled_providers) : undefined
  return providers.filter((p) => !disabled.has(p.id) && (!enabled || enabled.has(p.id)))
}

async function discovered(providers?: DiscoveredProvider[]): Promise<DiscoveredProvider[]> {
  return providers ?? (Discovery.last().length > 0 ? Discovery.last() : await Discovery.run())
}

/** The resolved default brain for this machine right now (merged config + plan + discovery). */
export async function currentDefaultBrain(): Promise<DefaultBrain> {
  let cfg: BrainConfig = {}
  try {
    cfg = await Config.get()
  } catch {
    cfg = await Config.getGlobal().catch(() => ({}))
  }
  const providers = await discovered()
  return resolveDefaultBrain({
    modelAuto: cfg.model_auto,
    currentModel: cfg.model,
    plan: getCachedAllternitPlan(),
    providers: allowedProviders(cfg, providers),
  })
}

/**
 * The default CLI brain for worker fan-out (`gizzi swarm`, `gizzi mission`):
 * the user's current default brain when it is a CLI brain (workers follow the
 * user's pick), else the resolver's CLI rule (lowest `defaultRank`, signed in
 * first, sign-in unknown second, never signed out). Null when no CLI is usable.
 */
export async function defaultCliBrain(): Promise<string | null> {
  let cfg: BrainConfig = {}
  try {
    cfg = await Config.get()
  } catch {
    cfg = await Config.getGlobal().catch(() => ({}))
  }
  const providers = allowedProviders(cfg, await discovered())
  const current = await currentDefaultBrain().catch(() => null)
  if (current?.model) {
    const providerID = current.model.split("/")[0]
    const cli = providers.find((p) => p.id === providerID && p.source === "subprocess")
    if (cli && cli.signedIn !== false) return current.model
  }
  return pickCli(providers).model
}

export async function applyDefaultBrain(providers?: DiscoveredProvider[]): Promise<string | null> {
  await refreshAllternitPlan()
  const found = await discovered(providers)
  let cfg: BrainConfig = {}
  try {
    cfg = await Config.getGlobal()
  } catch (err) {
    log.warn("default brain: failed to read global config", { error: err })
    return null
  }

  const resolved = resolveDefaultBrain({
    modelAuto: cfg.model_auto,
    currentModel: cfg.model,
    plan: getCachedAllternitPlan(),
    providers: allowedProviders(cfg, found),
  })
  // A pin is the user's decision: never overwritten here.
  if (resolved.pinned) return resolved.model
  if (!resolved.model) {
    log.info("default brain: nothing usable", { reason: resolved.reason })
    return null
  }
  // Persist when the pick changed, or to repair `model_auto: false` with no usable pin.
  if (resolved.model === cfg.model && cfg.model_auto !== false) return resolved.model

  const next = resolved.model
  try {
    await Config.updateGlobal({ model: next, model_auto: true }, { reload: false })
    try {
      const live = await Config.get()
      live.model = next
      live.model_auto = true
    } catch {
      // No instance yet — file write is enough for the next session.
    }
    log.info("default brain set", { model: next, reason: resolved.reason })
  } catch (err) {
    log.warn("default brain: failed to persist", { model: next, error: err })
    return null
  }
  return next
}

export async function pinBrain(model: string): Promise<void> {
  const trimmed = model.trim()
  if (!trimmed.includes("/")) return
  await Config.updateGlobal({ model: trimmed, model_auto: false })
}

export async function unpinBrain(): Promise<void> {
  await Config.updateGlobal({ model_auto: true })
}

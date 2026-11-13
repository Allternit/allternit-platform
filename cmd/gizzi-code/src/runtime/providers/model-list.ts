/**
 * The one model list. `gizzi models`, the TUI picker and every other
 * surface that lists models build their rows here, so they agree on which
 * providers are offered, in what order, which model is each provider's
 * default and which one is running.
 *
 * Sections follow the picker rule in `offer.ts` (Allternit Cloud, your
 * subscriptions, API keys, local). Inside "Your subscriptions" the CLIs
 * with a catalog `defaultRank` come first, best rank first. Models keep
 * the vendor's order (`rank`), never alphabetical.
 */

import { sortBy } from "remeda"
import type { DiscoveredModel, DiscoveredProvider } from "./discovery"
import { Discovery } from "./discovery"
import { SUBPROCESS_PROVIDERS } from "./discovery/subprocess"
import { SelfhostInference } from "./selfhost-inference"
import { ProviderOffer } from "./offer"
import { ModelsDev } from "./adapters/models"
import { currentDefaultBrain, defaultRankOf } from "./default-brain"

// provider.ts is loaded lazily in build(): it pulls in the whole SDK, and
// compose() must stay importable without it. Provider.sort delegates here.

export namespace ModelList {
  // Current model families; later entries rank higher (findIndex, "desc").
  const FAMILY_PRIORITY = ["gpt-6", "claude-sonnet-5", "gemini-3", "kimi"]

  /**
   * The model order `Provider.sort` uses: for a catalog CLI the vendor's
   * flagged default first, then the vendor's order; then current families,
   * `latest`, and id.
   */
  export function sortModels<T extends { id: string; providerID?: string }>(models: T[]): T[] {
    const catalog = new Map(
      (SUBPROCESS_PROVIDERS.find((s) => s.id === models[0]?.providerID)?.models ?? []).map((m) => [m.id, m]),
    )
    return sortBy(
      models,
      [(model) => (catalog.get(model.id)?.default ? 0 : 1), "asc"],
      [(model) => catalog.get(model.id)?.rank ?? Number.MAX_SAFE_INTEGER, "asc"],
      [(model) => FAMILY_PRIORITY.findIndex((filter) => model.id.includes(filter)), "desc"],
      [(model) => (model.id.includes("latest") ? 0 : 1), "asc"],
      [(model) => model.id, "desc"],
    )
  }

  export type Row = {
    /** `provider/model` */
    ref: string
    providerID: string
    modelID: string
    name: string
    /** The provider's flagged default model (catalog or Allternit Cloud flag). */
    default: boolean
    /** The resolved default brain (`currentDefaultBrain`). */
    current: boolean
    /** Position in the vendor's order, 0 = first. */
    rank?: number
    context?: number
    output?: number
    /** A retired alias shown only because it is running (or with `all`). */
    hidden?: boolean
  }

  export type ProviderRows = {
    id: string
    name: string
    /** Subprocess CLIs: sign-in check result (unset = not checked). */
    signedIn?: boolean
    models: Row[]
  }

  export type Section = {
    kind: ProviderOffer.Kind
    title: string
    providers: ProviderRows[]
  }

  /** The fields of a `Provider.Info` the list reads (so tests need no Instance). */
  export type ConfiguredProvider = {
    id: string
    name: string
    source?: string
    env?: string[]
    key?: string
    token?: string
    auth_type?: string
    options?: Record<string, unknown>
    models: Record<string, { id: string; providerID?: string; name: string; limit?: { context?: number; output?: number } }>
  }

  export type ComposeInput = {
    discovered: DiscoveredProvider[]
    configured: Record<string, ConfiguredProvider>
    /** Override the "connected" rule (default: `ModelList.connected`). */
    connected?: (providerID: string) => boolean
    /** The resolved default brain, `provider/model`. */
    currentRef?: string | null
    /** Every provider and model, aggregators and hidden aliases included. */
    all?: boolean
  }

  let _last: Section[] = []

  /** The last `build()` result (sync), `[]` before the first build. */
  export function last(): Section[] {
    return _last
  }

  /**
   * A key or sign-in is on file: discovered on this machine, an env var,
   * `gizzi connect`, or a key in gizzi.json.
   */
  export function connected(
    providerID: string,
    discovered: Record<string, DiscoveredProvider> | DiscoveredProvider[],
    configured: Record<string, ConfiguredProvider>,
    env: NodeJS.ProcessEnv = process.env,
  ): boolean {
    const found = Array.isArray(discovered) ? discovered.some((p) => p.id === providerID) : Boolean(discovered[providerID])
    if (found) return true
    const info = configured[providerID]
    if (!info) return false
    return (
      info.source === "env" ||
      info.source === "api" ||
      Boolean(info.key || info.token || info.options?.["apiKey"]) ||
      (info.env ?? []).some((name) => Boolean(env[name]))
    )
  }

  function catalogModel(providerID: string, modelID: string): DiscoveredModel | undefined {
    return SUBPROCESS_PROVIDERS.find((s) => s.id === providerID)?.models.find((m) => m.id === modelID)
  }

  /**
   * `default` / `rank` / `hidden` for one model: the discovered model's
   * flags (Allternit Cloud's server flag, live CLI lists), else the shared
   * CLI catalog. Shared with `GET /provider`.
   */
  export function modelFlags(
    providerID: string,
    modelID: string,
    discovered?: DiscoveredProvider,
  ): { default: boolean; rank?: number; hidden?: boolean } {
    const index = discovered ? discovered.models.findIndex((m) => m.id === modelID) : -1
    const found = index >= 0 ? discovered!.models[index] : undefined
    const catalog = catalogModel(providerID, modelID)
    // When discovery flagged a default, it is the only source of the flag.
    const discoveryFlags = discovered?.models.some((m) => m.default) ?? false
    const isDefault = discoveryFlags ? Boolean(found?.default) : Boolean(found?.default ?? catalog?.default)
    const rank = found?.rank ?? catalog?.rank ?? (index >= 0 ? index : undefined)
    return { default: isDefault, rank, ...(found?.hidden ? { hidden: true } : {}) }
  }

  const EMBEDDING = /embed/i

  function isEmbedding(id: string, name: string): boolean {
    return EMBEDDING.test(id) || EMBEDDING.test(name)
  }

  type Draft = { id: string; name: string; context?: number; output?: number; sortIndex: number }

  function providerRows(
    id: string,
    dp: DiscoveredProvider | undefined,
    info: ConfiguredProvider | undefined,
    currentRef: string | null | undefined,
    all: boolean,
  ): Row[] {
    const drafts: Draft[] = []
    // Subprocess CLIs (and providers only discovery knows) list the discovered models.
    if (dp && (dp.source === "subprocess" || !info)) {
      dp.models.forEach((m, i) => drafts.push({ id: m.id, name: m.name, context: m.context, output: m.output, sortIndex: i }))
    } else if (info) {
      const sorted = sortModels(Object.values(info.models).map((m) => ({ ...m, providerID: m.providerID ?? id })))
      sorted.forEach((m, i) => {
        drafts.push({ id: m.id, name: m.name, context: m.limit?.context, output: m.limit?.output, sortIndex: i })
      })
    }

    const rows: Array<{ row: Row; sortIndex: number }> = []
    for (const draft of drafts) {
      if (isEmbedding(draft.id, draft.name)) continue
      const ref = `${id}/${draft.id}`
      const flags = modelFlags(id, draft.id, dp)
      const current = Boolean(currentRef) && ref === currentRef
      if (flags.hidden && !current && !all) continue
      const row: Row = { ref, providerID: id, modelID: draft.id, name: draft.name, default: flags.default, current }
      if (flags.rank !== undefined) row.rank = flags.rank
      if (draft.context !== undefined) row.context = draft.context
      if (draft.output !== undefined) row.output = draft.output
      if (flags.hidden) row.hidden = true
      rows.push({ row, sortIndex: draft.sortIndex })
    }
    // Vendor order (`rank`) first; models without one keep Provider.sort order after them.
    rows.sort((a, b) => {
      const ra = a.row.rank ?? Number.POSITIVE_INFINITY
      const rb = b.row.rank ?? Number.POSITIVE_INFINITY
      if (ra !== rb) return ra - rb
      return a.sortIndex - b.sortIndex
    })
    return rows.map((x) => x.row)
  }

  /** Pure row builder: sources in, sections out. */
  export function compose(input: ComposeInput): Section[] {
    const all = input.all === true
    const discoveredMap: Record<string, DiscoveredProvider> = {}
    for (const dp of input.discovered) if (!discoveredMap[dp.id]) discoveredMap[dp.id] = dp
    const isConnected =
      input.connected ?? ((providerID: string) => connected(providerID, discoveredMap, input.configured))

    const ids = new Set<string>([...input.discovered.map((p) => p.id), ...Object.keys(input.configured)])
    const byKind = new Map<ProviderOffer.Kind, Array<ProviderRows & { defaultRank?: number }>>()

    for (const id of ids) {
      const dp = discoveredMap[id]
      const info = input.configured[id]
      const candidate: ProviderOffer.Candidate = {
        id,
        source: dp?.source ?? info?.source,
        auth_type: dp?.auth_type ?? info?.auth_type,
        baseURL: dp?.base_url ?? (info?.options?.["baseURL"] as string | undefined),
        options: dp?.options ?? info?.options,
      }
      if (!all && !ProviderOffer.isOffered(candidate, { connected: isConnected(id) })) continue

      const models = providerRows(id, dp, info, input.currentRef, all)
      if (models.length === 0) continue

      const kind = ProviderOffer.kind(candidate)
      const name = (dp?.source === "subprocess" ? dp.name : info?.name) ?? dp?.name ?? id
      const entry: ProviderRows & { defaultRank?: number } = {
        id,
        name,
        ...(dp?.signedIn !== undefined ? { signedIn: dp.signedIn } : {}),
        models,
      }
      if (kind === "subscription") entry.defaultRank = defaultRankOf({ id, defaultRank: dp?.defaultRank })
      const list = byKind.get(kind) ?? []
      list.push(entry)
      byKind.set(kind, list)
    }

    const sections: Section[] = []
    for (const kind of ProviderOffer.ORDER) {
      const list = byKind.get(kind)
      if (!list?.length) continue
      list.sort((a, b) => {
        if (kind === "subscription") {
          const ra = a.defaultRank ?? Number.POSITIVE_INFINITY
          const rb = b.defaultRank ?? Number.POSITIVE_INFINITY
          if (ra !== rb) return ra - rb
        }
        return a.name.localeCompare(b.name)
      })
      sections.push({
        kind,
        title: ProviderOffer.TITLE[kind],
        providers: list.map(({ defaultRank: _rank, ...rest }) => rest),
      })
    }
    return sections
  }

  /**
   * Build the list from this machine's sources: discovery plus the
   * configured providers (needs an Instance for those; without one the
   * list is discovery only). `all` adds every models.dev provider.
   */
  export async function build(opts: { all?: boolean } = {}): Promise<Section[]> {
    // Self-hosted inference: only the server's model_map, no local CLI/provider discovery.
    const discovered = SelfhostInference.enabled()
      ? []
      : Discovery.last().length > 0
        ? Discovery.last()
        : await Discovery.run()
    const { Provider } = await import("./provider")
    let configured: Record<string, ConfiguredProvider> = await Promise.resolve()
      .then(() => Provider.list())
      .catch(() => ({}))
    if (opts.all) {
      const dev = await ModelsDev.get().catch(() => ({}) as Record<string, ModelsDev.Provider>)
      const fromDev: Record<string, ConfiguredProvider> = {}
      for (const [id, provider] of Object.entries(dev)) fromDev[id] = Provider.fromModelsDevProvider(provider)
      configured = { ...fromDev, ...configured }
    }
    const brain = await currentDefaultBrain().catch(() => null)
    _last = compose({ discovered, configured, currentRef: brain?.model ?? null, all: opts.all })
    return _last
  }
}
